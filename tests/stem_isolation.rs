//! Proves the separator actually isolates a voice, by transcribing the stems.
//!
//! Loudness figures can only hint at isolation. Speech recognition answers it
//! directly: run the mix and each stem through the same transcriber, and the
//! words should come back from the vocal stem and from nowhere else.
//!
//! No expected text is stored here. The reference is whatever the transcriber
//! makes of the **original mix at run time**, so the test works on any track
//! with a voice in it and holds no lyrics of its own.
//!
//! `#[ignore]`d, because it needs three things the normal test run does not:
//!
//! ```sh
//! pipx install demucs openai-whisper          # or a shared venv
//! export MUSICAI_TEST_TRACK=/path/to/song-with-vocals.flac
//! export MUSICAI_DEMUCS_BIN=demucs            # optional, defaults to PATH
//! export MUSICAI_WHISPER_BIN=whisper          # optional, defaults to PATH
//! cargo test --release --test stem_isolation -- --ignored --nocapture
//! ```

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use musicai::audio::decode::decode_file;
use musicai::audio::encode::{write_file, Codec, EncodeOptions};
use musicai::audio::Audio;
use musicai::stems::{demucs, Stem, StemSet};

/// Seconds of the track to use. Long enough to contain several sung phrases,
/// short enough that separating and transcribing it four times is bearable on
/// a CPU.
const EXCERPT_SECONDS: f64 = 60.0;

/// Skip rather than fail when the track has not been supplied, matching how
/// the AcoustID test handles a missing key.
macro_rules! require_env {
    ($name:expr) => {
        match std::env::var($name) {
            Ok(value) => value,
            Err(_) => {
                eprintln!("{} is not set; skipping", $name);
                return;
            }
        }
    };
}

fn tool(env_name: &str, default: &str) -> String {
    std::env::var(env_name).unwrap_or_else(|_| default.to_string())
}

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("musicai-isolation-{name}"));
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

/// Take the middle of a track, where a song is most likely to be singing.
fn excerpt(audio: &Audio, seconds: f64) -> Audio {
    let rate = audio.sample_rate as f64;
    let wanted = (seconds * rate) as usize;
    if audio.frames() <= wanted {
        return audio.clone();
    }

    let start = (audio.frames() - wanted) / 2;
    let planes = audio.planes.iter().map(|p| p[start..start + wanted].to_vec()).collect::<Vec<_>>();
    Audio::new(audio.sample_rate, planes).unwrap()
}

/// Run Whisper over a file and return what it heard.
fn transcribe(path: &Path, whisper_bin: &str, out_dir: &Path) -> String {
    let output = Command::new(whisper_bin)
        .arg(path)
        .args(["--model", "base"])
        .args(["--language", "en"])
        .args(["--output_format", "txt"])
        // Half precision is a GPU feature; asking for it on CPU only produces
        // a warning and slower maths.
        .args(["--fp16", "False"])
        .arg("--output_dir")
        .arg(out_dir)
        .output()
        .unwrap_or_else(|e| panic!("could not run {whisper_bin}: {e}"));

    assert!(
        output.status.success(),
        "whisper failed on {}: {}",
        path.display(),
        String::from_utf8_lossy(&output.stderr).lines().last().unwrap_or("no output")
    );

    let stem = path.file_stem().unwrap().to_string_lossy();
    let transcript = out_dir.join(format!("{stem}.txt"));
    std::fs::read_to_string(&transcript)
        .unwrap_or_else(|e| panic!("no transcript at {}: {e}", transcript.display()))
}

