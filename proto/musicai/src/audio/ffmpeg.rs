//! Windows Media Audio, through ffmpeg when it is installed.
//!
//! Nothing written in Rust decodes WMA, and no player opens it, so a WMA in a
//! collection is a file waiting to be converted. ffmpeg decodes every version
//! of it there is. It is not bundled: it is large, it is a separate install on
//! every platform, and the RoFormer separator already needs it, so a DJ with
//! WMA files is told to install it rather than carrying it for everybody.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

use anyhow::{anyhow, bail, Context, Result};

use super::Audio;

/// The extensions decoded here rather than by symphonia.
pub const EXTENSIONS: [&str; 1] = ["wma"];

/// Whether a file is one only ffmpeg decodes.
pub fn needed_for(extension: &str) -> bool {
    EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str())
}

/// Where ffmpeg is, looked for once a run.
///
/// `BOOTH_FFMPEG` names one outright. Then `PATH`, then the two places Homebrew
/// puts it, because an app started from the Finder inherits launchd's `PATH`,
/// `/usr/bin:/bin:/usr/sbin:/sbin`, rather than the shell's, and Homebrew is
/// how most Macs have ffmpeg.
pub fn find() -> Option<&'static Path> {
    static FOUND: OnceLock<Option<PathBuf>> = OnceLock::new();
    FOUND
        .get_or_init(|| {
            if let Some(named) = std::env::var_os("BOOTH_FFMPEG") {
                let named = PathBuf::from(named);
                return named.is_file().then_some(named);
            }
            crate::stems::install::find_executable("ffmpeg").or_else(|| {
                ["/opt/homebrew/bin/ffmpeg", "/usr/local/bin/ffmpeg"]
                    .into_iter()
                    .map(PathBuf::from)
                    .find(|path| path.is_file())
            })
        })
        .as_deref()
}

/// What to say when a file needs ffmpeg and there is none.
pub fn missing(path: &Path) -> anyhow::Error {
    anyhow!(
        "{} is WMA, which only ffmpeg decodes here, and no ffmpeg was found: install it \
         (`brew install ffmpeg`, or your package manager's) and try again",
        path.display()
    )
}

/// Decode a whole file through ffmpeg, at its own rate and channel count.
///
/// ffmpeg is asked for 32-bit float WAV on its standard output, so nothing is
/// lost on the way and no temporary file is left behind if it fails.
pub fn decode(path: &Path) -> Result<Audio> {
    let ffmpeg = find().ok_or_else(|| missing(path))?;
    let output = Command::new(ffmpeg)
        .args(["-nostdin", "-v", "error", "-i"])
        .arg(path)
        .args(["-map", "0:a:0", "-c:a", "pcm_f32le", "-f", "wav", "-"])
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("running {}", ffmpeg.display()))?;
    if !output.status.success() {
        let said = String::from_utf8_lossy(&output.stderr);
        bail!(
            "ffmpeg could not decode {}: {}",
            path.display(),
            said.lines().last().unwrap_or("it gave no reason")
        );
    }
    float_wav(&output.stdout)
        .with_context(|| format!("reading what ffmpeg made of {}", path.display()))
}

/// A float WAV as ffmpeg writes one to a pipe.
///
/// Written to a pipe, it cannot go back and fill in the lengths, so the RIFF
/// and data sizes are not to be trusted: the data runs to the end of what was
/// written, whatever its header says.
fn float_wav(bytes: &[u8]) -> Result<Audio> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        bail!("not a WAV");
    }
    let word = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
    let half = |at: usize| u16::from_le_bytes(bytes[at..at + 2].try_into().unwrap());
    let mut at = 12;
    let mut format: Option<(u16, u32, u16)> = None;
    while at + 8 <= bytes.len() {
        let id = &bytes[at..at + 4];
        let size = word(at + 4) as usize;
        let body = at + 8;
        if id == b"data" {
            let (channels, rate, bits) = format.ok_or_else(|| anyhow!("no fmt chunk"))?;
            if bits != 32 {
                bail!("{bits}-bit samples where 32-bit floats were asked for");
            }
            let end = match body.checked_add(size) {
                Some(end) if end <= bytes.len() => end,
                _ => bytes.len(),
            };
            let channels = channels as usize;
            let samples: Vec<f32> = bytes[body..end]
                .chunks_exact(4)
                .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
                .collect();
            let frames = samples.len() / channels;
            let planes = (0..channels)
                .map(|ch| (0..frames).map(|f| samples[f * channels + ch]).collect())
                .collect();
            return Audio::new(rate, planes);
        }
        if id == b"fmt " {
            if body + 16 > bytes.len() {
                bail!("a fmt chunk cut short");
            }
            // 3 is IEEE float; 0xFFFE is extensible, which ffmpeg uses past two
            // channels, and whose sub-format starts with the same 3.
            let tag = match half(body) {
                0xFFFE if body + 26 <= bytes.len() => half(body + 24),
                tag => tag,
            };
            if tag != 3 {
                bail!("format {tag} where IEEE float was asked for");
            }
            format = Some((half(body + 2), word(body + 4), half(body + 14)));
        }
        at = body + size + size % 2;
    }
    bail!("no data chunk")
}

