//! Material the ignored measurement tests make for themselves, since the
//! container they run in has no music: a spoken lyric from espeak-ng over a
//! synthetic band, and the scoring of what a recogniser makes of it.
//!
//! Shared between tests rather than copied, so that the record two
//! measurements are made on is the same record.

#![allow(dead_code)]

pub mod drive;

use std::f32::consts::TAU;
use std::path::{Path, PathBuf};
use std::process::Command;

use booth_cli::audio::decode::decode_file;
use booth_cli::audio::resample::resample;
use booth_cli::audio::Audio;
use booth_cli::transcribe::Transcript;

pub const RATE: u32 = 44_100;
pub const BPM: f32 = 125.0;
pub const SECONDS: f32 = 64.0;

/// A lyric nobody wrote, so there is nothing of anybody's in the test: eight
/// lines over two verses, one of them twice so there is a hook, with an
/// eight-second intro, a twelve-second break and an outro with nobody singing.
pub const LINES: &[(f32, &str)] = &[
    (8.0, "the city never sleeps tonight"),
    (11.5, "we are dancing under neon light"),
    (15.0, "hold on to the morning sun"),
    (18.5, "every record plays for everyone"),
    (36.0, "turn it up and let it ride"),
    (39.5, "nobody wants to go outside"),
    (43.0, "the floor is moving like a tide"),
    (46.5, "we are dancing under neon light"),
];

/// Whether a program answers to `--version`, which is as much as a test
/// should ask before deciding to skip.
pub fn on_path(program: &str) -> bool {
    Command::new(program).arg("--version").output().is_ok()
}

/// A directory that is gone when the test is.
pub struct Scratch(pub PathBuf);

