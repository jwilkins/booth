//! The window.
//!
//! Layout is a left column of files, a right column of options for whichever
//! task is selected, and a strip along the bottom that runs it and shows what
//! happened. Nothing in here does any audio work: it edits a [`Settings`] and
//! hands it to a [`Runner`].

use std::path::PathBuf;
use std::sync::Arc;

use booth_cli::report::Event;
use eframe::egui::{self, Color32, RichText};

use crate::job::{Runner, Settings, Task, Update};

/// How many log lines to keep. A folder of a thousand files would otherwise
/// grow the window's memory without bound for no benefit.
const MAX_LOG_LINES: usize = 2_000;

pub struct App {
    settings: Settings,
    runner: Option<Runner>,
    log: Vec<LogLine>,
    /// Files done and files expected, while a job runs.
    progress: Option<(usize, usize)>,
    /// How far into the file in hand, for a step too long to wait out.
    step: Option<u8>,
    /// Which step of a batch is running, as (name, index, total).
    stage: Option<(String, usize, usize)>,
    status: Status,
    /// Set when a file dialog is open, so a second click does not open another.
    picking: bool,
    pick_rx: Option<std::sync::mpsc::Receiver<Vec<PathBuf>>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Status {
    Idle,
    Running,
    Done(String),
    Failed(String),
}

#[derive(Clone)]
struct LogLine {
    text: String,
    kind: Kind,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Normal,
    Heading,
    Note,
    Error,
}

impl Default for App {
    fn default() -> Self {
        Self {
            settings: Settings::default(),
            runner: None,
            log: Vec::new(),
            progress: None,
            step: None,
            stage: None,
            status: Status::Idle,
            picking: false,
            pick_rx: None,
        }
    }
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, files: Vec<PathBuf>) -> Self {
        // Slightly roomier than the default, which is tuned for dense tools;
        // this one is mostly labelled controls and reads better with air.
        cc.egui_ctx.style_mut(|style| {
            style.spacing.item_spacing = egui::vec2(8.0, 8.0);
            style.spacing.button_padding = egui::vec2(10.0, 5.0);
        });

        let mut app = Self::default();
        app.settings.add_files(files);

        // Lets the screenshot check open the app on a chosen tab. Compiled out
        // of any ordinary build.
        #[cfg(feature = "screenshot")]
        if let Ok(name) = std::env::var("BOOTH_GUI_TASK") {
            if let Some(task) = Task::ALL.iter().find(|t| t.title().eq_ignore_ascii_case(&name)) {
                app.settings.task = *task;
            }
        }

        app
    }

    fn running(&self) -> bool {
        matches!(self.status, Status::Running)
    }

    fn push(&mut self, text: String, kind: Kind) {
        self.log.push(LogLine { text, kind });
        if self.log.len() > MAX_LOG_LINES {
            let excess = self.log.len() - MAX_LOG_LINES;
            self.log.drain(0..excess);
        }
    }

