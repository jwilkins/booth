//! Optional backend that drives a locally installed Whisper.
//!
//! Like the demucs backend, nothing here installs or downloads anything. Two
//! programs are recognised, because both are common and they are invoked quite
//! differently:
//!
//! * `whisper-cli` from whisper.cpp — a C++ binary and a model file on disk.
//!   It needs no Python and no network, which makes it the one to reach for.
//! * `whisper` from OpenAI — the Python reference implementation. It downloads
//!   its weights the first time it is run, so it is only offline afterwards.
//!
//! Either way the audio is handed over as 16 kHz mono, which is what the models
//! are trained on and the only thing whisper.cpp will read. The stem on disk is
//! 44.1 kHz stereo, so it is converted on the way — see [`resample`], which is
//! written for speech recognition and for nothing else.
//!
//! What comes back is timed lines. What to do with them is [`super`]'s problem.

use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{anyhow, bail, Context, Result};

use crate::audio::Audio;

use super::{Line, Transcript};

/// What the models are trained on, and all whisper.cpp will accept.
pub const SPEECH_RATE: u32 = 16_000;

/// Which program is being driven, which decides how it is called and what its
/// output looks like.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Flavour {
    /// whisper.cpp's `whisper-cli`.
    Cpp,
    /// OpenAI's `whisper`.
    Python,
}

impl Flavour {
    /// Which of the two a program name is, guessed from the name itself.
    ///
    /// OpenAI's is called exactly `whisper`; everything else in the family —
    /// `whisper-cli`, `whisper-cpp`, `main` from an older build — is
    /// whisper.cpp. Guessing wrong costs a run with the wrong flags and a clear
    /// error, and [`Config::flavour`] is there for when the guess is wrong.
    pub fn of(program: &Path) -> Self {
        match program.file_stem().map(|stem| stem.to_string_lossy().to_lowercase()) {
            Some(stem) if stem == "whisper" => Flavour::Python,
            _ => Flavour::Cpp,
        }
    }
}

/// Where to find Whisper and which weights to ask it for.
#[derive(Clone, Debug)]
pub struct Config {
    /// Command to run. Defaults to `whisper-cli` on `PATH`.
    pub program: OsString,
    /// The model. A path to a `.bin` for whisper.cpp, which has no default and
    /// cannot run without one; a name like `small` or `turbo` for the Python
    /// program, which will pick its own.
    pub model: Option<PathBuf>,
    /// The language to transcribe as, e.g. `en`. Left unset, Whisper decides
    /// for itself — which on a sung vocal with no words yet is a coin toss, so
    /// setting it is worth doing where the library is one language.
    pub language: Option<String>,
    /// Override the guess made from the program's name.
    pub flavour: Option<Flavour>,
    /// Directory the converted audio and the transcript are written into.
    /// Callers pass a temporary directory.
    pub work_dir: PathBuf,
}

impl Config {
    pub fn new(work_dir: PathBuf) -> Self {
        Self {
            program: OsString::from("whisper-cli"),
            model: None,
            language: None,
            flavour: None,
            work_dir,
        }
    }

    fn kind(&self) -> Flavour {
        self.flavour.unwrap_or_else(|| Flavour::of(Path::new(&self.program)))
    }
}

/// Transcribe a vocal stem.
///
/// `on_progress` is called with a percentage, 0 to 99, as the work goes on.
/// Transcription is a minute or two a track on a laptop, which is long enough
/// that a caller with nothing to show has nothing to show for a long time.
pub fn transcribe(stem: &Path, config: &Config, on_progress: &dyn Fn(u8)) -> Result<Transcript> {
    let audio = crate::audio::decode::decode_file(stem)
        .with_context(|| format!("reading {}", stem.display()))?;
    transcribe_audio(&audio, config, on_progress)
}

