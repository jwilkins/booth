//! Optional backend that drives a locally installed `demucs`.
//!
//! Demucs is a neural separator and gets far cleaner results than the built-in
//! DSP pass, but it is a Python program you have to install yourself
//! (`pipx install demucs`). Nothing here downloads or installs anything; if
//! the binary is not on the machine, the caller is told to use `--backend dsp`.
//!
//! Demucs emits four stems — vocals, drums, bass, other — so its `bass` and
//! `other` are summed to make our `melody`.
//!
//! Demucs is asked for `--float32` output. Left to itself it writes 16-bit wav,
//! and a separated stem routinely peaks above full scale — the split
//! redistributes energy, so a stem can be louder than the mix it came from — so
//! demucs' own int16 writer clips it before we ever see it. Float carries the
//! peaks through intact; keeping the audio under full scale is then done once,
//! on the way out, where it can be measured rather than clamped.

use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{anyhow, bail, Context, Result};

use crate::audio::decode::decode_file;
use crate::audio::Audio;

use super::StemSet;

/// Where to find demucs and which weights to ask it for.
#[derive(Clone, Debug)]
pub struct Config {
    /// Command to run. Defaults to `demucs` on `PATH`.
    pub program: OsString,
    /// Pretrained model name passed to `-n`.
    pub model: String,
    /// Optional torch device (`cpu`, `cuda`, `mps`). `None` lets demucs pick.
    pub device: Option<String>,
    /// Test-time shifts: demucs separates the track several times at small
    /// random offsets and averages them, which smooths artefacts at a roughly
    /// linear cost in time. Demucs' own default is 0; more is cleaner.
    ///
    /// Ours is 2, which is a deliberate trade: a stem is rendered once and then
    /// played for years, so minutes spent here are cheap against a vocal that
    /// warbles every time it is used.
    pub shifts: u32,
    /// How much neighbouring windows overlap, 0.0 to just under 1.0. More
    /// overlap means fewer seams between windows, and more compute. Demucs'
    /// default is 0.25.
    pub overlap: f32,
    /// Directory demucs writes into. Callers pass a temporary directory.
    pub work_dir: PathBuf,
}

impl Config {
    pub fn new(work_dir: PathBuf) -> Self {
        Self {
            program: OsString::from("demucs"),
            // The fine-tuned model rather than the base one. It is four
            // specialist models rather than one, so it costs about four times
            // as long and separates noticeably better — which is the right way
            // round for a render that happens once.
            model: "htdemucs_ft".to_string(),
            device: None,
            shifts: 2,
            overlap: 0.25,
            work_dir,
        }
    }

    /// How many progress bars a run will draw.
    ///
    /// The fine-tuned models are a bag of four — one per stem — and demucs
    /// draws a bar for each, so a caller watching the raw percentage sees it
    /// reach 100 four times. Anything else is one model and one bar.
    ///
    /// Getting this wrong costs a bar that moves at the wrong speed, not a
    /// wrong answer: what is reported never goes backwards and never claims to
    /// have finished.
    pub fn passes(&self) -> u32 {
        match self.model.ends_with("_ft") {
            true => 4,
            false => 1,
        }
    }
}

