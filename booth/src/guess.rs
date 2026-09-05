//! What a file's own path says it is.
//!
//! A music library is filed by hand long before anything reads its tags:
//! `Peverelist/Roll With The Punches/02 - Roll With The Punches.flac` says who
//! made it, what it came off and what it is called, and it says so in a layout
//! that has been the same since people started keeping music in folders. A
//! program that ignores that and reports "unknown artist" is throwing away the
//! best evidence on the disk.
//!
//! It is evidence, not truth. Folders get named `New folder`, `320`,
//! `Downloads`; a stem can be a catalogue number. So every guess here carries
//! how much structure it actually found, and callers are expected to act on a
//! [`Strength::Strong`] one and to treat the rest as a hint.
//!
//! # What it reads
//!
//! - `<artist>/<album>/<n> - <title>` — the usual shape of a ripped album.
//! - `<artist>/<album>/<title>`
//! - `<artist>/<n> - <title>`
//! - `<artist> - <title>` in the file name, with or without a leading number,
//!   which is how nearly every download and promo arrives.
//! - `<n> - <artist> - <title>`, the same with the number in front.
//!
//! A directory whose name is a container rather than a name — `Music`,
//! `Downloads`, `FLAC`, `Disc 2`, `Various Artists` — is never taken for an
//! artist.

use std::path::Path;

/// How much of a guess came from structure rather than assumption.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Strength {
    /// Nothing usable. A bare stem with no separator and no useful folder
    /// above it.
    None,
    /// A title, and no confident artist to go with it.
    Weak,
    /// An artist and a title, each from somewhere that means one.
    Strong,
}

/// Names read out of a path.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Guess {
    pub artist: String,
    pub title: String,
    pub album: String,
    pub track_number: Option<u32>,
}

impl Guess {
    /// How much this is worth acting on.
    pub fn strength(&self) -> Strength {
        match (self.artist.trim().is_empty(), self.title.trim().is_empty()) {
            (false, false) => Strength::Strong,
            (true, false) => Strength::Weak,
            _ => Strength::None,
        }
    }

    /// Worth acting on without asking anyone.
    pub fn is_strong(&self) -> bool {
        self.strength() == Strength::Strong
    }

    pub fn describe(&self) -> String {
        let mut text = match self.artist.is_empty() {
            true => self.title.clone(),
            false => format!("{} — {}", self.artist, self.title),
        };
        if !self.album.is_empty() {
            text.push_str(&format!(" ({})", self.album));
        }
        text
    }
}

/// Folder names that are a filing system rather than a name.
const CONTAINERS: [&str; 26] = [
    "music",
    "musik",
    "downloads",
    "download",
    "incoming",
    "new",
    "unsorted",
    "sorted",
    "tracks",
    "songs",
    "audio",
    "media",
    "library",
    "itunes",
    "itunes media",
    "compilations",
    "various artists",
    "various",
    "va",
    "singles",
    "albums",
    "album",
    "eps",
    "mixes",
    "sets",
    "promo",
];

/// Whether a folder name is a container rather than somebody's name.
///
/// Formats and bitrates (`flac`, `320`), disc markers (`CD2`, `Disc 1`), years
/// on their own and anything that is only punctuation go the same way.
fn is_container(name: &str) -> bool {
    let lower = name.trim().to_lowercase();
    if lower.is_empty() || CONTAINERS.contains(&lower.as_str()) {
        return true;
    }
    if matches!(
        lower.as_str(),
        "flac" | "mp3" | "wav" | "aiff" | "aif" | "m4a" | "alac" | "lossless"
    ) {
        return true;
    }
    // A bitrate, a year, a disc: `320`, `1997`, `cd1`, `disc 2`, `disk two`.
    let digits = lower.trim_start_matches(|c: char| !c.is_ascii_digit());
    if !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) {
        let rest = lower[..lower.len() - digits.len()].trim();
        if rest.is_empty() || matches!(rest, "cd" | "disc" | "disk" | "vol" | "volume" | "part") {
            return true;
        }
    }
    !lower.chars().any(char::is_alphanumeric)
}

/// A leading track number, and what is left after it.
///
/// `02 - Title`, `02. Title`, `02_Title`, `02 Title` — but not `1999 - Title`,
/// where four digits are a year or part of the name, and not `12 Monkeys`,
/// where nothing separates the number from the word.
fn strip_number(stem: &str) -> (Option<u32>, String) {
    let digits: String = stem.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() || digits.len() > 3 {
        return (None, stem.to_string());
    }
    let rest = stem[digits.len()..].trim_start();
    let rest = match rest.strip_prefix(['-', '.', '_']) {
        Some(after) => after.trim_start(),
        // `07 Title` counts, but only because the number was separated from it
        // by a space; `07Title` is one word.
        None if stem[digits.len()..].starts_with(' ') => rest,
        None => return (None, stem.to_string()),
    };
    if rest.is_empty() {
        return (None, stem.to_string());
    }
    (digits.parse().ok(), rest.to_string())
}

