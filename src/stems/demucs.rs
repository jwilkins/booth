//! Optional backend that drives a locally installed `demucs`.
//!
//! Demucs is a neural separator and gets far cleaner results than the built-in
//! DSP pass, but it is a Python program you have to install yourself
//! (`pipx install demucs`). Nothing here downloads or installs anything; if
//! the binary is not on the machine, the caller is told to use `--backend dsp`.
//!
//! Demucs emits four stems — vocals, drums, bass, other — so its `bass` and
//! `other` are summed to make our `melody`.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

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
    /// Directory demucs writes into. Callers pass a temporary directory.
    pub work_dir: PathBuf,
}

impl Config {
    pub fn new(work_dir: PathBuf) -> Self {
        Self {
            program: OsString::from("demucs"),
            model: "htdemucs".to_string(),
            device: None,
            work_dir,
        }
    }
}

/// Separate `input` by invoking demucs.
///
/// Takes the path rather than decoded audio because demucs reads files itself.
pub fn separate(input: &Path, config: &Config) -> Result<StemSet> {
    let input = input.canonicalize().with_context(|| format!("resolving {}", input.display()))?;

    std::fs::create_dir_all(&config.work_dir)
        .with_context(|| format!("creating {}", config.work_dir.display()))?;

    let mut command = Command::new(&config.program);
    command.arg("-n").arg(&config.model).arg("--out").arg(&config.work_dir).arg(&input);
    if let Some(device) = &config.device {
        command.arg("-d").arg(device);
    }

    let output = command.output().map_err(|e| {
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

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let last = stderr.trim().lines().last().unwrap_or("no output");
        match diagnose(&stderr) {
            Some(hint) => bail!("demucs exited with {}: {last}\n\n{hint}", output.status),
            None => bail!("demucs exited with {}: {last}", output.status),
        }
    }

    let stem_dir = locate_output(&config.work_dir, &config.model, &input)?;
    load_stems(&stem_dir)
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
        let err = separate(Path::new("Cargo.toml"), &config).unwrap_err();
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
