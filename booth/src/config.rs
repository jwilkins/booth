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
/// What to do about music on a drive that the library has no copy of.
///
/// Only ever somebody else's drive: a drive this program wrote holds the
/// library's own files, which are linked rather than copied. The question is
/// what a copy of a stranger's stick should be — a record of what was on it, a
/// complete thing that can be put back, or an invitation to keep the music.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum OnForeign {
    /// Name it in the manifest and store none of it.
    #[default]
    Ignore,
    /// Copy it into the backup, so that copy is complete on its own.
    Keep,
    /// Copy it into the library and add it to the collection.
    Adopt,
}

impl OnForeign {
    pub const ALL: [OnForeign; 3] = [OnForeign::Ignore, OnForeign::Keep, OnForeign::Adopt];

    pub fn label(self) -> &'static str {
        match self {
            OnForeign::Ignore => "Note what was on it",
            OnForeign::Keep => "Copy it into the backup",
            OnForeign::Adopt => "Copy it into the library",
        }
    }

    pub fn blurb(self) -> &'static str {
        match self {
            OnForeign::Ignore => {
                "The drive's databases, cues and analysis are kept; its music is named in the                  manifest and not stored. Costs nothing, and the music is gone if the drive is."
            }
            OnForeign::Keep => {
                "The backup holds the music too, so it can be put back on a stick as it was.                  Costs whatever the drive holds that you do not — gigabytes, for a stranger's."
            }
            OnForeign::Adopt => {
                "The music is copied into the library and added to the collection, where it can                  be analysed and played like anything else. The same cost, and a browser with                  somebody else's records in it."
            }
        }
    }
}

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

    /// Where a beat falls, counted from the top of the track.
    ///
    /// `1.1 1.2 1.3 1.4 2.1` — the bar, then the beat within it, both counted
    /// from one. It is the way a DJ counts out loud, and it is four characters,
    /// which is what makes it worth having over a timecode.
    ///
    /// In beats it is just the beat number, because a bar-and-beat reading is
    /// the whole point of the other setting.
    pub fn position(self, beat: usize) -> String {
        match self {
            Length::Bars => format!("{}.{}", beat / 4 + 1, beat % 4 + 1),
            Length::Beats => format!("{}", beat + 1),
        }
    }

    /// Where the playhead is and how much is left, in one compact reading.
    ///
    /// Elapsed as a position, remaining as a count with a minus in front of it
    /// — the way a player shows time and remain. They are different kinds of
    /// thing and are deliberately not written the same way: a position is
    /// one-based and a count is not, and printing both as `12.3` would invite
    /// reading a remainder as a place in the track.
    pub fn elapsed_and_left(self, beat: usize, total_beats: usize) -> String {
        let played = beat.min(total_beats);
        let left = total_beats.saturating_sub(played);
        format!("{} · -{}", self.position(played), self.count(left))
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

/// How much work a separation is worth.
///
/// The command line owns the real definition — which model, how many shifts —
/// and this is the same two choices in a form that can be saved with the rest
/// of the settings. Keeping the mapping in one place is what stops the window
/// and the tool disagreeing about what "high" means.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Quality {
    #[default]
    High,
    Standard,
}

impl Quality {
    pub const ALL: [Quality; 2] = [Quality::High, Quality::Standard];

    pub fn to_cli(self) -> booth_cli::cli::StemQuality {
        match self {
            Quality::High => booth_cli::cli::StemQuality::High,
            Quality::Standard => booth_cli::cli::StemQuality::Standard,
        }
    }

    pub fn label(self) -> &'static str {
        self.to_cli().label()
    }

    pub fn blurb(self) -> &'static str {
        self.to_cli().blurb()
    }
}

