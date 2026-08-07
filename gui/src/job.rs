//! What the window asks for, and the thread that carries it out.
//!
//! Nothing here draws anything. The window owns a [`Runner`], hands it a
//! [`Settings`], and reads updates out of it; everything the job actually does
//! is the same library call the command-line tool makes, so the two front ends
//! cannot drift apart.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::thread::JoinHandle;

use musicai::cli::{AnalyzeArgs, NormalizeArgs, RunArgs, StemsArgs, Step, TagArgs};
use musicai::commands::Pipeline;
use musicai::report::{Event, Reporter};

/// The things this tool does: the whole pipeline, or any one part of it.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Task {
    Batch,
    Analyze,
    Normalize,
    Stems,
    Tag,
}

impl Task {
    pub const ALL: [Task; 5] =
        [Task::Batch, Task::Analyze, Task::Normalize, Task::Stems, Task::Tag];

    pub fn title(self) -> &'static str {
        match self {
            Task::Batch => "Batch",
            Task::Analyze => "Analyze",
            Task::Normalize => "Normalize",
            Task::Stems => "Stems",
            Task::Tag => "Tag",
        }
    }

    /// One line under the title, saying what the task is for.
    pub fn blurb(self) -> &'static str {
        match self {
            Task::Batch => "Normalize, tag and separate, in one pass over every file.",
            Task::Analyze => "Measure loudness and peaks. Changes nothing.",
            Task::Normalize => "Bring files to a consistent loudness.",
            Task::Stems => "Split each file into vocals, melody and drums.",
            Task::Tag => "Identify files by sound and write metadata tags.",
        }
    }

    /// The verb on the button that starts it.
    pub fn verb(self) -> &'static str {
        match self {
            Task::Batch => "Run all",
            Task::Analyze => "Analyze",
            Task::Normalize => "Normalize",
            Task::Stems => "Separate",
            Task::Tag => "Tag",
        }
    }

    fn subcommand(self) -> &'static str {
        match self {
            Task::Batch => "run",
            Task::Analyze => "analyze",
            Task::Normalize => "normalize",
            Task::Stems => "stems",
            Task::Tag => "tag",
        }
    }
}

/// Everything the window is currently editing.
///
/// The per-task options are the command-line tool's own argument structs,
/// seeded from the defaults the `run` pipeline would apply — which are in turn
/// each command's own clap defaults, with the handful the pipeline curates
/// (loudness mode, stems directory, separator) overriding them. So a default in
/// the window is by construction a default of the tool, the window and the
/// pipeline agree, and adding an option to the CLI without considering the
/// window is a compile error rather than a silent difference.
#[derive(Clone, Debug)]
pub struct Settings {
    pub task: Task,
    pub files: Vec<PathBuf>,
    /// Whether a chosen folder is searched all the way down. One flag rather
    /// than one per task: it describes the file list, which every task shares.
    pub recursive: bool,
    /// Which steps the batch runs.
    pub steps: Vec<Step>,
    pub analyze: AnalyzeArgs,
    pub normalize: NormalizeArgs,
    pub stems: StemsArgs,
    pub tag: TagArgs,
}

impl Default for Settings {
    fn default() -> Self {
        let run = RunArgs::defaults();
        let mut stems = run.stems_args();
        stems.out_dir = default_stems_dir(&stems.out_dir);

        Self {
            task: Task::Batch,
            files: Vec::new(),
            recursive: false,
            steps: run.steps.clone(),
            analyze: AnalyzeArgs::defaults(),
            normalize: run.normalize_args(),
            stems,
            tag: run.tag_args(),
        }
    }
}

/// Where stems go when nobody has said otherwise.
///
/// The command line's default is the relative `stems`, which means "under
/// wherever you are" — exactly right in a terminal. An app launched from the
/// Finder has no meaningful working directory (it is `/`, which is not
/// writable), so the same default would fail on every run. Anchoring it under
/// the home directory gives one predictable, writable, visible place instead.
fn default_stems_dir(relative: &Path) -> PathBuf {
    if relative.is_absolute() {
        return relative.to_path_buf();
    }
    match std::env::var_os("HOME") {
        Some(home) => Path::new(&home).join("Music").join("musicai-stems"),
        None => relative.to_path_buf(),
    }
}

