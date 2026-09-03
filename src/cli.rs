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
    about = "Loudness-normalize, tag and stem-separate mp3, flac and wav files, locally",
    // With no subcommand the arguments below are the `run` pipeline's, so
    // `musicai ~/Music` does the lot. With one, they belong to it instead.
    args_conflicts_with_subcommands = true,
    subcommand_negates_reqs = true,
    max_term_width = 100
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,

    /// The pipeline's arguments, used when no subcommand is given.
    ///
    /// Not an `Option`: a required argument inside an optional group is a
    /// contradiction clap resolves by never filling the group in, which is
    /// exactly the bug it looks like. `subcommand_negates_reqs` is what makes
    /// the paths optional when a subcommand supplies its own.
    #[command(flatten)]
    pub run: RunArgs,

    /// Worker threads. Defaults to one per core.
    #[arg(long, short = 'j', global = true, value_name = "N")]
    pub jobs: Option<usize>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Report loudness and peak levels without changing anything.
    Analyze(AnalyzeArgs),
    /// Write the per-track analysis files a Pioneer/AlphaTheta player reads.
    Anlz(AnlzArgs),
    /// Build a whole USB drive a Pioneer/AlphaTheta player can browse.
    Export(ExportArgs),
    /// Bring files to a consistent loudness.
    Normalize(NormalizeArgs),
    /// Normalize, tag and separate, in one pass. This is what running
    /// `musicai` with no subcommand does.
    Run(RunArgs),
    /// Split files into vocals, melody and drums.
    Stems(StemsArgs),
    /// Identify files by sound and write metadata tags from MusicBrainz.
    Tag(TagArgs),
    /// Read rekordbox's own encrypted libraries.
    #[command(subcommand)]
    Rekordbox(RekordboxCommand),
}

#[derive(Subcommand, Debug)]
pub enum RekordboxCommand {
    /// List what is in a rekordbox library, without changing anything.
    ///
    /// Point it at `master.db` from a rekordbox installation, or at a mounted
    /// OneLibrary drive.
    Read(RekordboxArgs),
    /// Describe the tables of a database, for a format its vendor has not
    /// published.
    ///
    /// This is how the OneLibrary schema in `docs/onelibrary.md` gets checked
    /// against a drive. It reads; it does not write one.
    Schema(RekordboxArgs),
}

#[derive(clap::Args, Debug)]
pub struct RekordboxArgs {
    /// The `master.db`, the `exportLibrary.db`, or the drive holding one.
    pub path: PathBuf,

    /// The SQLCipher key. Defaults to `REKORDBOX_KEY`, then to whatever this
    /// build was compiled with.
    #[arg(long, value_name = "HEX")]
    pub key: Option<String>,
}

/// One step of the pipeline.
#[derive(Copy, Clone, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Step {
    Normalize,
    Tag,
    Stems,
}

impl Step {
    /// In the order they run. Tagging comes before separation, so the stems
    /// inherit the tags that were just written rather than whatever was there
    /// before; normalizing comes first so that everything downstream sees the
    /// finished audio.
    pub const ALL: [Step; 3] = [Step::Normalize, Step::Tag, Step::Stems];

    pub fn name(self) -> &'static str {
        match self {
            Step::Normalize => "normalize",
            Step::Tag => "tag",
            Step::Stems => "stems",
        }
    }
}

#[derive(Args, Clone, Debug)]
pub struct RunArgs {
    #[command(flatten)]
    pub input: InputArgs,

    /// Which steps to run. They always run in the order normalize, tag, stems,
    /// whatever order they are listed in.
    #[arg(long, value_enum, value_delimiter = ',', default_values_t = Step::ALL)]
    pub steps: Vec<Step>,

    /// How to normalize. The default writes ReplayGain tags rather than new
    /// files, so the pipeline keeps working on one set of files instead of
    /// leaving a second copy of the library behind. With `reencode`, the later
    /// steps follow the newly written files.
    #[arg(long, value_enum, default_value_t = NormalizeMode::Replaygain)]
    pub mode: NormalizeMode,

    /// Target loudness in LUFS. Defaults to -14 when re-encoding and -18 for
    /// ReplayGain.
    #[arg(long, value_name = "LUFS", allow_negative_numbers = true)]
    pub target: Option<f64>,

