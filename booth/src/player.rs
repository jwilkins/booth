//! Auditioning: one deck, enough to hear what is being prepped.
//!
//! This is not a performance player and is not trying to become one. It exists
//! because setting a cue by looking at a waveform is guesswork — the ear is the
//! instrument that says whether a cue is on the right beat — and because a
//! crate cannot be dug through in silence.
//!
//! The mixing is [`fill`], which is a pure function of a buffer, a sound and a
//! position. Everything about it that could be wrong — the resampling, the
//! channel mapping, running off the end — is tested without opening a device,
//! because a test that needs a sound card is a test that does not run.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use booth_cli::audio::Audio;

/// Decoded audio, interleaved, ready to be fed to a device.
///
/// Interleaved rather than planar because that is what an output buffer wants,
/// and the conversion is done once here rather than per callback.
pub struct Sound {
    pub samples: Vec<f32>,
    pub channels: usize,
    pub rate: u32,
}

impl Sound {
    pub fn from_audio(audio: &Audio) -> Self {
        let channels = audio.channels();
        let frames = audio.frames();
        let mut samples = Vec::with_capacity(frames * channels);
        for frame in 0..frames {
            for plane in &audio.planes {
                samples.push(plane[frame]);
            }
        }
        Self { samples, channels, rate: audio.sample_rate }
    }

    pub fn frames(&self) -> usize {
        match self.channels {
            0 => 0,
            channels => self.samples.len() / channels,
        }
    }

    pub fn duration_secs(&self) -> f64 {
        match self.rate {
            0 => 0.0,
            rate => self.frames() as f64 / rate as f64,
        }
    }
}

/// Fill an output buffer from a sound, and say where playback got to.
///
/// `position` and the return value are in source frames, fractional because the
/// device's rate is rarely the file's: `step` is how far to advance per output
/// frame. Between frames it interpolates linearly, which is not a resampler
/// anyone would master through and is inaudible for auditioning.
///
/// Anything past the end is silence, and the position stops at the end rather
/// than running away — a player that keeps counting after a track finishes
/// reports a position that means nothing.
pub fn fill(
    out: &mut [f32],
    out_channels: usize,
    sound: &Sound,
    position: f64,
    step: f64,
    gain: f32,
) -> f64 {
    out.fill(0.0);
    if out_channels == 0 || sound.channels == 0 || sound.frames() == 0 {
        return position;
    }

    let frames = sound.frames();
    let mut at = position;
    for frame in out.chunks_mut(out_channels) {
        if at >= frames as f64 {
            at = frames as f64;
            break;
        }
        let index = at.floor() as usize;
        let fraction = (at - index as f64) as f32;

        for (channel, sample) in frame.iter_mut().enumerate() {
            // A mono file plays out of both; anything wider than the device is
            // taken from its first channels rather than folded down, because a
            // fold-down of a track that is not stereo is a guess about what it
            // is.
            let source = channel.min(sound.channels - 1);
            let here = sound.samples[index * sound.channels + source];
            let next = match index + 1 < frames {
                true => sound.samples[(index + 1) * sound.channels + source],
                false => here,
            };
            *sample = (here + (next - here) * fraction) * gain;
        }
        at += step;
    }
    at.min(frames as f64)
}

/// What the window tells the audio callback.
enum Command {
    Load(Arc<Sound>),
    /// Jump to a source frame.
    Seek(f64),
}

/// The handful of numbers both threads read.
struct Shared {
    playing: AtomicBool,
    /// The position, in source frames, as `f64` bits.
    position: AtomicU64,
    /// The gain, as `f32` bits.
    gain: AtomicU32,
    /// Set when playback reached the end, so the window can show a stopped
    /// transport rather than a playing one that is silent.
    ended: AtomicBool,
}

impl Shared {
    fn position(&self) -> f64 {
        f64::from_bits(self.position.load(Ordering::Relaxed))
    }

    fn set_position(&self, frames: f64) {
        self.position.store(frames.to_bits(), Ordering::Relaxed);
    }
}

/// One deck.
pub struct Player {
    shared: Arc<Shared>,
    commands: Sender<Command>,
    /// Held because dropping it closes the device.
    _stream: cpal::Stream,
    /// The device's rate, which decides the resampling step.
    out_rate: u32,
    /// Which track is loaded, and how long it is.
    loaded: Option<u32>,
    sound_rate: u32,
    duration_secs: f64,
}

impl Player {
    /// Open the default output device.
    ///
    /// Fails rather than panicking when there is no device — over SSH, in CI,
    /// on a machine with no sound card — because a library tool that will not
    /// start without speakers is worse than one that cannot audition.
    pub fn open() -> Result<Self> {
        let host = cpal::default_host();
        let device = host.default_output_device().context("no audio output device")?;
        let config = device.default_output_config().context("no usable output config")?;
        let out_rate = config.sample_rate();
        let out_channels = config.channels() as usize;

        let shared = Arc::new(Shared {
            playing: AtomicBool::new(false),
            position: AtomicU64::new(0.0f64.to_bits()),
            gain: AtomicU32::new(0.8f32.to_bits()),
            ended: AtomicBool::new(false),
        });
        let (commands, inbox) = channel();

        let stream = build(&device, &config, Arc::clone(&shared), inbox, out_channels)?;
        stream.play().context("starting the audio stream")?;

        Ok(Self {
            shared,
            commands,
            _stream: stream,
            out_rate,
            loaded: None,
            sound_rate: out_rate,
            duration_secs: 0.0,
        })
    }