impl Scratch {
    pub fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("booth-measure-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    pub fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A linear congruential generator: noise that is the same every run.
struct Noise(u32);

impl Noise {
    fn next(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (self.0 >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0
    }
}

/// A band: a kick on the beat, a hat on the eighths, a snare on two and four,
/// a bass on the root and a pad on the triad, round four chords.
pub fn backing() -> Audio {
    let frames = (SECONDS * RATE as f32) as usize;
    let mut left = vec![0.0f32; frames];
    let mut right = vec![0.0f32; frames];
    let beat = 60.0 / BPM;
    let bar = beat * 4.0;
    // A minor, F, C, G: root, third, fifth.
    let chords: [(f32, f32, f32); 4] = [
        (110.0, 130.81, 164.81),
        (87.31, 110.0, 130.81),
        (130.81, 164.81, 196.0),
        (98.0, 123.47, 146.83),
    ];
    let mut noise = Noise(7);
    let dt = 1.0 / RATE as f32;
    for i in 0..frames {
        let t = i as f32 * dt;
        let in_beat = t % beat;
        let in_eighth = t % (beat / 2.0);
        let in_bar = t % bar;
        let beat_no = (t / beat) as usize;
        let (root, third, fifth) = chords[(t / bar) as usize % 4];

        // A kick is a sine whose pitch falls from 150 to 50 Hz as it dies.
        let kick = if in_beat < 0.3 {
            let phase = 50.0 * in_beat + 100.0 * 0.04 * (1.0 - (-in_beat / 0.04).exp());
            0.9 * (-in_beat / 0.12).exp() * (TAU * phase).sin()
        } else {
            0.0
        };
        let hat =
            if in_eighth < 0.05 { 0.25 * (-in_eighth / 0.02).exp() * noise.next() } else { 0.0 };
        let snare = if beat_no % 2 == 1 && in_beat < 0.2 {
            0.5 * (-in_beat / 0.06).exp()
                * (0.7 * noise.next() + 0.3 * (TAU * 180.0 * in_beat).sin())
        } else {
            0.0
        };
        let open = if in_eighth < 0.2 { 1.0 } else { (-(in_eighth - 0.2) / 0.05).exp() };
        let bass = 0.35
            * open
            * ((TAU * root * t).sin()
                + 0.5 * (TAU * 2.0 * root * t).sin()
                + 0.25 * (TAU * 3.0 * root * t).sin());
        let swell = (in_bar / 0.4).min(1.0);
        let pad = 0.12
            * swell
            * ((TAU * 2.0 * root * t).sin()
                + (TAU * 2.0 * third * t).sin()
                + (TAU * 2.0 * fifth * t).sin());

        left[i] = kick + 0.8 * hat + snare + bass + 0.7 * pad;
        right[i] = kick + 0.3 * hat + snare + bass + pad;
    }
    Audio::new(RATE, vec![left, right]).unwrap()
}

/// Speak a line, and bring it to the record's rate.
pub fn say(espeak: &str, text: &str, into: &Path) -> Audio {
    let status = Command::new(espeak)
        .args(["-v", "en-us", "-s", "140", "-w"])
        .arg(into)
        .arg(text)
        .status()
        .unwrap_or_else(|e| panic!("could not run {espeak}: {e}"));
    assert!(status.success(), "{espeak} failed on {text:?}");
    let spoken = decode_file(into).expect("espeak-ng's wav should decode");
    let mono = spoken.to_mono();
    Audio::new(RATE, vec![resample(&mono, spoken.sample_rate, RATE)]).unwrap()
}

/// The lines, spoken where [`LINES`] puts them, centred, peaking at `peak`.
/// Returns the track and where each line really starts and ends, in ms.
pub fn voice(espeak: &str, dir: &Scratch, peak: f32) -> (Audio, Vec<(u32, u32)>) {
    let frames = (SECONDS * RATE as f32) as usize;
    let mut plane = vec![0.0f32; frames];
    let mut when = Vec::with_capacity(LINES.len());
    for (nth, (at, text)) in LINES.iter().enumerate() {
        let spoken = say(espeak, text, &dir.path(&format!("line-{nth}.wav")));
        let start = (at * RATE as f32) as usize;
        for (i, sample) in spoken.planes[0].iter().enumerate() {
            if let Some(slot) = plane.get_mut(start + i) {
                *slot += sample;
            }
        }
        let start_ms = (at * 1000.0) as u32;
        when.push((start_ms, start_ms + (spoken.frames() as u64 * 1000 / RATE as u64) as u32));
    }
    let loudest = plane.iter().fold(0.0f32, |acc, s| acc.max(s.abs()));
    for sample in &mut plane {
        *sample *= peak / loudest;
    }
    (Audio::new(RATE, vec![plane.clone(), plane]).unwrap(), when)
}

/// The band under the singer, peaking a hair under full scale.
pub fn mixed(band: &Audio, singer: &Audio) -> Audio {
    let mut band = band.clone();
    band.scale(0.6 / band.sample_peak());
    band.add_assign(singer).unwrap();
    let peak = band.sample_peak();
    if peak > 0.98 {
        band.scale(0.98 / peak);
    }
    band
}

pub fn words(text: &str) -> Vec<String> {
    text.split(|c: char| !(c.is_alphanumeric() || c == '\''))
        .map(|word| word.trim_matches('\'').to_lowercase())
        .filter(|word| !word.is_empty())
        .collect()
}

/// Word error rate: the edits that turn what was heard into what was said,
/// over how much was said.
pub fn wer(said: &[String], heard: &[String]) -> f32 {
    let mut previous: Vec<usize> = (0..=heard.len()).collect();
    for (i, want) in said.iter().enumerate() {
        let mut current = vec![i + 1; heard.len() + 1];
        for (j, got) in heard.iter().enumerate() {
            let cost = usize::from(want != got);
            current[j + 1] = (previous[j] + cost).min(previous[j + 1] + 1).min(current[j] + 1);
        }
        previous = current;
    }
    previous[heard.len()] as f32 / said.len() as f32
}

/// How a reading did: its words against the lyric, and its times against
/// where the lines were really spoken.
pub struct Score {
    pub lines: usize,
    pub wer: f32,
    /// How many spoken lines a heard line starts within half a second of.
    pub lines_found: usize,
    /// Of the heard lines, how many start inside a spoken stretch, with the
    /// margin either side that a cut allows for.
    pub lines_placed: usize,
    pub median_off_ms: u32,
}

pub fn score(
    transcript: &Transcript,
    said: &[String],
    when: &[(u32, u32)],
    slack_ms: u32,
) -> Score {
    let heard: Vec<String> = transcript.lines.iter().flat_map(|line| words(&line.text)).collect();
    let starts: Vec<u32> = transcript.lines.iter().map(|line| line.start_ms).collect();
    let lines_found =
        when.iter().filter(|(start, _)| starts.iter().any(|at| at.abs_diff(*start) <= 500)).count();
    let lines_placed = starts
        .iter()
        .filter(|at| {
            when.iter().any(|(start, end)| **at + slack_ms >= *start && **at <= *end + slack_ms)
        })
        .count();
    let mut off: Vec<u32> = starts
        .iter()
        .map(|at| when.iter().map(|(start, _)| at.abs_diff(*start)).min().unwrap_or(0))
        .collect();
    off.sort_unstable();
    Score {
        lines: transcript.lines.len(),
        wer: wer(said, &heard),
        lines_found,
        lines_placed,
        median_off_ms: off.get(off.len() / 2).copied().unwrap_or(0),
    }
}

/// The RMS level of a stretch of a track, in dBFS.
pub fn level_db(audio: &Audio, from_secs: f32, to_secs: f32) -> f32 {
    let mono = audio.to_mono();
    let from = ((from_secs * audio.sample_rate as f32) as usize).min(mono.len());
    let to = ((to_secs * audio.sample_rate as f32) as usize).min(mono.len());
    if to <= from {
        return f32::NEG_INFINITY;
    }
    let mean_square = mono[from..to].iter().map(|s| s * s).sum::<f32>() / (to - from) as f32;
    10.0 * mean_square.max(1e-12).log10()
}
