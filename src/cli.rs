//! Command-line surface.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

use crate::audio::encode::Codec;
use crate::normalize::PeakPolicy;
use crate::stems::install::InstallPolicy;
use crate::stems::{Backend, Stem};
use crate::tag::OnExisting;

#[derive(Parser, Debug)]
#[command(
    name = "musicai",
    version,
    about = "Loudness-normalize and stem-separate mp3, flac and wav files, locally",
    max_term_width = 100
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,

    /// Worker threads. Defaults to one per core.
    #[arg(long, short = 'j', global = true, value_name = "N")]
    pub jobs: Option<usize>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Report loudness and peak levels without changing anything.
    Analyze(AnalyzeArgs),
    /// Bring files to a consistent loudness.
    Normalize(NormalizeArgs),
    /// Split files into vocals, melody and drums.
    Stems(StemsArgs),
    /// Identify files by sound and write metadata tags from MusicBrainz.
    Tag(TagArgs),
}

/// What to do with a file whose best match is below the confidence threshold.
#[derive(Copy, Clone, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum OnAmbiguous {
    /// Leave the file alone and report it as unmatched.
    Skip,
    /// Tag it with the best match anyway.
    Best,
}

#[derive(Args, Debug)]
pub struct TagArgs {
    #[command(flatten)]
    pub input: InputArgs,

    /// AcoustID API key. Free from https://acoustid.org/new-application.
    /// MusicBrainz itself needs no key.
    #[arg(long, value_name = "KEY", env = "ACOUSTID_API_KEY", hide_env_values = true)]
    pub acoustid_key: Option<String>,

    /// What to do with tags the file already has.
    #[arg(long, value_enum, default_value_t = OnExisting::Keep)]
    pub on_existing: OnExisting,

    /// Minimum AcoustID confidence, from 0 to 1, for a match to be trusted.
    #[arg(long, value_name = "SCORE", default_value_t = 0.8)]
    pub min_score: f64,

    /// What to do when no match reaches --min-score.
    #[arg(long, value_enum, default_value_t = OnAmbiguous::Skip)]
    pub on_ambiguous: OnAmbiguous,

    /// Also fetch front cover art from the Cover Art Archive and embed it.
    #[arg(long)]
    pub cover_art: bool,

    /// Largest cover image to accept, in bytes.
    #[arg(long, value_name = "BYTES", default_value_t = crate::tag::coverart::DEFAULT_MAX_BYTES)]
    pub max_cover_bytes: usize,

    /// Print each file's fingerprint and duration, then stop. Needs no API key
    /// and makes no network requests.
    #[arg(long)]
    pub print_fingerprint: bool,

    /// Look everything up and report what would change, but write nothing.
    #[arg(long, short = 'n')]
    pub dry_run: bool,

    /// Milliseconds between MusicBrainz requests. Their terms ask for at least
    /// 1000; going below that will get you rate limited and then blocked.
    #[arg(long, value_name = "MS", default_value_t = 1_100)]
    pub musicbrainz_interval: u64,
}

#[derive(Args, Debug)]
pub struct InputArgs {
    /// Files or directories to process.
    #[arg(value_name = "PATH", required = true)]
    pub inputs: Vec<PathBuf>,

    /// Descend into subdirectories.
    #[arg(long, short = 'r')]
    pub recursive: bool,
}

#[derive(Args, Debug)]
pub struct AnalyzeArgs {
    #[command(flatten)]
    pub input: InputArgs,

    /// Emit one JSON object per file instead of a table.
    #[arg(long)]
    pub json: bool,
}

/// Whether normalization rewrites the audio or only tags it.
#[derive(Copy, Clone, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum NormalizeMode {
    /// Decode, apply gain, and write a new file.
    Reencode,
    /// Leave the audio untouched and write ReplayGain 2.0 tags into the
    /// existing file.
    Replaygain,
}

#[derive(Args, Debug)]
pub struct NormalizeArgs {
    #[command(flatten)]
    pub input: InputArgs,

    /// How to apply the correction.
    #[arg(long, value_enum, default_value_t = NormalizeMode::Reencode)]
    pub mode: NormalizeMode,

    /// Target loudness in LUFS. Defaults to -14 when re-encoding (the usual
    /// streaming level) and -18 for ReplayGain (the level its spec defines).
    #[arg(long, value_name = "LUFS", allow_negative_numbers = true)]
    pub target: Option<f64>,

    /// Maximum permitted true peak, in dBTP. Re-encode mode only.
    #[arg(long, value_name = "DBTP", default_value_t = -1.0, allow_negative_numbers = true)]
    pub ceiling: f64,

    /// What to do when the loudness target would push peaks past the ceiling.
    #[arg(long, value_enum, default_value_t = PeakPolicy::Attenuate)]
    pub on_peak: PeakPolicy,

    /// Limiter look-ahead in milliseconds, for `--on-peak limit`.
    #[arg(long, value_name = "MS", default_value_t = crate::normalize::limiter::DEFAULT_LOOKAHEAD_MS)]
    pub lookahead: f64,

    /// Output format. Defaults to matching each input.
    #[arg(long, value_enum, value_name = "FORMAT")]
    pub format: Option<Codec>,

    /// Write outputs into this directory instead of beside the inputs.
    #[arg(long, short = 'o', value_name = "DIR")]
    pub out_dir: Option<PathBuf>,

    /// Suffix added to output file names.
    #[arg(long, default_value = "-normalized")]
    pub suffix: String,

    /// Bit depth for wav and flac output.
    #[arg(long, value_name = "BITS", default_value_t = 16)]
    pub bit_depth: u16,

