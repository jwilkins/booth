//! What a player will and will not open, in one place.
//!
//! The rules live here rather than in the preflight because the preflight is
//! the wrong moment to learn them. A file that will not play is worth knowing
//! about when it is added to the collection, when there is time to do
//! something; the check before a write is the last chance to catch one, not the
//! first.
//!
//! # Which player
//!
//! Not one set of rules but one per generation, because "a player" is not one
//! thing. The current line takes 96 kHz lossless; the nexus 2 line stops at 48
//! and the line before it has no FLAC at all. A library checked against a
//! CDJ-3000 and carried to a booth full of NXS2s is a library that passed every
//! check and will not load.
//!
//! So the question a drive is checked against is **the oldest player it has to
//! work on**, and the answer is [`Player`]. It is deliberately one small table:
//! the numbers come from AlphaTheta's published specifications for each model,
//! they change when firmware does, and a person with the hardware in front of
//! them should be able to correct one line of it.

use std::path::Path;

/// The oldest player a drive has to work on.
///
/// Ordered oldest-last, so `Player::ALL` reads newest first the way the
/// hardware is usually spoken about, and a variant further down the list is a
/// stricter set of rules than one above it. Each stands for its generation
/// rather than only for the model it is named after: the CDJ-2000NXS2 entry is
/// also the XDJ-1000MK2's rules, and the entry below it is the CDJ-2000nexus's
/// and the CDJ-900NXS's.
#[derive(
    Copy,
    Clone,
    Debug,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    serde::Serialize,
    serde::Deserialize,
)]
pub enum Player {
    /// The current line, and the only one that reads a OneLibrary database.
    Cdj3000X,
    /// The CDJ-3000 as shipped: three-band waveforms, phrases, 96 kHz.
    #[default]
    Cdj3000,
    /// The nexus 2 line. Plays FLAC and ALAC, and stops at 48 kHz.
    Cdj2000Nxs2,
    /// The nexus line and the original CDJ-2000. No lossless but WAV and AIFF.
    Cdj2000Nexus,
}

impl Player {
    /// Newest first, which is the order a picker should offer them in.
    pub const ALL: [Player; 4] =
        [Player::Cdj3000X, Player::Cdj3000, Player::Cdj2000Nxs2, Player::Cdj2000Nexus];

    pub fn name(self) -> &'static str {
        match self {
            Player::Cdj3000X => "CDJ-3000X",
            Player::Cdj3000 => "CDJ-3000",
            Player::Cdj2000Nxs2 => "CDJ-2000NXS2",
            Player::Cdj2000Nexus => "CDJ-2000NXS",
        }
    }

    /// What else in the same generation these rules cover.
    pub fn also(self) -> &'static str {
        match self {
            Player::Cdj3000X => "and the rest of the current line",
            Player::Cdj3000 => "CDJ-3000 and newer",
            Player::Cdj2000Nxs2 => "XDJ-1000MK2, XDJ-700, and the rest of the nexus 2 line",
            Player::Cdj2000Nexus => "CDJ-2000, CDJ-900NXS, and the nexus line",
        }
    }

    /// The highest sample rate it will load.
    ///
    /// The 96 kHz ceiling arrived with the CDJ-3000. Handing an NXS2 a 96 kHz
    /// FLAC is a track that copies, browses and will not load, which is the
    /// single most useful thing this table knows.
    pub fn max_sample_rate(self) -> u32 {
        match self {
            Player::Cdj3000X | Player::Cdj3000 => 96_000,
            Player::Cdj2000Nxs2 | Player::Cdj2000Nexus => 48_000,
        }
    }

    /// The file extensions it opens.
    ///
    /// FLAC and ALAC arrived with the nexus 2 line; before it there is lossless
    /// only as WAV and AIFF.
    pub fn plays(self) -> &'static [&'static str] {
        match self {
            Player::Cdj3000X | Player::Cdj3000 | Player::Cdj2000Nxs2 => {
                &["mp3", "flac", "wav", "aiff", "aif", "m4a", "aac"]
            }
            Player::Cdj2000Nexus => &["mp3", "wav", "aiff", "aif", "m4a", "aac"],
        }
    }

    /// Whether it will mount an exFAT drive.
    ///
    /// FAT32 is what everything reads, which is why it is what the writer
    /// formats an image as. This is for saying so about a stick somebody else
    /// formatted.
    pub fn reads_exfat(self) -> bool {
        matches!(self, Player::Cdj3000X | Player::Cdj3000)
    }

    /// Whether it reads the `exportLibrary.db` half of a drive.
    pub fn reads_onelibrary(self) -> bool {
        self == Player::Cdj3000X
    }

    /// Whether it draws the phrase bar under the waveform, out of `PSSI`.
    pub fn shows_phrases(self) -> bool {
        matches!(self, Player::Cdj3000X | Player::Cdj3000)
    }

    /// Whether it draws the three-band waveform out of the `.2EX` file.
    pub fn shows_three_band(self) -> bool {
        matches!(self, Player::Cdj3000X | Player::Cdj3000)
    }

    /// Whether this file's extension is one it opens.
    pub fn opens(self, extension: &str) -> bool {
        let extension = extension.to_ascii_lowercase();
        self.plays().contains(&extension.as_str())
    }
}