/// Split on the separator people actually use, which is a hyphen with spaces
/// around it.
///
/// A hyphen without spaces belongs to the words either side of it — `Re-Up`,
/// `Jean-Michel` — and splitting on it turns names into nonsense.
fn split_on_dash(text: &str) -> Vec<String> {
    let mut parts: Vec<String> = text
        .split(" - ")
        .flat_map(|part| part.split(" – ")) // an en dash, which download sites like
        .map(|part| part.trim().to_string())
        .filter(|part| !part.is_empty())
        .collect();
    if parts.len() == 1 {
        parts = text
            .split('_')
            .map(|part| part.trim().to_string())
            .filter(|part| !part.is_empty())
            .collect();
        // Underscores only count when they are clearly doing the job of a
        // separator: `artist_title`, not `a_b_c_d_e`.
        if parts.len() != 2 {
            return vec![text.trim().to_string()];
        }
    }
    parts
}

/// Read what a path says about the track in it.
pub fn from_path(path: &Path) -> Guess {
    let mut guess = Guess::default();

    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let (number, stem) = strip_number(stem.trim());
    guess.track_number = number;

    // The folders above the file, nearest first, ignoring the ones that are a
    // filing system rather than a name.
    let folders: Vec<String> = path
        .parent()
        .map(|parent| {
            parent
                .components()
                .filter_map(|part| part.as_os_str().to_str())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let above: Vec<&String> = folders.iter().rev().take(2).collect();
    let parent = above.first().filter(|name| !is_container(name));
    let grandparent = above.get(1).filter(|name| !is_container(name));

    match split_on_dash(&stem).as_slice() {
        // `Artist - Title`, the commonest name a file arrives under.
        [artist, title] => {
            guess.artist = artist.clone();
            guess.title = title.clone();
        }
        // `Artist - Album - Title`, or a remixer in the middle. The ends are
        // the parts that mean the same thing in every one of those layouts.
        [artist, middle, title] => {
            guess.artist = artist.clone();
            guess.title = title.clone();
            guess.album = middle.clone();
        }
        // No separator: the folders have to say who it is.
        [title] => {
            guess.title = title.clone();
            match (parent, grandparent) {
                // `<artist>/<album>/<title>`
                (Some(album), Some(artist)) => {
                    guess.artist = (*artist).clone();
                    guess.album = (*album).clone();
                }
                // `<artist>/<title>`
                (Some(artist), None) => guess.artist = (*artist).clone(),
                _ => {}
            }
        }
        // Four or more parts is somebody's own scheme, and picking two of them
        // would be a guess about a guess.
        _ => guess.title = stem.clone(),
    }

    // An album from the folder when the name did not carry one, which is where
    // it usually is.
    if guess.album.is_empty() && !guess.artist.is_empty() {
        if let (Some(album), Some(_)) = (parent, grandparent) {
            guess.album = (*album).clone();
        }
    }

    guess.artist = tidy(&guess.artist);
    guess.title = tidy(&guess.title);
    guess.album = tidy(&guess.album);
    guess
}

/// Trim the decoration a download picks up on its way to a disk.
fn tidy(text: &str) -> String {
    let mut out = text.trim().to_string();
    // `Title (www.example.com)` and `[Free Download]`, which are somebody's
    // advertising rather than part of the name.
    for opener in ['(', '['] {
        let closer = match opener {
            '(' => ')',
            _ => ']',
        };
        if let Some(at) = out.rfind(opener) {
            let inside = out[at + 1..].trim_end_matches(closer).to_lowercase();
            let junk = inside.contains("www.")
                || inside.contains(".com")
                || inside.contains("free download")
                || inside.contains("320kbps")
                || inside.contains("hq");
            if junk {
                out.truncate(at);
            }
        }
    }
    out.trim().to_string()
}

/// Lower case, no punctuation, single spaces — the same shape [`crate::identify`]
/// compares names in.
fn simplify(text: &str) -> String {
    let mut out = String::new();
    let mut spaced = true;
    for ch in text.chars() {
        if ch.is_alphanumeric() {
            for lower in ch.to_lowercase() {
                out.push(lower);
            }
            spaced = false;
        } else if !spaced {
            out.push(' ');
            spaced = true;
        }
    }
    out.trim_end().to_string()
}

/// How much of a name a file's own name accounts for, from 0 to 1.
///
/// The share of the words in `artist` and `title` that appear in the file's
/// name. Word-wise rather than character-wise because the interesting failure
/// is a file called `track04.mp3` about to be tagged as somebody's single, and
/// no amount of character overlap makes that pair look alike.
pub fn resemblance(file_name: &str, artist: &str, title: &str) -> f64 {
    let name = simplify(file_name);
    let haystack: Vec<&str> = name.split(' ').filter(|word| !word.is_empty()).collect();
    let wanted = simplify(&format!("{artist} {title}"));
    let words: Vec<&str> = wanted.split(' ').filter(|word| word.len() > 1).collect();
    if words.is_empty() {
        return 1.0;
    }
    let found = words.iter().filter(|word| haystack.contains(*word)).count();
    found as f64 / words.len() as f64
}

/// Below this, a file's name and the names about to be written into it have so
/// little in common that it is worth a person looking.
///
/// A third: enough that `Peverelist - Roll With The Punches.flac` tagged as
/// *Roll With The Punches* passes on the title alone, and little enough that
/// `track04.mp3` tagged as anything at all does not.
pub const RESEMBLES: f64 = 0.34;

/// Whether the names about to be written match the file they are going into.
pub fn resembles(file_name: &str, artist: &str, title: &str) -> bool {
    resemblance(file_name, artist, title) >= RESEMBLES
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn guess(path: &str) -> Guess {
        from_path(&PathBuf::from(path))
    }

    #[test]
    fn an_album_folder_says_who_what_and_which_track() {
        let found = guess("/music/Peverelist/Tessellations/02 - Roll With The Punches.flac");
        assert_eq!(found.artist, "Peverelist");
        assert_eq!(found.album, "Tessellations");
        assert_eq!(found.title, "Roll With The Punches");
        assert_eq!(found.track_number, Some(2));
        assert!(found.is_strong());
    }

    #[test]
    fn a_file_named_artist_and_title_needs_no_folders_at_all() {
        let found = guess("/tmp/Peverelist - Roll With The Punches.mp3");
        assert_eq!(found.artist, "Peverelist");
        assert_eq!(found.title, "Roll With The Punches");
        assert!(found.is_strong());
    }

    #[test]
    fn a_leading_number_is_a_track_number_and_not_part_of_the_name() {
        assert_eq!(guess("/x/Artist/01 - Title.flac").title, "Title");
        assert_eq!(guess("/x/Artist/01. Title.flac").track_number, Some(1));
        assert_eq!(guess("/x/Artist/07 Title.flac").track_number, Some(7));
        // Four digits are a year or part of a name, not a track number.
        assert_eq!(guess("/x/Artist/1999 - Title.flac").track_number, None);
        // And a number with nothing between it and the word is one word.
        assert_eq!(guess("/x/Artist/12Monkeys.flac").title, "12Monkeys");
    }

    #[test]
    fn a_hyphen_inside_a_word_is_part_of_the_word() {
        // The bug this is here for: splitting on every hyphen turns names into
        // nonsense, and dance music is full of them.
        let found = guess("/x/Peverelist - Re-Up.flac");
        assert_eq!(found.artist, "Peverelist");
        assert_eq!(found.title, "Re-Up");
        assert_eq!(guess("/x/Artist/Jean-Michel Jarre.flac").title, "Jean-Michel Jarre");
    }

    #[test]
    fn a_folder_that_is_a_filing_system_is_not_an_artist() {
        for container in ["Downloads", "music", "FLAC", "320", "Various Artists", "CD2", "2019"] {
            let path = format!("/home/dj/{container}/Some Title.flac");
            let found = guess(&path);
            assert!(found.artist.is_empty(), "{container} was taken for an artist: {found:?}");
            assert_eq!(found.strength(), Strength::Weak);
        }
    }

    #[test]
    fn a_bare_name_in_a_bare_folder_says_nothing_worth_acting_on() {
        let found = guess("/Downloads/track04.mp3");
        assert_eq!(found.title, "track04");
        assert!(!found.is_strong(), "nothing here names an artist");
    }

    #[test]
    fn three_parts_are_read_from_the_ends_in() {
        // `Artist - Album - Title`, or a remixer in the middle: whatever the
        // middle is, the ends mean the same thing in all of them.
        let found = guess("/x/Peverelist - Livity Sound - Roll With The Punches.flac");
        assert_eq!(found.artist, "Peverelist");
        assert_eq!(found.title, "Roll With The Punches");
        assert_eq!(found.album, "Livity Sound");
    }

    #[test]
    fn advertising_is_trimmed_off_a_name() {
        assert_eq!(guess("/x/Artist - Title (www.example.com).mp3").title, "Title");
        assert_eq!(guess("/x/Artist - Title [Free Download].mp3").title, "Title");
        // And a parenthesis that is part of the name stays.
        assert_eq!(guess("/x/Artist - Title (Original Mix).mp3").title, "Title (Original Mix)");
    }

    #[test]
    fn a_file_name_that_shares_nothing_with_its_tags_is_noticed() {
        assert!(resembles(
            "Peverelist - Roll With The Punches.flac",
            "Peverelist",
            "Roll With The Punches"
        ));
        // The title alone is enough: plenty of files are named without the
        // artist, and that is not a warning.
        assert!(resembles("Roll With The Punches.flac", "Peverelist", "Roll With The Punches"));
        // This is the one worth stopping for.
        assert!(!resembles("track04.mp3", "Peverelist", "Roll With The Punches"));
        assert!(!resembles("06.flac", "Autechre", "Gantz Graf"));
    }

    #[test]
    fn resemblance_ignores_case_and_punctuation() {
        assert!(resembles(
            "peverelist_roll-with-the-punches.mp3",
            "Peverelist",
            "Roll With The Punches"
        ));
        assert_eq!(resemblance("Anything At All.mp3", "", ""), 1.0, "nothing to disagree with");
    }
}
