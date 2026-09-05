//! End-to-end tests over real encoded files.
//!
//! These build a synthetic stereo "song" — a steady bass, a sustained chord, a
//! vibrato lead standing in for a voice, and a drum pattern — then push it
//! through the actual encoders, decoders and commands.

use std::path::PathBuf;
use std::process::Command;

use musicai::audio::decode::decode_file;
use musicai::audio::encode::{write_file, Codec, EncodeOptions};
use musicai::audio::Audio;
use musicai::loudness;
use musicai::normalize::{self, PeakPolicy, Settings};
use musicai::stems::{dsp, Stem};

const SAMPLE_RATE: u32 = 44_100;
const SECONDS: usize = 4;

/// A scratch directory that cleans itself up.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("musicai-e2e-{name}"));
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

fn sine(frames: usize, freq: f32, amplitude: f32) -> Vec<f32> {
    (0..frames)
        .map(|i| {
            let t = i as f32 / SAMPLE_RATE as f32;
            amplitude * (2.0 * std::f32::consts::PI * freq * t).sin()
        })
        .collect()
}

fn vibrato(frames: usize, freq: f32, amplitude: f32) -> Vec<f32> {
    let mut phase = 0.0f32;
    (0..frames)
        .map(|i| {
            let t = i as f32 / SAMPLE_RATE as f32;
            let f = freq * (1.0 + 0.05 * (2.0 * std::f32::consts::PI * 5.5 * t).sin());
            phase += 2.0 * std::f32::consts::PI * f / SAMPLE_RATE as f32;
            amplitude * phase.sin()
        })
        .collect()
}

fn drum_pattern(frames: usize, amplitude: f32) -> Vec<f32> {
    let mut out = vec![0.0f32; frames];
    let mut noise = 0xC0FF_EE00u32;
    let beat = SAMPLE_RATE as usize / 2;
    let mut at = 0;
    while at < frames {
        for k in 0..400.min(frames - at) {
            noise = noise.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let white = (noise >> 8) as f32 / (1u32 << 23) as f32 - 1.0;
            out[at + k] += amplitude * white * (-(k as f32) / 70.0).exp();
        }
        at += beat;
    }
    out
}

/// A four-second stereo mix with a centred lead, as a real one would be.
fn synthetic_song(level: f32) -> Audio {
    let frames = SAMPLE_RATE as usize * SECONDS;
    let bass = sine(frames, 110.0, 0.30);
    let chord = sine(frames, 329.6, 0.18);
    let lead = vibrato(frames, 880.0, 0.25);
    let drums = drum_pattern(frames, 0.5);

    let mut left = Vec::with_capacity(frames);
    let mut right = Vec::with_capacity(frames);
    for i in 0..frames {
        // Bass and lead centred, the chord panned slightly left.
        left.push(level * (bass[i] + lead[i] + chord[i] * 1.2 + drums[i]));
        right.push(level * (bass[i] + lead[i] + chord[i] * 0.8 + drums[i]));
    }
    Audio::new(SAMPLE_RATE, vec![left, right]).unwrap()
}

fn write_song(dir: &Scratch, name: &str, codec: Codec, level: f32) -> PathBuf {
    let path = dir.path(&format!("{name}.{}", codec.extension()));
    write_file(&path, &synthetic_song(level), codec, &EncodeOptions::default()).unwrap();
    path
}

fn musicai() -> Command {
    Command::new(env!("CARGO_BIN_EXE_musicai"))
}

#[test]
fn wav_round_trips_faithfully() {
    let dir = Scratch::new("wav-roundtrip");
    let original = synthetic_song(0.5);
    let path = dir.path("song.wav");
    write_file(
        &path,
        &original,
        Codec::Wav,
        &EncodeOptions { dither: false, ..Default::default() },
    )
    .unwrap();

    let decoded = decode_file(&path).unwrap();
    assert_eq!(decoded.sample_rate, original.sample_rate);
    assert_eq!(decoded.channels(), 2);
    assert_eq!(decoded.frames(), original.frames());

    // 16-bit quantization, and nothing else.
    for (a, b) in original.planes[0].iter().zip(&decoded.planes[0]) {
        assert!((a - b).abs() < 2.0 / 32_768.0, "{a} vs {b}");
    }
}