/// Where a rendered stem kit is kept.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum StemsIn {
    /// In the same folder as the track they came from.
    ///
    /// The default. A kit belongs to one record: keeping it beside that record
    /// means copying the folder takes the stems with it, every other tool sees
    /// them, and there is no second place to remember to back up. They are
    /// named after the track, so they sort next to it.
    #[default]
    Beside,
    /// All of them together in one folder, wherever `stems_path` points.
    ///
    /// For a library on a small disk with the stems on a big one, which is the
    /// case the folder was there for.
    Folder,
}

impl StemsIn {
    pub fn label(self) -> &'static str {
        match self {
            StemsIn::Beside => "beside the track",
            StemsIn::Folder => "one folder",
        }
    }

    pub fn blurb(self) -> &'static str {
        match self {
            StemsIn::Beside => "Next to the file they came from, named after it",
            StemsIn::Folder => "All together, wherever the stems folder points",
        }
    }
}

/// Where stems are written, and everywhere they might already be.
///
/// Carried into the separation job as a value rather than as the whole config,
/// because the job runs on another thread and needs no more than this.
#[derive(Clone, Debug, PartialEq)]
pub struct StemsLocation {
    pub in_: StemsIn,
    pub folder: PathBuf,
}

impl StemsLocation {
    /// Where a track's stems go.
    pub fn for_source(&self, source: &Path) -> PathBuf {
        match self.in_ {
            StemsIn::Beside => source.parent().unwrap_or(Path::new(".")).to_path_buf(),
            StemsIn::Folder => self.folder.clone(),
        }
    }

    /// Everywhere a kit for this track might be, best first.
    ///
    /// Both, always. Changing the setting must not make rendered stems vanish,
    /// and a kit is minutes of work — finding one already on disk is worth a
    /// second `exists` call.
    pub fn search(&self, source: &Path) -> Vec<PathBuf> {
        let mut places = vec![self.for_source(source)];
        for other in [source.parent().unwrap_or(Path::new(".")).to_path_buf(), self.folder.clone()]
        {
            if !places.contains(&other) {
                places.push(other);
            }
        }
        places
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Where audio copied into the collection is kept.
    pub library_path: PathBuf,
    /// Where rendered stem kits go, when they go in one folder.
    pub stems_path: PathBuf,
    /// Whether a kit sits beside its track or in that folder.
    pub stems_in: StemsIn,
    pub on_external: OnExternal,
    /// Which column the browser is ordered by. A preference rather than part
    /// of the collection: it describes how this person likes to look at their
    /// music, not anything about the music.
    pub sort: crate::rows::Sort,
    /// Which columns the browser shows, and how wide each was left. Also a
    /// preference: it says how this person reads a list, not what is in it.
    #[serde(default)]
    pub columns: crate::rows::Layout,
    /// How the waveform is coloured.
    pub paint: crate::wave::Paint,
    /// Whether a track's length reads as bars or as beats.
    pub length: Length,
    /// How much work a separation is worth.
    pub stem_quality: Quality,
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
    /// How much of what the collection knows reaches the files themselves.
    pub write_tags: WriteTags,
    /// The SQLCipher key rekordbox's own libraries are encrypted with.
    ///
    /// Empty means fall back to `REKORDBOX_KEY`, and then to whatever the
    /// build was compiled with. Kept here rather than in the collection for
    /// the same reason the library path is: it describes this machine.
    #[serde(default)]
    pub rekordbox_key: String,
    /// The key the OneLibrary database on a drive is encrypted with.
    ///
    /// A different key from the one above, for a different file, and the two
    /// are not interchangeable: that one opens rekordbox's library on this
    /// computer, this one writes the database the newer players read off a
    /// drive. Empty means fall back to `ONELIBRARY_KEY`, and then to writing
    /// no such database at all.
    #[serde(default)]
    pub onelibrary_key: String,
    /// Where copies of prepared drives go.
    ///
    /// A drive holds hours of work in the one place most likely to be dropped,
    /// left in a booth, or simply to stop working. What goes here is the part
    /// that cannot be made again — the databases, the analysis, the cues, the
    /// play history — with the audio linked rather than copied, so keeping
    /// every drive costs megabytes rather than gigabytes.
    #[serde(default = "default_backups_path")]
    pub backups_path: PathBuf,
    /// Whether a drive is copied when it is written or plugged in.
    #[serde(default = "yes")]
    pub keep_drives: bool,
    /// What to do about music on a drive that the library has no copy of.
    #[serde(default)]
    pub on_foreign: OnForeign,
    /// How wide or tall each panel was left.
    #[serde(default)]
    pub panels: Panels,
}

/// The panel sizes, in points, as the window was last left.
///
/// A preference about this person's screen and how they like to work, which is
/// what this file is for — and the one thing about a window that is genuinely
/// annoying to set twice.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Panels {
    pub collection: f32,
    pub inspector: f32,
    pub dock: f32,
}