    /// Take everything the worker has said since the last frame.
    fn collect_updates(&mut self) {
        let Some(runner) = &self.runner else { return };
        let updates = runner.drain();
        let cancelled = runner.cancelled();

        for update in updates {
            match update {
                Update::Event(Event::Stage { name, index, of }) => {
                    // Each step restarts the file count, so the stage is what
                    // says how far through the whole run this is.
                    self.stage = Some((name.clone(), index, of));
                    self.progress = None;
                    self.push(format!("{index}/{of} {name}"), Kind::Heading);
                }
                Update::Event(Event::Started { total }) => {
                    self.progress = Some((0, total));
                    self.step = None;
                }
                Update::Event(Event::Progress { done, total }) => {
                    self.progress = Some((done, total));
                    // A new file; the last one's position is not this one's.
                    self.step = None;
                }
                Update::Event(Event::Step { percent }) => self.step = Some(percent),
                Update::Event(Event::Heading(text)) => self.push(text, Kind::Heading),
                Update::Event(Event::Line(text)) => self.push(text, Kind::Normal),
                Update::Event(Event::Summary(text)) => self.push(text, Kind::Note),
                // This window keeps one list and no log, and a drive write is
                // several detail lines per file — enough to bury the results
                // they sit between. Booth has the log that these are for.
                Update::Event(Event::Detail(_)) => {}
                Update::Event(Event::Failed { path, message }) => {
                    self.push(format!("{}: {message}", path.display()), Kind::Error)
                }
                Update::Event(Event::Finished { processed, failed, cancelled }) => {
                    let done = processed.saturating_sub(failed);
                    self.status = if cancelled {
                        Status::Done(format!("Stopped after {done} of {processed}"))
                    } else if failed > 0 {
                        Status::Failed(format!("{done} done, {failed} failed"))
                    } else {
                        Status::Done(format!("{done} done"))
                    };
                }
                Update::Done(Ok(())) => {
                    self.progress = None;
                    self.step = None;
                    self.stage = None;
                    if self.running() {
                        // A command that reported nothing still ended.
                        self.status = Status::Done(if cancelled {
                            "Stopped".to_string()
                        } else {
                            "Done".to_string()
                        });
                    }
                    self.finish();
                }
                Update::Done(Err(message)) => {
                    self.progress = None;
                    self.stage = None;
                    // The per-file errors are already in the log; this is the
                    // summary the command returned.
                    if self.running() {
                        self.status = Status::Failed(message.clone());
                    }
                    self.push(message, Kind::Error);
                    self.finish();
                }
            }
        }
    }

    fn finish(&mut self) {
        if let Some(runner) = &mut self.runner {
            runner.join();
        }
        self.runner = None;
    }

    fn start(&mut self) {
        self.log.clear();
        self.stage = None;
        self.progress = Some((0, self.settings.files.len()));
        self.status = Status::Running;
        self.push(self.settings.command_line(), Kind::Note);
        self.runner = None;
    }

    /// Open a native file picker without blocking the window.
    ///
    /// The dialog is async on some platforms and blocking on others, so it runs
    /// on its own thread either way and posts its answer back. That also means
    /// the window keeps painting while a picker is open.
    fn pick(&mut self, folders: bool, ctx: &egui::Context) {
        if self.picking {
            return;
        }
        self.picking = true;
        let (tx, rx) = std::sync::mpsc::channel();
        self.pick_rx = Some(rx);
        let ctx = ctx.clone();

        std::thread::spawn(move || {
            let dialog = rfd::AsyncFileDialog::new();
            let chosen = if folders {
                let dialog = dialog.set_title("Choose folders of audio");
                pollster(dialog.pick_folders())
            } else {
                let dialog = dialog
                    .set_title("Choose audio files")
                    .add_filter("Audio", &["mp3", "flac", "wav"]);
                pollster(dialog.pick_files())
            };
            let paths =
                chosen.map(|files| files.into_iter().map(|f| f.path().to_path_buf()).collect());
            let _ = tx.send(paths.unwrap_or_default());
            ctx.request_repaint();
        });
    }

    fn collect_picked(&mut self) {
        let Some(rx) = &self.pick_rx else { return };
        if let Ok(paths) = rx.try_recv() {
            self.settings.add_files(paths);
            self.picking = false;
            self.pick_rx = None;
        }
    }
}

/// Block on a future on a thread that is not the UI thread.
fn pollster<T>(future: impl std::future::Future<Output = T>) -> T {
    async_std::task::block_on(future)
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.collect_updates();
        self.collect_picked();
        self.take_dropped_files(ctx);

        egui::TopBottomPanel::top("tasks").show(ctx, |ui| self.task_bar(ui));
        egui::TopBottomPanel::bottom("run").show(ctx, |ui| self.run_bar(ui));
        egui::SidePanel::left("files").default_width(280.0).show(ctx, |ui| self.file_list(ui, ctx));
        egui::CentralPanel::default().show(ctx, |ui| self.options(ui));
    }
}

impl App {
    /// Files dragged from Finder land here.
    fn take_dropped_files(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> =
            ctx.input(|i| i.raw.dropped_files.iter().filter_map(|f| f.path.clone()).collect());
        if !dropped.is_empty() {
            self.settings.add_files(dropped);
        }
    }