    /// Bitrate in kbps for mp3 output.
    #[arg(long, value_name = "KBPS", default_value_t = 192)]
    pub bitrate: u32,

    /// Skip the dither normally added when writing 16-bit output.
    #[arg(long)]
    pub no_dither: bool,

    /// Also compute album gain, grouping files by the directory they sit in.
    /// ReplayGain mode only.
    #[arg(long)]
    pub album: bool,

    /// Overwrite output files that already exist.
    #[arg(long)]
    pub force: bool,

    /// Measure and report, but write nothing.
    #[arg(long, short = 'n')]
    pub dry_run: bool,
}

impl NormalizeArgs {
    /// Resolve the target, which depends on the mode when not given.
    pub fn target_lufs(&self) -> f64 {
        self.target.unwrap_or(match self.mode {
            NormalizeMode::Reencode => crate::normalize::STREAMING_TARGET_LUFS,
            NormalizeMode::Replaygain => crate::normalize::REPLAYGAIN_REFERENCE_LUFS,
        })
    }
}

#[derive(Args, Debug)]
pub struct StemsArgs {
    #[command(flatten)]
    pub input: InputArgs,

    /// Which separator to use.
    #[arg(long, value_enum, default_value_t = Backend::Demucs)]
    pub backend: Backend,

    /// Directory to write stems into, as <dir>/<track>/<stem>.<ext>.
    #[arg(long, short = 'o', value_name = "DIR", default_value = "stems")]
    pub out_dir: PathBuf,

    /// Output format for the stems.
    #[arg(long, value_enum, default_value_t = Codec::Wav)]
    pub format: Codec,

    /// Only write these stems.
    #[arg(long, value_enum, value_delimiter = ',', default_values_t = Stem::ALL)]
    pub only: Vec<Stem>,

    /// Bit depth for wav and flac output.
    #[arg(long, value_name = "BITS", default_value_t = 16)]
    pub bit_depth: u16,

    /// Bitrate in kbps for mp3 output.
    #[arg(long, value_name = "KBPS", default_value_t = 192)]
    pub bitrate: u32,

    /// Overwrite stems that already exist.
    #[arg(long)]
    pub force: bool,

    #[command(flatten)]
    pub dsp: DspArgs,

    #[command(flatten)]
    pub demucs: DemucsArgs,
}

/// Tuning for the built-in separator. All of it has sensible defaults; these
/// exist for when a particular track fights them.
#[derive(Args, Debug)]
#[command(next_help_heading = "Built-in separator tuning")]
pub struct DspArgs {
    /// FFT size for the drum pass. Smaller sharpens transients, but too small
    /// stops resolving chords and leaks them into the drum stem.
    #[arg(long, value_name = "N", default_value_t = 4096)]
    pub drum_fft: usize,

    /// FFT size for the vocal pass. Larger separates pitch more finely.
    #[arg(long, value_name = "N", default_value_t = 8192)]
    pub voice_fft: usize,

    /// Drum pass: how long a sound must last to count as pitched.
    #[arg(long, value_name = "MS", default_value_t = 400.0)]
    pub drum_time: f64,

    /// Drum pass: how wide a sound must be to count as a transient.
    #[arg(long, value_name = "HZ", default_value_t = 400.0)]
    pub drum_bandwidth: f64,

    /// Vocal pass: how long a sound must last to count as steady.
    #[arg(long, value_name = "MS", default_value_t = 200.0)]
    pub voice_time: f64,

    /// Vocal pass: how far a sound must smear to count as a voice. Raising
    /// this pushes the lead back into the melody stem.
    #[arg(long, value_name = "HZ", default_value_t = 30.0)]
    pub voice_bandwidth: f64,

    /// Mask sharpness. 1 is gentle, 2 is the usual choice, higher is harder.
    #[arg(long, value_name = "P", default_value_t = 2.0)]
    pub mask_power: f32,

    /// How strongly stereo centring steers the vocal mask, 0 to 1.
    #[arg(long, value_name = "W", default_value_t = 0.5)]
    pub center_weight: f32,

    /// STFT overlap factor, 2 or 4. Use 2 to halve memory use on long files.
    #[arg(long, value_name = "N", default_value_t = 4)]
    pub overlap: usize,
}

impl From<&DspArgs> for crate::stems::dsp::Config {
    fn from(args: &DspArgs) -> Self {
        Self {
            drum_fft: args.drum_fft,
            voice_fft: args.voice_fft,
            drum_time_ms: args.drum_time,
            drum_freq_hz: args.drum_bandwidth,
            voice_time_ms: args.voice_time,
            voice_freq_hz: args.voice_bandwidth,
            mask_power: args.mask_power,
            center_weight: args.center_weight,
            overlap: args.overlap,
        }
    }
}

#[derive(Args, Debug)]
#[command(next_help_heading = "Demucs backend")]
pub struct DemucsArgs {
    /// The demucs executable to run.
    #[arg(long, value_name = "PATH", default_value = "demucs")]
    pub demucs_bin: PathBuf,

    /// Pretrained model name to pass to demucs.
    #[arg(long, value_name = "NAME", default_value = "htdemucs")]
    pub demucs_model: String,

    /// Torch device for demucs, e.g. cpu or cuda.
    #[arg(long, value_name = "DEVICE")]
    pub demucs_device: Option<String>,

    /// What to do when demucs is not installed. `ask` offers to install it,
    /// but only on macOS and only when there is a terminal to answer on.
    #[arg(long, value_enum, default_value_t = InstallPolicy::Ask)]
    pub install_demucs: InstallPolicy,
}