impl Settings {
    /// Whether there is anything to run.
    pub fn ready(&self) -> bool {
        !self.files.is_empty()
    }

    /// Add paths, ignoring ones already listed.
    pub fn add_files(&mut self, paths: impl IntoIterator<Item = PathBuf>) {
        for path in paths {
            if !self.files.contains(&path) {
                self.files.push(path);
            }
        }
    }

    /// The command-line that would do the same thing, for the window to show.
    /// Seeing it makes the window scriptable: run it once, then copy the line.
    pub fn command_line(&self) -> String {
        let mut parts = vec!["musicai".to_string(), self.task.subcommand().to_string()];
        parts.extend(self.arguments());
        parts.push(match self.files.len() {
            0 => "<no files>".to_string(),
            1 => quote(&self.files[0].display().to_string()),
            n => format!("<{n} files>"),
        });
        parts.join(" ")
    }

    /// The flags that differ from the defaults, as they would be typed.
    fn arguments(&self) -> Vec<String> {
        let mut out = Vec::new();
        match self.task {
            Task::Batch => {
                flag(&mut out, "recursive", self.recursive);
                if self.steps.len() != Step::ALL.len() {
                    let names: Vec<&str> = self.steps.iter().map(|s| s.name()).collect();
                    out.push(format!("--steps {}", names.join(",")));
                }
                out.push(format!("--mode {}", value_name(self.normalize.mode)));
                out.push(format!("--backend {}", value_name(self.stems.backend)));
                out.push(format!(
                    "--stems-dir {}",
                    quote(&self.stems.out_dir.display().to_string())
                ));
                flag(&mut out, "cover-art", self.tag.cover_art);
                flag(&mut out, "dry-run", self.normalize.dry_run);
            }
            Task::Analyze => {
                flag(&mut out, "recursive", self.recursive);
                flag(&mut out, "json", self.analyze.json);
            }
            Task::Normalize => {
                let a = &self.normalize;
                flag(&mut out, "recursive", self.recursive);
                out.push(format!("--mode {}", value_name(a.mode)));
                out.push(format!("--target {}", a.target_lufs()));
                if a.mode == musicai::cli::NormalizeMode::Reencode {
                    out.push(format!("--ceiling {}", a.ceiling));
                    out.push(format!("--on-peak {}", value_name(a.on_peak)));
                }
                flag(&mut out, "album", a.album);
                flag(&mut out, "force", a.force);
                flag(&mut out, "dry-run", a.dry_run);
                if let Some(dir) = &a.out_dir {
                    out.push(format!("-o {}", quote(&dir.display().to_string())));
                }
                if let Some(codec) = a.format {
                    out.push(format!("--format {}", value_name(codec)));
                }
            }
            Task::Stems => {
                let a = &self.stems;
                flag(&mut out, "recursive", self.recursive);
                out.push(format!("--backend {}", value_name(a.backend)));
                out.push(format!("-o {}", quote(&a.out_dir.display().to_string())));
                if let Some(codec) = a.format {
                    out.push(format!("--format {}", value_name(codec)));
                }
                if a.only.len() != musicai::stems::Stem::ALL.len() {
                    let names: Vec<&str> = a.only.iter().map(|s| s.name()).collect();
                    out.push(format!("--only {}", names.join(",")));
                }
                flag(&mut out, "no-tags", a.no_tags);
                flag(&mut out, "force", a.force);
            }
            Task::Tag => {
                let a = &self.tag;
                flag(&mut out, "recursive", self.recursive);
                out.push(format!("--min-score {}", a.min_score));
                out.push(format!("--on-existing {}", value_name(a.on_existing)));
                out.push(format!("--on-ambiguous {}", value_name(a.on_ambiguous)));
                flag(&mut out, "cover-art", a.cover_art);
                flag(&mut out, "dry-run", a.dry_run);
            }
        }
        out
    }