    /// Also compute album gain, grouping files by the directory they sit in.
    #[arg(long)]
    pub album: bool,

    /// AcoustID API key. Without one the tag step is skipped, because every
    /// lookup would fail.
    #[arg(long, value_name = "KEY", env = "ACOUSTID_API_KEY", hide_env_values = true)]
    pub acoustid_key: Option<String>,

    /// Minimum AcoustID confidence, from 0 to 1, for a match to be trusted.
    #[arg(long, value_name = "SCORE", default_value_t = 0.8)]
    pub min_score: f64,

    /// Also fetch front cover art and embed it.
    #[arg(long)]
    pub cover_art: bool,

    /// Which separator to use.
    #[arg(long, value_enum, default_value_t = Backend::Demucs)]
    pub backend: Backend,

    /// Directory to write stems into.
    #[arg(long, value_name = "DIR", default_value = "stems")]
    pub stems_dir: PathBuf,

    /// What to do when demucs is not installed.
    #[arg(long, value_enum, default_value_t = InstallPolicy::Ask)]
    pub install_demucs: InstallPolicy,

    /// Overwrite files that already exist.
    #[arg(long)]
    pub force: bool,

    /// Report what every step would do, and write nothing.
    #[arg(long, short = 'n')]
    pub dry_run: bool,
}

impl RunArgs {
    /// The `normalize` arguments this run implies, starting from the defaults
    /// the command-line tool would apply.
    pub fn normalize_args(&self) -> NormalizeArgs {
        NormalizeArgs {
            input: self.input.clone(),
            mode: self.mode,
            target: self.target,
            album: self.album,
            force: self.force,
            dry_run: self.dry_run,
            ..NormalizeArgs::defaults()
        }
    }

    pub fn tag_args(&self) -> TagArgs {
        TagArgs {
            input: self.input.clone(),
            acoustid_key: self.acoustid_key.clone(),
            min_score: self.min_score,
            cover_art: self.cover_art,
            dry_run: self.dry_run,
            ..TagArgs::defaults()
        }
    }

    pub fn stems_args(&self) -> StemsArgs {
        let mut args = StemsArgs {
            input: self.input.clone(),
            backend: self.backend,
            out_dir: self.stems_dir.clone(),
            force: self.force,
            ..StemsArgs::defaults()
        };
        args.demucs.install_demucs = self.install_demucs;
        args
    }
}

/// Give an arguments struct a `defaults()` constructor, filled in by clap.
///
/// Defaults are then stated once, in the `#[arg]` attributes, and everything
/// else — the pipeline, the window — asks for them rather than restating them
/// and slowly drifting out of step.
macro_rules! defaults_from_clap {
    ($type:ident, $subcommand:literal, $variant:ident) => {
        impl $type {
            pub fn defaults() -> Self {
                // Every subcommand needs at least one input path; callers
                // replace this placeholder before anything runs.
                match Cli::parse_from(["musicai", $subcommand, "<none>"]).command {
                    Some(Command::$variant(args)) => args,
                    other => unreachable!("clap parsed {other:?} for {}", $subcommand),
                }
            }
        }
    };
}

defaults_from_clap!(AnalyzeArgs, "analyze", Analyze);
defaults_from_clap!(NormalizeArgs, "normalize", Normalize);
defaults_from_clap!(RunArgs, "run", Run);
defaults_from_clap!(StemsArgs, "stems", Stems);
defaults_from_clap!(TagArgs, "tag", Tag);

impl StemsArgs {
    /// The VBR quality to write mp3 stems at, or `None` for a constant bitrate.
    pub fn vbr(&self) -> Option<u8> {
        (!self.stem_cbr).then_some(self.stem_vbr)
    }

    /// The model to separate with: whatever was named, else what the chosen
    /// quality implies.
    pub fn model(&self) -> String {
        self.demucs.demucs_model.clone().unwrap_or_else(|| self.quality.model().to_string())
    }

    /// Likewise for the number of test-time shifts.
    pub fn shifts(&self) -> u32 {
        self.demucs.demucs_shifts.unwrap_or_else(|| self.quality.shifts())
    }
}

