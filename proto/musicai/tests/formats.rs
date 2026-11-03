//! The formats no player opens, and the rates none takes: decoded, named,
//! converted, and carried onto a drive as an mp3.
//!
//! The fixtures are made with ffmpeg; see `tests/fixtures/formats/README.md`.
//! `tones.*` are two seconds of a different tone in each channel, for what a
//! decoder gets right; `beat.*` are ten seconds of a kick, a hat and a pad at
//! 128 BPM, because writing a drive means finding a grid.

mod common;

use std::path::{Path, PathBuf};

use booth_cli::audio::decode::decode_file;
use booth_cli::audio::encode::{write_file, Codec, EncodeOptions};
use booth_cli::audio::Audio;
use booth_cli::commands;
use booth_cli::report::Collected;
use common::drive::*;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/formats").join(name)
}

/// A tone's amplitude in a signal, by correlating against it under a Hann
/// window.
fn amplitude(signal: &[f32], rate: u32, hz: f64) -> f64 {
    let n = signal.len() as f64;
    let w = 2.0 * std::f64::consts::PI * hz / rate as f64;
    let (mut re, mut im, mut norm) = (0.0, 0.0, 0.0);
    for (i, &s) in signal.iter().enumerate() {
        let hann = 0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / n).cos();
        re += s as f64 * hann * (w * i as f64).cos();
        im += s as f64 * hann * (w * i as f64).sin();
        norm += hann;
    }
    2.0 * (re * re + im * im).sqrt() / norm
}

/// How loud each tone is in the fixtures: ffmpeg's sine source is an eighth of
/// full scale, halved, which is -24 dBFS.
const LEVEL: f64 = 0.0625;

/// Each channel holds its own tone at [`LEVEL`], and next to nothing of the
/// other's.
fn assert_two_tones(audio: &Audio, what: &str) {
    assert_eq!(audio.channels(), 2, "{what}");
    let middle = |plane: &[f32]| plane[plane.len() / 8..plane.len() * 7 / 8].to_vec();
    let (left, right) = (middle(&audio.planes[0]), middle(&audio.planes[1]));
    let rate = audio.sample_rate;
    for (plane, own, other, side) in
        [(&left, 1_000.0, 1_500.0, "left"), (&right, 1_500.0, 1_000.0, "right")]
    {
        let level = amplitude(plane, rate, own);
        assert!((level / LEVEL - 1.0).abs() < 0.05, "{what}: {side} has its tone at {level:.4}");
        let leak = amplitude(plane, rate, other) / LEVEL;
        assert!(leak < 0.01, "{what}: {side} has {leak:.4} of the other channel's tone");
    }
}

#[test]
fn a_vorbis_file_decodes_to_its_two_tones_in_their_own_channels() {
    let audio = decode_file(&fixture("tones.ogg")).unwrap();
    assert_eq!(audio.sample_rate, 44_100);
    assert_eq!(audio.frames(), 88_200, "two seconds, to the sample");
    assert_two_tones(&audio, "vorbis");
}

#[test]
fn an_opus_file_decodes_at_48_khz_with_its_encoder_delay_taken_off() {
    let audio = decode_file(&fixture("tones.opus")).unwrap();
    assert_eq!(audio.sample_rate, 48_000);
    assert_eq!(audio.frames(), 96_000, "two seconds, with the pre-skip and padding gone");
    assert_two_tones(&audio, "opus");

    // The tone starts at zero phase, so the decode lines up with a sine from
    // the first sample. Left on, libopus's 312-sample pre-skip would put the
    // best match 312 samples late.
    let left = &audio.planes[0];
    let lag = (-400i32..=400)
        .max_by(|&a, &b| {
            let score = |lag: i32| -> f64 {
                (4_800..91_200)
                    .map(|i| {
                        let t = (i as i32 - lag) as f64 / 48_000.0;
                        left[i] as f64 * (2.0 * std::f64::consts::PI * 1_000.0 * t).sin()
                    })
                    .sum()
            };
            score(a).total_cmp(&score(b))
        })
        .unwrap();
    // The tone repeats every 48 samples, so only the nearest repeat counts.
    let lag = (lag + 24).rem_euclid(48) - 24;
    assert!(lag.abs() <= 1, "the decode is {lag} samples off the source");
}