/// Separate `input` by invoking demucs, reporting progress as it goes.
///
/// Takes the path rather than decoded audio because demucs reads files itself.
///
/// `on_progress` is called with a percentage, 0 to 99, as demucs works.
/// Separation is minutes a track, which is long enough that a caller with no
/// way to show how far along it is has nothing to show at all.
pub fn separate(input: &Path, config: &Config, on_progress: &dyn Fn(u8)) -> Result<StemSet> {
    let input = input.canonicalize().with_context(|| format!("resolving {}", input.display()))?;

    std::fs::create_dir_all(&config.work_dir)
        .with_context(|| format!("creating {}", config.work_dir.display()))?;

    let mut command = Command::new(&config.program);
    command
        .arg("-n")
        .arg(&config.model)
        .arg("--out")
        .arg(&config.work_dir)
        // Float output, so a stem that peaks above full scale reaches us
        // intact rather than clipped by demucs' default int16 writer.
        .arg("--float32")
        .arg("--overlap")
        .arg(format!("{}", config.overlap));
    if config.shifts > 0 {
        command.arg("--shifts").arg(config.shifts.to_string());
    }
    if let Some(device) = &config.device {
        command.arg("-d").arg(device);
    }
    command.arg(&input);

    // Piped rather than inherited so that a GUI caller does not have demucs
    // writing over its terminal, and so the progress bar can be read as it is
    // drawn. stdout is discarded: demucs puts everything worth diagnosing on
    // stderr, and buffering a stream nothing reads is how a child deadlocks.
    command.stdout(Stdio::null()).stderr(Stdio::piped());

    let mut child = command.spawn().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            anyhow!(
                "could not run {:?}: demucs is not installed or not on PATH. \
                 Install it with `pipx install demucs`, point at it with --demucs-bin, \
                 or use --backend dsp for the built-in separator.",
                config.program
            )
        } else {
            anyhow!("could not run {:?}: {e}", config.program)
        }
    })?;

    let stderr = match child.stderr.take() {
        Some(stream) => read_progress(stream, config.passes(), on_progress),
        None => String::new(),
    };
    let status = child.wait().map_err(|e| anyhow!("waiting for {:?}: {e}", config.program))?;

    if !status.success() {
        let last = stderr.trim().lines().last().unwrap_or("no output");
        match diagnose(&stderr) {
            Some(hint) => bail!("demucs exited with {status}: {last}\n\n{hint}"),
            None => bail!("demucs exited with {status}: {last}"),
        }
    }

    let stem_dir = locate_output(&config.work_dir, &config.model, &input)?;
    load_stems(&stem_dir)
}

/// Read demucs' stderr as it is written, reporting how far along it is, and
/// return the whole of it for the error path.
///
/// Demucs draws a tqdm bar, which means carriage returns rather than newlines:
/// reading by line would hand back nothing until the bar had finished. So this
/// reads bytes as they arrive and breaks on either.
///
/// `passes` is how many bars to expect. Each runs to 100% and then the next
/// begins, so a caller told the raw figure would watch it reset to zero
/// several times and conclude the work had started over.
fn read_progress(mut stream: impl Read, passes: u32, on_progress: &dyn Fn(u8)) -> String {
    let mut whole = String::new();
    let mut line = String::new();
    let mut buffer = [0u8; 4096];
    let mut finished_passes = 0u32;
    let mut last = 0.0f32;
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
                // A bar that has gone backwards is the next model in the bag
                // starting, not this one losing ground.
                if percent + 1.0 < last {
                    finished_passes += 1;
                }
                last = percent;
                let overall = (finished_passes as f32 + percent / 100.0) / passes.max(1) as f32;
                // Never backwards, and never quite finished: the work is done
                // when the process exits, not when the last bar fills.
                let overall = (overall * 100.0).clamp(reported as f32, 99.0) as u8;
                if overall > reported {
                    reported = overall;
                    on_progress(overall);
                }
            }
            line.clear();
        }
    }
    whole
}

/// The percentage at the head of a tqdm bar, if the line carries one.
///
/// tqdm writes ` 45%|####5     | 45.0/100.0 [...]`, so what is wanted is the
/// run of digits immediately before the first `%`.
fn percentage(line: &str) -> Option<f32> {
    let at = line.find('%')?;
    let start = line[..at].rfind(|c: char| !c.is_ascii_digit()).map_or(0, |i| i + 1);
    let percent: f32 = line[start..at].parse().ok()?;
    (percent <= 100.0).then_some(percent)
}

