//! Optional backend that drives a locally installed `audio-separator` for a
//! RoFormer vocal stem.
//!
//! The band-split RoFormers — BS-RoFormer (Lu and others, ICASSP 2024) and
//! Mel-Band RoFormer — separate a voice about three decibels cleaner than the
//! Demucs family: 10.8 to 11.6 dB of vocal SDR on MUSDB18-HQ against 8.0 for
//! Hybrid Demucs, by the Mel-Band paper's own table, with nothing but the
//! benchmark's training set. Three decibels is the difference between a stem
//! that is quiet between the lines and one that is not, which is what the
//! words detector in [`crate::transcribe::cut`] lives on, and it is audible on
//! an acapella row.
//!
//! The released RoFormer weights split two ways, the voice and everything else,
//! and this program wants three stems. So there are two passes, both through
//! `audio-separator` (the `python-audio-separator` package, MIT): the RoFormer
//! takes the record to a voice and an instrumental, and `audio-separator`'s
//! own demucs takes the instrumental to drums, bass and other, which fold into
//! drums and melody the way [`super::demucs`] folds them. The instrumental
//! rather than the record, so the voice the RoFormer took out cannot leak back
//! into the melody, and so the three stems add back up to the record.
//!
//! Like the demucs backend, nothing here installs or downloads anything. The
//! program is Python with PyTorch behind it, installed as
//! `pipx install --python python3.12 "audio-separator[cpu]"` (`[gpu]` on an
//! NVIDIA card; the `[cpu]` extra is also the Apple Silicon one, where it
//! runs on the GPU through MPS; Python 3.12 or 3.13 because the beartype it
//! pins predates 3.14), it shells out to ffmpeg and will not start without
//! it, and
//! it fetches the weights it is asked for the first time it is asked, into the
//! directory [`Config::models_dir`] names. They are community-trained and come
//! without a stated licence, which is a reason to leave them where they are
//! rather than copy them anywhere.
//!
//! Two things it does that are worth knowing about. It works at 44.1 kHz
//! whatever it is given, resampling on the way in, so a record at another rate
//! gets its stems brought back to that rate here: a stem is compared with its
//! parent on a deck, and one at a different rate is a different file. And it
//! pulls anything over nine tenths of full scale down to that, the record on
//! the way in and each stem on the way out, before writing 16-bit audio; a
//! kit therefore comes back a shade under the record's level and never
//! clipped, and [`super::StemSet::fit_under`] has nothing left to do.
//!
//! # Measured here
//!
//! On the spoken lyric over a synthetic band that the words measurement is
//! made on — the only singing a container has — through this backend on four
//! CPU cores: the two passes took 29 minutes for 64 seconds of record, which
//! is why the GPU matters; the three stems summed back to the record with a
//! correlation of 0.997, a tenth of a decibel down; and the vocal stem was
//! −27 dBFS over the lines and digital silence — zero, not quiet — in the
//! intro, the break and the outro. The words detector found the eight spoken
//! stretches and nothing else, a quarter of the record, exactly as it does on
//! the voice before it was mixed, and the record cut by this stem read back
//! all eight lines within half a second of where they were spoken, at 19
//! percent of words wrong with `base.en`: the reading the ideal stem gave in
//! the words measurement, line for line, where the built-in separator's stem
//! gave two lines, neither in place. The weights came to 640 MB for the
//! RoFormer and 340 MB for `htdemucs_ft`. The ignored test `roformer_backend`
//! is the measurement.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{anyhow, bail, Context, Result};

use crate::audio::decode::decode_file;
use crate::audio::resample::resample;
use crate::audio::Audio;

use super::StemSet;

/// The RoFormer the voice is taken with when nothing says otherwise.
///
/// viperx's BS-RoFormer, epoch 317: the model `audio-separator` itself
/// defaults to, and the one at the top of its own vocal table at 12.9 dB.
pub const VOCAL_MODEL: &str = "model_bs_roformer_ep_317_sdr_12.9755.ckpt";

/// The demucs the rest is split with, at the high quality setting.
pub const REST_MODEL: &str = "htdemucs_ft.yaml";

