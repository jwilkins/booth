//! What happened, in order, with times.
//!
//! There is one of these for the whole program rather than a handle threaded
//! through everything, because the things worth logging are spread across the
//! window, the worker threads and the rayon pool, and a logger that is awkward
//! to reach is a logger nobody calls.
//!
//! Two sinks: a ring buffer the window shows, and a file that survives the
//! window closing — which is the one that matters when something goes wrong
//! and the answer is "it crashed, I did not see".
//!
//! Nothing here may be called from the audio callback. It takes a lock and
//! allocates, and doing either on a real-time thread is how playback starts
//! clicking.

use std::collections::VecDeque;
use std::fmt;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

/// How much detail. Ordered, so a threshold is a comparison.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Debug,
    Info,
    Warn,
    Error,
    /// Nothing at all. Only ever a threshold, never an entry's own level.
    Off,
}

impl Level {
    pub const SHOWN: [Level; 4] = [Level::Debug, Level::Info, Level::Warn, Level::Error];

    pub fn label(self) -> &'static str {
        match self {
            Level::Debug => "debug",
            Level::Info => "info",
            Level::Warn => "warn",
            Level::Error => "error",
            Level::Off => "off",
        }
    }

    /// Four characters, so the column lines up in a monospaced log.
    pub fn tag(self) -> &'static str {
        match self {
            Level::Debug => "dbg ",
            Level::Info => "info",
            Level::Warn => "warn",
            Level::Error => "err ",
            Level::Off => "off ",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        Some(match text.trim().to_ascii_lowercase().as_str() {
            "debug" | "trace" | "all" => Level::Debug,
            "info" => Level::Info,
            "warn" | "warning" => Level::Warn,
            "error" | "err" => Level::Error,
            "off" | "none" | "quiet" => Level::Off,
            _ => return None,
        })
    }

    fn code(self) -> u8 {
        self as u8
    }

    fn from_code(code: u8) -> Self {
        match code {
            0 => Level::Debug,
            1 => Level::Info,
            2 => Level::Warn,
            3 => Level::Error,
            _ => Level::Off,
        }
    }
}

impl fmt::Display for Level {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// One line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// Milliseconds since the program started. Not a wall clock: what a run's
    /// log is read for is what happened relative to everything else in it, and
    /// elapsed time needs no timezone to be unambiguous.
    pub at_ms: u64,
    pub level: Level,
    pub text: String,
}

impl Entry {
    /// `  12.345  info  text`, the shape the file and the window both use.
    pub fn line(&self) -> String {
        format!("{} {} {}", self.stamp(), self.level.tag(), self.text)
    }

    pub fn stamp(&self) -> String {
        format!("{:>7}.{:03}", self.at_ms / 1000, self.at_ms % 1000)
    }
}

/// How many lines the window keeps. The file keeps all of them.
const KEPT: usize = 5_000;

struct Sink {
    entries: VecDeque<Entry>,
    file: Option<std::fs::File>,
    /// Bumped on every write, so the window can tell whether to scroll without
    /// comparing the whole buffer.
    revision: u64,
}

static SINK: OnceLock<Mutex<Sink>> = OnceLock::new();
static THRESHOLD: AtomicU8 = AtomicU8::new(0);
static START: OnceLock<Instant> = OnceLock::new();

fn sink() -> &'static Mutex<Sink> {
    SINK.get_or_init(|| Mutex::new(Sink { entries: VecDeque::new(), file: None, revision: 0 }))
}

/// Start logging, to the ring buffer and optionally to a file.
///
/// The threshold comes from `BOOTH_LOG` when it is set and understood, so a run
/// can be made noisier without a rebuild. Default is everything: this is a young
/// program, and the log is most of what there is to go on.
pub fn start(file: Option<&Path>) {
    let _ = START.set(Instant::now());
    let level = std::env::var("BOOTH_LOG").ok().and_then(|text| Level::parse(&text));
    THRESHOLD.store(level.unwrap_or(Level::Debug).code(), Ordering::Relaxed);

    if let Some(path) = file {
        // The previous run is kept alongside, because the run worth reading is
        // usually the one that just ended badly.
        if path.exists() {
            let _ = std::fs::rename(path, path.with_extension("log.1"));
        }
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(handle) = std::fs::File::create(path) {
            if let Ok(mut sink) = sink().lock() {
                sink.file = Some(handle);
            }
        }
    }
}

/// The file a run writes to.
pub fn default_path() -> PathBuf {
    crate::library::data_dir().join("booth.log")
}

pub fn threshold() -> Level {
    Level::from_code(THRESHOLD.load(Ordering::Relaxed))
}

pub fn set_threshold(level: Level) {
    THRESHOLD.store(level.code(), Ordering::Relaxed);
}

/// Whether a message at this level would be kept. Checked before formatting, so
/// a filtered-out debug line costs nothing but the comparison.
pub fn enabled(level: Level) -> bool {
    level >= threshold() && threshold() != Level::Off
}

/// Write one line. Prefer the macros.
pub fn record(level: Level, text: String) {
    if !enabled(level) {
        return;
    }
    let at_ms = START.get().map(|start| start.elapsed().as_millis() as u64).unwrap_or(0);
    let entry = Entry { at_ms, level, text };

    let Ok(mut sink) = sink().lock() else { return };
    if let Some(file) = &mut sink.file {
        // A failed write is not worth a second error about the first: the ring
        // buffer still has it, and the window is still running.
        let _ = writeln!(file, "{}", entry.line());
        let _ = file.flush();
    }
    sink.entries.push_back(entry);
    while sink.entries.len() > KEPT {
        sink.entries.pop_front();
    }
    sink.revision += 1;
}