/// Words that carry no evidence either way.
///
/// Whisper does not return nothing for instrumental audio — it invents filler,
/// typically function words and stock phrases like "thanks for watching". Those
/// would overlap a lyric transcript by chance and flatter a stem with no voice
/// in it at all, so they are excluded from both sides of the comparison.
const FILLER: &[&str] = &[
    "and",
    "are",
    "but",
    "for",
    "from",
    "had",
    "has",
    "have",
    "her",
    "him",
    "his",
    "its",
    "not",
    "our",
    "out",
    "she",
    "that",
    "the",
    "their",
    "them",
    "then",
    "there",
    "they",
    "this",
    "was",
    "were",
    "what",
    "when",
    "which",
    "who",
    "will",
    "with",
    "you",
    "your",
    "yeah",
    "oooh",
    "ooh",
    "mmm",
    "hmm",
    "thanks",
    "watching",
    "subscribe",
    "music",
    "applause",
];

/// Distinct lowercase content words of three or more letters.
fn words(text: &str) -> BTreeSet<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphabetic())
        .filter(|w| w.len() >= 3 && !FILLER.contains(w))
        .map(str::to_string)
        .collect()
}

/// Fraction of the reference's words that also appear in `candidate`.
fn recall(reference: &BTreeSet<String>, candidate: &BTreeSet<String>) -> f64 {
    if reference.is_empty() {
        return 0.0;
    }
    reference.intersection(candidate).count() as f64 / reference.len() as f64
}

fn separate_with_demucs(path: &Path, work_dir: &Path, demucs_bin: &str) -> StemSet {
    let mut config = demucs::Config::new(work_dir.to_path_buf());
    config.program = demucs_bin.into();
    demucs::separate(path, &config).unwrap_or_else(|e| panic!("demucs separation failed: {e:#}"))
}

#[test]
#[ignore = "needs demucs, whisper and MUSICAI_TEST_TRACK"]
fn lyrics_appear_only_in_the_vocal_stem() {
    let track = require_env!("MUSICAI_TEST_TRACK");
    let demucs_bin = tool("MUSICAI_DEMUCS_BIN", "demucs");
    let whisper_bin = tool("MUSICAI_WHISPER_BIN", "whisper");

    let dir = Scratch::new("lyrics");
    let encode = EncodeOptions { bit_depth: 16, ..Default::default() };

    // Trim first: separating and transcribing a whole track four times over is
    // needlessly slow, and a minute of singing is plenty to tell the stems
    // apart.
    let full = decode_file(Path::new(&track)).expect("could not decode the test track");
    let clip = excerpt(&full, EXCERPT_SECONDS);
    let mix_path = dir.path("original.wav");
    write_file(&mix_path, &clip, Codec::Wav, &encode).unwrap();

    let stems = separate_with_demucs(&mix_path, &dir.path("work"), &demucs_bin);

    // Whisper reads files, so each stem has to land on disk first.
    let mut stem_paths = Vec::new();
    for (stem, audio) in stems.iter() {
        let path = dir.path(&format!("{stem}.wav"));
        write_file(&path, audio, Codec::Wav, &encode).unwrap();
        stem_paths.push((stem, path));
    }

    let transcripts = dir.path("transcripts");
    std::fs::create_dir_all(&transcripts).unwrap();

    let heard = |path: &Path| words(&transcribe(path, &whisper_bin, &transcripts));
    let mix_words = heard(&mix_path);
    let stem_words: Vec<(Stem, BTreeSet<String>)> =
        stem_paths.iter().map(|(stem, path)| (*stem, heard(path))).collect();

    let of = |want: Stem| &stem_words.iter().find(|(s, _)| *s == want).unwrap().1;
    let vocals = of(Stem::Vocals);
    let melody = of(Stem::Melody);
    let drums = of(Stem::Drums);

    // Counts only. The transcripts are song lyrics and have no business in
    // test output or in this repository.
    eprintln!("  mix    {:>3} distinct words", mix_words.len());
    for (stem, words) in &stem_words {
        eprintln!("  {stem:<7}{:>3} distinct words", words.len());
    }

    // The vocal stem is the reference, not the mix. Separation makes singing
    // *more* intelligible than it was in the mix — on a dense, loud master
    // Whisper can return nothing at all for the original and a full transcript
    // for the isolated voice — so the mix is the least reliable thing to
    // measure against.
    assert!(
        vocals.len() >= 10,
        "the vocal stem yielded only {} distinct words, so there is nothing to test; point          MUSICAI_TEST_TRACK at a track with clearly audible singing",
        vocals.len()
    );

    // The claim under test: those words are in the vocal stem and not the
    // others. A small margin rather than zero, because a little bleed is
    // normal and Whisper invents words when handed instrumental audio.
    for (name, other) in [("melody", melody), ("drums", drums)] {
        let leaked = recall(vocals, other);
        assert!(
            leaked <= 0.25,
            "the {name} stem reproduced {:.1}% of the words sung in the vocal stem; the voice \
             is bleeding into it",
            leaked * 100.0
        );
    }

    // Whatever the mix did yield should be accounted for by the vocal stem.
    // Skipped when the mix transcribes to almost nothing, which is a normal
    // outcome for a loud mix rather than a failure.
    if mix_words.len() >= 10 {
        let captured = recall(&mix_words, vocals);
        eprintln!("  vocal stem accounts for {:.1}% of the mix's words", captured * 100.0);
        assert!(
            captured >= 0.5,
            "the vocal stem accounts for only {:.1}% of the words heard in the mix",
            captured * 100.0
        );
    } else {
        eprintln!(
            "  mix yielded too few words ({}) to compare against; the loud master is hard for \
             speech recognition, which is itself the point of separating it",
            mix_words.len()
        );
    }
}