/// Where to find `audio-separator` and which weights to ask it for.
#[derive(Clone, Debug)]
pub struct Config {
    /// Command to run. Defaults to `audio-separator` on `PATH`.
    pub program: OsString,
    /// The RoFormer model file for the voice: a `.ckpt` or `.yaml` name from
    /// `audio-separator --list_models`.
    pub vocal_model: String,
    /// The demucs model for drums, bass and other, likewise.
    pub rest_model: String,
    /// Test-time shifts for the demucs pass. See [`super::demucs::Config`].
    pub shifts: u32,
    /// Where `audio-separator` keeps the weights it downloads. `None` is
    /// [`Config::default_models_dir`]; its own default is under `/tmp`, which
    /// is a gigabyte fetched again after every reboot.
    pub models_dir: Option<PathBuf>,
    /// Directory the passes write into. Callers pass a temporary directory.
    pub work_dir: PathBuf,
}

impl Config {
    pub fn new(work_dir: PathBuf) -> Self {
        Self {
            program: OsString::from("audio-separator"),
            vocal_model: VOCAL_MODEL.to_string(),
            rest_model: REST_MODEL.to_string(),
            shifts: 2,
            models_dir: None,
            work_dir,
        }
    }

    /// `~/.cache/booth/models/audio-separator/`, honouring `XDG_CACHE_HOME`:
    /// beside the other weights this program is pointed at, and a place that
    /// survives a reboot.
    pub fn default_models_dir() -> Option<PathBuf> {
        let cache = std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .filter(|path| !path.as_os_str().is_empty())
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))?;
        Some(cache.join("booth").join("models").join("audio-separator"))
    }

    /// Where the weights go: the directory named, else the default.
    pub fn models_dir(&self) -> Option<PathBuf> {
        self.models_dir.clone().or_else(Self::default_models_dir)
    }

    /// How many progress bars the demucs pass draws. See
    /// [`super::demucs::Config::passes`].
    fn rest_passes(&self) -> u32 {
        match self.rest_model.contains("_ft") {
            true => 4,
            false => 1,
        }
    }
}

/// The two runs of the program, and what each is for.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Pass {
    /// The record to a voice and an instrumental.
    Voice,
    /// The instrumental to drums, bass and other.
    Rest,
}

impl Pass {
    fn model(self, config: &Config) -> &str {
        match self {
            Pass::Voice => &config.vocal_model,
            Pass::Rest => &config.rest_model,
        }
    }

    /// Where this pass writes, under the work directory.
    fn dir(self, config: &Config) -> PathBuf {
        config.work_dir.join(match self {
            Pass::Voice => "voice",
            Pass::Rest => "rest",
        })
    }

    /// The names the stems are asked to be written under, so they can be
    /// found without knowing how the program names files this week. Every
    /// stem any of its models emits is named, since a key for a stem a model
    /// does not produce costs nothing.
    fn output_names(self) -> &'static str {
        match self {
            Pass::Voice => r#"{"Vocals": "vocals", "Instrumental": "instrumental"}"#,
            Pass::Rest => {
                r#"{"Vocals": "vocals", "Drums": "drums", "Bass": "bass", "Other": "other", "Guitar": "guitar", "Piano": "piano"}"#
            }
        }
    }

    /// The share of the whole run this pass is, for one progress figure
    /// across both: the RoFormer and a bag of four demucs models take about
    /// as long as each other.
    fn progress_range(self) -> (u8, u8) {
        match self {
            Pass::Voice => (0, 50),
            Pass::Rest => (50, 99),
        }
    }
}