/// Recognise demucs failures whose cause is not obvious from the traceback,
/// and say what to do about them.
///
/// The traceback demucs prints for a missing numpy ends in a bare
/// `ModuleNotFoundError`, which reads like a broken Python install rather than
/// what it is: demucs imports numpy but does not list it as a dependency, and
/// torch no longer pulls it in, so *every* clean install of demucs 4.1.0 is
/// born broken until numpy is added alongside it.
fn diagnose(stderr: &str) -> Option<String> {
    let missing_numpy =
        stderr.contains("No module named 'numpy'") || stderr.contains("Failed to initialize NumPy");
    if !missing_numpy {
        return None;
    }

    Some(
        "demucs imports numpy but does not declare it as a dependency, so a clean install of \
         it cannot run. Add numpy to wherever demucs lives:\n\
         \n    uv tool install --force demucs --with numpy    # if you installed it with uv\n\
         \n    pipx inject demucs numpy                       # if you installed it with pipx\n\
         \n    pip install numpy                              # if it is in a plain virtualenv\n\
         \nThis is an upstream packaging bug, not a problem with your machine."
            .to_string(),
    )
}

/// Demucs writes to `<out>/<model>/<track name>/`, but the model directory is
/// not always named exactly as the `-n` argument was (a bag of models expands
/// to its own name). Look for the expected name first, then fall back to
/// whichever directory holds a `vocals` file for this track.
fn locate_output(work_dir: &Path, model: &str, input: &Path) -> Result<PathBuf> {
    let track = input.file_stem().ok_or_else(|| anyhow!("{} has no file name", input.display()))?;

    let expected = work_dir.join(model).join(track);
    if expected.is_dir() {
        return Ok(expected);
    }

    for entry in std::fs::read_dir(work_dir)
        .with_context(|| format!("listing {}", work_dir.display()))?
        .flatten()
    {
        let candidate = entry.path().join(track);
        if candidate.is_dir() {
            return Ok(candidate);
        }
    }

    bail!(
        "demucs finished but produced no output for {} under {}",
        track.to_string_lossy(),
        work_dir.display()
    )
}

/// Read demucs' four stems back in and fold them into our three.
fn load_stems(dir: &Path) -> Result<StemSet> {
    let vocals = read_stem(dir, "vocals")?;
    let drums = read_stem(dir, "drums")?;

    let mut melody = read_stem(dir, "other")?;
    // `bass` is a separate demucs stem but belongs with the accompaniment for
    // our three-way split. Some models do not emit it, which is fine.
    if let Ok(bass) = read_stem(dir, "bass") {
        melody.add_assign(&bass).context("mixing demucs' bass stem into the melody stem")?;
    }

    Ok(StemSet { vocals, melody, drums })
}

