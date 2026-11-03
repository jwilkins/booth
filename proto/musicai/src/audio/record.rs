//! Recording a set: what the mixer sends, as it arrives, into a WAV that can
//! be played at every moment of the recording rather than only once it ends.
//!
//! The file side only. Where the samples come from — a mixer's USB send, an
//! interface — is the app's business; this takes interleaved frames and makes
//! sure that whatever happens to the machine halfway through a set, the set up
//! to a couple of seconds ago is on the disk and opens.
//!
//! Two things make that so. The header is rewritten every [`CHECKPOINT_SECS`],
//! so a crash, a pulled power lead or a full disk leaves a file whose header
//! says how long it is. And a file never grows past what a WAV header can
//! describe: the data length is 32 bits, so a recording that would pass
//! [`PART_BYTES`] carries on in a second file, sample-continuous with the
//! first. At 48 kHz, 24-bit stereo that is a part every three hours and
//! fifty-one minutes.

use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

/// How much audio a crash can cost, at most: the header is brought up to date
/// this often.
pub const CHECKPOINT_SECS: u32 = 2;

/// The most audio one file holds. A WAV's sizes are 32-bit, so 4 GiB is the
/// hard limit; this stays under it with room for the header.
pub const PART_BYTES: u64 = 4_000_000_000;

/// Bits a sample is written at: what a mixer's USB send carries, so nothing it
/// sends is lost, and the headroom a set recorded too hot can still be mixed
/// down from.
pub const BITS: u16 = 24;

/// A recording in progress.
pub struct Recorder {
    folder: PathBuf,
    name: String,
    rate: u32,
    channels: u16,
    /// The data bytes a part may hold, a whole number of frames.
    part_bytes: u64,
    writer: Option<hound::WavWriter<BufWriter<File>>>,
    parts: Vec<PathBuf>,
    /// Frames in the part being written, and in all of them.
    part_frames: u64,
    frames: u64,
    /// Frames since the header was last brought up to date.
    unsaved: u64,
    /// Samples that reached full scale, which on a mixer's send means the
    /// master was in the red.
    clipped: u64,
    /// The loudest sample, at full scale 1.
    peak: f32,
}

/// A recording that has finished.
#[derive(Clone, Debug, PartialEq)]
pub struct Recorded {
    /// Every file, in order. More than one only past [`PART_BYTES`].
    pub parts: Vec<PathBuf>,
    pub seconds: f64,
    pub clipped: u64,
    /// The loudest sample, at full scale 1: nothing at all is an input
    /// that was never open, which on a Mac is a refused microphone.
    pub peak: f32,
}

impl Recorder {
    /// Start a recording called `name` in `folder`, which is made if it is not
    /// there. A file already called that is left alone and the new one takes a
    /// number.
    pub fn start(folder: &Path, name: &str, rate: u32, channels: u16) -> Result<Self> {
        Self::with_part_bytes(folder, name, rate, channels, PART_BYTES)
    }

    fn with_part_bytes(
        folder: &Path,
        name: &str,
        rate: u32,
        channels: u16,
        part_bytes: u64,
    ) -> Result<Self> {
        if rate == 0 || channels == 0 {
            bail!("a recording needs a rate and at least one channel");
        }
        std::fs::create_dir_all(folder)
            .with_context(|| format!("making the recordings folder {}", folder.display()))?;
        let frame = channels as u64 * (BITS as u64 / 8);
        let mut recorder = Self {
            folder: folder.to_path_buf(),
            name: name.to_string(),
            rate,
            channels,
            part_bytes: (part_bytes / frame).max(1) * frame,
            writer: None,
            parts: Vec::new(),
            part_frames: 0,
            frames: 0,
            unsaved: 0,
            clipped: 0,
            peak: 0.0,
        };
        recorder.open_part()?;
        Ok(recorder)
    }

    /// The file the next part goes in: the name, then " (part 2)" and on, and
    /// a number after that if something is already there.
    fn next_path(&self) -> PathBuf {
        let base = match self.parts.len() {
            0 => self.name.clone(),
            n => format!("{} (part {})", self.name, n + 1),
        };
        let mut path = self.folder.join(format!("{base}.wav"));
        let mut n = 2;
        while path.exists() {
            path = self.folder.join(format!("{base} ({n}).wav"));
            n += 1;
        }
        path
    }