/// The same, for audio already in memory.
pub fn transcribe_audio(
    audio: &Audio,
    config: &Config,
    on_progress: &dyn Fn(u8),
) -> Result<Transcript> {
    let flavour = config.kind();
    // Before the audio is looked at, because a recogniser that is not set up
    // is not set up whatever it was handed — and a misconfiguration that only
    // showed itself on tracks that *do* have singing would be one somebody
    // finds out about in a booth.
    if flavour == Flavour::Cpp && config.model.is_none() {
        bail!(
            "whisper.cpp needs a model file and none is set. Download one — \
             ggml-base.en.bin is a good first choice — and point at it with \
             --whisper-model, or use OpenAI's `whisper` instead."
        );
    }
    // Then, before anything expensive: a stem with no voice on it is an
    // instrumental, and a recogniser handed one does not hand back an empty
    // transcript — it hallucinates. See [`super::align::has_singing`].
    if !super::align::has_singing(audio) {
        return Ok(Transcript::default());
    }

    std::fs::create_dir_all(&config.work_dir)
        .with_context(|| format!("creating {}", config.work_dir.display()))?;

    let speech = config.work_dir.join("words.wav");
    write_speech(audio, &speech)?;

    let mut command = Command::new(&config.program);
    match flavour {
        Flavour::Cpp => {
            // Checked above, before the audio was looked at.
            let model = config.model.as_ref().expect("a cpp config without a model got this far");
            command
                .arg("-m")
                .arg(model)
                .arg("-f")
                .arg(&speech)
                // JSON out, written next to the audio under a name we chose,
                // rather than parsed off the console: the console format is for
                // people and changes between releases.
                .arg("-oj")
                .arg("-of")
                .arg(config.work_dir.join("words"))
                .arg("--print-progress");
            if let Some(language) = &config.language {
                command.arg("-l").arg(language);
            }
        }
        Flavour::Python => {
            command
                .arg(&speech)
                .arg("--output_format")
                .arg("json")
                .arg("--output_dir")
                .arg(&config.work_dir);
            if let Some(model) = &config.model {
                command.arg("--model").arg(model);
            }
            if let Some(language) = &config.language {
                command.arg("--language").arg(language);
            }
        }
    }

    // Piped for the same reasons demucs' output is: a window should not have a
    // recogniser writing over its terminal, and a stream nothing reads is how a
    // child process deadlocks. Both programs put their progress on stderr.
    command.stdout(Stdio::null()).stderr(Stdio::piped());

    let mut child = command.spawn().map_err(|e| missing(&config.program, flavour, e))?;
    let stderr = match child.stderr.take() {
        Some(stream) => watch(stream, on_progress),
        None => String::new(),
    };
    let status = child.wait().map_err(|e| anyhow!("waiting for {:?}: {e}", config.program))?;

    if !status.success() {
        let last = stderr.trim().lines().last().unwrap_or("no output");
        match diagnose(&stderr) {
            Some(hint) => bail!("whisper exited with {status}: {last}\n\n{hint}"),
            None => bail!("whisper exited with {status}: {last}"),
        }
    }

    let written = config.work_dir.join("words.json");
    let json = std::fs::read_to_string(&written).with_context(|| {
        format!("whisper finished but wrote no transcript at {}", written.display())
    })?;
    // Put the lines where the singing is before anybody sees them. Whisper's
    // own timing is wrong on a stem in a way that is measurable and fixable,
    // and the fixing needs the audio — which nothing downstream has, because
    // what is kept in a collection is the words. See [`super::align`].
    Ok(super::align::aligned(read(&json, flavour)?, audio))
}