/// Everything kept, oldest first, at or above `level`.
pub fn entries(level: Level) -> Vec<Entry> {
    let Ok(sink) = sink().lock() else { return Vec::new() };
    sink.entries.iter().filter(|entry| entry.level >= level).cloned().collect()
}

/// How many writes there have been, for spotting new ones cheaply.
pub fn revision() -> u64 {
    sink().lock().map(|sink| sink.revision).unwrap_or(0)
}

/// Forget everything in the window's buffer. The file keeps its copy.
pub fn clear() {
    if let Ok(mut sink) = sink().lock() {
        sink.entries.clear();
    }
}

#[macro_export]
macro_rules! debug {
    ($($arg:tt)*) => {
        if $crate::log::enabled($crate::log::Level::Debug) {
            $crate::log::record($crate::log::Level::Debug, format!($($arg)*));
        }
    };
}

#[macro_export]
macro_rules! info {
    ($($arg:tt)*) => {
        if $crate::log::enabled($crate::log::Level::Info) {
            $crate::log::record($crate::log::Level::Info, format!($($arg)*));
        }
    };
}

#[macro_export]
macro_rules! warn {
    ($($arg:tt)*) => {
        if $crate::log::enabled($crate::log::Level::Warn) {
            $crate::log::record($crate::log::Level::Warn, format!($($arg)*));
        }
    };
}

#[macro_export]
macro_rules! error {
    ($($arg:tt)*) => {
        if $crate::log::enabled($crate::log::Level::Error) {
            $crate::log::record($crate::log::Level::Error, format!($($arg)*));
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The sink is one global, so the tests that use it take turns.
    fn guard() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: Mutex<()> = Mutex::new(());
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn levels_are_ordered_from_chatty_to_serious() {
        assert!(Level::Debug < Level::Info);
        assert!(Level::Info < Level::Warn);
        assert!(Level::Warn < Level::Error);
        assert!(Level::Error < Level::Off);
    }

    #[test]
    fn a_threshold_keeps_what_is_at_least_as_serious() {
        let _guard = guard();
        set_threshold(Level::Warn);
        assert!(!enabled(Level::Debug));
        assert!(!enabled(Level::Info));
        assert!(enabled(Level::Warn));
        assert!(enabled(Level::Error));

        // Off means off, including for errors.
        set_threshold(Level::Off);
        assert!(!enabled(Level::Error));
        set_threshold(Level::Debug);
    }

    #[test]
    fn the_environment_names_are_the_ones_a_person_would_type() {
        assert_eq!(Level::parse("debug"), Some(Level::Debug));
        assert_eq!(Level::parse("  WARN "), Some(Level::Warn));
        assert_eq!(Level::parse("warning"), Some(Level::Warn));
        assert_eq!(Level::parse("off"), Some(Level::Off));
        assert_eq!(Level::parse("loud"), None);
    }

    #[test]
    fn entries_come_back_in_order_and_filtered() {
        let _guard = guard();
        clear();
        set_threshold(Level::Debug);

        record(Level::Debug, "first".into());
        record(Level::Error, "second".into());
        record(Level::Info, "third".into());

        let all: Vec<String> = entries(Level::Debug).iter().map(|e| e.text.clone()).collect();
        assert_eq!(all, vec!["first", "second", "third"], "oldest first");

        let loud: Vec<String> = entries(Level::Warn).iter().map(|e| e.text.clone()).collect();
        assert_eq!(loud, vec!["second"]);
        clear();
    }

    #[test]
    fn what_the_threshold_rejects_never_reaches_the_buffer() {
        let _guard = guard();
        clear();
        set_threshold(Level::Warn);
        record(Level::Debug, "noise".into());
        record(Level::Error, "kept".into());
        assert_eq!(entries(Level::Debug).len(), 1);

        set_threshold(Level::Debug);
        clear();
    }

    #[test]
    fn the_buffer_does_not_grow_without_bound() {
        let _guard = guard();
        clear();
        set_threshold(Level::Debug);
        for i in 0..KEPT + 100 {
            record(Level::Debug, format!("line {i}"));
        }
        let kept = entries(Level::Debug);
        assert_eq!(kept.len(), KEPT);
        // The oldest went, not the newest: a log that drops what just happened
        // is a log that is never any use.
        assert_eq!(kept.last().unwrap().text, format!("line {}", KEPT + 99));
        clear();
    }

    #[test]
    fn a_line_reads_as_a_time_a_level_and_a_message() {
        let entry = Entry { at_ms: 12_345, level: Level::Info, text: "started".into() };
        assert_eq!(entry.line(), "     12.345 info started");
        let entry = Entry { at_ms: 7, level: Level::Error, text: "no".into() };
        assert_eq!(entry.line(), "      0.007 err  no");
    }

    #[test]
    fn the_previous_run_is_kept_when_a_new_one_starts() {
        let dir = std::env::temp_dir().join(format!("booth-log-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("booth.log");
        std::fs::write(&path, b"the run before").unwrap();

        let _guard = guard();
        start(Some(&path));

        // The run that just ended is the one worth reading after a crash.
        let previous = path.with_extension("log.1");
        assert!(previous.exists(), "the previous run's log was thrown away");
        assert_eq!(std::fs::read(&previous).unwrap(), b"the run before");

        record(Level::Info, "this run".into());
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(written.contains("this run"), "{written}");

        // Detach the file so later tests do not write into a deleted directory.
        if let Ok(mut sink) = sink().lock() {
            sink.file = None;
        }
        std::fs::remove_dir_all(&dir).unwrap();
        clear();
    }
}
