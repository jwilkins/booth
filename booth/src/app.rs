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
use crate::library::{now, Drive, Library, Playlist, SavedQuery, Track, Written};
use crate::player::Player;
use crate::query::{self, Context, Paint, Query};
use crate::rows;
use crate::sync::{self, Level, Plan};
use crate::theme;
use crate::wave;

/// How many log lines to keep.
const MAX_LOG: usize = 500;

/// How much room under the list the prep editor needs.
///
/// Added up from its parts rather than guessed, because the failure is silent:
/// a budget a few points short does not overflow, it quietly clips the last row
/// off the bottom of the window, and the measurements line is the row it takes.
const PREP_HEIGHT: f32 = wave::HEIGHT
    + wave::STRIP_HEIGHT
    // the actions strip, the cue strip, and the measurements line
    + 3.0 * 24.0
    // the separator and the spacing between all of them
    + 40.0;

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
    selected: Option<u32>,
    /// The three-band waveform of the selected track, once it has been read.
    waveform: Option<(u32, Vec<u8>)>,

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

    runner: Option<Runner>,
    /// Work waiting for the runner. Jobs chain — an import that turns up
    /// external files queues the copy behind itself — and a queue is the only
    /// honest way to say so.
    queued: VecDeque<Job>,
    progress: Option<(usize, usize)>,
    log: Vec<(String, Color32)>,

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
    /// The track whose fields are being edited, and the text as typed. Kept
    /// apart from the collection so that a half-typed name is not a name.
    editing: Option<Edit>,

    pick: Option<(Picking, std::sync::mpsc::Receiver<Vec<PathBuf>>)>,
    /// Set when the command bar should take keyboard focus.
    focus_bar: bool,
    /// The playlist name being typed on the actions strip.
    playlist_entry: String,
    /// The tag being typed in the inspector.
    tag_entry: String,
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
    Adopt(u32),
    AddTag(u32, String),
    RemoveTag(u32, String),
    CommitEdit,
    CancelEdit,
    WriteTags(u32),
    /// Move a cue to a new time, or add one if it is not there yet.
    PlaceCue {
        id: u32,
        letter: u8,
        time_ms: u32,
    },
    TogglePlayback(u32),
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
}

/// A track's names, while they are being edited.
///
/// Held separately from the record so that what is typed is not the collection
/// until it is committed: a name is not half a name on its way to being one,
/// and a rebuild in the middle of typing should not rearrange the list under
/// the cursor.
struct Edit {
    id: u32,
    artist: String,
    title: String,
    album: String,
    year: String,
}

impl Edit {
    fn of(track: &Track) -> Self {
        Self {
            id: track.id,
            artist: track.artist.clone(),
            title: track.title.clone(),
            album: track.album.clone(),
            year: track.year.map(|y| y.to_string()).unwrap_or_default(),
        }
    }

    /// Whether anything was actually changed.
    fn differs_from(&self, track: &Track) -> bool {
        self.artist.trim() != track.artist
            || self.title.trim() != track.title
            || self.album.trim() != track.album
            || self.year.trim() != track.year.map(|y| y.to_string()).unwrap_or_default()
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
            selected: None,
            waveform: None,
            player: None,
            player_problem: None,
            loading: None,
            runner: None,
            queued: VecDeque::new(),
            progress: None,
            log: Vec::new(),
            drive: 0,
            plan: Plan::default(),
            sheet: false,
            settings: false,
            asking: Vec::new(),
            editing: None,
            pick: None,
            focus_bar: false,
            playlist_entry: String::new(),
            tag_entry: String::new(),
            playhead_ms: None,
            cue_entry: (None, String::new()),
            pending: Vec::new(),
            want_pick: None,
            status,
            wake: Arc::new(|| {}),
            wake_installed: false,
        };
        app.rebuild();

        // The device is opened once, at startup, and kept: opening one per
        // track costs a noticeable gap and, on some hosts, a click.
        match Player::open() {
            Ok(player) => app.player = Some(player),
            Err(e) => app.player_problem = Some(format!("{e:#}")),
        }

