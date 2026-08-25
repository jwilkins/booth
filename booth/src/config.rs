//! What the user has decided, as opposed to what the collection contains.
//!
//! Kept apart from the collection because it is a different kind of thing: the
//! collection describes music, and this describes how this machine should treat
//! it. Copying a library between machines should not bring the first machine's
//! idea of where its music folder is.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::library::data_dir;

/// What to do about a file that is not in the library folder.
///
/// A track played from a download folder, a network share or someone else's
/// stick is a track that will be missing the night it matters. The default is
/// to take a copy, because the cost of being wrong in that direction is some
/// disk, and the cost of being wrong in the other is an empty deck.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum OnExternal {
    /// Copy it in, without asking.
    #[default]
    Copy,
    /// Ask, each time some are found.
    Ask,
    /// Leave it where it is, and refer to it there.
    Leave,
}

impl OnExternal {
    pub const ALL: [OnExternal; 3] = [OnExternal::Copy, OnExternal::Ask, OnExternal::Leave];

    pub fn label(self) -> &'static str {
        match self {
            OnExternal::Copy => "Copy into the library",
            OnExternal::Ask => "Ask me",
            OnExternal::Leave => "Leave it where it is",
        }
    }

    pub fn blurb(self) -> &'static str {
        match self {
            OnExternal::Copy => {
                "Anything added from outside is copied in, so the collection is \
                 self-contained and a drive can always be written."
            }
            OnExternal::Ask => "A list of what is outside, each time some turns up.",
            OnExternal::Leave => {
                "Nothing is copied. A file that moves or unmounts becomes a track that \
                 cannot be written."
            }
        }
    }
}

/// Whether a track's length reads as bars or as beats.
///
/// Bars by default, because that is the unit a set is built in: an intro is
/// eight bars, a phrase is sixteen, and a DJ counting beats is doing arithmetic
/// nobody asked for.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Length {
    #[default]
    Bars,
    Beats,
}

impl Length {
    pub const ALL: [Length; 2] = [Length::Bars, Length::Beats];

    pub fn label(self) -> &'static str {
        match self {
            Length::Bars => "bars",
            Length::Beats => "beats",
        }
    }

    /// How long a track is, in whichever unit this is.
    ///
    /// Four beats to the bar. Everything else here assumes 4/4, because the
    /// format the drive is written in does too — its beat numbers run 1 to 4 —
    /// so a library that counted 3/4 bars would be telling the truth about the
    /// music and lying about the drive.
    pub fn count(self, beats: usize) -> usize {
        match self {
            Length::Bars => beats / 4,
            Length::Beats => beats,
        }
    }

    /// The count and its unit, singular where it should be.
    pub fn describe(self, beats: usize) -> String {
        let count = self.count(beats);
        match (count, self) {
            (1, Length::Bars) => "1 bar".to_string(),
            (1, Length::Beats) => "1 beat".to_string(),
            (n, unit) => format!("{n} {}", unit.label()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Where audio copied into the collection is kept.
    pub library_path: PathBuf,
    /// Where rendered stem kits go.
    pub stems_path: PathBuf,
    pub on_external: OnExternal,
    /// Which column the browser is ordered by. A preference rather than part
    /// of the collection: it describes how this person likes to look at their
    /// music, not anything about the music.
    pub sort: crate::rows::Sort,
    /// How the waveform is coloured.
    pub paint: crate::wave::Paint,
    /// Whether a track's length reads as bars or as beats.
    pub length: Length,
    /// Whether analysis also fingerprints a track and looks up what it is.
    pub identify: bool,
    /// AcoustID API key. Free from https://acoustid.org/new-application.
    ///
    /// Empty means no lookups; the window says so rather than failing quietly.
    /// `ACOUSTID_API_KEY` in the environment is used when this is empty, so a
    /// key never has to be written into a file to try the feature.
    pub acoustid_key: String,
    /// The confidence at or above which a match is written in without asking.
    ///
    /// Below it, and for anything that disagrees with the file's own tags, the
    /// match becomes a question instead.
    pub autotag_score: f64,
    /// Whether editing a track's artist, title or album also rewrites the tags
    /// in the file itself.
    ///
    /// Off by default: a collection edit is cheap and reversible, and rewriting
    /// someone's files because they fixed a spelling in a browser is not.
    pub write_tags_to_files: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            library_path: default_library_path(),
            stems_path: data_dir().join("stems"),
            on_external: OnExternal::default(),
            sort: crate::rows::Sort::default(),
            paint: crate::wave::Paint::default(),
            length: Length::default(),
            identify: true,
            acoustid_key: String::new(),
            // High, because the cost of being wrong is a library that quietly
            // renamed somebody's records. Anything less certain is a question.
            autotag_score: 0.9,
            write_tags_to_files: false,
        }
    }
}