impl ExportArgs {
    /// The same trick as the macro, with one extra step.
    ///
    /// An export has to be told where it is going, and clap enforces that by
    /// requiring one of `--drive` and `--image`. So the defaults are parsed with
    /// a placeholder drive, which is then cleared again: there is no default
    /// destination, and a caller that has not chosen one has not chosen one.
    pub fn defaults() -> Self {
        match Cli::parse_from(["musicai", "export", "--drive", "<none>", "<none>"]).command {
            Some(Command::Export(mut args)) => {
                args.drive = None;
                args
            }
            other => unreachable!("clap parsed {other:?} for export"),
        }
    }
}

/// What to do with a file whose best match is below the confidence threshold.
#[derive(Copy, Clone, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum OnAmbiguous {
    /// Leave the file alone and report it as unmatched.
    Skip,
    /// Tag it with the best match anyway.
    Best,
}

#[derive(Args, Clone, Debug)]
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

#[derive(Args, Clone, Debug)]
pub struct InputArgs {
    /// Files or directories to process.
    #[arg(value_name = "PATH", required = true)]
    pub inputs: Vec<PathBuf>,

    /// Descend into subdirectories.
    #[arg(long, short = 'r')]
    pub recursive: bool,
}

#[derive(Args, Clone, Debug)]
pub struct AnalyzeArgs {
    #[command(flatten)]
    pub input: InputArgs,

    /// Emit one JSON object per file instead of a table.
    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Clone, Debug)]
pub struct AnlzArgs {
    #[command(flatten)]
    pub input: InputArgs,

    /// Where the files go. Defaults to alongside each input.
    #[arg(long, short = 'o', value_name = "DIR")]
    pub output: Option<PathBuf>,

    /// Override the detected tempo, in BPM.
    ///
    /// The beats are still tracked against the audio; this only says how far
    /// apart they are, which is what a detector gets wrong when it hears a
    /// track at half or double speed.
    #[arg(long, value_name = "BPM")]
    pub bpm: Option<f64>,

    /// The path at which the player will find the audio, if it is not going to
    /// be `/Contents/<filename>`.
    #[arg(long, value_name = "PATH")]
    pub on_drive_path: Option<String>,
}

#[derive(Args, Clone, Debug)]
pub struct ExportArgs {
    #[command(flatten)]
    pub input: InputArgs,

    /// The drive to write, e.g. `/Volumes/USB`. Created if it does not exist.
    #[arg(
        long,
        short = 'o',
        value_name = "DIR",
        conflicts_with = "image",
        required_unless_present = "image"
    )]
    pub drive: Option<PathBuf>,

    /// Write a disk image instead of a folder.
    ///
    /// A partitioned, FAT32-formatted `.img` — the thing a player actually
    /// reads. It can go straight onto a stick with `dd`, or into the USB slot
    /// of a CDJ-3000 emulator, which is the closest thing to a player that
    /// does not involve a player.
    #[arg(long, value_name = "FILE", conflicts_with = "drive", required_unless_present = "drive")]
    pub image: Option<PathBuf>,

    /// The volume label an image is formatted with.
    #[arg(long, value_name = "NAME", default_value = "REKORDBOX")]
    pub label: String,

    /// Override the detected tempo, in BPM, for every file named.
    ///
    /// The beats are still tracked against the audio; this only says how far
    /// apart they are.
    #[arg(long, value_name = "BPM")]
    pub bpm: Option<f64>,

    /// Name of the playlist the exported tracks go into.
    ///
    /// Used when `playlists` is empty, which is the command line's case: one
    /// list of everything named, because there is nothing on a command line
    /// that says which file belongs to which playlist.
    #[arg(long, value_name = "NAME", default_value = "musicai")]
    pub playlist: String,

    /// The playlist tree to write, when the caller has one.
    ///
    /// Not a command-line option, for the same reason `companions` is not: it
    /// comes from a library that knows which track is in which list, and there
    /// is no way to say it on one line.
    #[arg(skip)]
    pub playlists: Vec<PlaylistSpec>,

    /// Report what would be written without touching the drive.
    #[arg(long)]
    pub dry_run: bool,

    /// The key for the OneLibrary database the newer players read.
    ///
    /// Different from the one `rekordbox read` wants: that opens rekordbox's
    /// own library, this encrypts a drive. Without it the export writes the
    /// legacy database only, which every player up to and including the
    /// CDJ-3000 reads and the CDJ-3000X does not. Defaults to
    /// `ONELIBRARY_KEY`.
    #[arg(long, value_name = "KEY")]
    pub onelibrary_key: Option<String>,

    /// Files that are stems of another track, as (stem, parent) pairs.
    ///
    /// A stem is the same audio with parts removed, so it has the same tempo,
    /// the same downbeats and the same structure — and a hot cue set on the
    /// track has to land in the same place on its acapella, or the two cannot
    /// be played against each other. So a companion takes the parent's grid,
    /// cues, key and phrases rather than being analysed on its own, where a
    /// vocal with no drums in it would produce a grid of its own and a
    /// different one.
    ///
    /// Not a command-line option: it comes from a library that knows which file
    /// came from which, and there is no way to say it on one line.
    #[arg(skip)]
    pub companions: Vec<(PathBuf, PathBuf)>,
    /// Rows already on the drive that this run is not rewriting, each with the
    /// file it was made from.
    ///
    /// A drive is written once and then added to. Preparing a track means
    /// decoding it, so a second write is given only what changed — and a
    /// database built from only that describes a drive that no longer exists,
    /// with everything written before it gone from the player's browse. These
    /// carry through unchanged, so the database always describes the whole
    /// drive rather than the last thing done to it.
    ///
    /// Empty for a command-line export, where the files given are the drive.
    #[arg(skip)]
    pub already: Vec<(PathBuf, crate::export::pdb::Track)>,
}