    /// The arguments for one task, with the selected files and the shared
    /// recursion flag in place.
    fn with_inputs<T: WithInput + Clone>(&self, args: &T) -> T {
        let mut args = args.clone();
        *args.input_mut() =
            musicai::cli::InputArgs { inputs: self.files.clone(), recursive: self.recursive };
        args
    }

    /// Do what the window is set up to do. Runs on the worker thread.
    fn execute(&self, reporter: &dyn Reporter) -> anyhow::Result<()> {
        use musicai::commands;
        match self.task {
            Task::Batch => {
                // The pipeline is handed the full list up front, so it can say
                // how much there is to do and walk a folder once rather than
                // once per step.
                let files = musicai::discover::collect(&self.files, self.recursive)?;
                let normalize = self.with_inputs(&self.normalize);
                let tag = self.with_inputs(&self.tag);
                let stems = self.with_inputs(&self.stems);
                commands::run_pipeline(
                    &files,
                    &Pipeline {
                        steps: &self.steps,
                        normalize: &normalize,
                        tag: &tag,
                        stems: &stems,
                    },
                    reporter,
                )
            }
            Task::Analyze => commands::analyze(&self.with_inputs(&self.analyze), reporter),
            Task::Normalize => commands::normalize(&self.with_inputs(&self.normalize), reporter),
            Task::Stems => commands::stems(&self.with_inputs(&self.stems), reporter),
            Task::Tag => commands::tag(&self.with_inputs(&self.tag), reporter),
        }
    }
}

/// Lets the window put its file selection into any task's arguments without a
/// copy of the same lines per task.
trait WithInput {
    fn input_mut(&mut self) -> &mut musicai::cli::InputArgs;
}

macro_rules! with_input {
    ($type:ty) => {
        impl WithInput for $type {
            fn input_mut(&mut self) -> &mut musicai::cli::InputArgs {
                &mut self.input
            }
        }
    };
}

with_input!(AnalyzeArgs);
with_input!(NormalizeArgs);
with_input!(StemsArgs);
with_input!(TagArgs);

/// Add a bare `--flag`, but only when it is switched on.
fn flag(out: &mut Vec<String>, name: &str, on: bool) {
    if on {
        out.push(format!("--{name}"));
    }
}

/// How clap spells a value, so the shown command line is one that would parse.
fn value_name<T: clap::ValueEnum>(value: T) -> String {
    value.to_possible_value().map(|v| v.get_name().to_string()).unwrap_or_else(|| "?".to_string())
}

/// Quote a path for display only when it needs it.
fn quote(text: &str) -> String {
    if text.contains(' ') {
        format!("\"{text}\"")
    } else {
        text.to_string()
    }
}

// -- running ---------------------------------------------------------------

/// Something that happened in the worker thread.
#[derive(Clone, Debug)]
pub enum Update {
    /// Progress or output from the command.
    Event(Event),
    /// The command returned. `Err` carries the message to show.
    Done(Result<(), String>),
}

/// A reporter that forwards to the window and can be told to stop.
struct Channel {
    tx: Sender<Update>,
    cancel: Arc<AtomicBool>,
    wake: Arc<dyn Fn() + Send + Sync>,
}