/// Separate `input` by invoking `audio-separator` twice, reporting progress as
/// it goes.
///
/// Takes the path rather than decoded audio because the program reads files
/// itself. `on_progress` is called with a percentage, 0 to 99, as the work
/// goes on; it is minutes a track on a CPU.
pub fn separate(input: &Path, config: &Config, on_progress: &dyn Fn(u8)) -> Result<StemSet> {
    let input = input.canonicalize().with_context(|| format!("resolving {}", input.display()))?;
    // Decoded for its rate and its length, which the stems come back at. A
    // second or two, against the minutes the separation takes.
    let record = decode_file(&input).with_context(|| format!("reading {}", input.display()))?;

    for pass in [Pass::Voice, Pass::Rest] {
        let dir = pass.dir(config);
        std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    if let Some(models) = config.models_dir() {
        std::fs::create_dir_all(&models)
            .with_context(|| format!("creating {}", models.display()))?;
    }

    run(config, Pass::Voice, &input, on_progress)?;
    let voice_dir = Pass::Voice.dir(config);
    let vocals = read_stem(&voice_dir, "vocals", "Vocals")?;
    let instrumental = find_stem(&voice_dir, "instrumental", "Instrumental")?;

    run(config, Pass::Rest, &instrumental, on_progress)?;
    let (drums, melody) = load_rest(&Pass::Rest.dir(config))?;

    Ok(StemSet {
        vocals: shaped_like(vocals, &record),
        melody: shaped_like(melody, &record),
        drums: shaped_like(drums, &record),
    })
}

/// The command for one pass, built where a test can read it back.
fn command(config: &Config, pass: Pass, input: &Path) -> Command {
    let mut command = Command::new(&config.program);
    command
        .arg(input)
        .arg("--model_filename")
        .arg(pass.model(config))
        .arg("--output_dir")
        .arg(pass.dir(config))
        // An intermediate, read straight back in: the cheapest thing to write
        // and to read.
        .arg("--output_format")
        .arg("WAV")
        .arg("--custom_output_names")
        .arg(pass.output_names());
    if let Some(models) = config.models_dir() {
        command.arg("--model_file_dir").arg(models);
    }
    if pass == Pass::Rest {
        command.arg("--demucs_shifts").arg(config.shifts.to_string());
    }
    command
}

fn run(config: &Config, pass: Pass, input: &Path, on_progress: &dyn Fn(u8)) -> Result<()> {
    let mut command = command(config, pass, input);
    // Piped for the reasons demucs' is: a window should not have the program
    // writing over its terminal, and a stream nothing reads is how a child
    // deadlocks. Its logging and its progress bars both go to stderr.
    command.stdout(Stdio::null()).stderr(Stdio::piped());

    let mut child = command.spawn().map_err(|e| missing(&config.program, e))?;
    let (from, to) = pass.progress_range();
    let passes = match pass {
        Pass::Voice => 1,
        Pass::Rest => config.rest_passes(),
    };
    let stderr = match child.stderr.take() {
        Some(stream) => super::demucs::read_progress(stream, passes, &|percent| {
            let span = (to - from) as f32;
            on_progress(from + (percent as f32 / 100.0 * span) as u8)
        }),
        None => String::new(),
    };
    let status = child.wait().map_err(|e| anyhow!("waiting for {:?}: {e}", config.program))?;
    if !status.success() {
        let last = stderr.trim().lines().last().unwrap_or("no output");
        match diagnose(&stderr, config) {
            Some(hint) => bail!("audio-separator exited with {status}: {last}\n\n{hint}"),
            None => bail!("audio-separator exited with {status}: {last}"),
        }
    }
    on_progress(to);
    Ok(())
}

/// What to say when the program is not there at all.
fn missing(program: &OsString, e: std::io::Error) -> anyhow::Error {
    if e.kind() != std::io::ErrorKind::NotFound {
        return anyhow!("could not run {program:?}: {e}");
    }
    anyhow!(
        "could not run {program:?}: audio-separator is not installed or not on PATH. Install it \
         with `pipx install \"audio-separator[cpu]\"` — `[gpu]` on an NVIDIA card — and point at \
         it with --roformer-bin if it lands somewhere else, or use --backend demucs or dsp."
    )
}

/// Recognise failures whose cause is not obvious from the traceback.
fn diagnose(stderr: &str, config: &Config) -> Option<String> {
    // Measured, on a Mac whose pipx ran Python 3.14: the RoFormer code checks
    // its own types with beartype, audio-separator pins beartype below 0.19,
    // and that beartype predates 3.14, where `Callable | None` became a
    // different kind of object at run time. It dies instantiating the model.
    if stderr.contains("unsupported by @beartype") {
        return Some(
            "audio-separator's RoFormer code checks its own types with beartype, and the \
             beartype it pins predates Python 3.14, where `Callable | None` became a different \
             kind of object at run time. Install it under Python 3.12 or 3.13 instead: \
             `uv tool install --python 3.12 \"audio-separator[cpu]\"`, or \
             `pipx install --python python3.12 \"audio-separator[cpu]\"` (`pipx reinstall` with \
             the same flag if it is already there)."
                .to_string(),
        );
    }
    // Measured: a fresh install refuses to start without it, before any model
    // is looked at, with a traceback that ends in this line.
    if stderr.contains("No such file or directory: 'ffmpeg'") {
        return Some(
            "audio-separator shells out to ffmpeg and will not start without it. Install \
             ffmpeg — `brew install ffmpeg`, or your package manager's — and run this again."
                .to_string(),
        );
    }
    if stderr.contains("No module named 'torch'")
        || stderr.contains("No module named 'onnxruntime'")
    {
        return Some(
            "audio-separator is installed without the runtime behind it. The extra is not \
             optional: install it as `audio-separator[cpu]` or `audio-separator[gpu]`."
                .to_string(),
        );
    }
    if stderr.contains("out of memory") {
        return Some(
            "the model did not fit in the GPU's memory. A shorter --mdxc_segment_size is \
             audio-separator's own answer; running it on the CPU is the other."
                .to_string(),
        );
    }
    let could_not_fetch = ["Max retries exceeded", "HTTPSConnectionPool", "Failed to download"]
        .iter()
        .any(|sign| stderr.contains(sign));
    if could_not_fetch {
        let where_ = config
            .models_dir()
            .map(|dir| dir.display().to_string())
            .unwrap_or_else(|| "its model directory".to_string());
        return Some(format!(
            "audio-separator fetches a model the first time it is asked for it, and could not. \
             It needs the network once per model; the files go in {where_}, and stay."
        ));
    }
    None
}

/// Read a stem the program wrote: under the name it was asked to use, or
/// failing that under its own naming, `<input>_(<Stem>)_<model>.<ext>`.
fn find_stem(dir: &Path, asked: &str, tag: &str) -> Result<PathBuf> {
    for extension in ["wav", "flac"] {
        let path = dir.join(format!("{asked}.{extension}"));
        if path.is_file() {
            return Ok(path);
        }
    }
    let marker = format!("({})", tag.to_lowercase());
    let mut found: Vec<PathBuf> = std::fs::read_dir(dir)
        .with_context(|| format!("listing {}", dir.display()))?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name().map(|name| name.to_string_lossy().to_lowercase()).is_some_and(|name| {
                name.contains(&marker) && (name.ends_with(".wav") || name.ends_with(".flac"))
            })
        })
        .collect();
    found.sort();
    found
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("audio-separator produced no {asked} stem in {}", dir.display()))
}

