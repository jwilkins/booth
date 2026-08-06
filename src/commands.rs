//! Implementations of the three subcommands.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use rayon::prelude::*;

use crate::audio::decode::decode_file;
use crate::audio::encode::{write_file, Codec, EncodeOptions};
use crate::cli::{AnalyzeArgs, NormalizeArgs, NormalizeMode, OnAmbiguous, StemsArgs, TagArgs};
use crate::discover;
use crate::loudness::{self, Loudness};
use crate::normalize::replaygain::{write_tags, ReplayGain};
use crate::normalize::{self, Settings};
use crate::stems::{demucs, dsp, install, Backend, StemSet};
use crate::tag::{acoustid, coverart, fingerprint, musicbrainz, Metadata, TagOutcome};

/// Outcome of a batch: how many files worked, and the failures.
pub struct Outcome {
    pub processed: usize,
    pub failures: Vec<(PathBuf, anyhow::Error)>,
}

impl Outcome {
    fn report(self) -> Result<()> {
        for (path, error) in &self.failures {
            eprintln!("error: {}: {error:#}", path.display());
        }
        if self.failures.is_empty() {
            Ok(())
        } else {
            bail!("{} of {} files failed", self.failures.len(), self.processed);
        }
    }
}

/// Split per-file results into printable lines and errors, keeping input order.
fn partition(paths: &[PathBuf], results: Vec<Result<Vec<String>>>) -> (Vec<String>, Outcome) {
    let mut lines = Vec::new();
    let mut failures = Vec::new();
    for (path, result) in paths.iter().zip(results) {
        match result {
            Ok(mut produced) => lines.append(&mut produced),
            Err(e) => failures.push((path.clone(), e)),
        }
    }
    (lines, Outcome { processed: paths.len(), failures })
}

// -- analyze ---------------------------------------------------------------

pub fn analyze(args: &AnalyzeArgs) -> Result<()> {
    let files = discover::collect(&args.input.inputs, args.input.recursive)?;

    let results: Vec<Result<Vec<String>>> = files
        .par_iter()
        .map(|path| {
            let audio = decode_file(path)?;
            let measured = loudness::measure(&audio)?;
            let line = if args.json {
                json_line(path, &measured, audio.duration_secs(), audio.channels())
            } else {
                table_row(path, &measured, audio.duration_secs())
            };
            Ok(vec![line])
        })
        .collect();

    let (lines, outcome) = partition(&files, results);

    if !args.json && !lines.is_empty() {
        println!(
            "{:<40} {:>8} {:>10} {:>8} {:>11}",
            "file", "length", "loudness", "range", "true peak"
        );
    }
    for line in lines {
        println!("{line}");
    }

    outcome.report()
}

fn table_row(path: &Path, measured: &Loudness, duration: f64) -> String {
    format!(
        "{:<40} {:>8} {:>10} {:>8} {:>11}",
        elide(&path.display().to_string(), 40),
        format_duration(duration),
        format!("{} LUFS", format_db(measured.integrated_lufs)),
        format!("{:.1} LU", measured.range_lu),
        format!("{} dBTP", format_db(measured.true_peak_db())),
    )
}

fn json_line(path: &Path, measured: &Loudness, duration: f64, channels: usize) -> String {
    format!(
        r#"{{"file":{},"duration_seconds":{:.3},"channels":{},"integrated_lufs":{},"loudness_range_lu":{:.2},"true_peak_dbtp":{},"sample_peak_dbfs":{}}}"#,
        json_string(&path.display().to_string()),
        duration,
        channels,
        json_number(measured.integrated_lufs),
        measured.range_lu,
        json_number(measured.true_peak_db()),
        json_number(measured.sample_peak_db()),
    )
}

// -- normalize -------------------------------------------------------------

pub fn normalize(args: &NormalizeArgs) -> Result<()> {
    let files = discover::collect(&args.input.inputs, args.input.recursive)?;
    match args.mode {
        NormalizeMode::Reencode => normalize_reencode(args, &files),
        NormalizeMode::Replaygain => normalize_replaygain(args, &files),
    }
}