        // Lets the layout check open the window on the sync sheet, which is
        // otherwise two clicks in. Animations are switched off with it, because
        // the check captures the first frame and would otherwise photograph
        // every fade half-finished. Compiled out of any ordinary build.
        #[cfg(feature = "screenshot")]
        if let Some(which) = std::env::var_os("BOOTH_OPEN_SHEET") {
            match which.to_string_lossy().as_ref() {
                "settings" => app.settings = true,
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

    fn note(&mut self, text: impl Into<String>, color: Color32) {
        self.log.push((text.into(), color));
        if self.log.len() > MAX_LOG {
            let excess = self.log.len() - MAX_LOG;
            self.log.drain(0..excess);
        }
    }

    // -- the list ----------------------------------------------------------

    /// Re-run the query and rebuild the rows.
    ///
    /// Everything the browser shows comes through here, so a fixed view and a
    /// typed query are the same mechanism — there is no second filtering path
    /// that could disagree with the one the bar describes.
    fn rebuild(&mut self) {
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

        let in_view: Vec<Track> = self
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
        }
        self.replan();
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
    fn step(&mut self, delta: isize) {
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
        }
    }

    // -- jobs --------------------------------------------------------------

    /// Queue a job, and run it if nothing else is.
    fn start(&mut self, job: Job) {
        self.queued.push_back(job);
        self.pump();
    }

    /// Start the next job if the runner is free.
    fn pump(&mut self) {
        if self.running() {
            return;
        }
        let Some(job) = self.queued.pop_front() else { return };
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

    /// Analyse everything showing that has not been analysed.
    fn analyze_unprepared(&mut self) {
        let waiting: Vec<(u32, PathBuf)> = self
            .rows
            .iter()
            .filter(|row| !row.indented && !row.track.analyzed)
            .map(|row| (row.track.id, row.track.path.clone()))
            .collect();
        if waiting.is_empty() {
            self.note("nothing showing needs analysing", theme::DIM);
            return;
        }
        // Reading a file is reaching for it, so this is one of the moments the
        // copy-in policy is about. The copy is queued first, and the analysis
        // behind it, so it runs against whatever the track's path is by then.
        let ids: Vec<u32> = waiting.iter().map(|(id, _)| *id).collect();
        self.ensure_local(&ids);
        self.start(Job::Analyze(waiting));
    }

    fn render_stems(&mut self) {
        let waiting: Vec<(u32, PathBuf)> = self
            .rows
            .iter()
            .filter(|row| !row.indented && row.track.stems.is_empty())
            .map(|row| (row.track.id, row.track.path.clone()))
            .collect();
        if waiting.is_empty() {
            self.note("everything showing already has a stem kit", theme::DIM);
            return;
        }
        let ids: Vec<u32> = waiting.iter().map(|(id, _)| *id).collect();
        self.ensure_local(&ids);
        self.start(Job::Separate {
            tracks: waiting,
            out_dir: self.config.stems_path.clone(),
            backend: Backend::Demucs,
        });
    }

    fn collect(&mut self) {
        let Some(runner) = &self.runner else { return };
        let updates = runner.drain();
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
                        merged.stems = job::find_stems(
                            &crate::library::data_dir().join("stems"),
                            &merged.path,
                        );
                        *track = merged;
                    }
                    imported.push(id);
                    changed = true;
                }
                Update::Decoded { id, sound } => {
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
                Update::Adopted { id, to } => {
                    if let Some(track) = self.library.get_mut(id) {
                        // The record now points at the copy. The original is
                        // untouched on disk; the collection simply stops
                        // depending on it.
                        track.path = to;
                    }
                    changed = true;
                }
                Update::Analyzed(analyzed) => {
                    if let Some(track) = self.library.get_mut(analyzed.id) {
                        track.bpm = analyzed.bpm;
                        track.grid_confidence = analyzed.grid_confidence;
                        track.has_grid = analyzed.has_grid;
                        track.beats = analyzed.beats;
                        track.key = analyzed.key.clone();
                        track.key_confidence = analyzed.key_confidence;
                        track.energy = analyzed.energy;
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
                Update::Progress { done, total } => self.progress = Some((done, total)),
                Update::Line(text) => self.note(text, theme::TEXT),
                Update::Failed { path, message } => {
                    // Whatever failed, nothing is arriving for the deck now.
                    self.loading = None;
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
                    match result {
                        Ok(()) => self.status = "done".into(),
                        Err(message) => {
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
            self.save();
        }
        if !imported.is_empty() {
            self.ensure_local(&imported);
        }
        if changed {
            self.rebuild();
        }
        // Whatever was queued behind this — the copy an import turned up, say —
        // starts now.
        self.pump();
    }

    fn save(&mut self) {
        if let Err(e) = self.library.save(&self.library_path) {
            self.note(format!("could not save the collection: {e:#}"), theme::ALERT);
        }
    }

    // -- the sync ----------------------------------------------------------

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
        args.playlist = drive.playlist.clone();

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
        let playlist = self
            .library
            .playlists
            .first()
            .map(|p| p.name.clone())
            .unwrap_or_else(|| "musicai".to_string());
        self.library.drives.push(Drive {
            label,
            path,
            is_image,
            playlist,
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

/// "1 track", "2 tracks".
fn plural(count: usize, noun: &str) -> String {
    if count == 1 {
        format!("1 {noun}")
    } else {
        format!("{count} {noun}s")
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
        egui::TopBottomPanel::bottom("dock").frame(bar_frame()).show(ctx, |ui| self.dock(ui));

        egui::SidePanel::left("collection")
            .exact_width(178.0)
            .frame(pane_frame())
            .resizable(false)
            .show(ctx, |ui| self.sidebar(ui));
        egui::SidePanel::right("inspector")
            .exact_width(210.0)
            .frame(pane_frame())
            .resizable(false)
            .show(ctx, |ui| self.inspector(ui));

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

        // Everything the panels asked for happens here, after they have all
        // drawn, so no panel ever reads a collection halfway through a change.
        self.apply_pending();
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
            if i.key_pressed(egui::Key::ArrowDown) {
                self.step(1);
            }
            if i.key_pressed(egui::Key::ArrowUp) {
                self.step(-1);
            }
            if i.key_pressed(egui::Key::Space) {
                if let Some(id) = self.selected {
                    self.pending.push(Pending::TogglePlayback(id));
                }
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

            // The queue indicator, right-aligned, which is the only place a
            // running job is reported. A modal progress dialog over a library
            // is a library you cannot use while it works.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                match &self.runner {
                    Some(runner) => {
                        let (done, total) = self.progress.unwrap_or((0, 0));
                        let text = if total > 0 {
                            format!("{} {} {done}/{total}", theme::SPINNER, runner.name)
                        } else {
                            format!("{} {}", theme::SPINNER, runner.name)
                        };
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

        ui.add_space(16.0);
        pane_label(ui, "Playlists");
        let tree: Vec<(String, Vec<(String, usize)>)> = self
            .library
            .playlist_tree()
            .into_iter()
            .map(|(folder, lists)| {
                (folder, lists.into_iter().map(|p| (p.name.clone(), p.tracks.len())).collect())
            })
            .collect();
        if tree.is_empty() {
            ui.label(RichText::new("none yet").color(theme::DIM).size(theme::SMALL));
        }
        for (folder, lists) in tree {
            if !folder.is_empty() {
                ui.label(RichText::new(format!("▾ {folder}")).color(theme::TEXT));
            }
            for (name, count) in lists {
                let on = self.view == View::Playlist && self.playlist == name;
                let color = if on { theme::AMBER } else { theme::DIM };
                let indent = if folder.is_empty() { 0.0 } else { 12.0 };
                ui.horizontal(|ui| {
                    ui.add_space(indent);
                    if ui
                        .add(
                            egui::Label::new(RichText::new(&name).color(color))
                                .sense(egui::Sense::click()),
                        )
                        .clicked()
                    {
                        self.view = View::Playlist;
                        self.playlist = name.clone();
                        self.rebuild();
                    }
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
        let list_height = (ui.available_height() - PREP_HEIGHT).max(120.0);

        egui::ScrollArea::vertical()
            .max_height(list_height)
            .auto_shrink([false, false])
            .show(ui, |ui| self.rows_table(ui));

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
        let showing = self.rows.iter().filter(|row| !row.indented).count();
        let unanalysed =
            self.rows.iter().filter(|row| !row.indented && !row.track.analyzed).count();
        let unstemmed =
            self.rows.iter().filter(|row| !row.indented && row.track.stems.is_empty()).count();

        ui.horizontal(|ui| {
            if ui.add_enabled(idle, egui::Button::new("Add music…")).clicked() {
                self.want_pick = Some(Picking::Music);
            }
            if ui
                .add_enabled(
                    idle && unanalysed > 0,
                    egui::Button::new(format!("Analyse {unanalysed}")),
                )
                .on_hover_text("Grid, key, phrases and cues for everything showing that has none")
                .clicked()
            {
                self.analyze_unprepared();
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
            ui.label(RichText::new("to playlist").color(theme::DIM).size(theme::SMALL));
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

    /// Put everything showing into a playlist, making it if it is new.
    fn add_to_playlist(&mut self, name: &str) {
        let ids: Vec<u32> =
            self.rows.iter().filter(|row| !row.indented).map(|row| row.track.id).collect();
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
    }

    fn rows_table(&mut self, ui: &mut Ui) {
        let widths = rows::columns(ui.available_width());
        rows::header_row(ui, &widths);

        let mut hit = None;
        for line in &self.rows {
            let selected = Some(line.track.id) == self.selected;
            if let Some(what) = rows::row(ui, &line.track, line.indented, selected, &widths) {
                hit = Some((line.track.id, what));
            }
        }
        if let Some((id, what)) = hit {
            if Some(id) != self.selected {
                self.selected = Some(id);
                self.waveform = None;
                // A different track: start it from the top rather than from
                // wherever the last one's playhead happened to be.
                self.playhead_ms = None;
            }
            if what == rows::Hit::Opened {
                self.pending.push(Pending::TogglePlayback(id));
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
        let waveform = wave::Waveform {
            bands,
            duration_secs: track.duration_secs,
            beat_ms: &beat_ms,
            cues: &track.cues,
            position: playhead
                .map(|ms| (ms as f64 / (track.duration_secs * 1000.0).max(1.0)) as f32),
        };
        // Drawn before anything below touches the collection: `waveform`
        // borrows the cached picture out of the window's own state, and that
        // borrow has to be finished with before the panel changes anything.
        let touched = wave::show(ui, &waveform);
        wave::phrase_strip(ui, &track.phrases, track.duration_secs);

        match touched {
            Some(wave::Touched::Scrubbed(ms)) => {
                self.playhead_ms = Some(ms);
                self.pending.push(Pending::SeekDeck { id: track.id, time_ms: ms });
            }
            Some(wave::Touched::Moved { letter, time_ms }) => {
                self.pending.push(Pending::PlaceCue { id: track.id, letter, time_ms })
            }
            None => {}
        }

        self.cue_strip(ui, &track);

        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 16.0;
            measurement(ui, "grid", &grid_text(&track), track.has_grid);
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

        // The waveform is only read when it is looked at: a collection of
        // thousands cannot keep every picture in memory, and re-measuring one
        // track takes less time than the click that asked for it.
        if !has_bands && track.analyzed {
            match crate::library::cached_waveform(track.id) {
                Some(cached) => self.waveform = Some((track.id, cached)),
                // Nothing cached — analysed by an older version, or the cache
                // was cleared. Measure it again, once, in the background.
                None if !self.running() => {
                    self.start(Job::Analyze(vec![(track.id, track.path.clone())]))
                }
                None => {}
            }
        }
    }

    // -- the deck ----------------------------------------------------------

    /// Play a track, decoding it first if it is not the one already loaded.
    fn audition(&mut self, id: u32, from_secs: Option<f64>) {
        let Some(player) = &self.player else { return };
        let Some(track) = self.library.get(id) else { return };
        if !track.path.exists() {
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
        let path = track.path.clone();
        self.ensure_local(&[id]);
        self.loading = Some((id, true));
        self.start(Job::Decode { id, path });
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

        let position = match loaded {
            true => player.position_secs(),
            false => 0.0,
        };
        ui.label(
            RichText::new(format!(
                "{} / {}",
                time_text((position * 1000.0) as u32),
                time_text((track.duration_secs * 1000.0) as u32)
            ))
            .font(theme::mono(10.5))
            .color(if loaded { theme::TEXT } else { theme::DIM }),
        );

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
                ui.label(RichText::new(label).color(theme::DIM).size(theme::SMALL));
                ui.add(
                    egui::TextEdit::singleline(value)
                        .desired_width(width)
                        .font(theme::sans(theme::BODY)),
                );
            }
            field(ui, width, "Title", &mut editing.title);
            field(ui, width, "Artist", &mut editing.artist);
            field(ui, width, "Album", &mut editing.album);
            field(ui, width, "Year", &mut editing.year);

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
            if self.config.write_tags_to_files {
                ui.label(
                    RichText::new(if taggable {
                        "Saving also writes these into the file."
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
    fn apply_pending(&mut self) {
        let pending = std::mem::take(&mut self.pending);
        if pending.is_empty() {
            return;
        }
        let mut touched = false;

        for action in pending {
            match action {
                Pending::Select(id) => {
                    if Some(id) != self.selected {
                        self.selected = Some(id);
                        self.waveform = None;
                        // Selecting from the neighbours list can leave the
                        // query showing something the track is not in; the
                        // browser widens rather than the selection being lost.
                        if !self.rows.iter().any(|row| row.track.id == id) {
                            self.view = View::All;
                            self.text.clear();
                        }
                        touched = true;
                    }
                }
                Pending::Forget(id) => {
                    self.library.remove(id);
                    touched = true;
                }
                Pending::Adopt(id) => self.adopt(&[id]),
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
                Pending::WriteTags(id) => self.write_tags(id),
                Pending::PlaceCue { id, letter, time_ms } => {
                    self.place_cue(id, letter, time_ms);
                    touched = true;
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
            self.rebuild();
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
        track.artist = edit.artist.trim().to_string();
        track.title = edit.title.trim().to_string();
        track.album = edit.album.trim().to_string();
        track.year = edit.year.trim().parse().ok();

        if self.config.write_tags_to_files {
            self.write_tags(edit.id);
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
    fn write_tags(&mut self, id: u32) {
        let Some(track) = self.library.get(id) else { return };
        if !matches!(track.format.as_str(), "flac" | "mp3") {
            self.note(format!("a .{} carries no standard tag block", track.format), theme::AMBER);
            return;
        }
        let job = Retag {
            id: track.id,
            path: track.path.clone(),
            artist: track.artist.clone(),
            title: track.title.clone(),
            album: track.album.clone(),
            date: track.year.map(|year| year.to_string()),
        };
        self.start(Job::Retag(vec![job]));
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
                    ui.label(format!(
                        "{mark} {} — {} · {}",
                        drive.label,
                        drive.playlist,
                        plural(drive.written.len(), "track")
                    ));
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

        // The log sits under the dock, at the size of a couple of lines: enough
        // to see what just happened without it becoming the window.
        if !self.log.is_empty() {
            ui.add_space(4.0);
            egui::ScrollArea::vertical().max_height(52.0).stick_to_bottom(true).show(ui, |ui| {
                for (text, color) in &self.log {
                    ui.label(RichText::new(text).font(theme::mono(10.5)).color(*color));
                }
            });
        }
    }

    /// The sheet: what would change, then whether it can.
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
            .frame(sheet_frame())
            .show(ctx, |ui| {
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
                    ui.label(RichText::new(policy.blurb()).color(theme::DIM).size(theme::SMALL));
                    ui.add_space(4.0);
                }

                ui.add_space(10.0);
                pane_label(ui, "Tags");
                if ui
                    .checkbox(
                        &mut self.config.write_tags_to_files,
                        "Editing a name also rewrites the file's tags",
                    )
                    .changed()
                {
                    changed = true;
                }
                ui.label(
                    RichText::new(
                        "Off by default. A collection edit is cheap and reversible; \
                         rewriting somebody's files is neither. FLAC and MP3 only — a WAV \
                         has nowhere to put them.",
                    )
                    .color(theme::DIM)
                    .size(theme::SMALL),
                );

                ui.add_space(14.0);
                pane_label(ui, "Stems folder");
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

                ui.add_space(12.0);
                let outside =
                    self.external(&self.library.tracks.iter().map(|t| t.id).collect::<Vec<_>>());
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
            .frame(
                egui::Frame::NONE
                    .fill(theme::BOOTH)
                    .stroke(egui::Stroke::new(1.0, theme::RULE))
                    .inner_margin(egui::Margin::same(14)),
            )
            .show(ctx, |ui| {
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
                        "{} A CDJ-3000X reads this format only in its compatibility mode; \
                         Device Library Plus has no public specification.",
                        theme::WARN
                    ))
                    .font(theme::mono(10.5))
                    .color(theme::ALERT),
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
                            RichText::new(&check.text).font(theme::mono(11.5)).color(theme::TEXT),
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

fn grid_text(track: &Track) -> String {
    if !track.analyzed {
        return "not analysed".to_string();
    }
    if !track.has_grid {
        return "none found".to_string();
    }
    format!("{:.2} · {} beats", track.bpm, track.beats)
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