    fn task_bar(&mut self, ui: &mut egui::Ui) {
        let running = self.running();
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            for task in Task::ALL {
                let selected = self.settings.task == task;
                if ui
                    .add_enabled(!running, egui::Button::selectable(selected, task.title()))
                    .clicked()
                {
                    self.settings.task = task;
                }
            }
        });
        ui.label(RichText::new(self.settings.task.blurb()).weak());
        ui.add_space(6.0);
    }

    fn file_list(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let running = self.running();
        ui.add_space(6.0);
        ui.heading("Files");
        ui.horizontal(|ui| {
            if ui.add_enabled(!running, egui::Button::new("Add files…")).clicked() {
                self.pick(false, ctx);
            }
            if ui.add_enabled(!running, egui::Button::new("Add folders…")).clicked() {
                self.pick(true, ctx);
            }
        });

        let count = self.settings.files.len();
        ui.horizontal(|ui| {
            ui.label(match count {
                0 => "Nothing selected".to_string(),
                1 => "1 file".to_string(),
                n => format!("{n} files"),
            });
            if count > 0 && ui.add_enabled(!running, egui::Button::new("Clear")).clicked() {
                self.settings.files.clear();
            }
        });

        if count == 0 {
            ui.add_space(8.0);
            ui.label(RichText::new("Drop audio files or folders here.").weak());
        }

        // Folders are expanded when the job runs, so the checkbox belongs with
        // the file list rather than with any one task's options.
        ui.add_enabled(
            !running && count > 0,
            egui::Checkbox::new(&mut self.settings.recursive, "Search subfolders"),
        );

        ui.separator();
        let mut remove = None;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            for (index, path) in self.settings.files.iter().enumerate() {
                ui.horizontal(|ui| {
                    // Plain "x": the default font has no dedicated cross glyph
                    // and draws a placeholder box instead.
                    if ui.add_enabled(!running, egui::Button::new("x").small()).clicked() {
                        remove = Some(index);
                    }
                    let name = path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| path.display().to_string());
                    ui.label(name).on_hover_text(path.display().to_string());
                });
            }
        });
        if let Some(index) = remove {
            self.settings.files.remove(index);
        }
    }

    fn options(&mut self, ui: &mut egui::Ui) {
        let running = self.running();
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.add_enabled_ui(!running, |ui| match self.settings.task {
                Task::Batch => batch_options(ui, &mut self.settings),
                Task::Analyze => analyze_options(ui, &mut self.settings),
                Task::Normalize => normalize_options(ui, &mut self.settings),
                Task::Stems => stems_options(ui, &mut self.settings),
                Task::Tag => tag_options(ui, &mut self.settings),
            });
        });
    }

    fn run_bar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if self.running() {
                if ui.button("Stop").clicked() {
                    if let Some(runner) = &self.runner {
                        runner.cancel();
                    }
                    self.push("Stopping after the current file…".into(), Kind::Note);
                }
            } else {
                let button = egui::Button::new(RichText::new(self.settings.task.verb()).strong());
                if ui.add_enabled(self.settings.ready(), button).clicked() {
                    self.start();
                    let ctx = ui.ctx().clone();
                    let wake = Arc::new(move || ctx.request_repaint());
                    self.runner = Some(Runner::start(&self.settings, wake));
                }
            }

            match &self.status {
                Status::Idle => {
                    if !self.settings.ready() {
                        ui.label(RichText::new("Add some files to begin.").weak());
                    }
                }
                Status::Running => {
                    if let Some((name, index, of)) = &self.stage {
                        ui.label(RichText::new(format!("{index}/{of} {name}")).strong());
                    }
                    if let Some((done, total)) = self.progress {
                        // The bar counts files; separation is minutes each, so
                        // the one in hand contributes its own share rather than
                        // leaving the bar still for the whole of it.
                        let within = self.step.unwrap_or(0) as f32 / 100.0;
                        let fraction = match total {
                            0 => 0.0,
                            total => (done as f32 + within) / total as f32,
                        };
                        let text = match self.step {
                            Some(percent) => format!("{done}/{total} · {percent}%"),
                            None => format!("{done}/{total}"),
                        };
                        ui.add(egui::ProgressBar::new(fraction).desired_width(220.0).text(text));
                    }
                }
                Status::Done(text) => {
                    ui.label(RichText::new(text).color(Color32::from_rgb(0x2e, 0x7d, 0x32)));
                }
                Status::Failed(text) => {
                    ui.label(RichText::new(text).color(Color32::from_rgb(0xc6, 0x28, 0x28)));
                }
            }
        });

        ui.add_space(4.0);
        // Grows with the log up to a limit, rather than reserving the space
        // before there is anything to put in it.
        egui::ScrollArea::vertical()
            .max_height(180.0)
            .stick_to_bottom(true)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                for line in &self.log {
                    let text = RichText::new(&line.text).monospace();
                    ui.label(match line.kind {
                        Kind::Normal => text,
                        Kind::Heading => text.strong(),
                        Kind::Note => text.weak(),
                        Kind::Error => text.color(Color32::from_rgb(0xc6, 0x28, 0x28)),
                    });
                }
            });
        ui.add_space(4.0);
    }
}