#[test]
fn flac_round_trips_losslessly() {
    let dir = Scratch::new("flac-roundtrip");
    let original = synthetic_song(0.5);
    let path = dir.path("song.flac");
    write_file(
        &path,
        &original,
        Codec::Flac,
        &EncodeOptions { dither: false, ..Default::default() },
    )
    .unwrap();

    let decoded = decode_file(&path).unwrap();
    assert_eq!(decoded.frames(), original.frames());
    assert_eq!(decoded.channels(), 2);

    // FLAC is lossless, so the only difference from the source is the same
    // 16-bit quantization the wav path applies.
    for (a, b) in original.planes[1].iter().zip(&decoded.planes[1]) {
        assert!((a - b).abs() < 2.0 / 32_768.0, "{a} vs {b}");
    }
}

#[test]
fn mp3_round_trips_recognisably() {
    let dir = Scratch::new("mp3-roundtrip");
    let original = synthetic_song(0.5);
    let path = dir.path("song.mp3");
    write_file(&path, &original, Codec::Mp3, &EncodeOptions::default()).unwrap();

    let decoded = decode_file(&path).unwrap();
    assert_eq!(decoded.sample_rate, SAMPLE_RATE);
    assert_eq!(decoded.channels(), 2);

    // mp3 is lossy and adds encoder delay, so compare loudness rather than
    // samples: the decoded file should measure within a fraction of a dB.
    let before = loudness::measure(&original).unwrap();
    let after = loudness::measure(&decoded).unwrap();
    assert!(
        (before.integrated_lufs - after.integrated_lufs).abs() < 0.5,
        "{} LUFS became {} LUFS",
        before.integrated_lufs,
        after.integrated_lufs
    );
}

#[test]
fn normalizing_a_real_file_lands_on_target() {
    let dir = Scratch::new("normalize-target");
    let path = write_song(&dir, "quiet", Codec::Flac, 0.05);

    let mut audio = decode_file(&path).unwrap();
    let settings = Settings {
        target_lufs: -16.0,
        ceiling_dbtp: -1.0,
        peak_policy: PeakPolicy::Limit,
        ..Default::default()
    };
    let report = normalize::apply(&mut audio, &settings).unwrap();

    assert!(report.gain_db > 0.0, "a quiet file should be turned up");
    assert!(
        report.target_error_lu(-16.0).abs() < 0.5,
        "landed at {} LUFS",
        report.after.integrated_lufs
    );
    assert!(report.after.true_peak_db() <= -1.0 + 0.01);

    // And it survives a round trip through the encoder at that level.
    let out = dir.path("loud.flac");
    write_file(&out, &audio, Codec::Flac, &EncodeOptions::default()).unwrap();
    let reloaded = loudness::measure(&decode_file(&out).unwrap()).unwrap();
    assert!((reloaded.integrated_lufs - report.after.integrated_lufs).abs() < 0.2);
}

#[test]
fn separating_a_real_file_conserves_the_mix() {
    let dir = Scratch::new("stems-conserve");
    let path = write_song(&dir, "song", Codec::Flac, 0.4);

    let audio = decode_file(&path).unwrap();
    let stems = dsp::separate(&audio, &dsp::Config::default()).unwrap();
    let remixed = stems.remix().unwrap();

    assert_eq!(remixed.channels(), audio.channels());
    assert_eq!(remixed.frames(), audio.frames());
    for channel in 0..audio.channels() {
        for (i, (&a, &b)) in audio.planes[channel].iter().zip(&remixed.planes[channel]).enumerate()
        {
            assert!((a - b).abs() < 1e-3, "channel {channel} sample {i}: {a} vs {b}");
        }
    }

    // Every stem should carry some signal; a stem that came out silent would
    // mean the split collapsed.
    for (stem, audio) in stems.iter() {
        assert!(audio.sample_peak() > 1e-3, "{stem} stem is silent");
    }
}

#[test]
fn drums_land_in_the_drum_stem() {
    let dir = Scratch::new("stems-drums");
    let path = write_song(&dir, "song", Codec::Flac, 0.4);
    let audio = decode_file(&path).unwrap();
    let stems = dsp::separate(&audio, &dsp::Config::default()).unwrap();

    // The drum pattern is the only broadband transient content in the mix, so
    // the drum stem should be much spikier than the pitched stems: a high
    // crest factor (peak over RMS) is the signature of transients.
    let crest = |a: &Audio| {
        let plane = &a.planes[0];
        let rms =
            (plane.iter().map(|&s| (s as f64).powi(2)).sum::<f64>() / plane.len() as f64).sqrt();
        a.sample_peak() as f64 / rms.max(1e-12)
    };

    let drums = crest(&stems.drums);
    let melody = crest(&stems.melody);
    assert!(drums > melody, "drum crest factor {drums} was not above melody's {melody}");
}