    /// Hand it a decoded track, and start at the beginning.
    pub fn load(&mut self, id: u32, sound: Arc<Sound>) {
        self.sound_rate = sound.rate;
        self.duration_secs = sound.duration_secs();
        self.loaded = Some(id);
        self.shared.set_position(0.0);
        self.shared.ended.store(false, Ordering::Relaxed);
        let _ = self.commands.send(Command::Load(sound));
    }

    /// Which track is loaded, if any.
    pub fn loaded(&self) -> Option<u32> {
        self.loaded
    }

    pub fn is_playing(&self) -> bool {
        self.shared.playing.load(Ordering::Relaxed) && !self.shared.ended.load(Ordering::Relaxed)
    }

    pub fn play(&self) {
        if self.loaded.is_none() {
            return;
        }
        // Playing after the end starts again, rather than doing nothing and
        // looking broken.
        if self.shared.ended.load(Ordering::Relaxed) {
            self.seek_secs(0.0);
        }
        self.shared.playing.store(true, Ordering::Relaxed);
    }

    pub fn pause(&self) {
        self.shared.playing.store(false, Ordering::Relaxed);
    }

    pub fn toggle(&self) {
        match self.is_playing() {
            true => self.pause(),
            false => self.play(),
        }
    }

    /// Where playback is, in seconds.
    pub fn position_secs(&self) -> f64 {
        match self.sound_rate {
            0 => 0.0,
            rate => self.shared.position() / rate as f64,
        }
    }

    pub fn duration_secs(&self) -> f64 {
        self.duration_secs
    }

    pub fn seek_secs(&self, secs: f64) {
        let frames = (secs.max(0.0) * self.sound_rate as f64).min(self.frames());
        self.shared.set_position(frames);
        self.shared.ended.store(false, Ordering::Relaxed);
        let _ = self.commands.send(Command::Seek(frames));
    }

    fn frames(&self) -> f64 {
        self.duration_secs * self.sound_rate as f64
    }

    pub fn gain(&self) -> f32 {
        f32::from_bits(self.shared.gain.load(Ordering::Relaxed))
    }