// -- per-task options ------------------------------------------------------

fn batch_options(ui: &mut egui::Ui, settings: &mut Settings) {
    use booth_cli::cli::{NormalizeMode, Step};

    ui.heading("Batch");
    ui.label(
        RichText::new(
            "Every file goes through each step in turn. Tagging happens before separation, so \
             the stems inherit the tags that were just written.",
        )
        .weak(),
    );

    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label("Steps");
        for step in Step::ALL {
            let mut on = settings.steps.contains(&step);
            if ui.checkbox(&mut on, step.name()).changed() {
                if on {
                    settings.steps.push(step);
                } else {
                    settings.steps.retain(|s| *s != step);
                }
            }
        }
    });

    // The settings each step uses are the ones on its own tab, so rather than a
    // second copy of every control, the batch shows the few that decide what
    // the run costs and links the rest.
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label("Loudness");
        ui.selectable_value(&mut settings.normalize.mode, NormalizeMode::Replaygain, "Tag only");
        ui.selectable_value(&mut settings.normalize.mode, NormalizeMode::Reencode, "Re-encode");
    });
    ui.label(
        RichText::new(match settings.normalize.mode {
            NormalizeMode::Replaygain => {
                "Writes ReplayGain tags. The audio is untouched, and one copy of \
                 the library stays one copy. Wav files cannot carry these tags and are \
                 left alone."
            }
            NormalizeMode::Reencode => {
                "Writes a new file per track, and the later steps follow those \
                 rather than the originals."
            }
        })
        .weak(),
    );

    ui.horizontal(|ui| {
        ui.label("Separator");
        ui.selectable_value(
            &mut settings.stems.backend,
            booth_cli::stems::Backend::Demucs,
            "Demucs",
        );
        ui.selectable_value(
            &mut settings.stems.backend,
            booth_cli::stems::Backend::Dsp,
            "Built-in",
        );
    });

    if settings.tag.acoustid_key.as_deref().unwrap_or_default().trim().is_empty()
        && settings.steps.contains(&Step::Tag)
    {
        ui.add_space(4.0);
        ui.label(
            RichText::new(
                "No AcoustID key, so the tag step will be skipped. Set one on the Tag tab.",
            )
            .color(Color32::from_rgb(0xb2, 0x6a, 0x00)),
        );
    }

    ui.add_space(8.0);
    ui.label(RichText::new("Everything else comes from each step's own tab.").weak());
    ui.checkbox(
        &mut settings.normalize.dry_run,
        "Dry run — report what would happen, write nothing",
    );
}

fn analyze_options(ui: &mut egui::Ui, settings: &mut Settings) {
    ui.heading("Analyze");
    ui.checkbox(&mut settings.analyze.json, "Report as JSON");
    ui.label(RichText::new("Nothing is written. Results appear in the log below.").weak());
}