fn read_stem(dir: &Path, asked: &str, tag: &str) -> Result<Audio> {
    decode_file(&find_stem(dir, asked, tag)?)
}

/// Fold what the demucs pass wrote into drums and melody.
///
/// Melody is other and bass, as in the demucs backend, plus whatever else a
/// six-stem model puts out, plus any voice demucs still heard in the
/// instrumental: that is a residue the RoFormer left in the band, and it
/// belongs with the band rather than nowhere.
fn load_rest(dir: &Path) -> Result<(Audio, Audio)> {
    let drums = read_stem(dir, "drums", "Drums")?;
    let mut melody = read_stem(dir, "other", "Other")?;
    for (asked, tag) in
        [("bass", "Bass"), ("guitar", "Guitar"), ("piano", "Piano"), ("vocals", "Vocals")]
    {
        if let Ok(part) = read_stem(dir, asked, tag) {
            melody.add_assign(&part).with_context(|| format!("folding {asked} into the melody"))?;
        }
    }
    Ok((drums, melody))
}

/// A stem at the record's rate and length.
///
/// The program works at 44.1 kHz whatever it is given and may be a few frames
/// out either way; the stems of a record have its rate and its length, so a
/// deck can compare them, and so the three line up with one another.
fn shaped_like(mut stem: Audio, record: &Audio) -> Audio {
    if stem.sample_rate != record.sample_rate {
        let planes = stem
            .planes
            .iter()
            .map(|plane| resample(plane, stem.sample_rate, record.sample_rate))
            .collect();
        stem = Audio { sample_rate: record.sample_rate, planes };
    }
    for plane in &mut stem.planes {
        plane.resize(record.frames(), 0.0);
    }
    stem
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args_of(command: &Command) -> Vec<String> {
        command.get_args().map(|arg| arg.to_string_lossy().into_owned()).collect()
    }

    fn after<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
        args.iter().position(|arg| arg == flag).and_then(|at| args.get(at + 1)).map(String::as_str)
    }

    #[test]
    fn the_voice_pass_asks_the_roformer_for_the_voice_and_the_instrumental_by_name() {
        let mut config = Config::new(PathBuf::from("/work"));
        config.models_dir = Some(PathBuf::from("/models"));
        let args = args_of(&command(&config, Pass::Voice, Path::new("/music/song.flac")));

        assert_eq!(args[0], "/music/song.flac");
        assert_eq!(after(&args, "--model_filename"), Some(VOCAL_MODEL));
        assert_eq!(after(&args, "--output_dir"), Some("/work/voice"));
        assert_eq!(after(&args, "--output_format"), Some("WAV"));
        assert_eq!(after(&args, "--model_file_dir"), Some("/models"));
        let names = after(&args, "--custom_output_names").expect("the stems were not named");
        assert!(names.contains(r#""Vocals": "vocals""#), "{names}");
        assert!(names.contains(r#""Instrumental": "instrumental""#), "{names}");
        assert!(!args.contains(&"--demucs_shifts".to_string()), "no demucs in the voice pass");
    }

    #[test]
    fn the_rest_pass_runs_demucs_on_the_instrumental_with_the_shifts_asked_for() {
        let mut config = Config::new(PathBuf::from("/work"));
        config.shifts = 0;
        config.rest_model = "htdemucs.yaml".into();
        let args =
            args_of(&command(&config, Pass::Rest, Path::new("/work/voice/instrumental.wav")));

        assert_eq!(args[0], "/work/voice/instrumental.wav");
        assert_eq!(after(&args, "--model_filename"), Some("htdemucs.yaml"));
        assert_eq!(after(&args, "--output_dir"), Some("/work/rest"));
        assert_eq!(after(&args, "--demucs_shifts"), Some("0"));
        let names = after(&args, "--custom_output_names").unwrap();
        for stem in ["Drums", "Bass", "Other", "Guitar", "Piano", "Vocals"] {
            assert!(names.contains(&format!(r#""{stem}": "#)), "{stem} is not named in {names}");
        }
    }

    #[test]
    fn the_weights_live_beside_this_programs_other_models_unless_said_otherwise() {
        let config = Config::new(PathBuf::from("/work"));
        let dir = config.models_dir().expect("no home directory");
        assert!(dir.ends_with("booth/models/audio-separator"), "{}", dir.display());

        let mut named = Config::new(PathBuf::from("/work"));
        named.models_dir = Some(PathBuf::from("/elsewhere"));
        assert_eq!(named.models_dir(), Some(PathBuf::from("/elsewhere")));
    }

    #[test]
    fn a_fine_tuned_demucs_draws_four_bars_and_the_rest_draw_one() {
        let mut config = Config::new(PathBuf::from("/work"));
        assert_eq!(config.rest_passes(), 4, "{}", config.rest_model);
        config.rest_model = "htdemucs.yaml".into();
        assert_eq!(config.rest_passes(), 1);
    }

    /// Write a short mono wav, for tests that need a stem on disk.
    fn wav(path: &Path, samples: &[f32]) {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 44_100,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(path, spec).unwrap();
        for sample in samples {
            writer.write_sample((sample * i16::MAX as f32) as i16).unwrap();
        }
        writer.finalize().unwrap();
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("musicai-roformer-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_stem_is_found_under_the_name_asked_for_or_under_the_programs_own() {
        let dir = scratch("naming");
        // The program honoured the names it was given.
        wav(&dir.join("vocals.wav"), &[0.1; 100]);
        assert_eq!(find_stem(&dir, "vocals", "Vocals").unwrap(), dir.join("vocals.wav"));
        // Or it did not, and wrote its own: `<input>_(<Stem>)_<model>.wav`.
        let own = dir.join("song_(Instrumental)_model_bs_roformer_ep_317_sdr_12.9755.wav");
        wav(&own, &[0.1; 100]);
        assert_eq!(find_stem(&dir, "instrumental", "Instrumental").unwrap(), own);
        // And a stem that was never written is said to be missing.
        let err = find_stem(&dir, "drums", "Drums").unwrap_err();
        assert!(err.to_string().contains("no drums stem"), "{err}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_rest_folds_into_drums_and_a_melody_of_everything_else() {
        let dir = scratch("folding");
        wav(&dir.join("drums.wav"), &[0.5; 10]);
        wav(&dir.join("other.wav"), &[0.1; 10]);
        wav(&dir.join("bass.wav"), &[0.2; 10]);
        wav(&dir.join("guitar.wav"), &[0.1; 10]);
        // A trace of voice demucs heard in the instrumental stays in the band.
        wav(&dir.join("vocals.wav"), &[0.05; 10]);

        let (drums, melody) = load_rest(&dir).unwrap();
        assert!((drums.planes[0][0] - 0.5).abs() < 0.001, "{}", drums.planes[0][0]);
        assert!((melody.planes[0][0] - 0.45).abs() < 0.001, "{}", melody.planes[0][0]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_stem_comes_back_at_the_records_rate_and_length() {
        let record = Audio::new(48_000, vec![vec![0.0; 48_000]; 2]).unwrap();
        // What the program hands back: 44.1 kHz, a few frames short.
        let stem = Audio::new(44_100, vec![vec![0.25; 44_000]; 2]).unwrap();
        let shaped = shaped_like(stem, &record);
        assert_eq!(shaped.sample_rate, 48_000);
        assert_eq!(shaped.frames(), 48_000);
        assert_eq!(shaped.channels(), 2);
        // The audio came along: a steady level stays a steady level.
        assert!((shaped.planes[0][24_000] - 0.25).abs() < 0.01, "{}", shaped.planes[0][24_000]);
        // A stem already at the record's rate is only trimmed or padded.
        let long = Audio::new(48_000, vec![vec![0.5; 48_010]]).unwrap();
        assert_eq!(shaped_like(long, &record).frames(), 48_000);
    }

    #[test]
    fn a_missing_program_says_what_to_install_and_what_else_to_use() {
        let dir = scratch("missing");
        let input = dir.join("song.wav");
        wav(&input, &[0.1; 4_410]);
        let mut config = Config::new(dir.join("work"));
        config.program = OsString::from("definitely-not-a-real-audio-separator");
        config.models_dir = Some(dir.join("models"));

        let err = separate(&input, &config, &|_| {}).unwrap_err().to_string();
        assert!(err.contains("audio-separator[cpu]"), "{err}");
        assert!(err.contains("--backend demucs or dsp"), "{err}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_python_too_new_for_its_beartype_is_explained() {
        // What a Mac whose pipx ran Python 3.14 said, word for word: the model
        // never loads, and nothing in the message says which Python to use.
        let config = Config::new(PathBuf::from("/work"));
        let stderr = "RuntimeError: Failed to instantiate Roformer model: Method \
                      audio_separator.separator.uvr_lib_v5.roformer.bs_roformer.BSRoformer.__init__() \
                      parameter \"stft_window_fn\" type hint collections.abc.Callable | None either \
                      PEP-noncompliant or currently unsupported by @beartype.. This may be due to \
                      missing parameters or configuration validation failures.";
        let hint = diagnose(stderr, &config).expect("the beartype failure was not recognised");
        assert!(hint.contains("Python 3.12 or 3.13"), "{hint}");
        assert!(hint.contains("--python 3.12"), "{hint}");
    }

    #[test]
    fn a_missing_ffmpeg_is_explained() {
        // What a fresh install says, measured: it checks for ffmpeg before it
        // looks at a model, and dies on the check.
        let config = Config::new(PathBuf::from("/work"));
        let stderr = "    ffmpeg_version_output = subprocess.check_output([\"ffmpeg\", \"-version\"], text=True)\n\
                      FileNotFoundError: [Errno 2] No such file or directory: 'ffmpeg'\n";
        let hint = diagnose(stderr, &config).expect("ffmpeg was not recognised");
        assert!(hint.contains("brew install ffmpeg"), "{hint}");
    }

    #[test]
    fn a_runtime_that_is_missing_is_explained_and_so_is_a_fetch_that_failed() {
        let config = Config::new(PathBuf::from("/work"));
        let hint = diagnose("ModuleNotFoundError: No module named 'torch'", &config).unwrap();
        assert!(hint.contains("audio-separator[cpu]"), "{hint}");
        let hint = diagnose(
            "requests.exceptions.ConnectionError: HTTPSConnectionPool(host='huggingface.co')",
            &config,
        )
        .unwrap();
        assert!(hint.contains("once per model"), "{hint}");
        assert!(diagnose("RuntimeError: CUDA out of memory", &config).is_some());
        assert!(diagnose("ValueError: something else", &config).is_none());
    }

    #[test]
    fn progress_runs_from_the_voice_pass_into_the_rest_pass_without_going_back() {
        assert_eq!(Pass::Voice.progress_range(), (0, 50));
        assert_eq!(Pass::Rest.progress_range(), (50, 99));
    }
}
