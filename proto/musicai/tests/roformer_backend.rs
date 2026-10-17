//! What the RoFormer backend hands back, measured on material the test makes
//! for itself — the same spoken lyric over the same synthetic band as the
//! words measurement, since the container has no music.
//!
//! Three things are asserted, because they must hold whatever the material:
//! the kit has the record's rate, length and channels; the three stems add
//! back up to the record, to a scale, since the program pulls levels down
//! rather than clipping; and the vocal stem is quiet where nobody is speaking
//! — the intro, the break, the outro — by enough that the words detector
//! finds separate sung stretches in it. The built-in separator's stem fails
//! that last one, which is what this backend exists to fix.
//!
//! With a recogniser set up as well, the record is read cut by this stem, and
//! the reading is printed alongside: what the words measurement could not run
//! for want of a separator in the container.
//!
//! `#[ignore]`d: it needs `audio-separator` with the network to fetch its
//! models the first time, and espeak-ng.
//!
//! ```sh
//! pipx install "audio-separator[cpu]"
//! export BOOTH_WHISPER_BIN=whisper-cli BOOTH_WHISPER_MODEL=~/.cache/booth/models/ggml-base.en.bin
//! cargo test -p booth-cli --release --test roformer_backend -- --ignored --nocapture
//! ```

mod common;

use std::path::PathBuf;
use std::time::Instant;

use booth_cli::audio::encode::{write_file, Codec, EncodeOptions};
use booth_cli::audio::Audio;
use booth_cli::stems::roformer;
use booth_cli::transcribe::align::voiced_spans;
use booth_cli::transcribe::cut::MARGIN_MS;
use booth_cli::transcribe::whisper::{self, Config, Handed};

use common::{level_db, score, words, Scratch, LINES, SECONDS};

/// Skip rather than fail when a tool is not there.
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

/// Normalised cross-correlation at zero lag: 1 is the same signal to a scale.
fn likeness(a: &Audio, b: &Audio) -> f32 {
    let a = a.to_mono();
    let b = b.to_mono();
    let n = a.len().min(b.len());
    let dot: f32 = a[..n].iter().zip(&b[..n]).map(|(x, y)| x * y).sum();
    let aa: f32 = a[..n].iter().map(|x| x * x).sum();
    let bb: f32 = b[..n].iter().map(|y| y * y).sum();
    dot / (aa.sqrt() * bb.sqrt()).max(1e-12)
}