    pub fn set_gain(&self, gain: f32) {
        self.shared.gain.store(gain.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }

    /// The device's rate, for reporting.
    pub fn out_rate(&self) -> u32 {
        self.out_rate
    }
}

/// Build the output stream for whatever sample format the device wants.
fn build(
    device: &cpal::Device,
    config: &cpal::SupportedStreamConfig,
    shared: Arc<Shared>,
    inbox: Receiver<Command>,
    out_channels: usize,
) -> Result<cpal::Stream> {
    let out_rate = config.sample_rate();
    let stream_config: cpal::StreamConfig = config.config();
    let on_error = |e| eprintln!("audio stream error: {e}");

    // The callback's own state: nothing here is shared, so nothing here needs a
    // lock. New tracks arrive through the channel instead, which is what keeps
    // the audio thread from ever waiting on the window.
    let mut current: Option<Arc<Sound>> = None;

    let stream = match config.sample_format() {
        cpal::SampleFormat::F32 => device.build_output_stream(
            stream_config,
            move |out: &mut [f32], _: &cpal::OutputCallbackInfo| {
                while let Ok(command) = inbox.try_recv() {
                    match command {
                        Command::Load(sound) => current = Some(sound),
                        Command::Seek(frames) => shared.set_position(frames),
                    }
                }

                let Some(sound) = &current else {
                    out.fill(0.0);
                    return;
                };
                if !shared.playing.load(Ordering::Relaxed) {
                    out.fill(0.0);
                    return;
                }

                let step = sound.rate as f64 / out_rate as f64;
                let gain = f32::from_bits(shared.gain.load(Ordering::Relaxed));
                let was = shared.position();
                let now = fill(out, out_channels, sound, was, step, gain);
                shared.set_position(now);
                if now >= sound.frames() as f64 {
                    shared.ended.store(true, Ordering::Relaxed);
                }
            },
            on_error,
            None,
        ),
        // Anything else is asked for as f32 anyway: every host cpal supports
        // will give one, and carrying three copies of the callback for formats
        // that do not turn up is worse than saying so.
        other => anyhow::bail!("this device wants {other:?} samples, which is not supported yet"),
    }
    .context("building the audio stream")?;

    Ok(stream)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A sound whose samples say which frame and channel they are, so a
    /// mis-mapping shows up as a number rather than as a wrong noise.
    fn ramp(frames: usize, channels: usize, rate: u32) -> Sound {
        let mut samples = Vec::new();
        for frame in 0..frames {
            for channel in 0..channels {
                samples.push(frame as f32 + channel as f32 / 10.0);
            }
        }
        Sound { samples, channels, rate }
    }

    #[test]
    fn a_sound_is_interleaved_from_the_planes() {
        let audio = Audio::new(44_100, vec![vec![1.0, 2.0, 3.0], vec![-1.0, -2.0, -3.0]]).unwrap();
        let sound = Sound::from_audio(&audio);
        assert_eq!(sound.samples, vec![1.0, -1.0, 2.0, -2.0, 3.0, -3.0]);
        assert_eq!(sound.channels, 2);
        assert_eq!(sound.frames(), 3);
    }

    #[test]
    fn at_matching_rates_the_samples_come_out_as_they_went_in() {
        let sound = ramp(4, 2, 44_100);
        let mut out = vec![0.0; 8];
        let end = fill(&mut out, 2, &sound, 0.0, 1.0, 1.0);

        assert_eq!(out, vec![0.0, 0.1, 1.0, 1.1, 2.0, 2.1, 3.0, 3.1]);
        assert_eq!(end, 4.0);
    }

    #[test]
    fn a_mono_file_plays_out_of_both_speakers() {
        let sound = ramp(3, 1, 44_100);
        let mut out = vec![0.0; 6];
        fill(&mut out, 2, &sound, 0.0, 1.0, 1.0);
        assert_eq!(out, vec![0.0, 0.0, 1.0, 1.0, 2.0, 2.0]);
    }

    #[test]
    fn a_device_with_one_channel_takes_the_first() {
        let sound = ramp(3, 2, 44_100);
        let mut out = vec![0.0; 3];
        fill(&mut out, 1, &sound, 0.0, 1.0, 1.0);
        assert_eq!(out, vec![0.0, 1.0, 2.0]);
    }

    #[test]
    fn a_44_1k_file_on_a_48k_device_is_resampled() {
        // Half the step means each source frame lasts two output frames, with
        // the value between them interpolated.
        let sound = ramp(4, 1, 24_000);
        let mut out = vec![0.0; 4];
        let end = fill(&mut out, 1, &sound, 0.0, 0.5, 1.0);

        assert_eq!(out, vec![0.0, 0.5, 1.0, 1.5]);
        assert_eq!(end, 2.0, "four output frames at half speed is two source frames");
    }

    #[test]
    fn playback_stops_at_the_end_rather_than_running_away() {
        let sound = ramp(3, 1, 44_100);
        let mut out = vec![9.9; 6];
        let end = fill(&mut out, 1, &sound, 0.0, 1.0, 1.0);

        assert_eq!(end, 3.0, "the position must stop at the end of the track");
        assert_eq!(&out[..3], &[0.0, 1.0, 2.0]);
        assert_eq!(&out[3..], &[0.0, 0.0, 0.0], "past the end is silence, not the last sample");
    }

    #[test]
    fn starting_past_the_end_is_silence() {
        let sound = ramp(3, 1, 44_100);
        let mut out = vec![9.9; 4];
        let end = fill(&mut out, 1, &sound, 99.0, 1.0, 1.0);
        assert_eq!(out, vec![0.0; 4]);
        assert_eq!(end, 3.0);
    }

    #[test]
    fn seeking_into_the_middle_plays_from_there() {
        let sound = ramp(8, 1, 44_100);
        let mut out = vec![0.0; 3];
        let end = fill(&mut out, 1, &sound, 4.0, 1.0, 1.0);
        assert_eq!(out, vec![4.0, 5.0, 6.0]);
        assert_eq!(end, 7.0);
    }

    #[test]
    fn the_gain_is_applied_to_every_sample() {
        let sound = ramp(3, 2, 44_100);
        let mut out = vec![0.0; 6];
        fill(&mut out, 2, &sound, 0.0, 1.0, 0.5);
        assert_eq!(out, vec![0.0, 0.05, 0.5, 0.55, 1.0, 1.05]);
    }

    #[test]
    fn an_empty_sound_plays_silence_rather_than_panicking() {
        let sound = Sound { samples: Vec::new(), channels: 2, rate: 44_100 };
        let mut out = vec![9.9; 4];
        assert_eq!(fill(&mut out, 2, &sound, 0.0, 1.0, 1.0), 0.0);
        assert_eq!(out, vec![0.0; 4]);

        // And a sound that claims no channels at all, which no decoder should
        // produce but which must not be an index out of bounds if one does.
        let broken = Sound { samples: vec![1.0], channels: 0, rate: 44_100 };
        let mut out = vec![9.9; 4];
        assert_eq!(fill(&mut out, 2, &broken, 0.0, 1.0, 1.0), 0.0);
        assert_eq!(out, vec![0.0; 4]);
    }

    #[test]
    fn a_position_survives_the_trip_through_the_atomic() {
        let shared = Shared {
            playing: AtomicBool::new(false),
            position: AtomicU64::new(0.0f64.to_bits()),
            gain: AtomicU32::new(1.0f32.to_bits()),
            ended: AtomicBool::new(false),
        };
        shared.set_position(123_456.75);
        assert_eq!(shared.position(), 123_456.75);
    }
}