/// Turn whichever program's JSON into timed lines.
fn read(json: &str, flavour: Flavour) -> Result<Transcript> {
    let value: serde_json::Value =
        serde_json::from_str(json).context("whisper's transcript is not JSON")?;
    let segments = match flavour {
        Flavour::Cpp => value.get("transcription"),
        Flavour::Python => value.get("segments"),
    };
    let segments = segments.and_then(|s| s.as_array()).ok_or_else(|| {
        anyhow!("whisper's transcript has no segments in it — was it run with a different flavour?")
    })?;

    let mut lines = Vec::with_capacity(segments.len());
    for segment in segments {
        let text = segment.get("text").and_then(|t| t.as_str()).unwrap_or("").trim().to_string();
        if !is_speech(&text) {
            continue;
        }
        let (start_ms, end_ms) = match flavour {
            // whisper.cpp gives milliseconds outright.
            Flavour::Cpp => {
                let offsets = segment.get("offsets");
                let at = |name: &str| {
                    offsets.and_then(|o| o.get(name)).and_then(|v| v.as_u64()).unwrap_or(0) as u32
                };
                (at("from"), at("to"))
            }
            // The Python one gives seconds as floats.
            Flavour::Python => {
                let at = |name: &str| {
                    let seconds =
                        segment.get(name).and_then(|v| v.as_f64()).unwrap_or(0.0).max(0.0);
                    (seconds * 1000.0).round() as u32
                };
                (at("start"), at("end"))
            }
        };
        lines.push(Line { start_ms, end_ms: end_ms.max(start_ms), text });
    }
    Ok(Transcript { lines })
}

/// Whether a line is somebody singing rather than the recogniser describing
/// what it heard.
///
/// Whisper annotates what is not speech — `[Music]`, `(upbeat music)`,
/// `♪♪♪` — and on a vocal stem, which is mostly silence between phrases, it
/// does it often. Those are not lyrics, and left in they would group together
/// into the most repeated "line" in half the library.
fn is_speech(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return false;
    }
    let bracketed = (trimmed.starts_with('[') && trimmed.ends_with(']'))
        || (trimmed.starts_with('(') && trimmed.ends_with(')'))
        || (trimmed.starts_with('*') && trimmed.ends_with('*'));
    if bracketed {
        return false;
    }
    // A line of music notes, which is how a sung passage with no words comes
    // back, and which contains no letters at all.
    trimmed.chars().any(char::is_alphanumeric)
}

/// Read the program's output as it arrives, reporting how far along it is, and
/// keep the whole of it for the error path.
///
/// whisper.cpp writes `whisper_print_progress_callback: progress =  45%` on its
/// own line; the Python one draws a tqdm bar with carriage returns. Reading
/// bytes rather than lines copes with both.
fn watch(mut stream: impl Read, on_progress: &dyn Fn(u8)) -> String {
    let mut whole = String::new();
    let mut line = String::new();
    let mut buffer = [0u8; 4096];
    let mut reported = 0u8;

    loop {
        let read = match stream.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        let chunk = String::from_utf8_lossy(&buffer[..read]);
        whole.push_str(&chunk);
        for c in chunk.chars() {
            if c != '\r' && c != '\n' {
                line.push(c);
                continue;
            }
            if let Some(percent) = percentage(&line) {
                // Never backwards, and never quite finished: the work is done
                // when the process exits, not when the last figure is printed.
                let percent = (percent.round() as u8).clamp(reported, 99);
                if percent > reported {
                    reported = percent;
                    on_progress(percent);
                }
            }
            line.clear();
        }
    }
    whole
}

/// The percentage a line carries, if it carries one: the run of digits
/// immediately before the first `%`.
fn percentage(line: &str) -> Option<f32> {
    let at = line.find('%')?;
    let start = line[..at].rfind(|c: char| !c.is_ascii_digit()).map_or(0, |i| i + 1);
    let percent: f32 = line[start..at].parse().ok()?;
    (percent <= 100.0).then_some(percent)
}

/// What to say when the program is not there at all.
fn missing(program: &OsString, flavour: Flavour, e: std::io::Error) -> anyhow::Error {
    if e.kind() != std::io::ErrorKind::NotFound {
        return anyhow!("could not run {program:?}: {e}");
    }
    let install = match flavour {
        Flavour::Cpp => {
            "Install whisper.cpp — `brew install whisper-cpp`, or build it from source — \
             and download a model file for it."
        }
        Flavour::Python => "Install it with `pipx install openai-whisper`.",
    };
    anyhow!(
        "could not run {program:?}: it is not installed or not on PATH. {install} \
         Cue points from the words are the only thing that needs it; everything else \
         works without it."
    )
}

