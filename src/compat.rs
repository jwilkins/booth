//! What a player will and will not open, in one place.
//!
//! The rules are from AlphaTheta's own CDJ-3000 documentation, recorded in
//! §4.2 of the spec: MP3 and AAC at 16-bit and 44.1–48 kHz; WAV, AIFF, FLAC and
//! ALAC at 16 or 24-bit up to 96 kHz; no 32-bit float, no AIFF-C, no DRM'd AAC,
//! nothing above 96 kHz.
//!
//! They live here rather than in the preflight because the preflight is the
//! wrong moment to learn them. A file that will not play is worth knowing about
//! when it is added to the collection, when there is time to do something; the
//! check before a write is the last chance to catch one, not the first.

use std::path::Path;

/// Why a file will not play on the hardware.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Problem {
    /// A container no player opens.
    Format(String),
    /// A FairPlay purchase.
    Protected,
    /// 32-bit float samples, which look fine everywhere except a CDJ.
    FloatSamples,
    /// Above the 96 kHz ceiling.
    TooFast(u32),
}

impl Problem {
    /// What is wrong, in the words a person would use about their own file.
    pub fn what(&self) -> String {
        match self {
            Problem::Format(extension) => format!("a .{extension} is not a format a player opens"),
            Problem::Protected => "a protected purchase".to_string(),
            Problem::FloatSamples => "32-bit float WAV".to_string(),
            Problem::TooFast(rate) => {
                format!("{:.1} kHz, above the 96 kHz ceiling", *rate as f64 / 1000.0)
            }
        }
    }

    /// What can be done about it.
    pub fn fix(&self) -> &'static str {
        match self {
            Problem::Format(_) if self.convertible() => "convert it to FLAC",
            Problem::Format(_) => "re-encode it to FLAC elsewhere — this cannot decode it",
            // Not a limitation of this program: a protected file is encrypted,
            // and the only lawful way to a playable copy is to get one from
            // somewhere that sells them without the encryption. Saying so is
            // more use than an offer that would fail.
            Problem::Protected => "buy or rip an unprotected copy — this cannot convert it",
            Problem::FloatSamples => "convert it to 24-bit FLAC",
            Problem::TooFast(_) => "resample it to 48 kHz in an editor first",
        }
    }

    /// Whether converting the file here would actually fix it.
    ///
    /// Two things are deliberately not offered. Resampling, because there is no
    /// resampler in this program worth writing somebody's library through, and
    /// doing it badly once is permanent in a way that saying so is not. And a
    /// format nothing here decodes — an offer that would fail is worse than no
    /// offer, because it costs the time to find out.
    pub fn convertible(&self) -> bool {
        match self {
            Problem::FloatSamples => true,
            Problem::Format(extension) => {
                crate::discover::SUPPORTED.contains(&extension.as_str())
            }
            Problem::Protected | Problem::TooFast(_) => false,
        }
    }
}

/// Everything wrong with one file, most serious first, or empty when it will
/// play as it is.
///
/// `float_samples` and `sample_rate` come from the scan that already read the
/// file's header, so this costs one small read for the MP4 case and nothing
/// otherwise.
pub fn problems(path: &Path, extension: &str, sample_rate: u32, float_samples: bool) -> Vec<Problem> {
    let mut found = Vec::new();
    let extension = extension.to_ascii_lowercase();

    if is_mp4_container(&extension) && crate::audio::mp4::is_protected(path) {
        found.push(Problem::Protected);
    }
    if !crate::commands::is_playable(&extension) {
        found.push(Problem::Format(extension));
    }
    if float_samples {
        found.push(Problem::FloatSamples);
    }
    if sample_rate > 96_000 {
        found.push(Problem::TooFast(sample_rate));
    }
    found
}

fn is_mp4_container(extension: &str) -> bool {
    matches!(extension, "m4a" | "m4b" | "m4p" | "mp4" | "aac")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nowhere() -> &'static Path {
        Path::new("/does/not/exist.flac")
    }

    #[test]
    fn an_ordinary_file_has_nothing_wrong_with_it() {
        assert!(problems(nowhere(), "flac", 44_100, false).is_empty());
        assert!(problems(nowhere(), "mp3", 44_100, false).is_empty());
        assert!(problems(nowhere(), "wav", 96_000, false).is_empty());
        // Capitals are a fact about the file name, not about the format.
        assert!(problems(Path::new("/x/T.FLAC"), "FLAC", 44_100, false).is_empty());
    }

    /// A CDJ-3000 plays AAC and ALAC, both of which live in `.m4a`. Warning
    /// about the container would be warning about the wrong thing — and would
    /// send somebody re-encoding a library that was already fine.
    #[test]
    fn an_m4a_is_a_format_the_hardware_plays() {
        assert!(problems(Path::new("/music/track.m4a"), "m4a", 44_100, false).is_empty());
    }

    #[test]
    fn the_things_that_fail_are_named_and_only_some_can_be_converted() {
        // Nothing here decodes Vorbis, so it is reported and not offered a
        // conversion that would fail after making somebody wait for it.
        let format = problems(nowhere(), "ogg", 44_100, false);
        assert_eq!(format, vec![Problem::Format("ogg".into())]);
        assert!(!format[0].convertible());
        assert!(format[0].fix().contains("cannot decode"));

        let float = problems(nowhere(), "wav", 44_100, true);
        assert_eq!(float, vec![Problem::FloatSamples]);
        assert!(float[0].convertible());

        // Resampling is a thing to be told about, not a thing to be done here.
        let fast = problems(nowhere(), "flac", 192_000, false);
        assert_eq!(fast, vec![Problem::TooFast(192_000)]);
        assert!(!fast[0].convertible());
        assert!(fast[0].what().contains("192.0 kHz"));
    }

    #[test]
    fn a_protected_purchase_is_reported_and_never_offered_a_conversion() {
        let problems = problems(Path::new("/music/bought.m4p"), "m4p", 44_100, false);
        assert!(problems.contains(&Problem::Protected));
        assert!(!Problem::Protected.convertible(), "there is nothing lawful to offer here");
    }

    #[test]
    fn several_things_can_be_wrong_with_one_file() {
        let both = problems(nowhere(), "ogg", 192_000, false);
        assert_eq!(both.len(), 2, "{both:?}");
    }
}
