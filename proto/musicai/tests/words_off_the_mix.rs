//! Whether reading the record where the stem is singing beats reading the
//! stem — measured, on material the test makes for itself.
//!
//! The claim behind `transcribe::cut` is a published one, made on real songs
//! with a different separator and a larger model. This measures the mechanism
//! on what a test can build: a spoken lyric from espeak-ng over a synthetic
//! band, separated with the built-in separator, then read with the installed
//! whisper.cpp four ways — the stem, the whole record, the record cut by the
//! stem, and the record cut by the voice itself, which is what a perfect
//! separator would have given. It prints each reading's word error rate
//! against the lines it knows it put there, and how far each line came back
//! from where it was spoken.
//!
//! What it asserts is only what must hold whatever the material: that the
//! lines read off the cut record come back on the record's clock, inside a
//! stretch where something was really said, and that the cut was a cut. The
//! error rates are printed and not asserted, because a synthetic voice over a
//! synthetic band is not a song, and a number from it measures the mechanism
//! and not the music.
//!
//! `#[ignore]`d: it needs whisper.cpp with a model, and espeak-ng.
//!
//! ```sh
//! export BOOTH_WHISPER_BIN=whisper-cli
//! export BOOTH_WHISPER_MODEL=~/.cache/booth/models/ggml-base.en.bin
//! cargo test -p booth-cli --release --test words_off_the_mix -- --ignored --nocapture
//! ```

use std::f32::consts::TAU;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use booth_cli::audio::decode::decode_file;
use booth_cli::audio::resample::resample;
use booth_cli::audio::Audio;
use booth_cli::stems::dsp;
use booth_cli::transcribe::cut::MARGIN_MS;
use booth_cli::transcribe::whisper::{self, Config, Handed, Heard};
use booth_cli::transcribe::Transcript;

const RATE: u32 = 44_100;
const BPM: f32 = 125.0;
const SECONDS: f32 = 64.0;

/// A lyric nobody wrote, so there is nothing of anybody's in the test: eight
/// lines over two verses, one of them twice so there is a hook, with an
/// eight-second intro, a twelve-second break and an outro with nobody singing.
const LINES: &[(f32, &str)] = &[
    (8.0, "the city never sleeps tonight"),
    (11.5, "we are dancing under neon light"),
    (15.0, "hold on to the morning sun"),
    (18.5, "every record plays for everyone"),
    (36.0, "turn it up and let it ride"),
    (39.5, "nobody wants to go outside"),
    (43.0, "the floor is moving like a tide"),
    (46.5, "we are dancing under neon light"),
];

/// Skip rather than fail when a tool is not there, so the test can sit in the
/// suite and be run on purpose.
macro_rules! require {
    ($what:expr, $why:expr) => {
        match $what {
            Some(value) => value,
            None => {
                eprintln!("{}; skipping", $why);
                return;
            }
        }
    };
}