#[test]
fn cli_analyze_reports_loudness() {
    let dir = Scratch::new("cli-analyze");
    write_song(&dir, "a", Codec::Flac, 0.3);
    write_song(&dir, "b", Codec::Mp3, 0.1);

    let output = musicai().arg("analyze").arg(&dir.0).output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("true peak"), "missing header:\n{stdout}");
    assert!(stdout.contains("a.flac"), "missing first file:\n{stdout}");
    assert!(stdout.contains("b.mp3"), "missing second file:\n{stdout}");
    assert_eq!(stdout.lines().count(), 3, "expected a header and two rows:\n{stdout}");
}

#[test]
fn cli_analyze_emits_one_json_object_per_file() {
    let dir = Scratch::new("cli-json");
    write_song(&dir, "a", Codec::Wav, 0.3);

    let output = musicai().arg("analyze").arg("--json").arg(&dir.0).output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_eq!(stdout.lines().count(), 1);
    let line = stdout.lines().next().unwrap();
    for key in ["\"file\"", "\"integrated_lufs\"", "\"true_peak_dbtp\"", "\"channels\":2"] {
        assert!(line.contains(key), "missing {key} in {line}");
    }
}

#[test]
fn cli_normalize_writes_a_new_file_and_leaves_the_input_alone() {
    let dir = Scratch::new("cli-normalize");
    let input = write_song(&dir, "song", Codec::Flac, 0.05);
    let before = std::fs::read(&input).unwrap();

    let output =
        musicai().arg("normalize").arg(&input).arg("--target").arg("-16").output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));

    let written = dir.path("song-normalized.flac");
    assert!(written.exists(), "no output file:\n{}", String::from_utf8_lossy(&output.stdout));
    assert_eq!(std::fs::read(&input).unwrap(), before, "the input was modified");

    let measured = loudness::measure(&decode_file(&written).unwrap()).unwrap();
    assert!(
        (measured.integrated_lufs - -16.0).abs() < 0.6,
        "output measured {} LUFS",
        measured.integrated_lufs
    );
}

#[test]
fn cli_normalize_refuses_to_clobber_without_force() {
    let dir = Scratch::new("cli-clobber");
    let input = write_song(&dir, "song", Codec::Wav, 0.2);
    std::fs::write(dir.path("song-normalized.wav"), b"existing").unwrap();

    let output = musicai().arg("normalize").arg(&input).output().unwrap();
    assert!(!output.status.success(), "should have refused to overwrite");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("--force"), "unhelpful error: {stderr}");

    // The existing file is untouched.
    assert_eq!(std::fs::read(dir.path("song-normalized.wav")).unwrap(), b"existing");

    // With --force it goes through.
    let output = musicai().arg("normalize").arg(&input).arg("--force").output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert!(decode_file(&dir.path("song-normalized.wav")).is_ok());
}

#[test]
fn cli_normalize_dry_run_writes_nothing() {
    let dir = Scratch::new("cli-dryrun");
    let input = write_song(&dir, "song", Codec::Wav, 0.2);

    let output = musicai().arg("normalize").arg(&input).arg("--dry-run").output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("dry run"), "{stdout}");
    assert!(!dir.path("song-normalized.wav").exists(), "dry run wrote a file");
}

#[test]
fn cli_replaygain_tags_without_touching_the_audio() {
    let dir = Scratch::new("cli-replaygain");
    let flac = write_song(&dir, "song", Codec::Flac, 0.05);
    let before = decode_file(&flac).unwrap();

    let output =
        musicai().arg("normalize").arg("--mode").arg("replaygain").arg(&flac).output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));

    // The tag is there and says to turn this quiet file up.
    let tag = metaflac::Tag::read_from_path(&flac).unwrap();
    let gain = tag
        .get_vorbis("REPLAYGAIN_TRACK_GAIN")
        .and_then(|mut v| v.next().map(str::to_string))
        .expect("no REPLAYGAIN_TRACK_GAIN tag");
    assert!(gain.ends_with(" dB"), "malformed gain tag: {gain}");
    let value: f64 = gain.trim_end_matches(" dB").parse().unwrap();
    assert!(value > 5.0, "expected a large positive gain, got {value}");

    // And the audio itself is unchanged.
    let after = decode_file(&flac).unwrap();
    assert_eq!(before.planes, after.planes, "replaygain mode altered the audio");
}