/// Why a file will not play on the hardware.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Problem {
    /// A format the player being checked against does not open.
    ///
    /// Which player is part of the problem and not context for it: a FLAC is a
    /// format players open, and the useful thing to say about one bound for a
    /// nexus deck is which deck will not take it.
    Format { extension: String, player: Player },
    /// A FairPlay purchase.
    Protected,
    /// 32-bit float samples, which look fine everywhere except a CDJ.
    FloatSamples,
    /// Above the ceiling of the player being checked against — 96 kHz on the
    /// current line, 48 on everything before it.
    ///
    /// The player rather than the number it implies, so the message can name
    /// the deck: "48 kHz" is a number somebody has to look up, and "the 48 kHz
    /// a CDJ-2000NXS2 will take" is the answer.
    TooFast { rate: u32, player: Player },
}

impl Problem {
    /// What is wrong, in the words a person would use about their own file.
    pub fn what(&self) -> String {
        match self {
            Problem::Format { extension, player } => {
                format!("a {} does not open a .{extension}", player.name())
            }
            Problem::Protected => "a protected purchase".to_string(),
            Problem::FloatSamples => "32-bit float WAV".to_string(),
            Problem::TooFast { rate, player } => format!(
                "{:.1} kHz, above the {:.0} kHz a {} will take",
                *rate as f64 / 1000.0,
                player.max_sample_rate() as f64 / 1000.0,
                player.name()
            ),
        }
    }

    /// What can be done about it.
    pub fn fix(&self) -> &'static str {
        match self {
            // A format the hardware will not take but this program can read:
            // for a nexus deck that is a FLAC, and FLAC is not the answer.
            Problem::Format { player, .. } if self.convertible() => match player.opens("flac") {
                true => "convert it to FLAC",
                false => "convert it to AIFF",
            },
            Problem::Format { .. } => "re-encode it elsewhere — this cannot decode it",
            // Not a limitation of this program: a protected file is encrypted,
            // and the only lawful way to a playable copy is to get one from
            // somewhere that sells them without the encryption. Saying so is
            // more use than an offer that would fail.
            Problem::Protected => "buy or rip an unprotected copy — this cannot convert it",
            Problem::FloatSamples => "convert it to 24-bit FLAC",
            Problem::TooFast { .. } => "resample it to 48 kHz in an editor first",
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
            Problem::Format { extension, .. } => {
                crate::discover::SUPPORTED.contains(&extension.as_str())
            }
            Problem::Protected | Problem::TooFast { .. } => false,
        }
    }
}