fn on_path(program: &str) -> bool {
    Command::new(program).arg("--version").output().is_ok()
}

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let dir = std::env::temp_dir().join("booth-words-off-the-mix");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn path(&self, name: &str) -> PathBuf {
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
fn backing() -> Audio {
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
fn say(espeak: &str, text: &str, into: &Path) -> Audio {
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
/// Returns the track and where each line really ends.
fn voice(espeak: &str, dir: &Scratch, peak: f32) -> (Audio, Vec<(u32, u32)>) {
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

fn mixed(band: &Audio, singer: &Audio) -> Audio {
    let mut band = band.clone();
    band.scale(0.6 / band.sample_peak());
    band.add_assign(singer).unwrap();
    let peak = band.sample_peak();
    if peak > 0.98 {
        band.scale(0.98 / peak);
    }
    band
}

fn words(text: &str) -> Vec<String> {
    text.split(|c: char| !(c.is_alphanumeric() || c == '\''))
        .map(|word| word.trim_matches('\'').to_lowercase())
        .filter(|word| !word.is_empty())
        .collect()
}

/// Word error rate: the edits that turn what was heard into what was said,
/// over how much was said.
fn wer(said: &[String], heard: &[String]) -> f32 {
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
struct Score {
    lines: usize,
    wer: f32,
    /// How many spoken lines a heard line starts within half a second of.
    lines_found: usize,
    /// Of the heard lines, how many start inside a spoken stretch, with the
    /// margin either side that the cut allows for.
    lines_placed: usize,
    median_off_ms: u32,
}

fn score(transcript: &Transcript, said: &[String], when: &[(u32, u32)]) -> Score {
    let heard: Vec<String> = transcript.lines.iter().flat_map(|line| words(&line.text)).collect();
    let starts: Vec<u32> = transcript.lines.iter().map(|line| line.start_ms).collect();
    let lines_found =
        when.iter().filter(|(start, _)| starts.iter().any(|at| at.abs_diff(*start) <= 500)).count();
    let slack = MARGIN_MS + 500;
    let lines_placed = starts
        .iter()
        .filter(|at| when.iter().any(|(start, end)| **at + slack >= *start && **at <= *end + slack))
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

fn coverage(spans: &[(u32, u32)]) -> f32 {
    spans.iter().map(|(from, to)| to - from).sum::<u32>() as f32 / (SECONDS * 1000.0) * 100.0
}

fn config(dir: &Scratch, name: &str) -> Config {
    let mut config = Config::new(dir.path(name));
    if let Ok(program) = std::env::var("BOOTH_WHISPER_BIN") {
        config.program = program.into();
    }
    config.model = std::env::var("BOOTH_WHISPER_MODEL").ok().map(PathBuf::from);
    config.language = Some("en".into());
    config
}

#[test]
#[ignore = "needs whisper.cpp with a model, and espeak-ng"]
fn the_record_cut_by_the_stem_is_read_on_the_records_clock() {
    let espeak = std::env::var("BOOTH_ESPEAK_BIN").unwrap_or_else(|_| "espeak-ng".into());
    require!(on_path(&espeak).then_some(()), format!("{espeak} is not installed"));
    require!(std::env::var("BOOTH_WHISPER_MODEL").ok(), "BOOTH_WHISPER_MODEL is not set");

    let dir = Scratch::new();
    let band = backing();
    let (singer, when) = voice(&espeak, &dir, 0.5);
    let mix = mixed(&band, &singer);
    let said: Vec<String> = LINES.iter().flat_map(|(_, text)| words(text)).collect();

    let started = Instant::now();
    let stems = dsp::separate(&mix, &dsp::Config::default()).expect("the separator should run");
    eprintln!(
        "separated {SECONDS} s with the built-in separator in {:.1} s",
        started.elapsed().as_secs_f32()
    );

    // What the detector makes of each stem, since the cut can only be as good
    // as that: a stem with no quiet in it is read whole.
    for (name, stem) in
        [("the built-in separator's stem", &stems.vocals), ("the voice itself", &singer)]
    {
        let spans = booth_cli::transcribe::align::voiced_spans(stem);
        eprintln!(
            "{name}: {} sung stretches covering {:.0}% of the record",
            spans.len(),
            coverage(&spans)
        );
    }

    let mut table: Vec<(&str, Score, Option<f32>, f32)> = Vec::new();
    let mut read = |name: &'static str, heard: anyhow::Result<Heard>, took: f32| -> Heard {
        let heard = heard.unwrap_or_else(|e| panic!("{name}: {e:#}"));
        let share = match &heard.handed {
            Handed::Stretches { share, .. } => Some(*share),
            _ => None,
        };
        eprintln!("{name}:");
        for line in &heard.transcript.lines {
            eprintln!("  {:>6} ms  {}", line.start_ms, line.text);
        }
        table.push((name, score(&heard.transcript, &said, &when), share, took));
        heard
    };
    let whole = |transcript: anyhow::Result<Transcript>| -> anyhow::Result<Heard> {
        transcript.map(|transcript| Heard { transcript, handed: Handed::WholeStem })
    };
    let quiet = |_: u8| {};

    let at = Instant::now();
    let stem_alone = whisper::transcribe_audio(&stems.vocals, &config(&dir, "stem"), &quiet);
    read("the stem", whole(stem_alone), at.elapsed().as_secs_f32());

    let at = Instant::now();
    let record = whisper::transcribe_audio(&mix, &config(&dir, "record"), &quiet);
    read("the whole record", whole(record), at.elapsed().as_secs_f32());

    let at = Instant::now();
    let by_stem = whisper::transcribe_mix(&mix, &stems.vocals, &config(&dir, "cut"), &quiet);
    let cut = read("the record cut by the stem", by_stem, at.elapsed().as_secs_f32());

    let at = Instant::now();
    let by_voice = whisper::transcribe_mix(&mix, &singer, &config(&dir, "ideal"), &quiet);
    let ideal = read("the record cut by the voice itself", by_voice, at.elapsed().as_secs_f32());

    eprintln!();
    eprintln!(
        "{:<36} {:>5} {:>6} {:>6} {:>7} {:>9} {:>6}",
        "reading", "lines", "WER", "found", "placed", "median", "took"
    );
    for (name, score, share, took) in &table {
        let of_record =
            share.map_or(String::new(), |share| format!("  ({:.0}% of the record)", share * 100.0));
        eprintln!(
            "{:<36} {:>5} {:>5.0}% {:>3}/{:<2} {:>3}/{:<3} {:>6} ms {:>5.1}s{}",
            name,
            score.lines,
            score.wer * 100.0,
            score.lines_found,
            LINES.len(),
            score.lines_placed,
            score.lines,
            score.median_off_ms,
            took,
            of_record
        );
    }
    eprintln!();

    // What must hold whatever the material. Every line read off a cut record
    // comes back where something was really said, rather than in the intro,
    // the break or the outro — which is the whole of what the way back has to
    // get right. Asked of the cut whose stem had quiet in it: a cut that kept
    // the whole record maps back onto itself and proves nothing.
    let Handed::Stretches { stretches, share } = &ideal.handed else {
        panic!("the record was not cut by the voice: {:?}", ideal.handed)
    };
    assert!(!stretches.is_empty());
    // The cut was a cut: the record has nobody singing for over half its length.
    assert!(*share > 0.0 && *share < 0.8, "the cut kept {:.0}% of the record", share * 100.0);
    let placed = score(&ideal.transcript, &said, &when);
    assert!(
        placed.lines > 0 && placed.lines_placed == placed.lines,
        "{} of {} lines came back somewhere nobody was speaking: {:?}",
        placed.lines - placed.lines_placed,
        placed.lines,
        ideal
            .transcript
            .lines
            .iter()
            .map(|line| (line.start_ms, line.text.as_str()))
            .collect::<Vec<_>>()
    );
    // The built-in separator's stem is read for the record regardless; what it
    // yields is in the table, not asserted.
    assert!(matches!(cut.handed, Handed::Stretches { .. }), "{:?}", cut.handed);
}
