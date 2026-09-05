//! Where a command's output goes, and how it is asked to stop.
//!
//! The commands used to print straight to stdout, which is exactly right for a
//! terminal and useless to anything else. A second front end would otherwise
//! have to reimplement the orchestration — planning outputs, grouping albums,
//! rate-limiting lookups — and would drift from the CLI the first time either
//! side was touched. Instead the commands report through this trait, and each
//! front end decides what to do with what they say.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;

/// Something a command has to say while it works.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// One step of a multi-step run is beginning. `index` counts from one.
    Stage { name: String, index: usize, of: usize },
    /// A batch is starting, with this many files to get through.
    Started { total: usize },
    /// A column heading for the lines that follow.
    Heading(String),
    /// One line of ordinary output, usually about one file.
    Line(String),
    /// A closing note about the batch as a whole. Kept apart from `Line`
    /// because it is commentary rather than a result, and the terminal sends it
    /// to stderr so that redirecting stdout still captures only the results.
    Summary(String),
    /// A file finished, successfully or not.
    Progress { done: usize, total: usize },
    /// How far through the file in hand the work has got, as a percentage.
    ///
    /// Separate from `Progress` because it is about one file rather than the
    /// batch, and only worth sending for a step long enough that finishing the
    /// file is not soon enough to hear about — stem separation is minutes a
    /// track, which without this is a spinner that never moves.
    Step { percent: u8 },
    /// A file that could not be processed. The batch carries on.
    Failed { path: PathBuf, message: String },
    /// The batch ended. `cancelled` means it stopped early on request.
    Finished { processed: usize, failed: usize, cancelled: bool },
}

/// Somewhere for a command's output to go.
///
/// Implementations are shared across worker threads, so this takes `&self` and
/// requires `Sync`.
pub trait Reporter: Send + Sync {
    fn event(&self, event: Event);

    /// Whether the caller has asked the batch to stop. Commands ask before
    /// starting each file, so a long batch gives up promptly without leaving a
    /// half-written file behind.
    fn cancelled(&self) -> bool {
        false
    }
}

/// Counts files as they finish and reports progress, so a batch running across
/// every core still advances the count one file at a time.
pub(crate) struct Progress<'a> {
    reporter: &'a dyn Reporter,
    done: AtomicUsize,
    total: usize,
}

impl<'a> Progress<'a> {
    pub(crate) fn new(reporter: &'a dyn Reporter, total: usize) -> Self {
        reporter.event(Event::Started { total });
        Self { reporter, done: AtomicUsize::new(0), total }
    }

    /// Record one finished file.
    pub(crate) fn tick(&self) {
        let done = self.done.fetch_add(1, Ordering::Relaxed) + 1;
        self.reporter.event(Event::Progress { done, total: self.total });
    }

    pub(crate) fn cancelled(&self) -> bool {
        self.reporter.cancelled()
    }
}

/// The terminal: lines to stdout, failures to stderr, progress ignored because
/// the lines themselves already show it arriving.
pub struct Stdio;

impl Reporter for Stdio {
    fn event(&self, event: Event) {
        match event {
            Event::Heading(text) | Event::Line(text) => println!("{text}"),
            Event::Summary(text) => eprintln!("{text}"),
            // A stage header is commentary about the run, not a result, so it
            // goes to stderr with the rest of the commentary.
            Event::Stage { name, index, of } => eprintln!("== {index}/{of} {name} =="),
            Event::Failed { path, message } => {
                eprintln!("error: {}: {message}", path.display())
            }
            Event::Finished { processed, failed, cancelled } if cancelled => {
                eprintln!("stopped after {} of {processed} files", processed - failed);
            }
            // A terminal already gets the child's own progress bar on its
            // stderr; printing a second one over it would fight with it.
            Event::Step { .. } => {}
            Event::Started { .. } | Event::Progress { .. } | Event::Finished { .. } => {}
        }
    }
}

/// Keeps every event, for tests and for anything that wants the output as data.
#[derive(Default)]
pub struct Collected {
    events: Mutex<Vec<Event>>,
    cancel: AtomicBool,
}

impl Collected {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn events(&self) -> Vec<Event> {
        self.events.lock().unwrap().clone()
    }

    /// Just the ordinary output lines, in order.
    pub fn lines(&self) -> Vec<String> {
        self.events()
            .into_iter()
            .filter_map(|e| match e {
                Event::Line(text) => Some(text),
                _ => None,
            })
            .collect()
    }

    /// Ask the batch to stop.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl Reporter for Collected {
    fn event(&self, event: Event) {
        self.events.lock().unwrap().push(event);
    }

    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

/// Report a file that failed. A batch keeps going after one bad file, so this
/// is a report rather than a return.
pub(crate) fn failed(reporter: &dyn Reporter, path: &Path, error: &anyhow::Error) {
    reporter.event(Event::Failed { path: path.to_path_buf(), message: format!("{error:#}") });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collected_keeps_lines_in_order() {
        let reporter = Collected::new();
        reporter.event(Event::Line("first".into()));
        reporter.event(Event::Progress { done: 1, total: 2 });
        reporter.event(Event::Line("second".into()));

        assert_eq!(reporter.lines(), vec!["first", "second"]);
        assert_eq!(reporter.events().len(), 3);
    }

    #[test]
    fn cancellation_is_visible_to_the_command() {
        let reporter = Collected::new();
        assert!(!reporter.cancelled());
        reporter.cancel();
        assert!(reporter.cancelled());
    }

    #[test]
    fn progress_counts_one_file_at_a_time() {
        let reporter = Collected::new();
        let progress = Progress::new(&reporter, 3);
        progress.tick();
        progress.tick();

        assert_eq!(
            reporter.events(),
            vec![
                Event::Started { total: 3 },
                Event::Progress { done: 1, total: 3 },
                Event::Progress { done: 2, total: 3 },
            ]
        );
    }

    #[test]
    fn progress_counts_correctly_from_many_threads() {
        // The count comes from files finishing in parallel, so it has to be
        // right regardless of who gets there first.
        let reporter = Collected::new();
        let progress = Progress::new(&reporter, 100);
        std::thread::scope(|scope| {
            for _ in 0..4 {
                scope.spawn(|| {
                    for _ in 0..25 {
                        progress.tick();
                    }
                });
            }
        });

        let mut counts: Vec<usize> = reporter
            .events()
            .into_iter()
            .filter_map(|e| match e {
                Event::Progress { done, .. } => Some(done),
                _ => None,
            })
            .collect();
        counts.sort_unstable();
        assert_eq!(counts, (1..=100).collect::<Vec<_>>());
    }

    #[test]
    fn a_plain_reporter_never_asks_to_stop() {
        struct Quiet;
        impl Reporter for Quiet {
            fn event(&self, _: Event) {}
        }
        assert!(!Quiet.cancelled());
    }
}
