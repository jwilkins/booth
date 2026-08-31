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

use eframe::egui::{self, Color32, RichText, Ui};
use musicai::cli::ExportArgs;
use musicai::stems::Backend;

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

/// How much room under the list the prep editor needs.
///
/// Added up from its parts rather than guessed, because the failure is silent:
/// a budget a few points short does not overflow, it quietly clips the last row
/// off the bottom of the window, and the measurements line is the row it takes.
/// The room the duplicates sheet keeps for its footer, so the button that does
/// the deleting is never scrolled away from.
const FOOTER_HEIGHT: f32 = 46.0;

const PREP_HEIGHT: f32 = wave::HEIGHT
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
    /// Set when a panel has been dragged and the new size is not written out
    /// yet. See [`App::save_panels`].
    panels_moved: bool,
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
    /// Rows whose waveform has already been asked for once this run, so a
    /// measurement that cannot succeed is not attempted on every frame.
    remeasured: std::collections::HashSet<u32>,
    /// The same for stem envelopes. A second set rather than a marker bit on
    /// the first: companion ids use the top bit too, so a bit that meant
    /// "stems" would sometimes also mean "acapella of track 3".
    re_enveloped: std::collections::HashSet<u32>,
    /// Where the playhead sits in the selected track, in milliseconds. It is
    /// where a new cue goes, so it is a position rather than a playing thing —
    /// nothing here makes a sound.
    playhead_ms: Option<u32>,
    /// The cue being named, and the text as typed.
    cue_entry: (Option<(u32, u8)>, String),
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
    /// Put these tracks in the playlist of that name, making it if it is new.
    AddToPlaylist(Vec<u32>, String),
    /// Fold these copies into the tracks being kept, then send their files to
    /// the trash and forget them. The picks say how each disagreement between a
    /// copy and its keeper was settled.
    TrashDuplicates {
        ticked: Vec<u32>,
        picked: Vec<(u32, crate::library::Field, crate::library::Side)>,
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
    /// Move a cue to a new time, or add one if it is not there yet.
    PlaceCue {
        id: u32,
        letter: u8,
        time_ms: u32,
    },
    TogglePlayback(u32),
    /// Change how the waveform is coloured.
    PaintAs(wave::Paint),
    /// Take or refuse one fingerprint match.
    AnswerMatch {
        id: u32,
        take: bool,
    },
    /// Move the deck to a position in a track, if that track is on it.
    SeekDeck {
        id: u32,
        time_ms: u32,
    },
    RemoveCue {
        id: u32,
        letter: u8,
    },
    RenameCue {
        id: u32,
        letter: u8,
        label: String,
    },
    /// Listen to one track, whether or not it has been listened to before.
    Analyze(u32),
    /// Ask the fingerprint services about one track.
    Identify(u32),
    /// Render one track's stem kit.
    Separate(u32),
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
        theme::install(&cc.egui_ctx);

        let config_path = Config::path();
        let config = Config::load(&config_path);
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
            panels_moved: false,
            pending_save: false,
            pending_rebuild: false,
            tag_entry: String::new(),
            started: None,
            incompatible: Vec::new(),
            to_retag: Vec::new(),
            remeasured: std::collections::HashSet::new(),
            re_enveloped: std::collections::HashSet::new(),
            playhead_ms: None,
            cue_entry: (None, String::new()),
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
        if std::env::var_os("BOOTH_OPEN_DUPES").is_some() {
            app.duplicates = Some(Dupes::default());
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
        let level = match color {
            theme::ALERT => crate::log::Level::Error,
            theme::AMBER => crate::log::Level::Warn,
            _ => crate::log::Level::Info,
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
    fn remember_panel(&mut self, which: fn(&mut crate::config::Panels) -> &mut f32, size: f32) {
        let stored = which(&mut self.config.panels);
        if !crate::config::Panels::differs(*stored, size) {
            return;
        }
        *stored = size;
        self.panels_moved = true;
    }

    /// Write the panel sizes out, if a drag has just finished moving one.
    fn save_panels(&mut self, ctx: &egui::Context) {
        if !self.panels_moved || ctx.input(|i| i.pointer.any_down()) {
            return;
        }
        self.panels_moved = false;
        match self.config.save(&self.config_path) {
            Ok(()) => crate::debug!("panel sizes saved: {:?}", self.config.panels),
            Err(e) => crate::warn!("could not save the panel sizes: {e:#}"),
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
        let next = (at as isize + delta).clamp(0, self.rows.len() as isize - 1) as usize;
        let id = self.rows[next].track.id;

        if Some(id) != self.selected {
            self.selected = Some(id);
            self.waveform = None;
            self.zoom = wave::Zoom::default();
            self.envelopes = None;
        }
        match extend {
            true => self.mark_range_to(id),
            false => self.mark_only(id),
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
        let anchor = self.anchor.unwrap_or(id);
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
    fn analyze_tracks(&mut self, ids: &[u32]) {
        let waiting = self.files_for(ids);
        if waiting.is_empty() {
            self.note("nothing to analyse", theme::DIM);
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
        let key = match musicai::rekordbox::resolve(self.config.rekordbox_key()) {
            Ok(key) => key,
            Err(e) => {
                crate::warn!("{e:#}");
                self.note(format!("{e:#}"), theme::AMBER);
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
    fn merge_rekordbox(&mut self, collection: &musicai::rekordbox::master::Collection) -> String {
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
        collection: &musicai::rekordbox::master::Collection,
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
            self.note("nothing to convert", theme::DIM);
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
            self.note("nothing showing needs analysing", theme::DIM);
            return;
        }
        self.analyze_tracks(&waiting);
    }

    /// Render stem kits for these tracks.
    fn separate_tracks(&mut self, ids: &[u32]) {
        let waiting = self.files_for(ids);
        if waiting.is_empty() {
            self.note("nothing to separate", theme::DIM);
            return;
        }
        let ids: Vec<u32> = waiting.iter().map(|(id, _)| *id).collect();
        for id in &ids {
            self.re_enveloped.remove(id);
        }
        self.ensure_local(&ids);
        self.start(Job::Separate {
            tracks: waiting,
            stems_in: self.config.stems_location(),
            backend: Backend::Demucs,
            quality: self.config.stem_quality.to_cli(),
        });
    }

    fn render_stems(&mut self) {
        let waiting = self.acting_on(|track| track.stems.is_empty());
        if waiting.is_empty() {
            self.note("everything showing already has a stem kit", theme::DIM);
            return;
        }
        self.separate_tracks(&waiting);
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
                    if let Some(found) = best {
                        self.consider(id, found);
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
                    self.note(brought, theme::GO);
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
                    if let Err(e) = crate::library::cache_waveform(analyzed.id, &analyzed.bands) {
                        self.note(format!("could not cache the waveform: {e}"), theme::DIM);
                    }
                    if Some(analyzed.id) == self.selected {
                        self.waveform = Some((analyzed.id, analyzed.bands.clone()));
                    }
                    changed = true;
                }
                Update::Separated { id, kit } => {
                    if let Some(track) = self.library.get_mut(id) {
                        track.stems = kit;
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
                Update::Line(text) => self.note(text, theme::TEXT),
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
                        theme::ALERT,
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
                            self.note(message, theme::ALERT);
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
            self.save();
        }
        if !imported.is_empty() {
            self.ensure_local(&imported);
            self.check_compatibility(&imported);
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
            Err(e) => self.note(format!("could not save the collection: {e:#}"), theme::ALERT),
        }
    }

    // -- the sync ----------------------------------------------------------

    /// The drive's playlists, as the exporter wants them: paths in play order.
    ///
    /// A stem companion follows its parent rather than going to the end, so
    /// the browse list on the player reads track, vocals, drums, melody and a
    /// companion is a turn of the encoder from the record it came from.
    /// Companions go in only when the drive carries them, and only the parts
    /// that were actually rendered.
    fn drive_playlists(&self, drive: &Drive) -> Vec<musicai::cli::PlaylistSpec> {
        drive
            .playlist_names()
            .iter()
            .filter_map(|name| self.library.playlists.iter().find(|p| p.name == *name))
            .map(|playlist| musicai::cli::PlaylistSpec {
                name: playlist.name.clone(),
                folder: playlist.folder.clone(),
                tracks: playlist
                    .tracks
                    .iter()
                    .filter_map(|id| self.library.get(*id))
                    .flat_map(|track| {
                        let mut paths = vec![track.path.clone()];
                        if drive.with_stems {
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

    fn write_drive(&mut self) {
        let Some(drive) = self.library.drives.get(self.drive).cloned() else { return };
        let files: Vec<PathBuf> = self
            .plan
            .writes()
            .iter()
            .filter_map(|id| self.library.get(*id))
            .map(|track| track.path.clone())
            .chain(self.plan.stems.iter().cloned())
            .collect();

        // Which stem came from which track, so each one takes its parent's
        // grid, cues, key and phrases rather than being listened to alone.
        let companions: Vec<(PathBuf, PathBuf)> = self
            .plan
            .writes()
            .iter()
            .filter_map(|id| self.library.get(*id))
            .flat_map(|track| {
                track
                    .stems
                    .each()
                    .into_iter()
                    .filter_map(|(_, stem)| stem.cloned())
                    .map(|stem| (stem, track.path.clone()))
                    .collect::<Vec<_>>()
            })
            .collect();
        if files.is_empty() {
            self.note("nothing to write", theme::DIM);
            return;
        }

        let mut args = ExportArgs::defaults();
        if drive.is_image {
            args.image = Some(drive.path.clone());
            args.label = drive.label.clone();
        } else {
            args.drive = Some(drive.path.clone());
        }
        args.playlists = self.drive_playlists(&drive);
        args.companions = companions;

        // The drive's record is updated before the write rather than after,
        // because the fingerprints being recorded are the ones being written.
        // A failure is reported in the log, and the next plan will find the
        // difference again from the drive itself.
        let written: Vec<Written> = self
            .plan
            .writes()
            .iter()
            .filter_map(|id| self.library.get(*id))
            .map(|track| Written { id: track.id, prep: sync::fingerprint(track) })
            .collect();
        if let Some(drive) = self.library.drives.get_mut(self.drive) {
            drive.written = written;
            drive.last_sync = Some(now());
        }

        self.sheet = false;
        self.start(Job::Sync { args: Box::new(args), files });
        self.replan();
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
            with_stems: false,
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
/// Pinning the content to what we were handed makes the stored size the panel's
/// own, so it only ever changes when someone drags it. The cross axis is left
/// alone — egui already makes a panel fill it.
fn pinned_width<R>(ui: &mut Ui, contents: impl FnOnce(&mut Ui) -> R) -> R {
    let width = ui.available_width();
    ui.set_min_width(width);
    ui.set_max_width(width);
    contents(ui)
}

/// [`pinned_width`], for a panel that resizes up and down instead.
///
/// This holds the floor on its own: a panel with little in it keeps its height
/// rather than collapsing to its contents. It cannot hold the ceiling by
/// itself, because a column of labels grows past a maximum where a line of text
/// would wrap inside one — so contents that can run long belong in a
/// `ScrollArea`, which is what stops them pushing the panel open.
fn pinned_height<R>(ui: &mut Ui, contents: impl FnOnce(&mut Ui) -> R) -> R {
    let height = ui.available_height();
    ui.set_min_height(height);
    ui.set_max_height(height);
    contents(ui)
}

/// What colour a log line is drawn in, by how much it matters.
fn log_color(level: crate::log::Level) -> egui::Color32 {
    match level {
        crate::log::Level::Error => theme::ALERT,
        crate::log::Level::Warn => theme::AMBER,
        _ => theme::DIM,
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
        self.collect();
        self.collect_picked();
        if let Some(what) = self.want_pick.take() {
            self.pick(what, ctx);
        }
        self.take_dropped(ctx);
        self.keys(ctx);
        self.follow_playback(ctx);

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
        let dock = egui::TopBottomPanel::bottom("dock")
            .frame(bar_frame())
            .resizable(true)
            .default_height(sizes.dock.max(DOCK_HEIGHT))
            .height_range(DOCK_HEIGHT..=460.0)
            .show(ctx, |ui| pinned_height(ui, |ui| self.dock(ui)));
        self.remember_panel(|panels| &mut panels.dock, dock.response.rect.height());

        // Both side panels drag. The inspector especially: it carries the
        // cue list, the stem rows and the notes field, and how much room those
        // want is a matter of the track and the person. egui remembers the
        // width against the panel id, so a drag survives a restart.
        //
        // The minimum is what the widest fixed thing in each still fits in,
        // not zero: a panel that can be dragged shut leaves no handle to drag
        // it back with.
        let collection = egui::SidePanel::left("collection")
            .default_width(sizes.collection)
            .width_range(150.0..=300.0)
            .frame(pane_frame())
            .resizable(true)
            .show(ctx, |ui| pinned_width(ui, |ui| self.sidebar(ui)));
        self.remember_panel(|panels| &mut panels.collection, collection.response.rect.width());

        let inspector = egui::SidePanel::right("inspector")
            .default_width(sizes.inspector)
            .width_range(180.0..=420.0)
            .frame(pane_frame())
            .resizable(true)
            .show(ctx, |ui| pinned_width(ui, |ui| self.inspector(ui)));
        self.remember_panel(|panels| &mut panels.inspector, inspector.response.rect.width());

        egui::CentralPanel::default().frame(pane_frame()).show(ctx, |ui| self.browser(ui));

        if self.sheet {
            self.sync_sheet(ctx);
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
        self.questions_sheet(ctx);
        if self.help {
            self.help_sheet(ctx);
        }
        if self.duplicates.is_some() {
            self.duplicates_sheet(ctx);
        }
        self.log_window(ctx);

        // Everything the panels asked for happens here, after they have all
        // drawn, so no panel ever reads a collection halfway through a change.
        self.save_panels(ctx);
        self.apply_pending(ctx);
        self.flush_retags();
    }

    fn on_exit(&mut self, _: Option<&eframe::glow::Context>) {
        self.save();
    }
}

fn bar_frame() -> egui::Frame {
    egui::Frame::NONE
        .fill(theme::BOOTH_2)
        .inner_margin(egui::Margin::symmetric(12, 8))
        .stroke(egui::Stroke::new(1.0, theme::RULE))
}

/// The chrome every sheet shares.
/// What the duplicates sheet is holding while it is open.
///
/// Kept across frames rather than worked out afresh each one: the ticks and the
/// answers to disagreements are decisions somebody made, and the groups behind
/// them are rebuilt every frame as tracks are hashed.
#[derive(Default)]
struct Dupes {
    /// The copies to be rid of.
    ticked: std::collections::HashSet<u32>,
    /// How a disagreement between one copy and the track being kept was
    /// settled, by copy and by field.
    picked: std::collections::HashMap<(u32, crate::library::Field), crate::library::Side>,
    /// Copies that have already been given their opening answer, so that
    /// unticking one is not undone on the next frame — and so that a copy that
    /// turns up later, as hashing goes on, still gets one.
    seen: std::collections::HashSet<u32>,
}

/// The most a sheet may be, so that it always fits on the screen.
///
/// A window taller than the viewport is still centred on it, which puts the
/// title bar — and with it the close button, the only way out — off the top of
/// the screen, and cuts the bottom off too. Capping the height and scrolling
/// the body inside it is what keeps a long sheet closable on a small display.
fn sheet_height(ctx: &egui::Context) -> f32 {
    (ctx.content_rect().height() - 72.0).max(240.0)
}

fn sheet_frame() -> egui::Frame {
    egui::Frame::NONE
        .fill(theme::BOOTH)
        .stroke(egui::Stroke::new(1.0, theme::RULE))
        .inner_margin(egui::Margin::same(14))
}

fn pane_frame() -> egui::Frame {
    egui::Frame::NONE.fill(theme::BOOTH).inner_margin(egui::Margin::same(12))
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
        if !self.asking.is_empty() {
            self.asking.clear();
            return true;
        }
        if std::mem::take(&mut self.settings) {
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
            if typing {
                return;
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
                .desired_width(ui.available_width() - 260.0)
                .hint_text(
                    RichText::new("bpm:124-128 key:~8A -played:30d tag:peak")
                        .monospace()
                        .color(theme::DIM),
                );
            let response = ui.add(field);
            if self.focus_bar {
                response.request_focus();
                self.focus_bar = false;
            }
            if response.changed() {
                self.rebuild();
            }

            ui.label(RichText::new("⌘K").font(theme::mono(10.5)).color(theme::DIM));
            if ui
                .add(
                    egui::Button::new(RichText::new("?").font(theme::mono(10.5)).color(theme::DIM))
                        .fill(theme::BOOTH),
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
                                RichText::new("stop").font(theme::mono(10.5)).color(theme::DIM),
                            ))
                            .clicked()
                        {
                            runner.cancel();
                        }
                        ui.label(RichText::new(text).font(theme::mono(11.5)).color(theme::AMBER));
                    }
                    None => {
                        let color = if self.query.has_errors() { theme::ALERT } else { theme::DIM };
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
                        Paint::Field => theme::AMBER,
                        Paint::Value => theme::TEXT,
                        Paint::Negated => theme::ALERT,
                        Paint::Text => theme::DIM,
                        Paint::Bad => theme::ALERT,
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

    fn sidebar(&mut self, ui: &mut Ui) {
        pane_label(ui, "Collection");
        let all = self.library.tracks.len();
        let unprepared = self.library.unprepared_count();
        let attention = self.library.attention_count();

        self.view_row(ui, View::All, "All tracks", all, theme::DIM);
        self.view_row(ui, View::Unprepared, "Unprepared", unprepared, theme::DIM);
        self.view_row(ui, View::Attention, "Needs attention", attention, theme::ALERT);

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
                        egui::Label::new(RichText::new("In here twice").color(theme::AMBER))
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
                        (n, 0) => (n.to_string(), theme::DIM),
                        (0, _) => ("?".to_string(), theme::AMBER),
                        (n, _) => (format!("{n}?"), theme::AMBER),
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
            let color = if on { theme::AMBER } else { theme::TEXT };
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
            && ui.button(RichText::new("save this one").size(theme::SMALL)).clicked()
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
            ui.label(RichText::new("none yet").color(theme::DIM).size(theme::SMALL));
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
                            RichText::new(format!("\u{25be} {folder}")).color(theme::TEXT),
                        )
                        .sense(egui::Sense::click()),
                    );
                    response.context_menu(|ui| {
                        if ui.button("Rename\u{2026}").clicked() {
                            self.naming = Some(Naming::rename_folder(&folder));
                            ui.close();
                        }
                        // The playlists come back to the top level rather than
                        // going with it, so this loses the filing and not the
                        // work — which is why it needs no confirmation.
                        if ui.button("Delete folder").clicked() {
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
                let color = if on { theme::AMBER } else { theme::DIM };
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
                            theme::AMBER.gamma_multiply(0.22),
                        );
                    }
                    if let Some(dragged) = response.dnd_release_payload::<rows::Dragged>() {
                        self.pending.push(Pending::AddToPlaylist(dragged.0.clone(), name.clone()));
                    }

                    if response.clicked() {
                        self.view = View::Playlist;
                        self.playlist = name.clone();
                        self.rebuild();
                    }
                    response.context_menu(|ui| {
                        ui.label(RichText::new(&name).color(theme::DIM).size(theme::SMALL));
                        ui.separator();
                        if ui.button("Rename\u{2026}").clicked() {
                            self.naming = Some(Naming::rename_playlist(&name));
                            ui.close();
                        }
                        ui.menu_button("Move to", |ui| {
                            if !folder.is_empty() && ui.button("Top level").clicked() {
                                self.move_playlist(&name, "");
                                ui.close();
                            }
                            for other in folders.iter().filter(|f| **f != folder) {
                                if ui.button(other).clicked() {
                                    self.move_playlist(&name, other);
                                    ui.close();
                                }
                            }
                        });
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
                                .color(theme::DIM),
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
        let response = ui.add(
            egui::TextEdit::singleline(&mut naming.text)
                .desired_width(f32::INFINITY)
                .hint_text(RichText::new(naming.what.hint()).color(theme::DIM)),
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
                Err(message) => self.note(message, theme::ALERT),
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
            let color = if on { theme::AMBER } else { theme::TEXT };
            if ui
                .add(egui::Label::new(RichText::new(name).color(color)).sense(egui::Sense::click()))
                .clicked()
            {
                self.view = view;
                self.rebuild();
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let color = if count > 0 { count_color } else { theme::DIM };
                ui.label(
                    RichText::new(count.to_string()).font(theme::mono(theme::SMALL)).color(color),
                );
            });
        });
    }

    /// The list, and the prep editor underneath it.
    fn browser(&mut self, ui: &mut Ui) {
        // The column widths are worked out here, once, and handed to both the
        // header and the rows, so the two cannot drift apart — and the header
        // is drawn outside the scroll area, so that scrolling a long list never
        // takes away the names of the columns or the way to sort by them.
        //
        // The width the rows will get is what is left after the scroll bar, and
        // that is what both are measured against: when there is no bar the
        // widths are unchanged and only a sliver of space goes unused at the
        // right, which is nothing next to a header that lines up either way.
        let widths = rows::columns(ui.available_width() - ui.spacing().scroll.allocated_width());
        self.header(ui, &widths);

        let list_height = (ui.available_height() - PREP_HEIGHT).max(120.0);
        egui::ScrollArea::vertical()
            .max_height(list_height)
            .auto_shrink([false, false])
            .show(ui, |ui| self.rows_table(ui, &widths));

        ui.add_space(6.0);
        self.actions(ui);
        ui.add_space(4.0);
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
            if ui
                .add_enabled(
                    idle && unanalysed > 0,
                    egui::Button::new(format!("Analyse {unanalysed}")),
                )
                .on_hover_text(match chosen {
                    true => "Grid, key, phrases and cues for the selected tracks that have none",
                    false => "Grid, key, phrases and cues for everything showing that has none",
                })
                .clicked()
            {
                self.analyze_unprepared();
            }
            let unnamed = self.acting_on(|track| !track.identified).len();
            if ui
                .add_enabled(idle && unnamed > 0, egui::Button::new(format!("Identify {unnamed}")))
                .on_hover_text(
                    "Fingerprint and look up what these are, filling in the names they lack",
                )
                .clicked()
            {
                self.identify_showing();
            }
            if ui
                .add_enabled(idle && unstemmed > 0, egui::Button::new(format!("Stems {unstemmed}")))
                .on_hover_text("Render a vocals/melody/drums kit with demucs")
                .clicked()
            {
                self.render_stems();
            }

            ui.separator();

            // Adding to a playlist is what turns a query into a set that can go
            // on a drive, so it sits with the prep actions rather than in a menu.
            ui.label(
                RichText::new(match chosen {
                    true => "selected to playlist",
                    false => "to playlist",
                })
                .color(if chosen { theme::AMBER } else { theme::DIM })
                .size(theme::SMALL),
            );
            let width = 110.0;
            ui.add(
                egui::TextEdit::singleline(&mut self.playlist_entry)
                    .desired_width(width)
                    .hint_text(RichText::new("name").color(theme::DIM)),
            );
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
        ids: &[u32],
        picked: &[(u32, crate::library::Field, crate::library::Side)],
    ) {
        let mut gone = 0usize;
        let mut freed = 0u64;
        // Which group each copy belongs to, so the merge knows what it is being
        // folded into. Worked out before anything is deleted, because removing
        // a track changes the groups.
        let keepers: std::collections::HashMap<u32, u32> = self
            .library
            .duplicate_groups(&self.config.library_path)
            .iter()
            .flat_map(|group| group.rest.iter().map(|copy| (copy.id, group.keep)))
            .collect();

        for id in ids {
            let Some(track) = self.library.get(*id) else { continue };
            let (path, bytes) = (track.path.clone(), track.bytes);
            // Folded in first, and only then deleted: doing it the other way
            // round would be reading a record that is already gone. A file that
            // then will not go leaves the copy in the collection with its own
            // answers still on it, which is untidy but loses nothing.
            if let Some(keep) = keepers.get(id) {
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
                        theme::ALERT,
                    );
                }
            }
        }
        if gone > 0 {
            self.note(
                format!("{} to the trash, {} freed", plural(gone, "file"), sync::bytes(freed)),
                theme::TEXT,
            );
        }
        // The copy kept is the one that knows the most about the record, which
        // is not always the one in the library folder — so de-duplicating can
        // leave the collection pointing at somebody's download folder. That is
        // the same situation as importing from outside, and gets the same
        // answer: whatever the setting says, which is to take a copy unless
        // told otherwise.
        let survivors: Vec<u32> = keepers.values().copied().collect();
        self.ensure_local(&survivors);
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
            self.note("nothing to add", theme::DIM);
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
        self.note(format!("{added} added to \u{201c}{name}\u{201d}"), theme::TEXT);
        self.save();
        self.replan();
        // The list is a built thing and has to follow. Without this a drop
        // onto the playlist being looked at changed the sidebar's count — read
        // straight from the collection — and not the rows under it, so the two
        // disagreed until something else happened to rebuild them.
        self.pending_rebuild = true;
    }

    /// The column names, pinned above the list.
    fn header(&mut self, ui: &mut Ui, widths: &rows::Widths) {
        let Some(column) = rows::header_row(ui, widths, self.sort) else { return };
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
            if selects && Some(id) != self.selected {
                self.selected = Some(id);
                self.waveform = None;
                self.zoom = wave::Zoom::default();
                self.envelopes = None;
                // A different track: start it from the top rather than from
                // wherever the last one's playhead happened to be.
                self.playhead_ms = None;
            }
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
                rows::Hit::Opened => self.pending.push(Pending::TogglePlayback(id)),
                rows::Hit::Chose(action) => self.pending.push(match action {
                    rows::Action::Play => Pending::TogglePlayback(id),
                    rows::Action::Analyze => Pending::Analyze(id),
                    rows::Action::Identify => Pending::Identify(id),
                    rows::Action::Separate => Pending::Separate(id),
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
                ui.label(RichText::new(message).color(theme::DIM));
            });
        }
    }

    /// The prep editor: waveform, phrase strip, and the line of measurements.
    fn prep(&mut self, ui: &mut Ui) {
        let track = self.selected_track().cloned();
        let Some(track) = track else { return };

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
        };
        // Drawn before anything below touches the collection: `waveform`
        // borrows the cached picture out of the window's own state, and that
        // borrow has to be finished with before the panel changes anything.
        let shown = wave::show(ui, &waveform);
        let moved = wave::phrase_strip(ui, &track.phrases, track.duration_secs, shown.zoom);
        let zoom = moved.unwrap_or(shown.zoom);

        match shown.touched {
            Some(wave::Touched::Scrubbed(ms)) => {
                self.playhead_ms = Some(ms);
                self.pending.push(Pending::SeekDeck { id: track.id, time_ms: ms });
            }
            Some(wave::Touched::Moved { letter, time_ms }) => {
                self.pending.push(Pending::PlaceCue { id: track.id, letter, time_ms })
            }
            None => {}
        }
        self.zoom = zoom;

        self.cue_strip(ui, &track);

        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 16.0;

            measurement(ui, "grid", &grid_text(&track, self.config.length), track.has_grid);
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

        if !has_bands && track.analyzed {
            match crate::library::cached_waveform(track.id) {
                Some(cached) => self.waveform = Some((track.id, cached)),
                // Nothing cached — analysed by an older version, or the cache
                // was cleared. Measure it again, once, in the background.
                //
                // Once, and remembered: this runs every frame the track is
                // selected, so a track that cannot be measured — its file has
                // moved, it will not decode — would otherwise start a job,
                // fail, and start another one for as long as it stayed
                // selected.
                None if !self.running()
                    && !self.remeasured.contains(&track.id)
                    && track.path.exists() =>
                {
                    self.remeasured.insert(track.id);
                    self.start(Job::Analyze(vec![(track.id, track.path.clone())]));
                }
                None => {}
            }
        }
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
            self.note(format!("no {} rendered for that track", track.role.label()), theme::ALERT);
            return;
        }
        if let Some(missing) = sources.iter().find(|path| !path.exists()) {
            crate::warn!("cannot play #{id}: {} is gone", missing.display());
            self.note("that file is not where it was", theme::ALERT);
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
                ui.label(RichText::new("no audio out").color(theme::DIM).size(theme::SMALL))
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
                egui::Button::new(RichText::new(label).font(theme::mono(12.0)).color(theme::BOOTH))
                    .fill(if playing { theme::GO } else { theme::AMBER })
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
        ui.label(
            RichText::new(match beat_at(track, at_ms) {
                Some(beat) => length.elapsed_and_left(beat, track.beats),
                // No grid, so no bars to count in — the clock is all there is.
                None => format!(
                    "{} / {}",
                    time_text(at_ms),
                    time_text((track.duration_secs * 1000.0) as u32)
                ),
            })
            .font(theme::mono(10.5))
            .color(if loaded { theme::TEXT } else { theme::DIM }),
        )
        .on_hover_text(format!(
            "{} of {} — {} / {}",
            length.position(beat_at(track, at_ms).unwrap_or(0)),
            length.describe(track.beats),
            time_text(at_ms),
            time_text((track.duration_secs * 1000.0) as u32)
        ));

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
                    .color(theme::DIM)
                    .strong(),
            );

            let at = self.playhead_ms;
            for letter in 0..=8u8 {
                let cue = track.cues.iter().find(|cue| cue.letter == letter);
                let name = match letter {
                    0 => "MEM".to_string(),
                    n => char::from(b'A' + n - 1).to_string(),
                };
                let color = match cue {
                    Some(cue) => Color32::from_rgb(cue.color[0], cue.color[1], cue.color[2]),
                    None => theme::RULE,
                };
                let response = ui
                    .add(
                        egui::Button::new(RichText::new(&name).font(theme::mono(10.5)).color(
                            match cue {
                                Some(_) => theme::BOOTH,
                                None => theme::DIM,
                            },
                        ))
                        .fill(match cue {
                            Some(_) => color,
                            None => theme::BOOTH,
                        })
                        .min_size(egui::vec2(30.0, 18.0)),
                    )
                    .on_hover_text(match cue {
                        Some(cue) => format!(
                            "{} — {}{}",
                            time_text(cue.time_ms),
                            if cue.label.is_empty() { "no name" } else { &cue.label },
                            "\nclick to jump, shift-click to clear, or drag it on the waveform"
                        ),
                        None => "click to put a cue at the playhead".to_string(),
                    });

                let shift = ui.input(|i| i.modifiers.shift);
                if response.clicked() {
                    match (cue, shift) {
                        (Some(_), true) => {
                            self.pending.push(Pending::RemoveCue { id: track.id, letter })
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
                        RichText::new(time_text(ms)).font(theme::mono(10.5)).color(theme::TEXT),
                    );
                }
                None => {
                    ui.label(
                        RichText::new("click the waveform to place the playhead")
                            .size(theme::SMALL)
                            .color(theme::DIM),
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
                        (true, _) => theme::BOOTH,
                        (false, true) => theme::DIM,
                        (false, false) => theme::RULE,
                    };
                    if ui
                        .add(
                            egui::Button::new(
                                RichText::new(mode.label()).font(theme::mono(10.0)).color(color),
                            )
                            .fill(if on {
                                theme::AMBER
                            } else {
                                theme::BOOTH
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
                            RichText::new("fit").font(theme::mono(10.0)).color(theme::DIM),
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
            if let Some(cue) = self.playhead_cue(track) {
                let letter = cue.letter;
                if self.cue_entry.0 != Some((track.id, letter)) {
                    self.cue_entry = (Some((track.id, letter)), cue.label.clone());
                }
                let response = ui.add(
                    egui::TextEdit::singleline(&mut self.cue_entry.1)
                        .desired_width(140.0)
                        .font(theme::mono(10.5))
                        .hint_text(RichText::new("name this cue").monospace().color(theme::DIM)),
                );
                if response.changed() || response.lost_focus() {
                    self.pending.push(Pending::RenameCue {
                        id: track.id,
                        letter,
                        label: self.cue_entry.1.clone(),
                    });
                }
            }
        });
    }

    /// The cue the playhead is sitting on, within half a beat.
    fn playhead_cue<'a>(&self, track: &'a Track) -> Option<&'a crate::library::CueMark> {
        let at = self.playhead_ms?;
        let tolerance = match track.bpm > 0.0 {
            true => (30_000.0 / track.bpm) as u32,
            false => 250,
        };
        track.cues.iter().find(|cue| cue.time_ms.abs_diff(at) <= tolerance)
    }

    fn inspector(&mut self, ui: &mut Ui) {
        pane_label(ui, "Now inspecting");
        let Some(track) = self.selected_track().cloned() else {
            ui.label(RichText::new("nothing selected").color(theme::DIM));
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
                let caption = ui.label(RichText::new(label).color(theme::DIM).size(theme::SMALL));
                let field = ui.add(
                    egui::TextEdit::singleline(value)
                        .desired_width(width)
                        .font(theme::sans(theme::BODY)),
                );
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
                if ui.add_enabled(changed, egui::Button::new("Save")).clicked() {
                    self.pending.push(Pending::CommitEdit);
                }
                if ui.add_enabled(changed, egui::Button::new("Revert")).clicked() {
                    self.pending.push(Pending::CancelEdit);
                }
                if changed {
                    ui.label(RichText::new("unsaved").color(theme::AMBER).size(theme::SMALL));
                }
            });
            // Writing to the file is a separate act from editing the record,
            // and it is spelled out rather than implied, because it changes
            // somebody's files.
            let taggable =
                musicai::tag::Metadata::default().get(musicai::tag::Field::Title).is_none()
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
                    .color(theme::DIM)
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
                .color(theme::DIM),
            );
            ui.label(
                RichText::new(track.path.display().to_string())
                    .font(theme::mono(9.5))
                    .color(theme::DIM),
            )
            .on_hover_text(track.path.display().to_string());

            if let Some(problem) = track.needs_attention() {
                ui.add_space(6.0);
                ui.label(
                    RichText::new(format!("{} {problem}", theme::WARN))
                        .color(theme::ALERT)
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
                        .color(theme::ALERT)
                        .size(theme::SMALL),
                );
                if ui.button("Forget this track").clicked() {
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
                        .color(theme::AMBER)
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
                        Some(_) => (theme::TICK, theme::GO),
                        None => ("·", theme::DIM),
                    };
                    ui.label(RichText::new(mark).color(color));
                    ui.label(RichText::new(name).color(if path.is_some() {
                        theme::TEXT
                    } else {
                        theme::DIM
                    }));
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
                                RichText::new(tag).font(theme::mono(10.0)).color(theme::TEXT),
                            )
                            .fill(theme::BOOTH_2),
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
                        .hint_text(RichText::new("add a tag").monospace().color(theme::DIM)),
                );
                let entered = entry.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if (entered || ui.button("Add").clicked()) && !self.tag_entry.trim().is_empty() {
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
                                    RichText::new(tag).font(theme::mono(10.0)).color(theme::DIM),
                                )
                                .fill(theme::BOOTH),
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
                    RichText::new("none in key and in range").color(theme::DIM).size(theme::SMALL),
                );
            }
            for (id, name) in neighbours {
                if ui
                    .add(
                        egui::Label::new(RichText::new(name).color(theme::TEXT).size(theme::SMALL))
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
                        self.selected = Some(id);
                        self.waveform = None;
                        self.zoom = wave::Zoom::default();
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
                            self.note(format!("removed from \u{201c}{name}\u{201d}"), theme::TEXT);
                            touched = true;
                        }
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
                Pending::TrashDuplicates { ticked, picked } => {
                    self.trash_duplicates(&ticked, &picked);
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
                        self.note(format!("copied {path}"), theme::TEXT);
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
                Pending::WriteTags(id) => self.write_tags(id, musicai::tag::OnExisting::Overwrite),
                Pending::PlaceCue { id, letter, time_ms } => {
                    self.place_cue(id, letter, time_ms);
                    touched = true;
                }
                Pending::PaintAs(mode) => {
                    self.config.paint = mode;
                    if let Err(e) = self.config.save(&self.config_path) {
                        crate::warn!("could not save the settings: {e:#}");
                    }
                    crate::debug!("waveform coloured by {}", mode.label());
                }
                Pending::AnswerMatch { id, take } => {
                    if let Some(at) = self.questions.iter().position(|q| q.id == id) {
                        let question = self.questions.remove(at);
                        if take {
                            self.apply_match(id, &question.candidate);
                        } else {
                            crate::info!("#{id} kept its own name over the fingerprint");
                            // Marked as answered, so the same question is not
                            // asked again on the next pass.
                            if let Some(track) = self.library.get_mut(id) {
                                track.from_tags = true;
                            }
                        }
                        touched = true;
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
                Pending::RemoveCue { id, letter } => {
                    if let Some(track) = self.library.get_mut(id) {
                        track.cues.retain(|cue| cue.letter != letter);
                    }
                    touched = true;
                }
                Pending::RenameCue { id, letter, label } => {
                    if let Some(track) = self.library.get_mut(id) {
                        if let Some(cue) = track.cues.iter_mut().find(|c| c.letter == letter) {
                            cue.label = label;
                        }
                    }
                    touched = true;
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
                self.note("nothing to identify", theme::DIM);
            }
            return;
        }
        let Some(key) = self.config.key() else {
            crate::warn!("no AcoustID key: {} left unidentified", plural(ids.len(), "track"));
            if announce {
                self.note(
                    "no AcoustID key — put one in Settings, or set ACOUSTID_API_KEY",
                    theme::AMBER,
                );
            }
            return;
        };
        let waiting = self.files_for(ids);
        if waiting.is_empty() {
            if announce {
                self.note("nothing to identify", theme::DIM);
            }
            return;
        }
        crate::info!("identifying {}", plural(waiting.len(), "track"));
        let ids: Vec<u32> = waiting.iter().map(|(id, _)| *id).collect();
        self.ensure_local(&ids);
        self.start(Job::Identify { tracks: waiting, key });
    }

    /// Fingerprint whatever showing is still unidentified.
    fn identify_showing(&mut self) {
        let waiting = self.acting_on(|track| !track.identified);
        if waiting.is_empty() {
            self.note("nothing showing needs identifying", theme::DIM);
            return;
        }
        self.identify_tracks(&waiting, true);
    }

    /// Decide what to do with one match, and do it.
    fn consider(&mut self, id: u32, found: crate::identify::Match) {
        let Some(track) = self.library.get(id) else { return };
        let decision = crate::identify::decide(track, &found, self.config.autotag_score);
        crate::debug!("#{id} {} at {:.0}%: {decision:?}", found.describe(), found.score * 100.0);

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
                };
                if !self.questions.iter().any(|q| q.id == id) {
                    self.questions.push(question);
                }
            }
            crate::identify::Decision::Reject => {}
        }
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
        // fingerprint will ask before overriding them.
        track.from_tags = true;
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
    }

    /// Write one track's names into the file's own tags.
    /// Put the collection's names into the file's own tag block.
    ///
    /// `on_existing` is the whole difference between the two ways this is
    /// reached. The button in the inspector is somebody saying "these ones,
    /// now", so it overwrites; the write-back that rides behind a fingerprint
    /// lookup fills in blanks and leaves anything already there alone, because
    /// nobody asked it to have an opinion about a value they typed.
    fn write_tags(&mut self, id: u32, on_existing: musicai::tag::OnExisting) {
        let Some(track) = self.library.get(id) else { return };
        // Asked of the writer rather than answered again here. This was a
        // second list of formats, and it had already fallen behind the first:
        // it still said FLAC and MP3 after the writer learned MP4, so an
        // identified `.m4a` was told it had nowhere to put a name that the
        // code underneath would have written.
        match musicai::tag::tag_kind(&track.path) {
            Some(musicai::tag::TagKind::None) => {
                self.note(format!("a .{} has nowhere to keep tags", track.format), theme::AMBER);
                return;
            }
            None => {
                self.note(
                    format!("a .{} is not a file whose tags can be written", track.format),
                    theme::AMBER,
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
        self.to_retag.push(Retag {
            id: track.id,
            path: track.path.clone(),
            artist: track.artist.clone(),
            title: track.title.clone(),
            album: track.album.clone(),
            date: track.year.map(|year| year.to_string()),
            on_existing,
        });
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
                            .color(theme::DIM)
                            .strong(),
                    )
                    .fill(theme::BOOTH),
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
                    .color(theme::DIM)
                    .strong(),
            );

            match self.library.drives.get(self.drive).cloned() {
                None => {
                    ui.label(RichText::new("none set up").color(theme::DIM));
                    if ui.button("Add a drive…").clicked() {
                        self.want_pick = Some(Picking::Drive);
                    }
                    if ui.button("Or an image…").clicked() {
                        self.want_pick = Some(Picking::Image);
                    }
                }
                Some(drive) => {
                    let mark = if drive.is_image { "▢" } else { "▣" };
                    let carries = drive.playlist_names();
                    let what = match carries.len() {
                        0 => "no playlists".to_string(),
                        1 => carries[0].clone(),
                        n => format!("{n} playlists"),
                    };
                    ui.label(format!(
                        "{mark} {} — {what} · {}",
                        drive.label,
                        plural(drive.written.len(), "track")
                    ));
                    // Which playlists go on the stick is the decision the dock
                    // exists for, so it is a menu here rather than a setting
                    // somewhere else.
                    ui.menu_button("playlists\u{2026}", |ui| {
                        ui.set_min_width(190.0);
                        if self.library.playlists.is_empty() {
                            ui.label(RichText::new("no playlists yet").color(theme::DIM));
                        }
                        for (folder, lists) in self.library.playlist_tree() {
                            if !folder.is_empty() {
                                ui.label(
                                    RichText::new(&folder).color(theme::DIM).size(theme::SMALL),
                                );
                            }
                            for playlist in lists {
                                let name = playlist.name.clone();
                                let mut on = carries.contains(&name);
                                if ui
                                    .checkbox(
                                        &mut on,
                                        format!("{name}  ({})", playlist.tracks.len()),
                                    )
                                    .changed()
                                {
                                    self.pending.push(Pending::DrivePlaylist { name, on });
                                }
                            }
                        }
                    });
                    if self.library.drives.len() > 1 && ui.button("next").clicked() {
                        self.drive = (self.drive + 1) % self.library.drives.len();
                        self.replan();
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let ready = !self.plan.is_empty() && !self.running();
                        if ui
                            .add_enabled(
                                ready,
                                egui::Button::new(
                                    RichText::new(theme::label_text("Sync"))
                                        .size(11.0)
                                        .color(theme::BOOTH)
                                        .strong(),
                                )
                                .fill(theme::AMBER),
                            )
                            .clicked()
                        {
                            self.sheet = true;
                        }
                        ui.label(
                            RichText::new(self.plan.delta())
                                .font(theme::mono(11.5))
                                .color(theme::AMBER),
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
                        .color(theme::DIM)
                        .strong(),
                )
                .fill(theme::BOOTH),
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
            ui.label(RichText::new("nothing yet").font(theme::mono(10.5)).color(theme::DIM));
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
                    .color(theme::DIM)
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
                                    .color(if confident { theme::GO } else { theme::AMBER }),
                            );
                            ui.vertical(|ui| {
                                ui.label(
                                    RichText::new(question.candidate.describe()).color(theme::TEXT),
                                );
                                ui.label(
                                    RichText::new(format!(
                                        "now: {} — from {}",
                                        question.current, question.source
                                    ))
                                    .font(theme::mono(10.0))
                                    .color(theme::DIM),
                                );
                            });
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui.button("Keep mine").clicked() {
                                        self.pending.push(Pending::AnswerMatch {
                                            id: question.id,
                                            take: false,
                                        });
                                    }
                                    if ui
                                        .add(
                                            egui::Button::new(
                                                RichText::new("Use this")
                                                    .color(theme::BOOTH)
                                                    .strong(),
                                            )
                                            .fill(theme::AMBER),
                                        )
                                        .clicked()
                                    {
                                        self.pending.push(Pending::AnswerMatch {
                                            id: question.id,
                                            take: true,
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
                    if ui.button("Use all").clicked() {
                        for question in &questions {
                            self.pending.push(Pending::AnswerMatch { id: question.id, take: true });
                        }
                    }
                    if ui.button("Keep all of mine").clicked() {
                        for question in &questions {
                            self.pending
                                .push(Pending::AnswerMatch { id: question.id, take: false });
                        }
                    }
                    ui.label(
                        RichText::new("Nothing is written to any file by answering these.")
                            .color(theme::DIM)
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
    /// What only one copy knows is folded into the one being kept before the
    /// rest go, so that being rid of a copy costs nothing: the album name that
    /// was only on the download, the cues placed on it, the playlist it was in.
    /// A copy that is byte-for-byte the kept one, or that merges without
    /// contradicting it, is ticked when the sheet opens — there is nothing to
    /// decide about either. A copy that disagrees is not, and says what about,
    /// until somebody says which answer is right.
    ///
    /// Nothing happens until the button at the bottom is pressed. This is the
    /// only thing in the program that deletes somebody's music, and it says how
    /// many files and how many megabytes before it does.
    fn duplicates_sheet(&mut self, ctx: &egui::Context) {
        use crate::library::{Field, Side};

        let groups = self.library.duplicate_groups(&self.config.library_path);
        let unchecked = self.library.unhashed().len();
        // What each copy would do to the track being kept, worked out once for
        // the frame: the rows read it, the opening ticks read it, and the
        // button at the bottom counts it.
        let plans: std::collections::HashMap<u32, crate::library::Merge> = groups
            .iter()
            .flat_map(|group| {
                group
                    .rest
                    .iter()
                    .map(|copy| (copy.id, self.library.plan_merge(group.keep, copy.id)))
            })
            .collect();

        let mut state = self.duplicates.take().unwrap_or_default();
        // A copy nobody has seen yet gets its opening answer: ticked when there
        // is nothing to decide, left alone when there is. Once seen it is the
        // user's, so unticking sticks.
        for group in &groups {
            for copy in &group.rest {
                if !state.seen.insert(copy.id) {
                    continue;
                }
                let clean = plans.get(&copy.id).is_none_or(|plan| plan.is_clean());
                if copy.identical || clean {
                    state.ticked.insert(copy.id);
                }
            }
        }

        let mut open = true;
        let mut delete = false;
        let mut look = false;

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
                    .fill(theme::BOOTH)
                    .stroke(egui::Stroke::new(1.0, theme::RULE))
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
                        .color(theme::AMBER)
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
                        .color(theme::DIM),
                    );
                    return;
                }

                ui.label(
                    RichText::new(
                        "Grouped by the sound in the file rather than by its name, so the \
                         same rip tagged twice is one record here. What only a copy knows \
                         is folded into the one kept, on top, before the rest go.",
                    )
                    .color(theme::DIM)
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
                        for group in &groups {
                            let Some(kept) = self.library.get(group.keep) else { continue };
                            ui.label(
                                RichText::new(kept.display_title())
                                    .color(theme::TEXT)
                                    .size(theme::BODY),
                            );
                            ui.horizontal(|ui| {
                                ui.add_space(14.0);
                                ui.label(
                                    RichText::new(format!("keep  {}", kept.path.display()))
                                        .font(theme::mono(10.0))
                                        .color(theme::GO),
                                );
                            });
                            for copy in &group.rest {
                                let Some(track) = self.library.get(copy.id) else { continue };
                                let path = track.path.display().to_string();
                                let plan = plans.get(&copy.id).cloned().unwrap_or_default();
                                // Every disagreement answered is a copy that is
                                // no longer waiting on anybody.
                                let answered = plan
                                    .conflicts
                                    .iter()
                                    .all(|c| state.picked.contains_key(&(copy.id, c.field)));

                                ui.horizontal(|ui| {
                                    ui.add_space(14.0);
                                    let mut on = state.ticked.contains(&copy.id);
                                    if ui
                                        .add_enabled(
                                            answered,
                                            egui::Checkbox::without_text(&mut on),
                                        )
                                        .on_disabled_hover_text("Say which answer is right first")
                                        .changed()
                                    {
                                        match on {
                                            true => state.ticked.insert(copy.id),
                                            false => state.ticked.remove(&copy.id),
                                        };
                                    }
                                    ui.label(
                                        RichText::new(path)
                                            .font(theme::mono(10.0))
                                            .color(theme::DIM),
                                    );
                                    let (note, color) = match (copy.identical, answered) {
                                        (true, _) => ("identical".to_string(), theme::DIM),
                                        (false, false) => (
                                            format!(
                                                "disagrees about {}",
                                                plan.conflicts
                                                    .iter()
                                                    .map(|c| c.field.name())
                                                    .collect::<Vec<_>>()
                                                    .join(", ")
                                            ),
                                            theme::ALERT,
                                        ),
                                        (false, true) => (plan.summary(), theme::AMBER),
                                    };
                                    ui.label(RichText::new(note).size(theme::SMALL).color(color));
                                });

                                // One line per disagreement, with both answers
                                // to choose between. Answering the last one
                                // ticks the copy, because that is what having
                                // answered it means.
                                for conflict in &plan.conflicts {
                                    let key = (copy.id, conflict.field);
                                    ui.horizontal(|ui| {
                                        ui.add_space(38.0);
                                        ui.label(
                                            RichText::new(format!("{}:", conflict.field.name()))
                                                .size(theme::SMALL)
                                                .color(theme::DIM),
                                        );
                                        for (side, value) in [
                                            (Side::Kept, &conflict.kept),
                                            (Side::Other, &conflict.other),
                                        ] {
                                            let chosen = state.picked.get(&key) == Some(&side);
                                            // A radio rather than a label that
                                            // happens to be clickable: this is
                                            // the one place in the sheet that
                                            // is waiting on a person, and it
                                            // has to look like it.
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
                                                if plan.conflicts.iter().all(|c| {
                                                    c.field == conflict.field
                                                        || state
                                                            .picked
                                                            .contains_key(&(copy.id, c.field))
                                                }) {
                                                    state.ticked.insert(copy.id);
                                                }
                                            }
                                        }
                                    });
                                }
                            }
                            ui.add_space(8.0);
                        }
                    });

                // The footer, in the space kept for it above.
                ui.separator();
                let bytes: u64 = state
                    .ticked
                    .iter()
                    .filter_map(|id| self.library.get(*id))
                    .map(|track| track.bytes)
                    .sum();
                let waiting = plans
                    .iter()
                    .filter(|(id, plan)| {
                        !plan.is_clean()
                            && !plan
                                .conflicts
                                .iter()
                                .all(|c| state.picked.contains_key(&(**id, c.field)))
                    })
                    .count();
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(
                            !state.ticked.is_empty(),
                            egui::Button::new(
                                RichText::new(format!(
                                    "Move {} to the trash  ({})",
                                    plural(state.ticked.len(), "file"),
                                    crate::sync::bytes(bytes)
                                ))
                                .color(theme::BOOTH)
                                .strong(),
                            )
                            .fill(theme::ALERT),
                        )
                        .on_hover_text(
                            "To the trash, not gone: this is the one thing here that touches \
                             your music",
                        )
                        .clicked()
                    {
                        delete = true;
                    }
                    if ui
                        .button(RichText::new("Untick all").size(theme::SMALL))
                        .on_hover_text("Start again from nothing chosen")
                        .clicked()
                    {
                        state.ticked.clear();
                    }
                    if waiting > 0 {
                        ui.label(
                            RichText::new(format!(
                                "{} still waiting on an answer",
                                plural(waiting, "copy")
                            ))
                            .size(theme::SMALL)
                            .color(theme::ALERT),
                        );
                    }
                });
            });

        if delete {
            // The picks travel with the ids: by the time this runs the sheet is
            // shut, and the merge still has to know how each disagreement was
            // settled.
            let ticked: Vec<u32> = state.ticked.iter().copied().collect();
            let picked: Vec<(u32, Field, Side)> =
                state.picked.iter().map(|((id, field), side)| (*id, *field, *side)).collect();
            self.pending.push(Pending::TrashDuplicates { ticked, picked });
            self.duplicates = None;
        } else {
            match open {
                true => self.duplicates = Some(state),
                false => self.duplicates = None,
            }
        }
        if look {
            self.hash_unchecked();
        }
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
                    .fill(theme::BOOTH)
                    .stroke(egui::Stroke::new(1.0, theme::RULE))
                    .inner_margin(egui::Margin::same(14)),
            )
            .show(ctx, |ui| {
                ui.label(
                    RichText::new(
                        "Terms narrow the list together. Put a - or ! in front of one to \
                         exclude it, and quotes around anything with a space in it.",
                    )
                    .color(theme::DIM)
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
                                                .color(theme::AMBER),
                                        )
                                        .sense(egui::Sense::click()),
                                    );
                                    if example.clicked() {
                                        chosen = Some(help.example.to_string());
                                    }
                                    if example.hovered() {
                                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                                    }
                                    ui.label(
                                        RichText::new(help.means)
                                            .color(theme::DIM)
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
                            .fill(theme::BOOTH_2)
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
                                                .color(if on { theme::BOOTH } else { theme::DIM }),
                                        )
                                        .fill(if on {
                                            theme::AMBER
                                        } else {
                                            theme::BOOTH
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
                                        .color(theme::DIM),
                                    )
                                    .on_hover_text(
                                        crate::log::default_path().display().to_string(),
                                    );
                                },
                            );
                        });
                    });

                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE.fill(theme::BOOTH).inner_margin(egui::Margin::same(8)))
                    .show(ctx, |ui| {
                        egui::ScrollArea::both()
                            .auto_shrink([false, false])
                            .stick_to_bottom(log.follows())
                            .show_rows(ui, 14.0, entries.len(), |ui, range| {
                                for entry in &entries[range] {
                                    let color = match entry.level {
                                        crate::log::Level::Error => theme::ALERT,
                                        crate::log::Level::Warn => theme::AMBER,
                                        crate::log::Level::Info => theme::TEXT,
                                        _ => theme::DIM,
                                    };
                                    ui.horizontal(|ui| {
                                        ui.spacing_mut().item_spacing.x = 8.0;
                                        ui.label(
                                            RichText::new(entry.stamp())
                                                .font(theme::mono(10.5))
                                                .color(theme::DIM),
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
                        .color(theme::DIM)
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
                        if ui.button("Choose…").clicked() {
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
                            RichText::new(policy.blurb()).color(theme::DIM).size(theme::SMALL),
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
                            RichText::new(quality.blurb()).color(theme::DIM).size(theme::SMALL),
                        );
                    }
                    ui.label(
                        RichText::new(
                            "A kit is rendered once and then played for years, so the slow one is \
                         the default. The fast one is for a first pass over a whole library.",
                        )
                        .color(theme::DIM)
                        .size(theme::SMALL),
                    );

                    ui.add_space(14.0);
                    pane_label(ui, "Track length");
                    ui.horizontal(|ui| {
                        for unit in crate::config::Length::ALL {
                            if ui.radio_value(&mut self.config.length, unit, unit.label()).changed()
                            {
                                changed = true;
                            }
                        }
                        ui.label(
                            RichText::new(
                                "Four beats to the bar, as the drive's own format counts.",
                            )
                            .color(theme::DIM)
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
                            RichText::new("AcoustID key").color(theme::DIM).size(theme::SMALL),
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
                                        .color(theme::DIM),
                                    ),
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
                        .color(theme::DIM)
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
                        ui.label(RichText::new(level.blurb()).color(theme::DIM).size(theme::SMALL));
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
                        .color(theme::DIM)
                        .size(theme::SMALL),
                    );

                    ui.add_space(14.0);
                    pane_label(ui, "rekordbox");
                    ui.label(
                        RichText::new(
                            "rekordbox keeps its library in an encrypted SQLite file. The key is \
                         the same on every installation; this build does not carry one. \
                         `python -m pyrekordbox download-key` prints it, or leave this blank \
                         and set REKORDBOX_KEY.",
                        )
                        .color(theme::DIM)
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
                        .changed()
                    {
                        self.config.rekordbox_key = key.trim().to_string();
                        changed = true;
                    }
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
                    pane_label(ui, "Where stems go");
                    for where_ in [crate::config::StemsIn::Beside, crate::config::StemsIn::Folder] {
                        if ui
                            .radio_value(&mut self.config.stems_in, where_, where_.label())
                            .changed()
                        {
                            changed = true;
                        }
                        ui.label(
                            RichText::new(where_.blurb()).color(theme::DIM).size(theme::SMALL),
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
                        .color(theme::DIM)
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
                            .color(theme::AMBER)
                            .size(theme::SMALL),
                        );
                        if ui.button("Copy them all in").clicked() {
                            self.pending.extend(outside.into_iter().map(Pending::Adopt));
                        }
                    }
                });
            });

        if changed {
            if let Err(e) = self.config.save(&self.config_path) {
                self.note(format!("could not save the settings: {e:#}"), theme::ALERT);
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

    /// What was just imported that a player will not open, and what to do.
    fn compatibility_sheet(&mut self, ctx: &egui::Context) {
        let waiting: Vec<(u32, String, PathBuf, musicai::compat::Problem)> = self
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
                ui.label(
                    RichText::new(
                        "These import, play here, and will be skipped when a drive is \
                         written. A CDJ-3000 takes MP3 and AAC at 44.1–48 kHz, and WAV, \
                         AIFF, FLAC and ALAC up to 96 kHz.",
                    )
                    .color(theme::DIM)
                    .size(theme::SMALL),
                );
                ui.add_space(8.0);

                egui::ScrollArea::vertical().max_height(240.0).show(ui, |ui| {
                    for (_, name, path, problem) in &waiting {
                        ui.label(RichText::new(name).color(theme::TEXT).size(theme::SMALL));
                        ui.label(
                            RichText::new(format!("{} — {}", problem.what(), problem.fix()))
                                .color(if problem.convertible() {
                                    theme::AMBER
                                } else {
                                    theme::ALERT
                                })
                                .size(theme::SMALL),
                        );
                        ui.label(
                            RichText::new(path.display().to_string())
                                .font(theme::mono(9.5))
                                .color(theme::DIM),
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
                                    .color(theme::BOOTH)
                                    .strong(),
                                )
                                .fill(theme::AMBER),
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
                            .color(theme::DIM)
                            .size(theme::SMALL),
                        );
                    }
                });
            });
        if !open {
            self.incompatible.clear();
        }
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
                    .color(theme::DIM)
                    .size(theme::SMALL),
                );
                ui.add_space(8.0);

                egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                    for (_, name, path) in &waiting {
                        ui.label(RichText::new(name).color(theme::TEXT).size(theme::SMALL));
                        ui.label(
                            RichText::new(path.display().to_string())
                                .font(theme::mono(9.5))
                                .color(theme::DIM),
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
                                    .color(theme::BOOTH)
                                    .strong(),
                            )
                            .fill(theme::AMBER),
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

    fn sync_sheet(&mut self, ctx: &egui::Context) {
        let Some(drive) = self.library.drives.get(self.drive).cloned() else {
            self.sheet = false;
            return;
        };
        let checks = sync::preflight(&self.library, &self.plan, &drive.path, drive.is_image);
        let worst = checks.iter().map(|c| c.level).max().unwrap_or(Level::Ok);

        let mut open = true;
        egui::Window::new(format!("SYNC → {}", drive.label))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(620.0)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .max_height(sheet_height(ctx))
            .frame(
                egui::Frame::NONE
                    .fill(theme::BOOTH)
                    .stroke(egui::Stroke::new(1.0, theme::RULE))
                    .inner_margin(egui::Margin::same(14)),
            )
            .show(ctx, |ui| {
                // Scrolled, because the sheet is no taller than the screen now.
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.label(
                        RichText::new(format!(
                            "Target: CDJ-3000 · writes: Device Library (export.pdb) + ANLZ{}",
                            if drive.is_image { " · into a FAT32 image" } else { "" }
                        ))
                        .font(theme::mono(10.5))
                        .color(theme::DIM),
                    );
                    ui.label(
                        RichText::new(format!(
                            "{} A CDJ-3000X will not read this drive: it needs OneLibrary, \
                         whose schema is not published.",
                            theme::WARN
                        ))
                        .font(theme::mono(10.5))
                        .color(theme::ALERT),
                    );

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
                            .color(if specs.is_empty() { theme::ALERT } else { theme::DIM }),
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
                        &match self.plan.stems.len() {
                            0 => "none".to_string(),
                            n => format!("{n} files"),
                        },
                        &sync::bytes(self.plan.stem_bytes),
                    );

                    ui.add_space(12.0);
                    for check in &checks {
                        ui.horizontal(|ui| {
                            let (mark, color) = match check.level {
                                Level::Ok => (theme::TICK, theme::GO),
                                Level::Warn => (theme::WARN, theme::AMBER),
                                Level::Bad => (theme::CROSS, theme::ALERT),
                            };
                            ui.label(RichText::new(mark).color(color).font(theme::mono(11.5)));
                            ui.label(
                                RichText::new(&check.text)
                                    .font(theme::mono(11.5))
                                    .color(theme::TEXT),
                            );
                        });
                    }

                    ui.add_space(6.0);
                    ui.label(
                    RichText::new(
                        "The database and every analysis file are read back off the drive after \
                         writing, by a parser that shares no code with the writer. Until that \
                         passes, this is not a finished drive.",
                    )
                    .font(theme::mono(10.0))
                    .color(theme::DIM),
                );

                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        let verb = if worst == Level::Bad { "Write anyway" } else { "Write" };
                        if ui
                            .add(
                                egui::Button::new(
                                    RichText::new(theme::label_text(verb))
                                        .size(11.0)
                                        .color(theme::BOOTH)
                                        .strong(),
                                )
                                .fill(if worst == Level::Bad {
                                    theme::ALERT
                                } else {
                                    theme::AMBER
                                }),
                            )
                            .clicked()
                        {
                            self.write_drive();
                        }
                        if ui.button("Cancel").clicked() {
                            self.sheet = false;
                        }
                        if worst == Level::Bad {
                            ui.label(
                                RichText::new("the files above will be skipped; the rest still go")
                                    .color(theme::DIM)
                                    .size(theme::SMALL),
                            );
                        }
                    });
                });
            });
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
                        self.note(format!("could not save the settings: {e:#}"), theme::ALERT);
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
    ui.label(RichText::new(theme::label_text(text)).size(theme::LABEL).color(theme::DIM).strong());
    ui.add_space(4.0);
}

fn measurement(ui: &mut Ui, name: &str, value: &str, good: bool) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        ui.label(RichText::new(name).font(theme::mono(11.0)).color(theme::DIM));
        ui.label(
            RichText::new(if good { theme::TICK } else { "·" })
                .font(theme::mono(11.0))
                .color(if good { theme::GO } else { theme::AMBER }),
        );
        ui.label(RichText::new(value).font(theme::mono(11.0)).color(if good {
            theme::GO
        } else {
            theme::AMBER
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
    format!("{:.2} · {}", track.bpm, length.describe(track.beats))
}

/// A track's beat times, reconstructed from its tempo and its first cue.
///
/// The full grid is not kept in the collection — it is thousands of numbers per
/// track, and the analysis file on the drive is where it belongs. What the
/// picture needs is where the bars fall, and a constant tempo from the first
/// downbeat gives that.
fn beat_times(track: &Track) -> Vec<u32> {
    if !track.has_grid || track.bpm <= 0.0 || track.duration_secs <= 0.0 {
        return Vec::new();
    }
    let period_ms = 60_000.0 / track.bpm;
    let first = track.cues.first().map(|cue| cue.time_ms).unwrap_or(0) as f64;
    let start = first % period_ms;
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
    let first = beats.first().copied().unwrap_or(0);
    if at_ms < first {
        return Some(0);
    }
    let period_ms = 60_000.0 / track.bpm;
    let index = ((at_ms - first) as f64 / period_ms).floor() as usize;
    Some(index.min(track.beats.saturating_sub(1)))
}

/// A position in a track, as minutes, seconds and hundredths.
fn time_text(ms: u32) -> String {
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

fn sheet_line(ui: &mut Ui, operation: &str, what: &str, size: &str) {
    ui.horizontal(|ui| {
        ui.allocate_ui_with_layout(
            egui::vec2(88.0, 18.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.label(
                    RichText::new(theme::label_text(operation))
                        .size(theme::LABEL)
                        .color(theme::DIM)
                        .strong(),
                );
            },
        );
        ui.allocate_ui_with_layout(
            egui::vec2(ui.available_width() - 100.0, 18.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.label(RichText::new(what).color(theme::TEXT));
            },
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(RichText::new(size).font(theme::mono(11.5)).color(theme::DIM));
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::CueMark;

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

        /// A window with one track and one playlist, showing browser and
        /// sidebar side by side — which is what a drag crosses.
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
                        ui.vertical(|ui| app.sidebar(ui));
                        ui.vertical(|ui| {
                            let widths = rows::columns(ui.available_width());
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

            // Pressed, moved, released — a drag is not one event, and egui only
            // starts one once the pointer has actually travelled.
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

            app.trash_duplicates(&[other], &[]);

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
                &[other],
                &[(other, crate::library::Field::Title, crate::library::Side::Other)],
            );

            assert_eq!(
                app.library.get(keep).unwrap().title,
                "Sirens (Original Mix)",
                "the answer given in the sheet was not the one applied"
            );
            let _ = std::fs::remove_dir_all(&dir);
        }

        #[test]
        fn the_copies_with_nothing_to_decide_are_ticked_and_the_rest_are_not() {
            // What opening the sheet should do on its own: an identical file is
            // a no-brainer and so is one that only fills blanks in, but a copy
            // that contradicts the kept track has to wait for a person.
            let (mut app, dir, _keep, clean) = two_copies("ticks");
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
            assert!(
                state.ticked.contains(&clean),
                "a copy that only fills blanks in was not ticked"
            );
            assert!(
                !state.ticked.contains(&arguing),
                "a copy that contradicts the kept track was ticked without being asked about"
            );

            let _ = std::fs::remove_dir_all(&dir);
        }

        #[test]
        fn unticking_a_copy_is_not_undone_on_the_next_frame() {
            let (mut app, dir, _keep, clean) = two_copies("sticky");
            app.duplicates = Some(Dupes::default());
            let mut harness = Harness::new_ui_state(
                |ui, app: &mut App| {
                    let ctx = ui.ctx().clone();
                    app.duplicates_sheet(&ctx);
                },
                app,
            );
            harness.run();
            assert!(harness.state().duplicates.as_ref().unwrap().ticked.contains(&clean));

            harness.state_mut().duplicates.as_mut().unwrap().ticked.remove(&clean);
            harness.run();
            harness.run();

            assert!(
                !harness.state().duplicates.as_ref().unwrap().ticked.contains(&clean),
                "the opening ticks were applied again over what the user did"
            );
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

            app.trash_duplicates(&[doomed], &[]);

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
            let id = app.library.add(std::path::Path::new("/nowhere/at/all/missing.flac"));

            app.trash_duplicates(&[id], &[]);
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
                            pinned_width(ui, |ui| {
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
                pinned_height(&mut dock, |ui| {
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
            pinned_width(&mut panel, |ui| draw(ui));
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