/// Under the home directory's music folder, not under the data directory.
///
/// The data directory is for a few hundred kilobytes of collection; this is for
/// however many gigabytes of audio, somewhere a person would look for it.
fn default_library_path() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    home.join("Music").join("Booth")
}

impl Config {
    /// The key to use, from the settings or the environment.
    pub fn key(&self) -> Option<String> {
        let configured = self.acoustid_key.trim();
        if !configured.is_empty() {
            return Some(configured.to_string());
        }
        std::env::var("ACOUSTID_API_KEY").ok().filter(|key| !key.trim().is_empty())
    }

    pub fn path() -> PathBuf {
        data_dir().join("config.json")
    }

    /// Read the settings, or the defaults if there are none yet.
    ///
    /// Unlike the collection, a settings file that will not parse is not fatal:
    /// there is nothing in it that cannot be set again, and refusing to open
    /// the window over it would be worse than starting from the defaults.
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, serde_json::to_vec_pretty(self)?)?;
        Ok(())
    }

    /// Whether a file is already inside the library folder.
    pub fn holds(&self, path: &Path) -> bool {
        // Compared after resolving both, so that a symlinked or relative path
        // into the library is recognised as being in it.
        let library = resolve(&self.library_path);
        resolve(path).starts_with(&library)
    }

    /// Where a file would go if it were copied in.
    ///
    /// One folder per artist, which is the layout the drive uses too, so a
    /// person browsing the library folder sees the same shape they will see on
    /// the stick.
    pub fn destination_for(&self, artist: &str, source: &Path) -> PathBuf {
        let name = source.file_name().unwrap_or_else(|| std::ffi::OsStr::new("track"));
        self.library_path.join(folder_name(artist)).join(name)
    }
}

/// Absolute and symlink-free where possible, and unchanged where not — a path
/// that does not exist yet cannot be canonicalised, and still has to compare.
fn resolve(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| match path.is_absolute() {
        true => path.to_path_buf(),
        false => std::env::current_dir().unwrap_or_default().join(path),
    })
}

/// A folder name a filesystem will accept, matching what the drive writer does
/// with the same artist.
fn folder_name(artist: &str) -> String {
    let cleaned: String = artist
        .chars()
        .map(|c| if c.is_control() || "/\\:*?\"<>|".contains(c) { '_' } else { c })
        .collect();
    let trimmed = cleaned.trim().trim_end_matches('.');
    if trimmed.is_empty() {
        "Unknown Artist".to_string()
    } else {
        trimmed.chars().take(60).collect()
    }
}

/// Copy a file into the library, and say where it landed.
///
/// Never overwrites. A file already there with the same contents is the answer
/// rather than a second copy — re-importing a folder should not double it — and
/// a *different* file with the same name gets a suffix, because two records
/// called `01 - Intro.mp3` are two different records.
pub fn copy_in(config: &Config, artist: &str, source: &Path) -> anyhow::Result<PathBuf> {
    let wanted = config.destination_for(artist, source);
    if let Some(parent) = wanted.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut candidate = wanted.clone();
    let mut attempt = 1;
    while candidate.exists() {
        if same_file(&candidate, source)? {
            return Ok(candidate);
        }
        attempt += 1;
        candidate = numbered(&wanted, attempt);
    }

    // Copied via a temporary name in the same folder, so an interrupted copy
    // does not leave a half-written file that looks like a track.
    let partial = candidate.with_extension("booth-partial");
    std::fs::copy(source, &partial)?;
    std::fs::rename(&partial, &candidate)?;
    Ok(candidate)
}

/// `name.flac` becomes `name (2).flac`.
fn numbered(path: &Path, attempt: u32) -> PathBuf {
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let extension = path.extension().map(|e| e.to_string_lossy().into_owned());
    let name = match extension {
        Some(extension) => format!("{stem} ({attempt}).{extension}"),
        None => format!("{stem} ({attempt})"),
    };
    path.with_file_name(name)
}

