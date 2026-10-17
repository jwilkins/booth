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
//! The material and the scoring are in `common`, shared with the RoFormer
//! backend's measurement, so the two are made on the same record.
//!
//! `#[ignore]`d: it needs whisper.cpp with a model, and espeak-ng.
//!
//! ```sh
//! export BOOTH_WHISPER_BIN=whisper-cli
//! export BOOTH_WHISPER_MODEL=~/.cache/booth/models/ggml-base.en.bin
//! cargo test -p booth-cli --release --test words_off_the_mix -- --ignored --nocapture
//! ```

mod common;

use std::path::PathBuf;
use std::time::Instant;

use booth_cli::stems::dsp;
use booth_cli::transcribe::cut::MARGIN_MS;
use booth_cli::transcribe::whisper::{self, Config, Handed, Heard};
use booth_cli::transcribe::Transcript;

use common::{score, words, Score, Scratch, LINES, SECONDS};

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
    require!(common::on_path(&espeak).then_some(()), format!("{espeak} is not installed"));
    require!(std::env::var("BOOTH_WHISPER_MODEL").ok(), "BOOTH_WHISPER_MODEL is not set");

    let dir = Scratch::new("words-off-the-mix");
    let band = common::backing();
    let (singer, when) = common::voice(&espeak, &dir, 0.5);
    let mix = common::mixed(&band, &singer);
    let said: Vec<String> = LINES.iter().flat_map(|(_, text)| words(text)).collect();
    let slack = MARGIN_MS + 500;

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
        table.push((name, score(&heard.transcript, &said, &when, slack), share, took));
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
    let placed = score(&ideal.transcript, &said, &when, slack);
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
