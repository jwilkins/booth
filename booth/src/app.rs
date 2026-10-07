//! One window. No modes.
//!
//! Rekordbox splits into an export mode and a performance mode — two mental
//! models over one library, a seam that exists for licensing reasons rather
//! than for anything a DJ wants. Here there is one view: the query bar across
//! the top, three panes under it, the prep editor beneath the browser rather
//! than in place of it, and the drive dock along the bottom where the delta is
//! permanently visible.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;

use booth_cli::cli::ExportArgs;
use booth_cli::stems::Backend;
use eframe::egui::{self, Color32, RichText, Ui};

use crate::config::{Config, OnExternal};
use crate::job::{self, Adoptable, Job, Retag, Runner, Update};
use crate::library::{now, plural, Drive, Library, Playlist, SavedQuery, Track, Written};
use crate::player::Player;
use crate::query::{self, Context, Paint, Query};
use crate::rows;
use crate::sync::{self, Level, Plan};
use crate::theme;
use crate::wave;

/// What the log window is showing.
///
/// Behind an `Arc` because the window is a viewport of its own, drawn by a
/// callback that egui keeps and calls on its own terms — it cannot borrow the
/// application, so what it needs has to be shared rather than passed.
#[derive(Default)]
pub struct LogWindow {
    open: std::sync::atomic::AtomicBool,
    /// The lowest level shown, as a [`crate::log::Level`] code.
    level: std::sync::atomic::AtomicU8,
    /// Whether it follows the tail.
    follow: std::sync::atomic::AtomicBool,
}

impl LogWindow {
    fn is_open(&self) -> bool {
        self.open.load(std::sync::atomic::Ordering::Relaxed)
    }

    fn set_open(&self, open: bool) {
        self.open.store(open, std::sync::atomic::Ordering::Relaxed);
    }

    fn level(&self) -> crate::log::Level {
        crate::log::Level::SHOWN
            .get(self.level.load(std::sync::atomic::Ordering::Relaxed) as usize)
            .copied()
            .unwrap_or(crate::log::Level::Debug)
    }

    fn set_level(&self, level: crate::log::Level) {
        let index = crate::log::Level::SHOWN.iter().position(|l| *l == level).unwrap_or(0);
        self.level.store(index as u8, std::sync::atomic::Ordering::Relaxed);
    }

    fn follows(&self) -> bool {
        self.follow.load(std::sync::atomic::Ordering::Relaxed)
    }

    fn set_follow(&self, follow: bool) {
        self.follow.store(follow, std::sync::atomic::Ordering::Relaxed);
    }
}

/// How many stray files the check names before saying how many more there are.
/// A first import into an empty library folder can turn up thousands, and a
/// sheet listing all of them is a scrollbar rather than an answer.
const ORPHANS_SHOWN: usize = 40;

/// The room the duplicates sheet keeps for its footer, so the button that does
/// the deleting is never scrolled away from.
const FOOTER_HEIGHT: f32 = 46.0;

/// How close a new beat mark has to be to one already placed to be taken as a
/// correction of it rather than another beat, in milliseconds.
///
/// Sixty. Two beats that close would be a record at a thousand BPM, so nobody
/// means them as a pair; what they mean is that the first one landed badly.
const MARKS_APART_MS: u32 = 60;

/// How much room to keep under the list for the prep editor, before it has
/// ever been drawn.
///
/// Only the first frame uses it. After that the editor reports the height it
/// actually came to and that is what is kept — see [`App::prep_height`] —
/// because a budget added up by hand is a budget that goes stale, and the way
/// it fails is silent: a few points short does not overflow, it quietly clips
/// the last row off the bottom of the window.
///
/// Which is what the grid tools did. They are a row of their own; this sum
/// knew nothing about them; and opening them pushed the measurements line off
/// the bottom — the tempo, the key, the energy, and the button that puts the
/// tools away again. With no way to close them, and the setting kept per
/// window rather than per track, selecting another track did not bring them
/// back either. The comment that used to sit here said the measurements line
/// is the row this takes. It was right, and saying so was not enough.
const PREP_HEIGHT_AT_FIRST: f32 = wave::HEIGHT
    + wave::STRIP_HEIGHT
    // the actions strip, the cue strip, and the measurements line
    + 3.0 * 24.0
    // the separator and the spacing between all of them
    + 40.0;

/// How tall the dock is before anyone drags it: its row of buttons, and one
/// line of log under them.
///
/// Also its minimum, because that row is the dock's job and a panel dragged
/// shorter than its own contents just clips them.
const DOCK_HEIGHT: f32 = 24.0 + 4.0 + 16.0;

/// What the last look at one drive found.
struct Seen {
    /// The digest of the files it had of its own.
    fingerprint: String,
    /// Those files, so a later look can say what changed rather than only that
    /// something did.
    listing: Vec<String>,
    /// When that state was acted on.
    at: std::time::Instant,
    /// Whether the log has already said this drive will not settle. Said once
    /// per state, because a line every four seconds is not a warning, it is a
    /// second problem.
    complained: bool,
}

/// How long a drive that changed on its own has to hold still before it is
/// stored again.
///
/// Nothing here changes a mounted drive, but an operating system does — an
/// index it decided to build, a folder view it decided to save — and a copy
/// started for every one of those is a disk full of near-identical copies by
/// morning. A drive this program writes is not held back by this: that path
/// knows a real change happened, and says so.
const SETTLE: std::time::Duration = std::time::Duration::from_secs(300);

/// Whether a drive found in this state is one to copy now.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Worth {
    /// It is the state it was already stored in.
    No,
    /// A different state, but it changed again so soon after the last copy
    /// that something other than a person is changing it.
    NotYet,
    Yes,
}

/// The decision that keeps a drive from being copied over and over.
fn worth_keeping(seen: Option<&Seen>, state: &str) -> Worth {
    match seen {
        None => Worth::Yes,
        Some(seen) if seen.fingerprint == state => Worth::No,
        Some(seen) if seen.at.elapsed() < SETTLE => Worth::NotYet,
        Some(_) => Worth::Yes,
    }
}

/// Whether a drive is there to be written to right now.
///
/// A drive in the collection is a place and a history of what went on it, not
/// a stick in a socket, and the two read identically in a list until something
/// asks. Saying which is which is the difference between a sync that does
/// nothing and a sync that was the wrong button.
///
/// An image is a file and a stick is a mounted folder, so the question is not
/// the same one twice. Neither asks whether the drive has been written yet: a
/// blank stick about to have a library put on it is plugged in.
fn plugged_in(drive: &crate::library::Drive) -> bool {
    match drive.is_image {
        true => drive.path.is_file(),
        false => drive.path.is_dir(),
    }
}

/// What to call a drive: the name it is mounted under, or the label it gave
/// when there is no mount point to read.
///
/// One rule, used by both the sweep and the write that just finished, because
/// the name is the folder its copies live in and two names would mean two
/// folders for one stick — and a copy stored under one name that the other
/// never finds.
fn drive_name(root: &std::path::Path, label: &str) -> String {
    match root.file_name().and_then(|n| n.to_str()) {
        Some(name) if !name.is_empty() => name.to_string(),
        _ => match label.trim().is_empty() {
            true => "drive".to_string(),
            false => label.trim().to_string(),
        },
    }
}

/// Which of the fixed collection views is showing.
#[derive(Copy, Clone, PartialEq, Eq)]
enum View {
    All,
    Unprepared,
    Attention,
    /// A playlist, by name.
    Playlist,
}

pub struct App {
    library: Library,
    library_path: PathBuf,
    config: Config,
    config_path: PathBuf,

    /// What is typed in the command bar.
    text: String,
    query: Query,
    view: View,
    /// The playlist selected in the sidebar, when the view is a playlist.
    playlist: String,

    /// The rows the query produced, rebuilt whenever anything changes.
    rows: Vec<Row>,
    /// Which column the browser is ordered by.
    sort: rows::Sort,
    selected: Option<u32>,
    /// Every row in the selection, the focused one included.
    ///
    /// Kept beside `selected` rather than replacing it: one row is still the
    /// one the inspector shows and the deck plays, and a selection of thirty
    /// does not have thirty waveforms. This is what an action applies to.
    marked: std::collections::HashSet<u32>,
    /// Where a shift-range is measured from — the last row picked without
    /// shift, which is what shift extends away from.
    anchor: Option<u32>,
    /// The three-band waveform of the selected track, once it has been read.
    waveform: Option<(u32, Vec<u8>)>,
    /// Which part of the selected track's waveform is showing. Reset with the
    /// selection: a view into one track means nothing in another, and carrying
    /// it over lands you eight minutes into a four-minute record.
    zoom: wave::Zoom,
    /// Its per-stem loudness, when a kit has been measured.
    envelopes: Option<(u32, wave::StemEnvelopes)>,

    /// The deck, if a device could be opened. `None` on a machine with no
    /// sound card, over SSH, or under a headless test — a library tool that
    /// will not start without speakers is worse than one that cannot audition.
    player: Option<Player>,
    /// Why there is no deck, for the transport to say once rather than
    /// silently doing nothing.
    player_problem: Option<String>,
    /// A track being decoded for playback, and whether to start it when it
    /// arrives.
    loading: Option<(u32, bool)>,

    /// The log window, and what it is showing.
    log: Arc<LogWindow>,

    runner: Option<Runner>,
    /// Work waiting for the runner. Jobs chain — an import that turns up
    /// external files queues the copy behind itself — and a queue is the only
    /// honest way to say so.
    queued: VecDeque<Job>,
    progress: Option<(usize, usize)>,
    /// How far into the file in hand, for work slow enough that finishing it
    /// is not soon enough to report.
    step: Option<u8>,

    /// The drive the dock is showing, as an index into the library's drives.
    drive: usize,
    plan: Plan,
    /// Set while the sync sheet is open.
    sheet: bool,
    /// Set while the sync sheet's details window is open.
    ///
    /// A separate window rather than a taller sheet: the summary answers "is
    /// this the write I meant", which is four lines, and the details answer
    /// "which track is that", which is as many lines as the write has changes.
    /// Putting the second inside the first made the sheet a scroll either way.
    details: bool,
    /// Set while the settings sheet is open.
    settings: bool,
    /// Tracks found outside the library, waiting for an answer. Only ever set
    /// when the policy is to ask.
    asking: Vec<u32>,
    /// Fingerprint matches that need a person to decide.
    questions: Vec<crate::identify::Question>,
    /// The track whose fields are being edited, and the text as typed. Kept
    /// apart from the collection so that a half-typed name is not a name.
    editing: Option<Edit>,

    pick: Option<(Picking, std::sync::mpsc::Receiver<Vec<PathBuf>>)>,
    /// Set when the command bar should take keyboard focus.
    focus_bar: bool,
    /// The playlist name being typed on the actions strip.
    playlist_entry: String,
    /// A playlist or folder name being typed in the sidebar, if one is.
    naming: Option<Naming>,
    /// Whether the query language's help is showing.
    help: bool,
    /// Whether the duplicates sheet is open, and what has been decided in it
    /// so far.
    duplicates: Option<Dupes>,
    /// What checking the collection against its files found, while the sheet
    /// showing it is open.
    checked: Option<Checked>,
    /// When the mounted volumes were last looked through for a player's drive,
    /// and what the last look at each found.
    ///
    /// The map is a cheap guard in front of the one on disk: a stick left
    /// plugged in comes past every few seconds, and reading a directory of
    /// backups each time to decide it is the same stick would be work for
    /// nothing.
    looked_for_drives: Option<std::time::Instant>,
    kept_drives: std::collections::HashMap<String, Seen>,
    /// Kept files whose names a copier wrote, and the name each could go back
    /// to now that what it was copied from has gone. Offered, never done on its
    /// own: renaming somebody's file is not a tidy-up to spring on them.
    renames: Vec<(u32, PathBuf)>,
    /// Set when a panel has been dragged and the new size is not written out
    /// yet. See [`App::save_layout`].
    layout_moved: bool,
    /// Set by the sidebar, acted on after every panel has drawn. The tree is
    /// walked while the library is borrowed, so it cannot save or rebuild from
    /// inside the walk.
    pending_save: bool,
    pending_rebuild: bool,
    /// The tag being typed in the inspector.
    tag_entry: String,
    /// When the running job started, for saying how long it took.
    started: Option<std::time::Instant>,
    /// Tracks an import found the hardware will not play, waiting to be shown.
    /// Cleared by answering the sheet, not by looking at it.
    incompatible: Vec<u32>,
    /// Files waiting to have their tags rewritten, collected so that
    /// identifying a crate is one tagging job rather than one per track.
    to_retag: Vec<Retag>,
    /// Tag writes held back because the file's own name looks nothing like
    /// what is about to be written into it. See [`App::write_tags`].
    unlike: Vec<Retag>,
    /// Rows whose waveform has already been asked for once this run, so a
    /// measurement that cannot succeed is not attempted on every frame.
    remeasured: std::collections::HashSet<u32>,
    /// The same for stem envelopes. A second set rather than a marker bit on
    /// the first: companion ids use the top bit too, so a bit that meant
    /// "stems" would sometimes also mean "acapella of track 3".
    re_enveloped: std::collections::HashSet<u32>,
    /// Tracks that asked for cues from their words before they had a vocal
    /// stem to read. They are waiting on a separation; when it lands, the
    /// recogniser is what happens next.
    want_cues: std::collections::HashSet<u32>,
    /// A batch of separations waiting to be agreed to. See [`App::stems_sheet`].
    stems_ahead: Option<StemsAhead>,
    /// Lyrics a server offered that are not certain enough to write in
    /// unasked. See [`App::wondering_sheet`].
    wondering: Vec<Wondering>,
    /// A track's words, open for correcting. See [`App::words_sheet`].
    editing_words: Option<EditingWords>,
    /// Whether the grid controls are showing. See [`App::grid_tools`].
    ///
    /// Off until asked for: they are for the few records a tracker gets wrong,
    /// and a row of buttons under every waveform would be eight controls in
    /// the way of the ninety per cent of tracks that need none of them.
    editing_grid: bool,
    /// Tracks changed here and on the drive since the two last agreed, worked
    /// out when the sync sheet opens rather than every frame: it reads the
    /// stick.
    clashes: Vec<sync::Conflict>,
    /// Which copy to keep for each of those, as the sheet has it. Seeded with
    /// whichever is newer and then whatever the person says.
    settled: std::collections::HashMap<u32, sync::Side>,
    /// What the drive was holding when it was last examined, so that a track
    /// left as the player left it can have that recorded as agreed rather than
    /// being asked about again on every sync.
    drive_now: std::collections::HashMap<u32, crate::library::Stamp>,
    /// Where the playhead sits in the selected track, in milliseconds. It is
    /// where a new cue goes, so it is a position rather than a playing thing —
    /// nothing here makes a sound.
    playhead_ms: Option<u32>,
    /// The cue being named, and the text as typed.
    cue_entry: (Option<(u32, usize)>, String),
    /// The same, for the field in a cue's right-click menu. Its own, because
    /// the two are open at once and typing in one is not typing in the other.
    cue_menu: (Option<(u32, usize)>, String),
    /// How tall the prep editor came to last time it was drawn.
    ///
    /// Kept so the list above it can reserve the right amount of room whatever
    /// the editor is showing. Measured rather than predicted: every row added
    /// to the editor since it was written has been a chance to forget to add
    /// it to a sum somewhere else, and the grid tools took that chance.
    prep_height: f32,
    /// The track whose tempo is being typed, and the text as typed.
    ///
    /// Kept rather than read back from the collection each frame so that a
    /// half-typed "12" of "128" is not a tempo of twelve: the field holds what
    /// somebody is in the middle of saying, and the collection hears it when
    /// they are finished.
    bpm_entry: (Option<u32>, String),
    /// What the panels asked for this frame.
    pending: Vec<Pending>,
    /// A picker a button asked for, opened after the panel has finished
    /// drawing — a dialog cannot be opened from inside a closure that already
    /// holds the window.
    want_pick: Option<Picking>,
    status: String,

    /// Asks the window to repaint. Jobs hold a clone, so a result that arrives
    /// while nothing is moving still lands on screen.
    wake: Arc<dyn Fn() + Send + Sync>,
    wake_installed: bool,
}

/// What cueing a batch of tracks from their words would take, worked out
/// before any of it is started.
///
/// The three steps of the pass — render the stem, read it, place the cues —
/// with each track in whichever one it has got to, plus the tracks worth
/// asking a lyrics server about before any of that is spent on them.
struct WordsPlan {
    /// Words already read and already placed: cue them now, no job at all.
    known: Vec<u32>,
    /// Words read before they were placed against the stem. A decode, not
    /// another pass through the recogniser.
    placing: Vec<job::Realigning>,
    /// Stems that are on disk and have not been listened to. Tracks a lyrics
    /// server is being asked about are not here: see [`App::words_plan`].
    reading: Vec<job::Transcribable>,
    /// No stem yet, so the slow part comes first.
    rendering: Vec<u32>,
    /// Worth asking a server about before listening.
    asking: Vec<job::Askable>,
}

/// A batch of separations that has been asked for but not yet paid for.
///
/// Separation is the only job here measured in hours, and the way into it is
/// often a click that does not mention it: cueing from the words needs an
/// isolated vocal, so asking for cues on a playlist asks for a separation per
/// track in it. So the batch is held here and described first.
struct StemsAhead {
    /// The tracks that would be separated.
    tracks: Vec<u32>,
    /// Whether the stems are wanted for cues rather than for themselves —
    /// which is the case nobody sees coming, and reads differently.
    for_cues: bool,
}

/// Words a lyrics server offered for a track, waiting on somebody to say
/// whether they are the right ones.
///
/// What makes this worth asking rather than guessing: a remix is a different
/// length from the record it is built on and keeps only some of its words, so
/// the evidence is real and partial at the same time. Writing the wrong lyric
/// in quietly is worse than writing none, because a wrong lyric looks right —
/// it reads like a lyric, it cues like one, and nothing downstream doubts it.
struct Wondering {
    id: u32,
    found: booth_cli::tag::lyrics::Found,
}

/// A track's words, open for correcting by hand.
///
/// The answer to the case neither a recogniser nor a server gets right: a
/// remix that keeps half a lyric, a bootleg with a different vocal over it, a
/// record whose words nobody has written down correctly. One person with the
/// track in front of them settles it in a minute.
struct EditingWords {
    id: u32,
    text: String,
}

/// One correction to a beat grid, as a button asked for it.
///
/// Named rather than applied on the spot because the panel that draws the
/// buttons is reading the track it would change — the same reason every
/// other action here is a [`Pending`].
// No `Eq`: a tempo is a float, and a float is not equal to itself in the way
// `Eq` promises. `PartialEq` is all the tests and the pending queue ask for.
#[derive(Copy, Clone, Debug, PartialEq)]
enum GridFix {
    Halve,
    Double,
    /// Slide the whole grid by this many milliseconds.
    Nudge(i32),
    /// Slide the grid so a beat lands on the playhead.
    MoveTo,
    /// Call the beat nearest the playhead a downbeat.
    Downbeat,
    /// Set the tempo to exactly this, from the typed field or the two
    /// tenth-of-a-BPM buttons.
    ///
    /// The target rather than a step, so the three controls that change a
    /// tempo are one operation: a tenth up is "set it to what it is plus a
    /// tenth", worked out where the button is drawn and knowing what it says.
    Bpm(f64),
}

impl GridFix {
    /// Whether it means nothing without somewhere to act.
    ///
    /// Halving a tempo is true of the whole track; putting a beat somewhere is
    /// a question about one place in it, and the window's only answer to
    /// "where" is the playhead.
    fn needs_playhead(self) -> bool {
        matches!(self, GridFix::MoveTo | GridFix::Downbeat)
    }
}

/// Which cue an action is about.
///
/// A letter is not a name for one. Every memory cue carries zero, so "the cue
/// with letter 0" is whichever one happens to come first in the list — and
/// naming the memory cue under the playhead renamed a different one the moment
/// a track had two of them. Where it sits in the list is the name it has.
///
/// What it was is carried as well, and not for show: an action in the same
/// pass can re-sort the list — turning a hot cue into a memory cue does — and
/// an index taken before that points at a different cue afterwards. So the
/// index is checked before it is used, and a cue that has moved is found again
/// by what it is.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct WhichCue {
    at: usize,
    letter: u8,
    time_ms: u32,
}

impl WhichCue {
    fn of(at: usize, cue: &crate::library::CueMark) -> Self {
        Self { at, letter: cue.letter, time_ms: cue.time_ms }
    }

    /// Where it sits now, or `None` where it has gone.
    ///
    /// The index first, and when that no longer holds, the moment. Not the
    /// letter: an action in the same pass may have changed it — that is what
    /// turning a hot cue into a memory cue does — and a cue looked for by the
    /// letter it used to carry is a cue that cannot be found at all, so the
    /// rename queued behind the recast would be dropped.
    ///
    /// Two cues at the same millisecond would be told apart only by the index,
    /// which is the case this cannot answer. It takes a drag to land one on
    /// another to the millisecond, and the worse of the two outcomes — acting
    /// on the wrong one of a pair sitting in the same place — is the one that
    /// matters least.
    fn find(self, cues: &[crate::library::CueMark]) -> Option<usize> {
        match cues.get(self.at) {
            Some(cue) if cue.letter == self.letter && cue.time_ms == self.time_ms => Some(self.at),
            _ => cues.iter().position(|cue| cue.time_ms == self.time_ms),
        }
    }
}

/// Something a click asked for, to be done once the panel that drew it has
/// finished.
///
/// A button inside a panel cannot change the collection the panel is reading,
/// so it records what it wants instead. The alternative is a panel that redraws
/// half from the old state and half from the new.
enum Pending {
    Select(u32),
    Forget(u32),
    /// Take a track out of one playlist, by name. The name is carried rather
    /// than read at the time so that the list the menu was opened on is the
    /// one it acts on.
    RemoveFromPlaylist(u32, String),
    /// Put a playlist on the selected drive, or take it off.
    DrivePlaylist {
        name: String,
        on: bool,
    },
    /// Work with this drive from now on, by its place in the list.
    UseDrive(usize),
    /// Take this drive off the list. Nothing on the drive itself is touched.
    ForgetDrive(usize),
    /// Put this drive's image into a running emulator's USB slot.
    ToEmulator(usize),
    /// Put these tracks in the playlist of that name, making it if it is new.
    AddToPlaylist(Vec<u32>, String),
    /// Fold each of these copies into the track it is paired with, then send
    /// its file to the trash and forget it. The picks say how each disagreement
    /// between a copy and the track keeping it was settled.
    TrashDuplicates {
        /// The copy to be rid of, and the track it is folded into.
        going: Vec<(u32, u32)>,
        picked: Vec<(u32, crate::library::Field, crate::library::Side)>,
        /// What the kept track's tags should be once the folding is done, for
        /// each track something was folded into.
        tags: Vec<(u32, Vec<String>)>,
    },
    /// Open the sidebar's naming field, and put these in whatever it is called.
    NamePlaylistFor(Vec<u32>),
    Adopt(u32),
    AddTag(u32, String),
    RemoveTag(u32, String),
    CommitEdit,
    CancelEdit,
    /// The list needs rebuilding in a new order.
    Resort,
    WriteTags(u32),
    /// Take words a lyrics server offered, after somebody looked at them.
    TakeFoundWords(u32, Box<booth_cli::tag::lyrics::Found>),
    /// Ask a lyrics server about a track, and do nothing else.
    LookUpWords(u32),
    /// Find words for a track: a lyric file beside it, else the server.
    FindWords(u32),
    /// Correct a track's beat grid by hand.
    FixGrid {
        id: u32,
        what: GridFix,
        /// Where the playhead was when the button was pressed, which is what
        /// the two positional fixes act on.
        at: Option<u32>,
    },
    /// Measure this track's tempo and beats again, and change nothing else.
    Regrid(u32),
    /// Say that a beat falls here, for a grid to be fitted through.
    MarkBeat {
        id: u32,
        at_ms: u32,
    },
    /// Take the marks back off a track.
    ClearMarks(u32),
    /// Fit a grid through the marks.
    FitGrid(u32),
    /// Turn a grid kept beat by beat into a tempo and a downbeat.
    Flatten(u32),
    /// Open a track's words for correcting by hand.
    EditWords(u32),
    /// Keep the words somebody typed.
    SaveWords(u32),
    /// Go ahead with a batch of separations that has been warned about.
    SeparateMany {
        tracks: Vec<u32>,
        /// Whether the stems are wanted for cues rather than for themselves.
        for_cues: bool,
    },
    /// Move the cue at this place in a track's list, which is how a drag on
    /// the waveform names one. See [`App::move_cue`].
    MoveCue {
        id: u32,
        at: usize,
        time_ms: u32,
    },
    PlaceCue {
        id: u32,
        letter: u8,
        time_ms: u32,
    },
    /// Move, split, join or rename one of a track's phrase sections.
    EditPhrase {
        id: u32,
        edit: crate::library::PhraseEdit,
    },
    TogglePlayback(u32),
    /// Change how the waveform is coloured.
    PaintAs(wave::Paint),
    /// Turn the transport round: where you are, or what is left.
    CountDifferently,
    /// Settle one question: the fingerprint's answer, the path's, or the one
    /// the track already had.
    AnswerMatch {
        id: u32,
        answer: crate::identify::Answer,
    },
    /// Write, or do not write, tags into a file whose name looks nothing like
    /// them.
    AnswerNaming {
        id: u32,
        write: bool,
    },
    /// Pick which of a track's two names is right: the tags, or the file's own
    /// name. Recorded rather than acted on — the sheet's button applies them.
    ChooseName {
        id: u32,
        from_path: bool,
    },
    /// Move the deck to a position in a track, if that track is on it.
    SeekDeck {
        id: u32,
        time_ms: u32,
    },
    RemoveCue {
        id: u32,
        which: WhichCue,
    },
    RenameCue {
        id: u32,
        which: WhichCue,
        label: String,
    },
    /// Repaint a cue. The colour goes on the drive, so this is what a player
    /// lights its button with rather than a choice about this window.
    RecolorCue {
        id: u32,
        which: WhichCue,
        color: [u8; 3],
    },
    /// Turn a hot cue into a memory cue or back.
    ///
    /// Two different things on a player: eight buttons against as many marks
    /// as a track needs, and only the hot ones can be dropped into. Which a
    /// mark should be is a judgement about how the track gets played, so it is
    /// somebody's to change rather than the cueing pass's to settle.
    RecastCue {
        id: u32,
        which: WhichCue,
        /// Where it is going: a letter 1–8, or 0 for a memory cue.
        into: u8,
    },
    /// Listen to one track, whether or not it has been listened to before.
    Analyze(u32),
    /// Ask the fingerprint services about one track.
    Identify(u32),
    /// Render one track's stem kit.
    Separate(u32),
    /// Set one track's cues from its sections and its words, rendering the
    /// vocal stem and reading it first if that has not been done.
    AutoCue(u32),
    /// Put one track's path on the clipboard.
    CopyPath(u32),
    /// Show the whole track again.
    FitWave,
    /// Re-encode these into something a player opens.
    Convert(Vec<u32>),
}

/// The four names the inspector lets you edit.
#[derive(Clone, Default, PartialEq, Eq)]
struct Names {
    artist: String,
    title: String,
    album: String,
    year: String,
}

impl Names {
    fn of(track: &Track) -> Self {
        Self {
            artist: track.artist.clone(),
            title: track.title.clone(),
            album: track.album.clone(),
            year: track.year.map(|y| y.to_string()).unwrap_or_default(),
        }
    }
}

/// A track's names, while they are being edited.
///
/// Held separately from the record so that what is typed is not the collection
/// until it is committed: a name is not half a name on its way to being one,
/// and a rebuild in the middle of typing should not rearrange the list under
/// the cursor.
struct Edit {
    id: u32,
    names: Names,
    /// What the record said when the fields were filled in.
    ///
    /// The difference between the two is what somebody typed. Without it there
    /// was no telling that apart from the record having changed underneath —
    /// so a fingerprint lookup would write a name into the collection and the
    /// panel would go on showing the old one, because it had a copy and no
    /// reason to think the copy was stale.
    taken: Names,
}

impl Edit {
    fn of(track: &Track) -> Self {
        let names = Names::of(track);
        Self { id: track.id, taken: names.clone(), names }
    }

    /// Whether anything was actually changed.
    fn differs_from(&self, track: &Track) -> bool {
        let names = &self.names;
        names.artist.trim() != track.artist
            || names.title.trim() != track.title
            || names.album.trim() != track.album
            || names.year.trim() != track.year.map(|y| y.to_string()).unwrap_or_default()
    }

    /// Whether these are still the record's own names rather than somebody's.
    fn untouched(&self) -> bool {
        self.names == self.taken
    }
}

/// One line of the browser: a track, or a stem companion under one.
struct Row {
    track: Track,
    /// Companions are drawn indented, in the dim colour.
    indented: bool,
}

#[derive(Copy, Clone, PartialEq, Eq)]
enum Picking {
    Music,
    /// A rekordbox `master.db`, or the folder holding one.
    Rekordbox,
    Drive,
    Image,
    LibraryFolder,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, files: Vec<PathBuf>) -> Self {
        // The scheme is chosen before the style is built, because the style is
        // built out of it — installing first would paint one frame in the
        // wrong colours and then correct itself.
        let config_path = Config::path();
        let config = Config::load(&config_path);
        if !theme::use_scheme(&config.theme) {
            crate::warn!("no colour scheme called {}, so this is the usual one", config.theme);
        }
        theme::install(&cc.egui_ctx);
        let library_path = Library::default_path();
        let (library, status) = match Library::load(&library_path) {
            Ok(library) => {
                let count = library.tracks.len();
                (library, format!("{count} tracks"))
            }
            // A collection that will not parse must not be replaced with an
            // empty one; the window opens on a scratch library and says so.
            Err(e) => (Library::new(), format!("could not read {}: {e:#}", library_path.display())),
        };

        crate::info!(
            "collection {} — {}, {}, {}",
            library_path.display(),
            plural(library.tracks.len(), "track"),
            plural(library.playlists.len(), "playlist"),
            plural(library.drives.len(), "drive")
        );
        crate::info!(
            "library folder {} — music from elsewhere: {}",
            config.library_path.display(),
            config.on_external.label().to_lowercase()
        );

        let mut app = Self::assemble(library, library_path, config, config_path, status);

        // The device is opened once, at startup, and kept: opening one per
        // track costs a noticeable gap and, on some hosts, a click.
        match Player::open() {
            Ok(player) => {
                crate::info!("audio out at {} Hz", player.out_rate());
                app.player = Some(player);
            }
            Err(e) => {
                crate::warn!("no audio out: {e:#}");
                app.player_problem = Some(format!("{e:#}"));
            }
        }
        Self::finish(app, cc, files)
    }

    /// The window around a collection already in hand, with nothing opened for
    /// it — no audio device, no files read.
    ///
    /// Split out from [`App::new`] so that the interface can be driven in a
    /// test: what a button does is worth checking, and reading somebody's real
    /// collection and claiming their sound card is not part of it.
    fn assemble(
        library: Library,
        library_path: PathBuf,
        config: Config,
        config_path: PathBuf,
        status: String,
    ) -> Self {
        let config_sort = config.sort;
        let mut app = Self {
            library,
            library_path,
            config,
            config_path,
            text: String::new(),
            query: Query::default(),
            view: View::All,
            playlist: String::new(),
            rows: Vec::new(),
            sort: config_sort,
            selected: None,
            marked: std::collections::HashSet::new(),
            anchor: None,
            waveform: None,
            zoom: wave::Zoom::default(),
            envelopes: None,
            log: {
                let window = Arc::new(LogWindow::default());
                window.set_follow(true);
                window
            },
            player: None,
            player_problem: None,
            loading: None,
            runner: None,
            queued: VecDeque::new(),
            progress: None,
            step: None,
            drive: 0,
            plan: Plan::default(),
            sheet: false,
            details: false,
            settings: false,
            asking: Vec::new(),
            questions: Vec::new(),
            editing: None,
            pick: None,
            focus_bar: false,
            playlist_entry: String::new(),
            naming: None,
            help: false,
            duplicates: None,
            checked: None,
            looked_for_drives: None,
            kept_drives: std::collections::HashMap::new(),
            renames: Vec::new(),
            layout_moved: false,
            pending_save: false,
            pending_rebuild: false,
            tag_entry: String::new(),
            started: None,
            incompatible: Vec::new(),
            to_retag: Vec::new(),
            unlike: Vec::new(),
            remeasured: std::collections::HashSet::new(),
            re_enveloped: std::collections::HashSet::new(),
            want_cues: std::collections::HashSet::new(),
            stems_ahead: None,
            wondering: Vec::new(),
            editing_words: None,
            editing_grid: false,
            clashes: Vec::new(),
            settled: std::collections::HashMap::new(),
            drive_now: std::collections::HashMap::new(),
            playhead_ms: None,
            prep_height: PREP_HEIGHT_AT_FIRST,
            cue_entry: (None, String::new()),
            cue_menu: (None, String::new()),
            bpm_entry: (None, String::new()),
            pending: Vec::new(),
            want_pick: None,
            status,
            wake: Arc::new(|| {}),
            wake_installed: false,
        };
        app.rebuild();
        app
    }

    /// The last of startup that needs the window: the layout check's hooks and
    /// anything named on the command line.
    #[allow(unused_variables, unused_mut)]
    fn finish(mut app: Self, cc: &eframe::CreationContext<'_>, files: Vec<PathBuf>) -> Self {
        // Lets the layout check open the window on the sync sheet, which is
        // otherwise two clicks in. Animations are switched off with it, because
        // the check captures the first frame and would otherwise photograph
        // every fade half-finished. Compiled out of any ordinary build.
        // Animations off for every one of these, not just the sheets: the
        // check captures the first frame, and a window caught halfway through
        // its fade-in photographs as a half-transparent one.
        #[cfg(feature = "screenshot")]
        if std::env::var_os("BOOTH_OPEN_SETTINGS").is_some() {
            app.settings = true;
            cc.egui_ctx.style_mut(|style| style.animation_time = 0.0);
        }
        #[cfg(feature = "screenshot")]
        if std::env::var_os("BOOTH_DRIVE_SYNCED").is_some() {
            let wanted = match app.library.drives.first() {
                Some(drive) => crate::sync::wanted(&app.library, drive),
                None => Vec::new(),
            };
            let written: Vec<crate::library::Written> = wanted
                .iter()
                .filter_map(|id| app.library.get(*id))
                .map(|track| crate::library::Written {
                    id: track.id,
                    prep: crate::sync::fingerprint(track),
                    ..Default::default()
                })
                .collect();
            if let Some(drive) = app.library.drives.first_mut() {
                drive.written = written;
            }
            app.replan();
        }
        #[cfg(feature = "screenshot")]
        if std::env::var_os("BOOTH_OPEN_SYNC").is_some() {
            app.sheet = true;
            cc.egui_ctx.style_mut(|style| style.animation_time = 0.0);
        }
        #[cfg(feature = "screenshot")]
        if std::env::var_os("BOOTH_CHECK").is_some() {
            app.verify_showing(false);
            cc.egui_ctx.style_mut(|style| style.animation_time = 0.0);
        }
        #[cfg(feature = "screenshot")]
        if std::env::var_os("BOOTH_OPEN_RENAMES").is_some() {
            app.renames = app
                .library
                .tracks
                .iter()
                .filter_map(|t| Some((t.id, crate::library::name_without_copy_number(&t.path)?)))
                .collect();
            cc.egui_ctx.style_mut(|style| style.animation_time = 0.0);
        }
        #[cfg(feature = "screenshot")]
        if std::env::var_os("BOOTH_OPEN_DUPES").is_some() {
            app.duplicates = Some(Dupes::default());
            if let Ok(also) = std::env::var("BOOTH_ALSO_KEEP") {
                if let Ok(id) = also.parse::<u32>() {
                    app.duplicates.as_mut().unwrap().keeping.insert(id);
                }
            }
            cc.egui_ctx.style_mut(|style| style.animation_time = 0.0);
        }
        #[cfg(feature = "screenshot")]
        if std::env::var_os("BOOTH_OPEN_HELP").is_some() {
            app.help = true;
            cc.egui_ctx.style_mut(|style| style.animation_time = 0.0);
        }
        #[cfg(feature = "screenshot")]
        if std::env::var_os("BOOTH_SELECT_FIRST").is_some() {
            app.selected = app.library.tracks.first().map(|t| t.id);
        }
        #[cfg(feature = "screenshot")]
        if let Some(which) = std::env::var_os("BOOTH_OPEN_SHEET") {
            match which.to_string_lossy().as_ref() {
                "settings" => app.settings = true,
                "log" => {
                    app.log.set_open(true);
                    app.log.set_level(crate::log::Level::Debug);
                }
                "adopt" => app.asking = app.library.tracks.iter().map(|t| t.id).take(3).collect(),
                "grid" => {
                    app.editing_grid = true;
                    app.playhead_ms = Some(40_000);
                    // Zoomed in, because that is where the grid tools are
                    // used and where the marks cross the picture. A shot of
                    // them at full-track zoom shows the row of buttons and
                    // none of what pressing one is for.
                    app.zoom = wave::Zoom { start: 0.2, span: 0.06 };
                    // A few beats marked, so the shot shows what a fit is
                    // worked out from rather than an empty row of buttons.
                    if let Some(id) = app.selected {
                        if let Some(track) = app.library.get_mut(id) {
                            // As a fraction of the track, so they land inside
                            // the view above whatever this record's length is
                            // — placed at a time in seconds they fell outside
                            // it, twice, and a mark nobody can see is a shot
                            // of the feature not working.
                            let period = 60_000.0 / track.bpm.max(1.0);
                            let from = track.duration_secs * 1000.0 * 0.21;
                            track.beat_marks = (0..3)
                                .map(|n| (from + n as f64 * period * 4.0).round() as u32)
                                .collect();
                        }
                        // The panel draws from its own copy of the track, so
                        // changing the collection without this shows a row
                        // that has not heard about it.
                        app.rebuild();
                    }
                }
                "stems" => {
                    let tracks: Vec<u32> =
                        app.library.tracks.iter().map(|t| t.id).take(6).collect();
                    app.stems_ahead = Some(StemsAhead { tracks, for_cues: true });
                }
                "words" => {
                    if let Some(track) = app.library.tracks.first() {
                        // The case the sheet exists for, rather than an easy
                        // one: a lyric the right length to be another
                        // pressing, keeping some of what was heard off this
                        // stem and not the rest. A fixture where both sides
                        // agree outright shows a sheet nobody would ever have
                        // needed to see.
                        let lines = [
                            "walking through the city at night",
                            "hold me closer now",
                            "and the lights go down on us",
                            "hold me closer now",
                            "hold me closer now",
                            "and the lights go down on us",
                        ];
                        app.wondering.push(Wondering {
                            id: track.id,
                            found: booth_cli::tag::lyrics::Found {
                                synced: booth_cli::transcribe::Transcript::default(),
                                plain: lines.iter().map(|line| line.to_string()).collect(),
                                instrumental: false,
                                artist: track.artist.clone(),
                                title: track.title.clone(),
                                duration_secs: track.duration_secs - 94.0,
                            },
                        });
                    }
                }
                _ => app.sheet = true,
            }
            cc.egui_ctx.style_mut(|style| style.animation_time = 0.0);
        }

        if !files.is_empty() {
            app.import(files);
        }
        app
    }

    fn running(&self) -> bool {
        self.runner.is_some()
    }

    /// Say something to the user, and to the log.
    ///
    /// One call rather than two so that what the window showed and what the
    /// file recorded cannot disagree about a run.
    fn note(&mut self, text: impl Into<String>, color: Color32) {
        let text = text.into();
        // Compared rather than matched on, because the colours are a scheme's
        // now and a scheme is chosen while the program runs.
        let level = if color == theme::alert() {
            crate::log::Level::Error
        } else if color == theme::amber() {
            crate::log::Level::Warn
        } else {
            crate::log::Level::Info
        };
        crate::log::record(level, text);
    }

    // -- the list ----------------------------------------------------------

    /// Re-run the query and rebuild the rows.
    ///
    /// Everything the browser shows comes through here, so a fixed view and a
    /// typed query are the same mechanism — there is no second filtering path
    /// that could disagree with the one the bar describes.
    /// Note the size a panel came out at, and write it to the settings once
    /// the drag that changed it has finished.
    ///
    /// Not on every frame it differs: a drag arrives as a stream of sub-point
    /// changes, and saving on each would rewrite the settings file a hundred
    /// times across one pull. So the number is kept as it moves and committed
    /// when the pointer comes up.
    ///
    /// And only while the pointer is down, because that is the only way a
    /// panel is ever meant to change size. Everything else that moves one is
    /// something being done to the window rather than to the panel: dragging
    /// the window narrower squeezes the side panels, and recording that would
    /// make a temporary squeeze into the size they open at from then on.
    fn remember_panel(
        &mut self,
        dragging: bool,
        which: fn(&mut crate::config::Panels) -> &mut f32,
        size: f32,
    ) {
        if !dragging {
            return;
        }
        let stored = which(&mut self.config.panels);
        if !crate::config::Panels::differs(*stored, size) {
            return;
        }
        *stored = size;
        self.layout_moved = true;
    }

    /// Whether a section is currently in a window of its own.
    fn is_out(&self, pane: Pane) -> bool {
        self.config.popped.contains(&pane)
    }

    /// Send a section to its own window, or bring it back.
    ///
    /// Written down straight away rather than on the next pointer release:
    /// this is one click with one answer, not a drag, and a second-screen
    /// arrangement that had to be rebuilt every morning would not be worth
    /// having.
    fn set_out(&mut self, pane: Pane, out: bool) {
        self.config.popped.retain(|&which| which != pane);
        if out {
            self.config.popped.push(pane);
        }
        crate::debug!(
            "{} {} the main window",
            pane.title(),
            if out { "left" } else { "came back to" }
        );
        if let Err(e) = self.config.save(&self.config_path) {
            crate::warn!("could not save which panels are out: {e:#}");
        }
    }

    /// Draw one section, wherever it happens to be.
    ///
    /// One place that says what each section is made of, so a pane in its own
    /// window and the same pane in the main one cannot come to differ.
    fn pane(&mut self, ui: &mut Ui, pane: Pane) {
        match pane {
            Pane::Collection => self.sidebar(ui),
            // Not sharing: whatever the prep editor was holding room for, it is
            // not underneath this one.
            Pane::Browser => self.track_list(ui, false),
            Pane::Prep => self.prep(ui),
            Pane::Inspector => self.inspector(ui),
            Pane::Drives => self.dock(ui),
        }
    }

    /// The sections that are in windows of their own.
    ///
    /// Immediate viewports rather than deferred ones. A deferred viewport
    /// repaints on its own clock, which is what the log's window wants — but it
    /// pays for that with a callback that must be `Send + Sync + 'static`, so
    /// all it can reach is what has been put behind an `Arc`. These panels read
    /// and write the collection, the settings and the queue of things to do
    /// next, all of which live on the window itself; an immediate viewport is
    /// drawn inside this frame and so can simply be handed it.
    ///
    /// What that costs is a repaint in step with the main window rather than
    /// on its own, which for panels that show the same collection is what you
    /// want anyway.
    fn popped_panes(&mut self, ctx: &egui::Context) {
        // Cloned first: each window is drawn with the whole of `self` in hand,
        // and one of them may ask to come back while the list is still being
        // walked.
        let out = self.config.popped.clone();
        let mut returning = Vec::new();

        for pane in out {
            let mut back = false;
            ctx.show_viewport_immediate(
                pane.window(),
                egui::ViewportBuilder::default()
                    .with_title(format!("Booth \u{2014} {}", pane.title()))
                    .with_inner_size(pane.size())
                    .with_min_inner_size([300.0, 160.0]),
                |ctx, _class| {
                    egui::TopBottomPanel::top("pane-bar").frame(bar_frame()).show(ctx, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(theme::label_text(pane.title()))
                                    .size(theme::LABEL)
                                    .color(theme::dim())
                                    .strong(),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui
                                        .button("Put it back")
                                        .on_hover_text(
                                            "Return this to where it sits in the main window",
                                        )
                                        .clicked()
                                    {
                                        back = true;
                                    }
                                },
                            );
                        });
                    });
                    egui::CentralPanel::default()
                        .frame(pane_frame())
                        .show(ctx, |ui| self.pane(ui, pane));

                    // Shutting the window puts the section back rather than
                    // hiding it. A pane that could be closed out of existence
                    // would leave a gap in the main window and nothing in it
                    // to say where the missing thing had gone.
                    if ctx.input(|i| i.viewport().close_requested()) {
                        back = true;
                    }
                },
            );
            if back {
                returning.push(pane);
            }
        }

        for pane in returning {
            self.set_out(pane, false);
        }
    }

    /// The middle of the window: the list, the prep editor, or neither.
    fn centre(&mut self, ui: &mut Ui) {
        match (self.is_out(Pane::Browser), self.is_out(Pane::Prep)) {
            (false, false) => self.browser(ui),
            (false, true) => self.track_list(ui, false),
            (true, false) => self.prep(ui),
            (true, true) => self.everything_elsewhere(ui),
        }
    }

    /// What the middle says when both halves of it are in other windows.
    ///
    /// Somewhere to say where things went and a way to get them back, because
    /// an empty rectangle says neither.
    fn everything_elsewhere(&mut self, ui: &mut Ui) {
        ui.add_space(28.0);
        ui.vertical_centered(|ui| {
            ui.label(
                RichText::new("The list and the prep editor are in windows of their own.")
                    .color(theme::dim()),
            );
            ui.add_space(8.0);
            if ui
                .button("Put them back")
                .on_hover_text("Bring the browser and the prep pane back into the main window.")
                .clicked()
            {
                self.set_out(Pane::Browser, false);
                self.set_out(Pane::Prep, false);
            }
        });
    }

    /// The menu that sends a section to its own window and brings it back.
    ///
    /// On the query bar rather than in the section it acts on, because the bar
    /// is the one strip that never goes anywhere: a control that popped out
    /// along with the thing it controls would be a door that shuts behind you.
    fn panes_menu(&mut self, ui: &mut Ui) {
        ui.menu_button(RichText::new("⧉").font(theme::mono(11.0)).color(theme::dim()), |ui| {
            ui.set_min_width(230.0);
            ui.label(RichText::new("In its own window").color(theme::dim()).size(theme::SMALL));
            ui.separator();
            for pane in Pane::ALL {
                let mut out = self.is_out(pane);
                if ui.checkbox(&mut out, pane.title()).on_hover_text(pane.about()).changed() {
                    self.set_out(pane, out);
                }
            }
            ui.separator();
            // The log has had a window of its own all along, and it works the
            // other way round — it is not in the main window to begin with —
            // so it is listed here rather than pretending to be a sixth pane.
            let mut log = self.log.is_open();
            if ui
                .checkbox(&mut log, "Log")
                .on_hover_text("The whole run, in a window of its own")
                .changed()
            {
                self.log.set_open(log);
            }
        })
        .response
        .on_hover_text("Send a part of the window to a window of its own");
    }

    /// Write the sizes out, if a drag has just finished changing one.
    ///
    /// Panels and columns both: they are dragged the same way, they are
    /// written to the same file, and one flag between them means a drag that
    /// moved both still costs one write.
    fn save_layout(&mut self, ctx: &egui::Context) {
        if !self.layout_moved || ctx.input(|i| i.pointer.any_down()) {
            return;
        }
        self.layout_moved = false;
        let widths: Vec<String> = self
            .config
            .columns
            .columns
            .iter()
            .filter(|slot| slot.shown)
            .map(|slot| format!("{} {:.0}", slot.column.name(), slot.width))
            .collect();
        match self.config.save(&self.config_path) {
            Ok(()) => crate::debug!(
                "layout saved: panels {:?}, columns {}",
                self.config.panels,
                widths.join(", ")
            ),
            Err(e) => crate::warn!("could not save the layout: {e:#}"),
        }
    }

    fn rebuild(&mut self) {
        let started = std::time::Instant::now();
        self.query = Query::parse(&self.text);

        let duplicates = self
            .query
            .terms
            .iter()
            .find_map(|term| match &term.test {
                query::Test::Duplicates(keys) => {
                    Some(query::duplicate_ids(&self.library.tracks, keys))
                }
                _ => None,
            })
            .unwrap_or_default();
        let drives = self.library.drive_index();
        let playlists = self.library.playlist_index();
        let context =
            Context { now: now(), drives: &drives, playlists: &playlists, duplicates: &duplicates };

        let mut in_view: Vec<Track> = self
            .library
            .tracks
            .iter()
            .filter(|track| match self.view {
                View::All => true,
                View::Unprepared => track.unprepared(),
                View::Attention => track.needs_attention().is_some(),
                View::Playlist => self
                    .library
                    .playlists
                    .iter()
                    .find(|p| p.name == self.playlist)
                    .is_some_and(|p| p.tracks.contains(&track.id)),
            })
            .filter(|track| self.query.is_empty() || self.query.matches(track, &context))
            .cloned()
            .collect();

        // Sorted before the companions are added, so a stem stays under the
        // track it came from however the list is ordered.
        let sort = self.sort;
        in_view.sort_by(|a, b| rows::compare(a, b, sort));

        // The companions come after the query rather than through it: an
        // acapella is shown because its parent matched, which is what keeps a
        // stem kit from splitting away from the record it belongs to.
        self.rows.clear();
        for track in in_view {
            let companions = self.library.companions(&track);
            self.rows.push(Row { track, indented: false });
            for companion in companions {
                self.rows.push(Row { track: companion, indented: true });
            }
        }

        if !self.rows.iter().any(|row| Some(row.track.id) == self.selected) {
            self.selected = self.rows.first().map(|row| row.track.id);
            self.waveform = None;
            self.zoom = wave::Zoom::default();
            self.envelopes = None;
        }
        self.replan();

        crate::debug!(
            "listed {} of {} ({}) by {} in {:.1} ms{}",
            self.rows.iter().filter(|row| !row.indented).count(),
            plural(self.library.tracks.len(), "track"),
            plural(self.rows.len(), "row"),
            self.sort.column.name(),
            started.elapsed().as_secs_f64() * 1000.0,
            match self.text.trim().is_empty() {
                true => String::new(),
                false => format!(" — query {:?}", self.text.trim()),
            }
        );
    }

    fn replan(&mut self) {
        self.plan = match self.library.drives.get(self.drive) {
            Some(drive) => sync::plan(&self.library, drive),
            None => Plan::default(),
        };
    }

    /// Ask the drive what has happened to it since it was last written.
    ///
    /// A CDJ-3000X can move a cue or re-grid a track on the deck, and the next
    /// sync would write over it without a word. So before the sheet offers to
    /// write, the stick is read: which tracks it has changed, and which of
    /// those have also been changed here. Only the ones changed in both places
    /// are a question — a track changed only on the drive is not being
    /// rewritten anyway, and one changed only here is what a sync is for.
    ///
    /// Reads the drive, so it is done when the sheet opens and when the button
    /// on it is pressed, not on every frame. An image is skipped: there is no
    /// player that could have edited one.
    fn examine_drive(&mut self) {
        self.clashes.clear();
        self.settled.clear();
        self.drive_now.clear();
        let Some(drive) = self.library.drives.get(self.drive).cloned() else { return };
        if drive.is_image || !drive.path.exists() {
            return;
        }

        let found = sync::on_the_drive(&drive.path, &drive, self.config.onelibrary_key());
        self.clashes = sync::conflicts(&self.library, &drive, &found);
        self.drive_now = found;
        for clash in &self.clashes {
            self.settled.insert(clash.id, clash.default_side());
        }
        if !self.clashes.is_empty() {
            crate::info!(
                "{} on {} changed both here and on the drive",
                plural(self.clashes.len(), "track"),
                drive.label
            );
        }
    }

    /// Make what the player left the collection's copy, for every track whose
    /// drive version is being kept.
    ///
    /// Read off the stick's own analysis files. This is what turns "leave it
    /// alone" into "take it": without it, keeping the drive's copy protects an
    /// edit nobody here can see, and the collection goes on showing something
    /// the drive does not have.
    ///
    /// The track is still not written again afterwards. Only the tempo comes
    /// back from the grid — a collection keeps a tempo and a downbeat, not
    /// thousands of beat times — so writing it back would flatten a grid the
    /// deck may have bent, which is the thing being protected.
    fn take_what_the_player_left(&mut self) {
        let Some(drive) = self.library.drives.get(self.drive).cloned() else { return };
        if drive.is_image {
            return;
        }
        let keeping: Vec<u32> = self
            .settled
            .iter()
            .filter(|(_, side)| **side == sync::Side::Theirs)
            .map(|(id, _)| *id)
            .collect();

        let (mut taken, mut unreadable) = (0usize, 0usize);
        for id in keeping {
            let Some(written) = drive.written.iter().find(|w| w.id == id) else { continue };
            let Some(found) = sync::what_the_player_left(&drive.path, written) else {
                unreadable += 1;
                continue;
            };
            if let Some(track) = self.library.get_mut(id) {
                if crate::library::take_prep(track, &found) {
                    taken += 1;
                }
            }

            // The reconciliation, recorded here rather than left to the write:
            // taking the drive's copy *is* the two sides agreeing, and it has
            // to hold even when there turns out to be nothing else to write.
            // Without this the same question comes back on every sync.
            let Some(track) = self.library.get(id) else { continue };
            let settled = (sync::fingerprint(track), self.drive_now.get(&id).cloned());
            if let Some(drive) = self.library.drives.get_mut(self.drive) {
                if let Some(written) = drive.written.iter_mut().find(|w| w.id == id) {
                    written.prep = settled.0;
                    written.theirs = settled.1;
                }
            }
        }

        if taken > 0 {
            crate::info!("took what the player left on {}", plural(taken, "track"));
            self.note(
                format!("{} now show what the player made of them", plural(taken, "track")),
                theme::text(),
            );
        }
        // Said rather than passed over. The drive's copy is still protected —
        // the track is not written — but the collection cannot show it, and
        // somebody who chose "the drive's" is entitled to know that is all
        // that happened.
        if unreadable > 0 {
            self.note(
                format!(
                    "{} left alone, but {} analysis could not be read back",
                    plural(unreadable, "track"),
                    match unreadable {
                        1 => "its",
                        _ => "their",
                    }
                ),
                theme::amber(),
            );
        }
    }

    /// The stamps to record for the tracks a write has just touched.
    ///
    /// Taken after the write rather than before it: writing a track rewrites
    /// its analysis files, so a stamp taken beforehand describes a drive that
    /// no longer exists and the next examination would read this program's own
    /// write as a player's edit.
    fn restamp_drive(&mut self) {
        let Some(drive) = self.library.drives.get(self.drive).cloned() else { return };
        if drive.is_image || !drive.path.exists() {
            return;
        }
        let found = sync::on_the_drive(&drive.path, &drive, self.config.onelibrary_key());
        let Some(drive) = self.library.drives.get_mut(self.drive) else { return };
        for written in &mut drive.written {
            if let Some(stamp) = found.get(&written.id) {
                written.theirs = Some(stamp.clone());
            }
        }
    }

    fn selected_track(&self) -> Option<&Track> {
        let id = self.selected?;
        self.rows.iter().find(|row| row.track.id == id).map(|row| &row.track)
    }

    /// Move the selection by `delta` rows, which is how a crate is dug through.
    ///
    /// With `extend`, the row moved to joins the selection instead of
    /// replacing it, measured from the anchor — so holding shift and pressing
    /// down four times takes five rows, and letting go and pressing down once
    /// takes one.
    fn step(&mut self, delta: isize, extend: bool) {
        if self.rows.is_empty() {
            return;
        }
        let at = self
            .selected
            .and_then(|id| self.rows.iter().position(|row| row.track.id == id))
            .unwrap_or(0);
        // The row being left is where a range grows from, if nothing has set
        // that yet. Without this, shift-down in a window nobody had clicked in
        // took only the row it arrived at, and took only the next one again on
        // the press after that — the gesture did nothing at all until a plain
        // click or arrow had happened first.
        if self.anchor.is_none() {
            self.anchor = self.selected.or_else(|| self.rows.first().map(|row| row.track.id));
        }
        let next = (at as isize + delta).clamp(0, self.rows.len() as isize - 1) as usize;
        let id = self.rows[next].track.id;

        self.select(id);
        match extend {
            true => self.mark_range_to(id),
            false => self.mark_only(id),
        }
    }

    /// Move the cursor to a row. Says whether the deck went with it.
    ///
    /// Switching between a track and its stems is a comparison — is the vocal
    /// clean through the drop, is the groove still there without it — and a
    /// comparison you have to re-cue by hand is not one anybody makes twice.
    /// See [`carry`] for what that means for the deck.
    fn select(&mut self, id: u32) -> bool {
        let was = self.selected;
        if was == Some(id) {
            return false;
        }
        self.selected = Some(id);
        // The picture is per file — a stem's is not its parent's — so it is
        // dropped either way and read again for the row now showing.
        self.waveform = None;
        self.envelopes = None;

        // Taken off the deck rather than from the running note of the
        // playhead, because that note only follows the row that is selected
        // and the selection has just moved.
        let deck = self.player.as_ref().and_then(|player| {
            Some((player.loaded()?, player.position_secs(), player.is_playing()))
        });

        match carry(was, id, deck) {
            Carry::Restart => {
                self.zoom = wave::Zoom::default();
                self.playhead_ms = None;
                false
            }
            // The zoom stays as well as the playhead: it is the same minute of
            // the same music, and throwing the view away to show it again is
            // the opposite of a comparison.
            Carry::Hold => false,
            Carry::From { secs, playing } => {
                self.playhead_ms = Some((secs * 1000.0) as u32);
                if playing {
                    self.audition(id, Some(secs));
                }
                playing
            }
        }
    }

    /// Make one row the whole selection, and the point a range grows from.
    fn mark_only(&mut self, id: u32) {
        self.marked.clear();
        self.marked.insert(id);
        self.anchor = Some(id);
    }

    /// Select every row between the anchor and `id`, in the order they are
    /// listed — which is what the eye means by "these ones", whichever
    /// direction they were picked in.
    fn mark_range_to(&mut self, id: u32) {
        // Pinned, not just read: a range that left the anchor unset would find
        // it unset again on the next shift-press and measure from wherever the
        // cursor had got to, so the selection would never grow past two rows.
        let anchor = *self.anchor.get_or_insert(id);
        let at = |wanted: u32| self.rows.iter().position(|row| row.track.id == wanted);
        let (Some(from), Some(to)) = (at(anchor), at(id)) else {
            self.mark_only(id);
            return;
        };
        let (low, high) = (from.min(to), from.max(to));
        self.marked = self.rows[low..=high]
            .iter()
            // Companions come along with their parents rather than on their
            // own: a range drawn down the list sweeps over them, and a stem
            // is not a thing to analyse or put on a drive by itself.
            .filter(|row| !row.indented)
            .map(|row| row.track.id)
            .collect();
    }

    /// Take everything the query has left showing.
    ///
    /// Everything showing rather than the whole collection, because the query
    /// bar is how a set is picked here: narrowing to what you want and then
    /// taking all of it is the gesture, and a select-all that reached past the
    /// filter would undo the narrowing it was meant to finish.
    ///
    /// The cursor does not move, and the anchor follows it, so a shift-arrow
    /// straight after this grows from the row being looked at rather than
    /// from the top of the list.
    fn mark_showing(&mut self) {
        self.marked = self
            .rows
            .iter()
            // Companions are not selected on their own: a stem is not a thing
            // to analyse or put on a drive by itself, and it goes wherever its
            // parent goes.
            .filter(|row| !row.indented)
            .map(|row| row.track.id)
            .collect();
        self.anchor = self.selected.or_else(|| self.rows.first().map(|row| row.track.id));
    }

    /// Add a row to the selection, or take it out again.
    fn mark_toggle(&mut self, id: u32) {
        if !self.marked.remove(&id) {
            self.marked.insert(id);
        }
        self.anchor = Some(id);
    }

    /// What an action applies to: the selection when there is one worth the
    /// name, and otherwise everything the query has left showing.
    ///
    /// One selected row is not a selection — it is where the cursor happens to
    /// be, which is not the same as having chosen anything, and treating it as
    /// one would turn "analyse what I am looking at" into "analyse this one"
    /// for anybody who had clicked a row to see its waveform.
    fn acting_on(&self, wanted: impl Fn(&Track) -> bool) -> Vec<u32> {
        match self.marked.len() > 1 {
            true => self
                .rows
                .iter()
                .filter(|row| !row.indented && self.marked.contains(&row.track.id))
                .filter(|row| wanted(&row.track))
                .map(|row| row.track.id)
                .collect(),
            false => self.showing(wanted),
        }
    }

    // -- jobs --------------------------------------------------------------

    /// Queue a job, and run it if nothing else is.
    fn start(&mut self, job: Job) {
        crate::debug!("queued {} ({} waiting)", job.name(), self.queued.len());
        self.queued.push_back(job);
        self.pump();
    }

    /// Start the next job if the runner is free.
    fn pump(&mut self) {
        if self.running() {
            return;
        }
        let Some(job) = self.queued.pop_front() else { return };
        crate::info!("started {}", job.name());
        self.started = Some(std::time::Instant::now());
        self.progress = Some((0, 0));
        self.runner = Some(Runner::start(job, self.wake.clone()));
    }

    fn import(&mut self, paths: Vec<PathBuf>) {
        self.start(Job::Import { paths, recursive: true });
    }

    // -- keeping a local copy ----------------------------------------------

    /// The tracks among these whose files are not in the library folder.
    ///
    /// Stem companions are skipped: they are rows, not files, and their parent
    /// answers for them.
    fn external(&self, ids: &[u32]) -> Vec<u32> {
        ids.iter()
            .filter_map(|id| self.library.get(*id))
            .filter(|track| {
                track.role == crate::library::Role::Track
                    && track.path.exists()
                    && !self.config.holds(&track.path)
            })
            .map(|track| track.id)
            .collect()
    }

    /// Act on the tracks among these that have no local copy.
    ///
    /// Called wherever music is actually reached for — after an import, and
    /// before anything that reads or writes the files. What happens is the
    /// user's decision, and the default is to take a copy: a track referenced
    /// on a stick that is not plugged in is a track that is not there on the
    /// night.
    fn ensure_local(&mut self, ids: &[u32]) {
        let external = self.external(ids);
        if external.is_empty() {
            return;
        }
        match self.config.on_external {
            OnExternal::Copy => self.adopt(&external),
            OnExternal::Ask => {
                for id in external {
                    if !self.asking.contains(&id) {
                        self.asking.push(id);
                    }
                }
            }
            OnExternal::Leave => {}
        }
    }

    /// Queue copies of these tracks into the library folder.
    fn adopt(&mut self, ids: &[u32]) {
        let tracks: Vec<Adoptable> = ids
            .iter()
            .filter_map(|id| self.library.get(*id))
            .map(|track| Adoptable {
                id: track.id,
                path: track.path.clone(),
                artist: match track.artist.trim() {
                    "" => "Unknown Artist".to_string(),
                    artist => artist.to_string(),
                },
            })
            .collect();
        if tracks.is_empty() {
            return;
        }
        self.asking.retain(|id| !ids.contains(id));
        self.start(Job::Adopt { tracks, config: Box::new(self.config.clone()) });
    }

    /// The tracks showing that a batch action should act on.
    ///
    /// Companions are never included: they are rows, not files, and their
    /// parent answers for them.
    fn showing(&self, wanted: impl Fn(&Track) -> bool) -> Vec<u32> {
        self.rows
            .iter()
            .filter(|row| !row.indented && wanted(&row.track))
            .map(|row| row.track.id)
            .collect()
    }

    /// Pair ids with the files they name, dropping any that have gone.
    ///
    /// A job handed a path that is not there fails per file and says so, which
    /// is right for a batch and wasteful for one track the user just clicked;
    /// this is where that is caught once.
    fn files_for(&mut self, ids: &[u32]) -> Vec<(u32, PathBuf)> {
        let mut found = Vec::new();
        for id in ids {
            let Some(track) = self.library.get(*id) else { continue };
            match track.path.exists() {
                true => found.push((track.id, track.path.clone())),
                false => crate::warn!("#{id} is not where it was: {}", track.path.display()),
            }
        }
        found
    }

    /// Listen to these tracks, whether or not they have been listened to before.
    ///
    /// The same call behind the batch button and the one on a single row: a
    /// re-analysis is not a different operation from a first one, and having
    /// two of them is how they come to disagree.
    /// Every stem row of these tracks that has files to be drawn from.
    ///
    /// Taken off the collection rather than off the browser: a crate being
    /// prepared has its stems drawn whether or not the companion rows happen to
    /// be showing, and whether or not anybody has clicked on one.
    fn stem_pictures(&self, ids: &[u32]) -> Vec<job::Drawable> {
        let mut drawing = Vec::new();
        for id in ids {
            let Some(track) = self.library.get(*id) else { continue };
            for companion in self.library.companions(track) {
                if let Some(sources) = picture_sources(&companion) {
                    drawing.push(job::Drawable { id: companion.id, sources });
                }
            }
        }
        drawing
    }

    fn analyze_tracks(&mut self, ids: &[u32]) {
        let waiting = self.files_for(ids);
        if waiting.is_empty() {
            self.note("nothing to analyse", theme::dim());
            return;
        }
        crate::info!("analysing {}", plural(waiting.len(), "track"));

        // Reading a file is reaching for it, so this is one of the moments the
        // copy-in policy is about. The copy is queued first, and the analysis
        // behind it, so it runs against whatever the track's path is by then.
        let ids: Vec<u32> = waiting.iter().map(|(id, _)| *id).collect();
        // Asking by hand clears the "already measured" mark, so the waveform is
        // read again rather than the cached one being kept.
        for id in &ids {
            self.remeasured.remove(id);
            self.re_enveloped.remove(id);
        }
        if self.selected.is_some_and(|selected| ids.contains(&selected)) {
            self.waveform = None;
            self.zoom = wave::Zoom::default();
            self.envelopes = None;
        }
        self.ensure_local(&ids);
        self.start(Job::Analyze(waiting.clone()));

        // And the stems, where there are any. Listening to a record is the
        // moment its pictures are made, and a stem row has a picture of its
        // own — so leaving those to be drawn one at a time as somebody happens
        // to click on them means a crate that has just been prepared is not
        // prepared. The grid, the cues and the phrases are still the record's
        // and are not measured again here; only the drawing is per file.
        let drawing = self.stem_pictures(&ids);
        if !drawing.is_empty() {
            crate::info!("drawing {}", plural(drawing.len(), "stem"));
            for row in &drawing {
                self.remeasured.remove(&row.id);
                crate::library::forget_waveform(row.id);
            }
            self.start(Job::Draw(drawing));
        }

        // Listening to a track and asking what it is are the same errand, so
        // they are queued together — but the lookup is paced by two services'
        // rate limits, so it goes behind the analysis rather than in front.
        if self.config.identify {
            let unnamed: Vec<u32> = waiting
                .iter()
                .map(|(id, _)| *id)
                .filter(|id| self.library.get(*id).is_some_and(|track| !track.identified))
                .collect();
            self.identify_tracks(&unnamed, false);
        }
    }

    // -- rekordbox -------------------------------------------------------

    fn import_rekordbox(&mut self, path: PathBuf) {
        let key = match booth_cli::rekordbox::resolve(self.config.rekordbox_key()) {
            Ok(key) => key,
            Err(e) => {
                crate::warn!("{e:#}");
                self.note(format!("{e:#}"), theme::amber());
                return;
            }
        };
        crate::info!("reading {}", path.display());
        self.start(Job::Rekordbox { path, key });
    }

    /// Merge a rekordbox library into the collection.
    ///
    /// Matched by file path, because that is the only thing the two libraries
    /// genuinely share — an id means nothing across them, and matching on
    /// artist and title would merge two versions of a record.
    ///
    /// Nothing already here is overwritten. A track this program has analysed
    /// has a grid it measured itself, and rekordbox's opinion of the same file
    /// is not better for being older; what comes across is what is *missing* —
    /// names on an untitled file, cues where there are none, the play count and
    /// the rating and the My Tags, which this program has no other way to know.
    /// Files rekordbox knows about that are not here yet are added.
    fn merge_rekordbox(&mut self, collection: &booth_cli::rekordbox::master::Collection) -> String {
        use std::collections::HashMap;

        let mut by_path: HashMap<PathBuf, u32> =
            self.library.tracks.iter().map(|t| (t.path.clone(), t.id)).collect();

        let mut added = 0usize;
        let mut filled = 0usize;
        let mut missing = 0usize;
        // rekordbox id to this collection's id, so the playlists can be
        // rebuilt afterwards.
        let mut ours: HashMap<String, u32> = HashMap::new();

        for track in &collection.tracks {
            let id = match by_path.get(&track.path) {
                Some(id) => *id,
                None => {
                    // A path rekordbox has and this does not. It is still worth
                    // adding: a track whose file has moved is a track to go
                    // looking for, and it carries all its prep with it.
                    if !track.path.exists() {
                        missing += 1;
                    }
                    let id = self.library.add(&track.path);
                    by_path.insert(track.path.clone(), id);
                    added += 1;
                    id
                }
            };
            ours.insert(track.id.clone(), id);
            if self.library.get_mut(id).is_some_and(|into| into.fill_from(track)) {
                filled += 1;
            }
        }

        let playlists = self.merge_rekordbox_playlists(collection, &ours);

        crate::info!(
            "rekordbox: added {added}, filled in {filled}, {playlists} playlists, \
             {missing} whose files are not where rekordbox left them"
        );
        let mut said = format!(
            "rekordbox: {} added, {} filled in, {}",
            added,
            filled,
            plural(playlists, "playlist")
        );
        if missing > 0 {
            said.push_str(&format!(" · {missing} files not where rekordbox left them"));
        }
        said
    }

    /// Bring the playlists across, keeping their folders.
    fn merge_rekordbox_playlists(
        &mut self,
        collection: &booth_cli::rekordbox::master::Collection,
        ours: &std::collections::HashMap<String, u32>,
    ) -> usize {
        let mut brought = 0;
        for playlist in &collection.playlists {
            let tracks: Vec<u32> =
                playlist.track_ids.iter().filter_map(|id| ours.get(id).copied()).collect();
            if tracks.is_empty() {
                continue;
            }
            // A playlist of the same name in the same folder is the same
            // playlist, and re-importing must not leave two of it.
            let existing = self
                .library
                .playlists
                .iter_mut()
                .find(|p| p.name == playlist.name && p.folder == playlist.folder);
            match existing {
                Some(found) => {
                    for id in tracks {
                        if !found.tracks.contains(&id) {
                            found.tracks.push(id);
                        }
                    }
                }
                None => self.library.playlists.push(crate::library::Playlist {
                    name: playlist.name.clone(),
                    folder: playlist.folder.clone(),
                    tracks,
                }),
            }
            brought += 1;
        }
        brought
    }

    /// Re-encode these into a format the hardware opens.
    fn convert_tracks(&mut self, ids: &[u32]) {
        let waiting: Vec<job::Convertible> = self
            .files_for(ids)
            .into_iter()
            .map(|(id, path)| job::Convertible { id, path })
            .collect();
        if waiting.is_empty() {
            self.note("nothing to convert", theme::dim());
            return;
        }
        crate::info!("converting {}", plural(waiting.len(), "file"));
        self.incompatible.retain(|id| !ids.contains(id));
        self.start(Job::Convert(waiting));
    }

    /// Everything showing that has never been listened to.
    fn analyze_unprepared(&mut self) {
        let waiting = self.acting_on(|track| !track.analyzed);
        if waiting.is_empty() {
            self.note("nothing showing needs analysing", theme::dim());
            return;
        }
        self.analyze_tracks(&waiting);
    }

    /// Analyse everything showing, whether it has been analysed or not.
    ///
    /// What this is for is a change in what analysis produces — a new
    /// waveform, a different grid, a better key detector. Every track already
    /// in the collection is then holding an answer from the old code, and
    /// until now there was no way to ask for them all again: the batch button
    /// only ever offered the tracks that had never been done, and the row menu
    /// meant doing a crate one right-click at a time.
    ///
    /// The same call as a first analysis, because a re-analysis is not a
    /// different operation and two of them is how they come to disagree.
    fn analyze_showing(&mut self) {
        let waiting = self.acting_on(|_| true);
        if waiting.is_empty() {
            self.note("nothing showing to analyse", theme::dim());
            return;
        }
        self.analyze_tracks(&waiting);
    }

    /// Render stem kits for these tracks.
    fn separate_tracks(&mut self, ids: &[u32]) {
        let waiting = self.files_for(ids);
        if waiting.is_empty() {
            self.note("nothing to separate", theme::dim());
            return;
        }
        let ids: Vec<u32> = waiting.iter().map(|(id, _)| *id).collect();
        for id in &ids {
            self.re_enveloped.remove(id);
            // The companions are about to be made of different audio, so the
            // pictures measured from the old files are no longer of anything.
            // The parent's is untouched: its own file has not changed.
            for role in crate::library::Role::PARTS {
                let companion = crate::library::companion_id(*id, role);
                crate::library::forget_waveform(companion);
                self.remeasured.remove(&companion);
            }
        }
        self.ensure_local(&ids);
        self.start(Job::Separate {
            tracks: waiting,
            stems_in: self.config.stems_location(),
            backend: Backend::Demucs,
            quality: self.config.stem_quality.to_cli(),
        });
    }

    /// `again` renders over kits that are already there, which is what to do
    /// when the model or the quality setting has changed since.
    fn render_stems(&mut self, again: bool) {
        let waiting = self.acting_on(|track| again || track.stems.is_empty());
        if waiting.is_empty() {
            self.note("everything showing already has a stem kit", theme::dim());
            return;
        }
        self.separate_or_warn(&waiting, false);
    }

    /// Start separating these, and cue each one from its words once its vocal
    /// is there when that is what the stems were for.
    fn separate_for(&mut self, ids: &[u32], for_cues: bool) {
        if for_cues {
            self.want_cues.extend(ids.iter().copied());
            self.note(
                format!("rendering stems for {} first", plural(ids.len(), "track")),
                theme::dim(),
            );
        }
        self.separate_tracks(ids);
    }

    /// The same, except that a batch says what it will cost before spending it.
    ///
    /// One track goes straight through: the note it prints is warning enough
    /// for a few minutes, and a dialog in front of every record somebody cues
    /// one at a time would be worse than no dialog at all. More than one is
    /// where the minutes turn into an afternoon.
    fn separate_or_warn(&mut self, ids: &[u32], for_cues: bool) {
        if ids.len() < 2 || !self.config.warn_before_stems {
            self.separate_for(ids, for_cues);
            return;
        }
        self.stems_ahead = Some(StemsAhead { tracks: ids.to_vec(), for_cues });
    }

    // -- cues from the words -----------------------------------------------

    /// What a lyrics server needs to be asked about one track.
    ///
    /// `then_read` is the vocal stem owed to the recogniser if the server does
    /// not settle it, and is `None` where nothing is owed — a track with no
    /// stem yet, or a lookup somebody asked for on its own.
    fn askable(&self, track: &Track, then_read: Option<PathBuf>) -> job::Askable {
        job::Askable {
            id: track.id,
            artist: track.artist.clone(),
            title: track.display_title(),
            duration_secs: track.duration_secs,
            identified_surely: track.identified_surely,
            heard: track.lyrics.clone(),
            then_read,
        }
    }

    /// Ask a lyrics server about these tracks and nothing else.
    ///
    /// The whole of it: no separation, no recogniser, and nothing started if
    /// the server comes back empty-handed. What somebody gets for a click is
    /// one request a track and either the words or a note saying there were
    /// none — which is worth having on its own, because a hit here is the
    /// minutes of a separation and a recogniser pass not spent.
    ///
    /// Find words for one track: a lyric file beside it, else the server.
    ///
    /// The file first, and not only because it is free. A file somebody put
    /// next to *this* file is their answer about this record; a server match
    /// on name and length is an inference about it, however good. And a lyric
    /// file works for the white labels and the dubplates, which is most of
    /// what a database has never heard of.
    ///
    /// Returns whether the collection changed here and now.
    fn find_words(&mut self, id: u32) -> bool {
        let id = crate::library::family(id);
        let Some(track) = self.library.get(id) else { return false };
        let (path, duration_secs) = (track.path.clone(), track.duration_secs);

        let Some(file) = booth_cli::tag::lyrics::beside(&path) else {
            // Nothing beside it, so ask. `look_up_words` says its own piece
            // when there is no name to ask under.
            self.look_up_words(&[id]);
            return false;
        };
        let found = match booth_cli::tag::lyrics::read_beside(&file, duration_secs) {
            Ok(found) => found,
            Err(e) => {
                crate::warn!("#{id}: could not read {}: {e:#}", file.display());
                self.note(format!("could not read {}", file.display()), theme::alert());
                return false;
            }
        };
        if found.is_empty() {
            self.note(
                format!(
                    "{} has nothing in it",
                    file.file_name().unwrap_or_default().to_string_lossy()
                ),
                theme::amber(),
            );
            return false;
        }

        let took = self.take_found_words(id, &found);
        if took {
            if let Some(track) = self.library.get_mut(id) {
                // `take_found_words` marks them as a server's, which is right
                // for everything that goes through it but this.
                track.words_from = crate::library::WordsFrom::Beside;
            }
            self.note(
                format!(
                    "{} from {}",
                    plural(found.plain.len(), "line"),
                    file.file_name().unwrap_or_default().to_string_lossy()
                ),
                theme::go(),
            );
        }
        took
    }

    /// Which of these tracks a lyrics server can be asked about, and with
    /// what. Starts nothing, so a test can ask what a click would send.
    ///
    /// A companion row has no words of its own; it shows its parent's. So
    /// asking about an acapella is asking about the record.
    fn worth_asking(&self, ids: &[u32]) -> Vec<job::Askable> {
        let mut wanted: Vec<u32> = ids.iter().map(|id| crate::library::family(*id)).collect();
        wanted.sort_unstable();
        wanted.dedup();
        wanted
            .iter()
            .filter_map(|id| self.library.get(*id))
            .filter(|track| !track.artist.trim().is_empty() && !track.title.trim().is_empty())
            .map(|track| self.askable(track, None))
            .collect()
    }

    /// Returns whether anything could be asked about at all.
    fn look_up_words(&mut self, ids: &[u32]) -> bool {
        let asking = self.worth_asking(ids);
        if asking.is_empty() {
            // Said rather than passed over in silence: somebody asked for this
            // one, and a lyrics database has nothing to go on without a name.
            self.note(
                "a lyrics server is asked by artist and title, and these have none",
                theme::amber(),
            );
            return false;
        }
        crate::info!("asking a lyrics server about {}", plural(asking.len(), "track"));
        self.start(Job::LookUpWords { tracks: asking });
        true
    }

    /// Where each of these tracks is in the three steps of cueing from the
    /// words, and which of them are worth asking a lyrics server about first.
    ///
    /// Separated from the pass that acts on it because this is the whole of
    /// the decision and none of the work: it starts nothing, so it can be
    /// asked what it would do.
    fn words_plan(&mut self, ids: &[u32], again: bool) -> WordsPlan {
        // A companion row has no cues of its own; it shows its parent's. So
        // asking for cues on an acapella is asking for them on the record.
        let mut wanted: Vec<u32> = ids.iter().map(|id| crate::library::family(*id)).collect();
        wanted.sort_unstable();
        wanted.dedup();

        let mut known = Vec::new();
        let mut placing = Vec::new();
        let mut reading: Vec<job::Transcribable> = Vec::new();
        let mut rendering: Vec<u32> = Vec::new();
        // A name that says "instrumental" is somebody telling you the answer,
        // and it is worth taking: finding it out instead costs a separation
        // and a pass through the recogniser. Marked before the pass below
        // rather than inside it, so the track is skipped by the same rule as
        // one that was listened to and found silent.
        let named: Vec<u32> = wanted
            .iter()
            .copied()
            .filter(|id| {
                self.library.get(*id).is_some_and(|track| {
                    !track.instrumental
                        && booth_cli::transcribe::named_as_instrumental(&format!(
                            "{} {}",
                            track.display_title(),
                            track.path.file_name().unwrap_or_default().to_string_lossy()
                        ))
                })
            })
            .collect();
        if !named.is_empty() {
            crate::info!(
                "{} say they are instrumental, so the words are not read off them",
                plural(named.len(), "track")
            );
            for id in &named {
                if let Some(track) = self.library.get_mut(*id) {
                    track.instrumental = true;
                }
            }
        }

        for id in wanted {
            let Some(track) = self.library.get(id) else { continue };
            // An instrumental is not a track waiting to be read: it has been
            // read, or its name said so. Asking again is minutes for the same
            // answer, so it takes holding shift like any other re-run.
            if track.instrumental && !again {
                known.push(id);
                continue;
            }
            // The words are not cleared here. They are replaced when the new
            // ones arrive, so a re-read that never starts — no recogniser, a
            // missing file — leaves the track with the words it had.
            let unread = again || track.lyrics.is_empty();
            match (unread, track.lyrics_aligned, track.stems.vocals.clone()) {
                // Words already read and already placed: nothing to wait for.
                (false, true, _) => known.push(id),
                // Words read before they were placed against the stem. The
                // words are right and the times are not, so this is a decode
                // rather than another pass through the recogniser.
                (false, false, Some(vocals)) => {
                    placing.push(job::Realigning { id, vocals, lyrics: track.lyrics.clone() })
                }
                // Same, but the stem has gone. Nothing can be measured, so cue
                // from what is there rather than refusing to cue at all.
                (false, false, None) => known.push(id),
                (true, _, Some(vocals)) => reading.push(job::Transcribable { id, vocals }),
                (true, _, None) => rendering.push(id),
            }
        }

        // Before anything expensive: somebody may have written these words
        // down already. A hit costs one request where listening costs a
        // recogniser pass — and a separation too where there is no stem yet —
        // and the answer is a person's rather than a machine's guess at a
        // vocal.
        //
        // Every track about to have its words read, whether it has a stem or
        // not. Asking only about the stemless ones meant that re-reading a
        // track never asked at all, since a track with a stem goes straight to
        // the recogniser — which is the one case where somebody has said out
        // loud that the words it has are wrong.
        //
        // Only where there is a name to ask under: "Unknown Artist" asks
        // nothing of anybody.
        let asking: Vec<job::Askable> = reading
            .iter()
            .map(|wanting| (wanting.id, Some(wanting.vocals.clone())))
            .chain(rendering.iter().map(|id| (*id, None)))
            .filter_map(|(id, stem)| self.library.get(id).map(|track| (track, stem)))
            .filter(|(track, _)| !track.artist.trim().is_empty() && !track.title.trim().is_empty())
            .map(|(track, then_read)| self.askable(track, then_read))
            .collect();
        // Held back rather than read alongside. A recogniser pass that runs
        // anyway would spend the minutes the lookup exists to save, and would
        // then write its guess over the words the lookup found. Whatever the
        // server does not settle is handed to it in `Update::WordsLookedUp`.
        let asked: std::collections::HashSet<u32> = asking.iter().map(|track| track.id).collect();
        reading.retain(|wanting| !asked.contains(&wanting.id));

        WordsPlan { known, placing, reading, rendering, asking }
    }

    /// Set cues from a track's sections and from what is sung over them.
    ///
    /// Three steps, each skipped when it has already been taken: render the
    /// stems, read the vocal one, place the cues. Only the last is instant,
    /// which is why the words are kept in the collection once they have been
    /// heard — a track whose lyrics are known is re-cued with no job at all.
    ///
    /// `again` reads the words a second time on tracks that already have
    /// them, which is what to do when the recogniser, the stems or the
    /// placing have improved since. Everything else about the pass is the
    /// same, including rendering a stem for anything that has lost one.
    ///
    /// Returns whether the collection changed here and now, as opposed to work
    /// having been queued that will change it later.
    fn auto_cue_tracks(&mut self, ids: &[u32], again: bool) -> bool {
        let WordsPlan { known, placing, reading, rendering, asking } = self.words_plan(ids, again);
        let mut placed = 0;
        for id in &known {
            placed += self.auto_cue(*id);
        }
        if !known.is_empty() {
            self.note(
                format!("{} from words already read", crate::library::plural(placed, "cue")),
                theme::text(),
            );
        }

        if !placing.is_empty() {
            crate::info!(
                "placing the words on {} against the stem they were read from",
                plural(placing.len(), "track")
            );
            self.start(Job::Realign { tracks: placing });
        }

        // Before anything expensive: somebody may have written these words
        // down already. A hit costs one request where listening costs a
        // recogniser pass — and a separation too where there is no stem yet.
        if !asking.is_empty() {
            crate::info!("asking a lyrics server about {}", plural(asking.len(), "track"));
            self.start(Job::LookUpWords { tracks: asking });
        }

        if reading.is_empty() && rendering.is_empty() {
            return !known.is_empty();
        }
        // Asked once, before any of the minutes are spent. A separation that
        // finishes and only then finds there is no recogniser to hand the stem
        // to has wasted the expensive half of the work.
        if let Err(why) = self.config.whisper.ready() {
            self.note(why, theme::amber());
            return !known.is_empty();
        }
        if !rendering.is_empty() {
            self.separate_or_warn(&rendering, true);
        }
        if !reading.is_empty() {
            self.read_words(reading);
        }
        !known.is_empty()
    }

    /// Whether a track's words are settled, so a stem landing on it is for
    /// placing them rather than for reading over them.
    ///
    /// A lyrics server answers in a second where a separation takes minutes,
    /// so by the time a stem is rendered the words may already be in. Words
    /// somebody wrote down outrank a recogniser's guess at the same vocal, and
    /// a recogniser handed a track already known to be an instrumental spends
    /// minutes to write down things nobody sang.
    fn words_answered(&self, id: u32) -> bool {
        self.library.get(id).is_some_and(|track| {
            track.instrumental
                || (track.words_from != crate::library::WordsFrom::Recogniser
                    && !track.lyrics.is_empty())
        })
    }

    /// Take the words a lyrics server offered, and cue from them.
    ///
    /// The synced lyric where there is one, because a line with a time on it
    /// is a cue; otherwise the plain text, which is still worth having for the
    /// panel and for searching even though it places nothing.
    ///
    /// Times that come back are **not** trusted as this pressing's. They were
    /// synced against one release and the library is full of extended mixes,
    /// so the words go in marked as needing placing and the existing aligner
    /// moves them onto singing measured off the stem — the same path words
    /// read before the aligner existed take. Where there is no stem they stay
    /// where the server put them, which is the best available answer and is
    /// why the words are still worth taking without one.
    ///
    /// Returns whether the collection changed.
    fn take_found_words(&mut self, id: u32, found: &booth_cli::tag::lyrics::Found) -> bool {
        let timed = !found.synced.lines.is_empty();
        let lyrics = match timed {
            true => crate::library::lyrics_from(&found.synced),
            false => found
                .plain
                .iter()
                .map(|text| crate::library::Lyric { start_ms: 0, end_ms: 0, text: text.clone() })
                .collect(),
        };

        let Some(track) = self.library.get_mut(id) else { return false };
        if found.instrumental {
            // The server saying so outright is worth as much as listening and
            // finding nothing, and costs one request instead of minutes.
            track.instrumental = true;
            crate::info!("#{id} is an instrumental, so there are no words to read");
            return true;
        }
        if lyrics.is_empty() {
            return false;
        }
        let lines = lyrics.len();
        track.refrains = crate::library::refrains_from(&lyrics);
        track.lyrics = lyrics;
        track.words_from = crate::library::WordsFrom::Server;
        // Somebody wrote these down, so there is no recogniser's doubt to
        // record — and the old doubt belonged to words that are gone.
        track.heard_surely = None;
        // Timed against another pressing, so they are words without places
        // until the stem says otherwise.
        track.lyrics_aligned = !timed;
        let has_stem = track.stems.vocals.is_some();

        crate::info!(
            "#{id} took {} from a lyrics server{}",
            plural(lines, "line"),
            match (timed, has_stem) {
                (true, true) => ", to be placed against its stem",
                (true, false) => ", timed against another pressing",
                _ => "",
            }
        );
        self.auto_cue(id);
        self.prep_changed(id);
        true
    }

    /// Hand stems that are already on disk to the recogniser.
    ///
    /// With the language the collection is mostly in, where the setting does
    /// not name one and the collection has an answer of its own. A recogniser
    /// left to guess takes it from the first few seconds of an isolated vocal,
    /// which are usually a breath — see [`crate::library::common_language`].
    fn read_words(&mut self, tracks: Vec<job::Transcribable>) {
        crate::info!("reading the words off {}", crate::library::plural(tracks.len(), "track"));
        let mut whisper = self.config.whisper.clone();
        if whisper.language.trim().is_empty() {
            if let Some(mostly) = crate::library::common_language(&self.library.tracks) {
                crate::info!("telling the recogniser this collection is mostly {mostly}");
                whisper.language = mostly;
            }
        }
        self.start(Job::Transcribe { tracks, whisper });
    }

    /// Set one track's cues from everything known about it, and say how many
    /// hot cues that came to.
    ///
    /// Two sources: the sections the phrase analysis found, and the moments the
    /// words did — where the singing starts, where the line the track repeats
    /// most first lands, and every time it comes back. Which eight of those a
    /// player ends up holding is the engine's decision rather than this one, so
    /// that a cue set placed here and a cue set placed by the analyser mean the
    /// same thing and are the same colours.
    ///
    /// The hot cues are replaced wholesale. The memory cue is not: it is what
    /// the grid is anchored to, and moving it would move every bar line in the
    /// track.
    fn auto_cue(&mut self, id: u32) -> usize {
        use booth_cli::analysis::cues::{self, Candidate, Reason};
        use booth_cli::analysis::structure::Kind;

        let Some(track) = self.library.get(id) else { return 0 };
        let beats = beat_times(track);
        let mut candidates: Vec<Candidate> = track
            .phrases
            .iter()
            .filter_map(|phrase| {
                let kind = Kind::from_label(&phrase.kind)?;
                Some(Candidate::new(snap_to(&beats, phrase.start_ms, 4), Reason::Section(kind)))
            })
            .collect();

        let words = crate::library::transcript(&track.lyrics);
        for moment in words.moments() {
            let mut candidate = cues::from_moment(&moment);
            candidate.time_ms = snap_back(&beats, candidate.time_ms);
            candidates.push(candidate);
        }

        let start = track
            .cues
            .iter()
            .find(|cue| cue.letter == 0)
            .map(|cue| cue.time_ms)
            .or_else(|| beats.first().copied())
            .unwrap_or(0);
        let before = track.cues.clone();
        let name = format!("{} \u{2014} {}", track.artist, track.display_title());
        let lines = track.lyrics.len();

        // The last bar the grid names, so "End" lands somewhere a player will
        // park rather than wherever the file stops.
        let end = beats.last().copied();
        let placed = crate::job::cue_marks(&cues::assemble(start, end, candidates));
        let hot = placed.iter().filter(|cue| cue.letter != 0).count();
        let decided = what_the_words_decided(&words, &before, &placed);
        crate::info!("cueing #{id} {name} from {}", plural(lines, "line"));
        for line in &decided {
            crate::info!("  {line}");
        }

        if let Some(track) = self.library.get_mut(id) {
            track.cues = placed;
            // Kept as well as logged. The log is where this went and only
            // went, so the account of why a track's cues are where they are
            // lasted until the next thing was logged.
            track.words_decided = decided;
        }
        self.prep_changed(id);
        hot
    }

    fn auto_cue_showing(&mut self, again: bool) {
        let waiting =
            self.acting_on(|track| again || (track.lyrics.is_empty() && !track.instrumental));
        if waiting.is_empty() {
            self.note("the words have been read for everything showing", theme::dim());
            return;
        }
        self.auto_cue_tracks(&waiting, again);
    }

    fn collect(&mut self) {
        let Some(runner) = &self.runner else { return };
        let updates = runner.drain();
        // Taken once, before the loop: an import looks for a kit already on
        // disk for every file it finds, and the collection is borrowed by then.
        let stems_in = self.config.stems_location();
        let mut finished = false;
        let mut changed = false;
        // What this job brought in, so that the copy-in policy can be applied
        // to it once, when the import is done rather than per file.
        let mut imported: Vec<u32> = Vec::new();
        // Stems that finished for a track which is waiting on its words. The
        // reading is queued once, after the whole batch has been folded in,
        // rather than a job per track as each kit lands.
        let mut to_read: Vec<job::Transcribable> = Vec::new();
        // Tracks whose stem landed on words they already had. See the
        // `Separated` arm below.
        let mut to_cue: Vec<u32> = Vec::new();
        // Lookups that found nothing and had no recogniser pass behind them.
        let mut unanswered = 0usize;
        // Whether anything timed a separation, so the settings are written
        // once at the end of the batch rather than once per track.
        let mut paced = false;

        for update in updates {
            match update {
                Update::Imported(record) => {
                    let id = self.library.add(&record.path);
                    if let Some(track) = self.library.get_mut(id) {
                        // The scan knows more about the file than the record
                        // the library made from its name; the id and anything
                        // the user has since added stay.
                        let mut merged = *record;
                        merged.id = id;
                        merged.tags = track.tags.clone();
                        merged.added = track.added;
                        merged.last_played = track.last_played;
                        merged.play_count = track.play_count;
                        merged.analyzed = track.analyzed;
                        merged.stems = job::find_stems(&stems_in, &merged.path);
                        *track = merged;
                    }
                    imported.push(id);
                    changed = true;
                }
                Update::Decoded { id, sound } => {
                    crate::debug!(
                        "decoded #{id}: {:.1}s, {} Hz, {} ch",
                        sound.duration_secs(),
                        sound.rate,
                        sound.channels
                    );
                    if let Some(player) = &mut self.player {
                        player.load(id, sound);
                        if let Some(ms) = self.playhead_ms {
                            player.seek_secs(ms as f64 / 1000.0);
                        }
                        if self.loading.take().is_some_and(|(_, play)| play) {
                            player.play();
                        }
                    }
                }
                Update::Wrote(rows) => {
                    // Matched back by the file each row was made from, which is
                    // the only thing the drive's database and the collection
                    // have in common — its ids are the drive's, not ours.
                    let Some(drive) = self.library.drives.get_mut(self.drive) else { continue };
                    for (from, row) in rows {
                        // A row is a track's or one of its stems'. The drive's
                        // database does not tell them apart — a stem is a track
                        // to a player — so the collection is what says which,
                        // and a stem's row is kept under the parent it hangs
                        // from rather than beside it.
                        if let Some(track) = self.library.tracks.iter().find(|t| t.path == from) {
                            if let Some(written) =
                                drive.written.iter_mut().find(|w| w.id == track.id)
                            {
                                written.row = Some(row);
                            }
                            continue;
                        }
                        let Some(parent) = self.library.tracks.iter().find(|t| t.stems.has(&from))
                        else {
                            continue;
                        };
                        let Some(written) = drive.written.iter_mut().find(|w| w.id == parent.id)
                        else {
                            continue;
                        };
                        match written.stems.iter_mut().find(|(path, _)| *path == from) {
                            Some(slot) => slot.1 = row,
                            None => written.stems.push((from, row)),
                        }
                    }
                    changed = true;

                    // A drive has just been written, which is the moment its
                    // contents are worth keeping and the one moment this
                    // program knows exactly where it is. Waiting for the next
                    // sweep of the mounted volumes would find it too, but only
                    // if it happens to be mounted somewhere a sweep looks.
                    let (path, label, is_image) =
                        (drive.path.clone(), drive.label.clone(), drive.is_image);
                    // What the drive holds for each track now, so the next
                    // sheet can tell a player's edit from this program's own.
                    self.restamp_drive();
                    if self.config.keep_drives && !is_image {
                        let listing = crate::backup::listing(&path);
                        let state = crate::backup::digest(&listing);
                        // The same name the sweep would give it, so that a
                        // drive written here and found again later is one drive
                        // with one folder of copies rather than two.
                        let name = drive_name(&path, &label);
                        let known =
                            self.kept_drives.get(&name).map(|seen| seen.fingerprint.clone());
                        // A write is a reason to store the drive whatever was
                        // stored before, so this asks only whether it is the
                        // same state — never how long ago the last copy was.
                        if !listing.is_empty() && known.as_deref() != Some(state.as_str()) {
                            self.keep_drive(&path, &name, &state, listing);
                        }
                    }
                }
                Update::Kept(kept) => {
                    self.note(format!("kept {}: {}", kept.drive, kept.summary()), theme::text());
                    // Music copied into the library is music the collection
                    // should know about, and importing is what reads a file and
                    // makes a row out of it.
                    if !kept.adopted.is_empty() {
                        self.import(kept.adopted.clone());
                    }
                }
                Update::Played { drive, root, sessions } => {
                    self.take_history(&drive, &root, sessions);
                    changed = true;
                }
                Update::Verified(report) => {
                    if let Some(checked) = &mut self.checked {
                        checked.troubles.push(*report);
                    }
                }
                Update::Orphans(paths) => {
                    if let Some(checked) = &mut self.checked {
                        checked.orphans = paths;
                    }
                }
                Update::Hashed { id, file, audio } => {
                    if let Some(track) = self.library.get_mut(id) {
                        track.file_hash = file;
                        track.audio_hash = audio;
                        changed = true;
                    }
                }
                Update::Identified { id, best } => {
                    if let Some(track) = self.library.get_mut(id) {
                        track.identified = true;
                    }
                    match best {
                        Some(found) => self.consider(id, found),
                        None => self.consider_path(id),
                    }
                    changed = true;
                }
                Update::Envelopes { id, envelopes } => {
                    crate::debug!("measured stems for #{id}: {} columns", envelopes.columns());
                    if let Err(e) = crate::library::cache_envelopes(id, &envelopes) {
                        crate::warn!("could not cache the stem envelopes: {e}");
                    }
                    if Some(id) == self.selected {
                        self.envelopes = Some((id, envelopes));
                    }
                }
                Update::Adopted { id, to } => {
                    crate::info!("#{id} copied into the library: {}", to.display());
                    if let Some(track) = self.library.get_mut(id) {
                        // The record now points at the copy. The original is
                        // untouched on disk; the collection simply stops
                        // depending on it.
                        track.path = to;
                    }
                    changed = true;
                }
                Update::Rekordbox(collection) => {
                    let brought = self.merge_rekordbox(&collection);
                    self.note(brought, theme::go());
                    changed = true;
                }
                Update::Converted { id, to } => {
                    crate::info!("#{id} converted to {}", to.display());
                    if let Some(track) = self.library.get_mut(id) {
                        // The record follows the new file. The original stays
                        // where it was — a conversion that turns out wrong
                        // should leave the thing it was made from behind.
                        track.path = to.clone();
                        track.format = "flac".into();
                        track.float_samples = false;
                        track.protected = false;
                        track.bytes = std::fs::metadata(&to).map(|m| m.len()).unwrap_or(0);
                        // Everything measured came off the old file, so it is
                        // measured again rather than assumed to carry over.
                        track.analyzed = false;
                    }
                    self.remeasured.remove(&id);
                    changed = true;
                }
                Update::Analyzed(analyzed) => {
                    crate::debug!(
                        "analysed #{}: {:.2} BPM (confidence {:.1}), key {}, {} beats, {} phrases, {} cues",
                        analyzed.id,
                        analyzed.bpm,
                        analyzed.grid_confidence,
                        if analyzed.key.is_empty() { "none" } else { &analyzed.key },
                        analyzed.beats,
                        analyzed.phrases.len(),
                        analyzed.cues.len()
                    );
                    if let Some(track) = self.library.get_mut(analyzed.id) {
                        track.bpm = analyzed.bpm;
                        track.grid_confidence = analyzed.grid_confidence;
                        track.has_grid = analyzed.has_grid;
                        track.beats = analyzed.beats;
                        track.beat_ms = analyzed.beat_ms.clone();
                        track.key = analyzed.key.clone();
                        track.key_confidence = analyzed.key_confidence;
                        track.energy = analyzed.energy;
                        track.intensity = analyzed.intensity;
                        track.phrases = analyzed.phrases.clone();
                        track.cues = analyzed.cues.clone();
                        track.loudness_lufs = analyzed.loudness_lufs;
                        track.peak_dbtp = analyzed.peak_dbtp;
                        track.duration_secs = analyzed.duration_secs;
                        track.sample_rate = analyzed.sample_rate;
                        track.channels = analyzed.channels;
                        track.bitrate_kbps = bitrate(track.bytes, analyzed.duration_secs);
                        track.analyzed = true;
                    }
                    // An analysis replaces the grid, the cues and the phrases,
                    // which is exactly what a player can also change.
                    self.prep_changed(analyzed.id);
                    let of = [analyzed.path.clone()];
                    if let Err(e) =
                        crate::library::cache_waveform(analyzed.id, &of, &analyzed.bands)
                    {
                        self.note(format!("could not cache the waveform: {e}"), theme::dim());
                    }
                    if Some(analyzed.id) == self.selected {
                        self.waveform = Some((analyzed.id, analyzed.bands.clone()));
                    }
                    changed = true;
                }
                Update::Regridded { id, bpm, grid_confidence, has_grid, beats, beat_ms } => {
                    crate::debug!("re-measured #{id}: {bpm:.2} BPM, {beats} beats");
                    // The six fields the measurement is allowed to touch. The
                    // cues, the phrases and the key are deliberately not here:
                    // see `App::regrid`.
                    if let Some(track) = self.library.get_mut(id) {
                        track.bpm = bpm;
                        track.grid_confidence = grid_confidence;
                        track.has_grid = has_grid;
                        track.beats = beats;
                        track.beat_ms = beat_ms;
                        track.analyzed = true;
                    }
                    // So the field stops showing the tempo that was replaced.
                    if self.bpm_entry.0 == Some(id) {
                        self.bpm_entry = (None, String::new());
                    }
                    self.prep_changed(id);
                    changed = true;
                }
                Update::Separated { id, kit, took_secs } => {
                    // Read before it is moved in, because what happens next
                    // depends on whether the part the recogniser needs is
                    // among what was rendered.
                    let vocals = kit.vocals.clone();
                    let audio = self.library.get(id).map(|track| track.duration_secs);
                    if let Some(track) = self.library.get_mut(id) {
                        track.stems = kit;
                    }
                    // So the next batch can say how long it will take. Timed
                    // here rather than guessed from the hardware, because the
                    // same track is minutes on a CPU and seconds on a GPU.
                    if let Some(audio) = audio {
                        self.config.record_stem_pace(took_secs, audio as f32);
                        paced = true;
                    }
                    if self.want_cues.remove(&id) {
                        match (self.words_answered(id), vocals) {
                            // Whether those words still need placing is the
                            // question `auto_cue_tracks` already answers, so
                            // it is left to answer it.
                            (true, _) => to_cue.push(id),
                            (false, Some(vocals)) => {
                                to_read.push(job::Transcribable { id, vocals })
                            }
                            (false, None) => self.note(
                                "the separation produced no vocal stem to read",
                                theme::amber(),
                            ),
                        }
                    }
                    changed = true;
                }
                Update::Drawn { id, sources, bands } => {
                    if let Err(e) = crate::library::cache_waveform(id, &sources, &bands) {
                        self.note(format!("could not cache the waveform: {e}"), theme::dim());
                    }
                    if Some(id) == self.selected {
                        self.waveform = Some((id, bands));
                    }
                    changed = true;
                }
                Update::WordsLookedUp { id, found, verdict, then_read } => {
                    use booth_cli::tag::lyrics::Verdict;
                    // The recogniser pass this track would have had was held
                    // back while the server was asked. Only a verdict that
                    // settles the words lets it go.
                    let settled = verdict == Verdict::Keep;
                    match verdict {
                        // Taken without asking. The track was identified by
                        // its sound, or what was written down agrees with what
                        // was heard — either way nothing here is a guess.
                        Verdict::Keep => {
                            if self.take_found_words(id, &found) {
                                changed = true;
                            }
                            // The words are this record's and the times are
                            // another pressing's, so the stem still has
                            // something to say about where they go.
                            to_cue.push(id);
                        }
                        // Plausible and not certain. The one thing not to do
                        // is write it in quietly: wrong words on a cue are
                        // worse than no words, because they look right.
                        Verdict::Ask => self.wondering.push(Wondering { id, found: *found }),
                        Verdict::No => {}
                    }
                    // Nothing settled it, so the slow way round after all —
                    // but only where a recogniser pass was what this stood in
                    // for. Reading the stem while the question is still open is
                    // deliberate: somebody who answers "not this record" wants
                    // the words that were sung, not an empty panel and another
                    // wait.
                    match (settled, then_read) {
                        (false, Some(vocals)) => to_read.push(job::Transcribable { id, vocals }),
                        // Asked for on its own and nothing came of it. Counted
                        // rather than said one at a time, because a batch of
                        // white labels is a batch of misses.
                        (false, None) => unanswered += 1,
                        (true, _) => {}
                    }
                }
                Update::Transcribed { id, lyrics, confidence, language } => {
                    let heard = lyrics.len();
                    if let Some(track) = self.library.get_mut(id) {
                        // What repeats in them, worked out here and kept: it
                        // is shown in two panels that redraw every frame, and
                        // it is the thing a DJ recognises a record by months
                        // later.
                        track.refrains = crate::library::refrains_from(&lyrics);
                        track.lyrics = lyrics;
                        // Only where the recogniser said. A realignment moves
                        // the times and never the words, so it has nothing new
                        // to report about either and must not wipe what is
                        // already known.
                        if confidence.is_some() {
                            track.heard_surely = confidence;
                        }
                        if let Some(language) = language.filter(|l| !l.trim().is_empty()) {
                            track.language = language;
                        }
                        if confidence.is_some() {
                            track.words_from = crate::library::WordsFrom::Recogniser;
                        }
                        // Both paths that produce this update place the words
                        // against the stem first, so arriving here is what
                        // being placed means.
                        track.lyrics_aligned = true;
                    }
                    // Said plainly, because an empty transcript is a real
                    // answer and looks exactly like a failure from outside: a
                    // track that turns out to have no words should say so
                    // rather than leave somebody waiting for cues.
                    match heard {
                        0 => {
                            // Recorded rather than only reported. An empty
                            // transcript off a stem that was listened to is a
                            // real answer — this record has no singing on it —
                            // and without keeping it every press of Words paid
                            // for the same minutes again.
                            if let Some(track) = self.library.get_mut(id) {
                                track.instrumental = true;
                            }
                            self.note("nothing sung was made out", theme::dim());
                        }
                        _ => {
                            let placed = self.auto_cue(id);
                            self.note(
                                format!(
                                    "{} heard, {}",
                                    crate::library::plural(heard, "line"),
                                    crate::library::plural(placed, "cue")
                                ),
                                theme::text(),
                            );
                        }
                    }
                    changed = true;
                }
                Update::Progress { done, total } => {
                    self.progress = Some((done, total));
                    // A new file: whatever the last one had got to is not
                    // this one's position.
                    self.step = None;
                }
                Update::Step { percent } => self.step = Some(percent),
                Update::Line(text) => self.note(text, theme::text()),
                Update::Failed { path, message } => {
                    // Whatever failed, nothing is arriving for the deck now.
                    self.loading = None;
                    crate::error!("{}: {message}", path.display());
                    let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
                    self.note(
                        format!(
                            "{}: {message}",
                            name.unwrap_or_else(|| path.display().to_string())
                        ),
                        theme::alert(),
                    );
                }
                Update::Done(result) => {
                    finished = true;
                    let took = self
                        .started
                        .take()
                        .map(|at| format!(" in {:.1}s", at.elapsed().as_secs_f32()))
                        .unwrap_or_default();
                    let name = self.runner.as_ref().map(|r| r.name).unwrap_or("job");
                    match result {
                        Ok(()) => {
                            crate::info!("finished {name}{took}");
                            self.status = "done".into();
                        }
                        Err(message) => {
                            crate::error!("{name} failed{took}: {message}");
                            self.status = message.clone();
                            self.note(message, theme::alert());
                        }
                    }
                }
            }
        }

        if finished {
            if let Some(runner) = &mut self.runner {
                runner.join();
            }
            self.runner = None;
            self.progress = None;
            self.step = None;
            if let Some(checked) = &mut self.checked {
                checked.running = false;
                // Worst first, and then in the order the collection is in, so
                // that running the same check twice reads the same way — the
                // answers arrive from several threads at once and their order
                // is otherwise whatever the disk felt like.
                checked.troubles.sort_by_key(|report| {
                    (report.troubles.first().map(|t| t.rank()).unwrap_or(u8::MAX), report.id)
                });
            }
            self.save();
        }
        if !imported.is_empty() {
            self.ensure_local(&imported);
            self.check_compatibility(&imported);
            self.check_storage(&imported);
        }
        if !to_read.is_empty() {
            self.read_words(to_read);
        }
        if !to_cue.is_empty() {
            changed |= self.auto_cue_tracks(&to_cue, false);
        }
        if unanswered > 0 {
            self.note(
                format!(
                    "no lyrics server had words for {}",
                    crate::library::plural(unanswered, "track")
                ),
                theme::dim(),
            );
        }
        if paced {
            if let Err(e) = self.config.save(&self.config_path) {
                crate::warn!("could not save what a separation took: {e:#}");
            }
        }
        if changed {
            self.rebuild();
        }
        // Whatever was queued behind this — the copy an import turned up, say —
        // starts now.
        self.flush_retags();
        self.pump();
    }

    fn save(&mut self) {
        match self.library.save(&self.library_path) {
            Ok(()) => crate::debug!(
                "saved {}, {}",
                plural(self.library.tracks.len(), "track"),
                plural(self.library.playlists.len(), "playlist")
            ),
            Err(e) => self.note(format!("could not save the collection: {e:#}"), theme::alert()),
        }
    }

    // -- the sync ----------------------------------------------------------

    /// How many of the drive's tracks have a kit rendered.
    ///
    /// Counted off the collection rather than off the plan, because the plan
    /// counts files and the question being asked on the sheet is about records:
    /// three files from one track and three from three read the same otherwise.
    fn kits_going_on(&self) -> usize {
        sync::wanted(
            &self.library,
            self.library.drives.get(self.drive).unwrap_or(&Drive::default()),
        )
        .iter()
        .filter_map(|id| self.library.get(*id))
        .filter(|track| track.stems.each().iter().any(|(_, part)| part.is_some()))
        .count()
    }

    /// The drive's playlists, as the exporter wants them: paths in play order.
    ///
    /// A stem companion follows its parent rather than going to the end, so
    /// the browse list on the player reads track, vocals, drums, melody and a
    /// companion is a turn of the encoder from the record it came from.
    /// Companions go in only when the drive carries them, and only the parts
    /// that were actually rendered.
    fn drive_playlists(&self, drive: &Drive) -> Vec<booth_cli::cli::PlaylistSpec> {
        drive
            .playlist_names()
            .iter()
            .filter_map(|name| self.library.playlists.iter().find(|p| p.name == *name))
            .map(|playlist| booth_cli::cli::PlaylistSpec {
                name: playlist.name.clone(),
                folder: playlist.folder.clone(),
                tracks: playlist
                    .tracks
                    .iter()
                    .filter_map(|id| self.library.get(*id))
                    .flat_map(|track| {
                        let mut paths = vec![track.path.clone()];
                        if !drive.skip_stems {
                            paths.extend(
                                track.stems.each().into_iter().filter_map(|(_, s)| s.cloned()),
                            );
                        }
                        paths
                    })
                    .collect(),
            })
            .collect()
    }

    /// Notice kits that were rendered but never recorded, for these tracks.
    ///
    /// What goes on a drive is decided from the collection, so a kit the
    /// collection does not know about is a kit that stays on the laptop — and
    /// that is a silence rather than an error. The files are named after their
    /// track, so asking the disk is cheap and the answer is not a guess.
    ///
    /// Returns how many tracks gained one, so the write can say so.
    fn notice_rendered_kits(&mut self, ids: &[u32]) -> usize {
        let stems_in = self.config.stems_location();
        let mut found = 0;
        for id in ids {
            let Some(track) = self.library.get(*id) else { continue };
            if track.stems.is_complete() {
                continue;
            }
            let on_disk = crate::job::find_stems(&stems_in, &track.path);
            if on_disk.is_empty() {
                continue;
            }
            let Some(track) = self.library.get_mut(*id) else { continue };
            if track.stems.fill_from(&on_disk) {
                found += 1;
            }
        }
        found
    }

    fn write_drive(&mut self) {
        let Some(drive) = self.library.drives.get(self.drive).cloned() else { return };
        // What should be on the drive when this is done: the union of its
        // playlists, which is what the plan was worked out against.
        let wanted: Vec<u32> = sync::wanted(&self.library, &drive);

        // Before anything is counted: a kit rendered in an earlier run, or by
        // hand into the stems folder, is one the collection may never have been
        // told about. Everything below decides from the collection, so this is
        // the last moment the drive can gain it.
        if !drive.skip_stems {
            match self.notice_rendered_kits(&wanted) {
                0 => {}
                found => {
                    crate::info!(
                        "found a kit on disk for {} that the collection had not recorded",
                        plural(found, "track")
                    );
                    self.save();
                }
            }
        }

        // Tracks the player has edited since this drive was written, and that
        // the person has said to leave as they are. Taken out of the plan
        // rather than written and then put back: preparing one is what would
        // overwrite it.
        //
        // What the deck did is read back into the collection first, so that
        // keeping the drive's copy means having it rather than only not losing
        // it. See `take_what_the_player_left`.
        //
        // Their row is carried through untouched, so the track stays on the
        // drive and in its playlists with whatever the deck made of it.
        self.take_what_the_player_left();
        self.replan();
        let mut plan = self.plan.clone();
        let kept_theirs: Vec<u32> = self
            .settled
            .iter()
            .filter(|(_, side)| **side == sync::Side::Theirs)
            .map(|(id, _)| *id)
            .collect();
        if !kept_theirs.is_empty() {
            plan.update.retain(|(id, _)| !kept_theirs.contains(id));
            crate::info!(
                "leaving {} as the player left {}",
                plural(kept_theirs.len(), "track"),
                match kept_theirs.len() {
                    1 => "it",
                    _ => "them",
                }
            );
        }

        // What is carried through from the last write and what has to be made
        // again — the rule for both, including how a stem follows its parent,
        // is in `sync::carry`.
        let carry = sync::carry(&self.library, &drive, &plan);

        // Which stem came from which track, so each one takes its parent's
        // grid, cues, key and phrases rather than being listened to alone, and
        // lands in its parent's folder rather than one worked out from its own
        // tags.
        //
        // Drawn from everything the drive is to hold, not from what is being
        // prepared: a stem goes on beside a track that is already there and
        // being carried, and a pairing missing here is a stem given its own
        // grid and its own folder — the two failures this list exists to
        // prevent.
        let companions: Vec<(PathBuf, PathBuf)> = wanted
            .iter()
            .filter_map(|id| self.library.get(*id))
            .flat_map(|track| {
                track
                    .stems
                    .each()
                    .into_iter()
                    .filter_map(|(_, stem): (&str, Option<&PathBuf>)| stem.cloned())
                    .map(|stem| (stem, track.path.clone()))
                    .collect::<Vec<_>>()
            })
            .collect();
        // What is happening about stems, said out loud. A stem that does not go
        // on is otherwise a silence — the drive simply ends up with fewer files
        // than expected, and nothing distinguishes the three reasons: the drive
        // not carrying them, nothing being rendered, or a kit naming files that
        // have since moved. All three look identical on the stick afterwards.
        let rendered = self.kits_going_on();
        if !drive.skip_stems {
            let missing = plan
                .stems
                .iter()
                .filter(|(_, path)| !path.exists())
                .map(|(_, path)| path.display().to_string())
                .collect::<Vec<_>>();
            match plan.stems.len() {
                0 => crate::warn!(
                    "this drive carries stems, but none of the {} has one rendered",
                    plural(wanted.len(), "track")
                ),
                n => crate::info!("carrying {n} stem files from {rendered} tracks"),
            }
            if !missing.is_empty() {
                crate::warn!(
                    "{} named by a kit {} not there, so {} cannot go on: {}",
                    plural(missing.len(), "stem"),
                    match missing.len() {
                        1 => "is",
                        _ => "are",
                    },
                    match missing.len() {
                        1 => "it",
                        _ => "they",
                    },
                    missing.join(", ")
                );
            }
        } else if rendered > 0 {
            crate::info!(
                "{} have stems rendered, and this drive is not carrying them — \
                 Carry stems is on the sync sheet",
                plural(rendered, "track")
            );
        }

        // Nothing to prepare is not nothing to do: a playlist that gained a
        // track already on the drive, or lost one, changes the database and
        // not a single audio file.
        if carry.files.is_empty() && carry.already.is_empty() {
            self.note("nothing to write", theme::dim());
            return;
        }

        let mut args = ExportArgs::defaults();
        if drive.is_image {
            args.image = Some(drive.path.clone());
            args.label = drive.label.clone();
        } else {
            args.drive = Some(drive.path.clone());
        }
        args.player = self.config.oldest_player;
        args.playlists = self.drive_playlists(&drive);
        args.companions = companions;
        args.already = carry.already;
        args.onelibrary_key = self.config.onelibrary_key().map(str::to_string);
        // What the collection knows, so the exporter writes it rather than
        // measuring its own and quietly writing that instead. Everything the
        // drive should hold, not only what is being prepared now: a stem looks
        // its prep up under its parent, and a parent being carried rather than
        // rewritten is still the parent of a stem that is being written.
        args.prepared = wanted
            .iter()
            .filter_map(|id| self.library.get(*id))
            .map(|track| (track.path.clone(), sync::prep(track)))
            .collect();

        // The drive's record is everything that should be on it once this is
        // done, not the part being written now. Recording only the part is how
        // a drive whose second write added one track came to have a record
        // saying one track was all it held.
        //
        // Written before the write rather than after, because the fingerprints
        // being recorded are the ones being written; the rows are filled in
        // when the write reports what the database ended up saying.
        let written: Vec<Written> = wanted
            .iter()
            .filter_map(|id| self.library.get(*id))
            .map(|track| {
                let before = drive.written.iter().find(|w| w.id == track.id);
                Written {
                    id: track.id,
                    // The collection's prep either way, including for a track
                    // whose drive copy is being kept: the two have been
                    // reconciled, by somebody saying which to keep, and asking
                    // again on the next sync would be asking a settled
                    // question for ever.
                    prep: sync::fingerprint(track),
                    row: before.and_then(|w| w.row.clone()),
                    // Carried through for a track being left alone, whose files
                    // are not about to change. For everything else it is filled
                    // in after the write, because writing a track rewrites the
                    // files this describes.
                    theirs: match kept_theirs.contains(&track.id) {
                        true => self.drive_now.get(&track.id).cloned(),
                        false => None,
                    },
                    // Kept only where the parent's row is: a stem being
                    // written again gets its row back from what the write
                    // reports, and one whose kit has been re-rendered names a
                    // file that is no longer part of it.
                    stems: match carry.carried.contains(&track.id) {
                        true => before
                            .map(|w| w.stems.iter().filter(|(p, _)| track.stems.has(p)))
                            .map(|kept| kept.cloned().collect())
                            .unwrap_or_default(),
                        false => Vec::new(),
                    },
                }
            })
            .collect();
        if let Some(drive) = self.library.drives.get_mut(self.drive) {
            drive.written = written;
            drive.last_sync = Some(now());
        }

        self.sheet = false;
        self.start(Job::Sync { args: Box::new(args), files: carry.files });
        self.replan();
    }

    /// Forget what a drive is holding, so the next write puts it all on again.
    ///
    /// The record is what lets a write be a small one: it says which tracks are
    /// already there and carries their rows into the new database rather than
    /// preparing them a second time. Dropping it makes the next write a first
    /// write — every track decoded, every row made afresh — which is what to do
    /// with a drive that something else has been at, or one whose database is
    /// not to be trusted.
    ///
    /// Nothing on the drive is touched here. This forgets, and the write that
    /// follows overwrites; a file on the drive that no longer belongs to any of
    /// its playlists is left where it is either way.
    fn forget_drive_contents(&mut self) {
        let Some(drive) = self.library.drives.get_mut(self.drive) else { return };
        let held = drive.written.len();
        if held == 0 {
            return;
        }
        drive.written.clear();
        let label = drive.label.clone();
        crate::info!("forgot what {label} was holding: {}", plural(held, "track"));
        self.note(
            format!("{label} will be written from scratch — {} to put on", plural(held, "track")),
            theme::amber(),
        );
        self.replan();
        self.save();
    }

    fn add_drive(&mut self, path: PathBuf, is_image: bool) {
        let label = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "DRIVE".to_string());
        // A new drive starts with the first playlist rather than all of them:
        // what goes on a stick is a decision, and guessing "everything" would
        // be a large one made on the user's behalf.
        let playlists =
            self.library.playlists.first().map(|p| p.name.clone()).into_iter().collect();
        self.library.drives.push(Drive {
            label,
            path,
            is_image,
            playlist: String::new(),
            playlists,
            written: Vec::new(),
            skip_stems: false,
            bytes: 0,
            last_sync: None,
        });
        self.drive = self.library.drives.len() - 1;
        self.replan();
        self.save();
    }
}

/// A name being typed in the sidebar, and what it is for.
///
/// One state for four jobs, because they are the same job: collect a name and
/// do one thing with it. Keeping it out of the collection is what makes a
/// half-typed name not a name.
struct Naming {
    what: What,
    /// The playlist or folder being renamed. Empty when making a new one.
    subject: String,
    text: String,
    /// Whether the field has been given the keyboard yet. Asked for once, on
    /// the frame it appears — see [`App::name_field`] for why not every frame.
    focused: bool,
    /// Tracks waiting on the name: "add these to a new playlist" is one act,
    /// and asking for the name should not turn it into two.
    holding: Vec<u32>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum What {
    NewPlaylist,
    NewFolder,
    RenamePlaylist,
    RenameFolder,
}

impl What {
    fn hint(self) -> &'static str {
        match self {
            What::NewPlaylist | What::RenamePlaylist => "playlist name",
            What::NewFolder | What::RenameFolder => "folder name",
        }
    }
}

impl Naming {
    fn new_playlist() -> Self {
        Self {
            what: What::NewPlaylist,
            subject: String::new(),
            text: String::new(),
            focused: false,
            holding: Vec::new(),
        }
    }

    fn new_folder() -> Self {
        Self {
            what: What::NewFolder,
            subject: String::new(),
            text: String::new(),
            focused: false,
            holding: Vec::new(),
        }
    }

    /// Renaming starts from the current name rather than from nothing: most
    /// renames are an edit to what is there.
    fn rename_playlist(name: &str) -> Self {
        Self {
            what: What::RenamePlaylist,
            subject: name.to_string(),
            text: name.to_string(),
            focused: false,
            holding: Vec::new(),
        }
    }

    fn rename_folder(name: &str) -> Self {
        Self {
            what: What::RenameFolder,
            subject: name.to_string(),
            text: name.to_string(),
            focused: false,
            holding: Vec::new(),
        }
    }
}

/// What moving the cursor from one row to another should do to the deck.
#[derive(Copy, Clone, Debug, PartialEq)]
enum Carry {
    /// A different recording. Nothing carries: it starts at its own beginning,
    /// rather than at wherever the last one's playhead happened to be.
    Restart,
    /// Another part of the same recording, with the deck holding neither of
    /// them. The playhead stays where it was, so pressing play picks up there.
    Hold,
    /// Another part of the same recording, and the deck is on one of them at
    /// this moment. `playing` says whether it follows the cursor or waits to
    /// be asked.
    From { secs: f64, playing: bool },
}

/// Where the cursor is going, against where it was and what the deck holds.
///
/// `deck` is which row the player has loaded, where it has got to, and whether
/// it is running.
///
/// A track and its stems are one recording cut three ways, so moving between
/// them is not changing record — it is listening to the same moment a
/// different way. The position carries, and a running deck carries with it. A
/// paused one moves its playhead and stays quiet, which is the same promise
/// without starting a sound nobody asked for.
fn carry(from: Option<u32>, to: u32, deck: Option<(u32, f64, bool)>) -> Carry {
    let together = |other: u32| crate::library::family(other) == crate::library::family(to);
    if !from.is_some_and(together) {
        return Carry::Restart;
    }
    match deck {
        Some((loaded, secs, playing)) if together(loaded) => Carry::From { secs, playing },
        _ => Carry::Hold,
    }
}

/// A part of the window that can be sent to a window of its own.
///
/// The five sections the one window is made of, which is what a second screen
/// is actually for: the waveform big on one monitor and the crate on the other
/// is how this gets used, and there is no arrangement of one window that gives
/// you that.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Pane {
    Collection,
    Browser,
    Prep,
    Inspector,
    Drives,
}

impl Pane {
    /// In the order they sit in the window, left to right and top to bottom,
    /// which is the order the menu lists them in.
    pub const ALL: [Pane; 5] =
        [Pane::Collection, Pane::Browser, Pane::Prep, Pane::Inspector, Pane::Drives];

    /// What it is called, in the menu and in its window's title bar.
    pub fn title(self) -> &'static str {
        match self {
            Pane::Collection => "Collection",
            Pane::Browser => "Browser",
            Pane::Prep => "Prep editor",
            Pane::Inspector => "Inspector",
            Pane::Drives => "Drives",
        }
    }

    fn about(self) -> &'static str {
        match self {
            Pane::Collection => "Views, playlists and saved queries",
            Pane::Browser => "The track list and the batch actions",
            Pane::Prep => "The waveform, the phrase strip and the measurements",
            Pane::Inspector => "Names, cues, stems and tags for the selected track",
            Pane::Drives => "The drive, what would go on it, and the log",
        }
    }

    /// The id its window keeps, so that one popped out, put back and popped out
    /// again is the same window to the operating system rather than a new one
    /// in a new place.
    fn window(self) -> egui::ViewportId {
        egui::ViewportId::from_hash_of(match self {
            Pane::Collection => "booth-pane-collection",
            Pane::Browser => "booth-pane-browser",
            Pane::Prep => "booth-pane-prep",
            Pane::Inspector => "booth-pane-inspector",
            Pane::Drives => "booth-pane-drives",
        })
    }

    /// How big its window opens, shaped like the thing it holds: the side
    /// panels are tall and narrow, the strips are wide and short.
    fn size(self) -> [f32; 2] {
        match self {
            Pane::Collection => [300.0, 720.0],
            Pane::Browser => [1100.0, 700.0],
            Pane::Prep => [1000.0, 340.0],
            Pane::Inspector => [380.0, 760.0],
            Pane::Drives => [900.0, 320.0],
        }
    }
}

/// Draw a panel's contents at exactly the size the panel was given.
///
/// egui stores a panel's size along its resizable axis from the rectangle its
/// *contents* ended up occupying, and reads that back as the size on the next
/// frame. So a panel whose content comes out smaller shrinks to it, and one
/// whose content comes out larger grows — up to the end of its range — and
/// either way the size stops being the one that was dragged to. The inspector
/// did both: empty, it collapsed to its narrowest column; with a long title or
/// path in it, it climbed until it was eating the browser.
///
/// Setting a minimum and a maximum on the contents is not enough to stop the
/// second half of that. A maximum is where egui wraps text and lays widgets
/// out to, not a wall: a row of things that will not fit — a long playlist
/// name beside its count, a button strip — runs past it, and the rectangle
/// the panel is measured by runs past it too. A 200-point panel holding one
/// such row came out 327 points wide, which is how a panel dragged narrow
/// found its own way back to its widest.
///
/// So the contents get a box of exactly the size the panel was handed, they
/// are clipped to it, and the panel takes up that much room whatever happened
/// inside. The stored size is then the panel's own, and only a drag changes
/// it. Contents that can run past the box belong in a `ScrollArea`, which is
/// what turns being clipped into being scrolled to.
fn pinned<R>(ui: &mut Ui, contents: impl FnOnce(&mut Ui) -> R) -> R {
    let rect = ui.available_rect_before_wrap();
    let mut inner = ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(*ui.layout()));
    inner.shrink_clip_rect(rect);
    let out = contents(&mut inner);
    ui.advance_cursor_after_rect(rect);
    out
}

/// What colour a log line is drawn in, by how much it matters.
fn log_color(level: crate::log::Level) -> egui::Color32 {
    match level {
        crate::log::Level::Error => theme::alert(),
        crate::log::Level::Warn => theme::amber(),
        _ => theme::dim(),
    }
}

/// Bits per second, from what is on disk and how long it plays for.
///
/// Every format this reads has a bitrate that can be worked out this way, which
/// is worth more than reading it out of one container's header and leaving the
/// others blank.
fn bitrate(bytes: u64, duration_secs: f64) -> u32 {
    if duration_secs <= 0.0 {
        return 0;
    }
    ((bytes as f64 * 8.0) / duration_secs / 1000.0).round() as u32
}

// -- painting --------------------------------------------------------------

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.wake_from(ctx);
        theme::help_delay(ctx, self.config.help_delay_ms);
        self.collect();
        self.collect_picked();
        if let Some(what) = self.want_pick.take() {
            self.pick(what, ctx);
        }
        self.take_dropped(ctx);
        self.keys(ctx);
        self.follow_playback(ctx);
        self.look_for_drives();

        egui::TopBottomPanel::top("bar").frame(bar_frame()).show(ctx, |ui| self.command_bar(ui));
        // The dock drags up, and the log fills whatever it is given. At its
        // shortest that is the one line that answers "did that work"; pulled
        // up, it is as much of the run as there is room for, without leaving
        // the window for the log's own.
        //
        // The size each panel opens at is the one it was left at. egui keeps
        // its own note of a panel's size for the life of a run, and consults
        // the default only when it has none — which is exactly the first frame
        // after starting up, so handing it the remembered size there is all it
        // takes for a drag to outlive the window.
        let sizes = self.config.panels;
        // A panel only changes size because somebody is dragging its edge, so
        // that is the only time a new size is worth keeping.
        let dragging = ctx.input(|i| i.pointer.any_down());
        if !self.is_out(Pane::Drives) {
            let dock = egui::TopBottomPanel::bottom("dock")
                .frame(bar_frame())
                .resizable(true)
                .default_height(sizes.dock.max(DOCK_HEIGHT))
                .height_range(DOCK_HEIGHT..=460.0)
                .show(ctx, |ui| pinned(ui, |ui| self.dock(ui)));
            self.remember_panel(dragging, |panels| &mut panels.dock, dock.response.rect.height());
        }

        // Both side panels drag. The inspector especially: it carries the
        // cue list, the stem rows and the notes field, and how much room those
        // want is a matter of the track and the person. egui remembers the
        // width against the panel id, so a drag survives a restart.
        //
        // The minimum is what the widest fixed thing in each still fits in,
        // not zero: a panel that can be dragged shut leaves no handle to drag
        // it back with.
        if !self.is_out(Pane::Collection) {
            let collection = egui::SidePanel::left("collection")
                .default_width(sizes.collection)
                .width_range(150.0..=300.0)
                .frame(pane_frame())
                .resizable(true)
                .show(ctx, |ui| pinned(ui, |ui| self.sidebar(ui)));
            self.remember_panel(
                dragging,
                |panels| &mut panels.collection,
                collection.response.rect.width(),
            );
        }

        if !self.is_out(Pane::Inspector) {
            let inspector = egui::SidePanel::right("inspector")
                .default_width(sizes.inspector)
                .width_range(180.0..=420.0)
                .frame(pane_frame())
                .resizable(true)
                .show(ctx, |ui| pinned(ui, |ui| self.inspector(ui)));
            self.remember_panel(
                dragging,
                |panels| &mut panels.inspector,
                inspector.response.rect.width(),
            );
        }

        egui::CentralPanel::default().frame(pane_frame()).show(ctx, |ui| self.centre(ui));

        // After the main window's own panels, so that a section drawn in a
        // window of its own is drawn with the same collection the rest of this
        // frame was drawn from.
        self.popped_panes(ctx);

        if self.sheet {
            self.sync_sheet(ctx);
        }
        // After the sheet, so it sits over it rather than under it.
        if self.details {
            self.details_window(ctx);
        }
        if self.settings {
            self.settings_sheet(ctx);
        }
        if !self.asking.is_empty() {
            self.adopt_sheet(ctx);
        }
        if !self.incompatible.is_empty() {
            self.compatibility_sheet(ctx);
        }
        if self.stems_ahead.is_some() {
            self.stems_sheet(ctx);
        }
        if !self.wondering.is_empty() {
            self.wondering_sheet(ctx);
        }
        // Over the sheet that opens it, so answering one leads into the other.
        if self.editing_words.is_some() {
            self.words_sheet(ctx);
        }
        self.questions_sheet(ctx);
        self.naming_sheet(ctx);
        if self.help {
            self.help_sheet(ctx);
        }
        if self.duplicates.is_some() {
            self.duplicates_sheet(ctx);
        }
        if !self.renames.is_empty() {
            self.renames_sheet(ctx);
        }
        if self.checked.is_some() {
            self.verify_sheet(ctx);
        }
        self.log_window(ctx);

        // Everything the panels asked for happens here, after they have all
        // drawn, so no panel ever reads a collection halfway through a change.
        self.save_layout(ctx);
        self.apply_pending(ctx);
        self.flush_retags();
    }

    fn on_exit(&mut self, _: Option<&eframe::glow::Context>) {
        self.save();
    }
}

fn bar_frame() -> egui::Frame {
    egui::Frame::NONE
        .fill(theme::booth_2())
        .inner_margin(egui::Margin::symmetric(12, 8))
        .stroke(egui::Stroke::new(1.0_f32, theme::rule()))
}

/// The chrome every sheet shares.
/// What the duplicates sheet is holding while it is open.
///
/// Kept across frames rather than worked out afresh each one: the ticks and the
/// answers to disagreements are decisions somebody made, and the groups behind
/// them are rebuilt every frame as tracks are hashed.
#[derive(Default)]
struct Dupes {
    /// The copies to keep. Everything else in a group goes, which is what makes
    /// the opening state — one tick per group — the tidiest one on offer.
    keeping: std::collections::HashSet<u32>,
    /// How a disagreement between one copy and the track being kept was
    /// settled, by copy and by field.
    picked: std::collections::HashMap<(u32, crate::library::Field), crate::library::Side>,
    /// Which tags each kept file should end up with, by that file. Tags are the
    /// one thing the copies hold that is a set rather than an answer, so
    /// instead of asking which copy is right the sheet offers the lot and lets
    /// them be picked over — the only place a merge is not simply additive.
    ///
    /// Per kept file rather than per group, because a group can legitimately
    /// keep two: the same audio on an EP and on a compilation is two records,
    /// and they do not have to be filed the same way. They choose from the same
    /// list, which is every tag anybody in the group wrote.
    tags: std::collections::HashMap<u32, std::collections::HashSet<String>>,
    /// Groups that have already been given their opening tick, so that moving
    /// it is not undone on the next frame — and so that a group that turns up
    /// later, as hashing goes on, still gets one.
    seen: std::collections::HashSet<u32>,
}

/// What a check of the collection turned up.
///
/// Held while the sheet is open rather than stored: it is a reading of the
/// files as they were a moment ago, and keeping it would mean showing somebody
/// yesterday's answer about a folder they have since tidied.
#[derive(Default)]
struct Checked {
    /// One per track that had something to say, worst first.
    troubles: Vec<crate::verify::Report>,
    /// Playable files in the library folder that no track points at.
    orphans: Vec<PathBuf>,
    /// How many were looked at, so a clean answer can say what it covered.
    looked_at: usize,
    /// Whether the reading is still going.
    running: bool,
    /// Whether it read every byte, which is what the wording turns on: a quick
    /// check finding nothing means less than a thorough one finding nothing.
    deep: bool,
    /// Which answer has been picked for each track whose file name disagrees
    /// with its names. Absent means the tags, which is the default: a choice
    /// nobody has made is not a reason to change anything.
    naming: std::collections::HashMap<u32, bool>,
}

/// One group of copies as the sheet is showing it this frame.
///
/// Worked out once per frame rather than per row: the rows read it, the button
/// counts it, and the button's own work uses it, so all three are looking at
/// the same thing.
struct DupeGroup {
    /// The group's stable name, which does not move when the tick does.
    key: u32,
    /// The kept copy the rest would be folded into.
    into: u32,
    /// Every copy: its id, whether it is being kept, whether it is
    /// byte-for-byte the one being kept, and what folding it in would do.
    members: Vec<(u32, bool, bool, crate::library::Merge)>,
    /// Every tag any copy in the group carries, in the order they were met,
    /// starting with the group's own first choice of keeper — so the list does
    /// not reshuffle when the tick moves. Each kept file picks from it.
    tags: Vec<String>,
}

/// Which copies the button would send to the trash, and what each is folded
/// into first.
///
/// Everything in a group that is not being kept — except a copy whose
/// disagreement with the kept one has not been answered. That copy is left
/// where it is: deleting it would settle the question by throwing away one of
/// the two answers, which is the one thing the sheet promises not to do.
///
/// Here rather than inline because the number on the button and the work the
/// button does must be the same answer, and two ways of working it out is how
/// they come to differ.
fn going_to_the_trash(groups: &[DupeGroup], state: &Dupes) -> Vec<(u32, u32)> {
    groups
        .iter()
        .flat_map(|group| {
            group.members.iter().filter_map(move |(id, kept, _, plan)| {
                let answered =
                    plan.conflicts.iter().all(|c| state.picked.contains_key(&(*id, c.field)));
                (!kept && answered).then_some((*id, group.into))
            })
        })
        .collect()
}

/// A path, wrapped rather than run off the edge.
///
/// The sheet is a list of paths and the decision is which of them to delete, so
/// a path that runs past the right-hand edge is the one thing it cannot afford
/// to hide. Wrapped rather than scrolled sideways, and rather than elided in
/// the middle: two copies of a record often differ only deep in the path, which
/// is exactly the part an ellipsis eats.
fn path_label(ui: &mut Ui, path: &std::path::Path, color: egui::Color32) {
    ui.add(
        egui::Label::new(
            RichText::new(path.display().to_string()).font(theme::mono(10.0)).color(color),
        )
        .wrap(),
    );
}

/// The most a sheet may be, so that it always fits on the screen.
///
/// A window taller than the viewport is still centred on it, which puts the
/// title bar — and with it the close button, the only way out — off the top of
/// the screen, and cuts the bottom off too. Capping the height and scrolling
/// the body inside it is what keeps a long sheet closable on a small display.
/// A span of seconds the way somebody waiting for it would say it.
///
/// Rounded on purpose. An estimate read off three timed separations does not
/// know the seconds, and printing them would claim it does.
fn roughly(secs: f64) -> String {
    let minutes = (secs / 60.0).round() as u64;
    match minutes {
        0 => "under a minute".to_string(),
        1 => "about a minute".to_string(),
        m if m < 60 => format!("about {m} minutes"),
        m => {
            let hours = match m / 60 {
                1 => "an hour".to_string(),
                hours => format!("{hours} hours"),
            };
            match m % 60 {
                0 => format!("about {hours}"),
                1 => format!("about {hours} and a minute"),
                left => format!("about {hours} and {left} minutes"),
            }
        }
    }
}

/// Below this, a transcript is worth doubting out loud.
///
/// Measured against whisper.cpp on a spoken line it got wrong: the sentence
/// came back at 0.67 with the invented words carrying 0.34, where the ones it
/// really heard carried over 0.95. Nothing is dropped on it — the separation
/// between a bad transcript and a good one is real but not sharp — so this
/// only decides when to say so.
const POORLY_HEARD: f32 = 0.7;

/// How tall the panels about a track's words are allowed to get in the
/// inspector, in points.
///
/// All three are as long as the record, and a panel that grows with the track
/// is a panel where everything under it moves depending on which row is
/// selected. Scrolled inside a fixed height instead, so the things below them
/// — the tags and the file — stay where somebody left them.
const WORDS_HEIGHT: f32 = 180.0;

fn sheet_height(ctx: &egui::Context) -> f32 {
    (ctx.content_rect().height() - 72.0).max(240.0)
}

fn sheet_frame() -> egui::Frame {
    egui::Frame::NONE
        .fill(theme::booth())
        .stroke(egui::Stroke::new(1.0_f32, theme::rule()))
        .inner_margin(egui::Margin::same(14))
}

fn pane_frame() -> egui::Frame {
    egui::Frame::NONE.fill(theme::booth()).inner_margin(egui::Margin::same(12))
}

impl App {
    /// A repaint callback the jobs can hold on to.
    fn wake_from(&mut self, ctx: &egui::Context) {
        if self.wake_installed {
            return;
        }
        let ctx = ctx.clone();
        self.wake = Arc::new(move || ctx.request_repaint());
        self.wake_installed = true;
    }

    fn take_dropped(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> =
            ctx.input(|i| i.raw.dropped_files.iter().filter_map(|f| f.path.clone()).collect());
        if !dropped.is_empty() {
            self.import(dropped);
        }
    }

    /// Shut the topmost open sheet, and say whether there was one.
    ///
    /// Topmost is last-drawn, which is the order `update` puts them on the
    /// screen in — so the answer matches what is actually in front of the eye
    /// rather than the order the fields happen to be declared in.
    fn close_top_sheet(&mut self) -> bool {
        if self.checked.take().is_some() {
            return true;
        }
        if !self.renames.is_empty() {
            self.renames.clear();
            return true;
        }
        if self.duplicates.take().is_some() {
            return true;
        }
        if std::mem::take(&mut self.help) {
            return true;
        }
        // Not the questions sheet: each of those is an answer the import is
        // waiting on, and dismissing the lot with a keystroke is not one.
        if !self.incompatible.is_empty() {
            self.incompatible.clear();
            return true;
        }
        // Escape is "not now", which is the safe answer: nothing has started.
        if self.editing_words.take().is_some() {
            return true;
        }
        if !self.wondering.is_empty() {
            self.wondering.remove(0);
            return true;
        }
        if self.stems_ahead.take().is_some() {
            return true;
        }
        if !self.asking.is_empty() {
            self.asking.clear();
            return true;
        }
        if std::mem::take(&mut self.settings) {
            return true;
        }
        // Before the sheet it was opened from, or one press would shut both
        // and the way back to the summary would be to open the sheet again.
        if std::mem::take(&mut self.details) {
            return true;
        }
        std::mem::take(&mut self.sheet)
    }

    /// The keys that make the browser a browser: arrows to dig, and one
    /// shortcut to the bar.
    fn keys(&mut self, ctx: &egui::Context) {
        let typing = ctx.memory(|m| m.focused().is_some());
        ctx.input_mut(|i| {
            if i.consume_key(egui::Modifiers::COMMAND, egui::Key::K) {
                self.focus_bar = true;
            }
            // Where every other program on the machine keeps its settings. On
            // a Mac that is cmd-comma; COMMAND is control everywhere else,
            // which is where the same habit lives there.
            if i.consume_key(egui::Modifiers::COMMAND, egui::Key::Comma) {
                self.settings = true;
            }
            if typing {
                return;
            }
            // Everything showing. Consumed rather than merely read, so that
            // nothing downstream reads the same press as something else.
            if i.consume_key(egui::Modifiers::COMMAND, egui::Key::A) {
                self.mark_showing();
            }
            let extend = i.modifiers.shift;
            if i.key_pressed(egui::Key::ArrowDown) {
                self.step(1, extend);
            }
            if i.key_pressed(egui::Key::ArrowUp) {
                self.step(-1, extend);
            }
            if i.key_pressed(egui::Key::Space) {
                if let Some(id) = self.selected {
                    self.pending.push(Pending::TogglePlayback(id));
                }
            }
            // Escape is the way back out of anything. A sheet is the
            // outermost thing to be inside, so it goes first, one press per
            // sheet; only once they are all shut does the key mean the
            // waveform, and there it does nothing when the whole track is
            // already showing rather than being a key that sometimes means
            // something else.
            if i.key_pressed(egui::Key::Escape) && !self.close_top_sheet() && !self.zoom.is_fit() {
                self.pending.push(Pending::FitWave);
            }
        });
    }

    /// Keep the playhead on the deck's position while it plays, and keep the
    /// window repainting so it moves.
    ///
    /// The playhead is the deck's when the deck is running and the window's
    /// Look through the mounted volumes for a drive worth keeping a copy of.
    ///
    /// Every few seconds rather than every frame: it is a `read_dir` of the
    /// places volumes mount plus a stat of two folders on each, which is
    /// nothing next to a repaint, but it is also not worth doing sixty times a
    /// second to notice something a person did with their hands.
    ///
    /// A drive is copied once per state. Plugging the same unchanged stick in
    /// again is not an event; writing to it and plugging it in again is.
    fn look_for_drives(&mut self) {
        const EVERY: std::time::Duration = std::time::Duration::from_secs(4);

        if !self.config.keep_drives || self.running() {
            return;
        }
        if self.looked_for_drives.is_some_and(|at| at.elapsed() < EVERY) {
            return;
        }
        self.looked_for_drives = Some(std::time::Instant::now());

        for root in crate::backup::volumes() {
            if !crate::backup::is_a_player_drive(&root) {
                continue;
            }
            let name = drive_name(&root, "");
            let listing = crate::backup::listing(&root);
            if listing.is_empty() {
                continue;
            }
            let state = crate::backup::digest(&listing);

            match worth_keeping(self.kept_drives.get(&name), &state) {
                Worth::No => continue,
                Worth::NotYet => {
                    // Something is changing this drive on its own. Say which
                    // file, once, and leave it alone until it settles: copying
                    // a drive every time an operating system touches it is how
                    // a disk fills up overnight.
                    if let Some(seen) = self.kept_drives.get_mut(&name) {
                        if !seen.complained {
                            seen.complained = true;
                            let changed = crate::backup::differences(&seen.listing, &listing);
                            crate::warn!(
                                "{name} changed again right after it was copied ({}); \
                                 leaving it be until it settles",
                                changed.join(", ")
                            );
                        }
                    }
                    continue;
                }
                Worth::Yes => {}
            }

            if crate::backup::already_kept(&self.config.backups_path, &name, &state) {
                // Stored on a previous run. Remember it, so the directory of
                // backups is not read again every few seconds for this answer.
                self.remember_drive(&name, &state, listing);
                continue;
            }
            self.keep_drive(&root, &name, &state, listing);
            // One at a time. The next one will be found on the next look, and a
            // queue of copies started at once would fight over the same disk.
            return;
        }
    }

    /// Note what a drive was found holding, without copying it.
    fn remember_drive(&mut self, name: &str, state: &str, listing: Vec<String>) {
        self.kept_drives.insert(
            name.to_string(),
            Seen {
                fingerprint: state.to_string(),
                listing,
                at: std::time::Instant::now(),
                complained: false,
            },
        );
    }

    /// Turn what a player recorded having played into playlists.
    ///
    /// One playlist per session, named the way the player named it, in a folder
    /// of the drive's own. Re-reading a drive replaces those playlists rather
    /// than making a second set: a night that has been read once and is read
    /// again is the same night.
    ///
    /// A track the library does not have is left out and counted. It cannot be
    /// a row in a playlist without being a row in the collection, and adding
    /// somebody else's music because it appeared in a history is a decision
    /// [`crate::config::OnForeign`] already asks about in the one place it
    /// belongs.
    fn take_history(
        &mut self,
        drive: &str,
        root: &std::path::Path,
        sessions: Vec<crate::history::Session>,
    ) {
        let known = crate::backup::what_the_library_has(&self.library);
        let (by_name, by_sound) = crate::backup::index(&known);
        let ids = crate::backup::whose(&self.library);

        let folder = crate::history::folder(drive);
        let (mut made, mut played, mut strangers) = (0usize, 0usize, 0usize);
        for session in sessions {
            let mut tracks = Vec::new();
            for track in &session.played {
                let file = track.file(root);
                let owner =
                    crate::backup::owner(&by_name, &by_sound, &track.file_name, track.bytes, &file);
                match owner.and_then(|known| ids.get(&known.path)) {
                    Some(id) => tracks.push(*id),
                    None => strangers += 1,
                }
            }
            if tracks.is_empty() {
                continue;
            }
            played += tracks.len();
            made += 1;
            let playlist = crate::library::Playlist {
                name: session.name.clone(),
                folder: folder.clone(),
                tracks,
            };
            match self
                .library
                .playlists
                .iter_mut()
                .find(|p| p.name == playlist.name && p.folder == folder)
            {
                Some(existing) => *existing = playlist,
                None => self.library.playlists.push(playlist),
            }
        }

        if made == 0 {
            return;
        }
        if !self.library.folders.contains(&folder) {
            self.library.folders.push(folder.clone());
        }
        self.note(
            match strangers {
                0 => {
                    format!("{} from {drive} ({})", plural(made, "night"), plural(played, "track"))
                }
                n => format!(
                    "{} from {drive} ({}, {} not in the library)",
                    plural(made, "night"),
                    plural(played, "track"),
                    n
                ),
            },
            theme::text(),
        );
        self.rebuild();
        self.save();
    }

    /// Start copying one drive.
    fn keep_drive(
        &mut self,
        root: &std::path::Path,
        name: &str,
        state: &str,
        listing: Vec<String>,
    ) {
        let known = crate::backup::what_the_library_has(&self.library);
        crate::info!("keeping a copy of {name} from {}", root.display());
        crate::debug!(
            "in state {state}, {} files on it, into {}",
            listing.len(),
            self.config.backups_path.display()
        );
        // Noted before the job rather than after it, so a copy that fails or is
        // stopped still counts as this drive having been looked at: the state
        // is what was decided on, and deciding it again next tick would start
        // the same copy over.
        self.remember_drive(name, state, listing);
        self.start(Job::Keep {
            root: root.to_path_buf(),
            drive: name.to_string(),
            state: state.to_string(),
            into: self.config.backups_path.clone(),
            known,
            foreign: self.config.from_other_drives,
            library: self.config.library_path.clone(),
            key: booth_cli::rekordbox::onelibrary_key(self.config.onelibrary_key()),
        });
    }

    /// when it is not — which is what lets a cue be placed by clicking while
    /// something is paused, without playback dragging the marker away.
    fn follow_playback(&mut self, ctx: &egui::Context) {
        let Some(player) = &self.player else { return };
        let Some(id) = player.loaded() else { return };
        if !player.is_playing() || self.selected != Some(id) {
            return;
        }
        self.playhead_ms = Some((player.position_secs() * 1000.0) as u32);
        // Sixty times a second while playing, and not at all otherwise: a
        // library browser has no business spinning a core to redraw a static
        // window.
        ctx.request_repaint_after(std::time::Duration::from_millis(16));
    }

    fn command_bar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            let field = egui::TextEdit::singleline(&mut self.text)
                .font(theme::mono(12.0))
                .frame(false)
                .desired_width(ui.available_width() - 290.0)
                .hint_text(
                    RichText::new("bpm:124-128 key:~8A -played:30d tag:peak")
                        .monospace()
                        .color(theme::dim()),
                );
            let response = ui.add(field).on_hover_text(
                "Narrow the browser to what matches. Words match the artist, title and album; \
                 bpm:, key:, tag:, played:, added: and missing: ask about one thing each. \
                 \u{2318}K from anywhere puts the cursor here.",
            );
            if self.focus_bar {
                response.request_focus();
                self.focus_bar = false;
            }
            if response.changed() {
                self.rebuild();
            }

            ui.label(RichText::new("⌘K").font(theme::mono(10.5)).color(theme::dim()));
            self.panes_menu(ui);
            if ui
                .add(
                    egui::Button::new(
                        RichText::new("?").font(theme::mono(10.5)).color(theme::dim()),
                    )
                    .fill(theme::booth()),
                )
                .on_hover_text("What can be typed here")
                .clicked()
            {
                self.help = !self.help;
            }

            // The queue indicator, right-aligned, which is the only place a
            // running job is reported. A modal progress dialog over a library
            // is a library you cannot use while it works.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                match &self.runner {
                    Some(runner) => {
                        let (done, total) = self.progress.unwrap_or((0, 0));
                        let mut text = match total > 0 {
                            true => format!("{} {} {done}/{total}", theme::SPINNER, runner.name),
                            false => format!("{} {}", theme::SPINNER, runner.name),
                        };
                        // Minutes a track means the count alone sits still long
                        // enough to look stuck, so how far into the one in hand
                        // goes beside it.
                        if let Some(percent) = self.step {
                            text.push_str(&format!(" · {percent}%"));
                        }
                        if ui
                            .add(egui::Button::new(
                                RichText::new("stop").font(theme::mono(10.5)).color(theme::dim()),
                            ))
                            .clicked()
                        {
                            runner.cancel();
                        }
                        ui.label(RichText::new(text).font(theme::mono(11.5)).color(theme::amber()));
                    }
                    None => {
                        let color =
                            if self.query.has_errors() { theme::alert() } else { theme::dim() };
                        let shown = if self.query.has_errors() {
                            "unknown term".to_string()
                        } else {
                            format!("{} showing", self.rows.iter().filter(|r| !r.indented).count())
                        };
                        ui.label(RichText::new(shown).font(theme::mono(11.5)).color(color));
                    }
                }
            });
        });

        // The parsed query, painted underneath what was typed: fields amber,
        // exclusions red, anything unrecognised struck through in red. It reads
        // from the same parse that filters, so it cannot flatter a term the
        // browser is actually ignoring.
        if !self.text.trim().is_empty() {
            ui.add_space(2.0);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 5.0;
                for token in &self.query.tokens {
                    let Some(text) = self.text.get(token.at.clone()) else { continue };
                    let color = match token.role {
                        Paint::Field => theme::amber(),
                        Paint::Value => theme::text(),
                        Paint::Negated => theme::alert(),
                        Paint::Text => theme::dim(),
                        Paint::Bad => theme::alert(),
                    };
                    let mut label = RichText::new(text).font(theme::mono(10.0)).color(color);
                    if token.role == Paint::Bad {
                        label = label.strikethrough();
                    }
                    ui.label(label);
                }
            });
        }
    }

    /// The collection panel, scrolled.
    ///
    /// A DJ with forty playlists has more of them than any panel is tall. They
    /// used to run off the bottom of the screen; once [`pinned`] started
    /// clipping a panel to its own rectangle — which is what stopped the panels
    /// growing themselves back to their widest — they stopped being drawn at
    /// all. Either way there was no way to reach them, which is what a scroll
    /// area is for.
    ///
    /// `auto_shrink` is off in both directions so the panel is the size it was
    /// dragged to and not the size of what happens to be in it. That is the
    /// same argument `pinned` makes, and the two work together: `pinned` hands
    /// this a box of exactly the panel's size, and this fills it and scrolls
    /// what will not fit. Without a box around it — in a test harness, say —
    /// it fills whatever it is given instead, which is worth knowing before
    /// wondering where the rest of the window went.
    fn sidebar(&mut self, ui: &mut Ui) {
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| self.collection(ui));
    }

    /// Everything in that panel: the views, the playlist tree, the saved
    /// queries.
    fn collection(&mut self, ui: &mut Ui) {
        pane_label(ui, "Collection");
        let all = self.library.tracks.len();
        let unprepared = self.library.unprepared_count();
        let attention = self.library.attention_count();

        self.view_row(ui, View::All, "All tracks", all, theme::dim());
        self.view_row(ui, View::Unprepared, "Unprepared", unprepared, theme::dim());
        self.view_row(ui, View::Attention, "Needs attention", attention, theme::alert());

        // Offered when there is either something to show or something to look
        // through. A row reading zero when everything has been checked is a
        // thing to check rather than a thing to know, so it goes; but a row
        // that never appears because nothing has been hashed yet is a feature
        // with no way in, which is worse — that is the case the question mark
        // is for.
        let copies: usize = self
            .library
            .duplicate_groups(&self.config.library_path)
            .iter()
            .map(|group| group.rest.len())
            .sum();
        let unchecked = self.library.unhashed().len();
        if copies > 0 || unchecked > 0 {
            ui.horizontal(|ui| {
                if ui
                    .add(
                        egui::Label::new(RichText::new("In here twice").color(theme::amber()))
                            .sense(egui::Sense::click()),
                    )
                    .on_hover_text(match unchecked {
                        0 => "The same recording in more than one file".to_string(),
                        n => format!(
                            "The same recording in more than one file — {} not looked at yet",
                            plural(n, "track")
                        ),
                    })
                    .clicked()
                {
                    self.duplicates = Some(Dupes::default());
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // A question mark rather than a count while there is
                    // anything left to look at: the number is only the copies
                    // among the files already hashed, and printing it as if it
                    // were the answer would be a lie in the direction that
                    // stops somebody looking.
                    let (text, colour) = match (copies, unchecked) {
                        (n, 0) => (n.to_string(), theme::dim()),
                        (0, _) => ("?".to_string(), theme::amber()),
                        (n, _) => (format!("{n}?"), theme::amber()),
                    };
                    ui.label(RichText::new(text).font(theme::mono(theme::SMALL)).color(colour));
                });
            });
        }

        ui.add_space(16.0);
        ui.horizontal(|ui| {
            pane_label(ui, "Playlists");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("+▾").on_hover_text("New folder").clicked() {
                    self.naming = Some(Naming::new_folder());
                }
                if ui.small_button("+").on_hover_text("New playlist").clicked() {
                    self.naming = Some(Naming::new_playlist());
                }
            });
        });
        self.playlist_tree(ui);

        ui.add_space(16.0);
        pane_label(ui, "Saved queries");
        let saved: Vec<SavedQuery> = self.library.saved.clone();
        for query in &saved {
            let on = self.text == query.text;
            let color = if on { theme::amber() } else { theme::text() };
            if ui
                .add(
                    egui::Label::new(RichText::new(&query.name).color(color))
                        .sense(egui::Sense::click()),
                )
                .on_hover_text(RichText::new(&query.text).monospace())
                .clicked()
            {
                self.text = query.text.clone();
                self.view = View::All;
                self.rebuild();
            }
        }
        if !self.text.trim().is_empty()
            && !saved.iter().any(|q| q.text == self.text)
            && ui
                .button(RichText::new("save this one").size(theme::SMALL))
                .on_hover_text("Keep this query in the sidebar, so the same question is one click from now on.")
                .clicked()
        {
            let name = self.text.clone();
            self.library.saved.push(SavedQuery { name, text: self.text.clone() });
            self.save();
        }
    }

    /// The playlist tree, and everything that can be done to it from here.
    ///
    /// Renaming happens in place rather than in a dialog: a name is one field,
    /// and a sheet over the window to collect one field is a sheet in the way.
    fn playlist_tree(&mut self, ui: &mut Ui) {
        let tree: Vec<(String, Vec<(String, usize)>)> = self
            .library
            .playlist_tree()
            .into_iter()
            .map(|(folder, lists)| {
                (folder, lists.into_iter().map(|p| (p.name.clone(), p.tracks.len())).collect())
            })
            .collect();

        // The field for a new name, wherever it is being typed. Drawn before
        // the tree when it is a new top-level thing, and in place of a row when
        // it is a rename.
        if matches!(self.naming, Some(Naming { what: What::NewPlaylist | What::NewFolder, .. })) {
            self.name_field(ui);
        }

        if tree.is_empty() && self.naming.is_none() {
            ui.label(RichText::new("none yet").color(theme::dim()).size(theme::SMALL));
        }

        let folders: Vec<String> =
            tree.iter().map(|(f, _)| f.clone()).filter(|f| !f.is_empty()).collect();

        for (folder, lists) in tree {
            if !folder.is_empty() {
                if self.renaming_folder(&folder) {
                    self.name_field(ui);
                } else {
                    let response = ui.add(
                        egui::Label::new(
                            RichText::new(format!("\u{25be} {folder}")).color(theme::text()),
                        )
                        .sense(egui::Sense::click()),
                    );
                    response.context_menu(|ui| {
                        if ui
                            .button("Rename\u{2026}")
                            .on_hover_text("Give this folder another name. The playlists in it stay where they are.")
                            .clicked()
                        {
                            self.naming = Some(Naming::rename_folder(&folder));
                            ui.close();
                        }
                        // The playlists come back to the top level rather than
                        // going with it, so this loses the filing and not the
                        // work — which is why it needs no confirmation.
                        if ui
                            .button("Delete folder")
                            .on_hover_text("Lose the filing, not the work: the playlists inside come back to the top level.")
                            .clicked()
                        {
                            self.library.remove_folder(&folder);
                            self.pending_save = true;
                            ui.close();
                        }
                    });
                }
            }
            for (name, count) in lists {
                if self.renaming_playlist(&name) {
                    self.name_field(ui);
                    continue;
                }
                let on = self.view == View::Playlist && self.playlist == name;
                let color = if on { theme::amber() } else { theme::dim() };
                let indent = if folder.is_empty() { 0.0 } else { 12.0 };
                ui.horizontal(|ui| {
                    ui.add_space(indent);
                    let response = ui.add(
                        egui::Label::new(RichText::new(&name).color(color))
                            .sense(egui::Sense::click()),
                    );

                    // A row dragged from the browser. Lit while it is over the
                    // name, because a drop target that looks the same as
                    // everything else is one you have to guess at.
                    if response.dnd_hover_payload::<rows::Dragged>().is_some() {
                        ui.painter().rect_filled(
                            response.rect.expand2(egui::vec2(4.0, 2.0)),
                            2.0,
                            theme::amber().gamma_multiply(0.22),
                        );
                    }
                    if let Some(dragged) = response.dnd_release_payload::<rows::Dragged>() {
                        self.pending.push(Pending::AddToPlaylist(dragged.0.clone(), name.clone()));
                    }

                    let response = response.on_hover_text(format!(
                        "Show {name} \u{2014} {}. Rows can be dragged onto it; right-click to \
                         rename, move or delete it.",
                        plural(count, "track")
                    ));
                    if response.clicked() {
                        self.view = View::Playlist;
                        self.playlist = name.clone();
                        self.rebuild();
                    }
                    response.context_menu(|ui| {
                        ui.label(RichText::new(&name).color(theme::dim()).size(theme::SMALL));
                        ui.separator();
                        if ui
                            .button("Rename\u{2026}")
                            .on_hover_text("Give this playlist another name. A drive carrying it follows the new one.")
                            .clicked()
                        {
                            self.naming = Some(Naming::rename_playlist(&name));
                            ui.close();
                        }
                        ui.menu_button("Move to", |ui| {
                            if !folder.is_empty()
                                && ui
                                    .button("Top level")
                                    .on_hover_text("Take it out of its folder.")
                                    .clicked()
                            {
                                self.move_playlist(&name, "");
                                ui.close();
                            }
                            for other in folders.iter().filter(|f| **f != folder) {
                                if ui
                                    .button(other)
                                    .on_hover_text(format!("File it under {other}."))
                                    .clicked()
                                {
                                    self.move_playlist(&name, other);
                                    ui.close();
                                }
                            }
                        })
.response
.on_hover_text("Put this playlist in one of the sidebar's folders.");
                        if ui
                            .button("Delete playlist")
                            .on_hover_text("The tracks stay in the collection")
                            .clicked()
                        {
                            self.library.remove_playlist(&name);
                            if self.playlist == name {
                                self.view = View::All;
                                self.playlist.clear();
                                self.pending_rebuild = true;
                            }
                            self.pending_save = true;
                            ui.close();
                        }
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            RichText::new(count.to_string())
                                .font(theme::mono(theme::SMALL))
                                .color(theme::dim()),
                        );
                    });
                });
            }
        }
    }

    fn renaming_playlist(&self, name: &str) -> bool {
        matches!(&self.naming, Some(n) if n.what == What::RenamePlaylist && n.subject == name)
    }

    fn renaming_folder(&self, name: &str) -> bool {
        matches!(&self.naming, Some(n) if n.what == What::RenameFolder && n.subject == name)
    }

    fn move_playlist(&mut self, name: &str, folder: &str) {
        if let Some(playlist) = self.library.playlists.iter_mut().find(|p| p.name == name) {
            playlist.folder = folder.to_string();
            self.pending_save = true;
        }
    }

    /// The one field a name is typed into, wherever it has been opened.
    ///
    /// Enter commits, Escape abandons, and losing focus abandons too: a
    /// half-typed name left behind by a click elsewhere is not an instruction.
    fn name_field(&mut self, ui: &mut Ui) {
        let Some(naming) = &mut self.naming else { return };
        let response = ui
            .add(
                egui::TextEdit::singleline(&mut naming.text)
                    .desired_width(f32::INFINITY)
                    .hint_text(RichText::new(naming.what.hint()).color(theme::dim())),
            )
            .on_hover_text(
                "Enter keeps the name, Escape abandons it. Clicking away abandons it too.",
            );

        // Asked for once, on the frame the field appears — never again.
        //
        // egui reports a committed edit as the field *losing* focus, and
        // `lost_focus` means "had it last frame and does not have it now". So
        // asking for focus every frame, as this did, took it straight back on
        // the same frame the field gave it up: `lost_focus` was false forever,
        // Enter did nothing, Escape did nothing, and a typed name could not be
        // turned into a playlist at all.
        let first_frame = !naming.focused;
        naming.focused = true;
        if first_frame {
            response.request_focus();
        }

        if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            let naming = self.naming.take().expect("just checked");
            let outcome = match naming.what {
                What::NewPlaylist => self.library.add_playlist(&naming.text, ""),
                What::NewFolder => self.library.add_folder(&naming.text),
                What::RenamePlaylist => self.library.rename_playlist(&naming.subject, &naming.text),
                What::RenameFolder => self.library.rename_folder(&naming.subject, &naming.text),
            };
            match outcome {
                Ok(()) => {
                    // Follow a rename, so the view does not silently empty out
                    // when the list it was showing changes its name.
                    if naming.what == What::RenamePlaylist && self.playlist == naming.subject {
                        self.playlist = naming.text.trim().to_string();
                    }
                    // Whatever the name was asked for on behalf of.
                    if !naming.holding.is_empty() {
                        let name = naming.text.trim().to_string();
                        self.pending.push(Pending::AddToPlaylist(naming.holding, name));
                    }
                    self.pending_save = true;
                    self.pending_rebuild = true;
                }
                Err(message) => self.note(message, theme::alert()),
            }
        } else if response.lost_focus() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.naming = None;
        }
    }

    fn view_row(
        &mut self,
        ui: &mut Ui,
        view: View,
        name: &str,
        count: usize,
        count_color: Color32,
    ) {
        let on = self.view == view;
        ui.horizontal(|ui| {
            let color = if on { theme::amber() } else { theme::text() };
            if ui
                .add(egui::Label::new(RichText::new(name).color(color)).sense(egui::Sense::click()))
                .clicked()
            {
                self.view = view;
                self.rebuild();
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let color = if count > 0 { count_color } else { theme::dim() };
                ui.label(
                    RichText::new(count.to_string()).font(theme::mono(theme::SMALL)).color(color),
                );
            });
        });
    }

    /// The list, and the prep editor underneath it.
    fn track_list(&mut self, ui: &mut Ui, sharing: bool) {
        // The column widths are worked out once, from the settings, and handed
        // to both the header and the rows, so the two cannot drift apart — and
        // the header is drawn outside the scroll area, so that scrolling a long
        // list never takes away the names of the columns, the way to sort by
        // one, or the menu that says which columns there are.
        //
        // The width they are measured against is what is left after the scroll
        // bar, because that is what the rows will get. When there is no bar a
        // sliver of space goes unused at the right, which is a great deal less
        // trouble than a header a scroll bar's width out of step with the list
        // under it.
        let widths = self.header(ui, ui.available_width() - ui.spacing().scroll.allocated_width());

        // Room kept for the prep editor only when it is underneath: popped out
        // into its own window, the space it was holding belongs to the list.
        let reserve = if sharing { self.prep_height } else { 0.0 };
        let list_height = (ui.available_height() - reserve).max(120.0);
        egui::ScrollArea::vertical()
            .max_height(list_height)
            .auto_shrink([false, false])
            .show(ui, |ui| self.rows_table(ui, &widths));

        ui.add_space(6.0);
        self.actions(ui);
        ui.add_space(4.0);
    }

    /// The list with the prep editor under it, which is what the middle of the
    /// window is when neither has been sent anywhere.
    fn browser(&mut self, ui: &mut Ui) {
        self.track_list(ui, true);
        ui.separator();
        self.prep(ui);
    }

    /// The prep actions, on the strip between the list and the waveform.
    ///
    /// Each one acts on everything currently showing, because that is what the
    /// query bar is for: narrow the list to the work, then do the work. There is
    /// no separate selection to keep in step with the filter.
    fn actions(&mut self, ui: &mut Ui) {
        let idle = !self.running();
        // Counted the same way the buttons act, so the number on a button is
        // what pressing it will do. Two ways of working that out is how they
        // come to disagree.
        let chosen = self.marked.len() > 1;
        let showing = self.acting_on(|_| true).len();
        let unanalysed = self.acting_on(|track| !track.analyzed).len();
        let unstemmed = self.acting_on(|track| track.stems.is_empty()).len();

        ui.horizontal(|ui| {
            if ui.add_enabled(idle, egui::Button::new("Add music…")).clicked() {
                self.want_pick = Some(Picking::Music);
            }
            // Shift turns every verb on this strip round, so that work already
            // done can be asked for again: one modifier beats five more
            // buttons. The labels follow it rather than only the hover text,
            // because each pair acts on a different number of tracks and a
            // button has to say what pressing it will do.
            //
            // Which is the whole of what a selection is for here. Pick the
            // tracks, hold shift, and the buttons offer them whether or not
            // they have been through this before — without it, a track that
            // has been analysed, stemmed or read is simply not counted, and
            // there was no way to ask for any of it a second time.
            let again = ui.input(|i| i.modifiers.shift);
            let (verb, count) =
                if again { ("Re-analyse", showing) } else { ("Analyse", unanalysed) };
            if ui
                .add_enabled(idle && count > 0, egui::Button::new(format!("{verb} {count}")))
                .on_hover_text(match (again, chosen) {
                    (true, true) => {
                        "Grid, key, phrases and cues for the selected tracks again, \
                         replacing what they have"
                    }
                    (true, false) => {
                        "Grid, key, phrases and cues for everything showing again, \
                         replacing what they have"
                    }
                    (false, true) => {
                        "Grid, key, phrases and cues for the selected tracks that have none \
                         \u{2014} hold shift to do them all again"
                    }
                    (false, false) => {
                        "Grid, key, phrases and cues for everything showing that has none \
                         \u{2014} hold shift to do it all again"
                    }
                })
                // The case this most needs saying in: a collection where
                // everything has been analysed reads "Analyse 0" and is greyed
                // out, so the way to ask for the work again is exactly where
                // there is no enabled button to hover over.
                .on_disabled_hover_text(match again {
                    true => "Nothing is showing to analyse",
                    false => "Everything here has been analysed \u{2014} \
                              hold shift to do it all again",
                })
                .clicked()
            {
                match again {
                    true => self.analyze_showing(),
                    false => self.analyze_unprepared(),
                }
            }
            let unnamed = self.acting_on(|track| !track.identified).len();
            let (verb, count) =
                if again { ("Re-identify", showing) } else { ("Identify", unnamed) };
            if ui
                .add_enabled(idle && count > 0, egui::Button::new(format!("{verb} {count}")))
                .on_hover_text(match again {
                    true => {
                        "Fingerprint and look up these again, whatever they are already called"
                    }
                    false => {
                        "Fingerprint and look up what these are, filling in the names they lack \
                         \u{2014} hold shift to ask again about ones already looked up"
                    }
                })
                .on_disabled_hover_text(match again {
                    true => "Nothing is showing to identify",
                    false => "Everything here has been looked up \u{2014} \
                              hold shift to ask again",
                })
                .clicked()
            {
                self.identify_showing(again);
            }
            let (verb, count) = if again { ("Re-stem", showing) } else { ("Stems", unstemmed) };
            if ui
                .add_enabled(idle && count > 0, egui::Button::new(format!("{verb} {count}")))
                .on_hover_text(match again {
                    true => {
                        "Render a vocals/melody/drums kit again, replacing the one each track \
                         has \u{2014} which is what a change of model or quality calls for"
                    }
                    false => {
                        "Render a vocals/melody/drums kit with demucs \u{2014} hold shift to \
                         render over the kits already there"
                    }
                })
                .on_disabled_hover_text(match again {
                    true => "Nothing is showing to separate",
                    false => "Everything here has a kit \u{2014} hold shift to render it again",
                })
                .clicked()
            {
                self.render_stems(again);
            }
            let unread =
                self.acting_on(|track| track.lyrics.is_empty() && !track.instrumental).len();
            let (verb, count) = if again { ("Re-read", showing) } else { ("Words", unread) };
            if ui
                .add_enabled(idle && count > 0, egui::Button::new(format!("{verb} {count}")))
                .on_hover_text(match again {
                    true => {
                        "Read the vocal stem again and re-cue from what it says, replacing the \
                         words each track is carrying"
                    }
                    false => {
                        "Read the vocal stem and cue the hook, the drops and the phrases \
                         \u{2014} rendering the stems first where there are none, or hold \
                         shift to read ones already read again"
                    }
                })
                .on_disabled_hover_text(match again {
                    true => "Nothing is showing to read",
                    false => "The words have been read for everything showing \u{2014} \
                              hold shift to read them again",
                })
                .clicked()
            {
                self.auto_cue_showing(again);
            }
            // The cheap half of the button before it, over a whole crate. A
            // lookup is seconds and one request where reading the words is a
            // separation and a recogniser pass each, so a box that has never
            // been prepared is worth asking about before any of those minutes
            // are spent — and what it finds is better than what a
            // recogniser would have guessed at the same vocal.
            //
            // No "again": a lookup is cheap enough that asking twice costs a
            // request, and there is nothing it would skip.
            let named = |track: &Track| {
                !track.artist.trim().is_empty() && !track.title.trim().is_empty()
            };
            let askable = self.acting_on(named).len();
            if ui
                .add_enabled(idle && askable > 0, egui::Button::new(format!("Lyrics {askable}")))
                .on_hover_text(
                    "Ask a lyrics server for the words of these, under their artist and \
                     title. No stems and no recogniser — what comes back is taken where \
                     it is certain and put to you where it is not.",
                )
                .on_disabled_hover_text(
                    "A lyrics server is asked by artist and title, and nothing showing has \
                     both",
                )
                .clicked()
            {
                let wanted = self.acting_on(named);
                self.look_up_words(&wanted);
            }
            // Reads rather than changes anything, so it sits at the end of the
            // strip after the four that do. Shift means the thorough version
            // here rather than "again", because a check is a read: it is
            // always done over, and there is nothing it could skip.
            let thorough = again;
            if ui
                .add_enabled(idle && showing > 0, egui::Button::new(format!("Check {showing}")))
                .on_hover_text(match thorough {
                    false => "Are the files still there, still that size, still tagged that                               way — hold shift to read every byte instead",
                    true => "Reads every byte of every file, which also catches one edited in                              place without changing length",
                })
                .clicked()
            {
                self.verify_showing(thorough);
            }

            ui.separator();

            // Adding to a playlist is what turns a query into a set that can go
            // on a drive, so it sits with the prep actions rather than in a menu.
            ui.label(
                RichText::new(match chosen {
                    true => "selected to playlist",
                    false => "to playlist",
                })
                .color(if chosen { theme::amber() } else { theme::dim() })
                .size(theme::SMALL),
            );
            let width = 110.0;
            ui.add(
                egui::TextEdit::singleline(&mut self.playlist_entry)
                    .desired_width(width)
                    .hint_text(RichText::new("name").color(theme::dim())),
            )
            .on_hover_text("What to call the playlist the tracks showing would go into.");
            let name = self.playlist_entry.trim().to_string();
            if ui
                .add_enabled(
                    !name.is_empty() && showing > 0,
                    egui::Button::new(format!("Add {showing}")),
                )
                .clicked()
            {
                self.add_to_playlist(&name);
            }
        });
    }

    /// Send duplicate files to the trash, and forget the tracks that named
    /// them.
    ///
    /// To the trash rather than unlinked, so that a wrong answer here is one
    /// the operating system can undo — this is the only thing in the program
    /// that touches somebody's music, and the difference between recoverable
    /// and not is the whole of how careful it has to be.
    ///
    /// A file that will not go stays in the collection. Forgetting a track
    /// whose file is still there would leave the file behind with nothing
    /// pointing at it, which is a worse state than the duplicate was.
    fn trash_duplicates(
        &mut self,
        going: &[(u32, u32)],
        picked: &[(u32, crate::library::Field, crate::library::Side)],
        tags: &[(u32, Vec<String>)],
    ) {
        let mut gone = 0usize;
        let mut freed = 0u64;

        for (id, keep) in going {
            let Some(track) = self.library.get(*id) else { continue };
            let (path, bytes) = (track.path.clone(), track.bytes);
            // Folded in first, and only then deleted: doing it the other way
            // round would be reading a record that is already gone. A file that
            // then will not go leaves the copy in the collection with its own
            // answers still on it, which is untidy but loses nothing.
            {
                let answers: std::collections::HashMap<_, _> = picked
                    .iter()
                    .filter(|(copy, _, _)| copy == id)
                    .map(|(_, field, side)| (*field, *side))
                    .collect();
                self.library.merge_copy(*keep, *id, &answers);
            }
            match trash::delete(&path) {
                Ok(()) => {
                    crate::info!("trashed {}", path.display());
                    self.library.remove(*id);
                    gone += 1;
                    freed += bytes;
                }
                Err(e) => {
                    crate::warn!("could not trash {}: {e}", path.display());
                    self.note(
                        format!("{} would not go to the trash", path.display()),
                        theme::alert(),
                    );
                }
            }
        }
        // After the folding, not before: folding a copy in unions its tags
        // onto the kept track, so this is the last word on which of them stay —
        // including a tag the kept track already had and somebody has just
        // struck out, which is the one part of a merge that can take away.
        for (id, wanted) in tags {
            let Some(track) = self.library.get_mut(*id) else { continue };
            if track.tags != *wanted {
                crate::debug!("#{id} tags: {} -> {}", track.tags.join(" "), wanted.join(" "));
                track.tags.clone_from(wanted);
            }
        }

        if gone > 0 {
            self.note(
                format!("{} to the trash, {} freed", plural(gone, "file"), sync::bytes(freed)),
                theme::text(),
            );
        }
        // The copy kept is the one that knows the most about the record, which
        // is not always the one in the library folder — so de-duplicating can
        // leave the collection pointing at somebody's download folder. That is
        // the same situation as importing from outside, and gets the same
        // answer: whatever the setting says, which is to take a copy unless
        // told otherwise.
        let survivors: Vec<u32> = going.iter().map(|(_, keep)| *keep).collect();
        self.ensure_local(&survivors);

        // A kept file called `track_04 (1).flac` was named by a copier, and now
        // that the `track_04.flac` it was copied from has gone, the plain name
        // is free again. Worked out here rather than before the deleting,
        // because until then the name is taken.
        let mut offers: Vec<(u32, PathBuf)> = Vec::new();
        for id in &survivors {
            let Some(track) = self.library.get(*id) else { continue };
            let Some(plain) = crate::library::name_without_copy_number(&track.path) else {
                continue;
            };
            if plain.exists() || offers.iter().any(|(_, taken)| *taken == plain) {
                continue;
            }
            offers.push((*id, plain));
        }
        offers.retain(|(id, _)| !self.renames.iter().any(|(seen, _)| seen == id));
        self.renames.extend(offers);
    }

    /// Put the selection, or everything showing, into a playlist.
    fn add_to_playlist(&mut self, name: &str) {
        let ids = self.acting_on(|_| true);
        self.add_tracks_to_playlist(&ids, name);
    }

    /// Put named tracks in a playlist, making it if it is new.
    ///
    /// A track already in it is not added twice: a playlist is an order to
    /// play things in, and the same record twice over is a mistake rather than
    /// an instruction. Companions are refused — a stem goes on a drive with
    /// its parent, and a playlist holding one without the other would write
    /// the acapella and not the record.
    fn add_tracks_to_playlist(&mut self, ids: &[u32], name: &str) {
        let ids: Vec<u32> =
            ids.iter().copied().filter(|id| self.library.get(*id).is_some()).collect();
        if ids.is_empty() {
            self.note("nothing to add", theme::dim());
            return;
        }
        let playlist = match self.library.playlists.iter_mut().find(|p| p.name == name) {
            Some(existing) => existing,
            None => {
                self.library.playlists.push(Playlist {
                    name: name.to_string(),
                    folder: String::new(),
                    tracks: Vec::new(),
                });
                self.library.playlists.last_mut().expect("just pushed")
            }
        };
        let added = ids.iter().filter(|id| !playlist.tracks.contains(id)).count();
        for id in ids {
            if !playlist.tracks.contains(&id) {
                playlist.tracks.push(id);
            }
        }
        self.note(format!("{added} added to \u{201c}{name}\u{201d}"), theme::text());
        self.save();
        self.replan();
        // The list is a built thing and has to follow. Without this a drop
        // onto the playlist being looked at changed the sidebar's count — read
        // straight from the collection — and not the rows under it, so the two
        // disagreed until something else happened to rebuild them.
        self.pending_rebuild = true;
    }

    /// The column names, pinned above the list.
    /// Draw the header across `total` points, and return the widths it drew
    /// with, for the rows to line up under.
    ///
    /// The same widths, not the ones a drag has just asked for: a boundary
    /// moved this frame lands on the next one, so the names and the rows under
    /// them are never a column apart even for a frame.
    fn header(&mut self, ui: &mut Ui, total: f32) -> rows::Widths {
        let widths = self.config.columns.widths(total);
        let head = rows::header_row(ui, &mut self.config.columns, &widths, self.sort);

        if head.resized {
            self.layout_moved = true;
        }
        // One click with one answer, unlike a drag: written now rather than
        // when the pointer next comes up, because there is no stream of them.
        if head.chosen {
            crate::debug!(
                "columns: {}",
                self.config
                    .columns
                    .columns
                    .iter()
                    .filter(|slot| slot.shown)
                    .map(|slot| slot.column.name())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            if let Err(e) = self.config.save(&self.config_path) {
                crate::warn!("could not save the columns: {e:#}");
            }
        }
        if let Some(column) = head.sorted {
            let was = self.sort;
            self.sort = self.sort.clicked(column);
            self.config.sort = self.sort;
            let _ = self.config.save(&self.config_path);
            crate::debug!(
                "sort {} {} (was {} {})",
                self.sort.column.name(),
                if self.sort.descending { "descending" } else { "ascending" },
                was.column.name(),
                if was.descending { "descending" } else { "ascending" }
            );
            self.pending.push(Pending::Resort);
        }
        widths
    }

    fn rows_table(&mut self, ui: &mut Ui, widths: &rows::Widths) {
        // Gathered once rather than per row: the menu names the same playlists
        // whichever line it was opened on, and the index it reports back is
        // into this.
        let playlist_names: Vec<String> =
            self.library.playlists.iter().map(|p| p.name.clone()).collect();
        // The selection in list order, gathered once: a drag from any of these
        // rows carries all of them.
        let chosen: Vec<u32> = match self.marked.len() > 1 {
            true => self
                .rows
                .iter()
                .filter(|row| !row.indented && self.marked.contains(&row.track.id))
                .map(|row| row.track.id)
                .collect(),
            false => Vec::new(),
        };
        let playing = self
            .player
            .as_ref()
            .filter(|player| player.is_playing())
            .and_then(|player| player.loaded());
        let mut hit = None;
        for line in &self.rows {
            // Marked rows read as chosen; the focused one is what the
            // inspector is showing, and is marked too.
            let selected =
                self.marked.contains(&line.track.id) || Some(line.track.id) == self.selected;
            // Grabbing a row that is part of the selection carries all of it;
            // grabbing one outside carries only that one, which is what a
            // drag starting somewhere else means.
            let carrying: Vec<u32> = match self.marked.contains(&line.track.id) {
                true => chosen.clone(),
                false => vec![line.track.id],
            };
            let menu = rows::Menu {
                in_library: self.config.holds(&line.track.path),
                playing: playing == Some(line.track.id),
                in_playlist: self.view == View::Playlist,
                playlists: &playlist_names,
                dragging: &carrying,
            };
            if let Some(what) = rows::row(ui, &line.track, line.indented, selected, widths, menu) {
                hit = Some((line.track.id, what));
            }
        }
        if let Some((id, what)) = hit {
            // A right-click does not move the selection. The menu names the
            // track it will act on, and stealing the selection would throw away
            // whatever is loaded on the deck to run an errand on something else.
            let selects = !matches!(what, rows::Hit::Chose(_));
            // Says whether picking this row already handed it the deck, which
            // it does when the row is another part of what is playing.
            let carried = selects && self.select(id);
            if selects {
                // Shift takes everything between; the command key takes this
                // one as well as what is already picked; a plain click starts
                // again from here.
                let keys = ui.input(|i| i.modifiers);
                match (keys.shift, keys.command) {
                    (true, _) => self.mark_range_to(id),
                    (_, true) => self.mark_toggle(id),
                    _ => self.mark_only(id),
                }
            }
            match what {
                // Already playing, from the moment the last part had reached:
                // double-clicking a stem means "play this", and the selection
                // did exactly that. Toggling on top would stop it again.
                rows::Hit::Opened if carried => {}
                rows::Hit::Opened => self.pending.push(Pending::TogglePlayback(id)),
                rows::Hit::Chose(action) => self.pending.push(match action {
                    rows::Action::Play => Pending::TogglePlayback(id),
                    rows::Action::Analyze => Pending::Analyze(id),
                    rows::Action::Identify => Pending::Identify(id),
                    rows::Action::Separate => Pending::Separate(id),
                    rows::Action::AutoCue => Pending::AutoCue(id),
                    rows::Action::CopyIn => Pending::Adopt(id),
                    rows::Action::Reveal => Pending::CopyPath(id),
                    rows::Action::RemoveFromPlaylist => {
                        Pending::RemoveFromPlaylist(id, self.playlist.clone())
                    }
                    // A menu drawn from this same list, so the index is in
                    // it; an empty name falls through to naming a new one
                    // rather than silently doing nothing.
                    rows::Action::AddTo(at) => match playlist_names.get(at) {
                        Some(name) => Pending::AddToPlaylist(vec![id], name.clone()),
                        None => Pending::NamePlaylistFor(vec![id]),
                    },
                    rows::Action::AddToNew => Pending::NamePlaylistFor(vec![id]),
                    rows::Action::Forget => Pending::Forget(id),
                }),
                rows::Hit::Clicked => {}
            }
        }

        if self.rows.is_empty() {
            ui.add_space(24.0);
            ui.vertical_centered(|ui| {
                let message = if self.library.tracks.is_empty() {
                    "Drop music here, or press ⌘K and start a query."
                } else {
                    "Nothing matches."
                };
                ui.label(RichText::new(message).color(theme::dim()));
            });
        }
    }

    /// The prep editor: waveform, phrase strip, and the line of measurements.
    fn prep(&mut self, ui: &mut Ui) {
        // Where the editor starts, so that what it came to can be measured
        // against it rather than against the top of whatever is above it.
        let from = ui.cursor().top();
        let track = self.selected_track().cloned();
        let Some(track) = track else {
            // Nothing to draw. In the main window that is a strip of nothing
            // under the list, which reads fine; in a window of its own it is
            // an empty window, which reads as broken.
            ui.label(RichText::new("nothing selected").color(theme::dim()));
            // Not measured from a frame that drew one line: the list would
            // take the room back, and selecting a track would then have the
            // editor open at a line's height with everything below the
            // waveform clipped away.
            return;
        };

        // Whether there is a picture is decided once, up front: the panel goes
        // on to change the collection, and a borrow of the cache held across
        // that would stop it.
        let has_bands = matches!(&self.waveform, Some((id, _)) if *id == track.id);
        let bands: &[u8] = match &self.waveform {
            Some((id, bands)) if *id == track.id => bands,
            _ => &[],
        };
        let beat_ms = beat_times(&track);
        let playhead = self.playhead_ms;
        let paint = self.config.paint;
        let envelopes = match &self.envelopes {
            Some((id, measured)) if *id == track.id => Some(measured),
            _ => None,
        };
        let has_envelopes = envelopes.is_some();
        let waveform = wave::Waveform {
            bands,
            duration_secs: track.duration_secs,
            beat_ms: &beat_ms,
            cues: &track.cues,
            position: playhead
                .map(|ms| (ms as f64 / (track.duration_secs * 1000.0).max(1.0)) as f32),
            paint,
            stems: envelopes,
            zoom: self.zoom,
            // Only while the grid is being corrected. They are scaffolding for
            // a fit, not a thing to look at the rest of the time.
            marks: match self.editing_grid {
                true => &track.beat_marks,
                false => &[],
            },
            words: &track.lyrics,
            aligning: self.editing_grid,
        };
        // Drawn before anything below touches the collection: `waveform`
        // borrows the cached picture out of the window's own state, and that
        // borrow has to be finished with before the panel changes anything.
        let shown = wave::show(ui, &waveform);
        let strip = wave::phrase_strip(
            ui,
            &track.phrases,
            track.duration_secs,
            &beat_ms,
            shown.zoom,
            bands,
        );
        let zoom = strip.zoom.unwrap_or(shown.zoom);
        if let Some(edit) = strip.edit {
            self.pending.push(Pending::EditPhrase { id: track.id, edit });
        }

        match shown.touched {
            Some(wave::Touched::Scrubbed(ms)) => {
                self.playhead_ms = Some(ms);
                self.pending.push(Pending::SeekDeck { id: track.id, time_ms: ms });
            }
            Some(wave::Touched::Moved { at, time_ms }) => {
                self.pending.push(Pending::MoveCue { id: track.id, at, time_ms })
            }
            Some(wave::Touched::Marked(ms)) => {
                self.pending.push(Pending::MarkBeat { id: track.id, at_ms: ms })
            }
            None => {}
        }
        self.zoom = zoom;

        self.cue_strip(ui, &track);
        self.grid_tools(ui, &track);

        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 16.0;

            measurement(ui, "grid", &grid_text(&track, self.config.length), track.has_grid);
            // Beside the reading it corrects. Off by default: these are for
            // the few records a tracker gets wrong, and a row of buttons under
            // every waveform is eight controls in the way of the nine tracks
            // in ten that need none of them.
            // Only the way in. The way out is in the row it closes, so that
            // the two are never the same control in a line that the row
            // itself can push off the bottom of the window.
            if !self.editing_grid
                && ui
                    .add_enabled(track.has_grid, egui::Button::new("fix").small())
                    .on_hover_text(
                        "Correct this grid by hand — halve or double the tempo, slide \
                         it onto the kicks, or say where the one is",
                    )
                    .on_disabled_hover_text("Nothing has measured a grid on this track yet")
                    .clicked()
            {
                self.editing_grid = true;
            }
            measurement(
                ui,
                "key",
                &if track.key.is_empty() {
                    "none".to_string()
                } else {
                    format!("{} {:.2}", track.key, track.key_confidence)
                },
                !track.key.is_empty(),
            );
            // The rank and the number it is a rank of, because five bars cannot
            // say whether a track sat just under a threshold or nowhere near
            // one — and a meter nobody can argue with is a meter nobody can
            // correct.
            measurement(
                ui,
                "energy",
                &match track.energy {
                    0 => "not measured".to_string(),
                    rank => format!("{rank}/5 · {:.3}", track.intensity),
                },
                track.energy > 0,
            );
            measurement(ui, "phrase", &track.phrases.len().to_string(), !track.phrases.is_empty());
            measurement(ui, "cues", &track.cues.len().to_string(), !track.cues.is_empty());
            measurement(
                ui,
                "loudness",
                &match (track.loudness_lufs, track.peak_dbtp) {
                    (Some(lufs), Some(peak)) => format!("{lufs:.1} LUFS · {peak:.1} dBTP"),
                    _ => "not measured".to_string(),
                },
                track.loudness_lufs.is_some(),
            );
            measurement(
                ui,
                "stems",
                if track.stems.is_complete() {
                    "kit"
                } else if track.stems.is_empty() {
                    "none"
                } else {
                    "partial"
                },
                track.stems.is_complete(),
            );
            // What the recogniser made of the vocal stem, and what it decided
            // the hook was. Shown because a hook found in a badly heard
            // transcript is a cue in the wrong place, and the only way to know
            // that has happened is to be told what it thinks it heard.
            measurement(
                ui,
                "words",
                &match (track.lyrics.len(), track.refrains.first()) {
                    (0, _) => "not read".to_string(),
                    (lines, Some(hook)) => format!(
                        "{lines} lines \u{b7} {}\u{d7} \u{201c}{}\u{201d}",
                        hook.times(),
                        hook.text
                    ),
                    (lines, None) => format!("{lines} lines \u{b7} nothing repeats"),
                },
                !track.lyrics.is_empty(),
            );
        });

        // The same three jobs the toolbar runs over everything showing, aimed
        // at the one track in front of you. They are named for what they will
        // do to *this* track rather than generically, because the answer to
        // "will this take twenty minutes" is different for a first pass and a
        // re-run, and a button that hides which one it is cannot be trusted
        // with a stem render.
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            let busy = self.runner.is_some();
            let there = track.path.exists();
            let reason = if !there {
                Some("the file is not where it was")
            } else if busy {
                Some("something is already running")
            } else {
                None
            };

            let offer = |ui: &mut Ui, label: &str, hint: &str| {
                let button = ui.add_enabled(reason.is_none(), egui::Button::new(label));
                match reason {
                    Some(why) => button.on_disabled_hover_text(why).clicked(),
                    None => button.on_hover_text(hint).clicked(),
                }
            };

            if offer(
                ui,
                if track.analyzed { "Re-analyse" } else { "Analyse" },
                "Beats, key, phrases, cues and the waveform, measured again from the file.",
            ) {
                self.pending.push(Pending::Analyze(track.id));
            }
            if offer(
                ui,
                if track.identified { "Look up again" } else { "Look up tags" },
                "Fingerprint it and ask AcoustID and MusicBrainz what it is.",
            ) {
                self.pending.push(Pending::Identify(track.id));
            }
            if offer(
                ui,
                if track.stems.is_empty() { "Render stems" } else { "Render stems again" },
                "Separate it into vocals, melody and drums. Minutes, not seconds.",
            ) {
                self.pending.push(Pending::Separate(track.id));
            }
            if offer(
                ui,
                if track.lyrics.is_empty() {
                    "Cue from the words"
                } else {
                    "Cue from the words again"
                },
                "Read the vocal stem, find the line it keeps coming back to, and cue that, \
                 the drops and the phrases. Replaces the hot cues it has.",
            ) {
                self.pending.push(Pending::AutoCue(track.id));
            }
            // Its own entry rather than only a step inside the one above,
            // because the two cost different things and somebody who knows
            // which record this is should be able to ask for the cheap one.
            // Seconds and one request, against minutes of separation and a
            // recogniser pass.
            let named = !track.artist.trim().is_empty() && !track.title.trim().is_empty();
            let button =
                ui.add_enabled(reason.is_none() && named, egui::Button::new("Look up the words"));
            let asked = match (reason, named) {
                (Some(why), _) => button.on_disabled_hover_text(why).clicked(),
                (None, false) => button
                    .on_disabled_hover_text(
                        "A lyrics server is asked by artist and title, and this has none.",
                    )
                    .clicked(),
                (None, true) => button
                    .on_hover_text(
                        "Ask a lyrics server for this track's words, under its artist and \
                         title. No stem and no recogniser — seconds rather than minutes. \
                         Words it is sure of are taken; anything less is put to you first.",
                    )
                    .clicked(),
            };
            if asked {
                self.pending.push(Pending::LookUpWords(track.id));
            }
        });

        // The waveform is only read when it is looked at: a collection of
        // thousands cannot keep every picture in memory, and re-measuring one
        // track takes less time than the click that asked for it.
        // Colouring by stems needs them measured. Once per run per track, the
        // same way the waveform is: a kit that will not decode must not start a
        // job on every frame.
        if paint == wave::Paint::Stems
            && !has_envelopes
            && track.stems.is_complete()
            && !self.re_enveloped.contains(&track.id)
        {
            match crate::library::cached_envelopes(track.id) {
                Some(cached) => self.envelopes = Some((track.id, cached)),
                None if !self.running() => {
                    self.re_enveloped.insert(track.id);
                    self.start(Job::StemEnvelopes { id: track.id, kit: track.stems.clone() });
                }
                None => {}
            }
        }

        if !has_bands && worth_drawing(&track) {
            // Worked out before the cache is consulted, because the cache can
            // only say whether its picture is still of these files if it knows
            // which files they are.
            let sources = picture_sources(&track);
            let cached =
                sources.as_deref().and_then(|of| crate::library::cached_waveform(track.id, of));
            match cached {
                Some(cached) => self.waveform = Some((track.id, cached)),
                // Nothing cached — analysed by an older version, the cache was
                // cleared, or this is a stem companion, which has never had a
                // picture of its own. Measure one, once, in the background.
                //
                // The picture only. This used to run a whole analysis, which
                // re-derived the grid, the key, the cues and the phrases and
                // wrote them over whatever the collection had — so selecting a
                // track whose cache had been cleared quietly discarded every
                // cue somebody had moved by hand. Re-analysing is a thing to
                // ask for, not a thing that happens because a row was clicked.
                //
                // Once, and remembered: this runs every frame the row is
                // selected, so one that cannot be measured — its file has
                // moved, it will not decode — would otherwise start a job,
                // fail, and start another for as long as it stayed selected.
                None if !self.running() && !self.remeasured.contains(&track.id) => {
                    if let Some(sources) = sources {
                        self.remeasured.insert(track.id);
                        self.start(Job::Draw(vec![job::Drawable { id: track.id, sources }]));
                    }
                }
                None => {}
            }
        }

        // What it came to, for the list above it to keep room for next frame.
        // One frame behind, which nobody sees: the editor's height changes
        // when a row is shown or hidden, and that is a press rather than
        // something that happens while the eye is on it.
        //
        // The widgets are laid out whether or not there is room to paint them,
        // so this is the height the editor *wanted* even on the frame it was
        // clipped — which is what makes it correct itself rather than settle
        // into the clipped height and stay there.
        //
        // With a few points of slack on top, because the sum is exact and an
        // exact fit leaves the last line sitting on the dock's edge — and one
        // point of rounding the other way clips it, which is the failure this
        // is here to end.
        self.prep_height = (ui.min_rect().bottom() - from).max(0.0) + 6.0;
    }

    // -- the deck ----------------------------------------------------------

    /// Play a track, decoding it first if it is not the one already loaded.
    fn audition(&mut self, id: u32, from_secs: Option<f64>) {
        let Some(player) = &self.player else { return };
        // `row` rather than `get`: a stem companion has an id of its own so it
        // can be selected, but it is not in the collection — it is made when
        // the list is built, and looking it up as a track finds nothing.
        let Some(track) = self.library.row(id) else { return };
        let sources = track.sources();
        if sources.is_empty() {
            self.note(format!("no {} rendered for that track", track.role.label()), theme::alert());
            return;
        }
        if let Some(missing) = sources.iter().find(|path| !path.exists()) {
            crate::warn!("cannot play #{id}: {} is gone", missing.display());
            self.note("that file is not where it was", theme::alert());
            return;
        }

        if player.loaded() == Some(id) {
            if let Some(secs) = from_secs {
                player.seek_secs(secs);
            }
            player.play();
            return;
        }

        // Playing a file is reaching for it, so this is one of the moments the
        // copy-in policy is about. The copy is queued first and the decode
        // behind it, so what plays is whatever the track's path is by then.
        // Only a real track is copied: a stem belongs to wherever its parent
        // ended up, and following it would put half a kit in the library.
        crate::debug!(
            "loading #{id} from {}",
            sources.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(" + ")
        );
        // Silence first. A track is decoded whole before it can be played, and
        // that is seconds on a long file — during which the deck was still
        // playing the last one, so asking for a new track left the old one
        // going and then cut to the new one whenever the decode happened to
        // land. Stopping now makes the deck do what was asked at the moment it
        // was asked, and the wait is silence rather than the wrong record.
        player.pause();

        if track.role == crate::library::Role::Track {
            self.ensure_local(&[id]);
        }
        self.loading = Some((id, true));
        self.start(Job::Decode { id, sources });
        if let Some(secs) = from_secs {
            // Remembered for when it lands: the deck cannot be seeked to a
            // position in a track it has not been given yet.
            self.playhead_ms = Some((secs * 1000.0) as u32);
        }
    }

    /// Space, and the transport under the waveform.
    fn transport(&mut self, ui: &mut Ui, track: &Track) {
        let Some(player) = &self.player else {
            // Short, because this row is mostly cue buttons and the reason is
            // rarely actionable; the whole of it is one hover away.
            if let Some(problem) = &self.player_problem {
                ui.label(RichText::new("no audio out").color(theme::dim()).size(theme::SMALL))
                    .on_hover_text(problem);
            }
            return;
        };

        let loaded = player.loaded() == Some(track.id);
        let playing = loaded && player.is_playing();
        let waiting = self.loading.is_some_and(|(id, _)| id == track.id);

        let label = match (playing, waiting) {
            (_, true) => theme::SPINNER,
            (true, _) => "\u{23F8}",
            (false, _) => "\u{25B6}",
        };
        if ui
            .add(
                egui::Button::new(
                    RichText::new(label).font(theme::mono(12.0)).color(theme::booth()),
                )
                .fill(if playing { theme::go() } else { theme::amber() })
                .min_size(egui::vec2(30.0, 18.0)),
            )
            .on_hover_text("Space")
            .clicked()
        {
            // Recorded rather than acted on: the deck is borrowed for the rest
            // of this row, and starting a track needs it back.
            self.pending.push(Pending::TogglePlayback(track.id));
        }

        // Where it is, in the unit a set is counted in. The playhead rather
        // than the deck when the deck is not on this track, so the reading
        // follows the marker being placed.
        let at_ms = match loaded {
            true => (player.position_secs() * 1000.0) as u32,
            false => self.playhead_ms.unwrap_or(0),
        };
        let length = self.config.length;
        let counting = self.config.counting;
        let total_ms = (track.duration_secs * 1000.0) as u32;
        let reading = match beat_at(track, at_ms) {
            Some(beat) => counting.reading(length, beat, track.beats, at_ms, total_ms),
            // No grid, so no bars to count in — the clock is all there is, and
            // it still answers whichever question is being asked.
            None => match counting {
                crate::config::Counting::Elapsed => crate::config::clock(at_ms),
                crate::config::Counting::Remaining => {
                    format!("-{}", crate::config::clock(total_ms.saturating_sub(at_ms)))
                }
            },
        };
        // Clickable, the way a player's time display is: the two questions a
        // DJ asks of a running track are where am I and how long have I got,
        // and they are never both wanted at once.
        let clicked = ui
            .add(
                egui::Label::new(RichText::new(reading).font(theme::mono(10.5)).color(if loaded {
                    theme::text()
                } else {
                    theme::dim()
                }))
                .sense(egui::Sense::click()),
            )
            .on_hover_text(format!(
                "{} of {} — {} of {}\nclick for {}",
                length.position(beat_at(track, at_ms).unwrap_or(0)),
                length.describe(track.beats),
                time_text(at_ms),
                time_text(total_ms),
                match counting {
                    crate::config::Counting::Elapsed => "what is left",
                    crate::config::Counting::Remaining => "where you are",
                }
            ))
            .clicked();
        if clicked {
            self.pending.push(Pending::CountDifferently);
        }

        let mut gain = player.gain();
        if ui
            .add(
                egui::Slider::new(&mut gain, 0.0..=1.0)
                    .show_value(false)
                    .handle_shape(egui::style::HandleShape::Rect { aspect_ratio: 0.4 }),
            )
            .on_hover_text(format!("volume {:.0}%", gain * 100.0))
            .changed()
        {
            player.set_gain(gain);
        }
    }

    /// Start or stop, loading the track first if it is not the one on the deck.
    fn toggle_playback(&mut self, id: u32) {
        let Some(player) = &self.player else { return };
        match player.loaded() == Some(id) {
            true => player.toggle(),
            // Playing a different track starts from wherever the playhead was
            // put, which is what makes clicking a waveform and pressing space
            // one gesture rather than two.
            false => {
                let from = self.playhead_ms.map(|ms| ms as f64 / 1000.0);
                self.audition(id, from);
            }
        }
    }

    /// The eight hot cues, as eight buttons.
    ///
    /// One press per cue, the way a player has it: an empty slot takes the
    /// playhead, a full one moves the playhead to it, and shift-clicking clears
    /// it. Everything is snapped to the grid on the way in, so a cue placed by
    /// eye still lands on a beat.
    fn cue_strip(&mut self, ui: &mut Ui, track: &Track) {
        let paint = self.config.paint;
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            self.transport(ui, track);
            ui.separator();
            ui.label(
                RichText::new(theme::label_text("Cues"))
                    .size(theme::LABEL)
                    .color(theme::dim())
                    .strong(),
            );

            let at = self.playhead_ms;
            let under = self.playhead_cue(track).map(|(at, _)| at);
            let memory = track.cues.iter().filter(|cue| cue.letter == 0).count();
            for letter in 0..=8u8 {
                // One MEM button, and a track can have as many memory cues as
                // it needs. The one under the playhead is the one being looked
                // at, so that is the one the button is about — which is what
                // makes every memory cue reachable from a single button rather
                // than only whichever sorts first.
                let found = match letter {
                    0 => under
                        .filter(|at| track.cues.get(*at).is_some_and(|cue| cue.letter == 0))
                        .map(|at| (at, &track.cues[at]))
                        .or_else(|| track.cues.iter().enumerate().find(|(_, cue)| cue.letter == 0)),
                    _ => track.cues.iter().enumerate().find(|(_, cue)| cue.letter == letter),
                };
                let cue = found.map(|(_, cue)| cue);
                let which = found.map(|(at, cue)| WhichCue::of(at, cue));
                let name = match letter {
                    0 => "MEM".to_string(),
                    n => char::from(b'A' + n - 1).to_string(),
                };
                let color = match cue {
                    Some(cue) => Color32::from_rgb(cue.color[0], cue.color[1], cue.color[2]),
                    None => theme::rule(),
                };
                let response = ui
                    .add(
                        egui::Button::new(RichText::new(&name).font(theme::mono(10.5)).color(
                            match cue {
                                Some(_) => theme::booth(),
                                None => theme::dim(),
                            },
                        ))
                        .fill(match cue {
                            Some(_) => color,
                            None => theme::booth(),
                        })
                        .min_size(egui::vec2(30.0, 18.0)),
                    )
                    .on_hover_text(match cue {
                        Some(cue) => format!(
                            "{} — {}{}{}",
                            time_text(cue.time_ms),
                            if cue.label.is_empty() { "no name" } else { &cue.label },
                            // Said only when there is more than one, so that a
                            // button acting on one of several says so rather
                            // than looking like it is the only one.
                            match letter == 0 && memory > 1 {
                                true => format!(
                                    "\n{} on this track — park the playhead on one to work \
                                     with it",
                                    plural(memory, "memory cue")
                                ),
                                false => String::new(),
                            },
                            "\nclick to jump, shift-click to clear, right-click for more, \
                             or drag it on the waveform"
                        ),
                        None => "click to put a cue at the playhead".to_string(),
                    });

                // Right-click: everything about a cue that is not where it
                // is. Where it is is a drag on the waveform, which is already
                // the better gesture for it.
                if let (Some(cue), Some(which)) = (cue, which) {
                    let letter = cue.letter;
                    let id = track.id;
                    response.context_menu(|ui| {
                        ui.set_min_width(220.0);
                        ui.label(
                            RichText::new(format!("{} \u{2014} {}", name, time_text(cue.time_ms)))
                                .font(theme::mono(10.0))
                                .color(theme::dim()),
                        );

                        // What it is called. The one thing a player shows
                        // under the cue, and the thing a DJ finds it by.
                        if self.cue_menu.0 != Some((id, which.at)) {
                            self.cue_menu = (Some((id, which.at)), cue.label.clone());
                        }
                        let typed = ui
                            .add(
                                egui::TextEdit::singleline(&mut self.cue_menu.1)
                                    .desired_width(f32::INFINITY)
                                    .font(theme::mono(10.5))
                                    .hint_text(
                                        RichText::new("what is sung here")
                                            .monospace()
                                            .color(theme::dim()),
                                    ),
                            )
                            .on_hover_text(
                                "What this cue is called. A player shows it under the cue, \
                                 and it goes on the drive with it.",
                            );
                        if typed.changed() || typed.lost_focus() {
                            self.pending.push(Pending::RenameCue {
                                id,
                                which,
                                label: self.cue_menu.1.clone(),
                            });
                        }
                        // The line the track sings there, for a cue that has
                        // not been named: it is almost always what somebody
                        // would have typed.
                        if let Some(sung) = crate::library::sung_at(&track.lyrics, cue.time_ms) {
                            if sung != cue.label.trim()
                                && ui
                                    .add(
                                        egui::Button::new(
                                            RichText::new(format!("\u{201c}{sung}\u{201d}"))
                                                .size(theme::SMALL)
                                                .color(theme::text()),
                                        )
                                        .fill(theme::booth_2()),
                                    )
                                    .on_hover_text("Name it after the line sung here")
                                    .clicked()
                            {
                                self.cue_menu = (None, String::new());
                                self.pending.push(Pending::RenameCue {
                                    id,
                                    which,
                                    label: sung.to_string(),
                                });
                                ui.close();
                            }
                        }

                        ui.separator();
                        ui.label(
                            RichText::new(theme::label_text("Colour"))
                                .size(theme::LABEL)
                                .color(theme::dim())
                                .strong(),
                        );
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 3.0;
                            for swatch in theme::CUE_COLORS {
                                let rgb = [swatch.r(), swatch.g(), swatch.b()];
                                let mine = rgb == cue.color;
                                if ui
                                    .add(
                                        egui::Button::new(
                                            RichText::new(match mine {
                                                true => theme::TICK,
                                                false => " ",
                                            })
                                            .font(theme::mono(9.0))
                                            .color(theme::booth()),
                                        )
                                        .fill(swatch)
                                        .min_size(egui::vec2(20.0, 16.0)),
                                    )
                                    .clicked()
                                {
                                    self.pending.push(Pending::RecolorCue {
                                        id,
                                        which,
                                        color: rgb,
                                    });
                                    ui.close();
                                }
                            }
                        });

                        ui.separator();
                        match letter {
                            // A memory cue has no button to press, so making
                            // it a hot one means choosing which button.
                            0 => {
                                ui.label(
                                    RichText::new(theme::label_text("Make it a hot cue"))
                                        .size(theme::LABEL)
                                        .color(theme::dim())
                                        .strong(),
                                );
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = 3.0;
                                    for into in 1..=8u8 {
                                        let free = !track.cues.iter().any(|cue| cue.letter == into);
                                        if ui
                                            .add_enabled(
                                                free,
                                                egui::Button::new(
                                                    RichText::new(char::from(b'A' + into - 1))
                                                        .font(theme::mono(10.0)),
                                                )
                                                .min_size(egui::vec2(20.0, 16.0)),
                                            )
                                            .on_disabled_hover_text("Already in use")
                                            .clicked()
                                        {
                                            self.pending.push(Pending::RecastCue {
                                                id,
                                                which,
                                                into,
                                            });
                                            ui.close();
                                        }
                                    }
                                });
                            }
                            _ => {
                                if ui
                                    .button("Make it a memory cue")
                                    .on_hover_text(
                                        "Keeps the mark and gives up the button. A player \
                                         shows memory cues as marks on the waveform and \
                                         cannot drop into them.",
                                    )
                                    .clicked()
                                {
                                    self.pending.push(Pending::RecastCue { id, which, into: 0 });
                                    ui.close();
                                }
                            }
                        }
                        if ui
                            .button("Clear this cue")
                            .on_hover_text(
                                "Take the mark off the track. Shift-clicking the button does \
                                 the same.",
                            )
                            .clicked()
                        {
                            self.pending.push(Pending::RemoveCue { id, which });
                            ui.close();
                        }
                    });
                }

                let shift = ui.input(|i| i.modifiers.shift);
                if response.clicked() {
                    match (cue, shift) {
                        (Some(_), true) => {
                            if let Some(which) = which {
                                self.pending.push(Pending::RemoveCue { id: track.id, which });
                            }
                        }
                        // Jumping to a cue takes the deck with it, so an ear
                        // can check a cue rather than only an eye.
                        (Some(cue), false) => {
                            self.playhead_ms = Some(cue.time_ms);
                            self.pending
                                .push(Pending::SeekDeck { id: track.id, time_ms: cue.time_ms });
                        }
                        (None, _) => {
                            if let Some(time_ms) = at {
                                self.pending.push(Pending::PlaceCue {
                                    id: track.id,
                                    letter,
                                    time_ms,
                                });
                            }
                        }
                    }
                }
            }

            ui.separator();
            match self.playhead_ms {
                Some(ms) => {
                    ui.label(
                        RichText::new(time_text(ms)).font(theme::mono(10.5)).color(theme::text()),
                    );
                }
                None => {
                    ui.label(
                        RichText::new("click the waveform to place the playhead")
                            .size(theme::SMALL)
                            .color(theme::dim()),
                    );
                }
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 3.0;
                // Right to left, so the modes read in their usual order.
                for mode in wave::Paint::ALL.iter().rev().copied() {
                    let on = paint == mode;
                    // Stem colouring is offered whatever the track has, and
                    // falls back to frequency without a kit — greying it out
                    // would hide that the choice is remembered for next time.
                    let ready = mode != wave::Paint::Stems || track.stems.is_complete();
                    let color = match (on, ready) {
                        (true, _) => theme::booth(),
                        (false, true) => theme::dim(),
                        (false, false) => theme::rule(),
                    };
                    if ui
                        .add(
                            egui::Button::new(
                                RichText::new(mode.label()).font(theme::mono(10.0)).color(color),
                            )
                            .fill(if on {
                                theme::amber()
                            } else {
                                theme::booth()
                            }),
                        )
                        .on_hover_text(match ready {
                            true => mode.blurb().to_string(),
                            false => format!("{} — no kit rendered yet", mode.blurb()),
                        })
                        .clicked()
                    {
                        self.pending.push(Pending::PaintAs(mode));
                    }
                }

                // Only there when there is something to undo. A permanent
                // "fit" next to the colour modes would read as a fourth mode.
                if !self.zoom.is_fit() {
                    ui.add_space(8.0);
                    if ui
                        .add(egui::Button::new(
                            RichText::new("fit").font(theme::mono(10.0)).color(theme::dim()),
                        ))
                        .on_hover_text("Show the whole track again (esc)")
                        .clicked()
                    {
                        self.pending.push(Pending::FitWave);
                    }
                }
            });

            // Naming the cue under the playhead, which is the one just placed
            // or just jumped to.
            if let Some((at, cue)) = self.playhead_cue(track) {
                let which = WhichCue::of(at, cue);
                if self.cue_entry.0 != Some((track.id, at)) {
                    self.cue_entry = (Some((track.id, at)), cue.label.clone());
                }
                let response = ui
                    .add(
                        egui::TextEdit::singleline(&mut self.cue_entry.1)
                            .desired_width(140.0)
                            .font(theme::mono(10.5))
                            .hint_text(
                                RichText::new("name this cue").monospace().color(theme::dim()),
                            ),
                    )
                    .on_hover_text(
                        "What a player shows under this cue. It goes on the drive with it.",
                    );
                if response.changed() || response.lost_focus() {
                    self.pending.push(Pending::RenameCue {
                        id: track.id,
                        which,
                        label: self.cue_entry.1.clone(),
                    });
                }
            }
        });
    }

    /// The cue the playhead is sitting on, within half a beat.
    fn playhead_cue<'a>(&self, track: &'a Track) -> Option<(usize, &'a crate::library::CueMark)> {
        let at = self.playhead_ms?;
        let tolerance = match track.bpm > 0.0 {
            true => (30_000.0 / track.bpm) as u32,
            false => 250,
        };
        // With its place in the list, because that is the only name a memory
        // cue has: they all carry letter zero, so acting on "the cue with
        // letter 0" acted on whichever came first however carefully somebody
        // had parked on a different one.
        track.cues.iter().enumerate().find(|(_, cue)| cue.time_ms.abs_diff(at) <= tolerance)
    }

    fn inspector(&mut self, ui: &mut Ui) {
        pane_label(ui, "Now inspecting");
        let Some(track) = self.selected_track().cloned() else {
            ui.label(RichText::new("nothing selected").color(theme::dim()));
            self.editing = None;
            return;
        };

        // The edit follows the selection: moving to another row is not a reason
        // to lose what was typed on this one, so it is saved first. It is taken
        // out of the window's state for the duration, so that the panel can
        // read the collection while it is being typed into.
        let mut edit = match self.editing.take() {
            // Still this track, and still the record's own names: take them
            // again, so that a lookup or an import that has since written to
            // the record shows up here rather than being hidden behind a copy
            // made before it. Anything typed is the person's and is kept.
            Some(edit) if edit.id == track.id && edit.untouched() => Edit::of(&track),
            Some(edit) if edit.id == track.id => edit,
            Some(stale) => {
                self.apply_edit(stale);
                Edit::of(&track)
            }
            None => Edit::of(&track),
        };

        let editing = &mut edit;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            let width = ui.available_width();
            fn field(ui: &mut Ui, width: f32, label: &str, value: &mut String) {
                let caption = ui.label(RichText::new(label).color(theme::dim()).size(theme::SMALL));
                let field = ui
                    .add(
                        egui::TextEdit::singleline(value)
                            .desired_width(width)
                            .font(theme::sans(theme::BODY)),
                    )
                    .on_hover_text(format!("The track's {}. Save writes it into the file's tags.", label.to_lowercase()));
                // The caption sits above the box rather than beside it, so
                // nothing but this says which is which: without it a screen
                // reader announces four unnamed text boxes, and so does
                // anything else reading the window through the same tree.
                field.labelled_by(caption.id);
            }
            field(ui, width, "Title", &mut editing.names.title);
            field(ui, width, "Artist", &mut editing.names.artist);
            field(ui, width, "Album", &mut editing.names.album);
            field(ui, width, "Year", &mut editing.names.year);

            let changed = editing.differs_from(&track);
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(changed, egui::Button::new("Save"))
                    .on_hover_text("Write these fields into the collection, and into the file's tags.")
                    .on_disabled_hover_text("Nothing has been changed yet.")
                    .clicked()
                {
                    self.pending.push(Pending::CommitEdit);
                }
                if ui
                    .add_enabled(changed, egui::Button::new("Revert"))
                    .on_hover_text("Put every field back to what the collection says.")
                    .on_disabled_hover_text("Nothing has been changed yet.")
                    .clicked()
                {
                    self.pending.push(Pending::CancelEdit);
                }
                if changed {
                    ui.label(RichText::new("unsaved").color(theme::amber()).size(theme::SMALL));
                }
            });
            // Writing to the file is a separate act from editing the record,
            // and it is spelled out rather than implied, because it changes
            // somebody's files.
            let taggable =
                booth_cli::tag::Metadata::default().get(booth_cli::tag::Field::Title).is_none()
                    && matches!(track.format.as_str(), "flac" | "mp3");
            if self.config.write_tags != crate::config::WriteTags::Never {
                ui.label(
                    RichText::new(if !taggable {
                        "The file's own format carries no tags; only the collection changes."
                    } else if self.config.write_tags == crate::config::WriteTags::Fill {
                        "Saving also writes these into the file, where it has nothing."
                    } else {
                        "The file's own format carries no tags; only the collection changes."
                    })
                    .color(theme::dim())
                    .size(theme::SMALL),
                );
            } else if taggable
                && ui
                    .button("Write these into the file")
                    .on_hover_text("Rewrites the tag block. The audio is untouched.")
                    .clicked()
            {
                self.pending.push(Pending::WriteTags(track.id));
            }

            ui.add_space(10.0);
            ui.label(
                RichText::new(format!(
                    "{} · {} · {} kHz · {}",
                    track.duration_text(),
                    track.format,
                    track.sample_rate as f64 / 1000.0,
                    sync::bytes(track.bytes)
                ))
                .font(theme::mono(10.0))
                .color(theme::dim()),
            );
            ui.label(
                RichText::new(track.path.display().to_string())
                    .font(theme::mono(9.5))
                    .color(theme::dim()),
            )
            .on_hover_text(track.path.display().to_string());

            if let Some(problem) = track.needs_attention() {
                ui.add_space(6.0);
                ui.label(
                    RichText::new(format!("{} {problem}", theme::WARN))
                        .color(theme::alert())
                        .size(theme::SMALL),
                );
            }

            // A file that has been moved or deleted since it was scanned. The
            // record can be forgotten; nothing on disk is touched either way,
            // which is why this is the one removal offered without asking.
            if !track.path.exists() {
                ui.add_space(6.0);
                ui.label(
                    RichText::new(format!("{} the file is not where it was", theme::WARN))
                        .color(theme::alert())
                        .size(theme::SMALL),
                );
                if ui
                    .button("Forget this track")
                    .on_hover_text("Take it out of the collection and out of every playlist. The file is not touched.")
                    .clicked()
                {
                    self.pending.push(Pending::Forget(track.id));
                }
            } else if !self.config.holds(&track.path) {
                // Outside the library folder. Under the default policy this
                // will already have been copied; this is for the tracks added
                // before the setting was changed, and for the ones left behind
                // deliberately.
                ui.add_space(6.0);
                ui.label(
                    RichText::new(format!("{} no local copy", theme::WARN))
                        .color(theme::amber())
                        .size(theme::SMALL),
                );
                if ui
                    .button("Copy into the library")
                    .on_hover_text("The original is left where it is.")
                    .clicked()
                {
                    self.pending.push(Pending::Adopt(track.id));
                }
            }

            ui.add_space(14.0);
            pane_label(ui, "Stem kit");
            for (name, path) in track.stems.each() {
                ui.horizontal(|ui| {
                    let (mark, color) = match path {
                        Some(_) => (theme::TICK, theme::go()),
                        None => ("·", theme::dim()),
                    };
                    ui.label(RichText::new(mark).color(color));
                    ui.label(RichText::new(name).color(if path.is_some() {
                        theme::text()
                    } else {
                        theme::dim()
                    }));
                });
            }

            ui.add_space(14.0);
            pane_label(ui, "What it keeps saying");
            // Here rather than only on the strip under the waveform, because
            // this is the panel the words are read in: somebody looking at
            // "the words have not been read" should be able to do something
            // about it without going looking for the button that does.
            if ui
                .button(match track.lyrics.is_empty() {
                    true => "Find the words",
                    false => "Find the words again",
                })
                .on_hover_text(
                    "Look for a lyric file next to the track — an .lrc or a .txt of the \
                     same name — and failing that ask a lyrics server. Neither needs a \
                     stem or a recogniser.",
                )
                .clicked()
            {
                self.pending.push(Pending::FindWords(track.id));
            }
            ui.add_space(4.0);
            // The line the room sings is what a DJ recognises a record by
            // months later, long after the file name and the artwork have gone
            // out of their head. It is already measured — the cues are placed
            // off it — and it was only ever shown one line at a time on a
            // marker.
            match (track.lyrics.is_empty(), track.refrains.is_empty()) {
                (true, _) => ui.label(
                    RichText::new("the words have not been read")
                        .color(theme::dim())
                        .size(theme::SMALL),
                ),
                (false, true) => ui.label(
                    RichText::new(format!(
                        "{} \u{2014} no line comes round twice",
                        plural(track.lyrics.len(), "line")
                    ))
                    .color(theme::dim())
                    .size(theme::SMALL),
                ),
                (false, false) => ui.label(
                    RichText::new(plural(track.lyrics.len(), "line"))
                        .color(theme::dim())
                        .size(theme::SMALL),
                ),
            };
            // Which it is decides how far to trust a cue that reads oddly: a
            // recogniser's words are a machine's guess at a vocal and go wrong
            // in ways that still read like lyrics, where a server's are what
            // somebody wrote down.
            if !track.lyrics.is_empty() && !track.words_from.label().is_empty() {
                ui.label(
                    RichText::new(track.words_from.label())
                        .color(theme::dim())
                        .size(theme::SMALL),
                )
                .on_hover_text(match track.words_from {
                    crate::library::WordsFrom::Recogniser => {
                        "Read off this track's own vocal stem by the recogniser."
                    }
                    crate::library::WordsFrom::Server => {
                        "Looked up on a lyrics server under this track's artist and title, and \
                         placed against its stem."
                    }
                    crate::library::WordsFrom::Beside => {
                        "Read out of a lyric file sitting next to the track."
                    }
                    crate::library::WordsFrom::ByHand => {
                        "Typed or corrected here, which nothing overwrites on its own."
                    }
                    crate::library::WordsFrom::Unsaid => "",
                });
            }
            // What the recogniser thought of its own answer. Shown only when
            // it is poor, because the useful case is the one where the words
            // read like a lyric and are not one: a transcript nobody doubts
            // places cues exactly as confidently as a transcript that is
            // right, and until now there was nothing on screen to tell them
            // apart.
            if let Some(sure) = track.heard_surely.filter(|sure| *sure < POORLY_HEARD) {
                ui.label(
                    RichText::new(format!(
                        "heard poorly \u{2014} {:.0}% sure, so these words may not be the words",
                        sure * 100.0
                    ))
                    .color(theme::amber())
                    .size(theme::SMALL),
                )
                .on_hover_text(
                    "What the recogniser made of its own answer. A low score usually means it \
                     wrote plausible words it did not hear, which read like a lyric and cue \
                     like one.",
                );
            }
            // Every line it comes back to, scrolled rather than the first four.
            // The cap was there because the panel grew with the track and
            // because four of them is not a lyric sheet — and there is a lyric
            // sheet below now, so the second reason has somewhere better to
            // live and the first is what a scroll area is for. The order is
            // still how often, so the lines worth seeing are the ones in view
            // without scrolling at all.
            egui::ScrollArea::vertical()
                .id_salt("what-it-keeps-saying")
                .max_height(WORDS_HEIGHT)
                .auto_shrink([false, true])
                .show(ui, |ui| {
            for refrain in &track.refrains {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!("{}\u{d7}", refrain.times()))
                            .font(theme::mono(10.0))
                            .color(theme::amber()),
                    );
                    // Clickable, because the use for a line you recognise is
                    // getting to where it lands.
                    if ui
                        .add(
                            egui::Label::new(
                                RichText::new(&refrain.text)
                                    .color(theme::text())
                                    .size(theme::SMALL),
                            )
                            .wrap()
                            .sense(egui::Sense::click()),
                        )
                        .on_hover_text("Go to where this line first lands")
                        .clicked()
                    {
                        // Takes the deck with it, the same as clicking a cue:
                        // a line you half remember is one an ear settles
                        // faster than an eye.
                        if let Some(&at) = refrain.at.first() {
                            self.playhead_ms = Some(at);
                            self.pending.push(Pending::SeekDeck { id: track.id, time_ms: at });
                        }
                    }
                });
                ui.label(
                    RichText::new(
                        refrain
                            .at
                            .iter()
                            .map(|at| time_text(*at))
                            .collect::<Vec<_>>()
                            .join(" \u{b7} "),
                    )
                    .font(theme::mono(9.5))
                    .color(theme::dim()),
                );
            }
                });

            // The words themselves, which until now were kept and never
            // shown: a line count, a provenance and four repeated lines, and
            // the lyric they were all drawn from nowhere on screen. What
            // reaches a player is a comment shortened to fit a four-kilobyte
            // row, so the collection is the only place the whole thing is.
            if !track.lyrics.is_empty() {
                ui.add_space(14.0);
                pane_label(ui, "The words");
                egui::ScrollArea::vertical()
                    .id_salt("the-words")
                    .max_height(WORDS_HEIGHT)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        for line in &track.lyrics {
                            let said = line.text.trim();
                            if said.is_empty() {
                                continue;
                            }
                            ui.horizontal_top(|ui| {
                                ui.spacing_mut().item_spacing.x = 6.0;
                                // The time is the control, not the line: a
                                // lyric sheet whose every line is a button is
                                // a lyric sheet nobody can select text in.
                                if ui
                                    .add(
                                        egui::Label::new(
                                            RichText::new(time_text(line.start_ms))
                                                .font(theme::mono(9.5))
                                                .color(theme::dim()),
                                        )
                                        .sense(egui::Sense::click()),
                                    )
                                    .on_hover_text("Go to this line")
                                    .clicked()
                                {
                                    self.playhead_ms = Some(line.start_ms);
                                    self.pending.push(Pending::SeekDeck {
                                        id: track.id,
                                        time_ms: line.start_ms,
                                    });
                                }
                                ui.label(
                                    RichText::new(said).color(theme::text()).size(theme::SMALL),
                                );
                            });
                        }
                    });
            }

            // And why the cues are where they are. The pass that places them
            // says all this as it goes and used to say it only to the log,
            // which is a record that lasts until the next thing is logged.
            if !track.words_decided.is_empty() {
                ui.add_space(14.0);
                pane_label(ui, "What the words decided");
                egui::ScrollArea::vertical()
                    .id_salt("what-the-words-decided")
                    .max_height(WORDS_HEIGHT)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        for line in &track.words_decided {
                            ui.label(
                                RichText::new(line)
                                    .font(theme::mono(9.5))
                                    // The indented lines are the ranking under
                                    // a heading; the rest are what it did.
                                    .color(match line.starts_with("  ") {
                                        true => theme::dim(),
                                        false => theme::text(),
                                    }),
                            );
                        }
                    });
            }

            ui.add_space(14.0);
            pane_label(ui, "Tags");
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 3.0;
                for tag in &track.tags {
                    // The chip is the remove control: a separate × per tag
                    // doubles the number of things in a crowded panel, and a
                    // tag put back is one click either way.
                    if ui
                        .add(
                            egui::Button::new(
                                RichText::new(tag).font(theme::mono(10.0)).color(theme::text()),
                            )
                            .fill(theme::booth_2()),
                        )
                        .on_hover_text("Remove")
                        .clicked()
                    {
                        self.pending.push(Pending::RemoveTag(track.id, tag.clone()));
                    }
                }
            });
            ui.horizontal(|ui| {
                let entry = ui.add(
                    egui::TextEdit::singleline(&mut self.tag_entry)
                        .desired_width(ui.available_width() - 46.0)
                        .font(theme::mono(10.5))
                        .hint_text(RichText::new("add a tag").monospace().color(theme::dim())),
                );
                let entered = entry.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                let added = ui
                    .button("Add")
                    .on_hover_text("Put this tag on the track. Enter does the same.")
                    .clicked();
                if (entered || added) && !self.tag_entry.trim().is_empty() {
                    self.pending.push(Pending::AddTag(track.id, self.tag_entry.trim().to_string()));
                }
            });
            // The tags already in use, as one click each: a vocabulary that
            // drifts into `peak`, `Peak` and `peaktime` is a vocabulary that
            // cannot be queried.
            let known = self.known_tags();
            let unused: Vec<&String> =
                known.iter().filter(|tag| !track.tags.contains(tag)).take(8).collect();
            if !unused.is_empty() {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 3.0;
                    for tag in unused {
                        if ui
                            .add(
                                egui::Button::new(
                                    RichText::new(tag).font(theme::mono(10.0)).color(theme::dim()),
                                )
                                .fill(theme::booth()),
                            )
                            .clicked()
                        {
                            self.pending.push(Pending::AddTag(track.id, tag.clone()));
                        }
                    }
                });
            }

            ui.add_space(14.0);
            pane_label(ui, "Harmonic neighbours");
            let neighbours: Vec<(u32, String)> = self
                .library
                .tracks
                .iter()
                .filter(|other| {
                    other.id != track.id
                        && !track.key.is_empty()
                        && query::mixes_with(&track.key, &other.key)
                        && (other.bpm - track.bpm).abs() < 6.0
                })
                .take(5)
                .map(|other| (other.id, format!("{} — {}", other.artist, other.title)))
                .collect();
            if neighbours.is_empty() {
                ui.label(
                    RichText::new("none in key and in range")
                        .color(theme::dim())
                        .size(theme::SMALL),
                );
            }
            for (id, name) in neighbours {
                if ui
                    .add(
                        egui::Label::new(
                            RichText::new(name).color(theme::text()).size(theme::SMALL),
                        )
                        .sense(egui::Sense::click()),
                    )
                    .clicked()
                {
                    self.pending.push(Pending::Select(id));
                }
            }
        });

        self.editing = Some(edit);
    }

    /// Carry out what the panels asked for.
    ///
    /// Run after every panel has drawn, so that a click and the state it
    /// changes never appear in the same frame — a list that reordered itself
    /// under a half-drawn row would be worse than one that catches up next
    /// frame.
    fn apply_pending(&mut self, ctx: &egui::Context) {
        // The panels' own changes first: the sidebar walks the collection to
        // draw its tree, so it cannot save or rebuild while it is doing so.
        self.flush_requests();

        let pending = std::mem::take(&mut self.pending);
        if pending.is_empty() {
            return;
        }
        // Two different consequences: `touched` means the collection changed
        // and has to be written; `relist` means only the order or the view did.
        // Sorting a list is not a reason to rewrite a library.
        let mut touched = false;
        let mut relist = false;

        for action in pending {
            match action {
                Pending::Select(id) => {
                    if Some(id) != self.selected {
                        // Through the same path as a click, so that what a
                        // selection does to the deck is decided in one place.
                        self.select(id);
                        // Selecting from the neighbours list can leave the
                        // query showing something the track is not in; the
                        // browser widens rather than the selection being lost.
                        if !self.rows.iter().any(|row| row.track.id == id) {
                            self.view = View::All;
                            self.text.clear();
                        }
                        relist = true;
                    }
                }
                Pending::Forget(id) => {
                    self.library.remove(id);
                    touched = true;
                }
                Pending::RemoveFromPlaylist(id, name) => {
                    if let Some(playlist) =
                        self.library.playlists.iter_mut().find(|p| p.name == name)
                    {
                        let before = playlist.tracks.len();
                        playlist.tracks.retain(|t| *t != id);
                        if playlist.tracks.len() != before {
                            self.note(
                                format!("removed from \u{201c}{name}\u{201d}"),
                                theme::text(),
                            );
                            touched = true;
                        }
                    }
                }
                Pending::UseDrive(at) => {
                    if at < self.library.drives.len() && at != self.drive {
                        self.drive = at;
                        self.replan();
                    }
                }
                Pending::ForgetDrive(at) => {
                    if at >= self.library.drives.len() {
                        continue;
                    }
                    let gone = self.library.drives.remove(at);
                    crate::info!("forgot {} \u{2014} nothing on it was touched", gone.label);
                    // Whatever is left, and the end of the list rather than a
                    // hole in it: an index kept past a removal points at the
                    // wrong drive, which is the worst kind of right-looking.
                    self.drive = self.drive.min(self.library.drives.len().saturating_sub(1));
                    self.replan();
                    touched = true;
                }
                Pending::ToEmulator(at) => {
                    let Some(drive) = self.library.drives.get(at).cloned() else { continue };
                    if !drive.is_image {
                        crate::warn!(
                            "{} is a stick, not an image, and the emulator takes an image",
                            drive.label
                        );
                        continue;
                    }
                    if !drive.path.is_file() {
                        crate::warn!("{} is not there to send", drive.path.display());
                        continue;
                    }
                    // Done here rather than on a worker because it is a
                    // loopback socket: the emulator not running is a refused
                    // connection, which comes back at once, and the wait only
                    // bites on a host that swallows packets.
                    let port = self.config.emulator_port;
                    match booth_cli::emulator::send(("127.0.0.1", port), &drive.path) {
                        booth_cli::emulator::Sent::Attached => {
                            crate::info!("{} is in the emulator's USB slot", drive.path.display())
                        }
                        booth_cli::emulator::Sent::Revealed => crate::info!(
                            "nothing is listening on port {port}, so {} is showing in a file \
                             manager \u{2014} attach it with USB \u{2192} Attach virtual image",
                            drive.path.display()
                        ),
                        booth_cli::emulator::Sent::Written(why) => crate::warn!(
                            "{} did not reach the emulator: {why}",
                            drive.path.display()
                        ),
                    }
                }
                Pending::DrivePlaylist { name, on } => {
                    let Some(drive) = self.library.drives.get_mut(self.drive) else { continue };
                    // Migrate off the single-playlist field the first time a
                    // drive is edited, so the two cannot disagree afterwards.
                    if drive.playlists.is_empty() && !drive.playlist.is_empty() {
                        drive.playlists = vec![std::mem::take(&mut drive.playlist)];
                    }
                    drive.playlist.clear();
                    match on {
                        true if !drive.playlists.contains(&name) => drive.playlists.push(name),
                        true => {}
                        false => drive.playlists.retain(|n| *n != name),
                    }
                    touched = true;
                    self.replan();
                }
                Pending::AddToPlaylist(ids, name) => self.add_tracks_to_playlist(&ids, &name),
                Pending::TrashDuplicates { going, picked, tags } => {
                    self.trash_duplicates(&going, &picked, &tags);
                    touched = true;
                }
                Pending::NamePlaylistFor(ids) => {
                    let mut naming = Naming::new_playlist();
                    naming.holding = ids;
                    self.naming = Some(naming);
                }
                Pending::Adopt(id) => self.adopt(&[id]),
                Pending::FitWave => self.zoom = wave::Zoom::default(),
                Pending::Convert(ids) => self.convert_tracks(&ids),
                Pending::Analyze(id) => self.analyze_tracks(&[id]),
                Pending::Identify(id) => self.identify_tracks(&[id], true),
                Pending::Separate(id) => self.separate_tracks(&[id]),
                Pending::SeparateMany { tracks, for_cues } => self.separate_for(&tracks, for_cues),
                Pending::TakeFoundWords(id, found) => {
                    touched |= self.take_found_words(id, &found);
                }
                Pending::SaveWords(id) => {
                    touched |= self.save_edited_words(id);
                }
                // The selection where there is one, the way every other row
                // action behaves: a menu opened on one of six marked rows acts
                // on the six.
                Pending::FindWords(id) => {
                    touched |= self.find_words(id);
                }
                Pending::FixGrid { id, what, at } => {
                    touched |= self.fix_grid(id, what, at);
                }
                Pending::Regrid(id) => self.regrid(id),
                Pending::MarkBeat { id, at_ms } => touched |= self.mark_beat(id, at_ms),
                Pending::ClearMarks(id) => touched |= self.clear_marks(id),
                Pending::FitGrid(id) => touched |= self.fit_grid(id),
                Pending::Flatten(id) => touched |= self.flatten_grid(id),
                Pending::LookUpWords(id) => {
                    let wanted = match self.marked.len() > 1 && self.marked.contains(&id) {
                        true => self.marked.iter().copied().collect(),
                        false => vec![id],
                    };
                    self.look_up_words(&wanted);
                }
                Pending::EditWords(id) => {
                    let text = self
                        .library
                        .get(id)
                        .map(|track| {
                            track
                                .lyrics
                                .iter()
                                .map(|line| line.text.as_str())
                                .collect::<Vec<_>>()
                                .join("\n")
                        })
                        .unwrap_or_default();
                    self.editing_words = Some(EditingWords { id, text });
                }
                Pending::AutoCue(id) => touched |= self.auto_cue_tracks(&[id], false),
                Pending::CopyPath(id) => {
                    // A companion's path is its stems, not its parent's file:
                    // copying the mix's path off an acapella row would be a
                    // wrong answer rather than a missing one.
                    if let Some(track) = self.library.row(id) {
                        let path = track
                            .sources()
                            .iter()
                            .map(|p| p.display().to_string())
                            .collect::<Vec<_>>()
                            .join("\n");
                        ctx.copy_text(path.clone());
                        self.note(format!("copied {path}"), theme::text());
                    }
                }
                Pending::AddTag(id, tag) => {
                    // Matched case-insensitively so that `Peak` joins `peak`
                    // rather than starting a second tag that queries miss.
                    let existing =
                        self.known_tags().into_iter().find(|k| k.eq_ignore_ascii_case(&tag));
                    let tag = existing.unwrap_or(tag);
                    if let Some(track) = self.library.get_mut(id) {
                        if !track.tags.iter().any(|t| t.eq_ignore_ascii_case(&tag)) {
                            track.tags.push(tag);
                            track.tags.sort();
                        }
                    }
                    self.tag_entry.clear();
                    touched = true;
                }
                Pending::RemoveTag(id, tag) => {
                    if let Some(track) = self.library.get_mut(id) {
                        track.tags.retain(|t| *t != tag);
                    }
                    touched = true;
                }
                Pending::CommitEdit => {
                    if let Some(edit) = self.editing.take() {
                        self.apply_edit(edit);
                    }
                }
                Pending::CancelEdit => self.editing = None,
                Pending::Resort => relist = true,
                Pending::WriteTags(id) => {
                    self.write_tags(id, booth_cli::tag::OnExisting::Overwrite)
                }
                Pending::EditPhrase { id, edit } => {
                    touched |= self.edit_phrase(id, edit);
                }
                Pending::PlaceCue { id, letter, time_ms } => {
                    self.place_cue(id, letter, time_ms);
                    touched = true;
                }
                Pending::MoveCue { id, at, time_ms } => {
                    self.move_cue(id, at, time_ms);
                    touched = true;
                }
                Pending::CountDifferently => {
                    self.config.counting = self.config.counting.flipped();
                    if let Err(e) = self.config.save(&self.config_path) {
                        crate::warn!("could not save the settings: {e:#}");
                    }
                }
                Pending::PaintAs(mode) => {
                    self.config.paint = mode;
                    if let Err(e) = self.config.save(&self.config_path) {
                        crate::warn!("could not save the settings: {e:#}");
                    }
                    crate::debug!("waveform coloured by {}", mode.label());
                }
                Pending::AnswerMatch { id, answer } => {
                    if let Some(at) = self.questions.iter().position(|q| q.id == id) {
                        let question = self.questions.remove(at);
                        match answer {
                            crate::identify::Answer::Fingerprint => {
                                crate::info!(
                                    "#{id} took the fingerprint's answer: {}",
                                    question.candidate.describe()
                                );
                                self.apply_match(id, &question.candidate);
                            }
                            crate::identify::Answer::Path => {
                                if let Some(from_path) = &question.from_path {
                                    crate::info!(
                                        "#{id} took the path's answer: {}",
                                        from_path.describe()
                                    );
                                    self.apply_guess(id, from_path);
                                }
                                // Answered, so the fingerprint does not come
                                // back with the same question.
                                if let Some(track) = self.library.get_mut(id) {
                                    track.from_tags = true;
                                }
                            }
                            crate::identify::Answer::Mine => {
                                crate::info!("#{id} kept its own name over the fingerprint");
                                // Marked as answered, so the same question is
                                // not asked again on the next pass.
                                if let Some(track) = self.library.get_mut(id) {
                                    track.from_tags = true;
                                }
                            }
                        }
                        touched = true;
                    }
                }
                Pending::AnswerNaming { id, write } => {
                    if let Some(at) = self.unlike.iter().position(|held| held.id == id) {
                        let held = self.unlike.remove(at);
                        match write {
                            true => {
                                crate::info!(
                                    "{}: tagging it {} — {} anyway",
                                    held.path.file_name().unwrap_or_default().to_string_lossy(),
                                    held.artist,
                                    held.title
                                );
                                // Straight past the check that held it up:
                                // it has been answered, and asking again on
                                // the way out would be the same question
                                // forever.
                                self.to_retag.push(held);
                                self.flush_retags();
                            }
                            false => crate::info!(
                                "{}: left as it is",
                                held.path.file_name().unwrap_or_default().to_string_lossy()
                            ),
                        }
                        touched = true;
                    }
                }
                Pending::ChooseName { id, from_path } => {
                    if let Some(checked) = &mut self.checked {
                        checked.naming.insert(id, from_path);
                    }
                }
                Pending::TogglePlayback(id) => self.toggle_playback(id),
                Pending::SeekDeck { id, time_ms } => {
                    if let Some(player) = &self.player {
                        if player.loaded() == Some(id) {
                            player.seek_secs(time_ms as f64 / 1000.0);
                        }
                    }
                }
                Pending::RemoveCue { id, which } => {
                    if let Some(track) = self.library.get_mut(id) {
                        if let Some(at) = which.find(&track.cues) {
                            track.cues.remove(at);
                        }
                    }
                    self.prep_changed(id);
                    touched = true;
                }
                Pending::RenameCue { id, which, label } => {
                    if let Some(track) = self.library.get_mut(id) {
                        if let Some(cue) =
                            which.find(&track.cues).and_then(|at| track.cues.get_mut(at))
                        {
                            cue.label = label;
                        }
                    }
                    self.prep_changed(id);
                    touched = true;
                }
                Pending::RecolorCue { id, which, color } => {
                    if let Some(track) = self.library.get_mut(id) {
                        if let Some(cue) =
                            which.find(&track.cues).and_then(|at| track.cues.get_mut(at))
                        {
                            cue.color = color;
                        }
                    }
                    self.prep_changed(id);
                    touched = true;
                }
                Pending::RecastCue { id, which, into } => {
                    touched |= self.recast_cue(id, which, into)
                }
            }
        }

        if touched {
            self.save();
        }
        if touched || relist {
            self.rebuild();
        }
        // And again for anything the queue itself asked for, in this pass
        // rather than the next one. Only flushing beforehand meant a request
        // made while the queue ran waited for another frame — and egui does
        // not paint frames nobody asked for, so "another frame" could be
        // whenever the pointer next moved. A track dropped on the playlist
        // being looked at went into the collection, was counted in the
        // sidebar, and did not appear in the list until something else
        // happened: the count said three and the list showed two.
        self.flush_requests();
    }

    /// Do what the panels asked for while they were drawing.
    fn flush_requests(&mut self) {
        if std::mem::take(&mut self.pending_rebuild) {
            self.rebuild();
            self.replan();
        }
        if std::mem::take(&mut self.pending_save) {
            self.save();
            self.replan();
        }
    }

    // -- identification ----------------------------------------------------

    /// Ask AcoustID and MusicBrainz what these tracks are.
    ///
    /// `announce` separates the two ways this is reached. Asked for directly,
    /// silence would look like the button did nothing, so a missing key or an
    /// empty selection is said out loud. Riding along behind an analysis, the
    /// same two lines are noise about something the user did not ask for —
    /// they still reach the log, which is where an unexplained absence of
    /// names should be answered.
    fn identify_tracks(&mut self, ids: &[u32], announce: bool) {
        if ids.is_empty() {
            if announce {
                self.note("nothing to identify", theme::dim());
            }
            return;
        }
        let Some(key) = self.config.key() else {
            crate::warn!("no AcoustID key: {} left unidentified", plural(ids.len(), "track"));
            if announce {
                self.note(
                    "no AcoustID key — put one in Settings, or set ACOUSTID_API_KEY",
                    theme::amber(),
                );
            }
            return;
        };
        let waiting = self.files_for(ids);
        if waiting.is_empty() {
            if announce {
                self.note("nothing to identify", theme::dim());
            }
            return;
        }
        crate::info!("identifying {}", plural(waiting.len(), "track"));
        let ids: Vec<u32> = waiting.iter().map(|(id, _)| *id).collect();
        self.ensure_local(&ids);
        self.start(Job::Identify { tracks: waiting, key });
    }

    /// Fingerprint whatever showing is still unidentified.
    /// `again` asks about tracks that have been looked up before, which is
    /// what to do when a lookup got one wrong or the database has moved on.
    fn identify_showing(&mut self, again: bool) {
        let waiting = self.acting_on(|track| again || !track.identified);
        if waiting.is_empty() {
            self.note("nothing showing needs identifying", theme::dim());
            return;
        }
        self.identify_tracks(&waiting, true);
    }

    /// Decide what to do with one match, and do it.
    ///
    /// Two sources of evidence, not one: the fingerprint, and what the file's
    /// own path says it is. They usually agree, and the interesting case is
    /// when they do not — a fingerprint is about the audio and a path is about
    /// what somebody filed it as, so a confident disagreement between them is
    /// a question rather than something to settle by rule.
    fn consider(&mut self, id: u32, found: crate::identify::Match) {
        let Some(track) = self.library.get(id) else { return };
        let from_path = crate::guess::from_path(&track.path);
        let decision = crate::identify::decide(track, &found, self.config.autotag_score);
        let clash = crate::identify::conflicts(&found, &from_path);
        crate::debug!(
            "#{id} {} at {:.0}%: {decision:?}; the path says {}{}",
            found.describe(),
            found.score * 100.0,
            from_path.describe(),
            match clash {
                true => ", which disagrees",
                false => "",
            }
        );

        // A disagreement with a strong path is asked about however confident
        // the fingerprint is. Applying it silently is how a track ends up
        // filed under a remix nobody has of a record they do have.
        let decision = match clash {
            true => crate::identify::Decision::Ask,
            false => decision,
        };

        match decision {
            crate::identify::Decision::Apply => self.apply_match(id, &found),
            crate::identify::Decision::Ask => {
                let question = crate::identify::Question {
                    id,
                    current: match crate::identify::source_of(track) {
                        "nothing" => "not named".to_string(),
                        _ => format!("{} — {}", track.artist, track.title),
                    },
                    source: crate::identify::source_of(track),
                    candidate: found,
                    from_path: clash.then_some(from_path),
                };
                if !self.questions.iter().any(|q| q.id == id) {
                    self.questions.push(question);
                }
            }
            crate::identify::Decision::Reject => {}
        }
    }

    /// What to do about a track no fingerprint could name.
    ///
    /// The path is then the only evidence there is. A record that is not in
    /// AcoustID — a white label, a promo, an edit, most of a DJ's crate — is
    /// still filed under somebody's name in somebody's folder, and that beats
    /// leaving it unnamed.
    fn consider_path(&mut self, id: u32) {
        let Some(track) = self.library.get(id) else { return };
        let from_path = crate::guess::from_path(&track.path);
        match crate::identify::decide_from_path(track, &from_path) {
            crate::identify::Decision::Apply => {
                crate::info!(
                    "#{id} not in AcoustID; taking {} from the path",
                    from_path.describe()
                );
                self.apply_guess(id, &from_path);
            }
            other => crate::debug!(
                "#{id} not in AcoustID; the path says {} ({other:?})",
                from_path.describe()
            ),
        }
    }

    /// Write what a path said into a track's record.
    fn apply_guess(&mut self, id: u32, from_path: &crate::guess::Guess) {
        let Some(track) = self.library.get_mut(id) else { return };
        if !from_path.artist.trim().is_empty() {
            track.artist = from_path.artist.clone();
        }
        if !from_path.title.trim().is_empty() {
            track.title = from_path.title.clone();
        }
        if track.album.trim().is_empty() && !from_path.album.trim().is_empty() {
            track.album = from_path.album.clone();
        }
        // Not `from_tags`: this came off a path, and a later fingerprint is
        // still allowed to correct it without asking.
        self.rebuild();
    }

    /// Write a match into a track's record.
    ///
    /// Only ever fills or replaces the names it has; nothing here touches the
    /// file, which is what the tag write-back is for and is separately asked
    /// for.
    fn apply_match(&mut self, id: u32, found: &crate::identify::Match) {
        let Some(track) = self.library.get_mut(id) else { return };
        if !found.artist.trim().is_empty() {
            track.artist = found.artist.trim().to_string();
        }
        if !found.title.trim().is_empty() {
            track.title = found.title.trim().to_string();
        }
        if !found.album.trim().is_empty() {
            track.album = found.album.trim().to_string();
        }
        if let Some(year) = found.year {
            track.year = Some(year);
        }
        // Its names are now an answer rather than a guess, so a later
        // fingerprint will ask before overriding them — and so a lyrics
        // server asked under them is being asked about this record.
        track.from_tags = true;
        track.identified_surely = Some(found.score);
        crate::info!("#{id} named from its fingerprint: {}", found.describe());

        if let Some(on_existing) = self.config.write_tags.on_existing() {
            self.write_tags(id, on_existing);
        }
    }

    /// Every tag in use anywhere, in order of how often.
    fn known_tags(&self) -> Vec<String> {
        let mut counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
        for track in &self.library.tracks {
            for tag in &track.tags {
                *counts.entry(tag.as_str()).or_default() += 1;
            }
        }
        let mut tags: Vec<(&str, usize)> = counts.into_iter().collect();
        tags.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        tags.into_iter().map(|(tag, _)| tag.to_string()).collect()
    }

    /// Put what was typed into the collection.
    fn apply_edit(&mut self, edit: Edit) {
        let Some(track) = self.library.get_mut(edit.id) else { return };
        if !edit.differs_from(track) {
            return;
        }
        track.artist = edit.names.artist.trim().to_string();
        track.title = edit.names.title.trim().to_string();
        track.album = edit.names.album.trim().to_string();
        track.year = edit.names.year.trim().parse().ok();

        if let Some(on_existing) = self.config.write_tags.on_existing() {
            self.write_tags(edit.id, on_existing);
        }
        self.save();
        self.rebuild();
    }

    /// Put a cue at a time, moving one that is already there.
    ///
    /// Snapped to the nearest beat, because a cue that is four milliseconds off
    /// the grid is a cue that stutters when it is pressed. A memory cue —
    /// letter zero — is snapped to the bar instead: it marks where a track
    /// starts, and starting one mid-bar is a different mistake.
    /// Move, split, join or rename one of a track's phrase sections.
    ///
    /// A boundary is snapped to the bar on the way in, like the memory cue and
    /// for the same reason: a section that starts three beats into a bar is a
    /// section in the wrong place, however carefully it was dragged. The grid
    /// is the window's to know, so the snapping happens here rather than in
    /// the strip that drew the drag or the model that applies it.
    fn edit_phrase(&mut self, id: u32, edit: crate::library::PhraseEdit) -> bool {
        use crate::library::PhraseEdit;
        let Some(track) = self.library.get(id) else { return false };
        let beats = beat_times(track);
        let snapped = match edit {
            PhraseEdit::Move { at, time_ms } => {
                PhraseEdit::Move { at, time_ms: snap_to(&beats, time_ms, 4) }
            }
            PhraseEdit::Split { at, time_ms } => {
                PhraseEdit::Split { at, time_ms: snap_to(&beats, time_ms, 4) }
            }
            other => other,
        };

        // Where the boundary is now, so the cues that mark it can be taken
        // with it. Read before the edit, because afterwards it is gone.
        let was = match &snapped {
            PhraseEdit::Move { at, .. } => track.phrases.get(*at).map(|p| p.start_ms),
            _ => None,
        };
        // A beat, or a quarter second on a track with no tempo to ask.
        let beat_ms = if track.bpm > 0.0 { 60_000.0 / track.bpm } else { 250.0 } as u32;

        let Some(track) = self.library.get_mut(id) else { return false };
        let changed = crate::library::edit_phrases(&mut track.phrases, &snapped);
        if changed {
            if let (Some(from), PhraseEdit::Move { at, .. }) = (was, &snapped) {
                if let Some(to) = track.phrases.get(*at).map(|p| p.start_ms) {
                    let moved = drag_cues(&mut track.cues, from, to, beat_ms);
                    if moved > 0 {
                        crate::debug!("took {} with the boundary", plural(moved, "cue"));
                    }
                }
            }
            self.prep_changed(id);
        }
        changed
    }

    /// Note that a track's prep changed here, and when.
    ///
    /// Called from every place that moves a cue, a boundary or a grid, because
    /// a CDJ-3000X can move the same things on the drive and the two have to be
    /// comparable. Deliberately not called for a tag, a rating or a play count:
    /// a player has no opinion about those, so there is nothing to disagree
    /// about and nothing to ask.
    fn prep_changed(&mut self, id: u32) {
        if let Some(track) = self.library.get_mut(id) {
            track.edited = Some(crate::library::now());
        }
    }

    /// Move the cue at this place in the track's list to a new moment.
    ///
    /// By place rather than by letter, because a letter stopped naming one cue
    /// when a track could have more than one memory cue: they all carry zero,
    /// so finding "the cue with letter 0" found whichever came first — the one
    /// the grid is anchored to — however far down the panel the one being
    /// dragged was. The index is the one the waveform drew from, and the list
    /// is not touched between the frame and this.
    fn move_cue(&mut self, id: u32, at: usize, time_ms: u32) {
        let Some(track) = self.library.get(id) else { return };
        let Some(letter) = track.cues.get(at).map(|cue| cue.letter) else { return };
        let beats = beat_times(track);
        // A memory cue takes the coarser grid: it is what the bars are counted
        // from, so one placed off the bar line re-phrases the whole track.
        let snapped =
            snap_to(&beats, time_ms, if letter == 0 { crate::library::BEATS_PER_BAR } else { 1 });

        let Some(track) = self.library.get_mut(id) else { return };
        let Some(cue) = track.cues.get_mut(at) else { return };
        cue.time_ms = snapped;
        track.cues.sort_by_key(|cue| (cue.letter, cue.time_ms));
        self.prep_changed(id);
    }

    /// Turn a hot cue into a memory cue, or a memory cue into a hot one.
    ///
    /// The colour follows the letter, because that is what lights the button
    /// on a player and a cue moved to B that stays the colour of A is a row of
    /// buttons that disagrees with itself. A name somebody typed does not
    /// follow it anywhere — that is theirs.
    ///
    /// Taking a hot cue's letter that is already in use is refused rather than
    /// resolved: silently bumping the cue already there would lose whichever
    /// of the two somebody cared about, and there is no way to ask which.
    fn recast_cue(&mut self, id: u32, which: WhichCue, into: u8) -> bool {
        if which.letter == into {
            return false;
        }
        let Some(track) = self.library.get_mut(id) else { return false };
        if into != 0 && track.cues.iter().any(|cue| cue.letter == into) {
            let taken = char::from(b'A' + (into - 1).min(7));
            self.note(format!("hot cue {taken} is already in use"), theme::amber());
            return false;
        }
        let Some(cue) = which.find(&track.cues).and_then(|at| track.cues.get_mut(at)) else {
            return false;
        };
        cue.letter = into;
        if into != 0 {
            cue.color = crate::job::cue_color(into);
        }
        track.cues.sort_by_key(|cue| (cue.letter, cue.time_ms));
        self.prep_changed(id);
        true
    }

    fn place_cue(&mut self, id: u32, letter: u8, time_ms: u32) {
        let Some(track) = self.library.get(id) else { return };
        let beats = beat_times(track);
        let snapped = snap_to(&beats, time_ms, if letter == 0 { 4 } else { 1 });
        let color =
            theme::CUE_COLORS[(letter.saturating_sub(1) as usize) % theme::CUE_COLORS.len()];

        let Some(track) = self.library.get_mut(id) else { return };
        match track.cues.iter_mut().find(|cue| cue.letter == letter) {
            Some(cue) => cue.time_ms = snapped,
            None => track.cues.push(crate::library::CueMark {
                letter,
                time_ms: snapped,
                label: String::new(),
                color: [color.r(), color.g(), color.b()],
            }),
        }
        track.cues.sort_by_key(|cue| (cue.letter, cue.time_ms));
        self.prep_changed(id);
    }

    /// Write one track's names into the file's own tags.
    /// Put the collection's names into the file's own tag block.
    ///
    /// `on_existing` is the whole difference between the two ways this is
    /// reached. The button in the inspector is somebody saying "these ones,
    /// now", so it overwrites; the write-back that rides behind a fingerprint
    /// lookup fills in blanks and leaves anything already there alone, because
    /// nobody asked it to have an opinion about a value they typed.
    fn write_tags(&mut self, id: u32, on_existing: booth_cli::tag::OnExisting) {
        let Some(track) = self.library.get(id) else { return };
        // Asked of the writer rather than answered again here. This was a
        // second list of formats, and it had already fallen behind the first:
        // it still said FLAC and MP3 after the writer learned MP4, so an
        // identified `.m4a` was told it had nowhere to put a name that the
        // code underneath would have written.
        match booth_cli::tag::tag_kind(&track.path) {
            Some(booth_cli::tag::TagKind::None) => {
                self.note(format!("a .{} has nowhere to keep tags", track.format), theme::amber());
                return;
            }
            None => {
                self.note(
                    format!("a .{} is not a file whose tags can be written", track.format),
                    theme::amber(),
                );
                return;
            }
            Some(_) => {}
        }
        crate::debug!(
            "tagging {} ({on_existing:?})",
            track.path.file_name().unwrap_or_default().to_string_lossy()
        );
        // Collected rather than started: identifying a crate produces a match
        // per track, and one job per file would be forty "started tagging,
        // finished tagging" lines for what is one errand.
        self.to_retag.retain(|waiting| waiting.id != track.id);
        self.unlike.retain(|waiting| waiting.id != track.id);
        let write = Retag {
            id: track.id,
            path: track.path.clone(),
            artist: track.artist.clone(),
            title: track.title.clone(),
            album: track.album.clone(),
            date: track.year.map(|year| year.to_string()),
            on_existing,
        };

        // A file whose own name has nothing in common with the names about to
        // go into it is the shape of a mistake worth stopping for: a
        // fingerprint that found the wrong record, or the wrong row acted on.
        // Tagging is the one thing here that writes to somebody's files, and
        // the wrong answer written into forty of them is a bad afternoon.
        let file_name = track.path.file_name().unwrap_or_default().to_string_lossy().into_owned();
        if crate::guess::resembles(&file_name, &write.artist, &write.title) {
            self.to_retag.push(write);
            return;
        }
        crate::warn!(
            "{file_name} would be tagged {} — {}, which its name says nothing about; asking first",
            write.artist,
            write.title
        );
        self.unlike.push(write);
    }

    /// Start the tagging that has piled up, if any has.
    fn flush_retags(&mut self) {
        if self.to_retag.is_empty() {
            return;
        }
        let waiting = std::mem::take(&mut self.to_retag);
        crate::info!("tagging {}", plural(waiting.len(), "file"));
        self.start(Job::Retag(waiting));
    }

    fn dock(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            if ui
                .add(
                    egui::Button::new(
                        RichText::new(theme::label_text("Settings"))
                            .size(theme::LABEL)
                            .color(theme::dim())
                            .strong(),
                    )
                    .fill(theme::booth()),
                )
                .on_hover_text("Where music is kept, and what to do about music from elsewhere")
                .clicked()
            {
                self.settings = true;
            }
            self.log_button(ui);
            ui.separator();
            ui.label(
                RichText::new(theme::label_text("Drives"))
                    .size(theme::LABEL)
                    .color(theme::dim())
                    .strong(),
            );

            match self.library.drives.get(self.drive).cloned() {
                None => {
                    ui.label(RichText::new("none set up").color(theme::dim()));
                    if ui
                        .button("Add a drive\u{2026}")
                        .on_hover_text("Point at a mounted USB stick. It is written the way a player expects to find it.")
                        .clicked()
                    {
                        self.want_pick = Some(Picking::Drive);
                    }
                    if ui
                        .button("Or an image\u{2026}")
                        .on_hover_text("Write to a FAT32 disk image file instead, which can be handed to the emulator.")
                        .clicked()
                    {
                        self.want_pick = Some(Picking::Image);
                    }
                }
                Some(drive) => {
                    // Whether this one is actually in a socket. A remembered
                    // drive is a place and a history, not a stick, and the two
                    // read identically until something asks — which is how a
                    // drive that went home in a bag stays on screen as the
                    // thing sync is about to write to.
                    let here = plugged_in(&drive);
                    let mark = if drive.is_image { "▢" } else { "▣" };
                    let carries = drive.playlist_names();
                    let what = match carries.len() {
                        0 => "no playlists".to_string(),
                        1 => carries[0].clone(),
                        n => format!("{n} playlists"),
                    };
                    ui.label(
                        RichText::new(format!(
                            "{mark} {} — {what} · {}",
                            drive.label,
                            plural(drive.written.len(), "track")
                        ))
                        .color(match here {
                            true => theme::text(),
                            false => theme::dim(),
                        }),
                    )
                    .on_hover_text(match here {
                        true => drive.path.display().to_string(),
                        false => format!("{} is not there", drive.path.display()),
                    });
                    if !here {
                        ui.label(RichText::new("not plugged in").color(theme::amber()));
                    }
                    // Which playlists go on the stick is the decision the dock
                    // exists for, so it is a menu here rather than a setting
                    // somewhere else.
                    ui.menu_button("playlists\u{2026}", |ui| {
                        ui.set_min_width(190.0);
                        if self.library.playlists.is_empty() {
                            ui.label(RichText::new("no playlists yet").color(theme::dim()));
                        }
                        for (folder, lists) in self.library.playlist_tree() {
                            if !folder.is_empty() {
                                ui.label(
                                    RichText::new(&folder).color(theme::dim()).size(theme::SMALL),
                                );
                            }
                            for playlist in lists {
                                let name = playlist.name.clone();
                                let mut on = carries.contains(&name);
                                let about = format!(
                                    "Carry {name} on this drive \u{2014} {}.",
                                    plural(playlist.tracks.len(), "track")
                                );
                                if ui
                                    .checkbox(
                                        &mut on,
                                        format!("{name}  ({})", playlist.tracks.len()),
                                    )
                                    .on_hover_text(about)
                                    .changed()
                                {
                                    self.pending.push(Pending::DrivePlaylist { name, on });
                                }
                            }
                        }
                    })
.response
.on_hover_text("Choose which playlists this drive carries. A track in two of them is one track on the stick.");
                    // Every drive the collection knows, named, with the ones
                    // actually here marked as such — and a way to add another
                    // from the same place. Adding used to be offered only
                    // while the list was empty and switching only while it
                    // held more than one, so the first drive set up was the
                    // last: no way to add a second, no way to reach it, and
                    // nothing saying the one on screen was in a drawer.
                    ui.menu_button("drives\u{2026}", |ui| {
                        ui.set_min_width(240.0);
                        for (at, other) in self.library.drives.iter().enumerate() {
                            let there = plugged_in(other);
                            let mark = if other.is_image { "\u{25a2}" } else { "\u{25a3}" };
                            let state = match there {
                                true => String::new(),
                                false => " · not plugged in".to_string(),
                            };
                            let label = RichText::new(format!("{mark} {}{state}", other.label))
                                .color(match there {
                                    true => theme::text(),
                                    false => theme::dim(),
                                });
                            if ui
                                .radio(at == self.drive, label)
                                .on_hover_text(match there {
                                    true => format!("Work with {}.", other.label),
                                    false => format!(
                                        "Work with {}. It is not plugged in, so the plan is \
                                         worked out and nothing is written until it is.",
                                        other.label
                                    ),
                                })
                                .clicked()
                            {
                                self.pending.push(Pending::UseDrive(at));
                                ui.close();
                            }
                        }
                        ui.separator();
                        if ui
                            .button("Add a drive\u{2026}")
                            .on_hover_text("Point at another mounted USB stick.")
                            .clicked()
                        {
                            self.want_pick = Some(Picking::Drive);
                            ui.close();
                        }
                        if ui
                            .button("Add an image\u{2026}")
                            .on_hover_text("Write to a FAT32 disk image file, which the emulator can be handed.")
                            .clicked()
                        {
                            self.want_pick = Some(Picking::Image);
                            ui.close();
                        }
                        // Only for an image: the emulator's slot takes a file,
                        // and a mounted stick is not one.
                        if drive.is_image
                            && ui
                                .button("Send to the emulator")
                                .on_hover_text(
                                    "Puts this image in a running CDJ-3000 emulator's USB slot",
                                )
                                .clicked()
                        {
                            self.pending.push(Pending::ToEmulator(self.drive));
                            ui.close();
                        }
                        // Forgetting is not unplugging: what goes is this
                        // program's record of what it wrote, and the stick
                        // keeps everything on it.
                        if ui
                            .button("Forget this one")
                            .on_hover_text("Takes it off this list. Nothing on the drive changes.")
                            .clicked()
                        {
                            self.pending.push(Pending::ForgetDrive(self.drive));
                            ui.close();
                        }
                    })
.response
.on_hover_text("Switch between the drives set up here, add another, or hand an image to the emulator.");

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // Open whenever there is a drive and nothing running,
                        // even with nothing to write. What the sheet is for is
                        // deciding whether to write, and "up to date" is a
                        // claim about the drive worth being able to look at —
                        // and to disagree with, since the way to make the next
                        // write a full one is in there. A drive that believed
                        // itself finished was a drive with no way in at all.
                        let empty = self.plan.is_empty();
                        if ui
                            .add_enabled(
                                !self.running(),
                                egui::Button::new(
                                    RichText::new(theme::label_text("Sync"))
                                        .size(11.0)
                                        .color(match empty {
                                            true => theme::text(),
                                            false => theme::booth(),
                                        })
                                        .strong(),
                                )
                                // Quiet when there is nothing to write, so the
                                // button being there is not itself a summons.
                                .fill(match empty {
                                    true => theme::booth_2(),
                                    false => theme::amber(),
                                }),
                            )
                            .on_hover_text(match empty {
                                true => "Look at what this drive is holding, or write it again",
                                false => "What would go on, and whether it can",
                            })
                            .clicked()
                        {
                            self.examine_drive();
                            self.sheet = true;
                        }
                        ui.label(
                            RichText::new(self.plan.delta())
                                .font(theme::mono(11.5))
                                .color(theme::amber()),
                        );
                    });
                }
            }
        });

        self.log_panel(ui);
    }

    /// The button that opens the log's own window, on the dock's top row.
    fn log_button(&mut self, ui: &mut Ui) {
        if ui
            .add(
                egui::Button::new(
                    RichText::new(theme::label_text("Log"))
                        .size(theme::LABEL)
                        .color(theme::dim())
                        .strong(),
                )
                .fill(theme::booth()),
            )
            .on_hover_text("Open the log in its own window")
            .clicked()
        {
            self.log.set_open(true);
        }
    }

    /// As much of the log as the dock has been given room for.
    ///
    /// At the height it opens at that is the last line, which is what "did that
    /// work" needs. Dragging the dock up shows more of the run without leaving
    /// the window; the log's own window is still there for reading a whole run
    /// beside it, or on another screen.
    ///
    /// Newest last and stuck to the bottom, so the line that just appeared is
    /// in the same place whether there is one line showing or twenty.
    fn log_panel(&mut self, ui: &mut Ui) {
        let entries = crate::log::entries(crate::log::Level::Info);
        ui.add_space(4.0);

        if entries.is_empty() {
            ui.label(RichText::new("nothing yet").font(theme::mono(10.5)).color(theme::dim()));
            return;
        }

        egui::ScrollArea::vertical().auto_shrink([false, false]).stick_to_bottom(true).show(
            ui,
            |ui| {
                ui.spacing_mut().item_spacing.y = 1.0;
                for entry in &entries {
                    ui.label(
                        RichText::new(&entry.text)
                            .font(theme::mono(10.5))
                            .color(log_color(entry.level)),
                    );
                }
            },
        );
    }

    /// What the fingerprints found that a person has to decide.
    ///
    /// One row per track: what it says now and where that came from, against
    /// what the audio was identified as and how sure that is. Both are shown in
    /// full, because the whole reason this is a question is that they disagree.
    /// Files whose names say nothing about what is about to be written into
    /// them.
    ///
    /// Not the same question as the match sheet, and deliberately a separate
    /// one: that sheet is about what a track *is*, and answering it changes
    /// only the collection. This one is about writing to somebody's files, and
    /// the answer is spent immediately.
    fn naming_sheet(&mut self, ctx: &egui::Context) {
        let held = self.unlike.clone();
        if held.is_empty() {
            return;
        }
        let mut open = true;

        egui::Window::new(format!("{} to check before tagging", plural(held.len(), "file")))
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_width(760.0)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .max_height(sheet_height(ctx))
            .frame(sheet_frame())
            .show(ctx, |ui| {
                ui.label(
                    RichText::new(
                        "These files are named nothing like the tags about to go into them.                          That is usually a fingerprint that found the wrong record — and                          tagging is the one thing here that writes to your files.",
                    )
                    .color(theme::dim())
                    .size(theme::SMALL),
                );
                ui.add_space(8.0);

                egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                    for write in &held {
                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                ui.label(
                                    RichText::new(format!("{} — {}", write.artist, write.title))
                                        .color(theme::text()),
                                );
                                ui.label(
                                    RichText::new(format!(
                                        "file: {}",
                                        write
                                            .path
                                            .file_name()
                                            .unwrap_or_default()
                                            .to_string_lossy()
                                    ))
                                    .font(theme::mono(10.0))
                                    .color(theme::amber()),
                                );
                            });
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui
                                        .button("Leave it")
                                        .on_hover_text("Keep the name the collection has. Nothing is written to the file.")
                                        .clicked()
                                    {
                                        self.pending.push(Pending::AnswerNaming {
                                            id: write.id,
                                            write: false,
                                        });
                                    }
                                    if ui
                                        .add(
                                            egui::Button::new(
                                                RichText::new("Tag it anyway")
                                                    .color(theme::booth())
                                                    .strong(),
                                            )
                                            .fill(theme::amber()),
                                        )
                                        .clicked()
                                    {
                                        self.pending.push(Pending::AnswerNaming {
                                            id: write.id,
                                            write: true,
                                        });
                                    }
                                },
                            );
                        });
                        ui.separator();
                    }
                });

                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui
                        .button("Tag them all")
                        .on_hover_text("Write the collection's names into every one of these files' tags.")
                        .clicked()
                    {
                        for write in &held {
                            self.pending
                                .push(Pending::AnswerNaming { id: write.id, write: true });
                        }
                    }
                    if ui
                        .button("Leave them all")
                        .on_hover_text("Answer no to every one. Nothing changes, here or on disk.")
                        .clicked()
                    {
                        for write in &held {
                            self.pending
                                .push(Pending::AnswerNaming { id: write.id, write: false });
                        }
                    }
                    ui.label(
                        RichText::new("Leaving one alone changes nothing, here or on disk.")
                            .color(theme::dim())
                            .size(theme::SMALL),
                    );
                });
            });

        // Closing the sheet is not an answer either way, so nothing is written
        // and nothing is lost: the files stay in the queue.
        if !open {
            self.unlike.clear();
            crate::info!("left the files whose names did not match as they are");
        }
    }

    fn questions_sheet(&mut self, ctx: &egui::Context) {
        let questions = self.questions.clone();
        if questions.is_empty() {
            return;
        }
        let threshold = self.config.autotag_score;
        let mut open = true;

        egui::Window::new(format!("{} to check", plural(questions.len(), "match")))
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_width(760.0)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .max_height(sheet_height(ctx))
            .frame(sheet_frame())
            .show(ctx, |ui| {
                ui.label(
                    RichText::new(format!(
                        "Identified by fingerprint. Anything at {:.0}% or above is applied \
                         without asking, unless it disagrees with the file's own tags — those \
                         are somebody's answer already.",
                        threshold * 100.0
                    ))
                    .color(theme::dim())
                    .size(theme::SMALL),
                );
                ui.add_space(8.0);

                egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                    for question in &questions {
                        let confident = question.candidate.score >= threshold;
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(format!("{:.0}%", question.candidate.score * 100.0))
                                    .font(theme::mono(11.0))
                                    .color(if confident { theme::go() } else { theme::amber() }),
                            );
                            ui.vertical(|ui| {
                                ui.label(
                                    RichText::new(question.candidate.describe())
                                        .color(theme::text()),
                                );
                                ui.label(
                                    RichText::new(format!(
                                        "now: {} — from {}",
                                        question.current, question.source
                                    ))
                                    .font(theme::mono(10.0))
                                    .color(theme::dim()),
                                );
                                // Only when the path says something else. Two
                                // answers that both look right is the case a
                                // person is here to settle.
                                if let Some(from_path) = &question.from_path {
                                    ui.label(
                                        RichText::new(format!("path: {}", from_path.describe()))
                                            .font(theme::mono(10.0))
                                            .color(theme::amber()),
                                    );
                                }
                            });
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui
                                        .button("Keep mine")
                                        .on_hover_text("Keep the names the collection already has for this track.")
                                        .clicked()
                                    {
                                        self.pending.push(Pending::AnswerMatch {
                                            id: question.id,
                                            answer: crate::identify::Answer::Mine,
                                        });
                                    }
                                    if question.from_path.is_some()
                                        && ui
                                            .button("Use the path")
                                            .on_hover_text("Take the artist and title read out of the file's own name.")
                                            .clicked()
                                    {
                                        self.pending.push(Pending::AnswerMatch {
                                            id: question.id,
                                            answer: crate::identify::Answer::Path,
                                        });
                                    }
                                    if ui
                                        .add(
                                            egui::Button::new(
                                                RichText::new("Use this")
                                                    .color(theme::booth())
                                                    .strong(),
                                            )
                                            .fill(theme::amber()),
                                        )
                                        .clicked()
                                    {
                                        self.pending.push(Pending::AnswerMatch {
                                            id: question.id,
                                            answer: crate::identify::Answer::Fingerprint,
                                        });
                                    }
                                },
                            );
                        });
                        ui.separator();
                    }
                });

                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui
                        .button("Use all")
                        .on_hover_text("Take the fingerprint's answer for every question here.")
                        .clicked()
                    {
                        for question in &questions {
                            self.pending.push(Pending::AnswerMatch {
                                id: question.id,
                                answer: crate::identify::Answer::Fingerprint,
                            });
                        }
                    }
                    if ui
                        .button("Keep all of mine")
                        .on_hover_text("Leave every one of these tracks named as the collection has it.")
                        .clicked()
                    {
                        for question in &questions {
                            self.pending.push(Pending::AnswerMatch {
                                id: question.id,
                                answer: crate::identify::Answer::Mine,
                            });
                        }
                    }
                    ui.label(
                        RichText::new("Nothing is written to any file by answering these.")
                            .color(theme::dim())
                            .size(theme::SMALL),
                    );
                });
            });

        if !open {
            self.questions.clear();
        }
    }

    /// The same recordings, more than once, and the offer to be rid of them.
    ///
    /// A tick means keep this one. Every group opens with exactly one ticked —
    /// the copy that knows the most about the record — because the point of the
    /// sheet is to end up with one file per recording, and anything else is a
    /// decision somebody has to make rather than a default worth having. The
    /// first copy is only a guess at which that should be, so it is a tick like
    /// any other and can be moved.
    ///
    /// What only the untick copies know is folded into the one kept before they
    /// go, so being rid of them costs nothing: the album name that was only on
    /// the download, the cues placed on it, the playlist it was in. Where two
    /// of them answer the same field differently there is nothing to fold and
    /// somebody has to say which is right; until they do, that copy is left
    /// where it is rather than deleted on a guess.
    ///
    /// Nothing happens until the button at the bottom is pressed. This is the
    /// only thing in the program that deletes somebody's music, and it says how
    /// many files and how many megabytes before it does.
    fn duplicates_sheet(&mut self, ctx: &egui::Context) {
        use crate::library::{Field, Merge, Side};

        let groups = self.library.duplicate_groups(&self.config.library_path);
        let unchecked = self.library.unhashed().len();
        let mut state = self.duplicates.take().unwrap_or_default();

        // A group nobody has looked at yet opens keeping one file: the copy
        // that knows the most. Keyed by the group rather than by the copy, so
        // that moving the tick within a group is not undone on the next frame.
        for group in &groups {
            if !state.seen.insert(group.key()) {
                continue;
            }
            state.keeping.insert(group.keep);
            // The file everything is folded into starts with every tag any
            // copy carries: a tag is somebody having said something about the
            // record, and the default is to keep what everybody said. A file
            // kept alongside it later starts as itself instead — see below.
            let all: std::collections::HashSet<String> = group
                .all()
                .into_iter()
                .filter_map(|id| self.library.get(id))
                .flat_map(|track| track.tags.iter().cloned())
                .collect();
            state.tags.insert(group.keep, all);
        }

        // What each group looks like this frame: its members in order, which of
        // them is being kept, and what folding each of the others in would do.
        // The plans are against the copy actually being kept, so they follow
        // the tick when it moves.
        let shown: Vec<DupeGroup> = groups
            .iter()
            .map(|group| {
                let ids = group.all();
                let into = ids.iter().copied().find(|id| state.keeping.contains(id));
                // Nothing ticked is a state the user can reach by untidying a
                // group; the plans still need something to be about, and the
                // group's own choice is the honest stand-in.
                let into = into.unwrap_or(group.keep);
                let hash = self.library.get(into).map(|t| t.file_hash.clone()).unwrap_or_default();
                let members = ids
                    .iter()
                    .map(|id| {
                        let kept = state.keeping.contains(id);
                        let same = self
                            .library
                            .get(*id)
                            .is_some_and(|t| !t.file_hash.is_empty() && t.file_hash == hash);
                        let plan = match kept {
                            true => Merge::default(),
                            false => self.library.plan_merge(into, *id),
                        };
                        (*id, kept, same && *id != into, plan)
                    })
                    .collect();

                // Every tag anybody in the group wrote, each named once however
                // many copies carry it, in the order the copies come in.
                let mut tags: Vec<String> = Vec::new();
                for id in &ids {
                    let Some(track) = self.library.get(*id) else { continue };
                    for tag in &track.tags {
                        if !tags.contains(tag) {
                            tags.push(tag.clone());
                        }
                    }
                }
                DupeGroup { key: group.key(), into, members, tags }
            })
            .collect();

        let mut open = true;
        let mut delete = false;
        let mut look = false;
        let mut tidy = false;

        egui::Window::new("The same record, more than once")
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_width(760.0)
            .default_height(560.0)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .max_height(sheet_height(ctx))
            .frame(
                egui::Frame::NONE
                    .fill(theme::booth())
                    .stroke(egui::Stroke::new(1.0_f32, theme::rule()))
                    .inner_margin(egui::Margin::same(14)),
            )
            .show(ctx, |ui| {
                // What has not been looked at yet, said before anything else:
                // "nothing is in here twice" means something quite different
                // when half the collection has never been read.
                if unchecked > 0 {
                    ui.label(
                        RichText::new(format!(
                            "{} {} never been looked at. Reading them is the only way to \
                             know whether they are copies.",
                            plural(unchecked, "track"),
                            if unchecked == 1 { "has" } else { "have" },
                        ))
                        .color(theme::amber())
                        .size(theme::SMALL),
                    );
                    ui.add_space(4.0);
                    if ui
                        .add_enabled(
                            !self.running(),
                            egui::Button::new(format!("Look through {unchecked}")),
                        )
                        .on_hover_text(
                            "Reads every byte of each file, but decodes nothing — minutes \
                             for a library, not hours",
                        )
                        .clicked()
                    {
                        look = true;
                    }
                    ui.add_space(8.0);
                }
                if groups.is_empty() {
                    ui.label(
                        RichText::new(match unchecked {
                            0 => "Nothing is in here twice.",
                            _ => "Nothing among the tracks looked at so far is in here twice.",
                        })
                        .color(theme::dim()),
                    );
                    return;
                }

                ui.label(
                    RichText::new(
                        "Grouped by the sound in the file rather than by its name, so the \
                         same rip tagged twice is one record here. Ticked is kept; what the \
                         rest know is folded into it before they go.",
                    )
                    .color(theme::dim())
                    .size(theme::SMALL),
                );
                ui.add_space(8.0);

                // The list scrolls; the footer does not. Its height is taken
                // out of the list's before the list is drawn, so the button
                // that does the deleting cannot be pushed off the bottom of the
                // sheet by a long enough collection.
                let list_height = (ui.available_height() - FOOTER_HEIGHT).max(120.0);
                egui::ScrollArea::vertical()
                    .max_height(list_height)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for (n, group) in shown.iter().enumerate() {
                            // A rule between records rather than only a gap:
                            // every line in a group is a path in the same
                            // typeface, and without one it is not obvious where
                            // one record's copies end and the next begin.
                            if n > 0 {
                                ui.add_space(6.0);
                                ui.separator();
                                ui.add_space(6.0);
                            }
                            if let Some(track) = self.library.get(group.into) {
                                ui.label(
                                    RichText::new(track.display_title())
                                        .color(theme::text())
                                        .size(theme::BODY),
                                );
                            }

                            let kept_here =
                                group.members.iter().filter(|(_, kept, _, _)| *kept).count();
                            let folding_in = group.members.iter().any(|(_, kept, _, _)| !kept);
                            for (id, kept, same, plan) in &group.members {
                                let Some(track) = self.library.get(*id) else { continue };
                                let path = track.path.clone();
                                // Every disagreement answered is a copy that is
                                // no longer waiting on anybody.
                                let answered = plan
                                    .conflicts
                                    .iter()
                                    .all(|c| state.picked.contains_key(&(*id, c.field)));

                                // The checkbox sits to the left of a column
                                // holding everything else, so that a path too
                                // long for the sheet wraps to under itself
                                // rather than back to the margin — which read
                                // as a new entry rather than the rest of one.
                                ui.horizontal_top(|ui| {
                                    ui.add_space(14.0);
                                    // The last tick in a group cannot be
                                    // cleared: a group with nothing kept is an
                                    // offer to delete every copy of a record,
                                    // which is not a thing to make reachable by
                                    // one stray click.
                                    let last = *kept && kept_here == 1;
                                    let mut on = *kept;
                                    if ui
                                        .add_enabled(!last, egui::Checkbox::without_text(&mut on))
                                        .on_hover_text("Keep this one")
                                        .on_disabled_hover_text(
                                            "Something has to stay — tick another first",
                                        )
                                        .changed()
                                    {
                                        match on {
                                            true => state.keeping.insert(*id),
                                            false => state.keeping.remove(id),
                                        };
                                    }
                                    ui.vertical(|ui| {
                                        ui.horizontal_wrapped(|ui| {
                                            path_label(
                                                ui,
                                                &path,
                                                match kept {
                                                    true => theme::go(),
                                                    false => theme::dim(),
                                                },
                                            );
                                            let (note, color) = match (kept, same, answered) {
                                                (true, _, _) => ("keep".to_string(), theme::go()),
                                                (false, true, _) => {
                                                    ("identical".to_string(), theme::dim())
                                                }
                                                (false, false, false) => (
                                                    format!(
                                                        "disagrees about {}",
                                                        plan.conflicts
                                                            .iter()
                                                            .map(|c| c.field.name())
                                                            .collect::<Vec<_>>()
                                                            .join(", ")
                                                    ),
                                                    theme::alert(),
                                                ),
                                                (false, false, true) => {
                                                    (plan.summary(), theme::amber())
                                                }
                                            };
                                            ui.label(
                                                RichText::new(note).size(theme::SMALL).color(color),
                                            );
                                        });

                                        // What this file itself says, so that
                                        // a tag can be traced to the copy it
                                        // came from rather than appearing in a
                                        // pooled list belonging to nobody.
                                        if !track.tags.is_empty() {
                                            ui.horizontal_wrapped(|ui| {
                                                ui.add_space(10.0);
                                                ui.label(
                                                    RichText::new(format!(
                                                        "tagged  {}",
                                                        track.tags.join("  ")
                                                    ))
                                                    .font(theme::mono(10.0))
                                                    .color(theme::dim()),
                                                );
                                            });
                                        }

                                        // Under each file being kept, and only
                                        // there: every tag in the group, to be
                                        // picked over, and what is ticked is
                                        // what that file ends up with. The
                                        // files going have nothing to choose —
                                        // what they know is folded in and then
                                        // they are gone — so they only say what
                                        // they are tagged, above.
                                        //
                                        // Only where something is actually
                                        // leaving, since otherwise there is no
                                        // merge for the choice to be part of.
                                        if *kept && folding_in && !group.tags.is_empty() {
                                            let own: Vec<String> = track.tags.clone();
                                            ui.horizontal_wrapped(|ui| {
                                                ui.add_space(10.0);
                                                ui.label(
                                                    RichText::new("keep tags")
                                                        .size(theme::SMALL)
                                                        .color(theme::dim()),
                                                );
                                                // A file kept alongside the
                                                // first starts as itself rather
                                                // than as the pile: nothing is
                                                // folded into it, so taking the
                                                // others' tags would be filing
                                                // it as a record it is not.
                                                let chosen =
                                                    state.tags.entry(*id).or_insert_with(|| {
                                                        own.iter().cloned().collect()
                                                    });
                                                for tag in &group.tags {
                                                    let on = chosen.contains(tag);
                                                    let mine = own.contains(tag);
                                                    if ui
                                                        .selectable_label(
                                                            on,
                                                            RichText::new(tag)
                                                                .size(theme::SMALL)
                                                                .color(match (on, mine) {
                                                                    (false, _) => theme::dim(),
                                                                    (true, true) => theme::text(),
                                                                    (true, false) => theme::amber(),
                                                                }),
                                                        )
                                                        .on_hover_text(match mine {
                                                            true => "already on this file",
                                                            false => "from one of the others",
                                                        })
                                                        .clicked()
                                                    {
                                                        match on {
                                                            true => chosen.remove(tag),
                                                            false => chosen.insert(tag.clone()),
                                                        };
                                                    }
                                                }
                                            });
                                        }

                                        // One line per disagreement, with both
                                        // answers to choose between.
                                        for conflict in &plan.conflicts {
                                            let key = (*id, conflict.field);
                                            ui.horizontal_wrapped(|ui| {
                                                ui.add_space(10.0);
                                                ui.label(
                                                    RichText::new(format!(
                                                        "{}:",
                                                        conflict.field.name()
                                                    ))
                                                    .size(theme::SMALL)
                                                    .color(theme::dim()),
                                                );
                                                for (side, value) in [
                                                    (Side::Kept, &conflict.kept),
                                                    (Side::Other, &conflict.other),
                                                ] {
                                                    let chosen =
                                                        state.picked.get(&key) == Some(&side);
                                                    // A radio rather than a
                                                    // label that happens to be
                                                    // clickable: this is the one
                                                    // place in the sheet that is
                                                    // waiting on a person, and
                                                    // it has to look like it.
                                                    if ui
                                                        .radio(
                                                            chosen,
                                                            RichText::new(value).size(theme::SMALL),
                                                        )
                                                        .on_hover_text(match side {
                                                            Side::Kept => "what the kept copy says",
                                                            Side::Other => "what this copy says",
                                                        })
                                                        .clicked()
                                                    {
                                                        state.picked.insert(key, side);
                                                    }
                                                }
                                            });
                                        }
                                    });
                                });
                            }
                        }
                    });

                // The footer, in the space kept for it above.
                ui.separator();
                let going = going_to_the_trash(&shown, &state);
                let bytes: u64 = going
                    .iter()
                    .filter_map(|(id, _)| self.library.get(*id))
                    .map(|track| track.bytes)
                    .sum();
                // Untidy is a group keeping more than the one file it needs to.
                let untidy = shown
                    .iter()
                    .filter(|group| {
                        group.members.iter().filter(|(_, kept, _, _)| *kept).count() > 1
                    })
                    .count();
                let waiting = shown
                    .iter()
                    .flat_map(|group| group.members.iter())
                    .filter(|(id, kept, _, plan)| {
                        !kept
                            && !plan
                                .conflicts
                                .iter()
                                .all(|c| state.picked.contains_key(&(*id, c.field)))
                    })
                    .count();
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(
                            !going.is_empty(),
                            egui::Button::new(
                                RichText::new(format!(
                                    "Move {} to the trash  ({})",
                                    plural(going.len(), "file"),
                                    crate::sync::bytes(bytes)
                                ))
                                .color(theme::booth())
                                .strong(),
                            )
                            .fill(theme::alert()),
                        )
                        .on_hover_text(
                            "To the trash, not gone: this is the one thing here that touches \
                             your music",
                        )
                        .clicked()
                    {
                        delete = true;
                    }
                    if untidy > 0
                        && ui
                            .button(RichText::new("Keep one of each").size(theme::SMALL))
                            .on_hover_text("Go back to keeping only the best copy of each record")
                            .clicked()
                    {
                        tidy = true;
                    }
                    if waiting > 0 {
                        ui.label(
                            RichText::new(format!(
                                "{} left alone until {} disagreement is answered",
                                plural(waiting, "copy"),
                                if waiting == 1 { "its" } else { "each" },
                            ))
                            .size(theme::SMALL)
                            .color(theme::alert()),
                        );
                    }
                });
            });

        if delete {
            // The picks travel with the ids: by the time this runs the sheet is
            // shut, and the merge still has to know how each disagreement was
            // settled.
            let going = going_to_the_trash(&shown, &state);
            let picked: Vec<(u32, Field, Side)> =
                state.picked.iter().map(|((id, field), side)| (*id, *field, *side)).collect();
            // Only for the groups something is actually leaving, and in the
            // order the chips were drawn in, so what is written reads the way
            // the sheet read.
            let tags: Vec<(u32, Vec<String>)> = shown
                .iter()
                .filter(|group| going.iter().any(|(_, into)| *into == group.into))
                .flat_map(|group| {
                    group.members.iter().filter(|(_, kept, _, _)| *kept).filter_map(|(id, ..)| {
                        let chosen = state.tags.get(id)?;
                        let wanted = group
                            .tags
                            .iter()
                            .filter(|tag| chosen.contains(*tag))
                            .cloned()
                            .collect();
                        Some((*id, wanted))
                    })
                })
                .collect();
            self.pending.push(Pending::TrashDuplicates { going, picked, tags });
            self.duplicates = None;
        } else {
            if tidy {
                for group in &shown {
                    for (id, _, _, _) in &group.members {
                        state.keeping.remove(id);
                    }
                    if let Some(best) = groups.iter().find(|g| g.key() == group.key) {
                        state.keeping.insert(best.keep);
                    }
                }
            }
            match open {
                true => self.duplicates = Some(state),
                false => self.duplicates = None,
            }
        }
        if look {
            self.hash_unchecked();
        }
    }

    /// The offer to put a copier's name right, once what it was copied from
    /// has gone.
    ///
    /// An offer rather than something done on the way past. The file is
    /// somebody's, its name may be what a playlist somewhere else refers to,
    /// and `track_04 (1).flac` is a perfectly working name — the only argument
    /// for changing it is tidiness, which is not an argument for doing it
    /// without being asked.
    fn renames_sheet(&mut self, ctx: &egui::Context) {
        let offers = self.renames.clone();
        let mut open = true;
        let mut rename: Option<Vec<(u32, PathBuf)>> = None;

        egui::Window::new("Names a copier wrote")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(680.0)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .max_height(sheet_height(ctx))
            .frame(sheet_frame())
            .show(ctx, |ui| {
                ui.label(
                    RichText::new(
                        "These were kept, and the files they were copied from have gone — so \
                         the plain name is free again.",
                    )
                    .color(theme::dim())
                    .size(theme::SMALL),
                );
                ui.add_space(8.0);
                egui::ScrollArea::vertical()
                    .max_height(sheet_height(ctx) - 150.0)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for (id, plain) in &offers {
                            let Some(track) = self.library.get(*id) else { continue };
                            let from = track.path.clone();
                            ui.horizontal_top(|ui| {
                                ui.add_space(6.0);
                                ui.vertical(|ui| {
                                    ui.horizontal_wrapped(|ui| {
                                        path_label(ui, &from, theme::dim());
                                    });
                                    ui.horizontal_wrapped(|ui| {
                                        ui.add_space(10.0);
                                        ui.label(
                                            RichText::new("\u{2192}")
                                                .font(theme::mono(10.0))
                                                .color(theme::go()),
                                        );
                                        path_label(ui, plain, theme::go());
                                    });
                                });
                            });
                            ui.add_space(6.0);
                        }
                    });
                ui.separator();
                ui.horizontal(|ui| {
                    if ui
                        .add(
                            egui::Button::new(
                                RichText::new(format!("Rename {}", plural(offers.len(), "file")))
                                    .strong(),
                            )
                            .fill(theme::booth_2()),
                        )
                        .on_hover_text("Renames the files on disk, and follows them here")
                        .clicked()
                    {
                        rename = Some(offers.clone());
                    }
                    if ui.button("Leave them").on_hover_text("The names stay as they are").clicked()
                    {
                        rename = Some(Vec::new());
                    }
                });
            });

        if let Some(doing) = rename {
            self.rename_files(&doing);
            self.renames.clear();
        } else if !open {
            self.renames.clear();
        }
    }

    /// Rename files on disk and follow them in the collection.
    ///
    /// A name already taken is skipped rather than written over: the whole
    /// point of the offer is that the name was free, and if something has taken
    /// it since then the offer was wrong.
    fn rename_files(&mut self, doing: &[(u32, PathBuf)]) {
        let mut done = 0usize;
        for (id, to) in doing {
            let Some(track) = self.library.get(*id) else { continue };
            let from = track.path.clone();
            if to.exists() {
                crate::warn!("not renaming {}: {} is taken", from.display(), to.display());
                continue;
            }
            match std::fs::rename(&from, to) {
                Ok(()) => {
                    crate::info!("renamed {} to {}", from.display(), to.display());
                    if let Some(track) = self.library.get_mut(*id) {
                        track.path = to.clone();
                    }
                    done += 1;
                }
                Err(e) => {
                    crate::warn!("could not rename {}: {e}", from.display());
                    self.note(format!("{} would not rename", from.display()), theme::alert());
                }
            }
        }
        if done > 0 {
            self.note(format!("{} renamed", plural(done, "file")), theme::text());
            self.save();
        }
    }

    /// Read the files of the tracks showing back, and compare.
    ///
    /// Everything showing, like the rest of the strip: narrow the list to the
    /// part of the collection in question and check that part. The whole
    /// collection is what showing means when nothing is typed in the bar.
    fn verify_showing(&mut self, deep: bool) {
        if self.running() {
            return;
        }
        let ids = self.acting_on(|_| true);
        let tracks: Vec<crate::library::Track> =
            ids.iter().filter_map(|id| self.library.get(*id)).cloned().collect();
        if tracks.is_empty() {
            return;
        }
        crate::info!(
            "checking {}{}",
            plural(tracks.len(), "track"),
            if deep { ", reading every byte" } else { "" }
        );
        self.checked =
            Some(Checked { looked_at: tracks.len(), running: true, deep, ..Default::default() });
        // The walk for files nobody knows about is only honest over the whole
        // collection: with the list narrowed, every file outside the filter
        // would be reported as a stray.
        let library = match ids.len() == self.library.tracks.len() {
            true => self.config.library_path.clone(),
            false => PathBuf::new(),
        };
        self.start(Job::Verify { tracks, deep, library });
    }

    /// What the check found, and what can be done about it.
    ///
    /// A reading of the files as they were a moment ago, so nothing here is
    /// kept: shutting the sheet throws it away rather than leaving somebody
    /// yesterday's answer about a folder they have since tidied.
    fn verify_sheet(&mut self, ctx: &egui::Context) {
        let Some(checked) = &self.checked else { return };
        let (running, deep, looked_at) = (checked.running, checked.deep, checked.looked_at);
        let reports = checked.troubles.clone();
        let orphans = checked.orphans.clone();
        let naming = checked.naming.clone();
        let mut open = true;
        let mut fix: Vec<u32> = Vec::new();
        let mut forget: Vec<u32> = Vec::new();
        let mut adopt_strays = false;
        let mut rename = false;

        let fixable: Vec<u32> = reports
            .iter()
            .filter(|report| report.troubles.iter().any(|t| t.is_fixable()))
            .map(|report| report.id)
            .collect();
        let missing: Vec<u32> = reports
            .iter()
            .filter(|report| report.troubles.contains(&crate::verify::Trouble::Missing))
            .map(|report| report.id)
            .collect();

        egui::Window::new("The collection against its files")
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_width(760.0)
            .default_height(520.0)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .max_height(sheet_height(ctx))
            .frame(
                egui::Frame::NONE
                    .fill(theme::booth())
                    .stroke(egui::Stroke::new(1.0_f32, theme::rule()))
                    .inner_margin(egui::Margin::same(14)),
            )
            .show(ctx, |ui| {
                ui.label(
                    RichText::new(match running {
                        true => format!("Reading {} back…", plural(looked_at, "track")),
                        false => format!(
                            "Read {} back{}.",
                            plural(looked_at, "track"),
                            match deep {
                                true => ", every byte of each",
                                false => "",
                            }
                        ),
                    })
                    .color(theme::dim())
                    .size(theme::SMALL),
                );
                ui.add_space(8.0);

                if !running && reports.is_empty() && orphans.is_empty() {
                    ui.label(
                        RichText::new(match deep {
                            true => "Every file is there and is the file it was.",
                            false => {
                                "Every file is there, the right size, and tagged as \
                                      the collection says."
                            }
                        })
                        .color(theme::go()),
                    );
                    if !deep {
                        ui.add_space(4.0);
                        ui.label(
                            RichText::new(
                                "A file edited in place without changing length would not \
                                 show up here. Checking that means reading every byte.",
                            )
                            .color(theme::dim())
                            .size(theme::SMALL),
                        );
                    }
                }

                let list_height = (ui.available_height() - FOOTER_HEIGHT).max(120.0);
                egui::ScrollArea::vertical()
                    .max_height(list_height)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for (n, report) in reports.iter().enumerate() {
                            if n > 0 {
                                ui.add_space(6.0);
                                ui.separator();
                                ui.add_space(6.0);
                            }
                            if let Some(track) = self.library.get(report.id) {
                                ui.label(
                                    RichText::new(track.display_title())
                                        .color(theme::text())
                                        .size(theme::BODY),
                                );
                            }
                            ui.horizontal_top(|ui| {
                                ui.add_space(14.0);
                                ui.vertical(|ui| {
                                    ui.horizontal_wrapped(|ui| {
                                        path_label(ui, &report.path, theme::dim());
                                    });
                                    for trouble in &report.troubles {
                                        ui.horizontal_wrapped(|ui| {
                                            ui.add_space(10.0);
                                            ui.label(
                                                RichText::new(trouble.what())
                                                    .size(theme::SMALL)
                                                    // A question is not a
                                                    // fault, so it is not
                                                    // coloured like one.
                                                    .color(
                                                        match (
                                                            trouble.is_a_choice(),
                                                            trouble.is_fixable(),
                                                        ) {
                                                            (true, _) => theme::text(),
                                                            (_, true) => theme::amber(),
                                                            _ => theme::alert(),
                                                        },
                                                    ),
                                            );
                                            // Both answers, where there are two
                                            // — a difference is not worth
                                            // reporting if it cannot be seen.
                                            if let crate::verify::Trouble::Field {
                                                stored,
                                                file,
                                                ..
                                            } = trouble
                                            {
                                                ui.label(
                                                    RichText::new(format!(
                                                        "{stored}  \u{2192}  {file}"
                                                    ))
                                                    .font(theme::mono(10.0))
                                                    .color(theme::text()),
                                                );
                                            }
                                        });
                                        // The one trouble with two answers that
                                        // might both be right, so it is picked
                                        // between rather than corrected.
                                        if let crate::verify::Trouble::Unlike {
                                            stored,
                                            from_path,
                                        } = trouble
                                        {
                                            let taking =
                                                naming.get(&report.id).copied().unwrap_or(false);
                                            ui.horizontal_wrapped(|ui| {
                                                ui.add_space(10.0);
                                                if ui
                                                    .radio(!taking, "")
                                                    .on_hover_text("Leave it as it is")
                                                    .clicked()
                                                {
                                                    self.pending.push(Pending::ChooseName {
                                                        id: report.id,
                                                        from_path: false,
                                                    });
                                                }
                                                ui.label(
                                                    RichText::new(stored)
                                                        .font(theme::mono(10.0))
                                                        .color(theme::text()),
                                                );
                                                ui.label(
                                                    RichText::new("the tags")
                                                        .size(theme::SMALL)
                                                        .color(theme::dim()),
                                                );
                                            });
                                            ui.horizontal_wrapped(|ui| {
                                                ui.add_space(10.0);
                                                if ui
                                                    .radio(taking, "")
                                                    .on_hover_text(
                                                        "Take the names the file is filed under",
                                                    )
                                                    .clicked()
                                                {
                                                    self.pending.push(Pending::ChooseName {
                                                        id: report.id,
                                                        from_path: true,
                                                    });
                                                }
                                                ui.label(
                                                    RichText::new(from_path.describe())
                                                        .font(theme::mono(10.0))
                                                        .color(theme::text()),
                                                );
                                                ui.label(
                                                    RichText::new("the file name")
                                                        .size(theme::SMALL)
                                                        .color(theme::dim()),
                                                );
                                            });
                                        }
                                    }
                                });
                            });
                        }

                        if !orphans.is_empty() {
                            if !reports.is_empty() {
                                ui.add_space(6.0);
                                ui.separator();
                                ui.add_space(6.0);
                            }
                            ui.label(
                                RichText::new(format!(
                                    "{} in the library folder that no track points at",
                                    plural(orphans.len(), "file")
                                ))
                                .color(theme::text())
                                .size(theme::BODY),
                            );
                            for path in orphans.iter().take(ORPHANS_SHOWN) {
                                ui.horizontal_top(|ui| {
                                    ui.add_space(14.0);
                                    ui.vertical(|ui| {
                                        ui.horizontal_wrapped(|ui| {
                                            path_label(ui, path, theme::dim());
                                        });
                                    });
                                });
                            }
                            if orphans.len() > ORPHANS_SHOWN {
                                ui.horizontal(|ui| {
                                    ui.add_space(14.0);
                                    ui.label(
                                        RichText::new(format!(
                                            "and {} more",
                                            orphans.len() - ORPHANS_SHOWN
                                        ))
                                        .size(theme::SMALL)
                                        .color(theme::dim()),
                                    );
                                });
                            }
                        }
                    });

                ui.separator();
                ui.horizontal(|ui| {
                    if !fixable.is_empty()
                        && ui
                            .add(
                                egui::Button::new(
                                    RichText::new(format!(
                                        "Take the files' word for {}",
                                        plural(fixable.len(), "track")
                                    ))
                                    .strong(),
                                )
                                .fill(theme::booth_2()),
                            )
                            .on_hover_text(
                                "A file's size and its tags are facts about the file, so where \
                                 they differ the collection is the one that is out of date",
                            )
                            .clicked()
                    {
                        fix = fixable.clone();
                    }
                    if !missing.is_empty()
                        && ui
                            .add(
                                egui::Button::new(
                                    RichText::new(format!(
                                        "Forget {}",
                                        plural(missing.len(), "missing track")
                                    ))
                                    .color(theme::booth())
                                    .strong(),
                                )
                                .fill(theme::alert()),
                            )
                            .on_hover_text(
                                "Only the records go. There is no file to delete — though an \
                                 unplugged drive looks the same from here as a deleted one",
                            )
                            .clicked()
                    {
                        forget = missing.clone();
                    }
                    if !orphans.is_empty()
                        && ui
                            .add(
                                egui::Button::new(
                                    RichText::new(format!(
                                        "Add {}",
                                        plural(orphans.len(), "stray file")
                                    ))
                                    .strong(),
                                )
                                .fill(theme::booth_2()),
                            )
                            .on_hover_text("Read them in, as an import would")
                            .clicked()
                    {
                        adopt_strays = true;
                    }
                    // Only when something has actually been picked. A button
                    // that does nothing is worse than no button.
                    let chosen = naming.values().filter(|from_path| **from_path).count();
                    if chosen > 0
                        && ui
                            .add(
                                egui::Button::new(
                                    RichText::new(format!(
                                        "Take the file name for {}",
                                        plural(chosen, "track")
                                    ))
                                    .strong(),
                                )
                                .fill(theme::booth_2()),
                            )
                            .on_hover_text(
                                "Only the ones set to the file name. Nothing is written to \
                                 any file — this changes what the collection calls them",
                            )
                            .clicked()
                    {
                        rename = true;
                    }
                });
            });

        if !fix.is_empty() {
            self.take_the_files_word(&fix);
        }
        if !forget.is_empty() {
            for id in &forget {
                self.library.remove(*id);
            }
            self.note(format!("{} forgotten", plural(forget.len(), "track")), theme::text());
            self.checked = None;
            self.rebuild();
            self.save();
        }
        if rename {
            self.take_the_names_off_the_files();
        }
        if adopt_strays {
            self.import(orphans);
            self.checked = None;
        }
        if !open {
            self.checked = None;
        }
    }

    /// Rename the tracks whose radio was set to the file's own name.
    ///
    /// The collection only: a name is written into a file by the tag write-back
    /// and nowhere else, and a check that quietly rewrote somebody's tags would
    /// be a check nobody could safely run.
    fn take_the_names_off_the_files(&mut self) {
        let Some(checked) = &self.checked else { return };
        let chosen: Vec<(u32, crate::guess::Guess)> = checked
            .troubles
            .iter()
            .filter(|report| checked.naming.get(&report.id).copied().unwrap_or(false))
            .filter_map(|report| {
                report.troubles.iter().find_map(|trouble| match trouble {
                    crate::verify::Trouble::Unlike { from_path, .. } => {
                        Some((report.id, from_path.clone()))
                    }
                    _ => None,
                })
            })
            .collect();

        let mut renamed = 0usize;
        for (id, from_path) in chosen {
            let Some(track) = self.library.get_mut(id) else { continue };
            crate::info!(
                "#{id} renamed from its file: {} — {} becomes {}",
                track.artist,
                track.title,
                from_path.describe()
            );
            if !from_path.artist.is_empty() {
                track.artist.clone_from(&from_path.artist);
            }
            if !from_path.title.is_empty() {
                track.title.clone_from(&from_path.title);
            }
            if track.album.trim().is_empty() && !from_path.album.is_empty() {
                track.album.clone_from(&from_path.album);
            }
            // The names came off the path, not out of the file, so a later
            // fingerprint is still allowed to correct them without asking.
            track.from_tags = false;
            renamed += 1;
        }

        if renamed > 0 {
            self.note(
                format!("{} renamed from their files", plural(renamed, "track")),
                theme::text(),
            );
            self.checked = None;
            self.rebuild();
            self.save();
        }
    }

    /// Bring the collection up to date with what its files actually say.
    ///
    /// Only what the file answers for itself: its size, its hashes, and the
    /// tags it carries. Never the other way about — writing the collection's
    /// answers into the files is what the inspector's own button is for, and
    /// doing it here would turn a check into an edit of somebody's music.
    ///
    /// A track whose audio has changed stops counting as analysed. The grid and
    /// the cues were measured against bytes that are no longer there, and
    /// keeping them would be keeping an answer to a question nobody asked.
    fn take_the_files_word(&mut self, ids: &[u32]) {
        let Some(checked) = &self.checked else { return };
        let reports: Vec<crate::verify::Report> =
            checked.troubles.iter().filter(|report| ids.contains(&report.id)).cloned().collect();

        // The one trouble in the sheet put right by changing the file rather
        // than the record. Done before the rest, because a file that cannot be
        // read is a file whose flag is why.
        let flagged: Vec<PathBuf> = reports
            .iter()
            .filter(|report| {
                report
                    .troubles
                    .contains(&crate::verify::Trouble::Slow(crate::storage::Slow::Quarantined))
            })
            .map(|report| report.path.clone())
            .collect();
        if !flagged.is_empty() {
            self.clear_quarantine(&flagged);
        }

        let mut put_right = 0usize;
        let mut restale = 0usize;
        for report in &reports {
            let Some(fresh) = &report.fresh else { continue };
            let Some(track) = self.library.get_mut(report.id) else { continue };
            let was_audio = track.audio_hash.clone();

            track.bytes = fresh.bytes;
            track.float_samples = fresh.float_samples;
            track.protected = fresh.protected;
            if !fresh.file_hash.is_empty() {
                track.file_hash.clone_from(&fresh.file_hash);
            }
            if !fresh.audio_hash.is_empty() {
                track.audio_hash.clone_from(&fresh.audio_hash);
            }
            for trouble in &report.troubles {
                match trouble {
                    crate::verify::Trouble::Field { field, file, .. } => {
                        use crate::library::Field;
                        match field {
                            Field::Artist => track.artist.clone_from(file),
                            Field::Album => track.album.clone_from(file),
                            Field::Title => track.title.clone_from(file),
                            Field::Year => track.year = file.parse().ok(),
                            _ => {}
                        }
                    }
                    crate::verify::Trouble::StemGone { part } => track.stems.forget(part),
                    _ => {}
                }
            }
            // Only when the sound itself changed, and only when both answers
            // are known: an empty hash on either side is not evidence.
            let changed_audio = !was_audio.is_empty()
                && !fresh.audio_hash.is_empty()
                && was_audio != fresh.audio_hash;
            if changed_audio && track.analyzed {
                track.analyzed = false;
                restale += 1;
            }
            put_right += 1;
        }

        if put_right > 0 {
            self.note(
                match restale {
                    0 => format!("{} brought up to date", plural(put_right, "track")),
                    n => format!(
                        "{} brought up to date, {} to listen to again",
                        plural(put_right, "track"),
                        n
                    ),
                },
                theme::text(),
            );
            self.checked = None;
            self.rebuild();
            self.save();
        }
    }

    /// Take the macOS quarantine flag off these files.
    ///
    /// A folder holding more than one of them is swept whole, which is what
    /// `xattr -r -d` does and what a person would run: clearing the files alone
    /// leaves the folder to hand the flag straight back to the next thing
    /// copied into it. The rest are cleared one by one, and a file somebody
    /// else owns is reported rather than allowed to stop the others.
    fn clear_quarantine(&mut self, paths: &[PathBuf]) {
        let roots = crate::storage::roots(paths);
        let mut cleared = 0usize;
        let mut refused = Vec::new();
        for root in &roots {
            let (count, no) = crate::storage::clear_tree(root);
            crate::info!("{}: cleared {count} entries", crate::storage::command_for(root, true));
            cleared += count;
            refused.extend(no);
        }
        for path in paths.iter().filter(|path| !roots.iter().any(|root| path.starts_with(root))) {
            match crate::storage::clear(path) {
                Ok(()) => cleared += 1,
                Err(e) => refused.push((path.clone(), e)),
            }
        }

        for (path, why) in &refused {
            crate::warn!("could not unquarantine {}: {why}", path.display());
        }
        crate::info!("took the quarantine flag off {}", plural(cleared, "file"));
        self.note(
            match refused.is_empty() {
                true => format!("unquarantined {}", plural(cleared, "file")),
                false => format!(
                    "unquarantined {}, {} refused — see the log",
                    plural(cleared, "file"),
                    refused.len()
                ),
            },
            match refused.is_empty() {
                true => theme::text(),
                false => theme::amber(),
            },
        );
    }

    /// Read the tracks that have never been hashed, so they can be compared.
    ///
    /// The sheet stays open while this runs: the groups are worked out afresh
    /// each frame, so they fill in as the answers arrive rather than all at the
    /// end.
    fn hash_unchecked(&mut self) {
        let waiting = self.library.unhashed();
        if waiting.is_empty() || self.running() {
            return;
        }
        crate::info!("looking through {} for copies", plural(waiting.len(), "track"));
        self.start(Job::Hash(waiting));
    }

    fn help_sheet(&mut self, ctx: &egui::Context) {
        let mut open = true;
        let mut chosen: Option<String> = None;

        egui::Window::new("The query bar")
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_width(460.0)
            .default_height(520.0)
            .anchor(egui::Align2::LEFT_TOP, [24.0, 56.0])
            .frame(
                egui::Frame::NONE
                    .fill(theme::booth())
                    .stroke(egui::Stroke::new(1.0_f32, theme::rule()))
                    .inner_margin(egui::Margin::same(14)),
            )
            .show(ctx, |ui| {
                ui.label(
                    RichText::new(
                        "Terms narrow the list together. Put a - or ! in front of one to \
                         exclude it, and quotes around anything with a space in it.",
                    )
                    .color(theme::dim())
                    .size(theme::SMALL),
                );
                ui.add_space(10.0);

                egui::ScrollArea::vertical()
                    .max_height(sheet_height(ctx) - 150.0)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for (heading, lines) in crate::query::HELP {
                            pane_label(ui, heading);
                            for help in *lines {
                                ui.horizontal(|ui| {
                                    let example = ui.add(
                                        egui::Label::new(
                                            RichText::new(help.example)
                                                .font(theme::mono(11.0))
                                                .color(theme::amber()),
                                        )
                                        .sense(egui::Sense::click()),
                                    );
                                    let example = example
                                        .on_hover_text("Click to put this in the search bar.");
                                    if example.clicked() {
                                        chosen = Some(help.example.to_string());
                                    }
                                    if example.hovered() {
                                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                                    }
                                    ui.label(
                                        RichText::new(help.means)
                                            .color(theme::dim())
                                            .size(theme::SMALL),
                                    );
                                });
                            }
                            ui.add_space(10.0);
                        }
                    });
            });

        if let Some(text) = chosen {
            self.text = text;
            self.rebuild();
        }
        if !open {
            self.help = false;
        }
    }

    fn log_window(&mut self, ctx: &egui::Context) {
        if !self.log.is_open() {
            return;
        }
        let log = Arc::clone(&self.log);

        ctx.show_viewport_deferred(
            egui::ViewportId::from_hash_of("booth-log"),
            egui::ViewportBuilder::default()
                .with_title("Booth — log")
                .with_inner_size([980.0, 560.0])
                .with_min_inner_size([420.0, 200.0]),
            move |ctx, _class| {
                let level = log.level();
                let entries = crate::log::entries(level);

                egui::TopBottomPanel::top("log-bar")
                    .frame(
                        egui::Frame::NONE
                            .fill(theme::booth_2())
                            .inner_margin(egui::Margin::symmetric(10, 7)),
                    )
                    .show(ctx, |ui| {
                        ui.horizontal(|ui| {
                            for shown in crate::log::Level::SHOWN {
                                let on = level == shown;
                                if ui
                                    .add(
                                        egui::Button::new(
                                            RichText::new(shown.label())
                                                .font(theme::mono(10.5))
                                                .color(if on {
                                                    theme::booth()
                                                } else {
                                                    theme::dim()
                                                }),
                                        )
                                        .fill(if on {
                                            theme::amber()
                                        } else {
                                            theme::booth()
                                        }),
                                    )
                                    .on_hover_text(format!("show {shown} and above"))
                                    .clicked()
                                {
                                    log.set_level(shown);
                                }
                            }

                            ui.separator();
                            let mut follow = log.follows();
                            if ui
                                .checkbox(&mut follow, "follow")
                                .on_hover_text("Stay at the newest line")
                                .changed()
                            {
                                log.set_follow(follow);
                            }
                            if ui.button("clear").clicked() {
                                crate::log::clear();
                            }
                            if ui
                                .button("copy")
                                .on_hover_text("Put everything shown on the clipboard")
                                .clicked()
                            {
                                let text: Vec<String> =
                                    entries.iter().map(|entry| entry.line()).collect();
                                ctx.copy_text(text.join("\n"));
                            }

                            // Just the count on the bar: a full path here is
                            // long enough to reach back across the buttons,
                            // and it is one hover away.
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.label(
                                        RichText::new(crate::library::plural(
                                            entries.len(),
                                            "line",
                                        ))
                                        .font(theme::mono(10.0))
                                        .color(theme::dim()),
                                    )
                                    .on_hover_text(
                                        crate::log::default_path().display().to_string(),
                                    );
                                },
                            );
                        });
                    });

                egui::CentralPanel::default()
                    .frame(
                        egui::Frame::NONE.fill(theme::booth()).inner_margin(egui::Margin::same(8)),
                    )
                    .show(ctx, |ui| {
                        egui::ScrollArea::both()
                            .auto_shrink([false, false])
                            .stick_to_bottom(log.follows())
                            .show_rows(ui, 14.0, entries.len(), |ui, range| {
                                for entry in &entries[range] {
                                    let color = match entry.level {
                                        crate::log::Level::Error => theme::alert(),
                                        crate::log::Level::Warn => theme::amber(),
                                        crate::log::Level::Info => theme::text(),
                                        _ => theme::dim(),
                                    };
                                    ui.horizontal(|ui| {
                                        ui.spacing_mut().item_spacing.x = 8.0;
                                        ui.label(
                                            RichText::new(entry.stamp())
                                                .font(theme::mono(10.5))
                                                .color(theme::dim()),
                                        );
                                        ui.label(
                                            RichText::new(entry.level.tag())
                                                .font(theme::mono(10.5))
                                                .color(color),
                                        );
                                        ui.label(
                                            RichText::new(&entry.text)
                                                .font(theme::mono(11.0))
                                                .color(color),
                                        );
                                    });
                                }
                            });
                    });

                // While it follows the tail it has to repaint to show new
                // lines; parked at a position, it can sit still.
                if log.follows() {
                    ctx.request_repaint_after(std::time::Duration::from_millis(200));
                }
                if ctx.input(|i| i.viewport().close_requested()) {
                    log.set_open(false);
                }
            },
        );
    }

    /// The sheet: what would change, then whether it can.    /// The sheet: what would change, then whether it can.
    /// Where the music is kept, and what to do about music that is not there.
    fn settings_sheet(&mut self, ctx: &egui::Context) {
        let mut open = true;
        let mut changed = false;
        let mut pick_library = false;

        egui::Window::new("Settings")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(560.0)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .max_height(sheet_height(ctx))
            .frame(sheet_frame())
            .show(ctx, |ui| {
                // Scrolled, because the sheet is no taller than the screen now.
                egui::ScrollArea::vertical().show(ui, |ui| {
                    pane_label(ui, "Library folder");
                    ui.label(
                        RichText::new(
                            "Where music copied into the collection is kept, one folder per \
                         artist — the same shape a drive gets.",
                        )
                        .color(theme::dim())
                        .size(theme::SMALL),
                    );
                    ui.horizontal(|ui| {
                        let mut shown = self.config.library_path.display().to_string();
                        if ui
                            .add(
                                egui::TextEdit::singleline(&mut shown)
                                    .desired_width(ui.available_width() - 90.0)
                                    .font(theme::mono(11.0)),
                            )
                            .changed()
                        {
                            self.config.library_path = PathBuf::from(shown.trim());
                            changed = true;
                        }
                        if ui
                            .button("Choose\u{2026}")
                            .on_hover_text("Pick the folder the collection's own copies of tracks are kept in.")
                            .clicked()
                        {
                            pick_library = true;
                        }
                    });

                    ui.add_space(14.0);
                    pane_label(ui, "Music from elsewhere");
                    for policy in OnExternal::ALL {
                        if ui
                            .radio_value(&mut self.config.on_external, policy, policy.label())
                            .changed()
                        {
                            changed = true;
                        }
                        ui.label(
                            RichText::new(policy.blurb()).color(theme::dim()).size(theme::SMALL),
                        );
                        ui.add_space(4.0);
                    }

                    ui.add_space(14.0);
                    pane_label(ui, "Stem quality");
                    for quality in crate::config::Quality::ALL {
                        if ui
                            .radio_value(&mut self.config.stem_quality, quality, quality.label())
                            .changed()
                        {
                            changed = true;
                        }
                        ui.label(
                            RichText::new(quality.blurb()).color(theme::dim()).size(theme::SMALL),
                        );
                    }
                    ui.label(
                        RichText::new(
                            "A kit is rendered once and then played for years, so the slow one is \
                         the default. The fast one is for a first pass over a whole library.",
                        )
                        .color(theme::dim())
                        .size(theme::SMALL),
                    );
                    ui.add_space(8.0);
                    if ui
                        .checkbox(
                            &mut self.config.warn_before_stems,
                            "Say what a batch will take before starting it",
                        )
                        .on_hover_text(
                            "Separating more than one track says how many and roughly how \
                             long first. Cueing from the words needs a separation per track, \
                             which is how an afternoon of it gets started by accident.",
                        )
                        .changed()
                    {
                        changed = true;
                    }
                    if let Some(pace) = self.config.stem_pace {
                        ui.label(
                            RichText::new(format!(
                                "Going by what has been rendered here, a five-minute track takes \
                                 {}.",
                                roughly(300.0 * pace as f64)
                            ))
                            .color(theme::dim())
                            .size(theme::SMALL),
                        );
                    }

                    ui.add_space(14.0);
                    pane_label(ui, "Oldest player a drive must work on");
                    ui.label(
                        RichText::new(
                            "Every generation's files go on whatever this says \u{2014} .DAT for \
                             a 2009 player, .EXT for the nexus 2 line, .2EX for the CDJ-3000's \
                             three-band waveforms \u{2014} because a player reads the richest \
                             one it knows and ignores the rest. What this changes is what the \
                             drive carries. A track this player cannot open, or one above its \
                             sample-rate ceiling, goes on as a 320 kbps mp3 at the best rate \
                             the hardware takes; the library's own file is not touched, so the \
                             same collection still writes a lossless stick for a CDJ-3000.",
                        )
                        .color(theme::dim())
                        .size(theme::SMALL),
                    );
                    for player in booth_cli::compat::Player::ALL {
                        if ui
                            .radio_value(
                                &mut self.config.oldest_player,
                                player,
                                format!("{} and newer", player.name()),
                            )
                            .on_hover_text(format!(
                                "{} \u{2014} up to {:.0} kHz, opens {}.",
                                player.also(),
                                player.max_sample_rate() as f64 / 1000.0,
                                player.plays().join(", ")
                            ))
                            .changed()
                        {
                            changed = true;
                        }
                    }

                    ui.add_space(14.0);
                    pane_label(ui, "Hover help");
                    ui.horizontal(|ui| {
                        let mut delay = self.config.help_delay_ms;
                        if ui
                            .add(
                                egui::Slider::new(&mut delay, 0..=2000)
                                    .suffix(" ms")
                                    .step_by(50.0),
                            )
                            .on_hover_text(
                                "How long the pointer has to rest on a control before its help \
                                 appears. Zero shows it at once; drag it right once you know \
                                 the window.",
                            )
                            .changed()
                        {
                            self.config.help_delay_ms = delay;
                            changed = true;
                        }
                    });
                    ui.label(
                        RichText::new(
                            "Every button, box and switch in the window says what it does when \
                             the pointer rests on it. This is the resting. Settings opens on \
                             \u{2318}, \u{2014} or ctrl-, where that is the key.",
                        )
                        .color(theme::dim())
                        .size(theme::SMALL),
                    );

                    ui.add_space(14.0);
                    pane_label(ui, "Colours");
                    ui.horizontal_wrapped(|ui| {
                        for scheme in theme::SCHEMES {
                            // Each name written in its own scheme's accent, so
                            // the list is a sample of what it is offering
                            // rather than eleven words.
                            let label = RichText::new(scheme.name).color(scheme.amber);
                            let picked = self.config.theme == scheme.name;
                            if ui
                                .radio(picked, label)
                                .on_hover_text(format!("Draw the window in {}.", scheme.name))
                                .clicked()
                                && !picked
                            {
                                self.config.theme = scheme.name.to_string();
                                theme::use_scheme(scheme.name);
                                theme::install(ui.ctx());
                                changed = true;
                            }
                        }
                    });
                    ui.label(
                        RichText::new(
                            "Ten colour schemes out of editors and one of this program's own. \
                             The eight hot cue colours are not among them: those are written \
                             into the cues and go on the drive, so a player lights its buttons \
                             with them.",
                        )
                        .color(theme::dim())
                        .size(theme::SMALL),
                    );

                    ui.add_space(14.0);
                    pane_label(ui, "Track length");
                    ui.horizontal(|ui| {
                        for unit in crate::config::Length::ALL {
                            if ui
                                .radio_value(&mut self.config.length, unit, unit.label())
                                .on_hover_text(format!("Show a track's length in {}.", unit.label()))
                                .changed()
                            {
                                changed = true;
                            }
                        }
                        ui.label(
                            RichText::new(
                                "Four beats to the bar, as the drive's own format counts.",
                            )
                            .color(theme::dim())
                            .size(theme::SMALL),
                        );
                    });

                    ui.add_space(14.0);
                    pane_label(ui, "Identifying tracks");
                    if ui
                        .checkbox(
                            &mut self.config.identify,
                            "Fingerprint tracks when analysing, and fill in missing names",
                        )
                        .changed()
                    {
                        changed = true;
                    }
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new("AcoustID key").color(theme::dim()).size(theme::SMALL),
                        );
                        if ui
                            .add(
                                egui::TextEdit::singleline(&mut self.config.acoustid_key)
                                    .desired_width(ui.available_width())
                                    .font(theme::mono(11.0))
                                    .hint_text(
                                        RichText::new(match std::env::var("ACOUSTID_API_KEY") {
                                            Ok(_) => "using ACOUSTID_API_KEY",
                                            Err(_) => "free from acoustid.org/new-application",
                                        })
                                        .monospace()
                                        .color(theme::dim()),
                                    ),
                            )
                            .on_hover_text(
                                "Your own AcoustID key, used when looking up what a track is. \
                                 Free from acoustid.org; the environment variable is used when \
                                 this is empty.",
                            )
                            .changed()
                        {
                            changed = true;
                        }
                    });
                    let mut percent = self.config.autotag_score * 100.0;
                    if ui
                        .add(
                            egui::Slider::new(&mut percent, 50.0..=100.0)
                                .suffix("%")
                                .text("apply without asking at"),
                        )
                        .on_hover_text(
                            "How sure a lookup has to be before its answer is written in \
                             without asking. Below this, the match is put to you.",
                        )
                        .changed()
                    {
                        self.config.autotag_score = percent / 100.0;
                        changed = true;
                    }
                    ui.label(
                        RichText::new(
                            "Below this, and for anything that disagrees with a name already in \
                         the file's tags, the match is put to you instead. Nothing under 50% \
                         is offered at all.",
                        )
                        .color(theme::dim())
                        .size(theme::SMALL),
                    );

                    ui.add_space(10.0);
                    pane_label(ui, "Tags");
                    for level in crate::config::WriteTags::ALL {
                        if ui
                            .radio_value(&mut self.config.write_tags, level, level.label())
                            .changed()
                        {
                            changed = true;
                        }
                        ui.label(
                            RichText::new(level.blurb()).color(theme::dim()).size(theme::SMALL),
                        );
                        ui.add_space(4.0);
                    }
                    ui.label(
                        RichText::new(
                            "Filling in a blank is not the same act as overwriting somebody's \
                         answer, which is why the middle one is the default: a lookup that \
                         names an untagged file has found out something true about it. \
                         FLAC and MP3 only — a WAV has nowhere to put them. The inspector's \
                         own button always overwrites, whatever this says.",
                        )
                        .color(theme::dim())
                        .size(theme::SMALL),
                    );

                    ui.add_space(14.0);
                    pane_label(ui, "Words");
                    ui.label(
                        RichText::new(
                            "Cueing a track by what is sung on it needs a speech recogniser, \
                         and Booth does not ship one or download one. Install whisper.cpp \
                         and point at a ggml model file, or install OpenAI's `whisper` and \
                         name the program `whisper` here. Everything else in the program \
                         works without this; only the words need it.",
                        )
                        .color(theme::dim())
                        .size(theme::SMALL),
                    );
                    let mut program = self.config.whisper.program.clone();
                    if ui
                        .add(
                            egui::TextEdit::singleline(&mut program)
                                .desired_width(ui.available_width())
                                .hint_text("whisper-cli, or a path to it")
                                .font(theme::mono(11.0)),
                        )
                        .on_hover_text(
                            "Which of the two it is comes from this name: `whisper` is \
                             OpenAI's, anything else is whisper.cpp. BOOTH_WHISPER_BIN when \
                             this is empty.",
                        )
                        .changed()
                    {
                        self.config.whisper.program = program.trim().to_string();
                        changed = true;
                    }
                    let mut model = self.config.whisper.model.clone();
                    if ui
                        .add(
                            egui::TextEdit::singleline(&mut model)
                                .desired_width(ui.available_width())
                                .hint_text("path to ggml-base.en.bin, or a model name")
                                .font(theme::mono(11.0)),
                        )
                        .on_hover_text(
                            "whisper.cpp cannot run without a model file. OpenAI's picks its \
                             own from a name like `small` or `turbo`. BOOTH_WHISPER_MODEL \
                             when this is empty.",
                        )
                        .changed()
                    {
                        self.config.whisper.model = model.trim().to_string();
                        changed = true;
                    }
                    let mut language = self.config.whisper.language.clone();
                    if ui
                        .add(
                            egui::TextEdit::singleline(&mut language)
                                .desired_width(ui.available_width())
                                .hint_text("en")
                                .font(theme::mono(11.0)),
                        )
                        .on_hover_text(
                            "Worth setting. Left to itself the recogniser guesses the language \
                             off the first few seconds, and the first few seconds of an \
                             isolated vocal are usually a breath.",
                        )
                        .changed()
                    {
                        self.config.whisper.language = language.trim().to_string();
                        changed = true;
                    }

                    ui.add_space(14.0);
                    pane_label(ui, "rekordbox");
                    ui.label(
                        RichText::new(
                            "rekordbox keeps its library in an encrypted SQLite file. The key is \
                         the same on every installation and this build carries it, so there is \
                         nothing to fill in here. It is only worth using if AlphaTheta ever \
                         changes the key: put the new one here, or in REKORDBOX_KEY.",
                        )
                        .color(theme::dim())
                        .size(theme::SMALL),
                    );
                    let mut key = self.config.rekordbox_key.clone();
                    if ui
                        .add(
                            egui::TextEdit::singleline(&mut key)
                                .desired_width(ui.available_width())
                                .hint_text("SQLCipher key")
                                .password(true)
                                .font(theme::mono(11.0)),
                        )
                        .on_hover_text(
                            "The key rekordbox's own master.db is encrypted with. This build \
                             carries it; this is here for the day it changes.",
                        )
                        .changed()
                    {
                        self.config.rekordbox_key = key.trim().to_string();
                        changed = true;
                    }
                    ui.add_space(10.0);
                    ui.label(
                        RichText::new(
                            "A drive carries a second, separate library for the CDJ-3000X and \
                         the other newer players, under its own key — a different one from \
                         above, also carried by this build. A sync writes both databases. \
                         This is here for the same reason as the one above: the day the key \
                         changes, it is what keeps the program working.",
                        )
                        .color(theme::dim())
                        .size(theme::SMALL),
                    );
                    let mut onelibrary = self.config.onelibrary_key.clone();
                    if ui
                        .add(
                            egui::TextEdit::singleline(&mut onelibrary)
                                .desired_width(ui.available_width())
                                .hint_text("OneLibrary key")
                                .password(true)
                                .font(theme::mono(11.0)),
                        )
                        .changed()
                    {
                        self.config.onelibrary_key = onelibrary.trim().to_string();
                        changed = true;
                    }
                    ui.add_space(10.0);
                    if ui
                        .button("Import a rekordbox library")
                        .on_hover_text(
                            "Reads master.db. Nothing already here is overwritten — what comes \
                         across is what is missing, plus the playlists.",
                        )
                        .clicked()
                    {
                        self.want_pick = Some(Picking::Rekordbox);
                    }

                    ui.add_space(14.0);
                    pane_label(ui, "Keeping copies of drives");
                    ui.label(
                        RichText::new(
                            "A stick holds hours of work in the place most likely to be dropped \
                         or left in a booth. When one is written, or a prepared one is plugged \
                         in, its databases, analysis, cues and history are copied here. The \
                         audio is linked to the library's own copy rather than copied, so a \
                         drive costs megabytes.",
                        )
                        .color(theme::dim())
                        .size(theme::SMALL),
                    );
                    if ui
                        .checkbox(&mut self.config.keep_drives, "Keep a copy of every drive")
                        .changed()
                    {
                        changed = true;
                    }
                    ui.horizontal_wrapped(|ui| {
                        path_label(ui, &self.config.backups_path.clone(), theme::dim());
                    });
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new(
                            "Music on another laptop's stick, or somebody else's, that the \
                             library has no copy of. Yours is always linked, whatever this says \
                             \u{2014} a track is recognised by its sound, so a rename or a \
                             retag does not make a second copy of it. And a track the \
                             collection names but the disk has lost is put back off the drive \
                             whichever of these is set: that row is broken rather than absent, \
                             and the stick that can fix it will not be here for ever.",
                        )
                        .color(theme::dim())
                        .size(theme::SMALL),
                    );
                    for what in crate::config::OnForeign::ALL {
                        if ui
                            .radio_value(&mut self.config.from_other_drives, what, what.label())
                            .on_hover_text(what.blurb())
                            .changed()
                        {
                            changed = true;
                        }
                        ui.label(
                            RichText::new(what.blurb()).color(theme::dim()).size(theme::SMALL),
                        );
                        ui.add_space(4.0);
                    }

                    ui.add_space(14.0);
                    pane_label(ui, "Where stems go");
                    for where_ in [crate::config::StemsIn::Beside, crate::config::StemsIn::Folder] {
                        if ui
                            .radio_value(&mut self.config.stems_in, where_, where_.label())
                            .changed()
                        {
                            changed = true;
                        }
                        ui.label(
                            RichText::new(where_.blurb()).color(theme::dim()).size(theme::SMALL),
                        );
                        ui.add_space(4.0);
                    }
                    if self.config.stems_in == crate::config::StemsIn::Folder {
                        let mut shown = self.config.stems_path.display().to_string();
                        if ui
                            .add(
                                egui::TextEdit::singleline(&mut shown)
                                    .desired_width(ui.available_width())
                                    .font(theme::mono(11.0)),
                            )
                            .on_hover_text(
                                "The one folder rendered kits go into. Both places are searched \
                                 whichever is set, so changing it never loses a kit.",
                            )
                            .changed()
                        {
                            self.config.stems_path = PathBuf::from(shown.trim());
                            changed = true;
                        }
                    }
                    ui.label(
                        RichText::new(
                            "Both places are searched whichever is set, so changing this never \
                         loses a kit that is already rendered.",
                        )
                        .color(theme::dim())
                        .size(theme::SMALL),
                    );

                    ui.add_space(12.0);
                    let outside = self
                        .external(&self.library.tracks.iter().map(|t| t.id).collect::<Vec<_>>());
                    if !outside.is_empty() {
                        ui.label(
                            RichText::new(format!(
                                "{} {} in the collection are outside this folder.",
                                outside.len(),
                                if outside.len() == 1 { "track is" } else { "tracks are" }
                            ))
                            .color(theme::amber())
                            .size(theme::SMALL),
                        );
                        if ui
                            .button("Copy them all in")
                            .on_hover_text("Copy every track that lives outside the library folder into it.")
                            .clicked()
                        {
                            self.pending.extend(outside.into_iter().map(Pending::Adopt));
                        }
                    }
                });
            });

        if changed {
            if let Err(e) = self.config.save(&self.config_path) {
                self.note(format!("could not save the settings: {e:#}"), theme::alert());
            }
        }
        if pick_library {
            self.want_pick = Some(Picking::LibraryFolder);
        }
        if !open {
            self.settings = false;
        }
    }

    /// What to do about tracks that turned up from outside the library.
    ///
    /// Only ever shown when the policy is to ask. It offers the same three
    /// answers the setting does, so that answering here is also a way of
    /// deciding it once.
    /// Note which of these the hardware will not play.
    ///
    /// At import rather than at sync, which is the whole point: the check
    /// before a write is the last chance to catch a file that will not load,
    /// and by then the answer is "leave it behind". Asked when the file is
    /// added, there is time to do something about it.
    fn check_compatibility(&mut self, ids: &[u32]) {
        for id in ids {
            let Some(track) = self.library.get(*id) else { continue };
            let Some(problem) = track.incompatibility() else { continue };
            crate::warn!(
                "{}: {} — {}",
                track.path.file_name().unwrap_or_default().to_string_lossy(),
                problem.what(),
                problem.fix()
            );
            if !self.incompatible.contains(id) {
                self.incompatible.push(*id);
            }
        }
    }

    /// What was just imported that will be slow to reach, and why.
    ///
    /// Said at import rather than at play, because at play it is too late and
    /// nothing about the file looks wrong in between. The quarantine flag is
    /// the half of this Booth can take off — "Check" and then "take the files'
    /// word" does it — and where somebody keeps their music is theirs to
    /// decide, so that half is said once and not again.
    fn check_storage(&mut self, ids: &[u32]) {
        use crate::storage::Slow;

        let mut flagged = 0usize;
        let mut downloads = 0usize;
        let mut synced: Vec<(&'static str, usize)> = Vec::new();
        for id in ids {
            let Some(track) = self.library.get(*id) else { continue };
            for slow in crate::storage::slow(&track.path) {
                match slow {
                    Slow::Quarantined => flagged += 1,
                    Slow::Downloads => downloads += 1,
                    Slow::Synced(service) => match synced.iter_mut().find(|(s, _)| *s == service) {
                        Some((_, count)) => *count += 1,
                        None => synced.push((service, 1)),
                    },
                }
            }
        }

        let mut lines = Vec::new();
        if flagged > 0 {
            crate::warn!(
                "{} carry the macOS quarantine flag, so every play is a Gatekeeper check —                  Check, then take the files' word, takes it off ({})",
                plural(flagged, "track"),
                crate::storage::command_for(std::path::Path::new("FOLDER"), true)
            );
            lines.push(format!("{} quarantined", plural(flagged, "track")));
        }
        if downloads > 0 {
            crate::warn!(
                "{} sit in the Downloads folder, where macOS re-flags what lands in it —                  move them into the library folder to be done with it",
                plural(downloads, "track")
            );
            lines.push(format!("{downloads} in Downloads"));
        }
        for (service, count) in &synced {
            crate::warn!(
                "{} are inside {service}, so playing one may mean fetching it first —                  copy them onto this machine before the night rather than during it",
                plural(*count, "track")
            );
            lines.push(format!("{count} in {service}"));
        }

        if !lines.is_empty() {
            self.note(format!("slow to reach: {}", lines.join(", ")), theme::amber());
        }
    }

    /// What was just imported that a player will not open, and what to do.
    fn compatibility_sheet(&mut self, ctx: &egui::Context) {
        let waiting: Vec<(u32, String, PathBuf, booth_cli::compat::Problem)> = self
            .incompatible
            .iter()
            .filter_map(|id| self.library.get(*id))
            .filter_map(|track| {
                track
                    .incompatibility()
                    .map(|problem| (track.id, track.display_title(), track.path.clone(), problem))
            })
            .collect();
        if waiting.is_empty() {
            self.incompatible.clear();
            return;
        }
        let convertible: Vec<u32> =
            waiting.iter().filter(|(_, _, _, p)| p.convertible()).map(|(id, ..)| *id).collect();

        let mut open = true;
        egui::Window::new(format!("{} a player will not open", plural(waiting.len(), "track")))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(600.0)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .max_height(sheet_height(ctx))
            .frame(sheet_frame())
            .show(ctx, |ui| {
                let target = self.config.oldest_player;
                ui.label(
                    RichText::new(format!(
                        "These import and play here. A {} takes MP3 and AAC at 44.1\u{2013}48 \
                         kHz, and opens {} at up to {:.0} kHz \u{2014} which is the booth this \
                         collection is being checked against, in Settings.",
                        target.name(),
                        target.plays().join(", "),
                        target.max_sample_rate() as f64 / 1000.0
                    ))
                    .color(theme::dim())
                    .size(theme::SMALL),
                );
                ui.add_space(8.0);

                egui::ScrollArea::vertical().max_height(240.0).show(ui, |ui| {
                    for (_, name, path, problem) in &waiting {
                        ui.label(RichText::new(name).color(theme::text()).size(theme::SMALL));
                        ui.label(
                            RichText::new(format!("{} — {}", problem.what(), problem.fix()))
                                .color(if problem.convertible() {
                                    theme::amber()
                                } else {
                                    theme::alert()
                                })
                                .size(theme::SMALL),
                        );
                        ui.label(
                            RichText::new(path.display().to_string())
                                .font(theme::mono(9.5))
                                .color(theme::dim()),
                        );
                        ui.add_space(4.0);
                    }
                });

                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if !convertible.is_empty()
                        && ui
                            .add(
                                egui::Button::new(
                                    RichText::new(theme::label_text(&format!(
                                        "Convert {}",
                                        plural(convertible.len(), "file")
                                    )))
                                    .size(11.0)
                                    .color(theme::booth())
                                    .strong(),
                                )
                                .fill(theme::amber()),
                            )
                            .on_hover_text(
                                "Writes a FLAC beside each original and points the \
                                 collection at it. The originals are not touched.",
                            )
                            .clicked()
                    {
                        self.pending.push(Pending::Convert(convertible.clone()));
                    }
                    if ui
                        .button("Leave them")
                        .on_hover_text("They stay in the collection and out of the drive.")
                        .clicked()
                    {
                        self.incompatible.clear();
                    }
                    if convertible.len() < waiting.len() {
                        ui.label(
                            RichText::new(match convertible.is_empty() {
                                true => "None of these can be converted here.".to_string(),
                                false => format!(
                                    "{} cannot be converted here.",
                                    waiting.len() - convertible.len()
                                ),
                            })
                            .color(theme::dim())
                            .size(theme::SMALL),
                        );
                    }
                });
            });
        if !open {
            self.incompatible.clear();
        }
    }

    /// What a batch of separations will cost, before it is spent.
    ///
    /// The number worth knowing is the time, so that is what this leads with,
    /// measured from what this machine has actually managed rather than from
    /// anything about the hardware. Until it has managed one there is no
    /// estimate and the sheet says so, which is better than a number made up.
    fn stems_sheet(&mut self, ctx: &egui::Context) {
        let Some(ahead) = &self.stems_ahead else { return };
        let (tracks, for_cues) = (ahead.tracks.clone(), ahead.for_cues);
        let named: Vec<String> = tracks
            .iter()
            .filter_map(|id| self.library.get(*id))
            .map(|track| format!("{} \u{2014} {}", track.artist, track.display_title()))
            .collect();
        let audio: f64 =
            tracks.iter().filter_map(|id| self.library.get(*id)).map(|t| t.duration_secs).sum();
        let estimate = self.config.stem_estimate(audio);

        let mut open = true;
        let mut go = false;
        let mut not_now = false;
        let mut stop_asking = false;
        egui::Window::new(format!("Stems for {} first", plural(tracks.len(), "track")))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(600.0)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .max_height(sheet_height(ctx))
            .frame(sheet_frame())
            .show(ctx, |ui| {
                if for_cues {
                    ui.label(
                        RichText::new(
                            "Cueing from the words means reading them off an isolated vocal, \
                             and none of these have one yet. So the words come after a \
                             separation each \u{2014} which is the slow part of this program, \
                             and the part nothing else here waits on.",
                        )
                        .color(theme::text())
                        .size(theme::SMALL),
                    );
                } else {
                    ui.label(
                        RichText::new(
                            "Separation is the slow part of this program. A kit is rendered \
                             once and then played for years, so it is worth doing \u{2014} but \
                             it is worth knowing what it costs first.",
                        )
                        .color(theme::text())
                        .size(theme::SMALL),
                    );
                }
                ui.add_space(8.0);
                ui.label(
                    RichText::new(format!(
                        "{}, {} of music. {}",
                        plural(tracks.len(), "track"),
                        roughly(audio),
                        match estimate {
                            Some(taking) => format!(
                                "About {} at the rate this machine has been managing.",
                                roughly(taking.as_secs_f64())
                            ),
                            None => "Nothing has been separated here yet, so there is no \
                                     estimate \u{2014} reckon on minutes a track without a GPU."
                                .to_string(),
                        }
                    ))
                    .color(theme::amber())
                    .size(theme::SMALL),
                );
                if for_cues {
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(
                            "Paid once per track: the words are kept once they have been read, \
                             and cueing from them again after that costs nothing.",
                        )
                        .color(theme::dim())
                        .size(theme::SMALL),
                    );
                }
                ui.add_space(8.0);

                egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                    for name in &named {
                        ui.label(RichText::new(name).color(theme::dim()).size(theme::SMALL));
                    }
                });

                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ui
                        .add(
                            egui::Button::new(
                                RichText::new(theme::label_text("Render stems"))
                                    .size(11.0)
                                    .color(theme::booth())
                                    .strong(),
                            )
                            .fill(theme::amber()),
                        )
                        .on_hover_text(
                            "Starts separating. It runs a track at a time in the background \
                             and each kit is usable as soon as it is done; the job can be \
                             stopped from the bar at the bottom.",
                        )
                        .clicked()
                    {
                        go = true;
                    }
                    if ui
                        .button("Not now")
                        .on_hover_text("Nothing is rendered and nothing is changed.")
                        .clicked()
                    {
                        not_now = true;
                    }
                    if ui
                        .checkbox(&mut stop_asking, "Stop asking")
                        .on_hover_text(
                            "Separations start without this sheet from now on. Settings puts \
                             it back.",
                        )
                        .changed()
                    {
                        // Only the setting. Whether this batch runs is still
                        // the question on screen, and answering a different
                        // one with a checkbox would be a trap.
                        self.config.warn_before_stems = false;
                        if let Err(e) = self.config.save(&self.config_path) {
                            crate::warn!("could not save the settings: {e:#}");
                        }
                    }
                });
            });

        if go {
            self.pending.push(Pending::SeparateMany { tracks, for_cues });
        }
        if go || not_now || !open {
            self.stems_ahead = None;
        }
    }

    /// Words a server offered that nobody has agreed to yet.
    ///
    /// One track at a time, because this is a judgement rather than a batch:
    /// what is on screen is what was found against what is known, and the only
    /// person who can say whether a remix really is built on that record is
    /// the one who owns it.
    fn wondering_sheet(&mut self, ctx: &egui::Context) {
        let Some(wondering) = self.wondering.first() else { return };
        let id = wondering.id;
        let found = wondering.found.clone();
        let Some(track) = self.library.get(id).cloned() else {
            self.wondering.remove(0);
            return;
        };
        let agrees = booth_cli::tag::lyrics::how_much_agrees(
            &found,
            &crate::library::transcript(&track.lyrics),
        );

        let mut open = true;
        let mut decided = false;
        egui::Window::new("Are these the words?")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(620.0)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .max_height(sheet_height(ctx))
            .frame(sheet_frame())
            .show(ctx, |ui| {
                ui.label(
                    RichText::new(format!("{} \u{2014} {}", track.artist, track.display_title()))
                        .color(theme::text()),
                );
                ui.label(
                    RichText::new(format!(
                        "A lyrics server has words for \u{201c}{} \u{2014} {}\u{201d}, {}. {}",
                        found.artist,
                        found.title,
                        time_text((found.duration_secs * 1000.0) as u32),
                        match found.apart_from(track.duration_secs) {
                            apart if apart < 10.0 => "The same length as this track.".to_string(),
                            apart => format!(
                                "That is {} from this track, so it is another pressing \u{2014} \
                                 an edit, an extended mix, or a different record with the same \
                                 name.",
                                roughly(apart)
                            ),
                        }
                    ))
                    .color(theme::dim())
                    .size(theme::SMALL),
                );
                ui.add_space(6.0);
                ui.label(
                    RichText::new(match agrees {
                        Some(share) => format!(
                            "{:.0}% of the lines already heard off this track turn up in them.",
                            share * 100.0
                        ),
                        None => "Nothing has been heard off this track to compare them with."
                            .to_string(),
                    })
                    .color(theme::amber())
                    .size(theme::SMALL),
                );

                ui.add_space(10.0);
                pane_label(ui, "What was found");
                egui::ScrollArea::vertical().max_height(200.0).id_salt("found").show(ui, |ui| {
                    let shown: Vec<&str> = match found.synced.lines.is_empty() {
                        true => found.plain.iter().map(String::as_str).collect(),
                        false => found.synced.lines.iter().map(|l| l.text.as_str()).collect(),
                    };
                    for line in shown {
                        ui.label(RichText::new(line).color(theme::text()).size(theme::SMALL));
                    }
                });

                if !track.lyrics.is_empty() {
                    ui.add_space(10.0);
                    pane_label(ui, "What was heard off the stem");
                    egui::ScrollArea::vertical().max_height(120.0).id_salt("heard").show(
                        ui,
                        |ui| {
                            for line in &track.lyrics {
                                ui.label(
                                    RichText::new(&line.text)
                                        .color(theme::dim())
                                        .size(theme::SMALL),
                                );
                            }
                        },
                    );
                }

                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ui
                        .add(
                            egui::Button::new(
                                RichText::new(theme::label_text("These are the words"))
                                    .size(11.0)
                                    .color(theme::booth())
                                    .strong(),
                            )
                            .fill(theme::amber()),
                        )
                        .on_hover_text(
                            "Takes them and re-cues the track. Where it has a vocal stem they \
                             are placed against it, since the times came from another pressing.",
                        )
                        .clicked()
                    {
                        self.pending.push(Pending::TakeFoundWords(id, Box::new(found.clone())));
                        decided = true;
                    }
                    if ui
                        .button("Not this record")
                        .on_hover_text(
                            "Leaves the track as it is. Its words are still read off the stem \
                             the ordinary way.",
                        )
                        .clicked()
                    {
                        decided = true;
                    }
                    if ui
                        .button("Edit…")
                        .on_hover_text(
                            "Takes them, then opens the words for correcting by hand \u{2014} \
                             for a remix that keeps some of a lyric and not the rest.",
                        )
                        .clicked()
                    {
                        self.pending.push(Pending::TakeFoundWords(id, Box::new(found.clone())));
                        self.pending.push(Pending::EditWords(id));
                        decided = true;
                    }
                });
            });

        if decided || !open {
            self.wondering.remove(0);
        }
    }

    /// A track's words, open for correcting by hand.
    ///
    /// One line per row, which is how the collection keeps them and how a
    /// cue is placed. Times are not shown and not editable here: they are
    /// measured off the stem by the aligner, and a number typed in by hand
    /// would be the one thing in a cue set that nothing had checked.
    fn words_sheet(&mut self, ctx: &egui::Context) {
        let Some(editing) = &self.editing_words else { return };
        let id = editing.id;
        let mut text = editing.text.clone();
        let name = self
            .library
            .get(id)
            .map(|track| format!("{} \u{2014} {}", track.artist, track.display_title()))
            .unwrap_or_default();

        let mut open = true;
        let mut save = false;
        let mut cancel = false;
        egui::Window::new("The words")
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_width(560.0)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .max_height(sheet_height(ctx))
            .frame(sheet_frame())
            .show(ctx, |ui| {
                ui.label(RichText::new(&name).color(theme::text()));
                ui.label(
                    RichText::new(
                        "One line each, in the order they are sung. Where the track has a vocal \
                         stem these are placed against it after saving, so the times look after \
                         themselves.",
                    )
                    .color(theme::dim())
                    .size(theme::SMALL),
                );
                ui.add_space(8.0);
                egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                    ui.add(
                        egui::TextEdit::multiline(&mut text)
                            .desired_width(ui.available_width())
                            .desired_rows(14)
                            .font(theme::mono(11.0)),
                    )
                    .on_hover_text("The track's words, one line each.");
                });
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui
                        .add(
                            egui::Button::new(
                                RichText::new(theme::label_text("Save"))
                                    .size(11.0)
                                    .color(theme::booth())
                                    .strong(),
                            )
                            .fill(theme::amber()),
                        )
                        .on_hover_text("Keeps these words and cues the track from them again.")
                        .clicked()
                    {
                        save = true;
                    }
                    if ui.button("Cancel").on_hover_text("Leaves the words as they were.").clicked()
                    {
                        cancel = true;
                    }
                });
            });

        if let Some(editing) = &mut self.editing_words {
            editing.text = text;
        }
        // Saving leaves the words in place for the pending call to take, which
        // is what closes the sheet on that path.
        if save {
            self.pending.push(Pending::SaveWords(id));
        } else if cancel || !open {
            self.editing_words = None;
        }
    }

    /// Keep words somebody typed, and cue from them.
    fn save_edited_words(&mut self, id: u32) -> bool {
        let Some(editing) = self.editing_words.take() else { return false };
        let lines: Vec<String> = editing
            .text
            .lines()
            .map(|line| line.trim().to_string())
            .filter(|line| !line.is_empty())
            .collect();
        let Some(track) = self.library.get_mut(id) else { return false };

        // Times are kept where a line is still the line that was there — a
        // correction usually fixes a word, not the order — and cleared where
        // the shape changed, since a time carried onto a different line is a
        // cue in the wrong place. Either way the aligner has the last word
        // when there is a stem to measure.
        let same_shape = lines.len() == track.lyrics.len();
        let lyrics: Vec<crate::library::Lyric> = lines
            .into_iter()
            .enumerate()
            .map(|(at, text)| match same_shape {
                true => crate::library::Lyric { text, ..track.lyrics[at].clone() },
                false => crate::library::Lyric { start_ms: 0, end_ms: 0, text },
            })
            .collect();

        let count = lyrics.len();
        let placed = track.lyrics_aligned;
        track.refrains = crate::library::refrains_from(&lyrics);
        track.lyrics = lyrics;
        track.words_from = crate::library::WordsFrom::ByHand;
        // Nobody's doubt survives somebody typing the words in.
        track.heard_surely = None;
        track.instrumental = false;
        // The aligner is owed a pass only where there are times for it to
        // place: a correction that kept the order kept them, where one that
        // changed it has none, and the aligner moves a line by the time on it
        // — hand it a dozen lines at zero and it stacks them all on the
        // first word sung.
        track.lyrics_aligned = placed || !same_shape;
        crate::info!("#{id} took {} corrected by hand", plural(count, "line"));
        self.auto_cue(id);
        self.prep_changed(id);
        true
    }

    /// Carry out one correction to a track's grid.
    ///
    /// Everything downstream of the grid is re-derived rather than left to
    /// drift: the picture is drawn from the beats, the phrase strip is placed
    /// against them and the cues were put on them, so a grid that moves and a
    /// waveform that does not is a window telling two stories about one track.
    ///
    /// The cues are **not** re-placed. They are where somebody put them, and a
    /// grid correction is a statement about the bar lines rather than about
    /// the moments — moving them would mean a fix for a tempo quietly
    /// throwing away the set.
    ///
    /// Returns whether anything changed.
    fn fix_grid(&mut self, id: u32, what: GridFix, at: Option<u32>) -> bool {
        let Some(track) = self.library.get_mut(id) else { return false };
        let changed = match what {
            GridFix::Halve => crate::grid::halve(track),
            GridFix::Double => crate::grid::double(track),
            GridFix::Nudge(by) => crate::grid::nudge(track, by),
            GridFix::MoveTo => match at {
                Some(at) => crate::grid::move_to(track, at),
                None => false,
            },
            GridFix::Downbeat => match at {
                Some(at) => crate::grid::set_downbeat(track, at),
                None => false,
            },
            GridFix::Bpm(bpm) => crate::grid::set_bpm(track, bpm),
        };
        if !changed {
            return false;
        }
        let said = match what {
            GridFix::Halve | GridFix::Double => {
                format!("#{id} is now {:.2} BPM", self.library.get(id).map_or(0.0, |t| t.bpm))
            }
            GridFix::Nudge(by) => format!("#{id} grid moved {by} ms"),
            GridFix::MoveTo => format!("#{id} grid moved to the playhead"),
            GridFix::Downbeat => format!("#{id} downbeat set at the playhead"),
            GridFix::Bpm(_) => {
                format!("#{id} is now {:.2} BPM", self.library.get(id).map_or(0.0, |t| t.bpm))
            }
        };
        crate::info!("{said}");
        self.note(said, theme::text());
        // The picture is kept. It is three bands of loudness measured from the
        // audio — `export::waveform::analyze` is handed samples and nothing
        // else — so a grid correction cannot have made it wrong, and the beat
        // ticks and bar lines drawn over it come from `beat_times` every
        // frame, which is already reading the grid this just changed.
        //
        // Dropping it cost a re-decode of the whole file on every press, and
        // the frame in between had no picture to draw, which is what reset the
        // view: a panel with no bands reports a fitted zoom. Somebody
        // correcting a grid is zoomed in on the kicks they are correcting it
        // against, which is the one view it is least use to throw away.
        self.prep_changed(id);
        true
    }

    /// Say that a beat falls at this moment.
    ///
    /// Snapped to nothing: the whole point of a mark is that it disagrees with
    /// the grid, so snapping it to the grid would be snapping it to the thing
    /// it is there to correct. A mark close enough to one already placed
    /// replaces it rather than making a pair, because two marks a few
    /// milliseconds apart are a slip of the hand and would be read as a
    /// 20,000 BPM record.
    fn mark_beat(&mut self, id: u32, at_ms: u32) -> bool {
        let Some(track) = self.library.get_mut(id) else { return false };
        match track.beat_marks.iter().position(|mark| mark.abs_diff(at_ms) <= MARKS_APART_MS) {
            Some(at) => track.beat_marks[at] = at_ms,
            None => track.beat_marks.push(at_ms),
        }
        track.beat_marks.sort_unstable();
        let count = track.beat_marks.len();
        self.note(format!("#{id} has {}", plural(count, "beat mark")), theme::text());
        self.prep_changed(id);
        true
    }

    fn clear_marks(&mut self, id: u32) -> bool {
        let Some(track) = self.library.get_mut(id) else { return false };
        if track.beat_marks.is_empty() {
            return false;
        }
        track.beat_marks.clear();
        self.note(format!("#{id} beat marks cleared"), theme::text());
        self.prep_changed(id);
        true
    }

    /// Fit a grid through the marks.
    fn fit_grid(&mut self, id: u32) -> bool {
        let Some(track) = self.library.get_mut(id) else { return false };
        let marks = track.beat_marks.clone();
        if !crate::grid::fit(track, &marks) {
            self.note("those marks do not make a grid", theme::amber());
            return false;
        }
        let said = match self.library.get(id) {
            Some(track) => format!(
                "#{id} fitted to {:.2} BPM, {}",
                track.bpm,
                match crate::grid::is_dynamic(track) {
                    true => "bending",
                    false => "steady",
                }
            ),
            None => format!("#{id} fitted"),
        };
        crate::info!("{said}");
        self.note(said, theme::text());
        self.prep_changed(id);
        true
    }

    /// Turn a grid kept beat by beat into a tempo and a downbeat.
    fn flatten_grid(&mut self, id: u32) -> bool {
        let Some(track) = self.library.get_mut(id) else { return false };
        if !crate::grid::flatten(track) {
            self.note("that grid is already a tempo and a downbeat", theme::dim());
            return false;
        }
        let said = format!("#{id} is now steady at {:.2} BPM", track.bpm);
        crate::info!("{said}");
        self.note(said, theme::text());
        self.prep_changed(id);
        true
    }

    /// Measure one track's tempo and beats again, and change nothing else.
    ///
    /// Not [`App::analyze_tracks`], which is the right call for "listen to
    /// this record again" and the wrong one here: it rewrites the cues, the
    /// phrases and the key from the audio, so a DJ who had corrected a grid by
    /// hand, decided it was worse than what was measured, and pressed this to
    /// get the measurement back would lose every cue they had moved as the
    /// price. The same decode either way; only what comes back is narrower.
    fn regrid(&mut self, id: u32) {
        let waiting = self.files_for(&[id]);
        if waiting.is_empty() {
            self.note("that file is not where it was", theme::amber());
            return;
        }
        crate::info!("measuring the grid of #{id} again");
        self.ensure_local(&[id]);
        self.start(Job::Regrid(waiting));
    }

    /// The controls for correcting a beat grid by hand.
    ///
    /// A tracker is right about most records and wrong about a few, and the
    /// few are not random: half-time read at double, drum and bass read at
    /// half, and the one on the snare of anything with a backbeat. None of
    /// that is fixed by analysing again — the same audio gives the same
    /// answer — and all of it is a press or two for somebody looking at the
    /// waveform. A player has had these for twenty years, and a DJ who has
    /// used them there knows what they do here.
    ///
    /// They act at the playhead, because that is the only place the window
    /// knows you are pointing at. Without one, the two that need a position
    /// say so rather than guessing at the middle of the track.
    fn grid_tools(&mut self, ui: &mut Ui, track: &Track) {
        if !self.editing_grid {
            return;
        }
        let at = self.playhead_ms;
        let id = track.id;
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            pane_label(ui, "Grid");

            // The tempo, where it can be read and typed over. Here rather than
            // in the measurement row below, because a number somebody can type
            // into should not look like one that is only reported.
            //
            // Held in `bpm_entry` while it is being typed: read back from the
            // collection every frame, the "12" on the way to "128" would be a
            // tempo of twelve.
            if self.bpm_entry.0 != Some(id) {
                self.bpm_entry = (Some(id), format!("{:.2}", track.bpm));
            }
            // The caption goes before the box and names it: without this the
            // tempo is an unnamed text field in the accessibility tree, which
            // is what a screen reader — and anything else reading the window
            // through it — has to work from.
            let caption = ui.label(RichText::new("BPM").font(theme::mono(9.5)).color(theme::dim()));
            let typed = ui
                .add_enabled(
                    track.has_grid,
                    egui::TextEdit::singleline(&mut self.bpm_entry.1)
                        .desired_width(54.0)
                        .font(theme::mono(10.5)),
                )
                .on_hover_text(
                    "The tempo. Type one and press return — from the sleeve, from another \
                     program, or counted by hand.",
                );
            typed.clone().labelled_by(caption.id);
            // Return, or clicking away: both are somebody finishing. Not every
            // keystroke, which would set a tempo of 1, then 12, then 128 and
            // save the collection three times to get there.
            //
            // Cleared either way, so the field shows what the tempo *became*:
            // a number outside the rails is refused, and one left sitting in
            // the box would read as though it had been taken.
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                self.bpm_entry = (None, String::new());
            } else if typed.lost_focus() {
                if let Ok(bpm) = self.bpm_entry.1.trim().parse::<f64>() {
                    self.pending.push(Pending::FixGrid { id, what: GridFix::Bpm(bpm), at });
                }
                self.bpm_entry = (None, String::new());
            }

            if ui
                .add_enabled(!self.running(), egui::Button::new("re-measure").small())
                .on_hover_text(
                    "Listen to the record again and take the tempo and the beats it finds. \
                     Leaves the cues, the phrases and the key alone — unlike re-analysing.",
                )
                .on_disabled_hover_text("Something else is running")
                .clicked()
            {
                self.pending.push(Pending::Regrid(id));
            }
            ui.separator();

            let mut fix = |ui: &mut Ui, label: &str, hint: &str, what: GridFix| {
                let needs_playhead = what.needs_playhead() && at.is_none();
                let button = ui.add_enabled(!needs_playhead, egui::Button::new(label).small());
                let clicked = match needs_playhead {
                    true => button
                        .on_disabled_hover_text(
                            "Click the waveform first — this one acts where the playhead is",
                        )
                        .clicked(),
                    false => button.on_hover_text(hint).clicked(),
                };
                if clicked {
                    self.pending.push(Pending::FixGrid { id, what, at });
                }
            };

            // A tenth of a BPM is the unit a long mix drifts by: ten minutes
            // at 128.1 against 128.0 ends five beats apart.
            fix(
                ui,
                "−0.1",
                "Take a tenth of a BPM off the tempo.",
                GridFix::Bpm(by_a_tenth(track.bpm, -1.0)),
            );
            fix(
                ui,
                "+0.1",
                "Put a tenth of a BPM on the tempo.",
                GridFix::Bpm(by_a_tenth(track.bpm, 1.0)),
            );
            ui.separator();
            fix(
                ui,
                "÷2",
                "Halve the tempo. For a record counted on its hi-hats — 174 read as 348.",
                GridFix::Halve,
            );
            fix(
                ui,
                "×2",
                "Double the tempo. For a half-time record — 140 read as 70, with every bar \
                 line two beats out.",
                GridFix::Double,
            );
            ui.separator();
            fix(
                ui,
                "«",
                "Shove the whole grid earlier, ten nudges at a time.",
                GridFix::Nudge(-crate::grid::SHOVE_MS),
            );
            fix(ui, "‹", "Nudge the whole grid earlier.", GridFix::Nudge(-crate::grid::NUDGE_MS));
            fix(ui, "›", "Nudge the whole grid later.", GridFix::Nudge(crate::grid::NUDGE_MS));
            fix(
                ui,
                "»",
                "Shove the whole grid later, ten nudges at a time.",
                GridFix::Nudge(crate::grid::SHOVE_MS),
            );
            ui.separator();
            fix(
                ui,
                "Beat here",
                "Slide the grid until a beat lands on the playhead, keeping the tempo and \
                 which beat of the bar it is.",
                GridFix::MoveTo,
            );
            fix(
                ui,
                "One here",
                "Call the beat nearest the playhead a downbeat. Moves no beat — for a grid \
                 whose lines are right and whose bars start on the two.",
                GridFix::Downbeat,
            );

            ui.separator();

            // Fitting a grid through marked beats. One goes on wherever the
            // playhead is, or anywhere at all with alt-click on the waveform —
            // which is said on the button, because a modifier nobody is told
            // about is a feature nobody has.
            let marks = track.beat_marks.len();
            let mark = ui
                .add_enabled(at.is_some(), egui::Button::new("mark").small())
                .on_hover_text(
                    "Say a beat falls at the playhead. Alt-click the waveform to put one \
                     anywhere instead. They need not be next to each other: a run of them \
                     where the grid is plainly wrong, and one more further along, measures \
                     the tempo more finely than a run alone.",
                )
                .on_disabled_hover_text(
                    "Click the waveform first — this one marks where the playhead is",
                );
            if mark.clicked() {
                if let Some(at) = at {
                    self.pending.push(Pending::MarkBeat { id, at_ms: at });
                }
            }
            let fit = ui
                .add_enabled(marks > 0, egui::Button::new(format!("fit {marks}")).small())
                .on_hover_text(
                    "Fit a grid through the marked beats. A plain tempo and downbeat where \
                     one explains them, and a grid that bends only where none does.",
                )
                .on_disabled_hover_text("Mark where a beat falls first");
            if fit.clicked() {
                self.pending.push(Pending::FitGrid(id));
            }
            if ui
                .add_enabled(marks > 0, egui::Button::new("clear").small())
                .on_hover_text("Take the beat marks off again. The grid stays as it is.")
                .on_disabled_hover_text("Nothing is marked")
                .clicked()
            {
                self.pending.push(Pending::ClearMarks(id));
            }

            // Converting a grid that is kept beat by beat into one a tempo can
            // say. Only offered on a track that has one, because on every
            // other track the button would be a no-op with a confident name.
            if ui
                .add_enabled(
                    crate::grid::is_dynamic(track),
                    egui::Button::new("make steady").small(),
                )
                .on_hover_text(
                    "Replace the beat-by-beat grid with the one tempo that best fits it. \
                     For beats that were tracked untidily rather than a record that moves.",
                )
                .on_disabled_hover_text("This grid is already a tempo and a downbeat")
                .clicked()
            {
                self.pending.push(Pending::Flatten(id));
            }

            // The way out, in the row it closes. There is one in the
            // measurements line below as well, which is the one that opens
            // these — and for a while it was the only one, so when this row
            // pushed that line off the bottom of the window the tools could
            // be opened and not shut. A control that opens something is not
            // a safe place to keep the only way to shut it.
            ui.separator();
            if ui
                .add(egui::Button::new("done").small())
                .on_hover_text("Put the grid controls away")
                .clicked()
            {
                self.editing_grid = false;
            }
        });
    }

    fn adopt_sheet(&mut self, ctx: &egui::Context) {
        let waiting: Vec<(u32, String, PathBuf)> = self
            .asking
            .iter()
            .filter_map(|id| self.library.get(*id))
            .map(|track| {
                (track.id, format!("{} — {}", track.artist, track.title), track.path.clone())
            })
            .collect();
        if waiting.is_empty() {
            self.asking.clear();
            return;
        }

        let mut open = true;
        egui::Window::new(format!("{} outside the library", plural(waiting.len(), "track")))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(560.0)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .max_height(sheet_height(ctx))
            .frame(sheet_frame())
            .show(ctx, |ui| {
                ui.label(
                    RichText::new(
                        "These play from where they are now. A file on a stick, a share or \
                         a download folder is a file that can be gone on the night.",
                    )
                    .color(theme::dim())
                    .size(theme::SMALL),
                );
                ui.add_space(8.0);

                egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                    for (_, name, path) in &waiting {
                        ui.label(RichText::new(name).color(theme::text()).size(theme::SMALL));
                        ui.label(
                            RichText::new(path.display().to_string())
                                .font(theme::mono(9.5))
                                .color(theme::dim()),
                        );
                        ui.add_space(3.0);
                    }
                });

                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ui
                        .add(
                            egui::Button::new(
                                RichText::new(theme::label_text("Copy in"))
                                    .size(11.0)
                                    .color(theme::booth())
                                    .strong(),
                            )
                            .fill(theme::amber()),
                        )
                        .clicked()
                    {
                        let ids: Vec<u32> = waiting.iter().map(|(id, _, _)| *id).collect();
                        self.pending.extend(ids.into_iter().map(Pending::Adopt));
                    }
                    if ui.button("Leave them").clicked() {
                        self.asking.clear();
                    }
                    if ui
                        .button("Always copy")
                        .on_hover_text("And stop asking. Changeable in Settings.")
                        .clicked()
                    {
                        self.config.on_external = OnExternal::Copy;
                        let _ = self.config.save(&self.config_path);
                        let ids: Vec<u32> = waiting.iter().map(|(id, _, _)| *id).collect();
                        self.pending.extend(ids.into_iter().map(Pending::Adopt));
                    }
                    if ui
                        .button("Never ask")
                        .on_hover_text("Leave everything where it is, from now on.")
                        .clicked()
                    {
                        self.config.on_external = OnExternal::Leave;
                        let _ = self.config.save(&self.config_path);
                        self.asking.clear();
                    }
                });
            });
        if !open {
            self.asking.clear();
        }
    }

    /// The tracks changed here and on the player, and which copy to keep.
    ///
    /// Always asked rather than decided, even where the clocks are clear about
    /// which came later. A time is not a reason, and the one thing worse than
    /// losing an edit is losing it without being told — so the newer one is
    /// what each row starts on, and the person still has to look.
    fn clashes_section(&mut self, ui: &mut Ui) {
        let mut all: Option<sync::Side> = None;
        ui.label(
            RichText::new(format!(
                "{} {} changed here and on the player since this drive was written",
                theme::WARN,
                plural(self.clashes.len(), "track")
            ))
            .font(theme::mono(11.5))
            .color(theme::amber()),
        );
        ui.label(
            RichText::new(
                "Keeping the drive's leaves the track exactly as the deck left it: it is not \
                 prepared again, and its row and playlists carry through. Nothing here can read \
                 a player's edits back, so what it changed will not appear in the collection — \
                 which is why this asks rather than picking for you.",
            )
            .font(theme::mono(10.0))
            .color(theme::dim()),
        );
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.add_space(14.0);
            if ui
                .button("Keep all mine")
                .on_hover_text("Write the collection's version of every one of these, over what the player left.")
                .clicked()
            {
                all = Some(sync::Side::Mine);
            }
            if ui
                .button("Keep all the drive's")
                .on_hover_text("Leave every one of these as the player left it, and read it back into the collection.")
                .clicked()
            {
                all = Some(sync::Side::Theirs);
            }
        });
        ui.add_space(4.0);

        for clash in &self.clashes {
            let name = self
                .library
                .get(clash.id)
                .map(|track| format!("{} — {}", track.artist, track.display_title()))
                .unwrap_or_else(|| format!("#{}", clash.id));
            let side = self.settled.entry(clash.id).or_insert(sync::Side::Mine);
            ui.horizontal(|ui| {
                ui.add_space(14.0);
                ui.label(RichText::new(name).font(theme::mono(11.0)).color(theme::text()));
            });
            ui.horizontal(|ui| {
                ui.add_space(24.0);
                ui.radio_value(
                    side,
                    sync::Side::Mine,
                    format!("mine, {}", how_long_ago(clash.mine)),
                )
                .on_hover_text("Write the collection's cues and grid over what the player left.");
                ui.radio_value(
                    side,
                    sync::Side::Theirs,
                    format!("the drive's, {}", how_long_ago(clash.theirs)),
                )
                .on_hover_text("Keep what the player did, and read it back into the collection.");
            });
        }

        if let Some(side) = all {
            for clash in &self.clashes {
                self.settled.insert(clash.id, side);
            }
        }

        let leaving = self.settled.values().filter(|side| **side == sync::Side::Theirs).count();
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.add_space(14.0);
            ui.label(
                RichText::new(match leaving {
                    0 => "All of them will be written from the collection.".to_string(),
                    n => format!(
                        "{} will be left as the player left {} and not written.",
                        plural(n, "track"),
                        match n {
                            1 => "it",
                            _ => "them",
                        }
                    ),
                })
                .font(theme::mono(10.5))
                .color(theme::dim()),
            );
        });
    }

    fn sync_sheet(&mut self, ctx: &egui::Context) {
        let Some(drive) = self.library.drives.get(self.drive).cloned() else {
            self.sheet = false;
            return;
        };
        let formatted = sync::filesystem(&drive.path);
        let checks = sync::preflight(
            &self.library,
            &self.plan,
            &drive.path,
            drive.is_image,
            self.config.oldest_player,
            formatted.as_deref(),
        );
        let worst = checks.iter().map(|c| c.level).max().unwrap_or(Level::Ok);

        let mut open = true;
        let mut forget = false;
        let mut details = false;
        // Toggled here rather than applied in place: the sheet is drawing a
        // clone of the drive, and the plan under it was worked out before this
        // frame. Read back after the window closes, so one change redraws once.
        let mut with_stems = !drive.skip_stems;
        egui::Window::new(format!("SYNC → {}", drive.label))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(620.0)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .max_height(sheet_height(ctx))
            .frame(
                egui::Frame::NONE
                    .fill(theme::booth())
                    .stroke(egui::Stroke::new(1.0_f32, theme::rule()))
                    .inner_margin(egui::Margin::same(14)),
            )
            .show(ctx, |ui| {
                // Scrolled, because the sheet is no taller than the screen now.
                egui::ScrollArea::vertical().show(ui, |ui| {
                    let both = self.config.writes_onelibrary();
                    let target = self.config.oldest_player;
                    ui.label(
                        RichText::new(format!(
                            "Target: {} and newer · writes: Device Library (export.pdb){} \
                             + ANLZ .DAT/.EXT/.2EX{}",
                            target.name(),
                            if both { " + OneLibrary (exportLibrary.db)" } else { "" },
                            if drive.is_image { " · into a FAT32 image" } else { "" }
                        ))
                        .font(theme::mono(10.5))
                        .color(theme::dim()),
                    )
                    .on_hover_text(format!(
                        "Every generation's files go on whatever the target is: .DAT is what a \
                         2009 player reads, .EXT adds the nexus 2 line's colour waveforms and \
                         named cues, .2EX carries the CDJ-3000's three-band waveforms, and a \
                         player reads the richest one it knows and ignores the rest. What the \
                         target changes is what gets checked \u{2014} a {} takes {:.0} kHz and \
                         opens {}. It is in Settings.",
                        target.name(),
                        target.max_sample_rate() as f64 / 1000.0,
                        target.plays().join(", ")
                    ));
                    // The OneLibrary half only matters to the players that
                    // read it. Warning a DJ whose booth is full of NXS2s that a
                    // CDJ-3000X would not browse their drive is a warning about
                    // hardware they have said they do not have.
                    let needs_onelibrary = booth_cli::compat::Player::Cdj3000X;
                    if target.reads_onelibrary() || both {
                        ui.label(
                            RichText::new(match both {
                                // A CDJ-3000X has browsed one of these. What it
                                // did with the analysis files is not known, and
                                // the sheet should not let the first half stand
                                // in for the second.
                                true => format!(
                                    "{} A {} browses a drive written this way. Its waveforms \
                                     and grids have not been seen on a player since the \
                                     naming they are found by was corrected.",
                                    theme::WARN,
                                    needs_onelibrary.name()
                                ),
                                false => format!(
                                    "{} A {} will not read this drive: it needs OneLibrary, \
                                     which needs its key in Settings.",
                                    theme::WARN,
                                    needs_onelibrary.name()
                                ),
                            })
                            .font(theme::mono(10.5))
                            .color(match both {
                                true => theme::amber(),
                                false => theme::alert(),
                            }),
                        );
                    } else {
                        // Not a tick. Everything a player of this generation
                        // reads is written — export.pdb and all three analysis
                        // files — and no player older than a CDJ-3000X has been
                        // seen to browse a drive from here. One on firmware
                        // 2.05 refused the database outright, and that is not
                        // known to have been fixed, so the sheet says so rather
                        // than letting "the files are there" stand in for "it
                        // works". See booth/README.md.
                        ui.label(
                            RichText::new(format!(
                                "{} A {} reads export.pdb and the analysis files, which all go \
                                 on \u{2014} but no player older than a {} has yet been seen to \
                                 browse a drive written here. Try it before the night.",
                                theme::WARN,
                                target.name(),
                                needs_onelibrary.name()
                            ))
                            .font(theme::mono(10.5))
                            .color(theme::amber()),
                        );
                    }

                    // What the browse tree on the player will look like, because
                    // that is the thing being written and the easiest to get wrong.
                    let specs = self.drive_playlists(&drive);
                    let line = match specs.is_empty() {
                        true => "no playlists — nothing will be written".to_string(),
                        false => specs
                            .iter()
                            .map(|p| match p.folder.is_empty() {
                                true => format!("{} ({})", p.name, p.tracks.len()),
                                false => format!("{}/{} ({})", p.folder, p.name, p.tracks.len()),
                            })
                            .collect::<Vec<_>>()
                            .join("  ·  "),
                    };
                    ui.label(
                        RichText::new(format!("Playlists: {line}"))
                            .font(theme::mono(10.5))
                            .color(if specs.is_empty() { theme::alert() } else { theme::dim() }),
                    );
                    ui.add_space(10.0);

                    sheet_line(
                        ui,
                        "Add",
                        &self.names(&self.plan.add),
                        &sync::bytes(self.plan.add_bytes),
                    );
                    sheet_line(
                        ui,
                        "Update",
                        &self.update_text(),
                        if self.plan.update.is_empty() { "—" } else { "analysis only" },
                    );
                    sheet_line(ui, "Remove", &self.names(&self.plan.remove), "—");
                    sheet_line(
                        ui,
                        "Stems",
                        &match with_stems {
                            false => "held back".to_string(),
                            true => match (self.plan.new_stems.len(), self.plan.stems.len()) {
                                (0, 0) => "none rendered".to_string(),
                                (0, on) => format!("{on} already on"),
                                (n, _) => format!(
                                    "{n} files from {}",
                                    plural(self.kits_going_on(), "track")
                                ),
                            },
                        },
                        &sync::bytes(self.plan.stem_bytes),
                    );
                    ui.horizontal(|ui| {
                        ui.add_space(74.0);
                        let changes = self.plan.add.len()
                            + self.plan.update.len()
                            + self.plan.remove.len()
                            + self.plan.new_stems.len();
                        if ui
                            .add_enabled(changes > 0, egui::Button::new("Details"))
                            .on_hover_text(
                                "Every change this write would make, one to a line. The                                  summary above names three of each; this names all of them,                                  which is what tells you whether the four it is about to                                  remove are the four you meant.",
                            )
                            .on_disabled_hover_text("Nothing to write, so there is nothing to list.")
                            .clicked()
                        {
                            details = true;
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.add_space(74.0);
                        ui.checkbox(&mut with_stems, "Carry stems").on_hover_text(
                            "On by default: a kit that has been rendered goes on the drive \
                             beside its track, in the same folder and next to it in the \
                             playlist. Only tracks with a kit are affected. Turn it off for a \
                             smaller stick — three files a track, and the same again in \
                             analysis.",
                        );
                    });

                    ui.add_space(12.0);
                    for check in &checks {
                        ui.horizontal(|ui| {
                            let (mark, color) = match check.level {
                                Level::Ok => (theme::TICK, theme::go()),
                                Level::Warn => (theme::WARN, theme::amber()),
                                Level::Bad => (theme::CROSS, theme::alert()),
                            };
                            ui.label(RichText::new(mark).color(color).font(theme::mono(11.5)));
                            ui.label(
                                RichText::new(&check.text)
                                    .font(theme::mono(11.5))
                                    .color(theme::text()),
                            );
                        });
                    }

                    if !self.clashes.is_empty() {
                        ui.add_space(12.0);
                        self.clashes_section(ui);
                    }

                    ui.add_space(6.0);
                    ui.label(
                    RichText::new(
                        "The database and every analysis file are read back off the drive after \
                         writing, by a parser that shares no code with the writer. Until that \
                         passes, this is not a finished drive.",
                    )
                    .font(theme::mono(10.0))
                    .color(theme::dim()),
                );

                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        let verb = if worst == Level::Bad { "Write anyway" } else { "Write" };
                        let nothing = self.plan.is_empty();
                        if ui
                            .add_enabled(
                                !nothing,
                                egui::Button::new(
                                    RichText::new(theme::label_text(verb))
                                        .size(11.0)
                                        .color(theme::booth())
                                        .strong(),
                                )
                                .fill(if worst == Level::Bad {
                                    theme::alert()
                                } else {
                                    theme::amber()
                                }),
                            )
                            .on_disabled_hover_text(
                                "This drive already holds what its playlists say — write it all \
                                 again to put it on from scratch",
                            )
                            .clicked()
                        {
                            self.write_drive();
                        }
                        if ui.button("Cancel").clicked() {
                            self.sheet = false;
                        }
                        // Forgetting what is on the drive rather than writing
                        // straight away: the plan and the space check above
                        // redraw as soon as it is pressed, so the size of what
                        // was just asked for is on screen before Write is. A
                        // whole drive prepared again is minutes to hours, and
                        // that is not a thing to start without seeing it.
                        if !drive.written.is_empty()
                            && ui
                                .button("Write it all again")
                                .on_hover_text(
                                    "Forget what this drive is holding, so the next write                                      prepares every track afresh — for a drive something else                                      has been at, or one whose database is not to be trusted",
                                )
                                .clicked()
                        {
                            forget = true;
                        }
                        if worst == Level::Bad {
                            ui.label(
                                RichText::new("the files above will be skipped; the rest still go")
                                    .color(theme::dim())
                                    .size(theme::SMALL),
                            );
                        }
                    });
                });
            });
        if with_stems == drive.skip_stems {
            if let Some(drive) = self.library.drives.get_mut(self.drive) {
                drive.skip_stems = !with_stems;
            }
            // The plan changes by three files a track, and so does the space
            // check under it. Both are on screen, so both are redone now rather
            // than on whatever happens next.
            self.replan();
            self.save();
        }
        if details {
            self.details = true;
        }
        if forget {
            self.forget_drive_contents();
        }
        if !open {
            self.sheet = false;
        }
    }

    fn names(&self, ids: &[u32]) -> String {
        if ids.is_empty() {
            return "none".to_string();
        }
        let names: Vec<String> = ids
            .iter()
            .filter_map(|id| self.library.get(*id))
            .take(3)
            .map(|track| track.artist.clone())
            .collect();
        match ids.len() {
            n if n <= 3 => format!("{} — {}", plural(n, "track"), names.join(", ")),
            n => format!("{} — {} +{}", plural(n, "track"), names.join(", "), n - 3),
        }
    }

    /// Every change the next write would make, one to a line.
    ///
    /// The summary above it says how many and names three; this says which,
    /// which is the question somebody opens it with — a drive about to remove
    /// four tracks is fine or a disaster depending entirely on which four.
    ///
    /// A track that has since left the collection still gets a line: it is
    /// exactly the case worth seeing, and a row that quietly vanished from the
    /// list would be the write doing something the list did not mention.
    fn change_lines(&self) -> Vec<(String, egui::Color32)> {
        let named = |id: u32| match self.library.get(id) {
            Some(track) => format!("{} \u{2014} {}", track.artist, track.display_title()),
            None => format!("#{id}, no longer in the collection"),
        };
        let mut lines = Vec::new();
        for id in &self.plan.add {
            let size = self.library.get(*id).map(|track| track.bytes).unwrap_or(0);
            lines.push((format!("+  {}  ({})", named(*id), sync::bytes(size)), theme::go()));
        }
        for (id, what) in &self.plan.update {
            lines.push((format!("~  {}  \u{2014} {what}", named(*id)), theme::amber()));
        }
        for id in &self.plan.remove {
            lines.push((format!("\u{2212}  {}", named(*id)), theme::alert()));
        }
        for (id, path) in &self.plan.new_stems {
            let file = path.file_name().unwrap_or_default().to_string_lossy();
            lines.push((format!("\u{266a}  {file}  \u{2014} with {}", named(*id)), theme::blue()));
        }
        lines
    }

    /// The whole list of changes, scrolled and wrapped, one to a line.
    fn details_window(&mut self, ctx: &egui::Context) {
        let lines = self.change_lines();
        let mut open = true;
        egui::Window::new("CHANGES")
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_width(620.0)
            .default_height(420.0)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .frame(
                egui::Frame::NONE
                    .fill(theme::booth())
                    .stroke(egui::Stroke::new(1.0_f32, theme::rule()))
                    .inner_margin(egui::Margin::same(14)),
            )
            .show(ctx, |ui| {
                ui.label(
                    RichText::new(format!(
                        "{} the next write would make, in the order it makes them.",
                        plural(lines.len(), "change")
                    ))
                    .color(theme::dim())
                    .size(theme::SMALL),
                );
                ui.add_space(8.0);
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    // Wrapped rather than cut off: a long title with a long
                    // reason after it is exactly the line worth reading, and
                    // it is the one that would run off the edge.
                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
                    for (line, color) in &lines {
                        ui.label(RichText::new(line).font(theme::mono(11.0)).color(*color));
                    }
                    if lines.is_empty() {
                        ui.label(
                            RichText::new("Nothing to write.")
                                .font(theme::mono(11.0))
                                .color(theme::dim()),
                        );
                    }
                });
            });
        if !open {
            self.details = false;
        }
    }

    fn update_text(&self) -> String {
        match self.plan.update.len() {
            0 => "none".to_string(),
            _ => {
                let first = &self.plan.update[0];
                let name = self
                    .library
                    .get(first.0)
                    .map(|t| format!("{} \u{201c}{}\u{201d}", t.artist, t.title))
                    .unwrap_or_default();
                format!("{} — {name}: {}", plural(self.plan.update.len(), "track"), first.1)
            }
        }
    }

    // -- pickers -----------------------------------------------------------

    fn pick(&mut self, what: Picking, ctx: &egui::Context) {
        if self.pick.is_some() {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        self.pick = Some((what, rx));
        let ctx = ctx.clone();

        std::thread::spawn(move || {
            let dialog = rfd::AsyncFileDialog::new();
            let paths: Vec<PathBuf> = match what {
                Picking::Music => block_on(dialog.set_title("Add music").pick_folders())
                    .unwrap_or_default()
                    .iter()
                    .map(|f| f.path().to_path_buf())
                    .collect(),
                Picking::Drive => block_on(dialog.set_title("Choose the drive").pick_folder())
                    .into_iter()
                    .map(|f| f.path().to_path_buf())
                    .collect(),
                Picking::LibraryFolder => {
                    block_on(dialog.set_title("Where music is kept").pick_folder())
                        .into_iter()
                        .map(|f| f.path().to_path_buf())
                        .collect()
                }
                Picking::Rekordbox => block_on(
                    dialog
                        .set_title("rekordbox master.db")
                        .add_filter("rekordbox library", &["db"])
                        .pick_file(),
                )
                .into_iter()
                .map(|f| f.path().to_path_buf())
                .collect(),
                Picking::Image => block_on(
                    dialog
                        .set_title("Write the drive image to")
                        .set_file_name("drive.img")
                        .add_filter("Disk image", &["img"])
                        .save_file(),
                )
                .into_iter()
                .map(|f| f.path().to_path_buf())
                .collect(),
            };
            let _ = tx.send(paths);
            ctx.request_repaint();
        });
    }

    fn collect_picked(&mut self) {
        let Some((what, rx)) = &self.pick else { return };
        let what = *what;
        let Ok(paths) = rx.try_recv() else { return };
        self.pick = None;

        match what {
            Picking::Music if !paths.is_empty() => self.import(paths),
            Picking::Rekordbox => {
                if let Some(path) = paths.into_iter().next() {
                    self.import_rekordbox(path);
                }
            }
            Picking::Drive => {
                if let Some(path) = paths.into_iter().next() {
                    self.add_drive(path, false);
                }
            }
            Picking::Image => {
                if let Some(path) = paths.into_iter().next() {
                    self.add_drive(path, true);
                }
            }
            Picking::LibraryFolder => {
                if let Some(path) = paths.into_iter().next() {
                    self.config.library_path = path;
                    if let Err(e) = self.config.save(&self.config_path) {
                        self.note(format!("could not save the settings: {e:#}"), theme::alert());
                    }
                }
            }
            Picking::Music => {}
        }
    }
}

fn block_on<T>(future: impl std::future::Future<Output = T>) -> T {
    async_std::task::block_on(future)
}

fn pane_label(ui: &mut Ui, text: &str) {
    ui.label(
        RichText::new(theme::label_text(text)).size(theme::LABEL).color(theme::dim()).strong(),
    );
    ui.add_space(4.0);
}

fn measurement(ui: &mut Ui, name: &str, value: &str, good: bool) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        ui.label(RichText::new(name).font(theme::mono(11.0)).color(theme::dim()));
        ui.label(
            RichText::new(if good { theme::TICK } else { "·" })
                .font(theme::mono(11.0))
                .color(if good { theme::go() } else { theme::amber() }),
        );
        ui.label(RichText::new(value).font(theme::mono(11.0)).color(if good {
            theme::go()
        } else {
            theme::amber()
        }));
    });
}

fn grid_text(track: &Track, length: crate::config::Length) -> String {
    if !track.analyzed {
        return "not analysed".to_string();
    }
    if !track.has_grid {
        return "none found".to_string();
    }
    // Said rather than left to be inferred from a number that cannot show it.
    // Whether a record holds its tempo is the question behind "will this loop
    // still be in time in eight bars", and the collection already knows the
    // answer by what it chose to keep.
    let shape = match crate::grid::is_dynamic(track) {
        true => "dynamic",
        false => "steady",
    };
    format!("{:.2} · {shape} · {}", track.bpm, length.describe(track.beats))
}

/// A tempo a tenth of a BPM up or down, kept to two places.
///
/// Rounded because the alternative accumulates: ten presses of up from 128.00
/// in plain float arithmetic is 128.99999999999997, which reads as 129.00,
/// compares as neither, and is a different grid from the one the readout
/// claims.
fn by_a_tenth(bpm: f64, direction: f64) -> f64 {
    ((bpm + direction * 0.1) * 100.0).round() / 100.0
}

/// A track's beat times: the ones it kept, or rebuilt from its tempo and its
/// first cue.
///
/// An even grid is not kept in the collection — it is thousands of numbers per
/// track, and the analysis file on the drive is where it belongs. What the
/// picture needs is where the bars fall, and a constant tempo from the first
/// downbeat gives that.
///
/// Wound back in whole bars rather than whole beats, so that beat zero is a
/// downbeat on the same bar as the anchor. Winding back a beat at a time
/// put beat zero on whichever beat happened to fall nearest the top of the
/// track, and everything that counts in fours — the red bar marks, the bar
/// number in the transport, the length on the phrase strip — then counted from
/// a beat that was not a downbeat at all. It only looked right on a track that
/// happens to start on one.
///
/// The cost is that a track starting part way through a bar has that part bar
/// unmarked, which is the right way round: a player numbers from the first
/// whole bar too, and three marks in the wrong colour is the worse trade.
fn beat_times(track: &Track) -> Vec<u32> {
    if !track.has_grid || track.bpm <= 0.0 || track.duration_secs <= 0.0 {
        return Vec::new();
    }
    // A grid that bends is kept beat for beat, because no tempo puts those
    // beats back. It already starts on a downbeat, which is the one thing
    // everything below counts on.
    if !track.beat_ms.is_empty() {
        return track.beat_ms.clone();
    }
    let period_ms = 60_000.0 / track.bpm;
    let first =
        track.downbeat_ms.or_else(|| track.cues.first().map(|cue| cue.time_ms)).unwrap_or(0) as f64;
    let start = first % (period_ms * crate::library::BEATS_PER_BAR as f64);
    let count = ((track.duration_secs * 1000.0 - start) / period_ms).floor().max(0.0) as usize;
    (0..count).map(|i| (start + i as f64 * period_ms).round() as u32).collect()
}

/// Which beat a moment in a track falls on, counting the first as zero.
///
/// `None` when there is no grid to count against: a track nobody has analysed
/// has no bars, and inventing some from a tempo of zero would be worse than
/// showing the clock.
fn beat_at(track: &Track, at_ms: u32) -> Option<usize> {
    if !track.has_grid || track.bpm <= 0.0 {
        return None;
    }
    let beats = beat_times(track);
    if beats.is_empty() {
        return None;
    }
    // Found in the grid rather than divided out of the tempo. They are the
    // same answer on an even grid and different ones on a grid that bends,
    // and the grid is the thing the marks are drawn from.
    let after = beats.partition_point(|beat| *beat <= at_ms);
    Some(after.saturating_sub(1))
}

/// Write down what the words decided, and why.
///
/// A cue placed off a transcript is a cue with a reason, and the reason is the
/// only thing that makes it arguable. The recogniser mishears, the same sung
/// line comes back written four ways, and the hook is picked off the top of a
/// ranking — so a DJ who finds cue B in the wrong place needs to be able to see
/// that it was put there because a line it heard as something else came round
/// three times. Without that, a wrong cue is indistinguishable from a bug.
///
/// Three things, in the order they were decided: every line the track comes
/// back to and how often, which of them the hook is, and what that moved.
///
/// Returned rather than logged from inside, so that what it says can be read
/// back by a test instead of being asserted against a global log buffer that
/// every other test is writing to at the same time.
fn what_the_words_decided(
    words: &booth_cli::transcribe::Transcript,
    before: &[crate::library::CueMark],
    after: &[crate::library::CueMark],
) -> Vec<String> {
    /// Enough of the ranking to see where it falls off, without a log that is
    /// one track long. A hook is never far from the top of it.
    const LISTED: usize = 8;

    let mut said = Vec::new();
    let refrains = words.refrains();
    if refrains.is_empty() {
        said.push("nothing was made out that is long enough to be a line".to_string());
    } else {
        said.push("lines by how often they come round:".to_string());
        for refrain in refrains.iter().take(LISTED) {
            let at: Vec<String> = refrain.at.iter().map(|at| time_text(*at)).collect();
            said.push(format!(
                "  {}\u{d7} \u{201c}{}\u{201d} \u{2014} {}",
                refrain.times(),
                refrain.text,
                at.join(", ")
            ));
        }
        if refrains.len() > LISTED {
            said.push(format!("  \u{2026}and {} more", refrains.len() - LISTED));
        }
    }

    said.push(match words.hook() {
        Some(hook) => format!(
            "hook: \u{201c}{}\u{201d}, sung {} times, first at {}",
            hook.text,
            hook.times(),
            time_text(hook.at.first().copied().unwrap_or(0))
        ),
        // Not a failure. A track can have words and no chorus, and saying so
        // is what stops somebody looking for a hook cue that was never going
        // to be there.
        None => "no line comes round twice, so the cues are the arrangement's alone".to_string(),
    });

    let reading = |cue: &crate::library::CueMark| match cue.label.is_empty() {
        true => time_text(cue.time_ms),
        false => format!("{} {}", time_text(cue.time_ms), cue.label),
    };
    let was = said.len();
    for cue in after {
        match before.iter().find(|old| old.letter == cue.letter) {
            Some(old) if old.time_ms == cue.time_ms && old.label == cue.label => {}
            Some(old) => {
                said.push(format!("{} {} \u{2192} {}", cue.name(), reading(old), reading(cue)))
            }
            None => said.push(format!("{} set to {}", cue.name(), reading(cue))),
        }
    }
    for old in before {
        if !after.iter().any(|cue| cue.letter == old.letter) {
            said.push(format!("{} cleared, was {}", old.name(), reading(old)));
        }
    }
    if said.len() == was {
        said.push("every cue is already where the words put it".to_string());
    }
    said
}

/// Take the cues that mark a boundary with it when it moves.
///
/// A cue that marks the drop is a cue for wherever the drop is. Dragging the
/// line and leaving the cue behind makes the two disagree about the same
/// moment, and the cue is the one somebody presses in a booth.
///
/// Only the cues that mark the section, which is what the name says: a cue
/// called "drop" or "break" is about the arrangement. One named for what is
/// sung there — a hook, a vocal entry — is about a moment in the music that has
/// not moved, and dragging it along would put it somewhere nothing happens.
///
/// `within` is how close a cue has to be to count as marking the boundary. A
/// beat: a section cue is snapped to the bar and so is the line, so they should
/// be on the same moment, and anything a whole beat away is marking something
/// else.
///
/// Returns how many moved.
fn drag_cues(cues: &mut [crate::library::CueMark], from: u32, to: u32, within: u32) -> usize {
    if from == to {
        return 0;
    }
    let mut moved = 0;
    for cue in cues.iter_mut() {
        let marks_a_section = crate::library::Phrase::KINDS.contains(&cue.label.as_str());
        if marks_a_section && cue.time_ms.abs_diff(from) <= within {
            cue.time_ms = to;
            moved += 1;
        }
    }
    moved
}

/// Whether this is a row this program draws a picture for at all.
///
/// A record has to have been analysed first. Arrowing down a library of
/// thousands of unprepared tracks should not start a decode per row, and a
/// track nobody has listened to is one nobody has asked anything of.
///
/// A stem row is exempt, and the exemption is the point rather than a special
/// case: a companion exists only because somebody rendered it, which is a more
/// deliberate act than analysing, not a less deliberate one. Waiting for the
/// record to be analysed before drawing the stems that were explicitly asked
/// for is asking twice for one thing.
fn worth_drawing(track: &Track) -> bool {
    track.analyzed || track.role != crate::library::Role::Track
}

/// The audio a row's picture is measured from, if it is all there.
///
/// A row's own files, which for a stem companion are its stems and not the mix
/// its parent points at: a companion is a clone of its parent with a different
/// role, so `path` is the record's file and drawing from it put the whole mix
/// under the acapella's name.
///
/// Only the picture works this way. Everything else a companion shows — the
/// grid, the cues, the phrases, the key — stays the parent's, and should: those
/// are properties of the record, and a cue that did not line up with the one on
/// the track would be worse than no cue at all. The waveform is the one thing
/// that is a property of the file, because it is a drawing of what will come
/// out of the speaker.
///
/// `None` when there is nothing to draw from or a file is missing — a blank
/// picture looks exactly like a quiet one, so it is better not to cache it.
fn picture_sources(track: &Track) -> Option<Vec<PathBuf>> {
    let sources = track.sources();
    let all_there = !sources.is_empty() && sources.iter().all(|path| path.exists());
    all_there.then_some(sources)
}

/// How long ago a moment was, in the roughest terms that are still useful.
///
/// For putting two edits beside each other and saying which came later, which
/// is a comparison nobody makes in seconds. An unknown time says so rather than
/// pretending to be the epoch.
fn how_long_ago(at: Option<u64>) -> String {
    let Some(at) = at else { return "at some point".to_string() };
    let seconds = crate::library::now().saturating_sub(at);
    match seconds {
        0..=90 => "just now".to_string(),
        91..=5_400 => format!("{} minutes ago", seconds / 60),
        5_401..=172_800 => format!("{} hours ago", seconds / 3_600),
        _ => format!("{} days ago", seconds / 86_400),
    }
}

/// A position in a track, as minutes, seconds and hundredths.
pub(crate) fn time_text(ms: u32) -> String {
    let total = ms / 1000;
    format!("{}:{:02}.{:02}", total / 60, total % 60, (ms % 1000) / 10)
}

/// The nearest beat to a time, counting only every `every`th one.
///
/// Falls back to the time itself when there is no grid: a track nobody has
/// analysed can still be marked up, and refusing to place a cue because the
/// tracker found no beats would be the wrong way round.
fn snap_to(beats: &[u32], time_ms: u32, every: usize) -> u32 {
    let candidates: Vec<u32> = beats.iter().step_by(every.max(1)).copied().collect();
    candidates.iter().min_by_key(|beat| beat.abs_diff(time_ms)).copied().unwrap_or(time_ms)
}

/// The last beat at or before a moment, rather than the nearest one.
///
/// For a cue placed off the words. A sung line rarely starts on the beat — a
/// pickup is the whole point of a pickup — and a recogniser's idea of where a
/// line starts is already a little late, because it trims the breath before it.
/// Rounding to the nearest beat can therefore land after the first word, and a
/// hook cue that clips its own first word is one nobody presses twice. So this
/// rounds down. Falls back to the moment itself where there is no grid.
fn snap_back(beats: &[u32], time_ms: u32) -> u32 {
    beats.iter().rev().find(|beat| **beat <= time_ms).copied().unwrap_or(time_ms)
}

fn sheet_line(ui: &mut Ui, operation: &str, what: &str, size: &str) {
    ui.horizontal(|ui| {
        ui.allocate_ui_with_layout(
            egui::vec2(88.0, 18.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.label(
                    RichText::new(theme::label_text(operation))
                        .size(theme::LABEL)
                        .color(theme::dim())
                        .strong(),
                );
            },
        );
        ui.allocate_ui_with_layout(
            egui::vec2(ui.available_width() - 100.0, 18.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.label(RichText::new(what).color(theme::text()));
            },
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(RichText::new(size).font(theme::mono(11.5)).color(theme::dim()));
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::CueMark;

    /// What the sweep does with a drive it has seen before.
    ///
    /// The bug these are here for: a drive whose state kept coming out
    /// different was copied every four seconds for as long as it stayed
    /// plugged in, because the only thing standing between a changed state and
    /// a copy was whether that exact state had been copied already.
    mod keeping_drives {
        use super::*;

        fn seen(fingerprint: &str, ago: std::time::Duration) -> Seen {
            Seen {
                fingerprint: fingerprint.to_string(),
                listing: vec!["rekordbox/export.pdb:1:2".to_string()],
                at: std::time::Instant::now() - ago,
                complained: false,
            }
        }

        #[test]
        fn a_drive_never_seen_before_is_copied() {
            assert_eq!(worth_keeping(None, "abc"), Worth::Yes);
        }

        #[test]
        fn the_same_drive_unchanged_is_left_alone() {
            let before = seen("abc", std::time::Duration::from_secs(0));
            assert_eq!(worth_keeping(Some(&before), "abc"), Worth::No);
        }

        #[test]
        fn a_drive_that_changed_again_at_once_is_not_copied_again_at_once() {
            let before = seen("abc", std::time::Duration::from_secs(1));
            assert_eq!(worth_keeping(Some(&before), "def"), Worth::NotYet);
        }

        #[test]
        fn a_drive_that_changed_long_after_being_copied_is_copied() {
            let before = seen("abc", SETTLE + std::time::Duration::from_secs(1));
            assert_eq!(worth_keeping(Some(&before), "def"), Worth::Yes);
        }

        #[test]
        fn a_drive_is_called_the_same_thing_by_both_the_sweep_and_a_write() {
            // Two names would mean two folders of copies for one stick, and a
            // copy stored under one name that the other would never find.
            let root = std::path::Path::new("/Volumes/MY STICK");
            assert_eq!(drive_name(root, ""), "MY STICK");
            assert_eq!(drive_name(root, "MY STICK"), drive_name(root, ""));
            assert_eq!(drive_name(root, "something else"), "MY STICK");
            assert_eq!(drive_name(std::path::Path::new("/"), "LABEL"), "LABEL");
            assert_eq!(drive_name(std::path::Path::new("/"), ""), "drive");
        }
    }

    /// How long a wait is described as, for the sheet that says what a batch
    /// of separations will cost.
    mod saying_how_long {
        use super::*;

        #[test]
        fn a_short_wait_is_not_described_in_seconds_it_does_not_know() {
            assert_eq!(roughly(20.0), "under a minute");
            assert_eq!(roughly(64.0), "about a minute");
            assert_eq!(roughly(7.0 * 60.0), "about 7 minutes");
        }

        #[test]
        fn an_hour_is_an_hour_rather_than_sixty_minutes() {
            assert_eq!(roughly(3_600.0), "about an hour");
            assert_eq!(roughly(3_600.0 + 20.0 * 60.0), "about an hour and 20 minutes");
            assert_eq!(roughly(3.0 * 3_600.0), "about 3 hours");
            assert_eq!(roughly(2.0 * 3_600.0 + 60.0), "about 2 hours and a minute");
        }
    }

    mod saying_what_the_grid_is {
        use super::*;

        /// A four-minute record at 128, gridded from the top.
        fn gridded() -> Track {
            let mut track = Track::placeholder(1);
            track.duration_secs = 240.0;
            track.bpm = 128.0;
            track.beats = 512;
            track.has_grid = true;
            track.analyzed = true;
            track
        }

        #[test]
        fn the_readout_says_whether_the_tempo_moves() {
            // The question behind "will this loop still be in time in eight
            // bars", and one no tempo on its own can answer.
            let steady = gridded();
            assert_eq!(
                grid_text(&steady, crate::config::Length::Beats),
                "128.00 · steady · 512 beats"
            );

            let mut dynamic = gridded();
            dynamic.beat_ms = vec![0, 469, 938];
            assert!(
                grid_text(&dynamic, crate::config::Length::Beats).contains("dynamic"),
                "a track keeping its own beats is the dynamic one"
            );
        }

        #[test]
        fn a_track_with_nothing_measured_says_so_rather_than_claiming_a_tempo() {
            let mut track = gridded();
            track.analyzed = false;
            assert_eq!(grid_text(&track, crate::config::Length::Beats), "not analysed");

            let mut no_grid = gridded();
            no_grid.has_grid = false;
            assert_eq!(grid_text(&no_grid, crate::config::Length::Beats), "none found");
        }

        #[test]
        fn ten_presses_of_a_tenth_land_on_a_whole_number() {
            // In plain float arithmetic they land on 128.99999999999997, which
            // reads as 129.00, compares as neither, and is a different grid
            // from the one the readout claims.
            let mut bpm = 128.0;
            for _ in 0..10 {
                bpm = by_a_tenth(bpm, 1.0);
            }
            assert_eq!(bpm, 129.0);
            for _ in 0..10 {
                bpm = by_a_tenth(bpm, -1.0);
            }
            assert_eq!(bpm, 128.0);
        }
    }

    /// Driving the sidebar the way a person does: click, type, press a key,
    /// and see what the collection holds afterwards.
    ///
    /// Through `egui_kittest`, which runs real frames and finds controls by the
    /// name they announce. That is what it takes: the bug these were written
    /// for left every function involved correct on its own, and only showed up
    /// as the field never reporting that Enter had been pressed. Nothing short
    /// of two frames with a keystroke between them sees it.
    mod window {
        use super::*;
        use egui_kittest::kittest::Queryable;
        use egui_kittest::Harness;

        /// A window on an empty collection, writing to a scratch directory so a
        /// save cannot land on the collection of whoever runs the tests.
        fn app(name: &str) -> App {
            let dir = std::env::temp_dir().join(format!("booth-ui-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            App::assemble(
                Library::new(),
                dir.join("library.json"),
                Config::default(),
                dir.join("config.json"),
                String::new(),
            )
        }

        /// Dragging a phrase boundary takes the cues that mark it along.
        mod moving_a_boundary {
            use super::*;
            use crate::library::{CueMark, Phrase, PhraseEdit};

            fn cue(letter: u8, time_ms: u32, label: &str) -> CueMark {
                CueMark { letter, time_ms, label: label.to_string(), color: [0; 3] }
            }

            /// 120 BPM, so a beat is 500 ms and a bar is two seconds.
            fn with_a_drop(name: &str) -> (App, u32) {
                let mut app = app(name);
                let id = app.library.add(std::path::Path::new("/music/track.flac"));
                let track = app.library.get_mut(id).unwrap();
                track.bpm = 120.0;
                track.has_grid = true;
                track.analyzed = true;
                track.duration_secs = 300.0;
                track.beats = 600;
                track.phrases = vec![
                    Phrase { start_ms: 0, end_ms: 32_000, kind: "intro".into() },
                    Phrase { start_ms: 32_000, end_ms: 96_000, kind: "drop".into() },
                    Phrase { start_ms: 96_000, end_ms: 160_000, kind: "break".into() },
                ];
                track.cues = vec![
                    cue(0, 0, ""),
                    cue(1, 32_000, "drop"),
                    cue(2, 40_000, "hold me closer now"),
                    cue(3, 96_000, "break"),
                ];
                app.rebuild();
                (app, id)
            }

            #[test]
            fn a_cue_that_marks_the_drop_goes_where_the_drop_goes() {
                // Otherwise the line and the cue disagree about the same
                // moment, and the cue is the one somebody presses in a booth.
                let (mut app, id) = with_a_drop("dragged");
                assert!(app.edit_phrase(id, PhraseEdit::Move { at: 1, time_ms: 40_000 }));

                let cues = &app.library.get(id).unwrap().cues;
                let drop = cues.iter().find(|c| c.label == "drop").unwrap();
                let boundary = app.library.get(id).unwrap().phrases[1].start_ms;
                assert_eq!(drop.time_ms, boundary, "the cue was left behind");
            }

            #[test]
            fn a_cue_named_for_what_is_sung_there_stays_where_it_is() {
                // It is about a moment in the music, not about the line above
                // it, and the music has not moved.
                let (mut app, id) = with_a_drop("sung");
                app.edit_phrase(id, PhraseEdit::Move { at: 1, time_ms: 40_000 });

                let cues = &app.library.get(id).unwrap().cues;
                let hook = cues.iter().find(|c| c.label == "hold me closer now").unwrap();
                assert_eq!(hook.time_ms, 40_000);
            }

            #[test]
            fn a_cue_marking_a_different_boundary_is_not_dragged_along() {
                let (mut app, id) = with_a_drop("elsewhere");
                app.edit_phrase(id, PhraseEdit::Move { at: 1, time_ms: 40_000 });

                let cues = &app.library.get(id).unwrap().cues;
                let other = cues.iter().find(|c| c.label == "break").unwrap();
                assert_eq!(other.time_ms, 96_000, "the far boundary's cue moved too");
                // And the memory cue, which anchors the grid, is untouched.
                assert_eq!(cues.iter().find(|c| c.letter == 0).unwrap().time_ms, 0);
            }

            #[test]
            fn a_cue_a_beat_away_from_the_line_is_marking_something_else() {
                // A section cue and its boundary are both snapped, so they sit
                // on the same moment. A whole beat of daylight between them
                // means the cue is for something the line is not.
                let mut cues = vec![cue(1, 32_000, "drop"), cue(2, 33_000, "drop")];
                let moved = drag_cues(&mut cues, 32_000, 48_000, 500);
                assert_eq!(moved, 1);
                assert_eq!(cues[0].time_ms, 48_000);
                assert_eq!(cues[1].time_ms, 33_000, "two beats away and dragged anyway");
            }

            #[test]
            fn a_boundary_that_did_not_move_moves_nothing() {
                let mut cues = vec![cue(1, 32_000, "drop")];
                assert_eq!(drag_cues(&mut cues, 32_000, 32_000, 500), 0);
                assert_eq!(cues[0].time_ms, 32_000);
            }
        }

        /// A stem's picture is of the stem; everything else it shows is the
        /// record's.
        mod what_a_companion_draws {
            use super::*;
            use crate::library::{CueMark, Role};

            /// A track with a kit whose files are really on disk, since a row
            /// with a missing file is deliberately not drawn.
            fn with_a_kit(name: &str) -> (App, u32) {
                let mut app = app(name);
                let dir = app.library_path.parent().unwrap().to_path_buf();
                let mix = dir.join("Sirens.flac");
                std::fs::write(&mix, b"a mix").unwrap();

                let id = app.library.add(&mix);
                let track = app.library.get_mut(id).unwrap();
                track.artist = "Peverelist".into();
                track.title = "Sirens".into();
                track.analyzed = true;
                track.has_grid = true;
                track.bpm = 130.0;
                track.cues = vec![CueMark {
                    letter: 1,
                    time_ms: 4_000,
                    label: "drop".into(),
                    color: [1, 2, 3],
                }];
                for (part, slot) in [("vocals", 0), ("drums", 1), ("melody", 2)] {
                    let path = dir.join(format!("Sirens-{part}.mp3"));
                    std::fs::write(&path, b"a stem").unwrap();
                    let kit = &mut app.library.get_mut(id).unwrap().stems;
                    match slot {
                        0 => kit.vocals = Some(path),
                        1 => kit.drums = Some(path),
                        _ => kit.melody = Some(path),
                    }
                }
                app.rebuild();
                (app, id)
            }

            fn companion(app: &App, id: u32, role: Role) -> Track {
                app.library
                    .row(crate::library::companion_id(id, role))
                    .expect("the companion row was not built")
            }

            #[test]
            fn an_acapella_is_drawn_from_the_vocal_stem_and_not_the_mix() {
                // The fault: a companion is a clone of its parent with a
                // different role, so its `path` is the record's file. Anything
                // drawing from that put the whole mix under the acapella's
                // name — and cached it there, so it stayed wrong.
                let (app, id) = with_a_kit("acapella");
                let acapella = companion(&app, id, Role::Vocals);

                let drawn = picture_sources(&acapella).expect("nothing to draw the acapella from");
                assert_eq!(drawn.len(), 1);
                assert!(
                    drawn[0].to_string_lossy().contains("vocals"),
                    "the acapella would have been drawn from {:?}",
                    drawn[0]
                );
                assert_ne!(drawn[0], acapella.path, "that is the mix, not the stem");
            }

            #[test]
            fn the_record_itself_is_still_drawn_from_its_own_file() {
                let (app, id) = with_a_kit("record");
                let track = app.library.get(id).unwrap().clone();
                assert_eq!(picture_sources(&track), Some(vec![track.path.clone()]));
            }

            #[test]
            fn everything_but_the_picture_is_still_the_records() {
                // The other half of the rule, and the half it would be easy to
                // break while fixing the first: a cue that did not line up
                // with the one on the track would be worse than no cue at all.
                let (app, id) = with_a_kit("inherited");
                let parent = app.library.get(id).unwrap().clone();
                let acapella = companion(&app, id, Role::Vocals);

                assert_eq!(acapella.cues, parent.cues);
                assert_eq!(acapella.bpm, parent.bpm);
                assert!(acapella.has_grid);
                assert_eq!(acapella.parent, Some(id));
            }

            #[test]
            fn analysing_a_record_draws_its_stems_too() {
                // What "re-analyse" has to mean for a track with a kit. Left
                // to be drawn one at a time as somebody clicks on them, a
                // crate that has just been prepared is not prepared.
                let (app, id) = with_a_kit("batch");
                let drawing = app.stem_pictures(&[id]);

                assert_eq!(drawing.len(), 3, "one per part of the kit");
                let mut named: Vec<String> = drawing
                    .iter()
                    .flat_map(|row| &row.sources)
                    .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
                    .collect();
                named.sort();
                assert_eq!(
                    named,
                    vec![
                        "Sirens-drums.mp3".to_string(),
                        "Sirens-melody.mp3".to_string(),
                        "Sirens-vocals.mp3".to_string()
                    ],
                    "the stems should be drawn from the stems"
                );
                // And never from the record, which has its own picture.
                assert!(drawing.iter().all(|row| row.id != id));
            }

            #[test]
            fn a_record_with_no_kit_has_no_stems_to_draw() {
                let (mut app, id) = with_a_kit("kitless");
                app.library.get_mut(id).unwrap().stems = Default::default();
                app.rebuild();
                assert!(app.stem_pictures(&[id]).is_empty());
            }

            #[test]
            fn a_stem_is_drawn_whether_or_not_the_record_was_ever_analysed() {
                // Rendering a kit is a more deliberate act than analysing, not
                // a less deliberate one. Making somebody analyse the record
                // before the stems they explicitly asked for will draw is
                // asking twice for one thing.
                let (mut app, id) = with_a_kit("unanalysed");
                app.library.get_mut(id).unwrap().analyzed = false;
                app.rebuild();

                assert!(worth_drawing(&companion(&app, id, Role::Vocals)));
                // The record itself still waits to be asked: arrowing down a
                // library of thousands of unprepared tracks should not start a
                // decode per row.
                assert!(!worth_drawing(app.library.get(id).unwrap()));
            }

            #[test]
            fn a_row_whose_audio_is_missing_is_not_drawn_at_all() {
                // A blank picture looks exactly like a quiet one, so it is
                // better not to draw and cache one.
                let (mut app, id) = with_a_kit("missing");
                let gone = app.library.get(id).unwrap().stems.vocals.clone().unwrap();
                std::fs::remove_file(&gone).unwrap();
                app.rebuild();

                assert_eq!(picture_sources(&companion(&app, id, Role::Vocals)), None);
                // And its neighbours are unaffected.
                assert!(picture_sources(&companion(&app, id, Role::Drums)).is_some());
            }
        }

        /// Noting when the prep changed, which is half of telling a player's
        /// edit from one made here.
        mod when_it_changed {
            use super::*;

            fn one_track(name: &str) -> (App, u32) {
                let mut app = app(name);
                let id = app.library.add(std::path::Path::new("/music/track.flac"));
                let track = app.library.get_mut(id).unwrap();
                track.bpm = 120.0;
                track.has_grid = true;
                track.analyzed = true;
                track.duration_secs = 300.0;
                app.rebuild();
                (app, id)
            }

            #[test]
            fn placing_a_cue_says_when() {
                let (mut app, id) = one_track("stamped");
                assert_eq!(app.library.get(id).unwrap().edited, None, "nothing has happened yet");

                app.place_cue(id, 1, 4_000);
                assert!(
                    app.library.get(id).unwrap().edited.is_some(),
                    "a moved cue with no time on it cannot be compared with the drive's"
                );
            }

            #[test]
            fn a_tag_is_not_a_change_a_player_could_disagree_about() {
                // Only the things a drive carries and a deck can edit count.
                // Stamping a rating would turn every one into a question about
                // a drive nobody has touched.
                let (mut app, id) = one_track("untouched");
                let track = app.library.get_mut(id).unwrap();
                track.tags.push("peak".into());
                track.play_count += 1;
                assert_eq!(app.library.get(id).unwrap().edited, None);
            }
        }

        /// What the log says a cue was placed for.
        ///
        /// The recogniser is wrong often enough that arguing with it is the
        /// point, and a cue whose reason cannot be read is a cue nobody can
        /// argue with.
        mod explaining_itself {
            use super::*;
            use crate::library::{CueMark, Lyric};

            fn heard(lines: &[(u32, &str)]) -> booth_cli::transcribe::Transcript {
                crate::library::transcript(
                    &lines
                        .iter()
                        .map(|&(start_ms, text)| Lyric {
                            start_ms,
                            end_ms: start_ms + 2_000,
                            text: text.to_string(),
                        })
                        .collect::<Vec<_>>(),
                )
            }

            fn cue(letter: u8, time_ms: u32, label: &str) -> CueMark {
                CueMark { letter, time_ms, label: label.to_string(), color: [0; 3] }
            }

            #[test]
            fn the_lines_are_listed_most_repeated_first() {
                let words = heard(&[
                    (20_000, "walking through the city at night"),
                    (40_000, "hold me closer now"),
                    (100_000, "hold me closer now"),
                    (160_000, "hold me closer now"),
                    (200_000, "and I don't want to go home"),
                    (260_000, "and I don't want to go home"),
                ]);
                let said = what_the_words_decided(&words, &[], &[]);

                let ranking: Vec<&String> =
                    said.iter().filter(|line| line.contains('\u{d7}')).collect();
                assert_eq!(ranking.len(), 3, "{said:#?}");
                assert!(ranking[0].contains("3\u{d7}"), "{}", ranking[0]);
                assert!(ranking[0].contains("hold me closer now"), "{}", ranking[0]);
                assert!(ranking[1].contains("2\u{d7}"), "{}", ranking[1]);
                assert!(ranking[2].contains("1\u{d7}"), "{}", ranking[2]);
                // And each says where it lands, so a cue can be checked
                // against the list that put it there.
                assert!(ranking[0].contains("0:40.00"), "{}", ranking[0]);
                assert!(ranking[0].contains("2:40.00"), "{}", ranking[0]);
            }

            #[test]
            fn the_lyric_the_hook_cue_is_for_is_named() {
                let words =
                    heard(&[(40_000, "hold me closer now"), (100_000, "hold me closer now")]);
                let said = what_the_words_decided(&words, &[], &[]);
                let hook = said.iter().find(|line| line.starts_with("hook:")).expect("{said:#?}");
                assert!(hook.contains("hold me closer now"), "{hook}");
                assert!(hook.contains("sung 2 times"), "{hook}");
                assert!(hook.contains("0:40.00"), "{hook}");
            }

            #[test]
            fn a_track_with_no_chorus_says_so_rather_than_staying_quiet() {
                // Otherwise somebody goes looking for a hook cue that was
                // never going to be there.
                let words = heard(&[(40_000, "one line, said once, and then nothing")]);
                let said = what_the_words_decided(&words, &[], &[]);
                assert!(
                    said.iter().any(|line| line.contains("no line comes round twice")),
                    "{said:#?}"
                );
            }

            #[test]
            fn every_cue_that_moved_says_what_it_was_and_what_it_became() {
                let words =
                    heard(&[(40_000, "hold me closer now"), (100_000, "hold me closer now")]);
                let before = [cue(1, 32_000, "drop"), cue(2, 96_000, "break")];
                let after = [cue(1, 40_000, "hold me closer now"), cue(3, 150_000, "outro")];

                let said = what_the_words_decided(&words, &before, &after);
                let moved = said.iter().find(|line| line.starts_with('A')).expect("{said:#?}");
                assert_eq!(moved, "A 0:32.00 drop \u{2192} 0:40.00 hold me closer now");
                // A slot that gained a cue, and one that lost one, both said.
                assert!(said.iter().any(|line| line == "C set to 2:30.00 outro"), "{said:#?}");
                assert!(
                    said.iter().any(|line| line == "B cleared, was 1:36.00 break"),
                    "{said:#?}"
                );
            }

            #[test]
            fn a_cue_that_did_not_move_is_not_reported_as_having_moved() {
                let words =
                    heard(&[(40_000, "hold me closer now"), (100_000, "hold me closer now")]);
                let same = [cue(1, 40_000, "hold me closer now")];
                let said = what_the_words_decided(&words, &same, &same);
                assert!(
                    said.iter().any(|line| line == "every cue is already where the words put it"),
                    "{said:#?}"
                );
                assert!(!said.iter().any(|line| line.contains('\u{2192}')), "{said:#?}");
            }
        }

        /// Cues placed off what is sung, rather than off what is played.
        mod from_the_words {
            use super::*;
            use crate::library::{Lyric, Phrase, Role};

            /// A window holding one analysed track: a grid, two phrases, and
            /// whatever has already been heard on its vocal stem.
            ///
            /// 120 BPM, so a beat is 500 ms and a bar is two seconds, and the
            /// arithmetic in these tests can be done in the head.
            fn sung(name: &str, lyrics: &[(u32, &str)]) -> (App, u32) {
                let mut app = app(name);
                // A real file: everything that spends minutes on a track
                // checks first that it is still where the collection says.
                let path = app.library_path.with_file_name("track.flac");
                std::fs::write(&path, b"not really a flac").unwrap();

                // A recogniser, as far as these tests are concerned. Whether
                // one is installed on the machine running them is not what any
                // of them is about, and reading that from the environment is
                // exactly how two tests that had been green for a week went
                // red without a line of the code under test changing — see
                // `config::Whisper::chosen`. Left to itself this passes on a
                // developer's machine, where the session-start hook exports
                // BOOTH_WHISPER_MODEL, and fails in CI, where it does not.
                app.config.whisper.program = "whisper-cli".into();
                app.config.whisper.model = "/not-really/base.en.bin".into();

                let id = app.library.add(&path);
                let track = app.library.get_mut(id).unwrap();
                track.bpm = 120.0;
                track.has_grid = true;
                track.analyzed = true;
                track.beats = 600;
                track.duration_secs = 300.0;
                track.cues =
                    vec![CueMark { letter: 0, time_ms: 0, label: String::new(), color: [0; 3] }];
                track.phrases = vec![
                    Phrase { start_ms: 0, end_ms: 32_000, kind: "intro".into() },
                    Phrase { start_ms: 32_000, end_ms: 64_000, kind: "drop".into() },
                ];
                track.lyrics = lyrics
                    .iter()
                    .map(|&(start_ms, text)| Lyric {
                        start_ms,
                        end_ms: start_ms + 2_000,
                        text: text.to_string(),
                    })
                    .collect();
                app.rebuild();
                (app, id)
            }

            fn labels(app: &App, id: u32) -> Vec<String> {
                app.library
                    .get(id)
                    .unwrap()
                    .cues
                    .iter()
                    .filter(|cue| cue.letter != 0)
                    .map(|cue| cue.label.clone())
                    .collect()
            }

            #[test]
            fn the_line_the_track_keeps_coming_back_to_becomes_a_cue() {
                let (mut app, id) = sung(
                    "hook",
                    &[
                        (20_000, "walking through the city at night"),
                        (40_100, "hold me closer now"),
                        (100_100, "hold me closer now"),
                        (160_100, "hold me closer now"),
                    ],
                );
                assert!(app.auto_cue(id) > 0, "no cues at all");

                let cues = app.library.get(id).unwrap().cues.clone();
                let hook = cues
                    .iter()
                    .find(|cue| cue.label == "hold me closer now")
                    .unwrap_or_else(|| panic!("the hook was not cued: {cues:?}"));
                // On the beat before the first word rather than the nearest
                // one, so the cue cannot land after the word it is for.
                assert_eq!(hook.time_ms, 40_000);
                // Once, wherever it first lands. A line sung three times used
                // to be three cues saying the same thing, which left the drops
                // and the breakdowns nowhere to go.
                assert_eq!(
                    cues.iter().filter(|cue| cue.label == "hold me closer now").count(),
                    1,
                    "{cues:?}"
                );
            }

            #[test]
            fn cueing_a_playlist_with_no_stems_asks_before_spending_the_afternoon() {
                // The fault: cueing from the words needs an isolated vocal,
                // so one menu click on a playlist started a separation per
                // track in it without the word "stems" appearing anywhere.
                let (mut app, first) = sung("batch-one", &[]);
                let second = {
                    let path = app.library_path.with_file_name("second.flac");
                    std::fs::write(&path, b"not really a flac").unwrap();
                    app.library.add(&path)
                };
                app.rebuild();

                app.auto_cue_tracks(&[first, second], false);

                let ahead = app.stems_ahead.as_ref().expect("the batch started unannounced");
                assert_eq!(ahead.tracks.len(), 2);
                assert!(ahead.for_cues, "the stems are for cues, which is what to say");
                assert!(!app.running(), "nothing should have started yet");
                assert!(app.want_cues.is_empty(), "nor should anything be queued to follow");
            }

            #[test]
            fn a_track_known_to_have_no_singing_is_not_paid_for_twice() {
                // An empty transcript off a stem that was listened to is a
                // real answer, and without keeping it every press of Words
                // bought the same minutes again.
                let (mut app, id) = sung("instrumental", &[]);
                app.library.get_mut(id).unwrap().stems.vocals =
                    Some(app.library_path.with_file_name("track-vocals.wav"));
                app.library.get_mut(id).unwrap().instrumental = true;
                app.rebuild();

                app.auto_cue_tracks(&[id], false);
                assert!(!app.running(), "an instrumental went back to the recogniser");

                // Holding shift still asks, because a better recogniser is
                // exactly the reason to doubt the old answer.
                app.auto_cue_tracks(&[id], true);
                assert!(app.running(), "asking again should still be possible");
            }

            #[test]
            fn a_name_that_says_instrumental_costs_no_separation_at_all() {
                // The filename is somebody telling you the answer. Finding it
                // out instead is a stem render and a pass through Whisper.
                let mut app = app("named-instrumental");
                let path =
                    app.library_path.with_file_name("Falco - Der Kommissar (Instrumental).flac");
                std::fs::write(&path, b"not really a flac").unwrap();
                let id = app.library.add(&path);
                app.rebuild();

                app.auto_cue_tracks(&[id], false);

                assert!(app.library.get(id).unwrap().instrumental, "the name was not believed");
                assert!(app.stems_ahead.is_none(), "it asked to separate an instrumental");
                assert!(!app.running(), "it started work on an instrumental");
            }

            #[test]
            fn reading_the_words_again_goes_back_to_the_recogniser() {
                // Without this a track that has already been read is re-cued
                // from the words it is carrying, which is the right answer
                // right up until the recogniser or the placing has improved —
                // and then it is the one answer that cannot be got past.
                let (mut app, id) = sung("re-read", &[(40_100, "get down")]);
                app.library.get_mut(id).unwrap().stems.vocals =
                    Some(app.library_path.with_file_name("track-vocals.wav"));
                app.rebuild();

                assert!(
                    app.auto_cue_tracks(&[id], false),
                    "the words it has should be cued without a job"
                );
                assert!(!app.running(), "nothing should have been queued for that");

                app.auto_cue_tracks(&[id], true);
                assert!(app.running(), "asking again did not go back to the recogniser");
                // And the words it had are still there until new ones arrive,
                // so a read that cannot start has cost nothing.
                assert_eq!(app.library.get(id).unwrap().lyrics.len(), 1);
            }

            #[test]
            fn one_track_is_not_worth_a_dialog() {
                // A few minutes, asked for by clicking on one record. A sheet
                // in front of every one of those is worse than no sheet.
                let (mut app, id) = sung("single", &[]);
                app.auto_cue_tracks(&[id], false);
                assert!(app.stems_ahead.is_none());
            }

            #[test]
            fn the_warning_can_be_turned_off_and_then_the_batch_just_runs() {
                let (mut app, first) = sung("unasked", &[]);
                let second = {
                    let path = app.library_path.with_file_name("second.flac");
                    std::fs::write(&path, b"not really a flac").unwrap();
                    app.library.add(&path)
                };
                app.rebuild();
                app.config.warn_before_stems = false;

                app.auto_cue_tracks(&[first, second], false);
                assert!(app.stems_ahead.is_none());
                assert_eq!(app.want_cues.len(), 2, "both should be cued once their stems land");
            }

            #[test]
            fn every_landing_of_a_line_is_marked_with_the_whole_of_it() {
                // A CDJ-3000X shows the whole comment, so there is no reason
                // to make somebody in a booth remember what V1 was.
                let (mut app, id) = sung(
                    "verses",
                    &[
                        (40_100, "everybody in the room"),
                        (100_100, "everybody in the room"),
                        (160_100, "everybody in the room"),
                    ],
                );
                app.auto_cue(id);
                let marks: Vec<String> = app
                    .library
                    .get(id)
                    .unwrap()
                    .cues
                    .iter()
                    .filter(|cue| cue.letter == 0)
                    .map(|cue| cue.label.clone())
                    .collect();

                assert_eq!(
                    marks.iter().filter(|mark| *mark == "V1 everybody in the room").count(),
                    3,
                    "{marks:?}"
                );
                assert!(!marks.iter().any(|mark| *mark == "V1"), "{marks:?}");
            }

            #[test]
            fn a_line_said_once_is_cued_as_itself_and_not_as_the_hook() {
                // It takes a spare button, because a track that repeats
                // nothing has nothing better to put there — but not the
                // hook's colour, which on a waveform is a promise that the
                // room is about to sing.
                use booth_cli::analysis::cues::Reason;
                let (mut app, id) = sung(
                    "once",
                    &[(20_000, "walking through the city at night"), (60_000, "and then home")],
                );
                app.auto_cue(id);

                let cues = app.library.get(id).unwrap().cues.clone();
                let line = cues
                    .iter()
                    .find(|cue| cue.letter != 0 && cue.label.contains("city"))
                    .unwrap_or_else(|| panic!("the only words on the track went uncued: {cues:?}"));
                let hook = Reason::Hook.color();
                assert_ne!(
                    line.color,
                    [hook.r, hook.g, hook.b],
                    "a line nobody repeats is not the hook: {line:?}"
                );
                let expected = Reason::Line.color();
                assert_eq!(line.color, [expected.r, expected.g, expected.b]);
            }

            #[test]
            fn a_track_with_no_words_still_gets_its_phrases_cued() {
                let (mut app, id) = sung("phrases", &[]);
                app.auto_cue(id);
                // The first section is on the memory cue, which is where the
                // player parks when the track loads, so it costs no hot cue.
                let cues = &app.library.get(id).unwrap().cues;
                assert_eq!(cues[0].letter, 0);
                assert_eq!(cues[0].label, "Start");
                assert_eq!(labels(&app, id), vec!["drop".to_string()]);

                // And every section it did not take is a named memory cue, so
                // the list reads as an arrangement rather than as a row of
                // identical markers.
                let marks: Vec<&str> = cues
                    .iter()
                    .filter(|cue| cue.letter == 0)
                    .map(|cue| cue.label.as_str())
                    .collect();
                assert_eq!(marks.first(), Some(&"Start"));
                assert!(marks.contains(&"Drop 1"), "{marks:?}");
                assert_eq!(marks.last(), Some(&"End"), "{marks:?}");
            }

            #[test]
            fn dragging_one_memory_cue_does_not_move_another() {
                // Every memory cue carries letter zero, so looking one up by
                // letter found whichever came first — the one the grid is
                // anchored to. Dragging "Drop 1" re-gridded the record and
                // nothing said so.
                let (mut app, id) = sung("two memory cues", &[]);
                {
                    let track = app.library.get_mut(id).unwrap();
                    track.cues = vec![
                        crate::library::CueMark {
                            letter: 0,
                            time_ms: 0,
                            label: "Start".into(),
                            color: [0, 0, 0],
                        },
                        crate::library::CueMark {
                            letter: 0,
                            time_ms: 60_000,
                            label: "Drop 1".into(),
                            color: [0, 0, 0],
                        },
                    ];
                }

                app.move_cue(id, 1, 90_000);

                let cues = &app.library.get(id).unwrap().cues;
                let anchor = cues.iter().find(|cue| cue.label == "Start").unwrap();
                let moved = cues.iter().find(|cue| cue.label == "Drop 1").unwrap();
                assert_eq!(anchor.time_ms, 0, "the anchor moved instead of the one dragged");
                assert!(moved.time_ms > 60_000, "the one dragged did not move: {moved:?}");
            }

            #[test]
            fn the_memory_cue_is_left_where_the_grid_is_anchored() {
                // Every bar line in the track is measured from it, so an
                // auto-cue pass that moved it would silently re-grid the
                // record.
                let (mut app, id) = sung("anchor", &[]);
                app.library.get_mut(id).unwrap().cues[0].time_ms = 317;
                app.auto_cue(id);

                let cues = &app.library.get(id).unwrap().cues;
                assert_eq!(cues[0].letter, 0);
                assert_eq!(cues[0].time_ms, 317);
            }

            #[test]
            fn asking_an_acapella_for_cues_asks_the_record_it_came_from() {
                let (mut app, id) = sung(
                    "acapella",
                    &[(40_100, "hold me closer now"), (100_100, "hold me closer now")],
                );
                let acapella = crate::library::companion_id(id, Role::Vocals);
                assert!(app.auto_cue_tracks(&[acapella], false), "the row did nothing");

                let placed = labels(&app, id);
                assert!(placed.iter().any(|label| label == "hold me closer now"), "{placed:?}");
            }

            #[test]
            fn a_track_with_no_vocal_stem_is_remembered_until_it_has_one() {
                let (mut app, id) = sung("render", &[]);
                app.config.whisper.model = "/models/ggml-base.en.bin".into();

                assert!(!app.auto_cue_tracks(&[id], false), "nothing can be cued yet");
                assert!(
                    app.want_cues.contains(&id),
                    "the track was not remembered, so its words will never be read"
                );
            }

            /// Words somebody wrote down, rather than words heard off a stem.
            mod looked_up {
                use super::*;
                use crate::library::WordsFrom;
                use booth_cli::tag::lyrics::Found;
                use booth_cli::transcribe::{Line, Transcript};

                /// What a server answers for a track whose lyric it has timed.
                fn found(lines: &[(u32, &str)]) -> Found {
                    Found {
                        synced: Transcript {
                            lines: lines
                                .iter()
                                .map(|&(start_ms, text)| Line {
                                    start_ms,
                                    end_ms: start_ms + 2_000,
                                    text: text.to_string(),
                                })
                                .collect(),
                            ..Transcript::default()
                        },
                        plain: lines.iter().map(|(_, text)| text.to_string()).collect(),
                        instrumental: false,
                        artist: "Falco".into(),
                        title: "Der Kommissar".into(),
                        duration_secs: 300.0,
                    }
                }

                #[test]
                fn words_a_server_timed_are_taken_but_left_for_the_stem_to_place() {
                    let (mut app, id) = sung("looked-up", &[]);
                    app.library.get_mut(id).unwrap().heard_surely = Some(0.4);

                    let offered = found(&[(1_000, "drei, vier"), (9_000, "jetzt gehn wir ab")]);
                    assert!(app.take_found_words(id, &offered), "nothing was taken");

                    let track = app.library.get(id).unwrap();
                    assert_eq!(
                        track.lyrics.iter().map(|line| line.text.as_str()).collect::<Vec<_>>(),
                        ["drei, vier", "jetzt gehn wir ab"]
                    );
                    assert_eq!(track.words_from, WordsFrom::Server);
                    // The lyric was synced against one pressing and this is a
                    // library of extended mixes, so the times are words-shaped
                    // evidence and nothing more until a stem is measured.
                    assert!(
                        !track.lyrics_aligned,
                        "another pressing's times were taken as this track's"
                    );
                    // The doubt that was recorded belonged to words that are
                    // gone, and nobody doubts the ones that replaced them.
                    assert_eq!(track.heard_surely, None);
                }

                #[test]
                fn words_a_server_only_had_as_text_are_kept_though_nothing_can_place_them() {
                    let (mut app, id) = sung("plain", &[]);
                    let mut offered = found(&[(1_000, "drei, vier"), (9_000, "jetzt gehn wir ab")]);
                    offered.synced = Transcript::default();
                    assert!(app.take_found_words(id, &offered), "nothing was taken");

                    let track = app.library.get(id).unwrap();
                    assert_eq!(track.lyrics.len(), 2);
                    assert!(
                        track.lyrics.iter().all(|line| line.start_ms == 0),
                        "{:?}",
                        track.lyrics
                    );
                    // Nothing for the aligner to do: it moves a line by the
                    // time on it, so a set of lines all at zero would come out
                    // stacked on the first word sung. These are here to be
                    // read in the panel and searched for, not to be cued from.
                    assert!(track.lyrics_aligned, "a line at zero was sent off to be placed");
                }

                #[test]
                fn a_server_calling_a_track_an_instrumental_saves_finding_out_the_slow_way() {
                    let (mut app, id) = sung("instrumental", &[]);
                    let offered = Found {
                        instrumental: true,
                        artist: "Floating Points".into(),
                        title: "Last Bloom".into(),
                        duration_secs: 300.0,
                        ..Found::default()
                    };
                    assert!(app.take_found_words(id, &offered), "the answer was thrown away");

                    assert!(app.library.get(id).unwrap().instrumental);
                    // Which is the row that earns the lookup its place: one
                    // request against a separation and a recogniser pass that
                    // end in "nothing was sung".
                    assert!(app.words_answered(id));
                }

                #[test]
                fn a_stem_landing_on_words_nobody_guessed_is_for_placing_them_not_reading_over() {
                    let (mut app, id) = sung("answered", &[(40_000, "hold me closer now")]);
                    app.library.get_mut(id).unwrap().words_from = WordsFrom::Recogniser;
                    // Read off a stem, so a stem is worth another pass: that
                    // is what asking again means.
                    assert!(!app.words_answered(id));

                    app.library.get_mut(id).unwrap().words_from = WordsFrom::Server;
                    assert!(
                        app.words_answered(id),
                        "a recogniser would have written over words somebody wrote down"
                    );
                }

                #[test]
                fn correcting_a_word_keeps_the_times_the_lines_already_had() {
                    let (mut app, id) = sung(
                        "corrected",
                        &[(40_000, "hold me close now"), (100_000, "hold me close now")],
                    );
                    app.editing_words = Some(EditingWords {
                        id,
                        text: "hold me closer now\nhold me closer now".into(),
                    });
                    assert!(app.save_edited_words(id), "the correction was dropped");

                    let track = app.library.get(id).unwrap();
                    assert_eq!(track.lyrics[0].text, "hold me closer now");
                    assert_eq!(track.lyrics[0].start_ms, 40_000);
                    assert_eq!(track.lyrics[1].start_ms, 100_000);
                    assert_eq!(track.words_from, WordsFrom::ByHand);
                    // Typing the words in is the last word on them: nothing
                    // reads them off the stem again on its own.
                    assert!(app.words_answered(id));
                }

                #[test]
                fn adding_a_line_by_hand_gives_up_the_times_rather_than_shifting_them_all_down() {
                    let (mut app, id) = sung(
                        "added",
                        &[(40_000, "hold me closer now"), (100_000, "hold me closer now")],
                    );
                    app.editing_words = Some(EditingWords {
                        id,
                        text: "walking through the city\nhold me closer now\nhold me closer now"
                            .into(),
                    });
                    assert!(app.save_edited_words(id), "the correction was dropped");

                    let track = app.library.get(id).unwrap();
                    assert_eq!(track.lyrics.len(), 3);
                    // A time carried onto a different line is a cue under the
                    // wrong words, which is worse than a line with no cue.
                    assert!(
                        track.lyrics.iter().all(|line| line.start_ms == 0),
                        "{:?}",
                        track.lyrics
                    );
                }

                /// A track with a stem, words already read, and a name to
                /// ask a server under.
                fn read_already(name: &str) -> (App, u32) {
                    let (mut app, id) = sung(name, &[(40_000, "hold me closer now")]);
                    let track = app.library.get_mut(id).unwrap();
                    track.artist = "Falco".into();
                    track.title = "Der Kommissar".into();
                    track.stems.vocals = Some(track.path.with_extension("vocals.wav"));
                    app.rebuild();
                    (app, id)
                }

                #[test]
                fn re_reading_a_track_asks_the_server_before_the_recogniser() {
                    // The case this is for: somebody has looked at the words
                    // on screen, seen that they are wrong, and asked for them
                    // again. A track that already has a stem went straight to
                    // the recogniser, which is the one thing already known not
                    // to have worked — so the lookup never ran on the pass
                    // where it was most wanted.
                    let (mut app, id) = read_already("re-read");
                    let plan = app.words_plan(&[id], true);

                    assert_eq!(
                        plan.asking.iter().map(|ask| ask.id).collect::<Vec<_>>(),
                        [id],
                        "a re-read did not ask the server"
                    );
                    assert!(
                        plan.reading.is_empty(),
                        "the recogniser was started alongside the lookup, so it would \
                         spend the minutes the lookup exists to save and then write over \
                         what came back"
                    );
                }

                #[test]
                fn a_first_reading_asks_the_server_too() {
                    let (mut app, id) = read_already("first-read");
                    app.library.get_mut(id).unwrap().lyrics.clear();
                    app.rebuild();

                    let plan = app.words_plan(&[id], false);
                    assert_eq!(plan.asking.len(), 1, "nothing was asked about");
                    assert!(plan.reading.is_empty());
                }

                #[test]
                fn a_track_with_no_name_to_ask_under_goes_straight_to_the_recogniser() {
                    // Most of a crate of white labels. Asking a lyrics
                    // database about "Unknown Artist" wastes a request and
                    // delays the only answer there is going to be.
                    let (mut app, id) = read_already("nameless");
                    let track = app.library.get_mut(id).unwrap();
                    track.artist.clear();
                    app.rebuild();

                    let plan = app.words_plan(&[id], true);
                    assert!(plan.asking.is_empty(), "a nameless track was looked up");
                    assert_eq!(
                        plan.reading.iter().map(|read| read.id).collect::<Vec<_>>(),
                        [id],
                        "and then it was not read either"
                    );
                }

                #[test]
                fn a_track_with_no_stem_is_asked_about_before_it_is_separated() {
                    let (mut app, id) = read_already("stemless");
                    let track = app.library.get_mut(id).unwrap();
                    track.stems.vocals = None;
                    track.lyrics.clear();
                    app.rebuild();

                    let plan = app.words_plan(&[id], false);
                    assert_eq!(plan.asking.len(), 1);
                    // Still separated: the words may come back needing placing
                    // against this pressing, and there is nothing to place
                    // them against until the stem exists.
                    assert_eq!(plan.rendering, [id]);
                }

                #[test]
                fn asking_for_a_lookup_on_its_own_owes_the_recogniser_nothing() {
                    // The context-menu entry. It is the cheap half of cueing
                    // from the words on its own — one request, seconds — so
                    // a miss ends there rather than quietly starting the
                    // separation and the recogniser pass somebody chose not to
                    // ask for.
                    let (app, id) = read_already("menu-lookup");
                    let asking = app.worth_asking(&[id]);

                    assert_eq!(asking.len(), 1);
                    assert_eq!(asking[0].id, id);
                    assert_eq!(asking[0].artist, "Falco");
                    assert!(
                        asking[0].then_read.is_none(),
                        "a lookup asked for on its own would have started a recogniser pass"
                    );
                }

                #[test]
                fn a_lookup_inside_the_cueing_pass_owes_the_stem_it_stood_in_for() {
                    let (mut app, id) = read_already("pass-lookup");
                    let plan = app.words_plan(&[id], true);

                    assert_eq!(plan.asking.len(), 1);
                    assert!(
                        plan.asking[0].then_read.is_some(),
                        "the held-back recogniser pass was not carried with the request, so a \
                         track the server could not answer for would never be read at all"
                    );
                }

                #[test]
                fn asking_about_an_acapella_asks_about_the_record_it_came_from() {
                    let (app, id) = read_already("companion");
                    let acapella = crate::library::companion_id(id, crate::library::Role::Vocals);

                    let asking = app.worth_asking(&[acapella, id]);
                    assert_eq!(
                        asking.iter().map(|ask| ask.id).collect::<Vec<_>>(),
                        [id],
                        "a stem row has no words of its own, and asking twice is two requests"
                    );
                }

                #[test]
                fn a_track_with_no_name_has_nothing_to_ask_a_lyrics_server_under() {
                    let (mut app, id) = read_already("nameless-lookup");
                    app.library.get_mut(id).unwrap().artist.clear();
                    app.rebuild();

                    assert!(app.worth_asking(&[id]).is_empty());
                }

                #[test]
                fn the_row_menu_offers_a_lookup_of_its_own() {
                    let (mut app, id) = read_already("menu");
                    app.selected = Some(id);
                    // A picture already in hand. Without one the panel starts a
                    // job to measure one on its first frame, and every action
                    // on the strip is disabled while anything is running — so
                    // the test would be photographing a panel mid-draw rather
                    // than the one somebody clicks.
                    app.waveform = Some((id, vec![0; 300]));

                    // The prep panel, which is where the per-track actions
                    // live, and tall because they sit under the waveform.
                    let mut harness = Harness::builder()
                        .with_size(egui::vec2(900.0, 1600.0))
                        .build_ui_state(|ui, app: &mut App| app.prep(ui), app);
                    harness.run();
                    harness.get_by_label("Look up the words").click();
                    harness.run();

                    assert!(
                        harness
                            .state()
                            .pending
                            .iter()
                            .any(|want| matches!(want, Pending::LookUpWords(_))),
                        "the entry did not ask for a lookup"
                    );
                }

                #[test]
                fn a_lyric_file_beside_the_track_is_taken_over_asking_anybody() {
                    // The point of looking on disk first: it needs no request,
                    // and a file somebody filed next to this file is their
                    // answer about this record rather than an inference about
                    // it.
                    let (mut app, id) = read_already("beside");
                    let track = app.library.get(id).unwrap();
                    let lyric = track.path.with_extension("lrc");
                    std::fs::write(
                        &lyric,
                        "[00:40.00] hold me closer now\n[01:40.00] and then home\n",
                    )
                    .unwrap();

                    assert!(app.find_words(id), "the file was not taken");
                    let track = app.library.get(id).unwrap();
                    assert_eq!(
                        track.lyrics.iter().map(|line| line.text.as_str()).collect::<Vec<_>>(),
                        ["hold me closer now", "and then home"]
                    );
                    assert_eq!(track.words_from, WordsFrom::Beside);
                }

                #[test]
                fn a_lyric_file_is_still_placed_against_the_stem() {
                    // An .lrc is synced against whatever pressing whoever made
                    // it had. That it was filed next to this file says the
                    // words are right, not that the times are.
                    let (mut app, id) = read_already("beside-times");
                    let lyric = app.library.get(id).unwrap().path.with_extension("lrc");
                    std::fs::write(&lyric, "[00:40.00] hold me closer now\n").unwrap();

                    assert!(app.find_words(id));
                    assert!(
                        !app.library.get(id).unwrap().lyrics_aligned,
                        "another pressing's times were taken as this track's"
                    );
                }

                #[test]
                fn a_text_file_beside_the_track_is_words_without_times() {
                    let (mut app, id) = read_already("beside-txt");
                    let lyric = app.library.get(id).unwrap().path.with_extension("txt");
                    std::fs::write(&lyric, "hold me closer now\nand then home\n").unwrap();

                    assert!(app.find_words(id));
                    let track = app.library.get(id).unwrap();
                    assert_eq!(track.lyrics.len(), 2);
                    // Nothing to place, so nothing is sent off to be placed.
                    assert!(track.lyrics_aligned);
                }

                #[test]
                fn an_empty_lyric_file_leaves_the_words_that_were_there() {
                    let (mut app, id) = read_already("beside-empty");
                    let before = app.library.get(id).unwrap().lyrics.clone();
                    let lyric = app.library.get(id).unwrap().path.with_extension("lrc");
                    std::fs::write(&lyric, "\n\n").unwrap();

                    assert!(!app.find_words(id));
                    assert_eq!(app.library.get(id).unwrap().lyrics, before);
                }

                #[test]
                fn the_panel_offers_to_find_the_words() {
                    let (mut app, id) = read_already("find-button");
                    app.selected = Some(id);
                    app.waveform = Some((id, vec![0; 300]));

                    let mut harness = Harness::builder()
                        .with_size(egui::vec2(360.0, 1600.0))
                        .build_ui_state(|ui, app: &mut App| app.inspector(ui), app);
                    harness.run();
                    harness.get_by_label_contains("Find the words").click();
                    harness.run();

                    assert!(
                        harness
                            .state()
                            .pending
                            .iter()
                            .any(|want| matches!(want, Pending::FindWords(_))),
                        "the button did not ask for anything"
                    );
                }

                /// A gridded track showing in the panel, with a picture in
                /// hand so the panel does not start a job on its first frame.
                fn on_the_grid(name: &str) -> (App, u32) {
                    let (mut app, id) = read_already(name);
                    let track = app.library.get_mut(id).unwrap();
                    track.bpm = 128.0;
                    track.has_grid = true;
                    track.downbeat_ms = Some(0);
                    app.rebuild();
                    app.selected = Some(id);
                    app.waveform = Some((id, vec![0; 300]));
                    (app, id)
                }

                fn panel(app: App) -> Harness<'static, App> {
                    Harness::builder()
                        .with_size(egui::vec2(900.0, 1600.0))
                        .build_ui_state(|ui, app: &mut App| app.prep(ui), app)
                }

                /// Which cue, by the letter it currently carries.
                fn which_cue(app: &App, id: u32, letter: u8) -> WhichCue {
                    let cues = &app.library.get(id).unwrap().cues;
                    let at = cues.iter().position(|cue| cue.letter == letter).expect("no such cue");
                    WhichCue::of(at, &cues[at])
                }

                /// Which cue, by where it sits in the track.
                fn which_cue_at(app: &App, id: u32, time_ms: u32) -> WhichCue {
                    let cues = &app.library.get(id).unwrap().cues;
                    let at =
                        cues.iter().position(|cue| cue.time_ms == time_ms).expect("no such cue");
                    WhichCue::of(at, &cues[at])
                }

                /// A track with a hot cue on B and a memory cue, and words.
                fn cued(name: &str) -> (App, u32) {
                    let (mut app, id) = on_the_grid(name);
                    let track = app.library.get_mut(id).unwrap();
                    track.cues = vec![
                        crate::library::CueMark {
                            letter: 0,
                            time_ms: 1_000,
                            label: String::new(),
                            color: [0x98, 0xA4, 0xAE],
                        },
                        crate::library::CueMark {
                            letter: 2,
                            time_ms: 62_500,
                            label: "Vocal".into(),
                            color: crate::job::cue_color(2),
                        },
                    ];
                    track.lyrics = vec![crate::library::Lyric {
                        start_ms: 62_000,
                        end_ms: 64_000,
                        text: "hold me closer now".into(),
                    }];
                    app.rebuild();
                    (app, id)
                }

                /// Three memory cues, so that "the one with letter 0" is a
                /// question rather than an answer.
                fn several_memory_cues(name: &str) -> (App, u32) {
                    let (mut app, id) = on_the_grid(name);
                    let track = app.library.get_mut(id).unwrap();
                    track.cues = [20_000u32, 60_000, 100_000]
                        .into_iter()
                        .map(|time_ms| crate::library::CueMark {
                            letter: 0,
                            time_ms,
                            label: String::new(),
                            color: [0x98, 0xA4, 0xAE],
                        })
                        .collect();
                    app.rebuild();
                    (app, id)
                }

                #[test]
                fn naming_the_memory_cue_under_the_playhead_names_that_one() {
                    // The fault: every memory cue carries letter zero, so
                    // acting on "the cue with letter 0" acted on whichever
                    // sorted first however carefully somebody had parked on a
                    // different one. Typing a name on the third memory cue put
                    // it on the first.
                    let (mut app, id) = several_memory_cues("cue-third");
                    app.playhead_ms = Some(100_000);
                    app.rebuild();

                    let mut harness = panel(app);
                    harness.run();
                    // Focused first, and a frame for the field to have the
                    // keyboard: typing goes wherever the keyboard is.
                    harness.get_by_role(accesskit::Role::TextInput).focus();
                    harness.run();
                    harness.get_by_role(accesskit::Role::TextInput).type_text("the one at the end");
                    harness.run();
                    let ctx = harness.ctx.clone();
                    harness.state_mut().apply_pending(&ctx);

                    let cues = &harness.state().library.get(id).unwrap().cues;
                    let named: Vec<(u32, &str)> = cues
                        .iter()
                        .filter(|cue| !cue.label.is_empty())
                        .map(|cue| (cue.time_ms, cue.label.as_str()))
                        .collect();
                    assert_eq!(
                        named,
                        vec![(100_000, "the one at the end")],
                        "the name landed on the wrong memory cue"
                    );
                }

                #[test]
                fn clearing_a_memory_cue_takes_one_rather_than_all_of_them() {
                    // The same fault with worse consequences: removing "every
                    // cue with letter 0" removed every memory cue on the
                    // track, which is somebody's whole arrangement.
                    let (mut app, id) = several_memory_cues("cue-clear-one");
                    let which = which_cue_at(&app, id, 60_000);
                    app.pending.push(Pending::RemoveCue { id, which });
                    let ctx = egui::Context::default();
                    app.apply_pending(&ctx);

                    let left: Vec<u32> =
                        app.library.get(id).unwrap().cues.iter().map(|cue| cue.time_ms).collect();
                    assert_eq!(left, vec![20_000, 100_000]);
                }

                #[test]
                fn an_action_queued_before_the_list_moves_still_finds_its_cue() {
                    // Closing the menu after turning a hot cue into a memory
                    // cue makes the name field give up the keyboard, so the
                    // rename is queued behind a recast that re-sorts the list
                    // and changes the letter the cue carries. Looked for by
                    // where it was it is a different cue; looked for by the
                    // letter it had it cannot be found at all.
                    let (mut app, id) = cued("cue-reorder");
                    let hot = which_cue(&app, id, 2);
                    app.pending.push(Pending::RecastCue { id, which: hot, into: 0 });
                    app.pending.push(Pending::RenameCue {
                        id,
                        which: hot,
                        label: "still the same cue".into(),
                    });
                    let ctx = egui::Context::default();
                    app.apply_pending(&ctx);

                    let cues = &app.library.get(id).unwrap().cues;
                    let named = cues.iter().find(|cue| cue.label == "still the same cue");
                    assert_eq!(
                        named.map(|cue| (cue.letter, cue.time_ms)),
                        Some((0, 62_500)),
                        "the rename did not follow the cue it was about: {cues:#?}"
                    );
                }

                #[test]
                fn a_cue_can_be_repainted_from_its_own_menu() {
                    // The colour goes on the drive, so it is what a player
                    // lights its button with rather than a choice about this
                    // window.
                    let (mut app, id) = cued("cue-colour");
                    let want = theme::CUE_COLORS[4];
                    let which = which_cue(&app, id, 2);
                    app.pending.push(Pending::RecolorCue {
                        id,
                        which,
                        color: [want.r(), want.g(), want.b()],
                    });
                    let ctx = egui::Context::default();
                    app.apply_pending(&ctx);

                    let cue = app
                        .library
                        .get(id)
                        .unwrap()
                        .cues
                        .iter()
                        .find(|cue| cue.letter == 2)
                        .expect("the cue went missing");
                    assert_eq!(cue.color, [want.r(), want.g(), want.b()]);
                }

                #[test]
                fn a_hot_cue_can_be_made_a_memory_cue_and_back() {
                    // Two different things on a player: eight buttons against
                    // as many marks as a track needs. Which one a mark should
                    // be is a judgement about how the track gets played.
                    let (mut app, id) = cued("cue-recast");
                    let ctx = egui::Context::default();

                    let b = which_cue(&app, id, 2);
                    assert!(app.recast_cue(id, b, 0), "B did not become a memory cue");
                    let track = app.library.get(id).unwrap();
                    assert!(!track.cues.iter().any(|cue| cue.letter == 2));
                    assert_eq!(track.cues.iter().filter(|cue| cue.letter == 0).count(), 2);

                    // And back onto a free button, which repaints it to match:
                    // a cue on D that stays the colour of B is a row of
                    // buttons that disagrees with itself.
                    let moved = which_cue_at(&app, id, 62_500);
                    app.pending.push(Pending::RecastCue { id, which: moved, into: 4 });
                    app.apply_pending(&ctx);
                    let moved = app
                        .library
                        .get(id)
                        .unwrap()
                        .cues
                        .iter()
                        .find(|cue| cue.letter == 4)
                        .expect("nothing landed on D");
                    assert_eq!(moved.color, crate::job::cue_color(4));
                }

                #[test]
                fn taking_a_letter_that_is_in_use_is_refused_rather_than_resolved() {
                    // Bumping the cue already there would lose whichever of
                    // the two somebody cared about, and there is no way to ask
                    // which.
                    let (mut app, id) = cued("cue-taken");
                    let mem = which_cue(&app, id, 0);
                    assert!(!app.recast_cue(id, mem, 2), "it took a letter already in use");

                    let track = app.library.get(id).unwrap();
                    assert_eq!(
                        track.cues.iter().find(|cue| cue.letter == 2).map(|cue| cue.label.as_str()),
                        Some("Vocal"),
                        "the cue that was already on B did not survive"
                    );
                    assert_eq!(track.cues.iter().filter(|cue| cue.letter == 0).count(), 1);
                }

                #[test]
                fn a_cue_named_for_nothing_is_offered_the_line_it_lands_on() {
                    // Which is almost always what somebody would have typed.
                    let (app, id) = cued("cue-offer");
                    let track = app.library.get(id).unwrap();
                    let cue = track.cues.iter().find(|cue| cue.letter == 2).unwrap();
                    assert_eq!(
                        crate::library::sung_at(&track.lyrics, cue.time_ms),
                        Some("hold me closer now")
                    );
                }

                #[test]
                fn the_grid_controls_stay_out_of_the_way_until_they_are_asked_for() {
                    // Nine tracks in ten need none of them, and a row of eight
                    // buttons under every waveform is eight controls in the
                    // way of the picture.
                    let (app, _) = on_the_grid("grid-hidden");
                    let mut harness = panel(app);
                    harness.run();

                    assert!(harness.query_by_label("×2").is_none(), "they were showing");
                    harness.get_by_label("fix").click();
                    harness.run();
                    harness.get_by_label("×2");
                }

                #[test]
                fn doubling_a_half_time_record_from_the_panel() {
                    let (mut app, id) = on_the_grid("grid-double");
                    app.library.get_mut(id).unwrap().bpm = 70.0;
                    app.editing_grid = true;
                    app.rebuild();

                    let mut harness = panel(app);
                    harness.run();
                    harness.get_by_label("×2").click();
                    harness.run();
                    // The click records what it wants; the pass that applies
                    // it runs on the next frame, the way every action here
                    // does.
                    let ctx = harness.ctx.clone();
                    harness.state_mut().apply_pending(&ctx);

                    assert_eq!(harness.state().library.get(id).unwrap().bpm, 140.0);
                }

                #[test]
                fn a_grid_fix_leaves_the_view_where_it_was() {
                    // Somebody correcting a grid is zoomed in on the kicks
                    // they are correcting it against, and the fix used to drop
                    // the picture — so the next frame had no bands, the panel
                    // reported a fitted view for a panel it had not drawn, and
                    // the view they were working in went with it.
                    let (mut app, id) = on_the_grid("grid-keeps-zoom");
                    app.library.get_mut(id).unwrap().bpm = 70.0;
                    app.editing_grid = true;
                    // A picture at the resolution a real one has — 150
                    // columns a second — because the view cannot be zoomed in
                    // past two points a column, and a coarse picture is pinned
                    // to the whole track whatever is asked for. No playhead, so
                    // nothing pulls the view along behind it either.
                    app.waveform = Some((id, vec![0; 3 * 150 * 300]));
                    app.playhead_ms = None;
                    app.rebuild();
                    // After the rebuild: a listing whose selection has gone
                    // fits the view on purpose, and that is not what is under
                    // test here.
                    let close = wave::Zoom { start: 0.25, span: 0.5 };
                    app.zoom = close;

                    let mut harness = panel(app);
                    harness.run();
                    assert_eq!(harness.state().zoom, close, "the view did not survive a frame");

                    harness.get_by_label("×2").click();
                    harness.run();
                    let ctx = harness.ctx.clone();
                    harness.state_mut().apply_pending(&ctx);
                    // The frame after the fix is the one that used to reset it.
                    harness.run();

                    assert_eq!(harness.state().library.get(id).unwrap().bpm, 140.0);
                    assert_eq!(harness.state().zoom, close, "the view moved");
                    assert!(
                        harness.state().waveform.is_some(),
                        "the picture is measured from the audio, so a grid fix cannot stale it"
                    );
                }

                #[test]
                fn the_grid_controls_can_always_be_put_away_again() {
                    // The trap: the way out used to live in the measurements
                    // line, and the controls are a row of their own that
                    // pushed that line off the bottom of the window. Opened,
                    // they could not be shut — and the setting is per window
                    // rather than per track, so selecting another track did
                    // not bring the measurements back either.
                    let (app, _) = on_the_grid("grid-closable");
                    let mut harness = panel(app);
                    harness.run();
                    harness.get_by_label("fix").click();
                    harness.run();
                    assert!(harness.state().editing_grid);

                    // Above the measurements line, not in it: that line is
                    // what these controls push off the bottom of the window,
                    // and a widget laid out past the bottom edge is still laid
                    // out — it just cannot be clicked. So where the way out is
                    // drawn is the thing worth asserting, not that it exists.
                    let out = harness.get_by_label("done").rect();
                    let measurements = harness.get_by_label("energy").rect();
                    assert!(
                        out.bottom() <= measurements.top(),
                        "the way out is in the line these controls displace: {out:?} against \
                         {measurements:?}"
                    );

                    harness.get_by_label("done").click();
                    harness.run();
                    assert!(!harness.state().editing_grid, "there was no way back");
                    harness.get_by_label("fix");
                }

                #[test]
                fn opening_the_grid_controls_keeps_room_for_what_is_under_them() {
                    // Measured rather than predicted, because every row added
                    // to this editor is a chance to forget to add it to a sum
                    // somewhere else — and the way that fails is silent, by
                    // clipping the measurements line away.
                    let (app, _) = on_the_grid("grid-room");
                    let mut harness = panel(app);
                    harness.run();
                    let shut = harness.state().prep_height;
                    assert!(shut > 0.0, "the editor reported no height at all");

                    harness.get_by_label("fix").click();
                    harness.run();
                    let open = harness.state().prep_height;
                    assert!(
                        open > shut,
                        "the controls added a row and asked for no more room: {shut} then {open}"
                    );
                }

                #[test]
                fn a_tempo_typed_into_the_panel_is_taken_on_return() {
                    // From the sleeve, from another program, or counted by
                    // hand — all of which beat a tempo this measured.
                    let (mut app, id) = on_the_grid("grid-typed-bpm");
                    app.editing_grid = true;
                    app.rebuild();

                    let mut harness = panel(app);
                    harness.run();
                    // Focused first, and a frame for the field to have the
                    // keyboard: typing goes wherever the keyboard is.
                    harness.get_by_role_and_label(accesskit::Role::TextInput, "BPM").focus();
                    harness.run();
                    // Over the tempo already in the box, the way somebody
                    // replacing a number does it rather than editing its digits.
                    harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
                    harness
                        .get_by_role_and_label(accesskit::Role::TextInput, "BPM")
                        .type_text("127.33");
                    harness.run();
                    harness.key_press(egui::Key::Enter);
                    harness.run();
                    let ctx = harness.ctx.clone();
                    harness.state_mut().apply_pending(&ctx);

                    assert_eq!(harness.state().library.get(id).unwrap().bpm, 127.33);
                }

                #[test]
                fn the_tenth_buttons_move_the_tempo_by_a_tenth_each_way() {
                    let (mut app, id) = on_the_grid("grid-tenths");
                    app.editing_grid = true;
                    app.rebuild();

                    let mut harness = panel(app);
                    harness.run();
                    harness.get_by_label("+0.1").click();
                    harness.run();
                    let ctx = harness.ctx.clone();
                    harness.state_mut().apply_pending(&ctx);
                    assert_eq!(harness.state().library.get(id).unwrap().bpm, 128.1);

                    // And back, from a panel that has redrawn and so is
                    // offering a tenth off the new tempo rather than the old.
                    harness.run();
                    harness.get_by_label("−0.1").click();
                    harness.run();
                    harness.state_mut().apply_pending(&ctx);
                    assert_eq!(harness.state().library.get(id).unwrap().bpm, 128.0);
                }

                #[test]
                fn changing_the_tempo_redraws_the_grid() {
                    // The marks are drawn from the beat times, and the panel
                    // reads those from its own copy of the track — so a tempo
                    // the collection has taken is only a grid the DJ can see
                    // once that copy has caught up.
                    let (mut app, id) = on_the_grid("grid-redraw");
                    app.editing_grid = true;
                    app.rebuild();
                    let before = beat_times(app.selected_track().unwrap());

                    let mut harness = panel(app);
                    harness.run();
                    harness.get_by_label("×2").click();
                    harness.run();
                    let ctx = harness.ctx.clone();
                    harness.state_mut().apply_pending(&ctx);
                    harness.run();

                    let after = beat_times(harness.state().selected_track().unwrap());
                    assert_eq!(harness.state().library.get(id).unwrap().bpm, 256.0);
                    assert!(
                        after.len() > before.len() * 3 / 2,
                        "twice the tempo is about twice the beats: {} against {}",
                        before.len(),
                        after.len()
                    );
                }

                #[test]
                fn the_grid_can_be_measured_again_without_touching_anything_else() {
                    // The case it is for: a DJ corrects a grid by hand,
                    // decides the measurement was better, and wants it back.
                    // Re-analysing would give it back and charge them every
                    // cue they had moved; this does not.
                    let (mut app, id) = on_the_grid("grid-remeasure");
                    app.editing_grid = true;
                    app.rebuild();
                    let was = app.library.get(id).unwrap().clone();

                    let mut harness = panel(app);
                    harness.run();
                    harness.get_by_label("re-measure").click();
                    harness.run();
                    let ctx = harness.ctx.clone();
                    harness.state_mut().apply_pending(&ctx);

                    assert!(harness.state().running(), "the press started nothing");
                    // Nothing of the DJ's was spent to ask: the press reaches
                    // for the audio and that is all it does. What comes back
                    // cannot spend it either — `Update::Regridded` has no field
                    // for a cue, a phrase or a key, so unlike an analysis it
                    // could not overwrite them if it tried.
                    let track = harness.state().library.get(id).unwrap();
                    assert_eq!(track.cues.len(), was.cues.len(), "the cues were touched");
                    assert_eq!(track.phrases, was.phrases, "the phrases were touched");
                    assert_eq!(track.key, was.key, "the key was touched");
                }

                #[test]
                fn a_grid_is_fitted_through_the_beats_somebody_marked() {
                    // The case: a tracker roughly right and a DJ who can see
                    // where the kicks actually are. Marks go on, a grid comes
                    // out, and it is a plain one because a plain one explains
                    // them.
                    let (mut app, id) = on_the_grid("grid-fit");
                    app.editing_grid = true;
                    {
                        let track = app.library.get_mut(id).unwrap();
                        track.bpm = 126.0;
                        // Eight beats of a 124 BPM record, from half a second in.
                        let period = 60_000.0 / 124.0;
                        track.beat_marks =
                            (0..8).map(|n| (500.0 + n as f64 * period).round() as u32).collect();
                    }
                    app.rebuild();

                    let mut harness = panel(app);
                    harness.run();
                    harness.get_by_label("fit 8").click();
                    harness.run();
                    let ctx = harness.ctx.clone();
                    harness.state_mut().apply_pending(&ctx);

                    let track = harness.state().library.get(id).unwrap();
                    assert!((track.bpm - 124.0).abs() < 0.5, "fitted {:.2}", track.bpm);
                    assert_eq!(track.downbeat_ms, Some(500));
                    assert!(!crate::grid::is_dynamic(track), "it kept beats a tempo could say");
                }

                #[test]
                fn the_mark_button_puts_a_beat_mark_at_the_playhead() {
                    // The discoverable half of placing marks. Alt-click puts
                    // one anywhere; this one is visible, which matters because
                    // a modifier nobody is told about is a feature nobody has.
                    let (mut app, id) = on_the_grid("grid-mark-button");
                    app.editing_grid = true;
                    app.playhead_ms = Some(12_345);
                    app.rebuild();

                    let mut harness = panel(app);
                    harness.run();
                    harness.get_by_label("mark").click();
                    harness.run();
                    let ctx = harness.ctx.clone();
                    harness.state_mut().apply_pending(&ctx);

                    assert_eq!(
                        harness.state().library.get(id).unwrap().beat_marks,
                        vec![12_345],
                        "the mark did not land where the playhead was"
                    );
                }

                #[test]
                fn marking_says_so_when_there_is_nowhere_to_mark() {
                    // The same rule the two positional fixes follow: without a
                    // playhead there is no answer to "where", and guessing at
                    // the middle of the track would be worse than saying so.
                    let (mut app, _) = on_the_grid("grid-mark-nowhere");
                    app.editing_grid = true;
                    app.playhead_ms = None;
                    app.rebuild();

                    let mut harness = panel(app);
                    harness.run();
                    let button = harness.get_by_label("mark");
                    assert!(format!("{button:?}").contains("disabled: true"), "{button:?}");
                }

                #[test]
                fn the_button_counts_the_marks_and_goes_quiet_without_any() {
                    // So that "fit" never looks like something to press on a
                    // track nobody has marked, and so the count is the
                    // feedback that an alt-click landed.
                    let (mut app, id) = on_the_grid("grid-fit-count");
                    app.editing_grid = true;
                    app.rebuild();

                    let mut harness = panel(app);
                    harness.run();
                    let none = harness.get_by_label("fit 0");
                    assert!(format!("{none:?}").contains("disabled: true"), "{none:?}");

                    harness.state_mut().library.get_mut(id).unwrap().beat_marks =
                        vec![1_000, 2_000];
                    harness.state_mut().rebuild();
                    harness.run();
                    let two = harness.get_by_label("fit 2");
                    assert!(format!("{two:?}").contains("disabled: false"), "{two:?}");
                }

                #[test]
                fn a_grid_kept_beat_by_beat_can_be_made_steady_from_the_panel() {
                    let (mut app, id) = on_the_grid("grid-flatten");
                    app.editing_grid = true;
                    {
                        let track = app.library.get_mut(id).unwrap();
                        track.beat_ms =
                            (0..64).map(|n| (n as f64 * 500.0).round() as u32).collect();
                        track.bpm = 119.0;
                    }
                    app.rebuild();

                    let mut harness = panel(app);
                    harness.run();
                    harness.get_by_label("make steady").click();
                    harness.run();
                    let ctx = harness.ctx.clone();
                    harness.state_mut().apply_pending(&ctx);

                    let track = harness.state().library.get(id).unwrap();
                    assert!(!crate::grid::is_dynamic(track));
                    assert_eq!(track.bpm, 120.0, "500 ms a beat is 120 BPM");
                }

                #[test]
                fn making_a_steady_grid_steady_is_offered_as_nothing_to_do() {
                    // Rather than a button with a confident name that does
                    // nothing when pressed.
                    let (mut app, _) = on_the_grid("grid-already-steady");
                    app.editing_grid = true;
                    app.rebuild();

                    let mut harness = panel(app);
                    harness.run();
                    let button = harness.get_by_label("make steady");
                    assert!(format!("{button:?}").contains("disabled: true"), "{button:?}");
                }

                #[test]
                fn a_second_mark_on_top_of_the_first_corrects_it_rather_than_pairing_with_it() {
                    // Two marks a few milliseconds apart are a slip of the
                    // hand, and read as beats they would be a 20,000 BPM
                    // record.
                    let (mut app, id) = on_the_grid("grid-mark-slip");
                    assert!(app.mark_beat(id, 10_000));
                    assert!(app.mark_beat(id, 10_020));
                    assert_eq!(app.library.get(id).unwrap().beat_marks, vec![10_020]);

                    assert!(app.mark_beat(id, 12_000));
                    assert_eq!(app.library.get(id).unwrap().beat_marks, vec![10_020, 12_000]);
                }

                #[test]
                fn the_marks_are_only_drawn_while_the_grid_is_being_corrected() {
                    // They are scaffolding for a fit, not a thing to look at
                    // the rest of the time.
                    let (mut app, id) = on_the_grid("grid-marks-hidden");
                    app.library.get_mut(id).unwrap().beat_marks = vec![1_000, 2_000];
                    app.editing_grid = false;
                    app.rebuild();
                    let mut harness = panel(app);
                    harness.run();
                    assert!(harness.query_by_label("clear").is_none(), "they were on show");

                    harness.state_mut().editing_grid = true;
                    harness.run();
                    harness.get_by_label("clear");
                }

                #[test]
                fn the_two_fixes_that_need_a_place_say_so_when_there_is_none() {
                    // Halving a tempo is true of the whole track; putting a
                    // beat somewhere is a question about one place in it, and
                    // the window's only answer to "where" is the playhead.
                    let (mut app, _) = on_the_grid("grid-no-playhead");
                    app.editing_grid = true;
                    app.playhead_ms = None;

                    let mut harness = panel(app);
                    harness.run();
                    let placed = harness.get_by_label("One here");
                    assert!(format!("{placed:?}").contains("disabled: true"), "{placed:?}");
                    let tempo = harness.get_by_label("×2");
                    assert!(format!("{tempo:?}").contains("disabled: false"), "{tempo:?}");
                }

                #[test]
                fn setting_the_one_from_the_panel_moves_the_phase_and_not_the_tempo() {
                    let (mut app, id) = on_the_grid("grid-downbeat");
                    app.editing_grid = true;
                    // A beat every 468.75 ms; park on the second one.
                    app.playhead_ms = Some(469);

                    let mut harness = panel(app);
                    harness.run();
                    harness.get_by_label("One here").click();
                    harness.run();
                    let ctx = harness.ctx.clone();
                    harness.state_mut().apply_pending(&ctx);

                    let track = harness.state().library.get(id).unwrap();
                    assert_eq!(track.downbeat_ms, Some(469));
                    assert_eq!(track.bpm, 128.0, "a phase change is not a tempo change");
                }

                #[test]
                fn the_panel_says_where_a_track_got_its_words() {
                    // The case it is for: a cue that reads oddly. A
                    // recogniser's mistakes still read like lyrics, so the
                    // only way to tell a doubtful cue from a solid one is to
                    // know which kind of answer it came from.
                    let (mut app, id) = sung("provenance", &[(40_000, "hold me closer now")]);
                    let track = app.library.get_mut(id).unwrap();
                    track.words_from = WordsFrom::Recogniser;
                    track.heard_surely = Some(0.4);
                    app.rebuild();
                    app.selected = Some(id);

                    // Tall, because the words sit below the tags and the file
                    // in a scroll area, and what a default-sized harness
                    // clips is not in the tree to be found.
                    let mut harness = Harness::builder()
                        .with_size(egui::vec2(360.0, 1600.0))
                        .build_ui_state(|ui, app: &mut App| app.inspector(ui), app);
                    harness.run();
                    harness.get_by_label_contains("heard off the stem");
                    harness.get_by_label_contains("heard poorly");

                    harness.state_mut().library.get_mut(id).unwrap().words_from = WordsFrom::Server;
                    harness.state_mut().rebuild();
                    harness.run();
                    harness.get_by_label_contains("looked up");
                }

                #[test]
                fn the_sheet_asking_about_found_words_shows_both_sets_and_takes_them() {
                    let (mut app, id) = sung("wondering", &[(40_000, "hold me closer now")]);
                    let offered = found(&[(1_000, "hold me closer now"), (9_000, "all night")]);
                    app.wondering.push(Wondering { id, found: offered });

                    let mut harness =
                        Harness::new_state(|ctx, app: &mut App| app.wondering_sheet(ctx), app);
                    harness.run();
                    // Both sides of the judgement, because the judgement is
                    // whether these two are the same record.
                    harness.get_by_label_contains("all night");
                    harness.get_by_label_contains("The same length as this track");

                    harness.get_by_label(&theme::label_text("These are the words")).click();
                    harness.run();

                    assert!(harness.state().wondering.is_empty(), "the sheet went unanswered");
                    assert!(
                        matches!(
                            harness.state().pending.first(),
                            Some(Pending::TakeFoundWords(..))
                        ),
                        "agreeing to them should take them"
                    );
                }
            }
        }

        /// Click the named button, type into the field it opens, press a key,
        /// and hand what the collection became to `check`. Each step gets its
        /// own frame, because that is how a person does it and the difference
        /// between one frame and three is the whole point.
        fn name_something(
            name: &str,
            button: &str,
            text: &str,
            key: egui::Key,
            check: impl FnOnce(&Library),
        ) {
            let mut harness = Harness::new_ui_state(|ui, app: &mut App| app.sidebar(ui), app(name));
            harness.get_by_label(button).click();
            harness.run();
            harness.get_by_role(accesskit::Role::TextInput).type_text(text);
            harness.run();
            harness.key_press(key);
            harness.run();
            check(&harness.state().library);
        }

        #[test]
        fn typing_a_name_and_pressing_enter_makes_a_playlist() {
            name_something("playlist", "+", "Saturday peak", egui::Key::Enter, |library| {
                let names: Vec<&str> = library.playlists.iter().map(|p| p.name.as_str()).collect();
                assert_eq!(names, vec!["Saturday peak"], "Enter did not make the playlist");
            });
        }

        #[test]
        fn typing_a_name_and_pressing_enter_makes_a_folder() {
            name_something("folder", "+\u{25be}", "September", egui::Key::Enter, |library| {
                assert_eq!(library.folders, vec!["September".to_string()]);
            });
        }

        /// A window showing one track, with the inspector open on it.
        fn inspecting(name: &str, path: &str) -> App {
            let mut app = app(name);
            let id = app.library.add(std::path::Path::new(path));
            let track = app.library.get_mut(id).unwrap();
            track.artist = "Unknown".into();
            track.title = "02 Tension".into();
            app.rebuild();
            app.selected = Some(id);
            app
        }

        /// A track with words and an account of what cueing from them did.
        fn with_words(name: &str) -> (App, u32) {
            let mut app = inspecting(name, "/music/02 Tension.m4a");
            let id = app.selected.unwrap();
            let track = app.library.get_mut(id).unwrap();
            track.lyrics = vec![
                crate::library::Lyric {
                    start_ms: 40_000,
                    end_ms: 42_000,
                    text: "hold me closer now".into(),
                },
                crate::library::Lyric {
                    start_ms: 44_000,
                    end_ms: 46_000,
                    text: "nothing here but us".into(),
                },
            ];
            track.words_decided =
                vec!["hook: \u{201c}hold me closer now\u{201d}, sung 3 times".into()];
            app.rebuild();
            (app, id)
        }

        #[test]
        fn every_line_the_track_comes_back_to_is_reachable() {
            // It showed the first four and stopped. A record whose fifth most
            // repeated line is the one somebody recognises it by had no way to
            // that line at all — and the cap was there because the panel grew
            // with the track, which is what a scroll area is for.
            let mut app = inspecting("sayings-all", "/music/02 Tension.m4a");
            let id = app.selected.unwrap();
            // Seven lines, each sung one time fewer than the last, so the
            // order is unambiguous and the seventh is well past the old cap.
            //
            // Plainly different from each other, because lines that share most
            // of their words are counted as the same line — seven readings of
            // "line number N" came back as one refrain heard twenty-nine
            // times, which is the grouping working and the fixture not. And
            // spread well apart, because repeats closer together than a few
            // seconds are one hearing.
            const LINES: [&str; 7] = [
                "hold me closer now",
                "the city will not sleep",
                "everybody in the room",
                "nothing here but us",
                "waiting for the sun",
                "let the record spin",
                "come back when it ends",
            ];
            {
                let track = app.library.get_mut(id).unwrap();
                track.lyrics = LINES
                    .iter()
                    .enumerate()
                    .flat_map(|(line, text)| {
                        (0..(8 - line as u32)).map(move |n| {
                            let at = 10_000 * line as u32 + 90_000 * n;
                            crate::library::Lyric {
                                start_ms: at,
                                end_ms: at + 2_000,
                                text: (*text).to_string(),
                            }
                        })
                    })
                    .collect();
                track.refrains = crate::library::refrains_from(&track.lyrics.clone());
            }
            app.rebuild();
            assert!(
                app.library.get(id).unwrap().refrains.len() >= 7,
                "the fixture does not have enough repeated lines to test the cap"
            );

            let mut harness = Harness::builder()
                .with_size(egui::vec2(420.0, 1_600.0))
                .build_ui_state(|ui, app: &mut App| app.inspector(ui), app);
            harness.run();

            // By the count beside each line, not by the line: the lyric
            // sheet below this panel lists every line of the track, so
            // looking for the words found them whether or not this panel had
            // drawn them. The first version of this test passed with the cap
            // still in place for exactly that reason.
            for (line, times) in LINES.iter().zip((2..=8).rev()) {
                assert!(
                    harness.query_all_by_label_contains(&format!("{times}\u{d7}")).count() > 0,
                    "{line:?}, sung {times} times, is not in the panel"
                );
            }
        }

        #[test]
        fn the_panel_shows_the_words_themselves_and_not_only_a_count_of_them() {
            // What was missing: a line count, where they came from and four
            // repeated lines, with the lyric all of that was drawn from
            // nowhere on screen. The whole of it is only in the collection —
            // what reaches a player is a comment shortened to fit a
            // four-kilobyte row.
            let (app, _) = with_words("words-shown");
            let mut harness = Harness::builder()
                .with_size(egui::vec2(420.0, 1_400.0))
                .build_ui_state(|ui, app: &mut App| app.inspector(ui), app);
            harness.run();

            // `query_all`, because the hook line is quoted in the account of
            // what the cueing decided as well as being in the lyric — which
            // is the point of showing both, not a clash.
            assert!(
                harness.query_all_by_label_contains("hold me closer now").count() > 0,
                "the first line is not on screen"
            );
            harness.get_by_label_contains("nothing here but us");
            // And each line says when it lands, which is what makes it worth
            // reading next to a waveform.
            harness.get_by_label_contains("0:40.00");
        }

        #[test]
        fn clicking_the_time_beside_a_line_goes_to_it() {
            let (app, _) = with_words("words-seek");
            let mut harness = Harness::builder()
                .with_size(egui::vec2(420.0, 1_400.0))
                .build_ui_state(|ui, app: &mut App| app.inspector(ui), app);
            harness.run();
            harness.get_by_label_contains("0:44.00").click();
            harness.run();

            assert_eq!(harness.state().playhead_ms, Some(44_000));
        }

        #[test]
        fn the_panel_says_why_the_cues_are_where_they_are() {
            // The pass that places cues says all this as it goes, and used to
            // say it only to the log — a record that lasts until the next
            // thing is logged.
            let (app, _) = with_words("words-decided");
            let mut harness = Harness::builder()
                .with_size(egui::vec2(420.0, 1_400.0))
                .build_ui_state(|ui, app: &mut App| app.inspector(ui), app);
            harness.run();

            harness.get_by_label_contains("hook:");
        }

        #[test]
        fn a_track_with_no_words_is_not_given_empty_panels_about_them() {
            let app = inspecting("words-none", "/music/02 Tension.m4a");
            let mut harness = Harness::builder()
                .with_size(egui::vec2(420.0, 1_400.0))
                .build_ui_state(|ui, app: &mut App| app.inspector(ui), app);
            harness.run();

            assert!(harness.query_by_label("The words").is_none());
            assert!(harness.query_by_label("What the words decided").is_none());
        }

        #[test]
        fn a_lookup_reaches_the_panel_that_is_showing_the_track() {
            // The panel keeps its own copy of the names so that typing is not
            // the collection until it is committed. The copy has to notice
            // when the record changes underneath it — a fingerprint lookup
            // writes one while the panel is open, and showing the old name
            // afterwards reads as the lookup having done nothing.
            let mut harness = Harness::new_ui_state(
                |ui, app: &mut App| app.inspector(ui),
                inspecting("lookup", "/music/02 Tension.m4a"),
            );
            harness.run();

            // What arriving at a fingerprint match does: write the names into
            // the collection and rebuild the list.
            let id = harness.state().selected.unwrap();
            let app = harness.state_mut();
            let track = app.library.get_mut(id).unwrap();
            track.artist = "All Your Sisters".into();
            track.title = "Tension".into();
            app.rebuild();
            harness.run();

            let edit = harness.state().editing.as_ref().expect("the panel kept no names");
            assert_eq!(edit.names.artist, "All Your Sisters", "the panel still shows the old name");
            assert_eq!(edit.names.title, "Tension");
        }

        #[test]
        fn a_lookup_does_not_overwrite_what_somebody_is_typing() {
            // The other half of the same rule: a field being typed into is
            // theirs, and a lookup landing mid-edit must not take it back.
            let mut harness = Harness::new_ui_state(
                |ui, app: &mut App| app.inspector(ui),
                inspecting("typing", "/music/02 Tension.m4a"),
            );
            harness.run();
            // Focused first: typing goes wherever the keyboard is, and a frame
            // has to pass for the field to have it.
            harness.get_by_role_and_label(accesskit::Role::TextInput, "Artist").focus();
            harness.run();
            harness
                .get_by_role_and_label(accesskit::Role::TextInput, "Artist")
                .type_text("My own answer");
            harness.run();

            let id = harness.state().selected.unwrap();
            let app = harness.state_mut();
            app.library.get_mut(id).unwrap().artist = "All Your Sisters".into();
            app.rebuild();
            harness.run();

            let edit = harness.state().editing.as_ref().expect("the panel kept no names");
            assert!(
                edit.names.artist.contains("My own answer"),
                "a lookup took back what was being typed: {:?}",
                edit.names.artist
            );
        }

        #[test]
        fn an_example_from_the_help_can_be_clicked_into_the_bar() {
            // The distance between reading an example and trying it is most of
            // what makes a query language worth having.
            let mut app = app("help");
            app.help = true;
            let mut harness = Harness::new_state(|ctx, app: &mut App| app.help_sheet(ctx), app);
            harness.run();

            harness.get_by_label("key:~8A").click();
            harness.run();

            assert_eq!(harness.state().text, "key:~8A", "the example did not reach the bar");
            assert!(
                harness.state().query.terms.iter().all(|t| t.test != crate::query::Test::Invalid),
                "and it has to be a query, not just text"
            );
        }

        #[test]
        fn the_sheet_warning_about_stems_says_the_cost_and_starts_the_batch() {
            let mut app = app("stems-sheet");
            let mut ids = Vec::new();
            for name in ["one.flac", "two.flac"] {
                let path = app.library_path.with_file_name(name);
                std::fs::write(&path, b"not really a flac").unwrap();
                let id = app.library.add(&path);
                app.library.get_mut(id).unwrap().duration_secs = 300.0;
                ids.push(id);
            }
            app.rebuild();
            // Half of real time, measured over the separations before this
            // one: ten minutes of music is five minutes of work.
            app.config.stem_pace = Some(0.5);
            app.stems_ahead = Some(StemsAhead { tracks: ids, for_cues: true });

            let mut harness = Harness::new_state(|ctx, app: &mut App| app.stems_sheet(ctx), app);
            harness.run();
            harness.get_by_label_contains("about 5 minutes");

            harness.get_by_label(&theme::label_text("Render stems")).click();
            harness.run();

            assert!(harness.state().stems_ahead.is_none(), "the sheet went unanswered");
            assert!(
                matches!(
                    harness.state().pending.first(),
                    Some(Pending::SeparateMany { for_cues: true, .. })
                ),
                "agreeing to it should start the batch: {:?}",
                harness.state().pending.len()
            );
        }

        /// A window with one track and one playlist, showing browser and
        /// sidebar side by side — which is what a drag crosses.
        /// Draw a pane in a box of exactly this size, the way a side panel
        /// does in the window. Returns the box.
        ///
        /// Not decoration, and not a size plucked out of the air. The sidebar
        /// fills whatever it is handed — since it gained a `ScrollArea` it
        /// fills it in both directions — and a `Ui` that merely *allocates* a
        /// box does not stop it: `max_rect` is where egui lays out to, not a
        /// wall. Handed the harness with nothing holding it, the sidebar took
        /// all 784 points of it and left the browser a list zero pixels wide,
        /// which is not a list a row can be dragged out of.
        ///
        /// So this is [`pinned`] with the box stated outright, and every part
        /// of it has to be. Inside a `horizontal`, `available_rect_before_wrap`
        /// reports the height of the row *so far* — eighteen points on the
        /// first frame — and a clip rect eighteen points tall is a playlist
        /// that `contains_pointer` says the pointer is not over, on the one
        /// frame where that decides whether a drop lands. The layout is stated
        /// for the same kind of reason: inherited from the enclosing
        /// `horizontal`, the sidebar lays itself out sideways.
        fn in_a_panel(ui: &mut Ui, size: egui::Vec2, contents: impl FnOnce(&mut Ui)) -> egui::Rect {
            let rect = egui::Rect::from_min_size(ui.available_rect_before_wrap().min, size);
            let mut inner = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(rect)
                    .layout(egui::Layout::top_down(egui::Align::Min)),
            );
            inner.shrink_clip_rect(rect);
            contents(&mut inner);
            ui.advance_cursor_after_rect(rect);
            rect
        }

        /// Wide enough for the browser to have room beside it, and tall enough
        /// that a short collection does not scroll.
        const PANEL: egui::Vec2 = egui::vec2(200.0, 560.0);
        /// Short enough that any collection overruns it.
        const SHORT_PANEL: egui::Vec2 = egui::vec2(200.0, 200.0);

        /// Where the panel's bottom edge ended up, noted as it was drawn.
        ///
        /// "Below the fold" is a claim about the edge the harness actually put
        /// there. A number typed in beside the test would go on passing after
        /// it moved.
        #[derive(Clone, Default)]
        struct Fold(std::sync::Arc<std::sync::atomic::AtomicU32>);

        impl Fold {
            fn note(&self, at: f32) {
                self.0.store(at.to_bits(), std::sync::atomic::Ordering::Relaxed);
            }

            fn at(&self) -> f32 {
                f32::from_bits(self.0.load(std::sync::atomic::Ordering::Relaxed))
            }
        }

        /// A collection with more playlists than any panel could show at once.
        fn with_many_playlists(name: &str, count: usize) -> App {
            let mut app = app(name);
            for i in 0..count {
                app.library.add_playlist(&format!("list {i:02}"), "").unwrap();
            }
            app.rebuild();
            app
        }

        /// Carry something from one place to another with the pointer.
        ///
        /// Pressed, moved, released — a drag is not one event, and egui only
        /// starts one once the pointer has actually travelled.
        fn drag(harness: &mut Harness<'_, App>, from: egui::Pos2, onto: egui::Pos2) {
            harness.event(egui::Event::PointerMoved(from));
            harness.run();
            harness.event(egui::Event::PointerButton {
                pos: from,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            });
            harness.run();
            for step in 1..=4 {
                let at = from + (onto - from) * (step as f32 / 4.0);
                harness.event(egui::Event::PointerMoved(at));
                harness.run();
            }
            harness.event(egui::Event::PointerButton {
                pos: onto,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            });
            harness.run();
            harness.run();
        }

        /// Spin the wheel over the panel, which is how a person reaches the
        /// bottom of it.
        ///
        /// `run_ok` rather than `run`: a scroll bar fades out over a second or
        /// so, which is a repaint per frame for as long as it takes, and the
        /// harness treats a ui that keeps asking to be redrawn as a runaway.
        fn scroll(harness: &mut Harness<'_, App>, over: egui::Pos2, by: f32) {
            harness.event(egui::Event::PointerMoved(over));
            harness.run_ok();
            harness.event(egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(0.0, by),
                modifiers: egui::Modifiers::NONE,
            });
            harness.run_ok();
        }

        #[test]
        fn a_playlist_below_the_fold_can_be_reached() {
            // What the panel had no answer for. A collection with more
            // playlists than there is room for used to run them off the bottom
            // of the screen; once the panel began clipping to its own
            // rectangle they stopped being drawn at all, and either way there
            // was no way to get at them.
            let fold = Fold::default();
            let edge = fold.clone();
            let mut harness = Harness::new_ui_state(
                move |ui, app: &mut App| {
                    edge.note(in_a_panel(ui, SHORT_PANEL, |ui| app.sidebar(ui)).bottom());
                },
                with_many_playlists("scrolling", 40),
            );
            harness.run();
            let fold = fold.at();

            let last = harness.query_by_label("list 39").map(|node| node.rect());
            assert!(
                last.is_none_or(|rect| rect.top() >= fold),
                "the last playlist is already in view, so this proves nothing: {last:?}"
            );

            scroll(&mut harness, egui::pos2(100.0, 100.0), -4_000.0);

            let last = harness
                .query_by_label("list 39")
                .unwrap_or_else(|| panic!("the last playlist is still not drawn"))
                .rect();
            assert!(last.top() < fold, "scrolling did not bring it into the panel: {last:?}");
        }

        #[test]
        fn a_row_can_be_dropped_on_a_playlist_that_had_to_be_scrolled_to() {
            // The two halves of this panel meeting: a playlist reachable only
            // by scrolling still has to be a thing a track can be dropped on.
            // A scroll area moves its contents without moving the rectangles
            // egui interacts against unless everything agrees, and a drop that
            // lands on where a playlist *was* is the kind of fault that looks
            // from the outside exactly like nothing happening.
            let mut app = with_many_playlists("dropping-after-scrolling", 40);
            let id = app.library.add(std::path::Path::new("/music/Sirens.flac"));
            let track = app.library.get_mut(id).unwrap();
            track.artist = "Peverelist".into();
            track.title = "Sirens".into();
            app.rebuild();

            let fold = Fold::default();
            let edge = fold.clone();
            let mut harness = Harness::new_ui_state(
                move |ui, app: &mut App| {
                    ui.horizontal(|ui| {
                        edge.note(in_a_panel(ui, SHORT_PANEL, |ui| app.sidebar(ui)).bottom());
                        ui.vertical(|ui| {
                            let widths = rows::Layout::default().widths(ui.available_width());
                            app.rows_table(ui, &widths)
                        });
                    });
                    let ctx = ui.ctx().clone();
                    app.apply_pending(&ctx);
                },
                app,
            );
            harness.run();

            let before = harness.get_by_label("list 39").rect();
            assert!(
                before.top() >= fold.at(),
                "the target is on screen already, so this proves nothing: {before:?}"
            );

            // To the bottom of the list, so the target is one that was not on
            // screen a moment ago.
            scroll(&mut harness, egui::pos2(100.0, 100.0), -4_000.0);

            let from = harness.get_by_label_contains("Sirens").rect().center();
            let onto = harness.get_by_label("list 39").rect().center();
            assert!(onto.y < fold.at(), "the target is still below the panel: {onto:?}");
            drag(&mut harness, from, onto);

            let reached = harness
                .state()
                .library
                .playlists
                .iter()
                .find(|playlist| playlist.name == "list 39")
                .expect("the playlist went missing");
            assert_eq!(reached.tracks, vec![id], "the drop did not reach the scrolled playlist");
        }

        fn with_a_track_and_a_playlist(name: &str) -> App {
            let mut app = app(name);
            let id = app.library.add(std::path::Path::new("/music/Sirens.flac"));
            let track = app.library.get_mut(id).unwrap();
            track.artist = "Peverelist".into();
            track.title = "Sirens".into();
            app.library.add_playlist("Saturday peak", "").unwrap();
            app.rebuild();
            app
        }

        #[test]
        fn a_row_can_be_dragged_onto_a_playlist() {
            // The thing this was all for. Drag-and-drop is a chain — the row
            // has to be draggable, the payload has to be set, the sidebar has
            // to accept it — and any link being missing looks identical from
            // the outside: nothing happens.
            let mut harness = Harness::new_ui_state(
                |ui, app: &mut App| {
                    ui.horizontal(|ui| {
                        in_a_panel(ui, PANEL, |ui| app.sidebar(ui));
                        ui.vertical(|ui| {
                            let widths = rows::Layout::default().widths(ui.available_width());
                            app.rows_table(ui, &widths)
                        });
                    });
                    // As the window does: the panels ask, and what they asked
                    // for happens once they have all drawn.
                    let ctx = ui.ctx().clone();
                    app.apply_pending(&ctx);
                },
                with_a_track_and_a_playlist("dragging"),
            );
            harness.run();

            let from = harness.get_by_label_contains("Sirens").rect().center();
            let onto = harness.get_by_label("Saturday peak").rect().center();
            drag(&mut harness, from, onto);

            let playlist = &harness.state().library.playlists[0];
            assert_eq!(playlist.tracks.len(), 1, "the drop did not add the track");
        }

        /// A window on `n` tracks, listed and ready to be selected in.
        fn listing(name: &str, n: u32) -> App {
            let mut app = app(name);
            for i in 1..=n {
                let id = app.library.add(std::path::Path::new(&format!("/music/{i}.flac")));
                let track = app.library.get_mut(id).unwrap();
                track.artist = format!("Artist {i:02}");
                track.title = format!("Track {i:02}");
            }
            app.library.add_playlist("peak", "").unwrap();
            app.rebuild();
            app
        }

        /// The selection, in the order the list has them.
        fn chosen(app: &App) -> Vec<u32> {
            app.rows
                .iter()
                .filter(|row| !row.indented && app.marked.contains(&row.track.id))
                .map(|row| row.track.id)
                .collect()
        }

        #[test]
        fn shift_and_an_arrow_takes_a_run_of_rows() {
            let mut app = listing("arrows", 5);
            let ids: Vec<u32> = app.rows.iter().map(|row| row.track.id).collect();

            // A rebuilt list already has its first row under the cursor, so
            // one press down is the second row.
            app.step(1, false);
            assert_eq!(chosen(&app), vec![ids[1]], "a plain arrow takes one");

            app.step(1, true);
            app.step(1, true);
            assert_eq!(chosen(&app), ids[1..=3], "shift extended from where it started");

            // Back up again, still holding shift: the range shrinks rather
            // than the rows behind it staying picked.
            app.step(-1, true);
            assert_eq!(chosen(&app), ids[1..=2]);

            // And letting go starts again from wherever the cursor is.
            app.step(1, false);
            assert_eq!(chosen(&app), vec![ids[3]]);
        }

        #[test]
        fn a_range_reads_the_same_drawn_upwards() {
            let mut app = listing("upwards", 5);
            let ids: Vec<u32> = app.rows.iter().map(|row| row.track.id).collect();

            app.selected = Some(ids[3]);
            app.mark_only(ids[3]);
            app.mark_range_to(ids[1]);
            assert_eq!(chosen(&app), ids[1..=3], "picked upwards, listed downwards");
        }

        #[test]
        fn an_action_takes_the_selection_when_there_is_one() {
            let mut app = listing("acting", 5);
            let ids: Vec<u32> = app.rows.iter().map(|row| row.track.id).collect();

            // Nothing chosen: everything showing.
            assert_eq!(app.acting_on(|_| true).len(), 5);

            // One row is where the cursor is, not a selection — otherwise
            // clicking a track to look at its waveform would quietly narrow
            // every button in the window to that one track.
            app.mark_only(ids[0]);
            assert_eq!(app.acting_on(|_| true).len(), 5, "one row narrowed everything");

            app.mark_range_to(ids[2]);
            assert_eq!(app.acting_on(|_| true), ids[..3], "the selection is what acts");
        }

        #[test]
        fn scrolling_the_list_leaves_the_column_headers_where_they_are() {
            // The header is what says which column is which and is the only way
            // to sort by one, so scrolling a long list past it would take away
            // the thing that makes the list readable at the moment there is
            // most list to read.
            let mut app = app("header");
            for n in 0..120 {
                let id = app.library.add(&std::path::PathBuf::from(format!("/music/{n}.flac")));
                let track = app.library.get_mut(id).unwrap();
                track.artist = "Peverelist".into();
                track.title = format!("Track {n}");
            }
            app.rebuild();

            let mut harness = Harness::new_ui_state(|ui, app: &mut App| app.browser(ui), app);
            harness.run();

            let header = harness.get_by_label("columns").rect();
            let row_before = harness.get_by_label("Peverelist — Track 0").rect();

            harness.get_by_label("Peverelist — Track 0").scroll_down();
            harness.run();
            harness.run();

            assert_eq!(
                harness.get_by_label("columns").rect(),
                header,
                "the header moved when the list was scrolled"
            );
            assert!(
                harness.get_by_label("Peverelist — Track 0").rect().top() < row_before.top(),
                "the list did not scroll, so this proves nothing"
            );
        }

        #[test]
        fn dragging_the_header_resizes_the_column_and_the_size_is_kept() {
            // The whole chain, because any link missing looks the same from
            // the outside: the boundary has to be grabbable where the eye says
            // it is, the drag has to move width from one column to the next,
            // and the result has to reach the settings rather than lasting
            // until the window is closed.
            let mut app = app("resize");
            let id = app.library.add(std::path::Path::new("/music/one.flac"));
            app.library.get_mut(id).unwrap().artist = "Peverelist".into();
            app.rebuild();

            let mut harness = Harness::new_ui_state(
                |ui, app: &mut App| {
                    app.browser(ui);
                    // As the window does at the end of a frame: what a drag
                    // changed is written once the pointer comes up.
                    let ctx = ui.ctx().clone();
                    app.save_layout(&ctx);
                },
                app,
            );
            harness.run();

            // The header is exactly as wide as the columns, so where the
            // boundary is can be worked out the same way the header worked it
            // out — no guessing at a scroll bar's width.
            let header = harness.get_by_label("columns").rect();
            let widths = harness.state().config.columns.widths(header.width());
            let artist = widths.of(rows::Column::Artist).unwrap();
            let title = widths.of(rows::Column::Title).unwrap();
            let edge = egui::pos2(header.left() + artist, header.center().y);

            harness.event(egui::Event::PointerMoved(edge));
            harness.run();
            harness.event(egui::Event::PointerButton {
                pos: edge,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            });
            harness.run();
            for step in 1..=4 {
                let at = edge + egui::vec2(15.0 * step as f32, 0.0);
                harness.event(egui::Event::PointerMoved(at));
                harness.run();
            }
            harness.event(egui::Event::PointerButton {
                pos: edge + egui::vec2(60.0, 0.0),
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            });
            harness.run();
            harness.run();

            let after = harness.state().config.columns.widths(header.width());
            assert!(
                after.of(rows::Column::Artist).unwrap() > artist + 50.0,
                "the artist column did not follow the drag: {artist} to {:?}",
                after.of(rows::Column::Artist)
            );
            assert!(
                after.of(rows::Column::Title).unwrap() < title - 50.0,
                "the width came from somewhere other than the column beside it"
            );
            assert!(
                (after.total() - header.width()).abs() < 0.01,
                "the row stopped filling the window"
            );

            // Written out once the pointer came up, not left in memory to be
            // lost with the window.
            let saved = crate::config::Config::load(&harness.state().config_path);
            assert_eq!(
                saved.columns,
                harness.state().config.columns,
                "the new widths never reached the settings"
            );
            assert_ne!(saved.columns, rows::Layout::default(), "nothing was actually changed");
        }

        #[test]
        fn the_header_menu_takes_a_column_away_and_puts_them_all_back() {
            // The way a column is got rid of and the way one is got back. Both
            // live on the header's right-click menu, so a menu that lists only
            // what is showing — or that has no way back to the defaults — is a
            // door that locks behind you.
            let mut app = app("columns");
            let id = app.library.add(std::path::Path::new("/music/one.flac"));
            app.library.get_mut(id).unwrap().artist = "Peverelist".into();
            app.rebuild();

            let mut harness = Harness::new_ui_state(|ui, app: &mut App| app.browser(ui), app);
            harness.run();

            let at = harness.get_by_label("columns").rect().center();
            right_click(&mut harness, at);

            // Every column, on or off, not just the ones showing.
            for column in rows::Column::ALL {
                harness.get_by_label(column.name());
            }
            harness.get_by_label("Reset to default");

            harness.get_by_label(rows::Column::Location.name()).click();
            harness.run();
            harness.run();
            let layout = &harness.state().config.columns;
            assert!(
                !layout.columns.iter().any(|s| s.column == rows::Column::Location && s.shown),
                "the location column is still showing"
            );
            assert_eq!(
                harness.state().config.columns.widths(900.0).iter().count(),
                rows::Column::ALL.len() - 1
            );

            right_click(&mut harness, at);
            harness.get_by_label("Reset to default").click();
            harness.run();
            harness.run();
            assert_eq!(
                harness.state().config.columns,
                rows::Layout::default(),
                "reset did not put the columns back"
            );
        }

        /// Open a context menu where the pointer is put.
        fn right_click(harness: &mut Harness<'_, App>, at: egui::Pos2) {
            for pressed in [true, false] {
                harness.event(egui::Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Secondary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                });
                harness.run();
            }
        }

        /// A window on a listing, with the keys read as the real one reads
        /// them: `keys` runs before the panels, as it does in `update`.
        fn browsing(name: &str, n: u32) -> Harness<'static, App> {
            let mut app = listing(name, n);
            app.selected = app.rows.first().map(|row| row.track.id);
            let mut harness = Harness::new_ui_state(
                |ui, app: &mut App| {
                    let ctx = ui.ctx().clone();
                    app.keys(&ctx);
                    app.browser(ui);
                    app.apply_pending(&ctx);
                },
                app,
            );
            harness.run();
            harness
        }

        fn press(harness: &mut Harness<'_, App>, key: egui::Key, modifiers: egui::Modifiers) {
            // On the raw input as well as on the event: the window reads
            // `i.modifiers`, which is what the platform says is held down
            // rather than what any one event carries, and that is the thing a
            // real shift-press changes.
            harness.input_mut().modifiers = modifiers;
            for pressed in [true, false] {
                harness.event(egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed,
                    repeat: false,
                    modifiers,
                });
                harness.run();
            }
            harness.input_mut().modifiers = egui::Modifiers::NONE;
        }

        /// The rows the window would act on, in list order.
        fn picked(harness: &Harness<'_, App>) -> Vec<u32> {
            let app = harness.state();
            app.rows
                .iter()
                .filter(|row| app.marked.contains(&row.track.id))
                .map(|row| row.track.id)
                .collect()
        }

        #[test]
        fn command_a_takes_everything_the_query_left_showing() {
            // Everything showing, not the whole collection: narrowing to what
            // you want and then taking all of it is the gesture the query bar
            // is for, and reaching past the filter would undo the narrowing.
            let mut harness = browsing("select all", 12);
            harness.state_mut().text = "Track 1".into();
            harness.state_mut().rebuild();
            harness.run();
            let showing: Vec<u32> = harness.state().rows.iter().map(|row| row.track.id).collect();
            assert!(
                showing.len() < 12 && !showing.is_empty(),
                "the query did not narrow anything, so this proves nothing"
            );

            press(&mut harness, egui::Key::A, egui::Modifiers::COMMAND);
            assert_eq!(picked(&harness), showing, "command-A did not take what was showing");
            assert_eq!(
                harness.state().acting_on(|_| true),
                showing,
                "the buttons would still act on everything"
            );
        }

        #[test]
        fn shift_and_the_arrow_keys_grow_the_selection_from_where_the_cursor_was() {
            let mut harness = browsing("shift arrows", 6);
            let ids: Vec<u32> = harness.state().rows.iter().map(|row| row.track.id).collect();

            for _ in 0..3 {
                press(&mut harness, egui::Key::ArrowDown, egui::Modifiers::SHIFT);
            }
            assert_eq!(picked(&harness), ids[..4], "shift-down did not take the rows it passed");

            // Back up one: the range is measured from the anchor, so it
            // shrinks rather than leaving the row behind still selected.
            press(&mut harness, egui::Key::ArrowUp, egui::Modifiers::SHIFT);
            assert_eq!(picked(&harness), ids[..3], "coming back up left a row behind");

            // And without shift it is a cursor again, not a selection.
            press(&mut harness, egui::Key::ArrowDown, egui::Modifiers::NONE);
            assert_eq!(picked(&harness), ids[3..4], "a plain arrow kept the old selection");
        }

        #[test]
        fn shift_clicking_a_row_takes_everything_between() {
            let mut harness = browsing("shift click", 6);
            let ids: Vec<u32> = harness.state().rows.iter().map(|row| row.track.id).collect();

            let row = |harness: &Harness<'_, App>, at: usize| {
                harness.get_by_label_contains(&format!("Track {:02}", at + 1)).rect().center()
            };
            let first = row(&harness, 0);
            let fourth = row(&harness, 3);

            click(&mut harness, first, egui::Modifiers::NONE);
            assert_eq!(picked(&harness), ids[..1], "a plain click did not start a selection");

            click(&mut harness, fourth, egui::Modifiers::SHIFT);
            assert_eq!(picked(&harness), ids[..4], "shift-click did not take everything between");

            // The command key adds one on its own without disturbing the rest.
            let sixth = row(&harness, 5);
            click(&mut harness, sixth, egui::Modifiers::COMMAND);
            let mut wanted = ids[..4].to_vec();
            wanted.push(ids[5]);
            assert_eq!(picked(&harness), wanted, "command-click did not add a single row");
        }

        /// Click where the pointer is put, with modifiers held.
        fn click(harness: &mut Harness<'_, App>, at: egui::Pos2, modifiers: egui::Modifiers) {
            harness.input_mut().modifiers = modifiers;
            harness.event(egui::Event::PointerMoved(at));
            harness.run();
            for pressed in [true, false] {
                harness.event(egui::Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers,
                });
                harness.run();
            }
            harness.input_mut().modifiers = egui::Modifiers::NONE;
        }

        #[test]
        fn a_dragged_phrase_boundary_lands_on_a_bar() {
            // The strip reports which boundary moved and roughly where to; the
            // window decides where a boundary may actually land, because that
            // is a question about the grid and the strip does not know the
            // grid. A section that starts three beats into a bar is a section
            // in the wrong place however carefully it was dragged.
            let mut app = listing("phrase edit", 1);
            let id = app.rows[0].track.id;
            {
                let track = app.library.get_mut(id).unwrap();
                track.duration_secs = 120.0;
                track.bpm = 120.0;
                track.has_grid = true;
                track.phrases = vec![
                    crate::library::Phrase { start_ms: 0, end_ms: 30_000, kind: "intro".into() },
                    crate::library::Phrase {
                        start_ms: 30_000,
                        end_ms: 120_000,
                        kind: "drop".into(),
                    },
                ];
            }

            // At 120 BPM a beat is 500 ms and a bar 2,000 ms, so a boundary
            // asked for at 41.3 seconds belongs at 42.
            app.edit_phrase(id, crate::library::PhraseEdit::Move { at: 1, time_ms: 41_300 });
            let phrases = &app.library.get(id).unwrap().phrases;
            assert_eq!(phrases[0].end_ms % 2_000, 0, "off the bar: {}", phrases[0].end_ms);
            assert_eq!(
                phrases[0].end_ms, phrases[1].start_ms,
                "the two sections came apart at the boundary"
            );
            assert!(
                (phrases[1].start_ms as i64 - 41_300).abs() < 2_000,
                "it landed on a bar, but not the near one: {}",
                phrases[1].start_ms
            );

            // And a rename goes straight through, with no grid involved.
            app.edit_phrase(id, crate::library::PhraseEdit::Name { at: 0, kind: "build".into() });
            assert_eq!(app.library.get(id).unwrap().phrases[0].kind, "build");
        }

        #[test]
        fn switching_between_a_track_and_its_stems_keeps_the_place() {
            use crate::library::{companion_id, Role};
            let track = 7u32;
            let vocals = companion_id(track, Role::Vocals);
            let drums = companion_id(track, Role::Drums);
            let other = 9u32;

            // The deck is running the parent, forty seconds in.
            let deck = Some((track, 40.0, true));
            assert_eq!(
                carry(Some(track), vocals, deck),
                Carry::From { secs: 40.0, playing: true },
                "the vocal should come in where the track had got to"
            );
            // And between two stems of the same kit, with the deck on one of
            // them: still the same recording, still the same moment.
            assert_eq!(
                carry(Some(vocals), drums, Some((vocals, 40.0, true))),
                Carry::From { secs: 40.0, playing: true }
            );
            // And back to the original.
            assert_eq!(
                carry(Some(drums), track, Some((drums, 40.0, true))),
                Carry::From { secs: 40.0, playing: true }
            );

            // Paused, the playhead still moves — pressing play then picks it
            // up there — but nothing starts on its own.
            assert_eq!(
                carry(Some(track), vocals, Some((track, 40.0, false))),
                Carry::From { secs: 40.0, playing: false }
            );

            // A different record is a different record, whatever the deck is
            // doing: it starts at its own beginning.
            assert_eq!(carry(Some(track), other, deck), Carry::Restart);
            assert_eq!(carry(Some(other), vocals, Some((other, 40.0, true))), Carry::Restart);
            assert_eq!(carry(None, vocals, deck), Carry::Restart);

            // The same recording, but the deck is somewhere else entirely:
            // the playhead stays where it was rather than being dragged to
            // another record's position.
            assert_eq!(carry(Some(track), vocals, Some((other, 12.0, true))), Carry::Hold);
            assert_eq!(carry(Some(track), vocals, None), Carry::Hold);
        }

        #[test]
        fn clicking_a_stem_keeps_the_playhead_and_clicking_away_drops_it() {
            // The same rule as it arrives through the window: a click moves the
            // cursor, and what that does to the position is decided in one
            // place whether it came from the pointer or the arrow keys.
            let mut app = listing("stem switch", 2);
            let ids: Vec<u32> = app.rows.iter().map(|row| row.track.id).collect();
            app.selected = Some(ids[0]);
            app.playhead_ms = Some(40_000);

            let vocals = crate::library::companion_id(ids[0], crate::library::Role::Vocals);
            app.select(vocals);
            assert_eq!(app.selected, Some(vocals));
            assert_eq!(app.playhead_ms, Some(40_000), "the stem started from the top");

            // And on to a different track, where it means nothing.
            app.select(ids[1]);
            assert_eq!(app.playhead_ms, None, "a different record kept the old playhead");
        }

        #[test]
        fn the_strip_offers_a_lyrics_lookup_over_everything_showing() {
            // The cheap half of "Words" on a whole crate: seconds and one
            // request a track, against a separation and a recogniser pass
            // each. A box that has never been prepared is worth asking about
            // before any of those minutes are spent.
            let mut app = listing("lyrics-batch", 3);
            for id in app.library.tracks.iter().map(|t| t.id).collect::<Vec<_>>() {
                let track = app.library.get_mut(id).unwrap();
                track.artist = "Falco".into();
                track.title = "Der Kommissar".into();
            }
            app.rebuild();

            // Wide, because the strip is the fifth button along and a
            // default-sized harness wraps it off the end.
            let mut harness = Harness::builder()
                .with_size(egui::vec2(1400.0, 200.0))
                .build_ui_state(|ui, app: &mut App| app.actions(ui), app);
            harness.run();
            harness.get_by_label("Lyrics 3");
        }

        #[test]
        fn a_crate_with_no_names_is_not_offered_a_lookup() {
            // Most of a box of white labels. A lyrics database is asked by
            // artist and title, so a button that offered to ask about tracks
            // with neither would be a button that cannot work.
            let mut app = listing("lyrics-nameless", 3);
            for id in app.library.tracks.iter().map(|t| t.id).collect::<Vec<_>>() {
                app.library.get_mut(id).unwrap().artist.clear();
            }
            app.rebuild();
            let mut harness = Harness::builder()
                .with_size(egui::vec2(1400.0, 200.0))
                .build_ui_state(|ui, app: &mut App| app.actions(ui), app);
            harness.run();
            harness.get_by_label("Lyrics 0");
        }

        #[test]
        fn shift_turns_the_analyse_button_into_a_batch_re_analysis() {
            // The case it exists for: a collection where everything has
            // already been analysed, and something about what analysis
            // produces has changed. Before this the batch button only ever
            // offered the tracks that had never been done — which is none of
            // them — and the way to ask again was one right-click per row.
            let mut app = listing("re-analyse", 5);
            for id in app.library.tracks.iter().map(|t| t.id).collect::<Vec<_>>() {
                app.library.get_mut(id).unwrap().analyzed = true;
            }
            app.rebuild();

            let mut harness = Harness::new_ui_state(|ui, app: &mut App| app.actions(ui), app);
            harness.run();

            // Nothing to do, and the button says so.
            harness.get_by_label("Analyse 0");

            harness.input_mut().modifiers = egui::Modifiers::SHIFT;
            harness.run();
            harness.run();
            // The verb turns round and the count is the whole of what is
            // showing — the label has to say what pressing it will do, because
            // the two act on different numbers of tracks.
            harness.get_by_label("Re-analyse 5");
            assert!(harness.query_by_label("Analyse 0").is_none(), "both buttons are showing");

            harness.input_mut().modifiers = egui::Modifiers::NONE;
            harness.run();
            harness.run();
            harness.get_by_label("Analyse 0");
        }

        #[test]
        fn re_analysing_takes_everything_showing_rather_than_what_is_unfinished() {
            // The query bar decides what a batch is, the same as every other
            // button on that strip — so narrowing the list narrows the work,
            // and a re-analysis does not quietly reach past the filter.
            let mut app = listing("batch", 12);
            for id in app.library.tracks.iter().map(|t| t.id).collect::<Vec<_>>() {
                app.library.get_mut(id).unwrap().analyzed = true;
            }
            app.text = "Track 1".into();
            app.rebuild();

            let showing = app.acting_on(|_| true);
            assert!(
                showing.len() < 12 && !showing.is_empty(),
                "the query did not narrow anything, so this proves nothing"
            );
            assert!(
                app.acting_on(|track| !track.analyzed).is_empty(),
                "some of these still need a first analysis, so this proves nothing"
            );

            let mut harness = Harness::new_ui_state(|ui, app: &mut App| app.actions(ui), app);
            harness.run();
            harness.input_mut().modifiers = egui::Modifiers::SHIFT;
            harness.run();
            harness.get_by_label(&format!("Re-analyse {}", showing.len()));
        }

        #[test]
        fn the_panel_says_what_the_track_keeps_coming_back_to() {
            // The thing a DJ actually recognises a record by. It was already
            // measured — the cues are placed off it — and until now it was
            // only ever shown one line at a time on a marker.
            let mut app = listing("sayings", 2);
            let id = app.library.tracks[0].id;
            {
                let track = app.library.get_mut(id).unwrap();
                track.lyrics = [40_000, 100_000, 160_000]
                    .into_iter()
                    .map(|start_ms| crate::library::Lyric {
                        start_ms,
                        end_ms: start_ms + 2_000,
                        text: "hold me closer now".into(),
                    })
                    .collect();
                track.refrains = crate::library::refrains_from(&track.lyrics.clone());
            }
            app.selected = Some(id);
            app.rebuild();

            let mut harness = Harness::new_ui_state(|ui, app: &mut App| app.inspector(ui), app);
            harness.run();
            // `query_all` for the line and the time: the lyric sheet below
            // this panel shows the same words and the same moments, which is
            // the point of having both and not a clash. What is unique to
            // this panel is the count.
            assert!(
                harness.query_all_by_label_contains("hold me closer now").count() > 0,
                "the line that comes round is not on screen"
            );
            // How many times, which is the whole of the analysis.
            harness.get_by_label_contains("3\u{d7}");
            // And where, so a line half remembered can be gone to.
            assert!(
                harness.query_all_by_label_contains("0:40.00").count() > 0,
                "where it lands is not on screen"
            );
        }

        #[test]
        fn a_track_nobody_has_read_the_words_of_says_so_rather_than_nothing() {
            // An empty section reads as a feature that is broken. The three
            // cases are different answers and the panel tells them apart.
            let mut app = listing("unread", 2);
            app.selected = Some(app.library.tracks[0].id);
            app.rebuild();

            let mut harness = Harness::new_ui_state(|ui, app: &mut App| app.inspector(ui), app);
            harness.run();
            harness.get_by_label_contains("the words have not been read");
        }

        #[test]
        fn shift_turns_every_verb_on_the_strip_round_not_only_analyse() {
            // The case this exists for: a few tracks picked out of a library
            // that has already been through everything, and one of the
            // processes has improved since. Without it those tracks are simply
            // not counted — "Stems 0", greyed out — and there was no way to
            // ask for any of it a second time but one right-click per row.
            let mut app = listing("re-everything", 5);
            for id in app.library.tracks.iter().map(|t| t.id).collect::<Vec<_>>() {
                let track = app.library.get_mut(id).unwrap();
                track.analyzed = true;
                track.identified = true;
                track.stems.vocals = Some("/stems/vocals.wav".into());
                track.lyrics = vec![crate::library::Lyric {
                    start_ms: 0,
                    end_ms: 900,
                    text: "get down".into(),
                }];
            }
            app.rebuild();

            let mut harness = Harness::new_ui_state(|ui, app: &mut App| app.actions(ui), app);
            harness.run();
            for nothing in ["Analyse 0", "Identify 0", "Stems 0", "Words 0"] {
                harness.get_by_label(nothing);
            }

            harness.input_mut().modifiers = egui::Modifiers::SHIFT;
            harness.run();
            harness.run();
            for every in ["Re-analyse 5", "Re-identify 5", "Re-stem 5", "Re-read 5"] {
                harness.get_by_label(every);
            }
        }

        #[test]
        fn a_selection_is_what_the_turned_round_verbs_act_on() {
            // Which is the whole point of picking tracks out: the count on the
            // button is the selection, not the library behind it.
            let mut app = listing("re-selection", 8);
            for id in app.library.tracks.iter().map(|t| t.id).collect::<Vec<_>>() {
                let track = app.library.get_mut(id).unwrap();
                track.analyzed = true;
                track.identified = true;
                track.stems.vocals = Some("/stems/vocals.wav".into());
            }
            app.rebuild();
            app.marked = app.library.tracks.iter().take(3).map(|t| t.id).collect();

            let mut harness = Harness::new_ui_state(|ui, app: &mut App| app.actions(ui), app);
            harness.run();
            harness.input_mut().modifiers = egui::Modifiers::SHIFT;
            harness.run();
            harness.run();
            harness.get_by_label("Re-stem 3");
            harness.get_by_label("Re-identify 3");
            assert!(
                harness.query_by_label("Re-stem 8").is_none(),
                "the batch reached past the selection"
            );
        }

        #[test]
        fn a_section_in_its_own_window_is_not_also_in_this_one() {
            // The point of popping one out: the space it was taking goes to
            // what is left, rather than the same thing being drawn twice.
            let mut app = listing("popping", 4);
            app.selected = app.rows.first().map(|row| row.track.id);
            let mut harness = Harness::new_ui_state(|ui, app: &mut App| app.centre(ui), app);
            harness.run();
            assert!(harness.query_by_label("columns").is_some(), "the list was never there");

            harness.state_mut().set_out(Pane::Browser, true);
            harness.run();
            harness.run();
            assert!(
                harness.query_by_label("columns").is_none(),
                "the list is still in the middle of the main window"
            );

            harness.state_mut().set_out(Pane::Browser, false);
            harness.run();
            harness.run();
            assert!(harness.query_by_label("columns").is_some(), "putting it back did nothing");
        }

        #[test]
        fn the_middle_of_the_window_says_where_everything_went() {
            // An empty rectangle says neither what is missing nor how to get
            // it back, and both halves of the middle can be out at once.
            let mut app = listing("both out", 4);
            app.selected = app.rows.first().map(|row| row.track.id);
            app.set_out(Pane::Browser, true);
            app.set_out(Pane::Prep, true);

            let mut harness = Harness::new_ui_state(|ui, app: &mut App| app.centre(ui), app);
            harness.run();
            harness.get_by_label("Put them back").click();
            harness.run();
            harness.run();

            assert!(harness.state().config.popped.is_empty(), "they did not come back");
            assert!(harness.query_by_label("columns").is_some(), "the list did not come back");
        }

        #[test]
        fn the_menu_that_pops_a_section_out_is_on_the_bar_that_never_moves() {
            // Every section is listed, and the control is on the query bar
            // rather than inside the thing it acts on: a button that left with
            // the panel it belonged to would be a door that shuts behind you.
            let mut harness =
                Harness::new_ui_state(|ui, app: &mut App| app.command_bar(ui), listing("menu", 2));
            harness.run();
            harness.get_by_label("⧉").click();
            harness.run();
            for pane in Pane::ALL {
                harness.get_by_label(pane.title());
            }

            harness.get_by_label(Pane::Inspector.title()).click();
            harness.run();
            harness.run();
            assert_eq!(
                harness.state().config.popped,
                vec![Pane::Inspector],
                "the inspector did not go anywhere"
            );

            // And written down, because rebuilding a second-screen layout
            // every morning is not worth having.
            let saved = crate::config::Config::load(&harness.state().config_path);
            assert_eq!(saved.popped, vec![Pane::Inspector], "it never reached the settings");
        }

        #[test]
        fn a_popped_out_section_carries_its_own_way_back() {
            // The window it goes to has to offer a way home, because the menu
            // that sent it there is in the other window — which may be on the
            // other screen, or behind this one. Closing the window does the
            // same thing, so a section cannot be shut out of existence.
            //
            // Drawn embedded here: with no real windowing behind it, egui runs
            // an immediate viewport's contents in the context it was asked
            // from, which is exactly the callback the real one runs.
            let mut app = listing("its own window", 3);
            app.selected = app.rows.first().map(|row| row.track.id);
            app.set_out(Pane::Inspector, true);

            let mut harness = Harness::new_state(
                |ctx, app: &mut App| {
                    app.popped_panes(ctx);
                },
                app,
            );
            harness.run();
            harness.get_by_label("Put it back").click();
            harness.run();
            harness.run();

            assert!(harness.state().config.popped.is_empty(), "it would not come back");
            assert!(
                harness.query_by_label("Put it back").is_none(),
                "the window it went to is still being drawn"
            );
        }

        #[test]
        fn every_section_can_be_drawn_on_its_own() {
            // Each pane has to stand up outside the panel it was written for —
            // a side panel's contents in a central panel, a strip in a window.
            // Cheap to get wrong and invisible until somebody pops that one.
            for pane in Pane::ALL {
                let mut app = listing("alone", 3);
                app.selected = app.rows.first().map(|row| row.track.id);
                let mut harness =
                    Harness::new_ui_state(move |ui, app: &mut App| app.pane(ui, pane), app);
                harness.run();
                harness.run();
            }
        }

        #[test]
        fn the_way_into_the_duplicate_finder_is_there_before_anything_is_hashed() {
            // The complaint that started this: with nothing hashed there were
            // no groups, so the row was hidden, so the finder could not be
            // reached — and the one thing that would have fixed it was behind
            // the row.
            let mut app = app("wayin");
            app.library.add(std::path::Path::new("/music/one.flac"));
            app.rebuild();

            let mut harness = Harness::new_ui_state(|ui, app: &mut App| app.sidebar(ui), app);
            harness.run();
            harness.get_by_label("In here twice").click();
            harness.run();

            assert!(
                harness.state().duplicates.is_some(),
                "clicking the row did not open the duplicates sheet"
            );
        }

        #[test]
        fn escape_shuts_the_sheet_that_is_in_front() {
            // Sheets are capped to the screen so the close button is always
            // reachable, but a key that always works is the belt to that
            // brace — and it is what a person tries first.
            let mut app = app("escape");
            app.settings = true;
            app.duplicates = Some(Dupes::default());

            assert!(app.close_top_sheet(), "nothing was closed");
            assert!(app.duplicates.is_none(), "the front sheet stayed open");
            assert!(app.settings, "the sheet behind it closed too, on one press");

            assert!(app.close_top_sheet());
            assert!(!app.settings, "the second press did not reach the settings");
            assert!(!app.close_top_sheet(), "it claimed to close a sheet with none open");
        }

        #[test]
        fn a_collection_from_before_hashing_can_still_be_checked() {
            // The case that made the feature invisible: every track imported
            // before hashing existed has no hash, so nothing groups, so the
            // sidebar row never appears and there is no way in at all.
            let dir = std::env::temp_dir().join(format!("booth-rehash-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let one = dir.join("one.flac");
            let two = dir.join("two.flac");
            std::fs::write(&one, b"identical bytes").unwrap();
            std::fs::write(&two, b"identical bytes").unwrap();

            let mut app = app("rehash");
            for path in [&one, &two] {
                app.library.add(path);
            }
            assert!(
                app.library.duplicate_groups(&app.config.library_path).is_empty(),
                "unhashed tracks should group into nothing, which is the whole problem"
            );
            assert_eq!(app.library.unhashed().len(), 2, "both are waiting to be looked at");

            // What the button does, and then what the window does with what
            // comes back.
            app.hash_unchecked();
            let mut waited = 0;
            while app.running() && waited < 6000 {
                app.collect();
                std::thread::sleep(std::time::Duration::from_millis(5));
                waited += 5;
            }
            app.collect();

            assert!(app.library.unhashed().is_empty(), "the tracks were not hashed");
            let groups = app.library.duplicate_groups(&app.config.library_path);
            assert_eq!(groups.len(), 1, "the two copies did not become a group");
            assert_eq!(groups[0].rest.len(), 1);

            let _ = std::fs::remove_dir_all(&dir);
        }

        /// Two real files holding the same "audio", so a group forms and the
        /// trashing has something to delete.
        ///
        /// One inside the library folder and one outside it, because that is
        /// the first rule for which copy is kept — and the only one that does
        /// not move when a test gives a copy more metadata than the other.
        fn two_copies(name: &str) -> (App, std::path::PathBuf, u32, u32) {
            let dir = std::env::temp_dir().join(format!("booth-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            let inside = dir.join("library");
            let outside = dir.join("downloads");
            std::fs::create_dir_all(&inside).unwrap();
            std::fs::create_dir_all(&outside).unwrap();
            let keep_at = inside.join("keep.flac");
            let copy_at = outside.join("copy.flac");
            std::fs::write(&keep_at, b"same bytes").unwrap();
            std::fs::write(&copy_at, b"same bytes").unwrap();

            let mut app = app(name);
            app.config.library_path = inside;
            let keep = app.library.add(&keep_at);
            let other = app.library.add(&copy_at);
            for id in [keep, other] {
                let track = app.library.get_mut(id).unwrap();
                track.audio_hash = "SAME".into();
                track.file_hash = format!("FILE{id}");
                track.artist = "Peverelist".into();
                track.title = "Sirens".into();
            }
            // The one to keep knows more than the copy, which is what makes it
            // the one to keep — so a test can add something to the copy without
            // quietly turning it into the keeper.
            {
                let track = app.library.get_mut(keep).unwrap();
                track.album = "Livity Sound".into();
                track.year = Some(2019);
                track.analyzed = true;
            }
            assert_eq!(
                app.library.duplicate_groups(&app.config.library_path)[0].keep,
                keep,
                "the copy that knows the most is the one kept"
            );
            (app, dir, keep, other)
        }

        #[test]
        fn a_group_opens_offering_every_tag_any_copy_carries() {
            let (mut app, dir, keep, other) = two_copies("tag-union");
            app.library.get_mut(keep).unwrap().tags = vec!["peak".into()];
            app.library.get_mut(other).unwrap().tags = vec!["warmup".into(), "peak".into()];
            app.duplicates = Some(Dupes::default());

            let mut harness = Harness::new_ui_state(
                |ui, app: &mut App| {
                    let ctx = ui.ctx().clone();
                    app.duplicates_sheet(&ctx);
                },
                app,
            );
            harness.run();

            let state = harness.state().duplicates.as_ref().unwrap();
            let chosen = state.tags.get(&keep).expect("no tags were offered for the kept file");
            assert_eq!(
                chosen.iter().cloned().collect::<std::collections::BTreeSet<_>>(),
                ["peak".to_string(), "warmup".to_string()].into_iter().collect(),
                "a tag is somebody having said something, so all of it is kept to begin with"
            );

            // And each file still says which of them are its own, so a tag can
            // be traced to the copy it came from.
            assert!(harness.query_by_label("tagged  warmup  peak").is_some());
            assert!(harness.query_by_label("tagged  peak").is_some());

            let _ = std::fs::remove_dir_all(&dir);
        }

        #[test]
        fn a_second_file_kept_gets_its_own_list_and_starts_as_itself() {
            // The same audio on an EP and on a compilation is two records, so
            // keeping both is a real answer — and they do not have to be filed
            // the same way. Nothing is folded into the second one, so it starts
            // tagged as it already is rather than taking the others' tags.
            let (mut app, dir, keep, other) = two_copies("two-kept");
            let third = app.library.add(&dir.join("downloads").join("third.flac"));
            {
                let track = app.library.get_mut(third).unwrap();
                track.audio_hash = "SAME".into();
                track.file_hash = "FILE-THIRD".into();
                track.artist = "Peverelist".into();
            }
            app.library.get_mut(keep).unwrap().tags = vec!["peak".into()];
            app.library.get_mut(other).unwrap().tags = vec!["compilation".into()];
            app.library.get_mut(third).unwrap().tags = vec!["warmup".into()];
            app.duplicates = Some(Dupes::default());

            let mut harness = Harness::new_ui_state(
                |ui, app: &mut App| {
                    let ctx = ui.ctx().clone();
                    app.duplicates_sheet(&ctx);
                },
                app,
            );
            harness.run();
            // Only the first has a list to begin with.
            assert!(!harness.state().duplicates.as_ref().unwrap().tags.contains_key(&other));

            // Keeping the second one is what gives it one.
            harness.state_mut().duplicates.as_mut().unwrap().keeping.insert(other);
            harness.run();

            let state = harness.state().duplicates.as_ref().unwrap();
            assert_eq!(
                state.tags.get(&other).map(|set| set.iter().cloned().collect::<Vec<_>>()),
                Some(vec!["compilation".to_string()]),
                "the second kept file was filed as a record it is not"
            );
            assert_eq!(
                state.tags.get(&keep).map(|set| set.len()),
                Some(3),
                "the file everything is folded into should still start with the lot"
            );

            let _ = std::fs::remove_dir_all(&dir);
        }

        #[test]
        fn a_tag_struck_out_of_the_selection_does_not_end_up_on_the_kept_file() {
            // The one part of a merge that can take something away, so it is
            // worth knowing it really does: this includes a tag the kept file
            // already had.
            let (mut app, dir, keep, other) = two_copies("tag-pick");
            app.library.get_mut(keep).unwrap().tags = vec!["peak".into(), "vinyl rip".into()];
            app.library.get_mut(other).unwrap().tags = vec!["warmup".into()];

            app.trash_duplicates(
                &[(other, keep)],
                &[],
                &[(keep, vec!["peak".to_string(), "warmup".to_string()])],
            );

            assert_eq!(
                app.library.get(keep).unwrap().tags,
                vec!["peak".to_string(), "warmup".to_string()],
                "the selection was not the last word on the tags"
            );

            let _ = std::fs::remove_dir_all(&dir);
        }

        #[test]
        fn a_kept_copy_is_offered_the_name_the_one_it_replaced_gave_up() {
            // The tidy-up the whole rule is for: the file a copier numbered
            // turned out to be the one worth keeping, and now that the file it
            // was copied from has gone, the plain name is free.
            let dir = std::env::temp_dir().join(format!("booth-rename-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let plain = dir.join("track_04.flac");
            let numbered = dir.join("track_04 (1).flac");
            std::fs::write(&plain, b"same bytes").unwrap();
            std::fs::write(&numbered, b"same bytes").unwrap();

            let mut app = app("rename");
            let going = app.library.add(&plain);
            let keep = app.library.add(&numbered);
            for id in [going, keep] {
                let track = app.library.get_mut(id).unwrap();
                track.audio_hash = "SAME".into();
                track.file_hash = format!("F{id}");
                track.artist = "Peverelist".into();
            }
            app.library.get_mut(keep).unwrap().album = "Livity Sound".into();

            app.trash_duplicates(&[(going, keep)], &[], &[]);
            assert_eq!(
                app.renames,
                vec![(keep, dir.join("track_04.flac"))],
                "no rename was offered though the plain name is now free"
            );

            app.rename_files(&app.renames.clone());
            assert!(dir.join("track_04.flac").exists(), "the file was not renamed on disk");
            assert!(!numbered.exists(), "the old name is still there");
            assert_eq!(
                app.library.get(keep).unwrap().path,
                dir.join("track_04.flac"),
                "the collection is still pointing at the old name"
            );

            let _ = std::fs::remove_dir_all(&dir);
        }

        #[test]
        fn a_name_that_is_still_taken_is_not_offered_or_written_over() {
            // The offer's whole premise is that the name came free. If the file
            // it was copied from is still there — because it was kept too, or
            // because it would not go to the trash — there is nothing to offer.
            let dir =
                std::env::temp_dir().join(format!("booth-rename-taken-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let plain = dir.join("track_04.flac");
            let numbered = dir.join("track_04 (1).flac");
            std::fs::write(&plain, b"the original").unwrap();
            std::fs::write(&numbered, b"the copy").unwrap();

            let mut app = app("rename-taken");
            let keep = app.library.add(&numbered);
            app.library.get_mut(keep).unwrap().audio_hash = "SAME".into();

            // Nothing was deleted, so nothing is offered.
            app.trash_duplicates(&[], &[], &[]);
            assert!(app.renames.is_empty(), "a name still in use was offered");

            // And asked to do it anyway, it refuses rather than overwriting.
            app.rename_files(&[(keep, plain.clone())]);
            assert_eq!(
                std::fs::read(&plain).unwrap(),
                b"the original",
                "the rename wrote over a file that was already there"
            );
            assert!(numbered.exists(), "and it moved the copy anyway");

            let _ = std::fs::remove_dir_all(&dir);
        }

        #[test]
        fn what_only_the_copy_knows_survives_being_rid_of_it() {
            // The point of merging: throwing away a duplicate should cost
            // nothing at all, so the album name that was only on the download
            // has to be on the kept track before the file goes.
            let (mut app, dir, keep, other) = two_copies("merge-trash");
            {
                let track = app.library.get_mut(other).unwrap();
                track.tags = vec!["peak".into()];
                track.cues = vec![crate::library::CueMark {
                    letter: 1,
                    time_ms: 32_000,
                    label: "in".into(),
                    color: [1, 2, 3],
                }];
            }
            app.library.add_playlist("Saturday", "").unwrap();
            app.library.playlists[0].tracks.push(other);

            app.trash_duplicates(&[(other, keep)], &[], &[]);

            let kept = app.library.get(keep).expect("the kept track went");
            assert_eq!(kept.tags, vec!["peak".to_string()], "the tag went with the file");
            assert_eq!(kept.cues.len(), 1, "so did somebody's cue");
            assert_eq!(
                app.library.playlists[0].tracks,
                vec![keep],
                "the playlist was emptied instead of being pointed at the copy that stayed"
            );
            assert!(app.library.get(other).is_none(), "the copy is still listed");

            let _ = std::fs::remove_dir_all(&dir);
        }

        #[test]
        fn a_disagreement_is_settled_the_way_it_was_answered() {
            let (mut app, dir, keep, other) = two_copies("merge-pick");
            app.library.get_mut(other).unwrap().title = "Sirens (Original Mix)".into();

            app.trash_duplicates(
                &[(other, keep)],
                &[(other, crate::library::Field::Title, crate::library::Side::Other)],
                &[],
            );

            assert_eq!(
                app.library.get(keep).unwrap().title,
                "Sirens (Original Mix)",
                "the answer given in the sheet was not the one applied"
            );
            let _ = std::fs::remove_dir_all(&dir);
        }

        #[test]
        fn a_path_too_long_for_the_sheet_wraps_instead_of_running_off_it() {
            // The sheet is a list of paths and the decision is which of them to
            // delete, so a path running past the right-hand edge is the one
            // thing it cannot afford to hide — and there is no scrolling
            // sideways to go and find it.
            let (mut app, dir, _keep, other) = two_copies("wrapping");
            let long = dir.join("downloads").join(
                "Peverelist - Sirens (Original Mix) - Livity Sound Recordings 2019 \
                 Remastered Edition - 24bit 44.1kHz FLAC - 01 Sirens.flac",
            );
            app.library.get_mut(other).unwrap().path = long.clone();
            app.duplicates = Some(Dupes::default());

            let mut harness = Harness::new_ui_state(
                |ui, app: &mut App| {
                    let ctx = ui.ctx().clone();
                    app.duplicates_sheet(&ctx);
                },
                app,
            );
            harness.run();

            let shown = harness.get_by_label(long.display().to_string().as_str()).rect();
            let sheet = harness.ctx.content_rect();
            assert!(
                shown.right() <= sheet.right(),
                "the path runs {:.0} points past the edge of the screen, where nothing can \
                 reach it",
                shown.right() - sheet.right()
            );
            assert!(
                shown.height() > 14.0,
                "the path fitted on one line, so this proves nothing about wrapping"
            );

            let _ = std::fs::remove_dir_all(&dir);
        }

        /// A collection with one track, one playlist and a drive holding it —
        /// which is to say a drive with nothing to do.
        fn a_drive_up_to_date(name: &str) -> App {
            let mut app = app(name);
            let id = app.library.add(std::path::Path::new("/music/Sirens.flac"));
            app.library.add_playlist("Saturday", "").unwrap();
            app.library.playlists[0].tracks.push(id);
            app.library.drives.push(crate::library::Drive {
                label: "USB".into(),
                path: std::path::PathBuf::from("/media/usb"),
                playlists: vec!["Saturday".into()],
                written: vec![crate::library::Written {
                    id,
                    prep: sync::fingerprint(app.library.get(id).unwrap()),
                    ..Default::default()
                }],
                ..Default::default()
            });
            app.drive = 0;
            app.replan();
            assert!(app.plan.is_empty(), "the drive should have nothing to do");
            app
        }

        #[test]
        fn a_drive_with_nothing_to_do_can_still_be_opened() {
            // The way in has to exist when the collection believes the drive is
            // finished, because disagreeing with that belief is exactly what
            // the sheet is for — and it was the one state that closed it.
            let mut harness =
                Harness::new_ui_state(|ui, app: &mut App| app.dock(ui), a_drive_up_to_date("dock"));
            harness.run();

            // The same spacing the button is drawn with, rather than a guess
            // at what it looks like.
            harness.get_by_label(theme::label_text("Sync").as_str()).click();
            harness.run();

            assert!(harness.state().sheet, "the sync sheet did not open for an idle drive");
        }

        #[test]
        fn a_carried_stem_goes_into_the_playlist_beside_its_track() {
            let mut app = a_drive_up_to_date("stems in lists");
            let id = app.library.playlists[0].tracks[0];
            {
                let track = app.library.get_mut(id).unwrap();
                track.stems.vocals = Some("/stems/Sirens-vocals.wav".into());
                track.stems.drums = Some("/stems/Sirens-drums.wav".into());
                track.stems.melody = Some("/stems/Sirens-melody.wav".into());
            }

            let drive = app.library.drives[0].clone();
            let specs = app.drive_playlists(&drive);
            assert_eq!(specs.len(), 1, "{specs:?}");
            let paths = &specs[0].tracks;
            assert_eq!(paths.len(), 4, "the track and its three stems: {paths:?}");
            assert_eq!(paths[0], app.library.get(id).unwrap().path, "the track comes first");
            for part in ["vocals", "drums", "melody"] {
                assert!(
                    paths.iter().any(|p| p.to_string_lossy().contains(part)),
                    "no {part} in the list: {paths:?}"
                );
            }
        }

        #[test]
        fn a_stem_is_in_the_files_the_write_prepares() {
            // The other half of the same question: a stem named in a playlist
            // but never written is an entry the database drops on the floor.
            let mut app = a_drive_up_to_date("stems on disk");
            let id = app.library.playlists[0].tracks[0];
            {
                let track = app.library.get_mut(id).unwrap();
                track.stems.vocals = Some("/stems/Sirens-vocals.wav".into());
                track.stems.drums = Some("/stems/Sirens-drums.wav".into());
                track.stems.melody = Some("/stems/Sirens-melody.wav".into());
            }
            app.replan();

            let drive = app.library.drives[0].clone();
            let carry = sync::carry(&app.library, &drive, &app.plan);
            for part in ["vocals", "drums", "melody"] {
                assert!(
                    carry.files.iter().any(|p| p.to_string_lossy().contains(part)),
                    "the {part} stem is not among the files to write: {:?}",
                    carry.files
                );
            }
        }

        #[test]
        fn settings_opens_on_the_key_every_other_program_uses() {
            let mut harness = browsing("comma", 2);
            assert!(!harness.state().settings);
            press(&mut harness, egui::Key::Comma, egui::Modifiers::COMMAND);
            assert!(
                harness.state().settings,
                "\u{2318}, did not open Settings, which is where every other program keeps it"
            );
        }

        #[test]
        fn the_hover_delay_setting_reaches_the_window_it_describes() {
            // A delay that only took effect after a restart would be one
            // nobody could tune: the whole point is to drag it while looking
            // at the window.
            let mut app = app("hover");
            app.config.help_delay_ms = 1500;
            let mut harness = Harness::new_ui_state(
                |ui, app: &mut App| {
                    theme::help_delay(ui.ctx(), app.config.help_delay_ms);
                },
                app,
            );
            harness.run();
            assert_eq!(harness.ctx.style().interaction.tooltip_delay, 1.5);

            harness.state_mut().config.help_delay_ms = 0;
            harness.run();
            assert_eq!(harness.ctx.style().interaction.tooltip_delay, 0.0);
        }

        #[test]
        fn the_details_button_lists_every_change_one_to_a_line() {
            // The summary names three of each and says "+7". Which seven is
            // the question somebody opens the sheet with: a drive about to
            // remove four tracks is fine or a disaster depending on which four.
            let mut app = a_drive_up_to_date("details");
            let added = app.library.add(std::path::Path::new("/music/Second.flac"));
            app.library.get_mut(added).unwrap().artist = "Peverelist".into();
            app.library.get_mut(added).unwrap().title = "Roll With The Punches".into();
            app.library.playlists[0].tracks.push(added);
            app.replan();

            let lines = app.change_lines();
            assert_eq!(lines.len(), 1, "one line for the one track being added: {lines:?}");
            assert!(lines[0].0.starts_with('+'), "{lines:?}");
            assert!(lines[0].0.contains("Roll With The Punches"), "{lines:?}");

            // And the button on the sheet opens the window that shows them.
            app.sheet = true;
            let mut harness = Harness::new_ui_state(
                |ui, app: &mut App| {
                    let ctx = ui.ctx().clone();
                    app.sync_sheet(&ctx);
                },
                app,
            );
            harness.run();
            harness.get_by_label("Details").click();
            harness.run();
            assert!(harness.state().details, "the Details button did not open the window");
        }

        #[test]
        fn a_track_the_collection_has_lost_still_gets_a_line() {
            // The case worth seeing above all: the drive holds a track that is
            // no longer in the collection, so the write takes it off. A row
            // that quietly vanished from the list would be the write doing
            // something the list did not mention.
            let mut app = a_drive_up_to_date("gone");
            let id = app.library.playlists[0].tracks[0];
            app.library.remove(id);
            // `remove` clears the drive's record too, so put back the one row
            // that makes this a removal rather than nothing at all.
            app.library.drives[0].written =
                vec![crate::library::Written { id, ..Default::default() }];
            app.replan();

            let lines = app.change_lines();
            assert_eq!(lines.len(), 1, "{lines:?}");
            assert!(lines[0].0.contains("no longer in the collection"), "{lines:?}");
        }

        #[test]
        fn a_rendered_kit_goes_on_without_anyone_ticking_a_box() {
            // Why nobody's stems reached a stick: carrying them was a choice
            // that defaulted to no, made on a sheet most people never opened.
            // A kit exists because somebody asked for one, so the drive it goes
            // with is the answer unless they say otherwise.
            let mut app = a_drive_up_to_date("kits by default");
            let id = app.library.playlists[0].tracks[0];
            {
                let track = app.library.get_mut(id).unwrap();
                track.stems.vocals = Some("/stems/Sirens-vocals.wav".into());
                track.stems.drums = Some("/stems/Sirens-drums.wav".into());
                track.stems.melody = Some("/stems/Sirens-melody.wav".into());
            }
            app.replan();

            assert_eq!(app.plan.stems.len(), 3, "a rendered kit was left off a fresh drive");
            assert!(app.plan.delta().contains("3 stems"), "{}", app.plan.delta());
        }

        #[test]
        fn a_kit_on_disk_the_collection_never_heard_about_is_noticed() {
            // A kit rendered in a run that ended before the collection was
            // saved, or dropped into the folder by hand, is three files the
            // drive would otherwise never be told about. The write is the last
            // chance to look, and the files are named after their track, so
            // looking is cheap and the answer is not a guess.
            let dir = std::env::temp_dir().join(format!("booth-found-kit-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let track = dir.join("Sirens.flac");
            std::fs::write(&track, b"").unwrap();
            for part in ["vocals", "drums", "melody"] {
                std::fs::write(dir.join(format!("Sirens-{part}.wav")), b"").unwrap();
            }

            let mut app = app("found kit");
            let id = app.library.add(&track);
            assert!(app.library.get(id).unwrap().stems.is_empty(), "the fixture starts with none");

            assert_eq!(app.notice_rendered_kits(&[id]), 1);
            let kit = &app.library.get(id).unwrap().stems;
            assert!(kit.is_complete(), "the kit beside the track was not picked up: {kit:?}");
            // Asking twice must not report a second find, or every write would
            // save the collection for nothing.
            assert_eq!(app.notice_rendered_kits(&[id]), 0);

            let _ = std::fs::remove_dir_all(&dir);
        }

        #[test]
        fn another_drive_can_be_reached_when_one_is_already_set_up() {
            // The hole this closes: adding was offered only while the list was
            // empty and switching only while it held more than one, so setting
            // up a first drive took away every way to add or reach a second.
            let mut app = a_drive_up_to_date("pick");
            // A second drive the dock is not currently on, so finding it can
            // only mean the menu listed it rather than the dock naming the one
            // already in use.
            app.library.drives.push(crate::library::Drive {
                label: "SPARE".into(),
                path: std::path::PathBuf::from("/media/spare"),
                ..Default::default()
            });
            let mut harness = Harness::new_ui_state(|ui, app: &mut App| app.dock(ui), app);
            harness.run();

            harness.get_by_label_contains("drives").click();
            harness.run();
            harness.get_by_label_contains("Add a drive");
            harness.get_by_label_contains("Add an image");
            harness.get_by_label_contains("SPARE").click();
            harness.run();

            // The dock only asks; the window is what acts, so the ask has to
            // be carried out here the way a frame would carry it out.
            let ctx = egui::Context::default();
            harness.state_mut().apply_pending(&ctx);
            assert_eq!(harness.state().drive, 1, "picking a drive did not switch to it");
        }

        #[test]
        fn a_drive_that_is_not_there_says_so_rather_than_looking_ready() {
            // `/media/usb` is not mounted while a test runs, which is the whole
            // point: the collection remembers a place and a history, and a
            // place is not a stick. Told nothing, you line up a sync against a
            // drive that went home in somebody's bag.
            let mut harness =
                Harness::new_ui_state(|ui, app: &mut App| app.dock(ui), a_drive_up_to_date("gone"));
            harness.run();
            harness.get_by_label_contains("not plugged in");
        }

        #[test]
        fn what_counts_as_plugged_in_is_different_for_an_image() {
            // One is a mounted folder and the other is a file, so asking the
            // same question of both would call every image missing.
            let dir = std::env::temp_dir().join(format!("booth-plugged-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let image = dir.join("drive.img");
            std::fs::write(&image, b"not really an image").unwrap();

            let stick = crate::library::Drive { path: dir.clone(), ..Default::default() };
            let missing = crate::library::Drive { path: dir.join("nowhere"), ..Default::default() };
            let made =
                crate::library::Drive { path: image.clone(), is_image: true, ..Default::default() };
            // A folder is not an image, and an image is not a folder.
            let unmade =
                crate::library::Drive { path: dir.clone(), is_image: true, ..Default::default() };

            assert!(plugged_in(&stick));
            assert!(!plugged_in(&missing));
            assert!(plugged_in(&made));
            assert!(!plugged_in(&unmade));

            std::fs::remove_dir_all(&dir).unwrap();
        }

        #[test]
        fn forgetting_a_drive_leaves_the_rest_reachable() {
            // An index kept across a removal points at a different drive, which
            // is the worst kind of wrong: everything still works and it is the
            // wrong stick.
            let mut app = a_drive_up_to_date("forget");
            app.library.drives.push(crate::library::Drive {
                label: "SPARE".into(),
                path: std::path::PathBuf::from("/media/spare"),
                ..Default::default()
            });
            app.drive = 1;

            let ctx = egui::Context::default();
            app.pending.push(Pending::ForgetDrive(1));
            app.apply_pending(&ctx);

            assert_eq!(app.library.drives.len(), 1);
            assert_eq!(app.library.drives[0].label, "USB");
            assert!(
                app.library.drives.get(app.drive).is_some(),
                "the index was left past the end of the list"
            );
        }

        #[test]
        fn forgetting_what_a_drive_holds_makes_the_next_write_a_first_write() {
            // The way back from a drive something else has been at. The record
            // is what makes a write a small one; without it every track is
            // prepared again and the database is made from scratch.
            let mut app = app("resync");
            let id = app.library.add(std::path::Path::new("/music/Sirens.flac"));
            app.library.add_playlist("Saturday", "").unwrap();
            app.library.playlists[0].tracks.push(id);
            app.library.drives.push(crate::library::Drive {
                label: "USB".into(),
                path: std::path::PathBuf::from("/media/usb"),
                playlists: vec!["Saturday".into()],
                written: vec![crate::library::Written {
                    id,
                    prep: sync::fingerprint(app.library.get(id).unwrap()),
                    row: Some(booth_cli::export::pdb::Track {
                        id: 1,
                        file_path: "/Contents/Peverelist/Sirens.flac".into(),
                        ..Default::default()
                    }),
                    ..Default::default()
                }],
                ..Default::default()
            });
            app.drive = 0;
            app.replan();
            assert!(app.plan.is_empty(), "the drive is up to date to begin with");

            app.forget_drive_contents();

            assert!(
                app.library.drives[0].written.is_empty(),
                "the record of what is on the drive was kept"
            );
            assert_eq!(app.plan.add, vec![id], "the next write should put the track on again");

            // And a second press has nothing to forget, so it says nothing.
            let said = app.status.clone();
            app.forget_drive_contents();
            assert_eq!(app.status, said, "forgetting nothing announced something");
        }

        #[test]
        fn taking_the_files_word_brings_the_collection_up_to_date() {
            // The whole point of checking: what it found has to be something
            // that can then be put right, and the record afterwards has to
            // agree with the file it describes.
            let dir = std::env::temp_dir().join(format!("booth-check-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let path = dir.join("track.flac");
            std::fs::write(&path, b"a longer file than the collection remembers").unwrap();

            let mut app = app("check");
            let id = app.library.add(&path);
            {
                let track = app.library.get_mut(id).unwrap();
                track.bytes = 12;
                track.stems.vocals = Some(dir.join("not-there.flac"));
            }

            let report = crate::verify::check(app.library.get(id).unwrap(), false);
            assert!(
                report.troubles.contains(&crate::verify::Trouble::Resized { was: 12, now: 43 }),
                "{:?}",
                report.troubles
            );
            assert!(
                report.troubles.contains(&crate::verify::Trouble::StemGone { part: "vocals" }),
                "{:?}",
                report.troubles
            );

            app.checked = Some(Checked { troubles: vec![report], ..Default::default() });
            app.take_the_files_word(&[id]);

            let track = app.library.get(id).unwrap();
            assert_eq!(track.bytes, 43, "the size was not brought up to date");
            assert!(track.stems.vocals.is_none(), "the kit still lists a stem that is not there");
            assert!(app.checked.is_none(), "the answer is stale once it has been acted on");

            let _ = std::fs::remove_dir_all(&dir);
        }

        #[test]
        fn a_track_whose_audio_changed_stops_counting_as_analysed() {
            // The grid and the cues were measured against bytes that are not
            // there any more. Keeping them would be keeping an answer to a
            // question nobody asked.
            let dir = std::env::temp_dir().join(format!("booth-restale-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let path = dir.join("track.flac");
            std::fs::write(&path, b"different audio entirely").unwrap();

            let mut app = app("restale");
            let id = app.library.add(&path);
            {
                let track = app.library.get_mut(id).unwrap();
                track.analyzed = true;
                track.bpm = 128.0;
                track.audio_hash = "THE-OLD-SOUND".into();
                track.bytes = 1;
            }

            let report = crate::verify::check(app.library.get(id).unwrap(), true);
            app.checked = Some(Checked { troubles: vec![report], ..Default::default() });
            app.take_the_files_word(&[id]);

            let track = app.library.get(id).unwrap();
            assert!(!track.analyzed, "the stale listening was kept");
            assert_ne!(track.audio_hash, "THE-OLD-SOUND", "the hash was not brought up to date");

            let _ = std::fs::remove_dir_all(&dir);
        }

        #[test]
        fn a_retag_by_another_program_is_not_a_reason_to_listen_again() {
            // The sound did not change, only what is written beside it, so the
            // analysis still describes the file exactly.
            let dir = std::env::temp_dir().join(format!("booth-retagged-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let path = dir.join("track.flac");
            std::fs::write(&path, b"the audio").unwrap();

            let mut app = app("retagged");
            let id = app.library.add(&path);
            let audio = booth_cli::hash::audio_sha256(&path).unwrap();
            {
                let track = app.library.get_mut(id).unwrap();
                track.analyzed = true;
                track.audio_hash = audio;
                track.file_hash = "SOMETHING-ELSE".into();
                track.bytes = std::fs::metadata(&path).unwrap().len();
            }

            let report = crate::verify::check(app.library.get(id).unwrap(), true);
            assert_eq!(report.troubles, vec![crate::verify::Trouble::Rewritten]);
            app.checked = Some(Checked { troubles: vec![report], ..Default::default() });
            app.take_the_files_word(&[id]);

            assert!(
                app.library.get(id).unwrap().analyzed,
                "hours of listening were thrown away over a tag write"
            );

            let _ = std::fs::remove_dir_all(&dir);
        }

        #[test]
        fn a_group_opens_keeping_one_file_and_only_one() {
            // The point of the sheet is to end up with one file per recording,
            // so that is what it opens proposing — and the copy it proposes is
            // a tick like any other, because the guess about which one is best
            // may not be the user's answer.
            let (mut app, dir, keep, clean) = two_copies("ticks");
            let arguing = app.library.add(&dir.join("downloads").join("third.flac"));
            {
                let track = app.library.get_mut(arguing).unwrap();
                track.audio_hash = "SAME".into();
                track.file_hash = "FILE-THIRD".into();
                track.artist = "Peverelist".into();
                track.title = "Sirens (Original Mix)".into();
            }
            app.library.get_mut(clean).unwrap().tags = vec!["peak".into()];
            app.duplicates = Some(Dupes::default());

            let mut harness = Harness::new_ui_state(
                |ui, app: &mut App| {
                    let ctx = ui.ctx().clone();
                    app.duplicates_sheet(&ctx);
                },
                app,
            );
            harness.run();

            let state = harness.state().duplicates.as_ref().expect("the sheet shut itself");
            assert_eq!(
                state.keeping.iter().copied().collect::<Vec<_>>(),
                vec![keep],
                "a group should open keeping exactly one file, and it should be the best one"
            );

            let _ = std::fs::remove_dir_all(&dir);
        }

        #[test]
        fn a_copy_that_disagrees_is_left_alone_until_it_is_answered() {
            // Deleting it would settle the question by throwing one of the two
            // answers away, which is the one thing the sheet promises not to do.
            let (mut app, dir, keep, clean) = two_copies("waiting");
            let arguing = app.library.add(&dir.join("downloads").join("third.flac"));
            {
                let track = app.library.get_mut(arguing).unwrap();
                track.audio_hash = "SAME".into();
                track.file_hash = "FILE-THIRD".into();
                track.artist = "Peverelist".into();
                track.title = "Sirens (Original Mix)".into();
            }
            app.duplicates = Some(Dupes::default());

            let mut harness = Harness::new_ui_state(
                |ui, app: &mut App| {
                    let ctx = ui.ctx().clone();
                    app.duplicates_sheet(&ctx);
                },
                app,
            );
            harness.run();
            harness.get_by_label("Sirens (Original Mix)").click();
            harness.run();

            let state = harness.state().duplicates.as_ref().unwrap();
            assert_eq!(
                state.picked.get(&(arguing, crate::library::Field::Title)),
                Some(&crate::library::Side::Other),
                "clicking the answer did not record it"
            );
            assert!(state.keeping.contains(&keep), "the kept copy stopped being kept");
            assert!(!state.keeping.contains(&clean), "the clean copy was not slated to go");

            let _ = std::fs::remove_dir_all(&dir);
        }

        #[test]
        fn moving_the_tick_within_a_group_is_not_undone_on_the_next_frame() {
            let (mut app, dir, keep, other) = two_copies("sticky");
            app.duplicates = Some(Dupes::default());
            let mut harness = Harness::new_ui_state(
                |ui, app: &mut App| {
                    let ctx = ui.ctx().clone();
                    app.duplicates_sheet(&ctx);
                },
                app,
            );
            harness.run();
            assert!(harness.state().duplicates.as_ref().unwrap().keeping.contains(&keep));

            // What clicking the other copy's box, and then the first one's,
            // amounts to: the group is now keeping the copy instead.
            {
                let state = harness.state_mut().duplicates.as_mut().unwrap();
                state.keeping.insert(other);
                state.keeping.remove(&keep);
            }
            harness.run();
            harness.run();

            let state = harness.state().duplicates.as_ref().unwrap();
            assert!(state.keeping.contains(&other), "the opening tick was put back over the user");
            assert!(!state.keeping.contains(&keep));
            let _ = std::fs::remove_dir_all(&dir);
        }

        #[test]
        fn trashing_a_duplicate_removes_the_file_and_the_track() {
            // Against real files, because this is the one thing in the program
            // that touches somebody's music and the only way to know it took
            // the right one is to look on disk afterwards.
            let dir = std::env::temp_dir().join(format!(
                "booth-trash-{}-{}",
                "dupes",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();

            let keep = dir.join("keep.flac");
            let copy = dir.join("copy.flac");
            std::fs::write(&keep, b"the same bytes").unwrap();
            std::fs::write(&copy, b"the same bytes").unwrap();

            let mut app = app("trash");
            let kept = app.library.add(&keep);
            let doomed = app.library.add(&copy);
            for id in [kept, doomed] {
                let track = app.library.get_mut(id).unwrap();
                track.file_hash = "SAME".into();
                track.audio_hash = "SAME".into();
            }

            app.trash_duplicates(&[(doomed, kept)], &[], &[]);

            assert!(keep.exists(), "the wrong file went");
            assert!(!copy.exists(), "the duplicate is still on disk");
            assert!(app.library.get(kept).is_some(), "the kept track left the collection");
            assert!(app.library.get(doomed).is_none(), "the trashed track is still listed");

            let _ = std::fs::remove_dir_all(&dir);
        }

        #[test]
        fn a_file_that_will_not_go_stays_in_the_collection() {
            // Forgetting a track whose file is still there would leave the
            // file behind with nothing pointing at it — a worse state than the
            // duplicate it was.
            let mut app = app("stubborn");
            let keep = app.library.add(std::path::Path::new("/nowhere/at/all/kept.flac"));
            let id = app.library.add(std::path::Path::new("/nowhere/at/all/missing.flac"));

            app.trash_duplicates(&[(id, keep)], &[], &[]);
            assert!(
                app.library.get(id).is_some(),
                "the track was forgotten though its file could not be trashed"
            );
        }

        #[test]
        fn every_track_dropped_on_a_playlist_shows_up_in_it() {
            // Reported: three dragged in, the sidebar counted three, and the
            // list showed two.
            let mut app = app("three");
            let ids: Vec<u32> = (1..=3)
                .map(|i| {
                    let id = app.library.add(std::path::Path::new(&format!("/music/{i}.flac")));
                    let track = app.library.get_mut(id).unwrap();
                    track.artist = format!("Artist {i}");
                    track.title = format!("Track {i}");
                    id
                })
                .collect();
            app.library.add_playlist("peak", "").unwrap();
            app.rebuild();

            // One at a time, as three drags are.
            for id in &ids {
                app.add_tracks_to_playlist(&[*id], "peak");
            }
            assert_eq!(app.library.playlists[0].tracks, ids, "the collection lost one");

            // Now look at the playlist, as a person would.
            app.view = View::Playlist;
            app.playlist = "peak".into();
            app.rebuild();

            let showing: Vec<u32> =
                app.rows.iter().filter(|row| !row.indented).map(|row| row.track.id).collect();
            assert_eq!(showing, ids, "the list showed fewer than the count did");
        }

        #[test]
        fn a_drop_shows_up_while_the_playlist_is_the_thing_being_looked_at() {
            // The reported shape of it: dropping onto the playlist you are
            // already looking at. The sidebar counts from the collection and
            // is right immediately; the list is a built thing and was not
            // being rebuilt, so it lagged the count by however many drops had
            // happened since something else happened to rebuild it.
            let mut app = app("watching");
            let ids: Vec<u32> = (1..=3)
                .map(|i| app.library.add(std::path::Path::new(&format!("/music/{i}.flac"))))
                .collect();
            app.library.add_playlist("peak", "").unwrap();
            app.view = View::Playlist;
            app.playlist = "peak".into();
            app.rebuild();
            assert!(app.rows.is_empty(), "nothing in it yet");

            let ctx = egui::Context::default();
            for id in &ids {
                app.pending.push(Pending::AddToPlaylist(vec![*id], "peak".into()));
                app.apply_pending(&ctx);
            }

            let showing: Vec<u32> =
                app.rows.iter().filter(|row| !row.indented).map(|row| row.track.id).collect();
            assert_eq!(showing.len(), 3, "the list did not follow the drops: {showing:?}");
            assert_eq!(showing, ids);
        }

        #[test]
        fn adding_the_same_track_twice_does_not_put_it_in_twice() {
            // A playlist is an order to play things in, so the same record
            // appearing twice is a mistake rather than an instruction.
            let mut app = with_a_track_and_a_playlist("twice");
            let id = app.library.tracks[0].id;

            app.add_tracks_to_playlist(&[id], "Saturday peak");
            app.add_tracks_to_playlist(&[id], "Saturday peak");
            assert_eq!(app.library.playlists[0].tracks, vec![id]);
        }

        #[test]
        fn adding_to_a_name_that_is_new_makes_the_playlist() {
            let mut app = with_a_track_and_a_playlist("newname");
            let id = app.library.tracks[0].id;

            app.add_tracks_to_playlist(&[id], "Sunday warmup");
            let made = app
                .library
                .playlists
                .iter()
                .find(|p| p.name == "Sunday warmup")
                .expect("no playlist was made");
            assert_eq!(made.tracks, vec![id]);
        }

        #[test]
        fn a_row_that_is_not_a_track_is_not_added() {
            // Companion rows have ids of their own so they can be selected,
            // but they are not in the collection. Putting one in a playlist
            // would write an acapella onto a drive without the record it came
            // from.
            let mut app = with_a_track_and_a_playlist("companion");
            let id = app.library.tracks[0].id;
            let companion = crate::library::companion_id(id, crate::library::Role::Vocals);

            app.add_tracks_to_playlist(&[companion], "Saturday peak");
            assert!(app.library.playlists[0].tracks.is_empty(), "a companion went in");
        }

        #[test]
        fn escape_abandons_the_name_rather_than_making_it() {
            name_something("escape", "+", "half a thought", egui::Key::Escape, |library| {
                assert!(library.playlists.is_empty(), "escape made one anyway");
            });
        }
    }

    #[test]
    fn a_panel_opens_at_the_size_it_was_left_at() {
        // The round trip has to be exact. The size that comes back is the
        // panel's outside and the one handed to the contents is its inside, so
        // a version of this that saved the wrong one of the two would shrink
        // every panel by its margins on each run — slowly, and only for people
        // who had used it for a while.
        for asked in [150.0f32, 178.0, 240.0, 300.0] {
            let got = std::cell::Cell::new(0.0);
            // One context across the frames, because the whole question is
            // what the panel does on the frames after the first — that is
            // where egui's own note of the size takes over from the default.
            let ctx = egui::Context::default();
            for _ in 0..3 {
                let _ = ctx.run(Default::default(), |ctx| {
                    let panel = egui::SidePanel::left("t")
                        .default_width(asked)
                        .width_range(150.0..=300.0)
                        .frame(pane_frame())
                        .resizable(true)
                        .show(ctx, |ui| {
                            pinned(ui, |ui| {
                                ui.label("something narrower than the panel");
                            })
                        });
                    got.set(panel.response.rect.width());
                });
            }
            assert_eq!(got.get(), asked, "asked for {asked}, got {}", got.get());
        }
    }

    #[test]
    fn the_dock_keeps_its_height_whatever_the_log_says() {
        // The same trap the side panels were in, the other way up: egui takes
        // the panel's height from what its contents came out as, so a dock
        // holding one line would shrink to it and one holding a long run would
        // climb until it had the window.
        let kept = |lines: usize| {
            let kept = std::cell::Cell::new(0.0);
            egui::__run_test_ui(|ui| {
                let rect =
                    egui::Rect::from_min_size(ui.max_rect().min, egui::vec2(900.0, DOCK_HEIGHT));
                let mut dock = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(rect)
                        .layout(egui::Layout::top_down(egui::Align::Min)),
                );
                pinned(&mut dock, |ui| {
                    // As the dock draws it: a scrolling list, which is what
                    // keeps a long run inside the height rather than pushing
                    // it open.
                    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                        for i in 0..lines {
                            ui.label(format!("line {i}"));
                        }
                    });
                });
                kept.set(dock.min_rect().height());
            });
            kept.get()
        };

        // The number itself is the harness's business; that it does not move
        // with the contents is the dock's.
        let empty = kept(0);
        assert_eq!(kept(1), empty, "one line changed the dock's height");
        assert_eq!(kept(200), empty, "a long run pushed the dock open");
        assert!(empty >= DOCK_HEIGHT, "the dock collapsed below its own row: {empty}");
    }

    /// The width a side panel would keep, given contents of a chosen width.
    ///
    /// egui reports back what the contents occupied, which is what it stores as
    /// the panel's width for the next frame — so this is the width the panel
    /// would come back as.
    fn kept_width(available: f32, draw: impl Fn(&mut Ui)) -> f32 {
        // A cell because the harness takes a `Fn`, and the width has to come
        // back out of it.
        let kept = std::cell::Cell::new(0.0);
        egui::__run_test_ui(|ui| {
            // The same shape a side panel makes: a child ui given the panel's
            // rectangle, whose own rectangle afterwards is what egui stores as
            // the width for the next frame.
            let rect = egui::Rect::from_min_size(ui.max_rect().min, egui::vec2(available, 600.0));
            let mut panel = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(rect)
                    .layout(egui::Layout::top_down(egui::Align::Min)),
            );
            pinned(&mut panel, |ui| draw(ui));
            kept.set(panel.min_rect().width());
        });
        kept.get()
    }

    #[test]
    fn a_panel_keeps_its_width_whatever_is_in_it() {
        // The bug this is here for: egui takes a side panel's width from the
        // rectangle its contents came out as, so the panel followed its own
        // contents instead of the drag. Empty, the inspector collapsed to its
        // narrowest column; with a long title in it, it grew until it was
        // eating the browser.
        let empty = kept_width(210.0, |_| {});
        assert_eq!(empty, 210.0, "an empty panel collapsed to nothing");

        let short = kept_width(210.0, |ui| {
            ui.label("Tension");
        });
        assert_eq!(short, 210.0, "a narrow label pulled the panel in");

        let long = kept_width(210.0, |ui| {
            ui.label(
                "All Your Sisters — Tension (Modern Failures) \u{2014} \
                 /home/user/Music/All Your Sisters/Modern Failures/02 Tension.m4a",
            );
        });
        assert_eq!(long, 210.0, "a long label pushed the panel out");

        // The case a maximum width does not cover, and the one the panels were
        // actually losing their size to. A label wraps at the maximum; a row
        // of things that will not fit simply runs past it, and the rectangle
        // egui measures the panel by runs past it too. Every panel here has
        // one — the sidebar's name-and-count rows, the inspector's button
        // strips — so before this held, a panel dragged narrow went back to
        // its widest as soon as somebody named a playlist something long.
        let row = kept_width(210.0, |ui| {
            ui.horizontal(|ui| {
                ui.label("a playlist name far longer than the panel is wide");
                ui.label("999");
            });
        });
        assert_eq!(row, 210.0, "a row too wide to fit pushed the panel out");
    }

    #[test]
    fn a_panel_is_as_wide_as_it_was_given() {
        // Different widths, because the point is that the panel takes the one
        // it was handed rather than settling on a width of its own.
        for width in [150.0, 210.0, 300.0, 420.0] {
            let kept = kept_width(width, |ui| {
                ui.label("Peverelist \u{2014} Roll With The Punches (Extended Club Mix)");
            });
            assert_eq!(kept, width, "asked for {width}");
        }
    }

    #[test]
    fn a_bitrate_is_worked_out_from_the_file_rather_than_a_header() {
        // A three-minute file of 7.2 MB is about 320 kbps.
        assert_eq!(bitrate(7_200_000, 180.0), 320);
        assert_eq!(bitrate(0, 0.0), 0, "an unmeasured track has no bitrate to report");
    }

    #[test]
    fn the_beat_grid_the_picture_draws_starts_from_the_first_cue() {
        let mut track = Track::placeholder(1);
        track.has_grid = true;
        track.bpm = 120.0;
        track.duration_secs = 10.0;
        track.cues.push(CueMark {
            letter: 0,
            time_ms: 2_000,
            label: String::new(),
            color: [0, 0, 0],
        });

        let beats = beat_times(&track);
        // 500 ms apart, and phased so that a beat lands on the first cue.
        assert_eq!(beats[0], 0);
        assert_eq!(beats[1], 500);
        assert!(beats.contains(&2_000), "the cue should fall on a beat");
        assert!(*beats.last().unwrap() < 10_000);
    }

    #[test]
    fn the_first_beat_of_the_grid_is_a_downbeat_on_the_memory_cues_bar() {
        // Everything that counts in fours counts from beat zero — the red bar
        // marks, the bar number in the transport, the length on a phrase. Wind
        // the grid back a beat at a time and beat zero is whichever beat lands
        // nearest the top of the track, so all three count from an offbeat on
        // any track that does not happen to start on the one.
        let mut track = Track::placeholder(1);
        track.has_grid = true;
        track.bpm = 126.0;
        track.duration_secs = 180.0;
        track.cues.push(CueMark {
            letter: 0,
            time_ms: 30_969,
            label: String::new(),
            color: [0, 0, 0],
        });

        let beats = beat_times(&track);
        let at = beats
            .iter()
            .position(|beat| beat.abs_diff(30_969) < 2)
            .expect("the memory cue should fall on a beat");
        assert_eq!(at % crate::library::BEATS_PER_BAR, 0, "the memory cue landed on beat {at}");
        // And it is wound back as far as a downbeat goes, so at most the part
        // bar at the head of the track is left unmarked.
        let bar_ms = (crate::library::BEATS_PER_BAR as f64 * 60_000.0 / 126.0) as u32;
        assert!(beats[0] < bar_ms, "the grid started {} ms in, past the first bar", beats[0]);
    }

    #[test]
    fn a_grid_that_bends_is_drawn_as_it_was_kept() {
        // No tempo puts these beats back, which is the whole reason they are
        // kept. Rebuilding one from the tempo would straighten a grid somebody
        // bent on a player, and the straightened version is what would then go
        // back on the drive.
        let mut track = Track::placeholder(1);
        track.has_grid = true;
        track.bpm = 120.0;
        track.duration_secs = 10.0;
        track.beat_ms = vec![0, 500, 1_010, 1_480, 2_030, 2_500];

        assert_eq!(beat_times(&track), track.beat_ms);
        // And the transport counts against those beats, not against the tempo:
        // 1_600 ms is past the fourth beat although a steady 120 would have
        // called it the third.
        assert_eq!(beat_at(&track, 1_600), Some(3));
        assert_eq!(beat_at(&track, 0), Some(0));
    }

    #[test]
    fn the_anchor_says_where_the_one_is_rather_than_the_memory_cue() {
        // They were the same thing, so setting the grid moved a cue and moving
        // a cue moved the grid. A DJ does both, and rarely means the other.
        let mut track = Track::placeholder(1);
        track.has_grid = true;
        track.bpm = 120.0;
        track.duration_secs = 20.0;
        track.cues.push(CueMark {
            letter: 0,
            time_ms: 3_000,
            label: String::new(),
            color: [0, 0, 0],
        });

        let off_the_cue = beat_times(&track);
        track.downbeat_ms = Some(3_250);
        let off_the_anchor = beat_times(&track);
        assert_ne!(off_the_cue[0], off_the_anchor[0], "the anchor was ignored");
        assert!(
            off_the_anchor.contains(&3_250),
            "the anchor should fall on a beat: {:?}",
            &off_the_anchor[..4]
        );
    }

    #[test]
    fn a_track_with_no_grid_draws_no_beats() {
        let mut track = Track::placeholder(1);
        track.duration_secs = 300.0;
        assert!(beat_times(&track).is_empty());

        track.has_grid = true;
        track.bpm = 0.0;
        assert!(beat_times(&track).is_empty(), "a zero tempo would divide by zero");
    }

    #[test]
    fn a_cue_is_snapped_to_the_nearest_beat() {
        // 500 ms apart, at 120 BPM.
        let beats: Vec<u32> = (0..16).map(|i| i * 500).collect();
        assert_eq!(snap_to(&beats, 1_480, 1), 1_500);
        assert_eq!(snap_to(&beats, 1_520, 1), 1_500);
        // Exactly between two beats: either is a beat, which is what matters.
        assert!([1_000, 1_500].contains(&snap_to(&beats, 1_250, 1)));
    }

    #[test]
    fn a_memory_cue_is_snapped_to_the_bar_rather_than_the_beat() {
        let beats: Vec<u32> = (0..16).map(|i| i * 500).collect();
        // Starting a track mid-bar is a different mistake from starting it a
        // few milliseconds early, so the memory cue gets the coarser grid.
        assert_eq!(snap_to(&beats, 2_600, 4), 2_000);
        assert_eq!(snap_to(&beats, 2_600, 1), 2_500);
    }

    #[test]
    fn a_track_with_no_grid_can_still_be_marked_up() {
        // Refusing to place a cue because the tracker found no beats would be
        // the wrong way round: the cue is what the user is sure of.
        assert_eq!(snap_to(&[], 1_234, 1), 1_234);
    }

    #[test]
    fn a_moment_in_a_track_lands_on_the_beat_it_falls_within() {
        let mut track = Track::placeholder(1);
        track.has_grid = true;
        track.bpm = 120.0;
        track.duration_secs = 60.0;
        track.beats = 120;

        // 500 ms a beat, and a moment belongs to the beat it is inside rather
        // than the nearest one — the third beat starts at 1000 ms and is still
        // the third beat at 1499.
        assert_eq!(beat_at(&track, 0), Some(0));
        assert_eq!(beat_at(&track, 499), Some(0));
        assert_eq!(beat_at(&track, 500), Some(1));
        assert_eq!(beat_at(&track, 1_499), Some(2));

        // Past the end it stops at the last beat rather than counting on.
        assert_eq!(beat_at(&track, 999_999), Some(119));
    }

    #[test]
    fn a_track_with_no_grid_has_no_beat_to_be_at() {
        // No bars to count in, so the transport shows the clock instead of
        // inventing a position from a tempo of zero.
        let mut track = Track::placeholder(1);
        assert_eq!(beat_at(&track, 1_000), None);
        track.has_grid = true;
        assert_eq!(beat_at(&track, 1_000), None, "a zero tempo would divide by zero");
    }

    #[test]
    fn a_grid_that_starts_late_counts_from_its_first_beat() {
        let mut track = Track::placeholder(1);
        track.has_grid = true;
        track.bpm = 120.0;
        track.duration_secs = 60.0;
        track.beats = 120;
        track.cues.push(CueMark {
            letter: 0,
            time_ms: 2_000,
            label: String::new(),
            color: [0, 0, 0],
        });

        // Anything before the first beat is the first beat, not a negative one.
        let first = beat_times(&track).first().copied().unwrap();
        assert_eq!(beat_at(&track, first.saturating_sub(1)), Some(0));
        assert_eq!(beat_at(&track, first), Some(0));
        assert_eq!(beat_at(&track, first + 500), Some(1));
    }

    #[test]
    fn a_position_reads_as_minutes_seconds_and_hundredths() {
        assert_eq!(time_text(0), "0:00.00");
        assert_eq!(time_text(63_450), "1:03.45");
        assert_eq!(time_text(3_599_990), "59:59.99");
    }

    #[test]
    fn a_grid_that_is_offset_from_zero_keeps_its_phase() {
        let mut track = Track::placeholder(1);
        track.has_grid = true;
        track.bpm = 128.0;
        track.duration_secs = 30.0;
        track.cues.push(CueMark {
            letter: 0,
            time_ms: 1_234,
            label: String::new(),
            color: [0, 0, 0],
        });

        let period = 60_000.0 / 128.0;
        let beats = beat_times(&track);
        for beat in &beats {
            let phase = (*beat as f64 - 1_234.0).rem_euclid(period);
            assert!(
                phase < 1.0 || period - phase < 1.0,
                "beat at {beat} is off the grid by {phase} ms"
            );
        }
    }
}
