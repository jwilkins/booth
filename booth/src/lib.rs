//! Booth: a DJ library that prepares tracks and writes the drives a
//! Pioneer/AlphaTheta player reads.
//!
//! The window is one view — a query bar, a browser, the prep editor under it,
//! and a dock that always shows how far the drive has drifted from the
//! playlist. Every byte it writes goes through the same library the
//! command-line tool uses, and is read back off the drive by a parser that
//! shares no code with the writer before the run is called done.
//!
//! This is a library so that the parts that are not the window — the
//! collection, the query language, the sync plan — can be tested as what they
//! are, rather than through a window that would have to be opened to reach
//! them. The binary is [`app::App`] and a `main` that shows it.

pub mod app;
pub mod job;
pub mod library;
pub mod query;
pub mod rows;
pub mod sync;
pub mod theme;
pub mod wave;