#[test]
#[ignore = "needs audio-separator, with the network for its models, and espeak-ng"]
fn the_roformer_kit_is_the_record_in_three_parts_with_a_voice_that_is_quiet_between_lines() {
    let espeak = std::env::var("BOOTH_ESPEAK_BIN").unwrap_or_else(|_| "espeak-ng".into());
    require!(common::on_path(&espeak).then_some(()), format!("{espeak} is not installed"));
    let program = std::env::var("BOOTH_ROFORMER_BIN").unwrap_or_else(|_| "audio-separator".into());
    require!(common::on_path(&program).then_some(()), format!("{program} is not installed"));

    let dir = Scratch::new("roformer");
    let band = common::backing();
    let (singer, when) = common::voice(&espeak, &dir, 0.5);
    let mix = common::mixed(&band, &singer);
    let record = dir.path("record.wav");
    write_file(&record, &mix, Codec::Wav, &EncodeOptions { bit_depth: 24, ..Default::default() })
        .unwrap();

    let mut config = roformer::Config::new(dir.path("work"));
    config.program = program.into();
    if let Ok(models) = std::env::var("BOOTH_ROFORMER_MODELS") {
        config.models_dir = Some(PathBuf::from(models));
    }
    let started = Instant::now();
    let last = std::cell::Cell::new(0u8);
    let kit = roformer::separate(&record, &config, &|percent| {
        if percent >= last.get() + 10 {
            eprintln!("  {percent}%");
            last.set(percent);
        }
    })
    .expect("audio-separator should separate the record");
    eprintln!("separated {SECONDS} s in {:.0} s", started.elapsed().as_secs_f32());

    // The record's shape, in every stem.
    for (name, stem) in kit.iter() {
        assert_eq!(stem.sample_rate, mix.sample_rate, "{name}");
        assert_eq!(stem.frames(), mix.frames(), "{name}");
        assert_eq!(stem.channels(), mix.channels(), "{name}");
    }

    // Adding back up, to a scale: the program keeps everything under nine
    // tenths of full scale, so the sum is the record a shade quieter.
    let remix = kit.remix().unwrap();
    let alike = likeness(&remix, &mix);
    let gain_db = 20.0 * (remix.sample_peak() / mix.sample_peak()).log10();
    eprintln!("the three stems summed are the record to {alike:.3}, {gain_db:+.1} dB");
    assert!(alike > 0.95, "the stems do not add back up to the record: {alike:.3}");

    // The voice: loud where the lines are, quiet where nobody speaks. The gap
    // is measured against what a separator was asked for.
    let sung = level_db(&kit.vocals, 8.0, 21.0);
    let intro = level_db(&kit.vocals, 0.5, 7.5);
    let brk = level_db(&kit.vocals, 22.0, 35.5);
    let outro = level_db(&kit.vocals, 49.5, 63.5);
    eprintln!(
        "vocal stem: {sung:.1} dBFS over the lines; {intro:.1} in the intro, {brk:.1} in the break, {outro:.1} in the outro"
    );
    let spans = voiced_spans(&kit.vocals);
    let covered: u32 = spans.iter().map(|(from, to)| to - from).sum();
    eprintln!(
        "the words detector finds {} sung stretches covering {:.0}% of the record",
        spans.len(),
        covered as f32 / (SECONDS * 1000.0) * 100.0
    );
    for (name, quiet) in [("intro", intro), ("break", brk), ("outro", outro)] {
        assert!(
            sung - quiet > 20.0,
            "the voice is only {:.1} dB quieter in the {name}",
            sung - quiet
        );
    }
    assert!(
        spans.len() >= 4 && covered < (SECONDS * 1000.0 * 0.6) as u32,
        "{} stretches covering {covered} ms: the stem is not quiet between the lines",
        spans.len()
    );

    // And, with a recogniser, what the words make of the record cut by it.
    let Ok(model) = std::env::var("BOOTH_WHISPER_MODEL") else {
        eprintln!("BOOTH_WHISPER_MODEL is not set, so the record is not read");
        return;
    };
    let mut whisper_config = Config::new(dir.path("whisper"));
    if let Ok(program) = std::env::var("BOOTH_WHISPER_BIN") {
        whisper_config.program = program.into();
    }
    whisper_config.model = Some(PathBuf::from(model));
    whisper_config.language = Some("en".into());
    let said: Vec<String> = LINES.iter().flat_map(|(_, text)| words(text)).collect();
    let at = Instant::now();
    let heard = whisper::transcribe_mix(&mix, &kit.vocals, &whisper_config, &|_| {})
        .expect("the recogniser should run");
    let took = at.elapsed().as_secs_f32();
    for line in &heard.transcript.lines {
        eprintln!("  {:>6} ms  {}", line.start_ms, line.text);
    }
    let scored = score(&heard.transcript, &said, &when, MARGIN_MS + 500);
    let share = match &heard.handed {
        Handed::Stretches { share, .. } => format!("{:.0}% of the record", share * 100.0),
        other => format!("{other:?}"),
    };
    eprintln!(
        "the record cut by the RoFormer stem: {} lines, WER {:.0}%, {}/{} found within 0.5 s, median {} ms off, {took:.1} s, {share}",
        scored.lines,
        scored.wer * 100.0,
        scored.lines_found,
        LINES.len(),
        scored.median_off_ms
    );
    assert!(matches!(heard.handed, Handed::Stretches { .. }), "{:?}", heard.handed);
    assert_eq!(scored.lines_placed, scored.lines, "a line came back where nobody was speaking");
}