fn normalize_reencode(args: &NormalizeArgs, files: &[PathBuf]) -> Result<()> {
    let plans = plan_outputs(args, files)?;

    let settings = Settings {
        target_lufs: args.target_lufs(),
        ceiling_dbtp: args.ceiling,
        peak_policy: args.on_peak,
        lookahead_ms: args.lookahead,
    };
    let encode = EncodeOptions {
        bit_depth: args.bit_depth,
        mp3_bitrate: args.bitrate,
        dither: !args.no_dither,
    };

    let results: Vec<Result<Vec<String>>> = plans
        .par_iter()
        .map(|plan| {
            let mut audio = decode_file(&plan.input)?;
            let report = normalize::apply(&mut audio, &settings)?;

            if report.silent {
                return Ok(vec![format!("{}: silent, skipped", plan.input.display())]);
            }

            let mut line = format!(
                "{}: {} LUFS -> {} LUFS ({:+.2} dB)",
                plan.input.display(),
                format_db(report.before.integrated_lufs),
                format_db(report.after.integrated_lufs),
                report.gain_db,
            );
            if report.withheld_db > 0.01 {
                line.push_str(&format!(
                    ", held back {:.2} dB to stay under {:.1} dBTP",
                    report.withheld_db, args.ceiling
                ));
            }
            if report.limiter_reduction_db < -0.01 {
                line.push_str(&format!(
                    ", limiter took off up to {:.2} dB",
                    -report.limiter_reduction_db
                ));
            }

            if args.dry_run {
                line.push_str(" [dry run]");
            } else {
                let report = write_file(&plan.output, &audio, plan.codec, &encode)?;
                line.push_str(&format!(" -> {}", plan.output.display()));
                if report.clipped_anything() {
                    line.push_str(&format!(" (warning: {} samples clipped)", report.clipped));
                }
            }
            Ok(vec![line])
        })
        .collect();

    let inputs: Vec<PathBuf> = plans.iter().map(|p| p.input.clone()).collect();
    let (lines, outcome) = partition(&inputs, results);
    for line in lines {
        println!("{line}");
    }
    outcome.report()
}

fn normalize_replaygain(args: &NormalizeArgs, files: &[PathBuf]) -> Result<()> {
    let reference = args.target_lufs();

    // Album gain is defined over a whole album, so group by directory. Without
    // --album each file is its own group and the album tags are omitted.
    let mut groups: BTreeMap<PathBuf, Vec<PathBuf>> = BTreeMap::new();
    for file in files {
        let key = if args.album {
            file.parent().unwrap_or(Path::new("")).to_path_buf()
        } else {
            file.clone()
        };
        groups.entry(key).or_default().push(file.clone());
    }

    let group_keys: Vec<PathBuf> = groups.keys().cloned().collect();
    let results: Vec<Result<Vec<String>>> = group_keys
        .par_iter()
        .map(|key| {
            let members = &groups[key];

            // Meter every track, keeping the meters so their gating histories
            // can be pooled for the album figure. The decoded audio itself is
            // dropped as soon as it has been measured.
            let mut meters = Vec::with_capacity(members.len());
            let mut measurements = Vec::with_capacity(members.len());
            for path in members {
                let audio = decode_file(path)?;
                let meter = loudness::meter_for(&audio)?;
                measurements.push(loudness::summarize(&meter)?);
                meters.push(meter);
            }

            let album = if args.album {
                let lufs = loudness::album_loudness(&meters)?;
                let peak = measurements.iter().fold(0.0f64, |acc, m| acc.max(m.sample_peak));
                Some((lufs, peak))
            } else {
                None
            };

            let mut lines = Vec::with_capacity(members.len());
            for (path, measured) in members.iter().zip(&measurements) {
                let mut rg = ReplayGain::for_track(measured, reference);
                if let Some((album_lufs, album_peak)) = album {
                    rg = rg.with_album(album_lufs, album_peak);
                }

                let mut line = format!(
                    "{}: {} LUFS, track gain {:+.2} dB",
                    path.display(),
                    format_db(measured.integrated_lufs),
                    rg.track_gain_db,
                );
                if let Some(gain) = rg.album_gain_db {
                    line.push_str(&format!(", album gain {gain:+.2} dB"));
                }

                if args.dry_run {
                    line.push_str(" [dry run]");
                } else {
                    write_tags(path, &rg)?;
                    line.push_str(", tagged");
                }
                lines.push(line);
            }
            Ok(lines)
        })
        .collect();

    let (mut lines, outcome) = partition(&group_keys, results);
    lines.sort();
    for line in lines {
        println!("{line}");
    }
    outcome.report()
}