#[test]
fn cli_replaygain_computes_album_gain_across_a_directory() {
    let dir = Scratch::new("cli-album");
    write_song(&dir, "loud", Codec::Flac, 0.4);
    write_song(&dir, "quiet", Codec::Flac, 0.04);

    let output = musicai()
        .arg("normalize")
        .arg("--mode")
        .arg("replaygain")
        .arg("--album")
        .arg(&dir.0)
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));

    let album_gain = |name: &str| -> f64 {
        let tag = metaflac::Tag::read_from_path(dir.path(name)).unwrap();
        tag.get_vorbis("REPLAYGAIN_ALBUM_GAIN")
            .and_then(|mut v| v.next().map(str::to_string))
            .expect("no album gain tag")
            .trim_end_matches(" dB")
            .parse()
            .unwrap()
    };

    // Album gain is a property of the album, so both tracks carry the same
    // value even though their track gains differ wildly.
    let loud = album_gain("loud.flac");
    let quiet = album_gain("quiet.flac");
    assert!((loud - quiet).abs() < 1e-9, "album gain differed: {loud} vs {quiet}");
}

#[test]
fn cli_stems_writes_three_files() {
    let dir = Scratch::new("cli-stems");
    let input = write_song(&dir, "song", Codec::Flac, 0.4);
    let out_dir = dir.path("out");

    // Pinned to the built-in backend on purpose: this is about the CLI's file
    // layout, not about separation quality, and it must run without demucs
    // installed.
    let output = musicai()
        .arg("stems")
        .arg(&input)
        .args(["--backend", "dsp"])
        .arg("--out-dir")
        .arg(&out_dir)
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));

    for stem in Stem::ALL {
        // mp3 whatever the source was: a kit is three more files per track.
        let path = out_dir.join(format!("song-{stem}.mp3"));
        assert!(path.exists(), "missing {}", path.display());
        let audio = decode_file(&path).unwrap();
        assert_eq!(audio.channels(), 2);
        assert_eq!(audio.sample_rate, SAMPLE_RATE);
        assert!(audio.sample_peak() > 1e-3, "{stem} came out silent");
    }
}

#[test]
fn cli_stems_are_mp3_whatever_the_source_is() {
    let dir = Scratch::new("cli-stems-format");

    // Whatever the source is, the stems come out mp3: a kit is three more
    // files per track, and a library of lossless ones is four times the disk
    // for audio that gets played under something else.
    for codec in [Codec::Flac, Codec::Mp3, Codec::Wav] {
        let input = write_song(&dir, &format!("song-{}", codec.extension()), codec, 0.4);
        let out_dir = dir.path(&format!("out-{}", codec.extension()));

        let output = musicai()
            .arg("stems")
            .arg(&input)
            .args(["--backend", "dsp", "--only", "vocals"])
            .arg("--out-dir")
            .arg(&out_dir)
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));

        let track = input.file_stem().unwrap().to_string_lossy().into_owned();
        let expected = out_dir.join(format!("{track}-vocals.mp3"));
        assert!(expected.exists(), "expected {}", expected.display());
        // And never the source's own format — except where that happens to be
        // mp3 too, which is the same file rather than a second one.
        if codec != Codec::Mp3 {
            assert!(!out_dir.join(format!("{track}-vocals.{}", codec.extension())).exists());
        }
    }

    // flac is the other choice, for a kit that will be worked on further.
    let input = write_song(&dir, "override", Codec::Mp3, 0.4);
    let out_dir = dir.path("out-override");
    let output = musicai()
        .arg("stems")
        .arg(&input)
        .args(["--backend", "dsp", "--only", "vocals", "--format", "flac"])
        .arg("--out-dir")
        .arg(&out_dir)
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert!(out_dir.join("override-vocals.flac").exists());
}

#[test]
fn cli_stems_honours_a_subset_and_format() {
    let dir = Scratch::new("cli-stems-subset");
    let input = write_song(&dir, "song", Codec::Wav, 0.4);
    let out_dir = dir.path("out");

    // Backend pinned for the same reason as above.
    let output = musicai()
        .arg("stems")
        .arg(&input)
        .args(["--backend", "dsp"])
        .arg("--out-dir")
        .arg(&out_dir)
        .arg("--only")
        .arg("vocals,drums")
        .arg("--format")
        .arg("flac")
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));

    assert!(out_dir.join("song-vocals.flac").exists());
    assert!(out_dir.join("song-drums.flac").exists());
    assert!(!out_dir.join("song-melody.flac").exists(), "wrote a stem that was not asked for");
}