    fn open_part(&mut self) -> Result<()> {
        if let Some(writer) = self.writer.take() {
            writer.finalize().context("closing a recording's part")?;
        }
        let path = self.next_path();
        let spec = hound::WavSpec {
            channels: self.channels,
            sample_rate: self.rate,
            bits_per_sample: BITS,
            sample_format: hound::SampleFormat::Int,
        };
        let writer = hound::WavWriter::create(&path, spec)
            .with_context(|| format!("starting {}", path.display()))?;
        self.writer = Some(writer);
        self.parts.push(path);
        self.part_frames = 0;
        Ok(())
    }

    /// Add interleaved frames to the recording.
    ///
    /// Samples are floats at full scale ±1, as an input device hands them over.
    /// Anything past full scale is clipped, and counted.
    pub fn write(&mut self, interleaved: &[f32]) -> Result<()> {
        let channels = self.channels as usize;
        if interleaved.len() % channels != 0 {
            bail!(
                "{} samples is not a whole number of {channels}-channel frames",
                interleaved.len()
            );
        }
        let frame_bytes = channels as u64 * (BITS as u64 / 8);
        let full = (1i32 << (BITS - 1)) - 1;
        for frame in interleaved.chunks_exact(channels) {
            if (self.part_frames + 1) * frame_bytes > self.part_bytes {
                self.open_part()?;
            }
            let writer = self.writer.as_mut().expect("a part is always open while recording");
            for &sample in frame {
                if sample.abs() >= 1.0 {
                    self.clipped += 1;
                }
                self.peak = self.peak.max(sample.abs().min(1.0));
                let value = (sample.clamp(-1.0, 1.0) as f64 * full as f64).round() as i32;
                writer.write_sample(value).context("writing the recording")?;
            }
            self.part_frames += 1;
            self.frames += 1;
            self.unsaved += 1;
        }
        if self.unsaved >= CHECKPOINT_SECS as u64 * self.rate as u64 {
            self.checkpoint()?;
        }
        Ok(())
    }

    /// Bring the header up to date, so the file opens at this length if
    /// nothing else is ever written to it.
    pub fn checkpoint(&mut self) -> Result<()> {
        if let Some(writer) = self.writer.as_mut() {
            writer.flush().context("saving the recording so far")?;
        }
        self.unsaved = 0;
        Ok(())
    }

    /// How many frames the recording holds so far, across every part.
    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// How long the recording is so far.
    pub fn seconds(&self) -> f64 {
        self.frames as f64 / self.rate as f64
    }

    pub fn clipped(&self) -> u64 {
        self.clipped
    }

    pub fn parts(&self) -> &[PathBuf] {
        &self.parts
    }

    /// Close the recording and say what it made.
    pub fn finish(mut self) -> Result<Recorded> {
        if let Some(writer) = self.writer.take() {
            writer.finalize().context("closing the recording")?;
        }
        Ok(Recorded {
            parts: std::mem::take(&mut self.parts),
            seconds: self.seconds(),
            clipped: self.clipped,
            peak: self.peak,
        })
    }
}