/// An input paired with where its normalized copy goes.
struct Plan {
    input: PathBuf,
    output: PathBuf,
    codec: Codec,
}

/// Work out every output path up front, so collisions and accidental
/// overwrites are caught before any file is written.
fn plan_outputs(args: &NormalizeArgs, files: &[PathBuf]) -> Result<Vec<Plan>> {
    let mut plans = Vec::with_capacity(files.len());
    let mut seen: BTreeMap<PathBuf, PathBuf> = BTreeMap::new();

    for input in files {
        let codec = match args.format {
            Some(codec) => codec,
            None => Codec::from_path(input).with_context(|| {
                format!(
                    "cannot tell what format {} is, so cannot match it; pass --format",
                    input.display()
                )
            })?,
        };

        let stem =
            input.file_stem().with_context(|| format!("{} has no file name", input.display()))?;
        let name = format!("{}{}.{}", stem.to_string_lossy(), args.suffix, codec.extension());
        let dir = match &args.out_dir {
            Some(dir) => dir.clone(),
            None => input.parent().unwrap_or(Path::new(".")).to_path_buf(),
        };
        let output = dir.join(name);

        if output == *input {
            bail!(
                "output for {} would overwrite the input; set --suffix or --out-dir",
                input.display()
            );
        }
        if let Some(other) = seen.get(&output) {
            bail!(
                "{} and {} would both be written to {}",
                other.display(),
                input.display(),
                output.display()
            );
        }
        if output.exists() && !args.force && !args.dry_run {
            bail!("{} already exists; pass --force to overwrite", output.display());
        }

        seen.insert(output.clone(), input.clone());
        plans.push(Plan { input: input.clone(), output, codec });
    }

    Ok(plans)
}

// -- stems -----------------------------------------------------------------

pub fn stems(args: &StemsArgs) -> Result<()> {
    let files = discover::collect(&args.input.inputs, args.input.recursive)?;
    let config = dsp::Config::from(&args.dsp);

    // Resolve demucs once, before any work starts. Doing it per file would ask
    // the same question repeatedly, and finding out that it is missing after
    // separating half a library would be worse still.
    let demucs_bin = match args.backend {
        Backend::Demucs => Some(install::ensure_available(
            args.demucs.demucs_bin.as_os_str(),
            args.demucs.install_demucs,
        )?),
        Backend::Dsp => None,
    };

    let encode =
        EncodeOptions { bit_depth: args.bit_depth, mp3_bitrate: args.bitrate, dither: true };

    let mut failures = Vec::new();

    // Separation is memory-hungry and already uses every core internally, so
    // files go one at a time. Each one prints as it finishes rather than at the
    // end, because a long batch would otherwise look like it had hung.
    for path in &files {
        match separate_one(path, args, &config, &encode, demucs_bin.as_deref()) {
            Ok(written) => {
                for line in written {
                    println!("{line}");
                }
            }
            Err(e) => failures.push((path.clone(), e)),
        }
    }

    Outcome { processed: files.len(), failures }.report()
}

fn separate_one(
    path: &Path,
    args: &StemsArgs,
    config: &dsp::Config,
    encode: &EncodeOptions,
    demucs_bin: Option<&std::ffi::OsStr>,
) -> Result<Vec<String>> {
    let track = path
        .file_stem()
        .with_context(|| format!("{} has no file name", path.display()))?
        .to_string_lossy()
        .into_owned();
    let dir = args.out_dir.join(&track);

    // Check the destinations before doing the expensive part.
    for stem in &args.only {
        let out = dir.join(format!("{}.{}", stem.name(), args.format.extension()));
        if out.exists() && !args.force {
            bail!("{} already exists; pass --force to overwrite", out.display());
        }
    }

    let separated = match args.backend {
        Backend::Dsp => {
            let audio = decode_file(path)?;
            dsp::separate(&audio, config)?
        }
        Backend::Demucs => {
            let work_dir = args.out_dir.join(".demucs-work");
            let mut demucs_config = demucs::Config::new(work_dir.clone());
            // Already resolved, and possibly to somewhere not on PATH.
            demucs_config.program = demucs_bin
                .map(|p| p.to_os_string())
                .unwrap_or_else(|| args.demucs.demucs_bin.clone().into_os_string());
            demucs_config.model = args.demucs.demucs_model.clone();
            demucs_config.device = args.demucs.demucs_device.clone();

            let result = demucs::separate(path, &demucs_config);
            // Demucs' own output is an intermediate; the stems we write are the
            // deliverable. Clean up whether or not it succeeded.
            let _ = std::fs::remove_dir_all(&work_dir);
            result?
        }
    };

    write_stems(&dir, &separated, args, encode)
}