#[test]
fn a_wma_decodes_through_ffmpeg_when_there_is_one_and_says_to_install_it_when_not() {
    match booth_cli::audio::ffmpeg::find() {
        Some(_) => {
            let audio = decode_file(&fixture("tones.wma")).unwrap();
            assert_eq!(audio.sample_rate, 44_100);
            assert!(audio.frames().abs_diff(88_200) < 4_096, "{} frames", audio.frames());
            assert_two_tones(&audio, "wma");
        }
        None => {
            let error = decode_file(&fixture("tones.wma")).unwrap_err().to_string();
            assert!(error.contains("install it"), "{error}");
        }
    }
}

#[test]
fn ogg_opus_and_wma_files_arrive_with_their_names() {
    let mut names = vec!["tones.ogg", "tones.opus"];
    if booth_cli::audio::ffmpeg::find().is_some() {
        names.push("tones.wma");
    }
    for name in names {
        let tags = booth_cli::tag::read_metadata(&fixture(name)).unwrap();
        assert_eq!(tags.title.as_deref(), Some("Two Tones"), "{name}");
        assert_eq!(tags.artist.as_deref(), Some("Booth Test"), "{name}");
        assert_eq!(tags.album.as_deref(), Some("Fixtures"), "{name}");
        assert_eq!(tags.date.as_deref(), Some("2026"), "{name}");
        assert_eq!(tags.genre.as_deref(), Some("Techno"), "{name}");
        assert_eq!((tags.track_number, tags.total_tracks), (Some(2), Some(9)), "{name}");
    }
    let refused = booth_cli::tag::write_tags(
        &fixture("tones.ogg"),
        &booth_cli::tag::Metadata::default(),
        booth_cli::tag::OnExisting::Overwrite,
        None,
    );
    assert!(format!("{:#}", refused.unwrap_err()).contains("converted to FLAC"));
}

#[test]
fn a_drive_carries_ogg_opus_wma_and_a_192_khz_file_as_mp3s_a_player_opens() {
    let scratch = Scratch::new("formats");
    let drive = scratch.path("USB");
    let mut inputs = Vec::new();
    // Named apart, because their tags name the same artist and the same title
    // and each goes on as an mp3 called after its file.
    let mut kinds = vec![("beat.ogg", "vorbis.ogg"), ("beat.opus", "opus.opus")];
    if booth_cli::audio::ffmpeg::find().is_some() {
        kinds.push(("beat.wma", "wma.wma"));
    }
    for (from, to) in kinds {
        let path = scratch.path(to);
        std::fs::copy(fixture(from), &path).unwrap();
        inputs.push(path);
    }
    // A WAV, because the FLAC encoder here stops at 96 kHz: flacenc refuses
    // anything above it.
    let hires = scratch.path("hires.wav");
    let song = booth_cli::audio::resample::for_library(&song(10.0, 320.0), 192_000).unwrap();
    write_file(&hires, &song, Codec::Wav, &EncodeOptions { bit_depth: 24, ..Default::default() })
        .unwrap();
    inputs.push(hires);

    let mut args = args_for(inputs.clone());
    args.drive = Some(drive.clone());
    commands::export(&args, &Collected::new()).expect("the write failed");

    let tracks = legacy_tracks(&drive);
    assert_eq!(tracks.len(), inputs.len(), "{tracks:#?}");
    for track in &tracks {
        assert!(track.file_path.ends_with(".mp3"), "{} went on as it was", track.file_path);
        let landed = drive.join(track.file_path.trim_start_matches('/'));
        let audio = decode_file(&landed).unwrap();
        assert!(
            matches!(audio.sample_rate, 44_100 | 48_000),
            "{} is at {} Hz",
            track.file_path,
            audio.sample_rate
        );
    }
}