impl Default for Panels {
    fn default() -> Self {
        Self { collection: 178.0, inspector: 210.0, dock: 44.0 }
    }
}

impl Panels {
    /// Whether `size` is far enough from what is stored to be worth writing.
    ///
    /// A drag arrives as a stream of sub-point changes, and a panel that
    /// rewrote the settings file on each of them would write a hundred times
    /// across one drag.
    pub fn differs(before: f32, after: f32) -> bool {
        (before - after).abs() >= 1.0
    }
}

/// When a name in the collection is also written into the file's tag block.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum WriteTags {
    /// Never. The collection is the only record.
    Never,
    /// Only where the file says nothing.
    ///
    /// The default, and the asymmetry is the point: filling in a blank is not
    /// the same act as overwriting somebody's answer. A fingerprint lookup that
    /// names an untagged file has found out something true about it, and
    /// leaving that only in the collection means the file stays anonymous to
    /// every other program that opens it.
    #[default]
    Fill,
    /// Every name change, including over a value already there.
    Always,
}

impl WriteTags {
    pub const ALL: [WriteTags; 3] = [WriteTags::Never, WriteTags::Fill, WriteTags::Always];

    pub fn label(self) -> &'static str {
        match self {
            WriteTags::Never => "never",
            WriteTags::Fill => "only where the file is blank",
            WriteTags::Always => "always",
        }
    }

    pub fn blurb(self) -> &'static str {
        match self {
            WriteTags::Never => "The collection keeps the names to itself",
            WriteTags::Fill => "A lookup fills in an untagged file; nothing is overwritten",
            WriteTags::Always => "Every edit and every match rewrites the file's tags",
        }
    }

    /// What to do about a field the file already has a value for, or `None`
    /// when the file should not be touched at all.
    pub fn on_existing(self) -> Option<booth_cli::tag::OnExisting> {
        match self {
            WriteTags::Never => None,
            WriteTags::Fill => Some(booth_cli::tag::OnExisting::Keep),
            WriteTags::Always => Some(booth_cli::tag::OnExisting::Overwrite),
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            library_path: default_library_path(),
            stems_path: data_dir().join("stems"),
            stems_in: StemsIn::default(),
            on_external: OnExternal::default(),
            sort: crate::rows::Sort::default(),
            columns: crate::rows::Layout::default(),
            panels: Panels::default(),
            paint: crate::wave::Paint::default(),
            length: Length::default(),
            stem_quality: Quality::default(),
            identify: true,
            acoustid_key: String::new(),
            // High, because the cost of being wrong is a library that quietly
            // renamed somebody's records. Anything less certain is a question.
            autotag_score: 0.9,
            write_tags: WriteTags::default(),
            rekordbox_key: String::new(),
            onelibrary_key: String::new(),
            backups_path: default_backups_path(),
            keep_drives: true,
            on_foreign: OnForeign::default(),
        }
    }
}

/// Beside the library rather than under the data directory: these are copies
/// of somebody's work, and a person should be able to find them, look inside
/// one and copy it back onto a stick without this program's help.
fn default_backups_path() -> PathBuf {
    default_library_path()
        .parent()
        .map(|at| at.join("booth-drives"))
        .unwrap_or_else(|| data_dir().join("drives"))
}

