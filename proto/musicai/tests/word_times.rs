//! Whether the words the recogniser hands back are where they were said, once
//! the record has been cut to its sung stretches.
//!
//! whisper.cpp times every token, and `Transcript::words` keeps those times
//! for timing written lines by — where a line's time is its segment's, and a
//! segment can be half a minute of a verse. Behind a long silence the token
//! times are seconds out, the same way the segment's start is, which is why
//! they are only kept where the recogniser was handed the cut: this measures
//! that the cut is enough.
//!
//! The material is whisper.cpp's own sample, "And so my fellow Americans, ask
//! not what your country can do for you, ask what you can do for your
//! country", put six seconds into a file and read through
//! `whisper::transcribe_mix` as both the record and its stem. Where each word
//! starts was measured off the sample's own audio — the onsets after its
//! pauses, at 0.33 s, 3.29 s and 8.19 s — so the words are checked against
//! the sound and not against another recogniser.
//!
//! `#[ignore]`d: it needs whisper.cpp with a model and its sample.
//!
//! ```sh
//! export BOOTH_WHISPER_BIN=~/.cache/booth/whisper.cpp/build/bin/whisper-cli
//! export BOOTH_WHISPER_MODEL=~/.cache/booth/models/ggml-base.en.bin
//! export BOOTH_WHISPER_SAMPLE=~/.cache/booth/whisper.cpp/samples/jfk.wav
//! cargo test -p booth-cli --test word_times -- --ignored --nocapture
//! ```

use std::path::PathBuf;

use booth_cli::audio::Audio;
use booth_cli::transcribe::whisper::{self, Config};

/// How far a word may come back from where its sound starts, in ms. A
/// recogniser's token time and an energy onset are two measurements of a
/// soft edge.
const WITHIN_MS: u32 = 150;

/// How much silence goes in front: longer than the sample's own lead-in by
/// enough that, read whole, its first words came back seconds early.
const SILENCE_MS: u32 = 6_000;

#[test]
#[ignore = "needs whisper.cpp with a model, and its jfk.wav sample"]
fn behind_a_long_silence_the_words_come_back_where_they_were_said() {
    let (Ok(model), Ok(sample)) =
        (std::env::var("BOOTH_WHISPER_MODEL"), std::env::var("BOOTH_WHISPER_SAMPLE"))
    else {
        eprintln!("BOOTH_WHISPER_MODEL or BOOTH_WHISPER_SAMPLE is not set; skipping");
        return;
    };
    let said = booth_cli::audio::decode::decode_file(&PathBuf::from(sample)).unwrap();
    let rate = said.sample_rate;
    let lead = vec![0.0f32; (rate * SILENCE_MS / 1000) as usize];
    let tail = vec![0.0f32; (rate * 4) as usize];
    let planes: Vec<Vec<f32>> = (0..said.channels())
        .map(|c| [lead.clone(), said.planes[c].clone(), tail.clone()].concat())
        .collect();
    let late = Audio::new(rate, planes).unwrap();

    let work = std::env::temp_dir().join("booth-word-times");
    let _ = std::fs::remove_dir_all(&work);
    let mut config = Config::new(work.clone());
    if let Ok(program) = std::env::var("BOOTH_WHISPER_BIN") {
        config.program = program.into();
    }
    config.model = Some(model.into());
    config.language = Some("en".into());
    let heard = whisper::transcribe_mix(&late, &late, &config, &|_| {}).unwrap();
    let _ = std::fs::remove_dir_all(&work);

    let words = &heard.transcript.words;
    for word in words {
        eprintln!("{:>6} {:>6}  {}", word.start_ms, word.end_ms, word.text);
    }
    // The first word, and the first word after each long pause: where the
    // sound starts is unambiguous there.
    let starts: Vec<u32> = words
        .iter()
        .filter(|word| {
            let said = word.text.to_lowercase();
            said.starts_with("and") || said.starts_with("ask")
        })
        .map(|word| word.start_ms)
        .collect();
    let measured = [330, 3_290, 8_190].map(|at| at + SILENCE_MS);
    assert_eq!(starts.len(), measured.len(), "{words:?}");
    for (heard, sounded) in starts.iter().zip(measured) {
        assert!(
            heard.abs_diff(sounded) <= WITHIN_MS,
            "a word came back at {heard} ms where its sound starts at {sounded}: {words:?}"
        );
    }
}