fn write_stems(
    dir: &Path,
    separated: &StemSet,
    args: &StemsArgs,
    encode: &EncodeOptions,
) -> Result<Vec<String>> {
    let mut written = Vec::new();
    for stem in &args.only {
        let out = dir.join(format!("{}.{}", stem.name(), args.format.extension()));
        let report = write_file(&out, separated.get(*stem), args.format, encode)?;

        let mut line = format!("wrote {}", out.display());
        if report.clipped_anything() {
            // A stem can peak higher than the mix it came from, so this
            // happens on loud masters even though the input never clipped.
            line.push_str(&format!(
                " — warning: {} samples clipped; the stem peaks above full scale",
                report.clipped
            ));
        }
        written.push(line);
    }
    Ok(written)
}

// -- tag -------------------------------------------------------------------

pub fn tag(args: &TagArgs) -> Result<()> {
    let files = discover::collect(&args.input.inputs, args.input.recursive)?;

    // Fingerprinting is local and CPU-bound, so it runs across every core
    // before any network work starts.
    let fingerprints: Vec<Result<fingerprint::Fingerprint>> = files
        .par_iter()
        .map(|path| {
            let audio = decode_file(path)?;
            fingerprint::fingerprint(&audio)
        })
        .collect();

    if args.print_fingerprint {
        let results = fingerprints
            .into_iter()
            .map(|r| r.map(|fp| vec![format!("{} {}", fp.duration_secs, fp.compressed)]))
            .collect();
        let (lines, outcome) = partition(&files, results);
        for (path, line) in files.iter().zip(lines) {
            println!("{}\t{line}", path.display());
        }
        return outcome.report();
    }

    let key = args.acoustid_key.clone().unwrap_or_default();
    let mut lookup = Lookup::new(key, args)?;

    let mut failures = Vec::new();
    let mut matched = 0usize;
    let mut unmatched = 0usize;

    // The network phase is sequential: MusicBrainz allows roughly one request
    // per second, so there is nothing to gain from parallelism and a real risk
    // of being blocked for ignoring the limit.
    for (path, fingerprint) in files.iter().zip(fingerprints) {
        let result = fingerprint.and_then(|fp| tag_one(path, &fp, args, &mut lookup));
        match result {
            Ok(Some(line)) => {
                matched += 1;
                println!("{line}");
            }
            Ok(None) => {
                unmatched += 1;
                println!("{}: no confident match", path.display());
            }
            Err(e) => failures.push((path.clone(), e)),
        }
    }

    eprintln!("{matched} tagged, {unmatched} unmatched, {} failed", failures.len());
    Outcome { processed: files.len(), failures }.report()
}

/// The three services, plus the caches that keep repeated lookups off the
/// network when a whole album is being tagged at once.
struct Lookup {
    acoustid: acoustid::Client,
    musicbrainz: musicbrainz::Client,
    cover: Option<coverart::Client>,
    recordings: BTreeMap<String, musicbrainz::Recording>,
    covers: BTreeMap<String, Option<coverart::CoverArt>>,
}

impl Lookup {
    fn new(key: String, args: &TagArgs) -> Result<Self> {
        Ok(Self {
            acoustid: acoustid::Client::new(key, acoustid::DEFAULT_MIN_INTERVAL),
            musicbrainz: musicbrainz::Client::new(std::time::Duration::from_millis(
                args.musicbrainz_interval,
            )),
            cover: args.cover_art.then(|| {
                coverart::Client::new(coverart::DEFAULT_MIN_INTERVAL, args.max_cover_bytes)
            }),
            recordings: BTreeMap::new(),
            covers: BTreeMap::new(),
        })
    }

    fn recording(&mut self, mbid: &str) -> Result<musicbrainz::Recording> {
        if let Some(cached) = self.recordings.get(mbid) {
            return Ok(cached.clone());
        }
        let recording = self.musicbrainz.lookup_recording(mbid)?;
        self.recordings.insert(mbid.to_string(), recording.clone());
        Ok(recording)
    }