#[test]
fn cli_reports_a_bad_file_without_giving_up_on_the_rest() {
    let dir = Scratch::new("cli-partial");
    write_song(&dir, "good", Codec::Flac, 0.3);
    std::fs::write(dir.path("broken.mp3"), b"this is not an mp3").unwrap();

    let output = musicai().arg("analyze").arg(&dir.0).output().unwrap();
    assert!(!output.status.success(), "expected a non-zero exit");

    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stdout.contains("good.flac"), "the good file was not reported:\n{stdout}");
    assert!(stderr.contains("broken.mp3"), "the bad file was not named:\n{stderr}");
}

#[test]
fn cli_rejects_a_missing_path() {
    let output = musicai().arg("analyze").arg("/no/such/file.mp3").output().unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8(output.stderr).unwrap().contains("does not exist"));
}

/// The default backend is demucs, so a machine without it must say so rather
/// than quietly falling back to the weaker separator.
#[test]
fn cli_stems_defaults_to_demucs() {
    let dir = Scratch::new("cli-stems-default");
    let input = write_song(&dir, "song", Codec::Wav, 0.3);

    let output = musicai()
        .arg("stems")
        .arg(&input)
        .arg("--demucs-bin")
        .arg("definitely-not-installed-demucs")
        .arg("--out-dir")
        .arg(dir.path("out"))
        .output()
        .unwrap();

    // No --backend given, yet it tried to run demucs.
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("demucs"), "the default backend was not demucs: {stderr}");
}

#[test]
fn cli_stems_reports_a_missing_demucs() {
    let dir = Scratch::new("cli-demucs");
    let input = write_song(&dir, "song", Codec::Wav, 0.3);

    let output = musicai()
        .arg("stems")
        .arg(&input)
        .arg("--backend")
        .arg("demucs")
        .arg("--demucs-bin")
        .arg("definitely-not-installed-demucs")
        .arg("--out-dir")
        .arg(dir.path("out"))
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("--backend dsp"), "unhelpful error: {stderr}");
}

/// Guards the claim in the README that separation is well behaved on mono.
#[test]
fn mono_input_separates_into_mono_stems() {
    let dir = Scratch::new("mono");
    let stereo = synthetic_song(0.4);
    let mono = Audio::new(SAMPLE_RATE, vec![stereo.to_mono()]).unwrap();
    let path = dir.path("mono.wav");
    write_file(&path, &mono, Codec::Wav, &EncodeOptions::default()).unwrap();

    let decoded = decode_file(&path).unwrap();
    assert_eq!(decoded.channels(), 1);

    let stems = dsp::separate(&decoded, &dsp::Config::default()).unwrap();
    for (stem, audio) in stems.iter() {
        assert_eq!(audio.channels(), 1, "{stem} changed channel count");
    }

    let remixed = stems.remix().unwrap();
    for (a, b) in decoded.planes[0].iter().zip(&remixed.planes[0]) {
        assert!((a - b).abs() < 1e-3);
    }
}

/// The overlap setting trades memory for smoothness; both settings must still
/// reconstruct the input exactly.
#[test]
fn both_overlap_settings_reconstruct_exactly() {
    let audio = synthetic_song(0.4);
    for overlap in [2, 4] {
        let config = dsp::Config { overlap, ..Default::default() };
        let stems = dsp::separate(&audio, &config).unwrap();
        let remixed = stems.remix().unwrap();
        for (i, (&a, &b)) in audio.planes[0].iter().zip(&remixed.planes[0]).enumerate() {
            assert!((a - b).abs() < 1e-3, "overlap {overlap}, sample {i}: {a} vs {b}");
        }
    }
}

// -- the default pipeline --------------------------------------------------