impl Reporter for Channel {
    fn event(&self, event: Event) {
        // A dead receiver means the window has gone; the send failing is not
        // worth interrupting the job over, and cancellation handles shutdown.
        let _ = self.tx.send(Update::Event(event));
        (self.wake)();
    }

    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

/// Owns the worker thread and the channel back from it.
pub struct Runner {
    rx: Receiver<Update>,
    cancel: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl Runner {
    /// Start a job. The `wake` callback is called whenever there is something
    /// new to read, so the window can redraw without polling.
    pub fn start(settings: &Settings, wake: Arc<dyn Fn() + Send + Sync>) -> Self {
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        // A snapshot: editing the options while a job runs must not change what
        // that job is doing halfway through.
        let settings = settings.clone();
        let reporter = Channel { tx: tx.clone(), cancel: Arc::clone(&cancel), wake };

        let handle = std::thread::spawn(move || {
            let result = settings.execute(&reporter);
            let _ = tx.send(Update::Done(result.map_err(|e| format!("{e:#}"))));
            (reporter.wake)();
        });

        Self { rx, cancel, handle: Some(handle) }
    }

    /// Everything that has arrived since the last look.
    pub fn drain(&self) -> Vec<Update> {
        let mut updates = Vec::new();
        // A disconnected channel means the worker has finished and dropped its
        // end; whatever it managed to send is still worth collecting.
        while let Ok(update) = self.rx.try_recv() {
            updates.push(update);
        }
        updates
    }

    /// Ask the job to stop. It finishes the file it is on and then gives up, so
    /// this returns long before the thread does.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    /// Wait for the thread, for tests and for shutdown.
    pub fn join(&mut self) {
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for Runner {
    fn drop(&mut self) {
        // Closing the window while a separation is running should not leave a
        // thread writing files into a directory nobody is watching.
        self.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use musicai::cli::NormalizeMode;

    fn noop_wake() -> Arc<dyn Fn() + Send + Sync> {
        Arc::new(|| {})
    }

    /// A quiet wav file, which is enough for analyze and normalize to have
    /// something real to do.
    fn write_tone(path: &std::path::Path) {
        use musicai::audio::encode::{write_file, Codec, EncodeOptions};
        use musicai::audio::Audio;

        let sample_rate = 44_100;
        let samples: Vec<f32> = (0..sample_rate)
            .map(|i| 0.2 * (std::f32::consts::TAU * 440.0 * i as f32 / sample_rate as f32).sin())
            .collect();
        let audio = Audio::new(sample_rate as u32, vec![samples.clone(), samples]).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        write_file(path, &audio, Codec::Wav, &EncodeOptions::default()).unwrap();
    }

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("musicai-gui-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn defaults_come_from_clap_rather_than_being_restated() {
        let settings = Settings::default();
        // These are the CLI's documented defaults. If clap's defaults change,
        // the window changes with them.
        assert_eq!(settings.normalize.ceiling, -1.0);
        assert_eq!(settings.normalize.bitrate, 192);
        assert_eq!(settings.tag.min_score, 0.8);
        assert_eq!(settings.stems.only, musicai::stems::Stem::ALL.to_vec());
        // The placeholder input never survives into the settings the user edits.
        assert!(settings.files.is_empty());
    }

    #[test]
    fn the_window_opens_on_the_batch_with_the_pipelines_defaults() {
        let settings = Settings::default();
        let run = RunArgs::defaults();

        assert_eq!(settings.task, Task::Batch);
        assert_eq!(settings.steps, Step::ALL.to_vec());
        // The pipeline curates a few defaults away from the single commands' —
        // notably it tags loudness rather than re-encoding. The window has to
        // start from the pipeline's, because the pipeline is what it opens on.
        assert_eq!(settings.normalize.mode, run.mode);
        assert_eq!(settings.stems.backend, run.backend);
    }

    #[test]
    fn the_stems_directory_does_not_depend_on_the_working_directory() {
        // Launched from the Finder, the working directory is `/`, so the command
        // line's relative default would try to write to an unwritable root.
        let settings = Settings::default();
        assert!(
            settings.stems.out_dir.is_absolute(),
            "a window has no useful working directory: {}",
            settings.stems.out_dir.display()
        );
        assert!(settings.stems.out_dir.ends_with("musicai-stems"));
    }

    #[test]
    fn an_absolute_stems_directory_is_left_alone() {
        let chosen = PathBuf::from("/somewhere/else");
        assert_eq!(default_stems_dir(&chosen), chosen);
    }

    #[test]
    fn the_batch_command_line_names_the_steps_only_when_some_are_off() {
        let mut settings = Settings { task: Task::Batch, ..Default::default() };
        settings.add_files([PathBuf::from("/music/track.flac")]);

        let line = settings.command_line();
        assert!(line.starts_with("musicai run "), "{line}");
        assert!(!line.contains("--steps"), "all steps is the default: {line}");

        settings.steps.retain(|s| *s != Step::Stems);
        let line = settings.command_line();
        assert!(line.contains("--steps normalize,tag"), "{line}");
    }

    #[test]
    fn the_batch_runs_every_step_over_every_file() {
        let dir = Scratch::new("batch");
        let track = dir.0.join("track.wav");
        write_tone(&track);

        let mut settings = Settings { task: Task::Batch, ..Default::default() };
        settings.add_files([dir.0.clone()]);
        // Keep it offline and off demucs: tagging is skipped without a key, and
        // the built-in separator needs nothing installed.
        settings.stems.backend = musicai::stems::Backend::Dsp;
        settings.stems.out_dir = dir.0.join("stems");

        let mut runner = Runner::start(&settings, noop_wake());
        runner.join();

        let updates = runner.drain();
        let stages: Vec<(String, usize, usize)> = updates
            .iter()
            .filter_map(|u| match u {
                Update::Event(Event::Stage { name, index, of }) => {
                    Some((name.clone(), *index, *of))
                }
                _ => None,
            })
            .collect();

        assert_eq!(
            stages,
            vec![
                ("normalize".to_string(), 1, 3),
                ("tag".to_string(), 2, 3),
                ("stems".to_string(), 3, 3),
            ],
            "every step should announce itself, in order"
        );
        assert!(matches!(updates.last(), Some(Update::Done(Ok(())))), "{updates:?}");

        // The separation really ran: the stems are on disk.
        for stem in musicai::stems::Stem::ALL {
            let path = dir.0.join("stems").join(format!("track-{}.wav", stem.name()));
            assert!(path.exists(), "missing {}", path.display());
        }
    }

    #[test]
    fn turning_a_step_off_leaves_it_out_of_the_run() {
        let dir = Scratch::new("batch-subset");
        write_tone(&dir.0.join("track.wav"));

        let mut settings = Settings { task: Task::Batch, ..Default::default() };
        settings.add_files([dir.0.clone()]);
        settings.steps = vec![Step::Normalize];

        let mut runner = Runner::start(&settings, noop_wake());
        runner.join();

        let names: Vec<String> = runner
            .drain()
            .iter()
            .filter_map(|u| match u {
                Update::Event(Event::Stage { name, .. }) => Some(name.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(names, vec!["normalize".to_string()]);
        assert!(!dir.0.join("stems").exists(), "stems ran when it was switched off");
    }

    #[test]
    fn the_normalize_target_follows_the_mode() {
        let mut settings = Settings::default();
        settings.normalize.mode = NormalizeMode::Reencode;
        assert_eq!(settings.normalize.target_lufs(), -14.0);
        settings.normalize.mode = NormalizeMode::Replaygain;
        assert_eq!(settings.normalize.target_lufs(), -18.0);
    }

    #[test]
    fn adding_the_same_file_twice_adds_it_once() {
        let mut settings = Settings::default();
        settings.add_files([PathBuf::from("a.flac"), PathBuf::from("b.flac")]);
        settings.add_files([PathBuf::from("a.flac")]);
        assert_eq!(settings.files, vec![PathBuf::from("a.flac"), PathBuf::from("b.flac")]);
    }

    #[test]
    fn nothing_can_run_without_files() {
        let mut settings = Settings::default();
        assert!(!settings.ready());
        settings.add_files([PathBuf::from("a.flac")]);
        assert!(settings.ready());
    }

    #[test]
    fn the_shown_command_line_matches_the_settings() {
        let mut settings = Settings { task: Task::Stems, ..Default::default() };
        settings.add_files([PathBuf::from("/music/track.flac")]);
        settings.stems.only = vec![musicai::stems::Stem::Vocals];

        let line = settings.command_line();
        assert!(line.starts_with("musicai stems "), "{line}");
        assert!(line.contains("--only vocals"), "{line}");
        assert!(line.ends_with("/music/track.flac"), "{line}");
        // A full selection is the default, so it is not worth showing.
        settings.stems.only = musicai::stems::Stem::ALL.to_vec();
        assert!(!settings.command_line().contains("--only"));
    }

    #[test]
    fn a_path_with_spaces_is_quoted_in_the_shown_command() {
        let mut settings = Settings { task: Task::Analyze, ..Default::default() };
        settings.add_files([PathBuf::from("/music/two words.flac")]);
        assert!(settings.command_line().contains("\"/music/two words.flac\""));
    }

    #[test]
    fn a_job_reports_progress_and_finishes() {
        let dir = Scratch::new("runs");
        let track = dir.0.join("track.wav");
        write_tone(&track);

        let mut settings = Settings { task: Task::Analyze, ..Default::default() };
        settings.add_files([track]);

        let mut runner = Runner::start(&settings, noop_wake());
        runner.join();

        let updates = runner.drain();
        let events: Vec<&Event> = updates
            .iter()
            .filter_map(|u| match u {
                Update::Event(e) => Some(e),
                _ => None,
            })
            .collect();

        assert!(events.contains(&&Event::Started { total: 1 }));
        assert!(events.contains(&&Event::Progress { done: 1, total: 1 }));
        assert!(events
            .iter()
            .any(|e| matches!(e, Event::Finished { processed: 1, failed: 0, .. })));
        assert!(events.iter().any(|e| matches!(e, Event::Line(_))), "no measurement reported");
        assert!(matches!(updates.last(), Some(Update::Done(Ok(())))), "{updates:?}");
    }

    #[test]
    fn a_failing_file_is_reported_without_taking_the_window_down() {
        let dir = Scratch::new("bad");
        let track = dir.0.join("broken.wav");
        std::fs::write(&track, b"not a wav").unwrap();

        let mut settings = Settings { task: Task::Analyze, ..Default::default() };
        settings.add_files([track]);

        let mut runner = Runner::start(&settings, noop_wake());
        runner.join();

        let updates = runner.drain();
        assert!(
            updates.iter().any(|u| matches!(u, Update::Event(Event::Failed { .. }))),
            "{updates:?}"
        );
        // The job as a whole reports failure, rather than the thread dying.
        assert!(matches!(updates.last(), Some(Update::Done(Err(_)))), "{updates:?}");
    }

    #[test]
    fn cancelling_before_the_work_starts_leaves_the_files_alone() {
        let dir = Scratch::new("cancel");
        let track = dir.0.join("track.wav");
        write_tone(&track);
        let out = dir.0.join("out");

        let mut settings = Settings { task: Task::Normalize, ..Default::default() };
        // Re-encode explicitly: the batch default is ReplayGain, which a wav
        // cannot carry, and that would fail or not depending on whether the
        // worker beat the cancel flag to the file.
        settings.normalize.mode = musicai::cli::NormalizeMode::Reencode;
        settings.normalize.out_dir = Some(out.clone());
        settings.add_files([track]);

        let mut runner = Runner::start(&settings, noop_wake());
        runner.cancel();
        runner.join();

        // Whether the single file got in before the flag was set is a race, but
        // the job must always end tidily rather than half-way.
        assert!(runner.cancelled());
        let updates = runner.drain();
        assert!(matches!(updates.last(), Some(Update::Done(Ok(())))), "{updates:?}");
    }

    #[test]
    fn the_wake_callback_fires_so_the_window_redraws() {
        let dir = Scratch::new("wake");
        let track = dir.0.join("track.wav");
        write_tone(&track);

        let woken = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = Arc::clone(&woken);

        let mut settings = Settings { task: Task::Analyze, ..Default::default() };
        settings.add_files([track]);

        let mut runner = Runner::start(
            &settings,
            Arc::new(move || {
                counter.fetch_add(1, Ordering::Relaxed);
            }),
        );
        runner.join();

        assert!(woken.load(Ordering::Relaxed) > 0, "the window was never told to redraw");
    }
}
