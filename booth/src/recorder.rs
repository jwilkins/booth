//! Recording a set from an input: a mixer's USB send, or an interface.
//!
//! The file is `booth_cli::audio::record`, which is tested without a sound
//! card. What is here is the half that needs one: finding the input, taking
//! the two channels that carry the master out of however many the device
//! sends, and getting them to the file without the audio thread ever waiting
//! on a disk.
//!
//! A DJM's USB send is several stereo pairs, and which pair carries the master
//! is the mixer's setting, not this program's: the pair is chosen in Settings
//! rather than guessed. No DJM has been recorded from here yet.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, RecvTimeoutError, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use booth_cli::audio::record::{Recorded, Recorder};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample};

use crate::config::Recording;

/// How many blocks can wait for the disk before the next one is dropped:
/// a callback is ten or twenty milliseconds, so this is seconds of a stall.
const WAITING: usize = 512;

/// An input, as the settings offer it.
#[derive(Clone, Debug, PartialEq)]
pub struct Input {
    /// What Settings keeps: cpal's identifier, as text.
    pub id: String,
    pub name: String,
    pub channels: u16,
    pub rate: u32,
}

/// Every input there is, the default first.
///
/// Asked when the settings open rather than every frame: listing devices takes
/// a moment, and on Linux ALSA has something to say on stderr about every
/// plugin it tries.
pub fn inputs() -> Vec<Input> {
    let host = cpal::default_host();
    let default = host.default_input_device().and_then(|device| device.id().ok());
    let default = default.map(|id| id.to_string());
    let Ok(devices) = host.input_devices() else { return Vec::new() };
    let mut found: Vec<Input> = devices
        .filter_map(|device| {
            let config = device.default_input_config().ok()?;
            Some(Input {
                id: device.id().ok()?.to_string(),
                name: device.to_string(),
                channels: config.channels(),
                rate: config.sample_rate(),
            })
        })
        .collect();
    found.sort_by_key(|input| Some(&input.id) != default.as_ref());
    found
}

/// The channels taken from each frame: the pair starting at `first`, or the
/// one a mono device has.
fn chosen(device_channels: usize, first: usize) -> Result<Vec<usize>> {
    match device_channels {
        0 => bail!("the input sends no channels"),
        1 => Ok(vec![0]),
        n if first + 1 < n => Ok(vec![first, first + 1]),
        n => bail!(
            "the input sends {n} channels, so there is no {}\u{2013}{} to record: choose another \
             pair in Settings",
            first + 1,
            first + 2
        ),
    }
}

/// Take `take` out of interleaved frames of `device_channels`, as floats, and
/// raise `peaks` to the loudest sample of each.
fn pick<T>(data: &[T], device_channels: usize, take: &[usize], peaks: &mut [f32; 2]) -> Vec<f32>
where
    T: Sample,
    f32: FromSample<T>,
{
    let mut block = Vec::with_capacity(data.len() / device_channels.max(1) * take.len());
    for frame in data.chunks_exact(device_channels) {
        for (slot, &channel) in take.iter().enumerate() {
            let sample: f32 = frame[channel].to_sample();
            peaks[slot] = peaks[slot].max(sample.abs());
            block.push(sample);
        }
    }
    block
}

/// What the audio thread, the writer and the window share.
#[derive(Default)]
struct Shared {
    /// The loudest sample of each channel since the window last looked, as
    /// `f32` bits: a non-negative float's bits order the same as the float.
    peaks: [AtomicU32; 2],
    /// Frames on the disk.
    frames: AtomicU64,
    /// Frames the disk could not keep up with, dropped rather than making the
    /// audio thread wait.
    dropped: AtomicU64,
    stopping: AtomicBool,
    failed: Mutex<Option<String>>,
}

/// A recording under way.
pub struct Capture {
    stream: Option<cpal::Stream>,
    writer: Option<JoinHandle<Result<Recorded>>>,
    shared: Arc<Shared>,
    pub rate: u32,
    pub channels: u16,
    pub device: String,
    /// The file it is going into.
    pub file: PathBuf,
    /// Held because a laptop that sleeps mid-set stops the recording.
    _awake: booth_cli::awake::Awake,
    pub awake: bool,
}