/// How many bytes an hour of recording takes.
pub fn bytes_per_hour(rate: u32, channels: u16) -> u64 {
    rate as u64 * channels as u64 * (BITS as u64 / 8) * 3_600
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("booth-record-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            Self(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A stereo ramp, a different one in each channel, so a sample out of
    /// place or a channel swapped shows.
    fn ramp(frames: usize, from: usize) -> Vec<f32> {
        (from..from + frames)
            .flat_map(|i| {
                let x = (i % 1000) as f32 / 1000.0 - 0.5;
                [x, -x * 0.5]
            })
            .collect()
    }

    fn read(path: &Path) -> (hound::WavSpec, Vec<i32>) {
        let mut reader = hound::WavReader::open(path).unwrap();
        let spec = reader.spec();
        (spec, reader.samples::<i32>().map(|s| s.unwrap()).collect())
    }

    fn as_int(samples: &[f32]) -> Vec<i32> {
        samples.iter().map(|&s| (s as f64 * 8_388_607.0).round() as i32).collect()
    }

    #[test]
    fn a_recording_is_a_24_bit_wav_of_exactly_what_was_sent() {
        let scratch = Scratch::new("plain");
        let mut recorder = Recorder::start(&scratch.0, "Set", 48_000, 2).unwrap();
        let sent = ramp(48_000, 0);
        for block in sent.chunks(1_024 * 2) {
            recorder.write(block).unwrap();
        }
        assert_eq!(recorder.seconds(), 1.0);
        let done = recorder.finish().unwrap();
        assert_eq!(done.parts, [scratch.0.join("Set.wav")]);
        let (spec, samples) = read(&done.parts[0]);
        assert_eq!((spec.sample_rate, spec.channels, spec.bits_per_sample), (48_000, 2, 24));
        assert_eq!(samples, as_int(&sent));
    }

    #[test]
    fn a_recording_cut_off_without_finishing_opens_to_its_last_checkpoint() {
        // The machine dies mid-set: nothing is finished, nothing is closed.
        // What was written up to the last checkpoint has to open, at the
        // length its header says.
        let scratch = Scratch::new("crash");
        let mut recorder = Recorder::start(&scratch.0, "Set", 8_000, 2).unwrap();
        let sent = ramp(8_000 * 5, 0);
        recorder.write(&sent).unwrap();
        // A second more, which is under the checkpoint and is lost with the
        // machine: the header still says five seconds, and that is what opens.
        recorder.write(&ramp(8_000, 0)).unwrap();
        let path = recorder.parts()[0].clone();
        std::mem::forget(recorder);

        let (_, samples) = read(&path);
        assert_eq!(samples.len(), sent.len(), "five seconds, to the last checkpoint");
        assert_eq!(samples, as_int(&sent));
    }

    #[test]
    fn a_recording_carries_on_in_a_second_file_before_a_wav_cannot_hold_it() {
        let scratch = Scratch::new("parts");
        // Six bytes a frame; a part of 1,000 frames.
        let mut recorder = Recorder::with_part_bytes(&scratch.0, "Set", 8_000, 2, 6_000).unwrap();
        let sent = ramp(2_500, 0);
        for block in sent.chunks(333 * 2) {
            recorder.write(block).unwrap();
        }
        let done = recorder.finish().unwrap();
        assert_eq!(
            done.parts,
            [
                scratch.0.join("Set.wav"),
                scratch.0.join("Set (part 2).wav"),
                scratch.0.join("Set (part 3).wav"),
            ]
        );
        let joined: Vec<i32> = done.parts.iter().flat_map(|part| read(part).1).collect();
        assert_eq!(joined, as_int(&sent), "the parts are the recording, end to end");
        assert_eq!(read(&done.parts[0]).1.len(), 2_000);
    }

    #[test]
    fn a_hot_master_is_clipped_and_counted_and_an_existing_file_is_kept() {
        let scratch = Scratch::new("hot");
        std::fs::create_dir_all(&scratch.0).unwrap();
        std::fs::write(scratch.0.join("Set.wav"), b"last week's").unwrap();
        let mut recorder = Recorder::start(&scratch.0, "Set", 8_000, 2).unwrap();
        recorder.write(&[1.2, -0.5, 0.25, -1.0]).unwrap();
        assert_eq!(recorder.clipped(), 2);
        let done = recorder.finish().unwrap();
        assert_eq!(done.peak, 1.0);
        assert_eq!(done.parts, [scratch.0.join("Set (2).wav")]);
        assert_eq!(std::fs::read(scratch.0.join("Set.wav")).unwrap(), b"last week's");
        let (_, samples) = read(&done.parts[0]);
        assert_eq!(samples, [8_388_607, -4_194_304, 2_097_152, -8_388_607]);
    }

    #[test]
    fn a_recording_of_nothing_but_zeros_says_it_heard_nothing() {
        let scratch = Scratch::new("silent");
        let mut recorder = Recorder::start(&scratch.0, "Set", 8_000, 2).unwrap();
        recorder.write(&[0.0; 16_000]).unwrap();
        assert_eq!(recorder.finish().unwrap().peak, 0.0);
    }

    #[test]
    fn a_frame_split_across_two_writes_is_refused_rather_than_shifting_the_channels() {
        let scratch = Scratch::new("odd");
        let mut recorder = Recorder::start(&scratch.0, "Set", 8_000, 2).unwrap();
        assert!(recorder.write(&[0.1, 0.2, 0.3]).is_err());
    }

    #[test]
    fn an_hour_at_48_khz_24_bit_stereo_is_a_little_over_a_gigabyte() {
        assert_eq!(bytes_per_hour(48_000, 2), 1_036_800_000);
        // So a part is just under four hours, which is most sets in one file.
        let hours = PART_BYTES as f64 / bytes_per_hour(48_000, 2) as f64;
        assert!((3.8..3.9).contains(&hours), "{hours}");
    }
}