/// Serde needs a function to call for a default that is not `false`.
fn yes() -> bool {
    true
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
        let mut config: Self = std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        // A settings file written before a column existed does not mention it,
        // and one edited by hand can say anything. Sorting that out here means
        // nothing downstream has to wonder whether the list it was handed
        // covers every column.
        config.columns.repair();
        config
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, serde_json::to_vec_pretty(self)?)?;
        Ok(())
    }

    /// Whether a file is already inside the library folder.
    /// The rekordbox key from the settings, if one was put there.
    pub fn rekordbox_key(&self) -> Option<&str> {
        let key = self.rekordbox_key.trim();
        (!key.is_empty()).then_some(key)
    }

    /// The OneLibrary key from the settings, if one was put there.
    pub fn onelibrary_key(&self) -> Option<&str> {
        let key = self.onelibrary_key.trim();
        (!key.is_empty()).then_some(key)
    }

    /// Whether a drive written now would carry the database the newer players
    /// read.
    ///
    /// The settings first, then the environment — the same order the export
    /// itself resolves it in, so what the sync sheet promises and what the
    /// write does cannot come apart.
    pub fn writes_onelibrary(&self) -> bool {
        booth_cli::rekordbox::onelibrary_key(self.onelibrary_key()).is_some()
    }

    /// Where stems go and where to look for ones already rendered.
    pub fn stems_location(&self) -> StemsLocation {
        StemsLocation { in_: self.stems_in, folder: self.stems_path.clone() }
    }

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
    fn a_position_counts_the_way_a_dj_counts_out_loud() {
        // 1.1 1.2 1.3 1.4 2.1 — bars and beats both from one.
        let readings: Vec<String> = (0..5).map(|beat| Length::Bars.position(beat)).collect();
        assert_eq!(readings, vec!["1.1", "1.2", "1.3", "1.4", "2.1"]);
        // The top of the track is the first beat of the first bar, not zero.
        assert_eq!(Length::Bars.position(0), "1.1");
        assert_eq!(Length::Bars.position(63), "16.4");
        assert_eq!(Length::Bars.position(64), "17.1");
    }

    #[test]
    fn in_beats_a_position_is_just_the_beat_number() {
        assert_eq!(Length::Beats.position(0), "1");
        assert_eq!(Length::Beats.position(63), "64");
    }

    #[test]
    fn a_reading_says_where_it_is_and_how_much_is_left() {
        let total = 64 * 4;
        assert_eq!(Length::Bars.elapsed_and_left(64, total), "17.1 · -48");
        assert_eq!(Length::Bars.elapsed_and_left(0, total), "1.1 · -64");
        // At the very end nothing is left, and the count does not go negative.
        assert_eq!(Length::Bars.elapsed_and_left(total, total), "65.1 · -0");
        assert_eq!(Length::Bars.elapsed_and_left(total + 99, total), "65.1 · -0");
    }

    #[test]
    fn a_reading_in_beats_uses_beats_for_both_halves() {
        assert_eq!(Length::Beats.elapsed_and_left(7, 32), "8 · -25");
    }

    #[test]
    fn stems_are_rendered_well_by_default() {
        // Rendered once, played for years: the slow one is the right default.
        assert_eq!(Config::default().stem_quality, Quality::High);
        assert_eq!(Quality::High.to_cli().model(), "htdemucs_ft");
        assert_eq!(Quality::High.to_cli().shifts(), 2);
        assert_eq!(Quality::Standard.to_cli().model(), "htdemucs");
        assert_eq!(Quality::Standard.to_cli().shifts(), 0);
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
        config.write_tags = WriteTags::Always;
        config.acoustid_key = "abc123".into();
        config.autotag_score = 0.75;
        config.paint = crate::wave::Paint::Stems;
        config.length = Length::Beats;
        config.stem_quality = Quality::Standard;
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