impl Capture {
    /// Start recording the chosen input into a file called `name`.
    pub fn start(settings: &Recording, name: &str) -> Result<Self> {
        let host = cpal::default_host();
        let device = match &settings.device {
            Some(wanted) => host
                .input_devices()
                .context("listing the inputs")?
                .find(|device| device.id().is_ok_and(|id| id.to_string() == *wanted))
                .with_context(|| {
                    format!(
                        "{} is not connected",
                        settings.device_name.as_deref().unwrap_or(wanted.as_str())
                    )
                })?,
            None => host.default_input_device().context("there is no audio input")?,
        };
        let config = device.default_input_config().context("the input offers no format")?;
        let rate = config.sample_rate();
        let device_channels = config.channels() as usize;
        let take = chosen(device_channels, settings.first_channel as usize)?;
        let channels = take.len() as u16;

        let recorder = Recorder::start(&settings.folder, name, rate, channels)?;
        let file = recorder.parts()[0].clone();
        let shared = Arc::new(Shared::default());
        let (blocks, waiting) = sync_channel::<Vec<f32>>(WAITING);

        // The writer: everything the callback sends, onto the disk, until it
        // is told to stop and has written what was still waiting.
        let writer = {
            let shared = Arc::clone(&shared);
            let mut recorder = recorder;
            std::thread::Builder::new()
                .name("recorder".into())
                .spawn(move || {
                    loop {
                        let block = match waiting.recv_timeout(Duration::from_millis(100)) {
                            Ok(block) => block,
                            Err(RecvTimeoutError::Timeout) => {
                                match shared.stopping.load(Ordering::Relaxed) {
                                    true => break,
                                    false => continue,
                                }
                            }
                            Err(RecvTimeoutError::Disconnected) => break,
                        };
                        if let Err(e) = recorder.write(&block) {
                            // Stopped here, and the file kept as it stands: up
                            // to the last checkpoint it opens.
                            *shared.failed.lock().unwrap() = Some(format!("{e:#}"));
                            break;
                        }
                        shared.frames.store(recorder.frames(), Ordering::Relaxed);
                    }
                    recorder.finish()
                })
                .context("starting the recorder")?
        };

        let stream_config: cpal::StreamConfig = config.config();
        let stream = {
            let shared = Arc::clone(&shared);
            let take = take.clone();
            let on_error = {
                let shared = Arc::clone(&shared);
                move |e: cpal::Error| {
                    shared.failed.lock().unwrap().get_or_insert(format!("the input stopped: {e}"));
                }
            };
            macro_rules! build {
                ($t:ty) => {
                    device.build_input_stream::<$t, _, _>(
                        stream_config,
                        move |data: &[$t], _: &cpal::InputCallbackInfo| {
                            if shared.stopping.load(Ordering::Relaxed) {
                                return;
                            }
                            let mut peaks = [0.0f32; 2];
                            let block = pick(data, device_channels, &take, &mut peaks);
                            for (slot, peak) in shared.peaks.iter().zip(peaks) {
                                slot.fetch_max(peak.to_bits(), Ordering::Relaxed);
                            }
                            let frames = (block.len() / take.len()) as u64;
                            if let Err(TrySendError::Full(_)) = blocks.try_send(block) {
                                shared.dropped.fetch_add(frames, Ordering::Relaxed);
                            }
                        },
                        on_error,
                        None,
                    )
                };
            }
            match config.sample_format() {
                cpal::SampleFormat::F32 => build!(f32),
                cpal::SampleFormat::I16 => build!(i16),
                cpal::SampleFormat::I24 => build!(cpal::I24),
                cpal::SampleFormat::I32 => build!(i32),
                other => bail!("the input sends {other:?} samples, which are not recorded yet"),
            }
            .context("opening the input")?
        };
        stream.play().context("starting the input")?;
        let (awake, _) = booth_cli::awake::Awake::hold("recording a set");
        let held = awake.holding();

        Ok(Self {
            stream: Some(stream),
            writer: Some(writer),
            shared,
            rate,
            channels,
            device: device.to_string(),
            file,
            _awake: awake,
            awake: held,
        })
    }