/// Whether two paths hold the same bytes.
///
/// The size is checked first, which settles almost every case without reading
/// anything; the contents are only compared when the sizes agree.
fn same_file(a: &Path, b: &Path) -> std::io::Result<bool> {
    let (left, right) = (std::fs::metadata(a)?, std::fs::metadata(b)?);
    if left.len() != right.len() {
        return Ok(false);
    }
    Ok(std::fs::read(a)? == std::fs::read(b)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("booth-cfg-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn config_in(dir: &Path) -> Config {
        Config { library_path: dir.join("library"), ..Config::default() }
    }

    #[test]
    fn the_default_is_to_take_a_copy() {
        // The whole point: a collection that is missing a file on the night is
        // worse than a collection that used some disk.
        assert_eq!(Config::default().on_external, OnExternal::Copy);
    }

    #[test]
    fn music_is_kept_where_a_person_would_look_for_it() {
        let config = Config::default();
        assert!(config.library_path.is_absolute());
        assert!(config.library_path.ends_with("Booth"));
        assert!(
            !config.library_path.starts_with(data_dir()),
            "gigabytes of audio do not belong in the data directory"
        );
    }

    #[test]
    fn a_key_comes_from_the_settings_or_the_environment() {
        let mut config = Config::default();
        assert_eq!(config.acoustid_key, "");

        config.acoustid_key = "  typed-in  ".into();
        assert_eq!(config.key().as_deref(), Some("typed-in"), "trimmed, because it is pasted");

        // Blank means the environment gets a turn, so a key can be tried
        // without writing it into a file.
        config.acoustid_key = "   ".into();
        // SAFETY: single-threaded test, and the variable is read back at once.
        unsafe { std::env::set_var("ACOUSTID_API_KEY", "from-env") };
        assert_eq!(config.key().as_deref(), Some("from-env"));
        unsafe { std::env::set_var("ACOUSTID_API_KEY", "") };
        assert_eq!(config.key(), None, "an empty variable is not a key");
        unsafe { std::env::remove_var("ACOUSTID_API_KEY") };
        assert_eq!(config.key(), None);
    }

    #[test]
    fn the_bar_for_writing_a_name_in_unasked_is_high() {
        // Being wrong here renames somebody's records without them noticing,
        // so the default sits well above the floor questions start at.
        let config = Config::default();
        assert!(config.autotag_score >= 0.9);
        assert!(config.autotag_score > crate::identify::FLOOR);
        assert!(config.identify, "a fingerprint is most of what fills a bare file in");
    }

    #[test]
    fn a_length_reads_in_bars_by_default() {
        // The unit a set is built in. Eight bars of intro is a thing a DJ
        // thinks; thirty-two beats of intro is arithmetic.
        assert_eq!(Config::default().length, Length::Bars);
        assert_eq!(Length::Bars.count(736), 184);
        assert_eq!(Length::Beats.count(736), 736);
    }

    #[test]
    fn a_part_bar_at_the_end_does_not_round_up_to_a_whole_one() {
        // 183 bars and three beats is 183 bars, not 184: a bar that is not
        // there is not a bar to cue into.
        assert_eq!(Length::Bars.count(735), 183);
        assert_eq!(Length::Bars.count(3), 0);
    }

    #[test]
    fn one_of_something_is_not_ones() {
        assert_eq!(Length::Bars.describe(4), "1 bar");
        assert_eq!(Length::Beats.describe(1), "1 beat");
        assert_eq!(Length::Bars.describe(8), "2 bars");
        assert_eq!(Length::Beats.describe(0), "0 beats");
    }

    #[test]
    fn settings_survive_a_round_trip() {
        let dir = scratch("roundtrip");
        let path = dir.join("config.json");

        let mut config = config_in(&dir);
        config.on_external = OnExternal::Ask;
        config.write_tags_to_files = true;
        config.acoustid_key = "abc123".into();
        config.autotag_score = 0.75;
        config.paint = crate::wave::Paint::Stems;
        config.length = Length::Beats;
        config.sort = crate::rows::Sort { column: crate::rows::Column::Bpm, descending: true };
        config.save(&path).unwrap();

        assert_eq!(Config::load(&path), config);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_broken_settings_file_is_not_fatal() {
        let dir = scratch("broken");
        let path = dir.join("config.json");
        std::fs::write(&path, b"{not json").unwrap();
        // Unlike the collection, there is nothing here that cannot be set
        // again, so the window opens on the defaults rather than not at all.
        assert_eq!(Config::load(&path), Config::default());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_missing_settings_file_is_the_defaults() {
        assert_eq!(Config::load(Path::new("/nowhere/config.json")), Config::default());
    }

    #[test]
    fn a_file_in_the_library_folder_is_not_external() {
        let dir = scratch("holds");
        let config = config_in(&dir);
        std::fs::create_dir_all(config.library_path.join("Batu")).unwrap();
        let inside = config.library_path.join("Batu").join("Marius.flac");
        std::fs::write(&inside, b"x").unwrap();

        assert!(config.holds(&inside));
        assert!(!config.holds(&dir.join("downloads").join("Marius.flac")));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_file_that_does_not_exist_yet_still_answers() {
        // Nothing has been created, so neither path can be canonicalised; the
        // comparison still has to work, because it decides whether to copy.
        let config = Config { library_path: PathBuf::from("/music/library"), ..Config::default() };
        assert!(config.holds(Path::new("/music/library/Batu/Marius.flac")));
        assert!(!config.holds(Path::new("/downloads/Marius.flac")));
    }

    #[test]
    fn the_library_is_laid_out_by_artist_the_way_the_drive_is() {
        let config = Config { library_path: PathBuf::from("/music"), ..Config::default() };
        let to = config.destination_for("Batu", Path::new("/downloads/Marius.flac"));
        assert_eq!(to, PathBuf::from("/music/Batu/Marius.flac"));
    }

    #[test]
    fn an_artist_name_a_filesystem_would_refuse_is_cleaned_up() {
        let config = Config { library_path: PathBuf::from("/music"), ..Config::default() };
        let to = config.destination_for("AC/DC", Path::new("/d/x.flac"));
        assert_eq!(to, PathBuf::from("/music/AC_DC/x.flac"));

        let to = config.destination_for("   ", Path::new("/d/x.flac"));
        assert_eq!(to, PathBuf::from("/music/Unknown Artist/x.flac"));
    }

    #[test]
    fn copying_in_puts_the_file_where_it_says_it_will() {
        let dir = scratch("copy");
        let config = config_in(&dir);
        let source = dir.join("downloads").join("Marius.flac");
        std::fs::create_dir_all(source.parent().unwrap()).unwrap();
        std::fs::write(&source, b"audio").unwrap();

        let landed = copy_in(&config, "Batu", &source).unwrap();
        assert_eq!(landed, config.library_path.join("Batu").join("Marius.flac"));
        assert_eq!(std::fs::read(&landed).unwrap(), b"audio");
        assert!(source.exists(), "the original must not be moved");
        assert!(config.holds(&landed));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn copying_the_same_file_twice_does_not_make_a_second_copy() {
        let dir = scratch("copy-twice");
        let config = config_in(&dir);
        let source = dir.join("Marius.flac");
        std::fs::write(&source, b"audio").unwrap();

        let first = copy_in(&config, "Batu", &source).unwrap();
        let second = copy_in(&config, "Batu", &source).unwrap();
        assert_eq!(first, second, "re-importing a folder should not double it");

        let files: Vec<_> = std::fs::read_dir(config.library_path.join("Batu")).unwrap().collect();
        assert_eq!(files.len(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_different_file_with_the_same_name_is_kept_separately() {
        let dir = scratch("collide");
        let config = config_in(&dir);
        let one = dir.join("a").join("01 - Intro.mp3");
        let two = dir.join("b").join("01 - Intro.mp3");
        for (path, bytes) in [(&one, b"first".as_slice()), (&two, b"second-and-longer".as_slice())]
        {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, bytes).unwrap();
        }

        let first = copy_in(&config, "Various", &one).unwrap();
        let second = copy_in(&config, "Various", &two).unwrap();
        assert_ne!(first, second, "two different records were merged into one");
        assert!(second.to_string_lossy().contains("(2)"), "{}", second.display());
        // And neither was overwritten.
        assert_eq!(std::fs::read(&first).unwrap(), b"first");
        assert_eq!(std::fs::read(&second).unwrap(), b"second-and-longer");

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_same_sized_but_different_file_is_still_a_different_file() {
        let dir = scratch("same-size");
        let config = config_in(&dir);
        let one = dir.join("a").join("x.mp3");
        let two = dir.join("b").join("x.mp3");
        for (path, bytes) in [(&one, b"AAAA".as_slice()), (&two, b"BBBB".as_slice())] {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, bytes).unwrap();
        }

        let first = copy_in(&config, "X", &one).unwrap();
        let second = copy_in(&config, "X", &two).unwrap();
        assert_ne!(first, second, "size alone must not be taken for sameness");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_interrupted_copy_leaves_nothing_that_looks_like_a_track() {
        let dir = scratch("partial");
        let config = config_in(&dir);
        let source = dir.join("Marius.flac");
        std::fs::write(&source, b"audio").unwrap();
        copy_in(&config, "Batu", &source).unwrap();

        let leftovers: Vec<String> = std::fs::read_dir(config.library_path.join("Batu"))
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(leftovers, vec!["Marius.flac".to_string()], "{leftovers:?}");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