fn normalize_options(ui: &mut egui::Ui, settings: &mut Settings) {
    use booth_cli::cli::NormalizeMode;
    use booth_cli::normalize::PeakPolicy;

    let args = &mut settings.normalize;
    ui.heading("Normalize");

    ui.horizontal(|ui| {
        ui.label("Mode");
        ui.selectable_value(&mut args.mode, NormalizeMode::Reencode, "Re-encode");
        ui.selectable_value(&mut args.mode, NormalizeMode::Replaygain, "ReplayGain tags");
    });
    ui.label(
        RichText::new(match args.mode {
            NormalizeMode::Reencode => "Writes new files with the gain applied.",
            NormalizeMode::Replaygain => "Leaves the audio alone and tags it. No generation loss.",
        })
        .weak(),
    );

    // The target defaults differ per mode, so show the one in force and let it
    // be overridden rather than pre-filling a number that may be wrong.
    let mut target = args.target_lufs();
    ui.horizontal(|ui| {
        ui.label("Target");
        if ui.add(egui::DragValue::new(&mut target).speed(0.5).suffix(" LUFS")).changed() {
            args.target = Some(target);
        }
        if args.target.is_some() && ui.button("Reset").clicked() {
            args.target = None;
        }
    });

    if args.mode == NormalizeMode::Reencode {
        ui.horizontal(|ui| {
            ui.label("Ceiling");
            ui.add(egui::DragValue::new(&mut args.ceiling).speed(0.1).suffix(" dBTP"));
        });
        ui.horizontal(|ui| {
            ui.label("When peaks would exceed it");
            ui.selectable_value(&mut args.on_peak, PeakPolicy::Attenuate, "Turn down");
            ui.selectable_value(&mut args.on_peak, PeakPolicy::Limit, "Limit");
        });

        output_format(ui, &mut args.format, &mut args.bit_depth, &mut args.bitrate);

        ui.horizontal(|ui| {
            ui.label("Output folder");
            let mut shown = args
                .out_dir
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "beside each input".to_string());
            if ui.text_edit_singleline(&mut shown).changed() {
                args.out_dir = (!shown.trim().is_empty() && shown != "beside each input")
                    .then(|| PathBuf::from(shown.trim()));
            }
        });
        ui.horizontal(|ui| {
            ui.label("Name suffix");
            ui.text_edit_singleline(&mut args.suffix);
        });
        ui.checkbox(&mut args.force, "Overwrite existing output files");
    } else {
        ui.checkbox(&mut args.album, "Also compute album gain, grouping by folder");
    }

    ui.checkbox(&mut args.dry_run, "Dry run — measure and report, write nothing");
}

fn stems_options(ui: &mut egui::Ui, settings: &mut Settings) {
    use booth_cli::stems::{Backend, Stem};

    let args = &mut settings.stems;
    ui.heading("Stems");

    ui.horizontal(|ui| {
        ui.label("Separator");
        ui.selectable_value(&mut args.backend, Backend::Demucs, "Demucs");
        ui.selectable_value(&mut args.backend, Backend::Dsp, "Built-in");
    });
    ui.label(
        RichText::new(match args.backend {
            Backend::Demucs => {
                "A neural model. Much better, needs demucs installed, slow on a CPU."
            }
            Backend::Dsp => "No install needed, but voices leak into every stem.",
        })
        .weak(),
    );

    ui.horizontal(|ui| {
        ui.label("Write");
        for stem in Stem::ALL {
            let mut on = args.only.contains(&stem);
            if ui.checkbox(&mut on, stem.name()).changed() {
                if on {
                    args.only.push(stem);
                    args.only.sort();
                } else {
                    args.only.retain(|s| *s != stem);
                }
            }
        }
    });

    ui.horizontal(|ui| {
        ui.label("Output folder");
        let mut shown = args.out_dir.display().to_string();
        if ui.text_edit_singleline(&mut shown).changed() {
            args.out_dir = PathBuf::from(shown);
        }
    });

    // Stems get their own two-format choice rather than the general one: a wav
    // stem is enormous for audio that plays under something else.
    ui.horizontal(|ui| {
        ui.label("Format");
        for format in booth_cli::cli::StemFormat::ALL {
            ui.selectable_value(&mut args.format, format, format.name());
        }
    });
    match args.format {
        booth_cli::cli::StemFormat::Mp3 => {
            let mut vbr = !args.stem_cbr;
            ui.horizontal(|ui| {
                ui.checkbox(&mut vbr, "Variable bitrate");
                args.stem_cbr = !vbr;
                if vbr {
                    ui.add(
                        egui::Slider::new(&mut args.stem_vbr, 0..=9).text("quality (0 is best)"),
                    );
                } else {
                    ui.add(egui::Slider::new(&mut args.bitrate, 96..=320).suffix(" kbps"));
                }
            });
        }
        booth_cli::cli::StemFormat::Flac => {
            ui.horizontal(|ui| {
                ui.label("Bit depth");
                ui.add(egui::Slider::new(&mut args.bit_depth, 16..=24));
            });
        }
    }

    let mut tags = !args.no_tags;
    if ui.checkbox(&mut tags, "Copy tags from the original file").changed() {
        args.no_tags = !tags;
    }
    ui.checkbox(&mut args.force, "Overwrite existing stems");
}