/// Recognise failures whose cause is not obvious from what is printed.
fn diagnose(stderr: &str) -> Option<String> {
    if stderr.contains("failed to initialize whisper context")
        || stderr.contains("failed to load model")
    {
        return Some(
            "whisper.cpp could not load the model file. Check the path, and check it is a \
             ggml model — the `.pt` files the Python program uses are a different format \
             and it cannot read them."
                .to_string(),
        );
    }
    if stderr.contains("ffmpeg") && stderr.contains("not found") {
        return Some(
            "OpenAI's whisper shells out to ffmpeg to read audio and cannot find it. \
             Install ffmpeg, or use whisper.cpp instead, which reads wav itself."
                .to_string(),
        );
    }
    None
}

/// Write audio out as the 16 kHz mono 16-bit wav the models want.
fn write_speech(audio: &Audio, to: &Path) -> Result<()> {
    let mono = audio.to_mono();
    let speech = resample(&mono, audio.sample_rate, SPEECH_RATE);

    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: SPEECH_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer =
        hound::WavWriter::create(to, spec).with_context(|| format!("creating {}", to.display()))?;
    for sample in speech {
        let clamped = (sample.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16;
        writer.write_sample(clamped)?;
    }
    writer.finalize().with_context(|| format!("finishing {}", to.display()))?;
    Ok(())
}

/// How many input samples either side of an output sample are read, at the
/// lower of the two rates. More is a sharper filter and a slower one.
const TAPS: f64 = 24.0;

/// Change a signal's sample rate, band-limited.
///
/// A windowed sinc, evaluated per output sample rather than built into a
/// polyphase table: a vocal stem is converted once, in a job that is about to
/// spend a minute in a neural network, so there is nothing to win by being
/// clever about it.
///
/// This is for feeding a recogniser and nothing else. It is honest about
/// aliasing — that is the whole reason it is not a bare pick-every-nth — but it
/// has had none of the care that writing somebody's library through a
/// resampler would need, and no audio anybody listens to goes through it.
fn resample(input: &[f32], from: u32, to: u32) -> Vec<f32> {
    if input.is_empty() || from == to || from == 0 || to == 0 {
        return input.to_vec();
    }
    let ratio = to as f64 / from as f64;
    // Cutoff in cycles per input sample. Below the output's Nyquist when going
    // down, with a little room for the filter to roll off in.
    let cutoff = 0.45 * ratio.min(1.0);
    let half = (TAPS / ratio.min(1.0)).ceil();
    let reach = half as isize;
    let out_len = (input.len() as f64 * ratio).floor() as usize;

    let mut out = Vec::with_capacity(out_len);
    for n in 0..out_len {
        let centre = n as f64 / ratio;
        let nearest = centre.floor() as isize;
        let mut sum = 0.0f64;
        let mut weight = 0.0f64;
        for i in (nearest - reach)..=(nearest + reach) {
            if i < 0 || i as usize >= input.len() {
                continue;
            }
            let x = i as f64 - centre;
            // Blackman window over the taps, so the stopband is deep enough
            // that what is filtered out stays out.
            let u = x / half;
            let window = 0.42
                + 0.5 * (std::f64::consts::PI * u).cos()
                + 0.08 * (2.0 * std::f64::consts::PI * u).cos();
            let tap = window * sinc(2.0 * cutoff * x);
            sum += tap * input[i as usize] as f64;
            weight += tap;
        }
        // Normalised by the weights actually used, which gives unity gain in
        // the middle and a sane answer at both ends rather than a fade.
        out.push(match weight.abs() > 1e-9 {
            true => (sum / weight) as f32,
            false => 0.0,
        });
    }
    out
}

fn sinc(x: f64) -> f64 {
    match x.abs() < 1e-9 {
        true => 1.0,
        false => {
            let pi_x = std::f64::consts::PI * x;
            pi_x.sin() / pi_x
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CPP_JSON: &str = r#"{
        "systeminfo": "AVX = 1",
        "model": { "type": "base" },
        "transcription": [
            { "timestamps": { "from": "00:00:10,000", "to": "00:00:12,500" },
              "offsets": { "from": 10000, "to": 12500 },
              "text": " Hold me closer now" },
            { "timestamps": { "from": "00:00:14,000", "to": "00:00:15,000" },
              "offsets": { "from": 14000, "to": 15000 },
              "text": " [Music]" },
            { "timestamps": { "from": "00:00:40,000", "to": "00:00:42,500" },
              "offsets": { "from": 40000, "to": 42500 },
              "text": " Hold me closer now" }
        ]
    }"#;

    const PYTHON_JSON: &str = r#"{
        "text": " Hold me closer now",
        "segments": [
            { "id": 0, "seek": 0, "start": 10.0, "end": 12.5, "text": " Hold me closer now" },
            { "id": 1, "seek": 0, "start": 14.0, "end": 15.0, "text": " (upbeat music)" },
            { "id": 2, "seek": 0, "start": 40.0, "end": 42.5, "text": " Hold me closer now" }
        ],
        "language": "en"
    }"#;

    #[test]
    fn reads_whisper_cpp_json() {
        let transcript = read(CPP_JSON, Flavour::Cpp).unwrap();
        assert_eq!(transcript.lines.len(), 2, "the music annotation is not a lyric");
        assert_eq!(transcript.lines[0].start_ms, 10_000);
        assert_eq!(transcript.lines[0].end_ms, 12_500);
        assert_eq!(transcript.lines[0].text, "Hold me closer now");
        assert_eq!(transcript.lines[1].start_ms, 40_000);
    }

    #[test]
    fn reads_openai_whisper_json() {
        let transcript = read(PYTHON_JSON, Flavour::Python).unwrap();
        assert_eq!(transcript.lines.len(), 2);
        // Seconds as floats, milliseconds on the way out.
        assert_eq!(transcript.lines[0].start_ms, 10_000);
        assert_eq!(transcript.lines[0].end_ms, 12_500);
        assert_eq!(transcript.lines[1].start_ms, 40_000);
    }

    #[test]
    fn both_flavours_find_the_same_hook() {
        let from_cpp = read(CPP_JSON, Flavour::Cpp).unwrap().hook().unwrap();
        let from_python = read(PYTHON_JSON, Flavour::Python).unwrap().hook().unwrap();
        assert_eq!(from_cpp, from_python);
        assert_eq!(from_cpp.at, vec![10_000, 40_000]);
    }

    #[test]
    fn the_wrong_flavour_is_an_error_rather_than_an_empty_transcript() {
        let err = read(CPP_JSON, Flavour::Python).unwrap_err();
        assert!(err.to_string().contains("different flavour"), "{err}");
    }

    /// Captured from whisper.cpp itself — `whisper-cli -m ggml-base.en.bin -oj`
    /// on five seconds of tone and noise, which is the shape of the problem:
    /// a stretch of a record with no words in it.
    ///
    /// Written down rather than described, because what a recogniser does with
    /// music is the thing every guard here is guarding against, and a fixture
    /// invented to match the guard proves only that it was written down twice.
    const NO_WORDS_IN_IT: &str = r#"{
        "systeminfo": "AVX = 1 | AVX2 = 1 | F16C = 1",
        "model": { "type": "base" },
        "params": { "model": "ggml-base.en.bin", "language": "en" },
        "result": { "language": "en" },
        "transcription": [
            { "timestamps": { "from": "00:00:00,000", "to": "00:00:02,240" },
              "offsets": { "from": 0, "to": 2240 },
              "text": " (gasping)" },
            { "timestamps": { "from": "00:00:02,240", "to": "00:00:04,580" },
              "offsets": { "from": 2240, "to": 4580 },
              "text": " (screaming)" }
        ]
    }"#;

    #[test]
    fn what_a_recogniser_makes_of_music_is_not_words() {
        // Both of those are the recogniser describing a noise, and neither is
        // a lyric. Left in, they would group together into the most repeated
        // "line" on every instrumental in the library and cue the hook to a
        // cymbal.
        let transcript = read(NO_WORDS_IN_IT, Flavour::Cpp).unwrap();
        assert!(transcript.lines.is_empty(), "{:?}", transcript.lines);
        assert!(transcript.is_empty());
        assert!(transcript.hook().is_none());
        assert!(transcript.moments().is_empty());
    }

    #[test]
    fn annotations_are_not_lyrics() {
        assert!(!is_speech("[Music]"));
        assert!(!is_speech(" (upbeat music) "));
        assert!(!is_speech("*laughs*"));
        assert!(!is_speech("♪♪♪"));
        assert!(!is_speech("   "));
        assert!(is_speech("hold me closer now"));
        // A line that merely mentions a bracket is still a line.
        assert!(is_speech("[Chorus] hold me closer now"));
    }

    #[test]
    fn the_program_name_says_which_one_it_is() {
        assert_eq!(Flavour::of(Path::new("/usr/local/bin/whisper")), Flavour::Python);
        assert_eq!(Flavour::of(Path::new("whisper-cli")), Flavour::Cpp);
        assert_eq!(Flavour::of(Path::new("/opt/whisper.cpp/build/bin/whisper-cli")), Flavour::Cpp);
        assert_eq!(Flavour::of(Path::new("whisper.exe")), Flavour::Python);
    }

    #[test]
    fn progress_is_read_off_either_programs_output() {
        let seen = std::sync::Mutex::new(Vec::new());
        let text = "whisper_print_progress_callback: progress =  15%\n\
                    whisper_print_progress_callback: progress =  45%\n\
                    whisper_print_progress_callback: progress = 100%\n";
        watch(text.as_bytes(), &|p| seen.lock().unwrap().push(p));
        assert_eq!(seen.into_inner().unwrap(), vec![15, 45, 99], "100% is not finished");
    }

    #[test]
    fn a_line_with_no_percentage_is_not_progress() {
        let seen = std::sync::Mutex::new(Vec::new());
        let whole = watch(
            "whisper_init_from_file_with_params_no_state: loading model\n".as_bytes(),
            &|p| seen.lock().unwrap().push(p),
        );
        assert!(seen.into_inner().unwrap().is_empty());
        assert!(whole.contains("loading model"), "the text is kept for the error path");
    }

    #[test]
    fn a_model_in_the_wrong_format_is_explained() {
        let hint = diagnose("whisper_init: failed to load model\n").expect("no hint");
        assert!(hint.contains("ggml model"), "{hint}");
    }

    #[test]
    fn a_missing_ffmpeg_is_explained() {
        let stderr = "FileNotFoundError: [Errno 2] No such file or directory: 'ffmpeg' not found";
        assert!(diagnose(stderr).is_some());
        assert!(diagnose("RuntimeError: CUDA out of memory").is_none());
    }

    #[test]
    fn a_stem_with_nothing_sung_on_it_never_reaches_the_recogniser() {
        // The binary does not exist, so reaching it would be an error. An
        // instrumental is answered before that and costs nothing.
        let dir = std::env::temp_dir().join("musicai-whisper-instrumental");
        let mut config = Config::new(dir);
        config.program = OsString::from("definitely-not-a-real-whisper-binary");
        config.model = Some(PathBuf::from("model.bin"));

        let audio = Audio::new(16_000, vec![vec![0.0; 16_000 * 4]]).unwrap();
        let heard = transcribe_audio(&audio, &config, &|_| {})
            .expect("an instrumental should not need a recogniser at all");
        assert!(heard.lines.is_empty(), "{heard:?}");
    }

    #[test]
    fn a_missing_binary_says_what_to_install() {
        let dir = std::env::temp_dir().join("musicai-whisper-missing");
        let mut config = Config::new(dir);
        config.program = OsString::from("definitely-not-a-real-whisper-binary");
        config.model = Some(PathBuf::from("model.bin"));

        // Audio with a voice on it: the binary is only reached for a stem
        // worth transcribing, and silence is now answered before that.
        let audio = Audio::new(16_000, vec![tone(220.0, 16_000, 1.0)]).unwrap();
        let err = transcribe_audio(&audio, &config, &|_| {}).unwrap_err();
        assert!(err.to_string().contains("not on PATH"), "{err}");
        assert!(err.to_string().contains("works without it"), "{err}");
    }

    #[test]
    fn whisper_cpp_without_a_model_says_so_before_running_anything() {
        // Including before deciding there is nothing to transcribe: silence
        // here, and it still says the recogniser is not set up.
        let config = Config::new(std::env::temp_dir().join("musicai-whisper-nomodel"));
        let audio = Audio::new(16_000, vec![vec![0.0; 16_000]]).unwrap();
        let err = transcribe_audio(&audio, &config, &|_| {}).unwrap_err();
        assert!(err.to_string().contains("needs a model file"), "{err}");
    }

    fn tone(hz: f32, rate: u32, secs: f32) -> Vec<f32> {
        let frames = (rate as f32 * secs) as usize;
        (0..frames)
            .map(|i| {
                let t = i as f32 / rate as f32;
                (2.0 * std::f32::consts::PI * hz * t).sin()
            })
            .collect()
    }

    fn energy(signal: &[f32]) -> f32 {
        signal.iter().map(|s| s * s).sum::<f32>() / signal.len().max(1) as f32
    }

    #[test]
    fn resampling_keeps_the_length_and_the_level() {
        let input = tone(440.0, 44_100, 1.0);
        let out = resample(&input, 44_100, SPEECH_RATE);

        assert!(out.len().abs_diff(SPEECH_RATE as usize) < 4, "{} samples", out.len());
        // A tone well inside the band comes through at the level it went in.
        let ratio = energy(&out[100..out.len() - 100]) / energy(&input);
        assert!((ratio - 1.0).abs() < 0.05, "level changed by a factor of {ratio:.3}");
    }

    #[test]
    fn resampling_does_not_fold_the_top_of_the_band_back_down() {
        // 15 kHz has nowhere to go at 16 kHz: taking every nth sample would
        // fold it down to 1 kHz and put a whistle right through the speech.
        let input = tone(15_000.0, 44_100, 1.0);
        let out = resample(&input, 44_100, SPEECH_RATE);

        let left = energy(&out[200..out.len() - 200]);
        assert!(left < 0.001, "{left:.5} of the 15 kHz tone survived as an alias");
    }

    #[test]
    fn resampling_a_signal_that_is_already_at_the_right_rate_leaves_it_alone() {
        let input = tone(440.0, SPEECH_RATE, 0.1);
        assert_eq!(resample(&input, SPEECH_RATE, SPEECH_RATE), input);
        assert!(resample(&[], 44_100, SPEECH_RATE).is_empty());
    }

    #[test]
    fn the_speech_file_is_mono_sixteen_bit_and_sixteen_kilohertz() {
        let dir = std::env::temp_dir().join("musicai-whisper-wav");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("words.wav");

        let left = tone(440.0, 44_100, 0.5);
        let right = left.iter().map(|s| -s).collect();
        let audio = Audio::new(44_100, vec![left, right]).unwrap();
        write_speech(&audio, &path).unwrap();

        let reader = hound::WavReader::open(&path).unwrap();
        let spec = reader.spec();
        assert_eq!(spec.channels, 1);
        assert_eq!(spec.sample_rate, SPEECH_RATE);
        assert_eq!(spec.bits_per_sample, 16);
        assert!(reader.duration().abs_diff(SPEECH_RATE / 2) < 8, "{} frames", reader.duration());

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