    /// Cover art for a release, fetched once however many tracks share it.
    fn cover_art(&mut self, release_mbid: &str) -> Result<Option<coverart::CoverArt>> {
        let Some(client) = self.cover.as_mut() else { return Ok(None) };
        if let Some(cached) = self.covers.get(release_mbid) {
            return Ok(cached.clone());
        }
        let art = client.front(release_mbid)?;
        self.covers.insert(release_mbid.to_string(), art.clone());
        Ok(art)
    }
}

/// Identify and tag one file. `Ok(None)` means no confident match.
fn tag_one(
    path: &Path,
    fingerprint: &fingerprint::Fingerprint,
    args: &TagArgs,
    lookup: &mut Lookup,
) -> Result<Option<String>> {
    let candidates = lookup.acoustid.lookup(fingerprint)?;
    let Some(best) = candidates.first() else {
        return Ok(None);
    };

    if best.score < args.min_score && args.on_ambiguous == OnAmbiguous::Skip {
        return Ok(None);
    }

    let recording = lookup.recording(&best.recording_mbid)?;
    let metadata = Metadata::from_musicbrainz(&recording, Some(&best.acoustid));

    let art = match metadata.release_mbid.as_deref() {
        Some(release) => lookup.cover_art(release)?,
        None => None,
    };

    let mut line = format!("{}: {} (score {:.2})", path.display(), metadata.describe(), best.score);
    if best.score < args.min_score {
        line.push_str(" [below --min-score]");
    }

    if args.dry_run {
        line.push_str(" [dry run]");
        return Ok(Some(line));
    }

    let outcome = crate::tag::write_tags(path, &metadata, args.on_existing, art.as_ref())?;
    line.push_str(&summarize_tagging(&outcome));
    Ok(Some(line))
}

fn summarize_tagging(outcome: &TagOutcome) -> String {
    let mut parts = Vec::new();
    if !outcome.written.is_empty() {
        parts.push(format!("wrote {} fields", outcome.written.len()));
    }
    if !outcome.unchanged.is_empty() {
        parts.push(format!("kept {}", outcome.unchanged.len()));
    }
    if outcome.cover_art {
        parts.push("cover art".to_string());
    }
    let mut summary = if parts.is_empty() {
        " — nothing to change".to_string()
    } else {
        format!(" — {}", parts.join(", "))
    };
    for conflict in &outcome.conflicts {
        summary.push_str(&format!(
            "\n    conflict: {} is {:?}, MusicBrainz says {:?}",
            conflict.field, conflict.existing, conflict.proposed
        ));
    }
    summary
}

// -- formatting ------------------------------------------------------------

fn format_duration(seconds: f64) -> String {
    let total = seconds.round() as u64;
    format!("{}:{:02}", total / 60, total % 60)
}

/// One decimal place, or a readable marker for digital silence.
fn format_db(value: f64) -> String {
    if value.is_finite() {
        format!("{value:.1}")
    } else {
        "-inf".to_string()
    }
}

/// JSON has no way to write infinity, so silence becomes `null`.
fn json_number(value: f64) -> String {
    if value.is_finite() {
        format!("{value:.2}")
    } else {
        "null".to_string()
    }
}

fn json_string(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len() + 2);
    out.push('"');
    for c in raw.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Trim from the left, so the file name stays visible when a path is long.
fn elide(text: &str, width: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= width {
        text.to_string()
    } else {
        let tail: String = chars[chars.len() - (width - 3)..].iter().collect();
        format!("...{tail}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_durations() {
        assert_eq!(format_duration(0.0), "0:00");
        assert_eq!(format_duration(61.4), "1:01");
        assert_eq!(format_duration(3_599.0), "59:59");
    }

    #[test]
    fn formats_silence_readably() {
        assert_eq!(format_db(f64::NEG_INFINITY), "-inf");
        assert_eq!(format_db(-14.25), "-14.2");
        assert_eq!(json_number(f64::NEG_INFINITY), "null");
    }

    #[test]
    fn escapes_json_strings() {
        assert_eq!(json_string(r#"a"b\c"#), r#""a\"b\\c""#);
        assert_eq!(json_string("tab\there"), r#""tab\there""#);
    }

    #[test]
    fn elides_from_the_left() {
        assert_eq!(elide("short", 10), "short");
        let elided = elide("/a/very/long/path/song.mp3", 12);
        assert_eq!(elided, ".../song.mp3");
        assert_eq!(elided.chars().count(), 12);
    }
}