/// One playlist to write onto a drive, named by the files that belong to it.
///
/// Paths rather than ids: the ids a drive uses are handed out during the
/// export, as each file is prepared, so a caller has none to give. It does
/// know which files it asked for, and that is enough to match on afterwards.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PlaylistSpec {
    pub name: String,
    /// The folder it sits in on the player, or empty for the top level.
    pub folder: String,
    /// The tracks, in play order.
    pub tracks: Vec<PathBuf>,
}

/// How much work a separation is worth.
///
/// The two ends of a real trade: the fine-tuned model with test-time shifts is
/// roughly eight times the work of demucs' own defaults and separates
/// noticeably better. A stem is rendered once and then played for years, so the
/// default is the slow one — but a first pass over a whole library, or a laptop
/// with somewhere else to be, wants the other.
#[derive(Copy, Clone, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum StemQuality {
    /// `htdemucs_ft` with two shifts. Cleaner, and about eight times slower.
    High,
    /// `htdemucs` with no shifts — demucs' own defaults.
    Standard,
}

impl StemQuality {
    pub const ALL: [StemQuality; 2] = [StemQuality::High, StemQuality::Standard];

    pub fn model(self) -> &'static str {
        match self {
            StemQuality::High => "htdemucs_ft",
            StemQuality::Standard => "htdemucs",
        }
    }

    pub fn shifts(self) -> u32 {
        match self {
            StemQuality::High => 2,
            StemQuality::Standard => 0,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            StemQuality::High => "high",
            StemQuality::Standard => "standard",
        }
    }

    pub fn blurb(self) -> &'static str {
        match self {
            StemQuality::High => {
                "htdemucs_ft with two shifts — cleaner, and roughly eight times slower"
            }
            StemQuality::Standard => "htdemucs, no shifts — demucs' own defaults",
        }
    }
}

/// What a stem kit is written as.
///
/// Deliberately narrower than [`Codec`]: a wav stem is enormous for no benefit,
/// and "the same as the input" would give a wav track wav stems.
#[derive(Copy, Clone, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum StemFormat {
    /// Variable-bitrate by default. Roughly a fifth of the size of flac.
    Mp3,
    /// Lossless, for a kit that will be worked on further.
    Flac,
}

impl StemFormat {
    pub const ALL: [StemFormat; 2] = [StemFormat::Mp3, StemFormat::Flac];