    /// The loudest each channel has been since this was last asked, at full
    /// scale 1.
    pub fn levels(&self) -> [f32; 2] {
        self.shared.peaks.each_ref().map(|peak| f32::from_bits(peak.swap(0, Ordering::Relaxed)))
    }

    /// How much is on the disk.
    pub fn seconds(&self) -> f64 {
        self.shared.frames.load(Ordering::Relaxed) as f64 / self.rate as f64
    }

    /// How much the disk could not keep up with, in seconds.
    pub fn dropped_seconds(&self) -> f64 {
        self.shared.dropped.load(Ordering::Relaxed) as f64 / self.rate as f64
    }

    /// Why the recording stopped on its own, if it did.
    pub fn failed(&self) -> Option<String> {
        self.shared.failed.lock().unwrap().clone()
    }

    /// Stop, and say what was made.
    pub fn stop(mut self) -> Result<Recorded> {
        self.finish()
    }

    fn finish(&mut self) -> Result<Recorded> {
        self.shared.stopping.store(true, Ordering::Relaxed);
        drop(self.stream.take());
        match self.writer.take() {
            Some(writer) => {
                writer.join().map_err(|_| anyhow::anyhow!("the recorder stopped with a panic"))?
            }
            None => bail!("the recording was already stopped"),
        }
    }
}

/// Closing the window mid-set still closes the file properly.
impl Drop for Capture {
    fn drop(&mut self) {
        if self.writer.is_some() {
            let _ = self.finish();
        }
    }
}

/// `1:02:03`, or `2:03` under an hour.
pub fn clock(seconds: f64) -> String {
    let total = seconds.max(0.0) as u64;
    let (hours, minutes, seconds) = (total / 3_600, total / 60 % 60, total % 60);
    match hours {
        0 => format!("{minutes}:{seconds:02}"),
        _ => format!("{hours}:{minutes:02}:{seconds:02}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pair_chosen_is_taken_out_of_every_frame_the_device_sends() {
        // Six channels, as a mixer's USB send might be, with the master on
        // 3–4: the samples are numbered so a wrong channel shows.
        let data: Vec<f32> = (0..12).map(|i| i as f32 / 100.0).collect();
        let mut peaks = [0.0; 2];
        let block = pick(&data, 6, &chosen(6, 2).unwrap(), &mut peaks);
        assert_eq!(block, [0.02, 0.03, 0.08, 0.09]);
        assert_eq!(peaks, [0.08, 0.09]);
    }

    #[test]
    fn integer_samples_come_out_at_the_same_scale_as_float_ones() {
        let data: [i16; 4] = [i16::MAX, i16::MIN, 16_384, -16_384];
        let mut peaks = [0.0; 2];
        let block = pick(&data, 2, &[0, 1], &mut peaks);
        assert!((block[0] - 1.0).abs() < 1e-4, "{block:?}");
        assert_eq!(block[1], -1.0);
        assert_eq!(block[2], 0.5);
        assert_eq!(peaks[1], 1.0);
    }

    #[test]
    fn a_pair_the_device_does_not_have_is_refused_with_how_many_it_has() {
        assert_eq!(chosen(1, 0).unwrap(), [0], "a mono input is recorded mono");
        assert_eq!(chosen(2, 0).unwrap(), [0, 1]);
        let said = chosen(2, 2).unwrap_err().to_string();
        assert!(said.contains("sends 2 channels") && said.contains("3\u{2013}4"), "{said}");
    }

    #[test]
    fn the_time_on_the_button_reads_like_a_clock() {
        assert_eq!(clock(0.0), "0:00");
        assert_eq!(clock(125.9), "2:05");
        assert_eq!(clock(3_723.0), "1:02:03");
    }
}