/// Read both streams, because the pipeline deliberately splits results
/// (stdout) from commentary about the run (stderr).
fn run_pipeline(args: &[&str]) -> (bool, String, String) {
    let output = musicai().args(args).output().unwrap();
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

#[test]
fn no_subcommand_runs_the_whole_pipeline() {
    let dir = Scratch::new("cli-default");
    write_song(&dir, "song", Codec::Flac, 0.4);

    let stems = dir.path("stems");

    // Backend pinned so this needs nothing installed; no key, so tagging is
    // skipped rather than failing against a service the tests must not call.
    let (ok, _out, err) = run_pipeline(&[
        &dir.0.display().to_string(),
        "--backend",
        "dsp",
        "--stems-dir",
        &stems.display().to_string(),
    ]);
    assert!(ok, "{err}");

    // Every step announced itself, in the order the pipeline promises.
    let stages: Vec<&str> = err.lines().filter(|l| l.starts_with("== ")).collect();
    assert_eq!(stages, vec!["== 1/3 normalize ==", "== 2/3 tag ==", "== 3/3 stems =="], "{err}");
    assert!(err.contains("1 file through normalize -> tag -> stems"), "{err}");

    for stem in Stem::ALL {
        let path = stems.join(format!("song-{stem}.mp3"));
        assert!(path.exists(), "missing {}", path.display());
    }
}

#[test]
fn the_pipeline_and_the_run_subcommand_are_the_same_thing() {
    let dir = Scratch::new("cli-run-alias");
    write_song(&dir, "song", Codec::Flac, 0.4);
    let stems = dir.path("stems");

    let (ok, _, err) = run_pipeline(&[
        "run",
        &dir.0.display().to_string(),
        "--backend",
        "dsp",
        "--stems-dir",
        &stems.display().to_string(),
    ]);
    assert!(ok, "{err}");
    assert!(err.contains("== 1/3 normalize =="), "{err}");
}

#[test]
fn the_pipeline_skips_tagging_without_a_key_rather_than_failing_every_file() {
    let dir = Scratch::new("cli-nokey");
    write_song(&dir, "song", Codec::Flac, 0.4);

    let (ok, _, err) = run_pipeline(&[
        &dir.0.display().to_string(),
        "--steps",
        "tag",
        // An empty key is the same as none: the environment must not decide
        // whether this test talks to a network service.
        "--acoustid-key",
        "",
    ]);

    assert!(ok, "a missing key should skip the step, not fail the run: {err}");
    assert!(err.contains("skipping tag"), "{err}");
    assert!(err.contains("acoustid.org"), "the message should say how to get one: {err}");
}

#[test]
fn wav_files_survive_a_replaygain_pipeline() {
    let dir = Scratch::new("cli-wav-rg");
    write_song(&dir, "song", Codec::Wav, 0.4);
    let stems = dir.path("stems");

    // Wav cannot carry a ReplayGain tag. On its own that is an error; in a
    // pipeline the file should still get separated.
    let (ok, _, err) = run_pipeline(&[
        &dir.0.display().to_string(),
        "--steps",
        "normalize,stems",
        "--backend",
        "dsp",
        "--stems-dir",
        &stems.display().to_string(),
    ]);

    assert!(ok, "{err}");
    assert!(err.contains("cannot carry ReplayGain"), "{err}");
    assert!(stems.join("song-vocals.mp3").exists(), "the wav was dropped instead of separated");
}

#[test]
fn re_encoding_hands_the_new_files_to_the_next_step() {
    let dir = Scratch::new("cli-chain");
    write_song(&dir, "song", Codec::Wav, 0.05);
    let stems = dir.path("stems");

    let (ok, _, err) = run_pipeline(&[
        &dir.0.display().to_string(),
        "--mode",
        "reencode",
        "--steps",
        "normalize,stems",
        "--backend",
        "dsp",
        "--stems-dir",
        &stems.display().to_string(),
    ]);
    assert!(ok, "{err}");

    // Separating the originals instead of the normalized copies would be a
    // silent and very confusing bug, so the stems must be named for the copy.
    assert!(dir.path("song-normalized.wav").exists());
    assert!(
        stems.join("song-normalized-vocals.mp3").exists(),
        "stems were taken from the original, not the normalized file"
    );
    assert!(!stems.join("song-vocals.mp3").exists());
}

#[test]
fn a_pipeline_with_no_steps_does_nothing_quietly() {
    let dir = Scratch::new("cli-nosteps");
    write_song(&dir, "song", Codec::Flac, 0.4);

    let (ok, _, err) =
        run_pipeline(&[&dir.0.display().to_string(), "--steps", "normalize", "--dry-run"]);
    assert!(ok, "{err}");
    assert!(err.contains("== 1/1 normalize =="), "{err}");
    // Nothing was written, and the file is untouched.
    assert!(!dir.path("stems").exists());
}