    pub fn codec(self) -> Codec {
        match self {
            StemFormat::Mp3 => Codec::Mp3,
            StemFormat::Flac => Codec::Flac,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            StemFormat::Mp3 => "mp3",
            StemFormat::Flac => "flac",
        }
    }
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

#[derive(Args, Clone, Debug)]
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

    /// Bit depth for wav and flac output. Stems default to 24-bit: they are
    /// already-processed audio, and quantising them to 16-bit throws away
    /// headroom for no benefit at their file sizes.
    #[arg(long, value_name = "BITS", default_value_t = 24)]
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

#[derive(Args, Clone, Debug)]
pub struct StemsArgs {
    #[command(flatten)]
    pub input: InputArgs,

    /// Which separator to use.
    #[arg(long, value_enum, default_value_t = Backend::Demucs)]
    pub backend: Backend,

    /// How much work to spend separating. See [`StemQuality`].
    #[arg(long, value_enum, default_value_t = StemQuality::High)]
    pub quality: StemQuality,

    /// Directory to write stems into, as <dir>/<track>-<stem>.<ext>.
    #[arg(long, short = 'o', value_name = "DIR", default_value = "stems")]
    pub out_dir: PathBuf,

    /// Output format for the stems.
    ///
    /// mp3 or flac, and mp3 by default: a stem kit is three more files per
    /// track, and a library of lossless ones is four times the disk for audio
    /// that is played under something else.
    #[arg(long, value_enum, value_name = "FORMAT", default_value_t = StemFormat::Mp3)]
    pub format: StemFormat,

    /// Variable-bitrate quality for mp3 stems, on LAME's `-V` scale: 0 is
    /// biggest and best, 9 smallest and worst. 2 is the usual "high quality"
    /// setting, averaging around 190 kbps.
    ///
    /// Pass `--stem-cbr` for a constant bitrate instead.
    #[arg(long, value_name = "0-9", default_value_t = 2)]
    pub stem_vbr: u8,

    /// Write stems at a constant bitrate — `--bitrate` — rather than VBR.
    #[arg(long)]
    pub stem_cbr: bool,

    /// Do not copy the source file's tags onto its stems.
    #[arg(long)]
    pub no_tags: bool,

    /// Only write these stems.
    #[arg(long, value_enum, value_delimiter = ',', default_values_t = Stem::ALL)]
    pub only: Vec<Stem>,

    /// Bit depth for wav and flac output. Stems default to 24-bit: they are
    /// already-processed audio, and quantising them to 16-bit throws away
    /// headroom for no benefit at their file sizes.
    #[arg(long, value_name = "BITS", default_value_t = 24)]
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
#[derive(Args, Clone, Debug)]
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

#[derive(Args, Clone, Debug)]
#[command(next_help_heading = "Demucs backend")]
pub struct DemucsArgs {
    /// The demucs executable to run.
    #[arg(long, value_name = "PATH", default_value = "demucs")]
    pub demucs_bin: PathBuf,

    /// Pretrained model name to pass to demucs.
    ///
    /// Overrides whatever `--quality` would have chosen. Left unset, `high`
    /// gives `htdemucs_ft` and `standard` gives `htdemucs`.
    #[arg(long, value_name = "NAME")]
    pub demucs_model: Option<String>,

    /// Torch device for demucs, e.g. cpu or cuda.
    #[arg(long, value_name = "DEVICE")]
    pub demucs_device: Option<String>,

    /// Test-time shifts: demucs separates the track this many extra times at
    /// small random offsets and averages the results, smoothing artefacts at a
    /// roughly linear cost in time.
    ///
    /// Overrides `--quality`. Left unset, `high` gives 2 and `standard` gives 0.
    #[arg(long, value_name = "N")]
    pub demucs_shifts: Option<u32>,

    /// Window overlap for demucs, 0.0 to just under 1.0. More overlap means
    /// fewer seams between windows and more compute. Demucs' default is 0.25.
    #[arg(long, value_name = "FRACTION", default_value_t = 0.25)]
    pub demucs_overlap: f32,

    /// What to do when demucs is not installed. `ask` offers to install it,
    /// but only on macOS and only when there is a terminal to answer on.
    #[arg(long, value_enum, default_value_t = InstallPolicy::Ask)]
    pub install_demucs: InstallPolicy,
}