#[test]
fn word_extraction_ignores_filler() {
    let extracted = words("Oh, a the CHORUS repeats -- chorus, yes! You and they.");
    // Case folded, punctuation dropped, duplicates collapsed.
    assert!(extracted.contains("chorus"));
    assert!(extracted.contains("repeats"));
    assert!(extracted.contains("yes"));
    // Too short to carry evidence.
    assert!(!extracted.contains("oh"));
    assert!(!extracted.contains("a"));
    // Long enough, but function words Whisper invents over instrumentals.
    assert!(!extracted.contains("the"));
    assert!(!extracted.contains("you"));
    assert!(!extracted.contains("and"));
    assert!(!extracted.contains("they"));
    assert_eq!(extracted.len(), 3);
}

#[test]
fn a_hallucinated_instrumental_transcript_scores_near_zero() {
    // What Whisper actually tends to emit when handed music with no singing.
    let reference = words("she walked the empty harbour road at midnight");
    let hallucinated = words("You. Thanks for watching! [Music] and the... you");
    assert_eq!(recall(&reference, &hallucinated), 0.0);
}

#[test]
fn recall_measures_the_reference_not_the_candidate() {
    let reference = words("alpha bravo charlie delta");
    // Half the reference present, plus noise that must not inflate the score.
    let candidate = words("alpha bravo xray yankee zulu whiskey");
    assert!((recall(&reference, &candidate) - 0.5).abs() < 1e-9);

    assert_eq!(recall(&reference, &words("")), 0.0);
    assert_eq!(recall(&BTreeSet::new(), &reference), 0.0);
    assert!((recall(&reference, &reference) - 1.0).abs() < 1e-9);
}

#[test]
fn excerpt_takes_the_middle_and_keeps_shorter_audio_whole() {
    let rate = 8_000u32;
    let plane: Vec<f32> = (0..rate * 10).map(|i| i as f32).collect();
    let audio = Audio::new(rate, vec![plane]).unwrap();

    let clip = excerpt(&audio, 4.0);
    assert_eq!(clip.frames(), (rate * 4) as usize);
    // Centred: three seconds in on a ten-second source.
    assert_eq!(clip.planes[0][0], (rate * 3) as f32);

    let short = excerpt(&audio, 30.0);
    assert_eq!(short.frames(), audio.frames());
}