/// Everything wrong with one file, most serious first, or empty when it will
/// play as it is.
///
/// `float_samples` and `sample_rate` come from the scan that already read the
/// file's header, so this costs one small read for the MP4 case and nothing
/// otherwise.
pub fn problems(
    player: Player,
    path: &Path,
    extension: &str,
    sample_rate: u32,
    float_samples: bool,
) -> Vec<Problem> {
    let mut found = Vec::new();
    let extension = extension.to_ascii_lowercase();

    if is_mp4_container(&extension) && crate::audio::mp4::is_protected(path) {
        found.push(Problem::Protected);
    }
    if !player.opens(&extension) {
        found.push(Problem::Format { extension, player });
    }
    if float_samples {
        found.push(Problem::FloatSamples);
    }
    if sample_rate > player.max_sample_rate() {
        found.push(Problem::TooFast { rate: sample_rate, player });
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
        assert!(problems(Player::default(), nowhere(), "flac", 44_100, false).is_empty());
        assert!(problems(Player::default(), nowhere(), "mp3", 44_100, false).is_empty());
        assert!(problems(Player::default(), nowhere(), "wav", 96_000, false).is_empty());
        // Capitals are a fact about the file name, not about the format.
        assert!(
            problems(Player::default(), Path::new("/x/T.FLAC"), "FLAC", 44_100, false).is_empty()
        );
    }

    /// A CDJ-3000 plays AAC and ALAC, both of which live in `.m4a`. Warning
    /// about the container would be warning about the wrong thing — and would
    /// send somebody re-encoding a library that was already fine.
    #[test]
    fn an_m4a_is_a_format_the_hardware_plays() {
        assert!(problems(Player::default(), Path::new("/music/track.m4a"), "m4a", 44_100, false)
            .is_empty());
    }

    #[test]
    fn the_things_that_fail_are_named_and_only_some_can_be_converted() {
        // Nothing here decodes Vorbis, so it is reported and not offered a
        // conversion that would fail after making somebody wait for it.
        let format = problems(Player::default(), nowhere(), "ogg", 44_100, false);
        assert_eq!(
            format,
            vec![Problem::Format { extension: "ogg".into(), player: Player::default() }]
        );
        assert!(!format[0].convertible());
        assert!(format[0].fix().contains("cannot decode"));

        let float = problems(Player::default(), nowhere(), "wav", 44_100, true);
        assert_eq!(float, vec![Problem::FloatSamples]);
        assert!(float[0].convertible());

        // Resampling is a thing to be told about, not a thing to be done here.
        let fast = problems(Player::default(), nowhere(), "flac", 192_000, false);
        assert_eq!(fast, vec![Problem::TooFast { rate: 192_000, player: Player::default() }]);
        assert!(!fast[0].convertible());
        assert!(fast[0].what().contains("192.0 kHz"));
    }

    #[test]
    fn a_protected_purchase_is_reported_and_never_offered_a_conversion() {
        let problems =
            problems(Player::default(), Path::new("/music/bought.m4p"), "m4p", 44_100, false);
        assert!(problems.contains(&Problem::Protected));
        assert!(!Problem::Protected.convertible(), "there is nothing lawful to offer here");
    }

    #[test]
    fn a_ninety_six_kilohertz_flac_is_fine_on_a_3000_and_will_not_load_on_an_nxs2() {
        // The single most useful thing the table knows, and the failure it
        // exists to stop: the file copies, browses, and the deck refuses it.
        let fine = problems(Player::Cdj3000, nowhere(), "flac", 96_000, false);
        assert!(fine.is_empty(), "{fine:?}");

        let older = problems(Player::Cdj2000Nxs2, nowhere(), "flac", 96_000, false);
        assert_eq!(older, vec![Problem::TooFast { rate: 96_000, player: Player::Cdj2000Nxs2 }]);
        // Named, so nobody has to know which generation 48 kHz belongs to.
        assert!(older[0].what().contains("48 kHz a CDJ-2000NXS2 will take"), "{}", older[0].what());
        assert!(!older[0].convertible(), "resampling is a thing to be told about, not done here");
    }

    #[test]
    fn a_flac_is_not_a_format_every_generation_opens() {
        // FLAC and ALAC arrived with the nexus 2 line. Before it, lossless
        // means WAV or AIFF, and "not a format a player opens" would be a
        // baffling thing to say about a FLAC.
        let nexus = problems(Player::Cdj2000Nexus, nowhere(), "flac", 44_100, false);
        assert_eq!(
            nexus,
            vec![Problem::Format { extension: "flac".into(), player: Player::Cdj2000Nexus }]
        );
        assert!(
            nexus[0].what().contains("CDJ-2000NXS does not open a .flac"),
            "{}",
            nexus[0].what()
        );
        // And the way out is not the one offered everywhere else, because the
        // format it would convert to is the format that is the problem.
        assert_eq!(nexus[0].fix(), "convert it to AIFF");

        assert!(problems(Player::Cdj2000Nexus, nowhere(), "aiff", 48_000, false).is_empty());
        assert!(problems(Player::Cdj2000Nxs2, nowhere(), "flac", 48_000, false).is_empty());
    }

    #[test]
    fn an_older_player_is_never_a_looser_set_of_rules_than_a_newer_one() {
        // The property that makes "the oldest player it has to work on" a
        // sound way to ask the question: a drive that passes for an older
        // deck passes for every newer one. A table that broke this would make
        // the setting a guess.
        for pair in Player::ALL.windows(2) {
            let (newer, older) = (pair[0], pair[1]);
            assert!(
                older.max_sample_rate() <= newer.max_sample_rate(),
                "{} takes more than {}",
                older.name(),
                newer.name()
            );
            for extension in older.plays() {
                assert!(
                    newer.opens(extension),
                    "{} opens .{extension} and {} does not",
                    older.name(),
                    newer.name()
                );
            }
        }
    }

    #[test]
    fn what_a_drive_carries_for_a_player_that_cannot_read_it_is_still_written() {
        // Stated as a property rather than left implicit, because it is the
        // reason supporting an older deck costs nothing: the phrase section
        // and the three-band waveforms are files and sections an older player
        // does not look for, and writing them does not stop it reading the
        // ones it does.
        assert!(!Player::Cdj2000Nxs2.shows_phrases());
        assert!(!Player::Cdj2000Nxs2.shows_three_band());
        assert!(!Player::Cdj2000Nxs2.reads_onelibrary());
        assert!(Player::Cdj3000X.reads_onelibrary(), "and only it does");
        assert!(!Player::Cdj3000.reads_onelibrary());
        // exFAT is the one that is not a matter of ignoring a file: a drive an
        // older deck will not mount is a drive with nothing on it.
        assert!(!Player::Cdj2000Nxs2.reads_exfat());
        assert!(Player::Cdj3000.reads_exfat());
    }

    #[test]
    fn several_things_can_be_wrong_with_one_file() {
        let both = problems(Player::default(), nowhere(), "ogg", 192_000, false);
        assert_eq!(both.len(), 2, "{both:?}");
    }
}
