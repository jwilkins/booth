//! A local audio toolkit: EBU R128 loudness normalization and stem separation
//! for mp3, flac and wav files.
//!
//! Everything runs on the machine it is invoked on. Nothing is uploaded, and
//! nothing is downloaded at run time.

pub mod analysis;
pub mod audio;
pub mod cli;
pub mod commands;
pub mod compat;
pub mod discover;
pub mod dsp;
pub mod export;
pub mod loudness;
pub mod normalize;
pub mod report;
pub mod stems;
pub mod tag;