fn tag_options(ui: &mut egui::Ui, settings: &mut Settings) {
    use booth_cli::cli::OnAmbiguous;
    use booth_cli::tag::OnExisting;

    let args = &mut settings.tag;
    ui.heading("Tag");
    ui.label(
        RichText::new(
            "Fingerprints each file locally, then asks AcoustID and MusicBrainz what it is. \
             This is the one thing here that uses the network.",
        )
        .weak(),
    );

    ui.horizontal(|ui| {
        ui.label("AcoustID key");
        let mut key = args.acoustid_key.clone().unwrap_or_default();
        if ui.add(egui::TextEdit::singleline(&mut key).password(true)).changed() {
            args.acoustid_key = (!key.trim().is_empty()).then(|| key.trim().to_string());
        }
    });
    ui.label(RichText::new("Free from acoustid.org/new-application.").weak());

    ui.horizontal(|ui| {
        ui.label("Minimum confidence");
        ui.add(egui::Slider::new(&mut args.min_score, 0.0..=1.0).fixed_decimals(2));
    });
    ui.horizontal(|ui| {
        ui.label("Below that");
        ui.selectable_value(&mut args.on_ambiguous, OnAmbiguous::Skip, "Leave the file alone");
        ui.selectable_value(&mut args.on_ambiguous, OnAmbiguous::Best, "Use the best guess");
    });
    ui.horizontal(|ui| {
        ui.label("Tags the file already has");
        ui.selectable_value(&mut args.on_existing, OnExisting::Keep, "Keep");
        ui.selectable_value(&mut args.on_existing, OnExisting::Overwrite, "Overwrite");
        ui.selectable_value(&mut args.on_existing, OnExisting::Report, "Just report");
    });

    ui.checkbox(&mut args.cover_art, "Fetch and embed cover art");
    ui.checkbox(&mut args.dry_run, "Dry run — look everything up, write nothing");
}

/// The format controls, which normalize and stems share.
fn output_format(
    ui: &mut egui::Ui,
    format: &mut Option<booth_cli::audio::encode::Codec>,
    bit_depth: &mut u16,
    bitrate: &mut u32,
) {
    use booth_cli::audio::encode::Codec;

    ui.horizontal(|ui| {
        ui.label("Format");
        ui.selectable_value(format, None, "Match input");
        ui.selectable_value(format, Some(Codec::Flac), "flac");
        ui.selectable_value(format, Some(Codec::Mp3), "mp3");
        ui.selectable_value(format, Some(Codec::Wav), "wav");
    });

    // Only one of these two applies to any given format, and showing the one
    // that does not invites setting a number that is quietly ignored.
    match format {
        Some(Codec::Mp3) => {
            ui.horizontal(|ui| {
                ui.label("Bitrate");
                ui.add(egui::DragValue::new(bitrate).range(64..=320).suffix(" kbps"));
            });
        }
        Some(Codec::Flac) | Some(Codec::Wav) => {
            ui.horizontal(|ui| {
                ui.label("Bit depth");
                ui.selectable_value(bit_depth, 16, "16");
                ui.selectable_value(bit_depth, 24, "24");
            });
        }
        // Matching the input means either could apply, so both are offered.
        None => {
            ui.horizontal(|ui| {
                ui.label("Bit depth (flac, wav)");
                ui.selectable_value(bit_depth, 16, "16");
                ui.selectable_value(bit_depth, 24, "24");
                ui.label("Bitrate (mp3)");
                ui.add(egui::DragValue::new(bitrate).range(64..=320).suffix(" kbps"));
            });
        }
    }
}
