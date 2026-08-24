//! One window. No modes.
//!
//! Rekordbox splits into an export mode and a performance mode — two mental
//! models over one library, a seam that exists for licensing reasons rather
//! than for anything a DJ wants. Here there is one view: the query bar across
//! the top, three panes under it, the prep editor beneath the browser rather
//! than in place of it, and the drive dock along the bottom where the delta is
//! permanently visible.

use std::path::PathBuf;
use std::sync::Arc;

use eframe::egui::{self, Color32, RichText, Ui};
use musicai::cli::ExportArgs;
use musicai::stems::Backend;

use crate::job::{self, Job, Runner, Update};
use crate::library::{now, Drive, Library, Playlist, SavedQuery, Track, Written};
use crate::query::{self, Context, Paint, Query};
use crate::rows;
use crate::sync::{self, Level, Plan};
use crate::theme;
use crate::wave;

/// How many log lines to keep.
const MAX_LOG: usize = 500;

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

    runner: Option<Runner>,
    progress: Option<(usize, usize)>,
    log: Vec<(String, Color32)>,

    /// The drive the dock is showing, as an index into the library's drives.
    drive: usize,
    plan: Plan,
    /// Set while the sync sheet is open.
    sheet: bool,

    pick: Option<(Picking, std::sync::mpsc::Receiver<Vec<PathBuf>>)>,
    /// Set when the command bar should take keyboard focus.
    focus_bar: bool,
    /// The playlist name being typed on the actions strip.
    playlist_entry: String,
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
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, files: Vec<PathBuf>) -> Self {
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

        let mut app = Self {
            library,
            library_path,
            text: String::new(),
            query: Query::default(),
            view: View::All,
            playlist: String::new(),
            rows: Vec::new(),
            selected: None,
            waveform: None,
            runner: None,
            progress: None,
            log: Vec::new(),
            drive: 0,
            plan: Plan::default(),
            sheet: false,
            pick: None,
            focus_bar: false,
            playlist_entry: String::new(),
            want_pick: None,
            status,
            wake: Arc::new(|| {}),
            wake_installed: false,
        };
        app.rebuild();

        // Lets the layout check open the window on the sync sheet, which is
        // otherwise two clicks in. Animations are switched off with it, because
        // the check captures the first frame and would otherwise photograph
        // every fade half-finished. Compiled out of any ordinary build.
        #[cfg(feature = "screenshot")]
        if std::env::var_os("BOOTH_OPEN_SHEET").is_some() {
            app.sheet = true;
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

    fn start(&mut self, job: Job) {
        if self.running() {
            self.note("something is already running", theme::AMBER);
            return;
        }
        self.progress = Some((0, 0));
        self.runner = Some(Runner::start(job, self.wake.clone()));
    }

    fn import(&mut self, paths: Vec<PathBuf>) {
        self.start(Job::Import { paths, recursive: true });
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
        self.start(Job::Separate {
            tracks: waiting,
            out_dir: crate::library::data_dir().join("stems"),
            backend: Backend::Demucs,
        });
    }

    fn collect(&mut self) {
        let Some(runner) = &self.runner else { return };
        let updates = runner.drain();
        let mut finished = false;
        let mut changed = false;

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
        if changed {
            self.rebuild();
        }
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
        });
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
        let bottom = wave::HEIGHT + wave::STRIP_HEIGHT + 78.0;
        let list_height = (ui.available_height() - bottom).max(120.0);

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

        let mut clicked = None;
        for line in &self.rows {
            let selected = Some(line.track.id) == self.selected;
            if rows::row(ui, &line.track, line.indented, selected, &widths) {
                clicked = Some(line.track.id);
            }
        }
        if let Some(id) = clicked {
            if Some(id) != self.selected {
                self.selected = Some(id);
                self.waveform = None;
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

        let bands: &[u8] = match &self.waveform {
            Some((id, bands)) if *id == track.id => bands,
            _ => &[],
        };
        let beat_ms = beat_times(&track);
        let waveform = wave::Waveform {
            bands,
            duration_secs: track.duration_secs,
            beat_ms: &beat_ms,
            cues: &track.cues,
            position: None,
        };
        wave::show(ui, &waveform);
        wave::phrase_strip(ui, &track.phrases, track.duration_secs);

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
        if bands.is_empty() && track.analyzed {
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

    fn inspector(&mut self, ui: &mut Ui) {
        pane_label(ui, "Now inspecting");
        let Some(track) = self.selected_track().cloned() else {
            ui.label(RichText::new("nothing selected").color(theme::DIM));
            return;
        };

        ui.label(RichText::new(track.display_title()).strong());
        let mut line = track.artist.clone();
        if !track.album.is_empty() {
            line.push_str(&format!(" · {}", track.album));
        }
        if let Some(year) = track.year {
            line.push_str(&format!(" · {year}"));
        }
        ui.label(RichText::new(line).color(theme::DIM).size(theme::SMALL));
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

        if let Some(problem) = track.needs_attention() {
            ui.add_space(6.0);
            ui.label(
                RichText::new(format!("{} {problem}", theme::WARN))
                    .color(theme::ALERT)
                    .size(theme::SMALL),
            );
        }

        // A file that has been moved or deleted since it was scanned. The
        // record can be forgotten; nothing on disk is touched either way, which
        // is why this is the one removal offered without a confirmation.
        if !track.path.exists() {
            ui.add_space(6.0);
            ui.label(
                RichText::new(format!("{} the file is not where it was", theme::WARN))
                    .color(theme::ALERT)
                    .size(theme::SMALL),
            );
            if ui.button("Forget this track").clicked() {
                self.library.remove(track.id);
                self.save();
                self.rebuild();
                return;
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
            for tag in &track.tags {
                ui.label(
                    RichText::new(tag)
                        .font(theme::mono(10.0))
                        .color(theme::DIM)
                        .background_color(theme::BOOTH_2),
                );
            }
            if track.tags.is_empty() {
                ui.label(RichText::new("none").color(theme::DIM).size(theme::SMALL));
            }
        });

        ui.add_space(14.0);
        pane_label(ui, "Harmonic neighbours");
        let neighbours: Vec<String> = self
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
            .map(|other| format!("{} — {}", other.artist, other.title))
            .collect();
        if neighbours.is_empty() {
            ui.label(
                RichText::new("none in key and in range").color(theme::DIM).size(theme::SMALL),
            );
        }
        for neighbour in neighbours {
            ui.label(RichText::new(neighbour).color(theme::TEXT).size(theme::SMALL));
        }
    }

    fn dock(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
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