fn read_stem(dir: &Path, name: &str) -> Result<Audio> {
    // Demucs writes wav by default but can be asked for mp3 or flac.
    for extension in ["wav", "flac", "mp3"] {
        let path = dir.join(format!("{name}.{extension}"));
        if path.is_file() {
            return decode_file(&path);
        }
    }
    bail!("demucs produced no {name} stem in {}", dir.display())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// A tqdm bar, drawn the way demucs draws one: carriage returns, no
    /// newline until it is finished.
    fn bar(percents: &[u32]) -> String {
        percents
            .iter()
            .map(|p| format!("{p:3}%|##        | {p}.0/100.0 [00:01<00:09,  9.1seconds/s]\r"))
            .collect()
    }

    fn watch(text: &str, passes: u32) -> (Vec<u8>, String) {
        let seen = Mutex::new(Vec::new());
        let whole = read_progress(text.as_bytes(), passes, &|p| seen.lock().unwrap().push(p));
        (seen.into_inner().unwrap(), whole)
    }

    #[test]
    fn reads_the_percentage_off_a_bar_that_never_ends_a_line() {
        let (seen, whole) = watch(&bar(&[0, 25, 50, 75, 100]), 1);
        assert_eq!(seen, vec![25, 50, 75, 99], "0 is not progress, and 100 is not finished");
        assert!(whole.contains("9.1seconds/s"), "the text is kept for the error path");
    }

    #[test]
    fn four_bars_of_a_fine_tuned_model_are_one_run() {
        // htdemucs_ft is a bag of four models, so the raw percentage returns
        // to zero three times. What the caller sees must not.
        let mut text = String::new();
        for _ in 0..4 {
            text.push_str(&bar(&[0, 50, 100]));
        }
        let (seen, _) = watch(&text, 4);

        assert!(seen.windows(2).all(|w| w[0] < w[1]), "went backwards: {seen:?}");
        assert_eq!(seen.first(), Some(&12), "half of the first of four");
        assert!(seen.last().is_some_and(|p| *p >= 87), "the last bar should be near the end");
        assert!(
            seen.iter().all(|p| *p < 100),
            "demucs is done when it exits, not when a bar fills"
        );
    }

    #[test]
    fn a_line_with_no_percentage_is_not_progress() {
        let (seen, _) = watch("Separating track /music/a.flac\nSelected model is a bag of 4\n", 1);
        assert!(seen.is_empty(), "{seen:?}");
    }

    #[test]
    fn percentages_are_read_off_the_end_of_the_run_of_digits() {
        assert_eq!(percentage("  7%|#  | 7.0/100.0"), Some(7.0));
        assert_eq!(percentage("100%|###|"), Some(100.0));
        assert_eq!(percentage("no bar here"), None);
        // A stray % with nothing numeric before it is not a reading.
        assert_eq!(percentage("100% done, 50% left"), Some(100.0));
        assert_eq!(percentage("% "), None);
    }

    #[test]
    fn explains_the_undeclared_numpy_dependency() {
        // The traceback a real user hit. On its own it reads like a broken
        // Python install rather than an upstream packaging bug.
        let stderr = "\
            File \"/x/site-packages/demucs/transformer.py\", line 14, in <module>\n\
            import numpy as np\n\
            ModuleNotFoundError: No module named 'numpy'\n";

        let hint = diagnose(stderr).expect("missing numpy was not recognised");
        assert!(hint.contains("does not declare it as a dependency"), "{hint}");
        // One command for each way demucs is commonly installed.
        assert!(hint.contains("uv tool install --force demucs --with numpy"), "{hint}");
        assert!(hint.contains("pipx inject demucs numpy"), "{hint}");
        assert!(hint.contains("pip install numpy"), "{hint}");
        // And says plainly whose fault it is.
        assert!(hint.contains("not a problem with your machine"), "{hint}");
    }

    #[test]
    fn also_recognises_the_torch_warning_form() {
        // torch reports the same underlying problem in its own words, and it
        // can appear without the traceback.
        let stderr = "UserWarning: Failed to initialize NumPy: No module named 'numpy'";
        assert!(diagnose(stderr).is_some());
    }

    #[test]
    fn unrelated_failures_get_no_spurious_hint() {
        assert!(diagnose("RuntimeError: CUDA out of memory").is_none());
        assert!(diagnose("").is_none());
    }

    #[test]
    fn reports_a_missing_binary_helpfully() {
        let dir = std::env::temp_dir().join("musicai-demucs-missing");
        let mut config = Config::new(dir);
        config.program = OsString::from("definitely-not-a-real-demucs-binary");

        // Any readable file will do; we never get as far as decoding it.
        let err = separate(Path::new("Cargo.toml"), &config, &|_| {}).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("--backend dsp"), "unhelpful error: {message}");
    }

    #[test]
    fn finds_output_under_a_renamed_model_directory() {
        let root = std::env::temp_dir().join("musicai-demucs-layout");
        let _ = std::fs::remove_dir_all(&root);
        // Ask for "mdx_extra_q" but have demucs write "mdx_extra".
        std::fs::create_dir_all(root.join("mdx_extra").join("song")).unwrap();

        let found = locate_output(&root, "mdx_extra_q", Path::new("/music/song.mp3")).unwrap();
        assert_eq!(found, root.join("mdx_extra").join("song"));

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn errors_when_there_is_no_output_at_all() {
        let root = std::env::temp_dir().join("musicai-demucs-empty");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();

        let err = locate_output(&root, "htdemucs", Path::new("/music/song.mp3")).unwrap_err();
        assert!(err.to_string().contains("no output"));

        std::fs::remove_dir_all(&root).unwrap();
    }
}