/// The tags ffmpeg reads from a file, as its own lower-case keys: `title`,
/// `artist`, `album_artist`, `date`, `track` and the rest.
pub fn tags(path: &Path) -> Result<Vec<(String, String)>> {
    let ffmpeg = find().ok_or_else(|| missing(path))?;
    let output = Command::new(ffmpeg)
        .args(["-nostdin", "-v", "error", "-i"])
        .arg(path)
        .args(["-f", "ffmetadata", "-"])
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("running {}", ffmpeg.display()))?;
    if !output.status.success() {
        bail!("ffmpeg could not read the tags of {}", path.display());
    }
    Ok(ffmetadata(&String::from_utf8_lossy(&output.stdout)))
}

/// ffmpeg's metadata format: a header line, then `key=value` until the first
/// `[STREAM]` or `[CHAPTER]` section, with `=`, `;`, `#`, `\` and newlines in a
/// value escaped by a backslash.
fn ffmetadata(text: &str) -> Vec<(String, String)> {
    let mut found = Vec::new();
    let mut lines = text.lines().skip(1).peekable();
    while let Some(line) = lines.next() {
        if line.starts_with('[') {
            break;
        }
        if line.starts_with(';') || line.starts_with('#') {
            continue;
        }
        let mut whole = line.to_string();
        // A value ending in an unescaped backslash carries on onto the next line.
        while whole.ends_with('\\') && !whole.ends_with("\\\\") {
            whole.pop();
            whole.push('\n');
            match lines.next() {
                Some(next) => whole.push_str(next),
                None => break,
            }
        }
        let mut key = String::new();
        let mut value = String::new();
        let mut in_value = false;
        let mut chars = whole.chars();
        while let Some(c) = chars.next() {
            match (c, in_value) {
                ('\\', _) => {
                    if let Some(escaped) = chars.next() {
                        if in_value {
                            value.push(escaped)
                        } else {
                            key.push(escaped)
                        }
                    }
                }
                ('=', false) => in_value = true,
                (c, true) => value.push(c),
                (c, false) => key.push(c),
            }
        }
        if in_value && !key.is_empty() {
            found.push((key.to_ascii_lowercase(), value));
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_metadata_listing_reads_as_keys_and_values_with_its_escapes_undone() {
        let text = ";FFMETADATA1\ntitle=Two \\= Tones\nartist=Booth Test\ntrack=2/9\n\
                    comment=one\\\ntwo\n[STREAM]\ntitle=not this\n";
        assert_eq!(
            ffmetadata(text),
            [
                ("title".to_string(), "Two = Tones".to_string()),
                ("artist".to_string(), "Booth Test".to_string()),
                ("track".to_string(), "2/9".to_string()),
                ("comment".to_string(), "one\ntwo".to_string()),
            ]
        );
    }

    #[test]
    fn a_float_wav_from_a_pipe_reads_to_its_end_whatever_its_sizes_say() {
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&u32::MAX.to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        for half in [3u16, 2] {
            wav.extend_from_slice(&half.to_le_bytes());
        }
        wav.extend_from_slice(&44_100u32.to_le_bytes());
        wav.extend_from_slice(&(44_100u32 * 8).to_le_bytes());
        for half in [8u16, 32] {
            wav.extend_from_slice(&half.to_le_bytes());
        }
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&u32::MAX.to_le_bytes());
        for sample in [0.5f32, -0.5, 0.25, -0.25] {
            wav.extend_from_slice(&sample.to_le_bytes());
        }
        let audio = float_wav(&wav).unwrap();
        assert_eq!(audio.sample_rate, 44_100);
        assert_eq!(audio.planes, [vec![0.5, 0.25], vec![-0.5, -0.25]]);
    }
}
