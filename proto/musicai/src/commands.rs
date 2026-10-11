//! Implementations of the subcommands, and the pipeline that runs several of
//! them over one set of files.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use rayon::prelude::*;

use crate::analysis;
use crate::audio::decode::decode_file;
use crate::audio::encode::{write_file, Codec, EncodeOptions};
use crate::audio::mp3::SeekIndex;
use crate::cli::{
    AnalyzeArgs, AnlzArgs, ExportArgs, NormalizeArgs, NormalizeMode, OnAmbiguous, RunArgs,
    StemsArgs, Step, TagArgs,
};
use crate::discover;
use crate::export::image::{capacity_for, Destination, DriveImage};
use crate::export::{anlz, pdb, waveform, BeatGrid};
use crate::loudness::{self, Loudness};
use crate::normalize::replaygain::{write_tags, ReplayGain};
use crate::normalize::{self, Settings};
use crate::report::{self, Event, Progress, Reporter};
use crate::stems::{demucs, dsp, install, Backend, Stem, StemSet};
use crate::tag::{acoustid, coverart, fingerprint, musicbrainz, Metadata, TagOutcome};

/// Outcome of a batch: how many files worked, and the failures.
pub struct Outcome {
    pub processed: usize,
    pub failures: Vec<(PathBuf, anyhow::Error)>,
    pub cancelled: bool,
}

impl Outcome {
    fn new(processed: usize, failures: Vec<(PathBuf, anyhow::Error)>) -> Self {
        Self { processed, failures, cancelled: false }
    }

    fn report(self, reporter: &dyn Reporter) -> Result<()> {
        for (path, error) in &self.failures {
            report::failed(reporter, path, error);
        }
        reporter.event(Event::Finished {
            processed: self.processed,
            failed: self.failures.len(),
            cancelled: self.cancelled,
        });
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
    (lines, Outcome::new(paths.len(), failures))
}

// -- anlz ------------------------------------------------------------------

/// Write the `.DAT`, `.EXT` and `.2EX` analysis files for each input.
///
/// This is half of what a playable drive needs: the players find these through
/// a database (`export.pdb`) that this does not write yet, so the files are
/// correct but nothing indexes them. It exists now because the analysis files
/// are the part that can be checked — against the format documentation, and
/// against a player once the other half lands.
pub fn anlz(args: &AnlzArgs, reporter: &dyn Reporter) -> Result<()> {
    let files = discover::collect(&args.input.inputs, args.input.recursive)?;
    let progress = Progress::new(reporter, files.len());

    let results: Vec<Result<Vec<String>>> = files
        .par_iter()
        .map(|path| {
            if progress.cancelled() {
                return Ok(Vec::new());
            }
            let outcome = write_analysis(args, path);
            progress.tick();
            outcome
        })
        .collect();

    let (lines, mut outcome) = partition(&files, results);
    outcome.cancelled = reporter.cancelled();
    for line in lines {
        reporter.event(Event::Line(line));
    }
    outcome.report(reporter)
}

fn write_analysis(args: &AnlzArgs, path: &Path) -> Result<Vec<String>> {
    let audio = decode_file(path)?;
    let waveforms = waveform::analyze(&audio);
    let listened = analysis::analyze_at(&audio, args.bpm);
    if !listened.found_beats() {
        bail!("no beat could be found; pass --bpm to say what the tempo is");
    }

    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| anyhow::anyhow!("{} has no usable file name", path.display()))?;
    let on_drive = args.on_drive_path.clone().unwrap_or_else(|| format!("/Contents/{name}"));

    let structure = listened.song_structure();
    let seek = seek_index(path);
    let analysis = anlz::Analysis {
        on_drive_path: &on_drive,
        grid: &listened.grid,
        cues: &listened.cues,
        waveforms: &waveforms,
        structure: structure.as_ref(),
        vbr: seek.as_ref(),
    };

    let dir = args
        .output
        .clone()
        .unwrap_or_else(|| path.parent().map_or_else(|| PathBuf::from("."), Path::to_path_buf));
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("track");

    let mut written = Vec::new();
    for (extension, bytes) in
        [("DAT", analysis.dat()), ("EXT", analysis.ext()), ("2EX", analysis.two_ex())]
    {
        let out = dir.join(format!("{stem}.{extension}"));
        std::fs::write(&out, &bytes).with_context(|| format!("writing {}", out.display()))?;
        // Read it back from disk rather than trusting what we just built. This
        // is the smallest version of the check every export will get: the file
        // is only written if something else can parse it.
        let read_back = std::fs::read(&out)?;
        let sections = anlz::inspect(&read_back)
            .with_context(|| format!("{} did not read back as an analysis file", out.display()))?;
        written.push(format!("{}: {} sections", out.display(), sections.len()));
    }

    let key = listened.camelot();
    Ok(vec![format!(
        "{}: {:.2} BPM{}, {} beats, {} phrases, {} cues\n  {}",
        path.display(),
        listened.bpm,
        if key.is_empty() { String::new() } else { format!(" {key}") },
        listened.grid.beats.len(),
        listened.structure.sections.len(),
        listened.cues.iter().filter(|c| c.is_hot()).count(),
        written.join("\n  ")
    )])
}

/// Put a drive image into a CDJ-3000 emulator's USB slot.
///
/// Succeeds when the image reached the emulator, and also when it did not but
/// somebody was shown where it is instead — both leave the image somewhere it
/// can be played from, and only one of them is automatic. `--no-reveal` makes
/// the second one a failure, which is what a script wants.
pub fn emulator(args: &crate::cli::EmulatorArgs, reporter: &dyn report::Reporter) -> Result<()> {
    use crate::emulator::{attach, send, Sent};

    let image = &args.image;
    if !image.is_file() {
        bail!("{} is not a file. `export --image` writes one.", image.display());
    }
    // Absolute, because the emulator opens it and nothing says what its
    // working directory is — a relative path that works here is a path it
    // would look for somewhere else.
    let full = std::fs::canonicalize(image)
        .with_context(|| format!("working out where {} really is", image.display()))?;
    let at = (args.host.as_str(), args.port);

    if args.no_reveal {
        attach(at, &full).map_err(|why| {
            anyhow::anyhow!("{} did not reach the emulator: {why}", full.display())
        })?;
        report::detail(reporter, format!("{} is in the slot", full.display()));
        return Ok(());
    }

    match send(at, &full) {
        Sent::Attached => {
            report::detail(reporter, format!("{} is in the slot", full.display()));
            Ok(())
        }
        Sent::Revealed => {
            report::detail(
                reporter,
                format!(
                    "the emulator is not listening on {}:{}, so {} is showing in a file \
                     manager instead \u{2014} attach it with USB \u{2192} Attach virtual image",
                    args.host,
                    args.port,
                    full.display()
                ),
            );
            Ok(())
        }
        Sent::Written(why) => {
            bail!("{} did not reach the emulator: {why}", full.display())
        }
    }
}

/// The variable-bitrate seek index for a file, when it needs one.
///
/// Only MP3 does: every other format the players read carries its own seek
/// information. A read failure is not fatal — a drive without a seek index
/// still plays, it just seeks a VBR file less precisely — so this reports the
/// problem by returning `None` rather than by stopping the export.
fn seek_index(path: &Path) -> Option<[u32; 401]> {
    if path.extension().and_then(|e| e.to_str()).map(|e| e.eq_ignore_ascii_case("mp3"))
        != Some(true)
    {
        return None;
    }
    SeekIndex::of_mp3(path).ok().flatten().map(|index| index.pvbr_offsets())
}

// -- export ----------------------------------------------------------------

/// The file formats a CDJ-3000 will play. Anything else is copied nowhere: a
/// file the player cannot open is worse on the drive than off it, because it
/// looks fine until the moment it is loaded.
const PLAYABLE: [&str; 7] = ["mp3", "flac", "wav", "aiff", "aif", "m4a", "aac"];
/// The longest path a player will follow, counting every folder name.
const MAX_DRIVE_PATH: usize = 255;

/// How many tracks are analysed before what they produced is written out.
///
/// Analysis is parallel and writing is not — a filesystem is one thing with one
/// position in it — so the work goes through in batches. The batch size bounds
/// how much prepared analysis is held in memory at once, which is a few
/// megabytes here rather than the whole library's worth.
const BATCH: usize = 16;

/// Turn the playlists a caller asked for into the rows the database holds.
///
/// A folder is a playlist row with `is_folder` set, and the lists inside it
/// point at it by id, which is how the players draw a tree. Folders are made
/// in the order they are first mentioned so that the drive's order matches the
/// one the caller sees.
///
/// Tracks that did not make it onto the drive are dropped from the lists that
/// named them: the database refuses a playlist entry for a track it has no row
/// for, and one unreadable file should cost that file rather than the export.
/// An empty playlist is still written — a set that lost its only track is worth
/// seeing on the player as empty rather than silently not being there.
fn playlist_tree(
    specs: &[crate::cli::PlaylistSpec],
    ids_by_path: &HashMap<PathBuf, u32>,
) -> Vec<pdb::Playlist> {
    let mut rows: Vec<pdb::Playlist> = Vec::new();
    let mut folder_ids: BTreeMap<&str, u32> = BTreeMap::new();
    // Ids are handed out as rows are made, so a folder and the list inside it
    // never collide.
    let mut next_id = 1u32;

    for spec in specs {
        let parent = match spec.folder.is_empty() {
            true => 0,
            false => match folder_ids.get(spec.folder.as_str()) {
                Some(id) => *id,
                None => {
                    let id = next_id;
                    next_id += 1;
                    folder_ids.insert(spec.folder.as_str(), id);
                    rows.push(pdb::Playlist::folder(id, &spec.folder));
                    id
                }
            },
        };

        let track_ids: Vec<u32> =
            spec.tracks.iter().filter_map(|path| ids_by_path.get(path).copied()).collect();
        let mut playlist = pdb::Playlist::new(next_id, &spec.name, track_ids);
        playlist.parent_id = parent;
        rows.push(playlist);
        next_id += 1;
    }
    rows
}

/// Build a drive: the audio, the analysis files, and the database that indexes
/// them.
/// Returns the drive's rows and the files they were made from, so that a
/// caller writing the same drive again can hand them back as `already` rather
/// than preparing everything a second time.
pub fn export(
    args: &ExportArgs,
    reporter: &dyn Reporter,
) -> Result<Vec<(PathBuf, crate::export::pdb::Track)>> {
    let began = std::time::Instant::now();
    let files = discover::collect(&args.input.inputs, args.input.recursive)?;
    report::detail(
        reporter,
        format!(
            "export to {}: {} to prepare, {} carried from the last write, {}, {}",
            args.drive
                .as_ref()
                .or(args.image.as_ref())
                .map(|at| at.display().to_string())
                .unwrap_or_else(|| "nowhere".to_string()),
            report::plural(files.len(), "file"),
            report::plural(args.already.len(), "row"),
            report::plural(args.playlists.len(), "playlist"),
            match args.dry_run {
                true => "a dry run",
                false => "for real",
            }
        ),
    );
    let destination = match args.dry_run {
        true => None,
        false => Some(open_destination(args, &files, reporter)?),
    };
    let progress = Progress::new(reporter, files.len());

    // What is already on the drive, carried through unchanged. Its ids are
    // taken, and so are the analysis directories derived from them, so this
    // run's ids start above the highest of them.
    let mut tracks: Vec<pdb::Track> = args.already.iter().map(|(_, row)| row.clone()).collect();
    let mut failures = Vec::new();
    let mut lines = Vec::new();
    let mut ids_by_path: HashMap<PathBuf, u32> =
        args.already.iter().map(|(from, row)| (from.clone(), row.id)).collect();
    let analyses = Analyses::default();
    let taken = tracks.iter().map(|row| row.id).max().unwrap_or(0);
    // What this run put on, to be handed back for the next one to carry.
    let mut made: Vec<(PathBuf, crate::export::pdb::Track)> = args.already.clone();

    // Which analysis directories are already spoken for. The name comes from a
    // hash of the audio path, so two tracks can want the same one; the second
    // to arrive takes the next numbered file rather than overwriting the
    // first. What is already on the drive counts, which is why this starts
    // from the rows being carried.
    let mut occupied: HashMap<String, u32> = HashMap::new();
    for (_, row) in &args.already {
        if let Some((dir, file)) = row.analyze_path.rsplit_once('/') {
            let number = file
                .trim_start_matches("ANLZ")
                .split('.')
                .next()
                .and_then(|n| n.parse::<u32>().ok())
                .unwrap_or(0);
            let next = occupied.entry(dir.to_string()).or_insert(0);
            *next = (*next).max(number + 1);
        }
    }

    for (batch, chunk) in files.chunks(BATCH).enumerate() {
        let first_id = taken + (batch * BATCH) as u32 + 1;
        let prepared: Vec<Result<Prepared>> = chunk
            .par_iter()
            .enumerate()
            .map(|(i, path)| {
                if progress.cancelled() {
                    return Ok(Prepared::skipped());
                }
                let outcome = prepare(args, path, first_id + i as u32, &analyses, reporter);
                progress.tick();
                outcome
            })
            .collect();

        for (path, result) in chunk.iter().zip(prepared) {
            match result {
                Ok(mut prepared) => {
                    prepared.place(&mut occupied);
                    let Some(track) = &prepared.track else { continue };
                    lines.push(format!(
                        "{} -> {} ({:.2} BPM{}, {} beats, {} phrases, {} cues)",
                        path.display(),
                        track.file_path,
                        prepared.bpm,
                        if prepared.key.is_empty() {
                            String::new()
                        } else {
                            format!(" {}", prepared.key)
                        },
                        prepared.beats,
                        prepared.phrases,
                        prepared.cues
                    ));
                    let landed = match &destination {
                        Some(destination) => match prepared.commit(destination, path, reporter) {
                            Ok(()) => Some(prepared.track.unwrap()),
                            Err(e) => {
                                lines.pop();
                                failures.push((path.clone(), e));
                                None
                            }
                        },
                        None => Some(prepared.track.unwrap()),
                    };
                    if let Some(track) = landed {
                        // Which file became which drive id, so a playlist
                        // given in paths can be written in ids. A file that
                        // failed is absent here and drops out of every
                        // playlist that named it, rather than leaving a row
                        // pointing at a track the drive does not have.
                        ids_by_path.insert(path.clone(), track.id);
                        made.push((path.clone(), track.clone()));
                        tracks.push(track);
                    }
                }
                Err(e) => failures.push((path.clone(), e)),
            }
        }
    }

    if !tracks.is_empty() {
        let playlists = match args.playlists.is_empty() {
            true => {
                vec![pdb::Playlist::new(1, &args.playlist, tracks.iter().map(|t| t.id).collect())]
            }
            false => playlist_tree(&args.playlists, &ids_by_path),
        };
        // The history row names the drive. `--label` is what an image is
        // formatted with and defaults to REKORDBOX, which is also what a
        // rekordbox-written stick is called, so it is the right answer for an
        // image and a harmless one for a drive that is already named
        // something else: nothing reads this field back.
        let database = pdb::Database { tracks, playlists, device_name: args.label.clone() };
        let building = std::time::Instant::now();
        let bytes = database.to_bytes()?;
        report::detail(
            reporter,
            format!(
                "built the database in {}: {}, {}, {} bytes",
                took(building),
                report::plural(database.tracks.len(), "track"),
                report::plural(database.playlists.len(), "playlist"),
                bytes.len()
            ),
        );

        match &destination {
            None => lines.push(format!(
                "would write PIONEER/rekordbox/export.pdb: {} tracks, {} bytes",
                database.tracks.len(),
                bytes.len()
            )),
            Some(destination) => {
                const DATABASE: &str = "/PIONEER/rekordbox/export.pdb";
                let writing = std::time::Instant::now();
                destination.write(DATABASE, &bytes)?;
                report::detail(reporter, format!("wrote {DATABASE} in {}", took(writing)));
                // Read the database back off the drive and walk it the way a
                // player would. The export is not finished until that works.
                let checking = std::time::Instant::now();
                let tables = pdb::inspect(&destination.read(DATABASE)?)
                    .context("the database did not read back off the drive")?;
                let rows: usize = tables.iter().map(|t| t.rows).sum();
                report::detail(
                    reporter,
                    format!("read {DATABASE} back and walked it in {}", took(checking)),
                );
                lines.push(format!(
                    "wrote {DATABASE} to {}: {} tables, {} rows, verified",
                    destination.describe(),
                    tables.len(),
                    rows
                ));
            }
        }

        // The same library again, in the format the newer players read. Both
        // are built from the one `database` above, so the two files on the
        // drive cannot disagree about what is on it — which is the failure
        // that firmware 3.30 turned into a room full of DJs with no playlists.
        lines.extend(write_onelibrary(args, &database, destination.as_ref(), reporter)?);
    }

    if let Some(destination) = destination {
        // For an image this is where the filesystem is flushed and the file
        // closed — the point at which a write that looked finished can still
        // fail, so it gets a line of its own.
        let closing = std::time::Instant::now();
        destination.finish()?;
        report::detail(reporter, format!("closed the destination in {}", took(closing)));
    }

    for line in lines {
        reporter.event(Event::Line(line));
    }
    report::detail(
        reporter,
        format!(
            "export finished in {}: {} prepared, {} failed, {} on the drive",
            took(began),
            report::plural(files.len() - failures.len(), "file"),
            failures.len(),
            report::plural(made.len(), "row")
        ),
    );
    let mut outcome = Outcome::new(files.len(), failures);
    outcome.cancelled = reporter.cancelled();
    outcome.report(reporter)?;
    Ok(made)
}

/// One track's worth of drive: the row that describes it, and the files that
/// have to land on the drive for that row to mean anything.
struct Prepared {
    track: Option<pdb::Track>,
    /// Where the audio goes, as the player will see it.
    on_drive: String,
    /// The bytes to write there, when they are not the source file's.
    ///
    /// Set only for a file re-encoded to reach an older player. `None` means
    /// copy the original, which is what everything else does.
    carried: Option<Vec<u8>>,
    /// The analysis files, by their paths on the drive.
    analysis: Vec<(String, Vec<u8>)>,
    beats: usize,
    bpm: f64,
    /// The key as it reads on the player, or empty when none was found.
    key: String,
    phrases: usize,
    cues: usize,
}

impl Prepared {
    fn skipped() -> Self {
        Self {
            track: None,
            on_drive: String::new(),
            carried: None,
            analysis: Vec::new(),
            beats: 0,
            bpm: 0.0,
            key: String::new(),
            phrases: 0,
            cues: 0,
        }
    }

    /// Put it on the drive, and read every analysis file back before calling it
    /// written.
    /// Settle which numbered analysis file this track gets, now that it is
    /// known what else is on the drive.
    ///
    /// Every track wants `ANLZ0000` in its own hashed directory, and usually
    /// gets it. Two audio paths hashing alike is the exception this exists
    /// for: the second one takes `ANLZ0001`, and the player tells them apart by
    /// the path each file names inside itself.
    fn place(&mut self, occupied: &mut HashMap<String, u32>) {
        let Some(track) = &mut self.track else { return };
        let Some(dir) = self
            .analysis
            .first()
            .and_then(|(at, _)| at.rsplit_once('/'))
            .map(|(dir, _)| dir.to_string())
        else {
            return;
        };
        let next = occupied.entry(dir.clone()).or_insert(0);
        let names = anlz::analysis_paths(&dir, *next);
        *next += 1;
        for (slot, name) in self.analysis.iter_mut().zip(names.iter()) {
            slot.0.clone_from(name);
        }
        track.analyze_path.clone_from(&names[0]);
    }

    fn commit(
        &self,
        destination: &Destination,
        source: &Path,
        reporter: &dyn Reporter,
    ) -> Result<()> {
        let name = short(source);
        let copying = std::time::Instant::now();
        let size = match &self.carried {
            Some(bytes) => {
                destination.write(&self.on_drive, bytes)?;
                bytes.len() as u64
            }
            None => {
                destination.copy_in(&self.on_drive, source)?;
                std::fs::metadata(source).map(|m| m.len()).unwrap_or(0)
            }
        };
        report::detail(
            reporter,
            format!("{name}: copied {size} bytes to {} in {}", self.on_drive, took(copying)),
        );

        let writing = std::time::Instant::now();
        let mut written = 0usize;
        for (at, bytes) in &self.analysis {
            destination.write(at, bytes)?;
            // Read back through a parser that shares no code with the writer,
            // here rather than at the end, so a drive that cannot hold what was
            // just put on it says so on the file it failed.
            //
            // Checked rather than merely parsed: a file can walk cleanly and
            // still hold a grid no player can draw, and the one place to find
            // that out is here rather than in a booth.
            anlz::check(&destination.read(at)?)
                .with_context(|| format!("{at} did not read back off the drive"))?;
            written += bytes.len();
        }
        report::detail(
            reporter,
            format!(
                "{name}: wrote and read back {} ({written} bytes) in {}",
                report::plural(self.analysis.len(), "analysis file"),
                took(writing)
            ),
        );
        Ok(())
    }
}

/// Where this export is going, and — for an image — how big it needs to be.
///
/// The size has to be settled before a byte is written, so it is estimated from
/// the audio plus a couple of megabytes a track for the analysis. That is
/// generous for anything under about half an hour long.
/// Write the OneLibrary database beside the legacy one, when there is a key
/// for it.
///
/// A drive without it is the drive this program used to write, which the
/// CDJ-3000X and the other newer players read nothing at all of. A drive with
/// it is one a CDJ-3000X has browsed.
///
/// No key is not a failure: a build can be made without one, so this reports
/// what was not written and why, and the
/// export carries on.
fn write_onelibrary(
    args: &ExportArgs,
    database: &pdb::Database,
    destination: Option<&Destination>,
    reporter: &dyn Reporter,
) -> Result<Vec<String>> {
    use crate::export::onelibrary;

    let Some(key) = crate::rekordbox::onelibrary_key(args.onelibrary_key.as_deref()) else {
        report::detail(reporter, "no OneLibrary key, so only the legacy database goes on");
        return Ok(vec![crate::rekordbox::no_onelibrary_key()]);
    };

    // What the drive calls itself. An image is formatted with a label; a
    // folder is only ever known by its name.
    let device = match (&args.image, &args.drive) {
        (Some(_), _) => args.label.clone(),
        (None, Some(drive)) => {
            drive.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string()
        }
        (None, None) => String::new(),
    };

    let analysed = onelibrary::analysed_bits(args.analysed_bits);
    let building = std::time::Instant::now();
    let bytes = onelibrary::to_bytes(database, &key, &device, analysed)
        .context("building the OneLibrary database")?;
    report::detail(reporter, format!("every track is marked analysedBits {analysed}"));
    report::detail(
        reporter,
        format!(
            "built the OneLibrary database for device {device:?} in {}: {} bytes",
            took(building),
            bytes.len()
        ),
    );
    let Some(destination) = destination else {
        return Ok(vec![format!(
            "would write {}: {} tracks, {} bytes",
            onelibrary::DRIVE_PATH,
            database.tracks.len(),
            bytes.len()
        )]);
    };

    let writing = std::time::Instant::now();
    destination.write(onelibrary::DRIVE_PATH, &bytes)?;
    report::detail(reporter, format!("wrote {} in {}", onelibrary::DRIVE_PATH, took(writing)));
    // Off the drive again, keyed again, counted again — the same standard the
    // legacy database is held to.
    let checking = std::time::Instant::now();
    let summary = onelibrary::inspect(&destination.read(onelibrary::DRIVE_PATH)?, &key)
        .context("the OneLibrary database did not read back off the drive")?;
    report::detail(
        reporter,
        format!("read {} back and opened it in {}", onelibrary::DRIVE_PATH, took(checking)),
    );
    Ok(vec![format!(
        "wrote {} to {}: {} tables, {} tracks, {} playlists, verified — a CDJ-3000X browses \
         these",
        onelibrary::DRIVE_PATH,
        destination.describe(),
        summary.tables,
        summary.tracks,
        summary.playlists
    )])
}

fn open_destination(
    args: &ExportArgs,
    files: &[PathBuf],
    reporter: &dyn Reporter,
) -> Result<Destination> {
    if let Some(root) = &args.drive {
        report::detail(reporter, format!("writing into the folder {}", root.display()));
        return Ok(Destination::Directory(root.clone()));
    }
    let image = args.image.as_ref().expect("clap requires one of --drive and --image");

    let audio: u64 = files.iter().filter_map(|f| std::fs::metadata(f).ok()).map(|m| m.len()).sum();
    let analysis = files.len() as u64 * 2 * 1024 * 1024;
    let capacity = capacity_for(audio + analysis);
    // The size has to be settled before a byte is written, so what it was
    // worked out from is worth saying: an image that turns out too small is a
    // write that fails a long way in.
    report::detail(
        reporter,
        format!(
            "creating a {capacity} byte image at {}, labelled {:?} — {audio} bytes of audio plus \
             {analysis} for analysis",
            image.display(),
            args.label
        ),
    );
    let making = std::time::Instant::now();
    let created = DriveImage::create(image, capacity, &args.label)?;
    report::detail(reporter, format!("formatted the image in {}", took(making)));
    Ok(Destination::Image(std::sync::Mutex::new(created)))
}

/// How long a step took, for the detail lines. Milliseconds under ten seconds,
/// and seconds above it, because "94318 ms" is a number nobody reads as a
/// minute and a half.
fn took(since: std::time::Instant) -> String {
    let ms = since.elapsed().as_millis();
    match ms {
        0..=9_999 => format!("{ms} ms"),
        _ => format!("{:.1} s", ms as f64 / 1000.0),
    }
}

/// A file's name on its own, which is what a detail line has room for. The
/// whole path is in the result line the same file gets.
fn short(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "?".to_string())
}

/// The analysis of a track, kept so that its stems can share it.
type Shared = std::sync::Arc<analysis::TrackAnalysis>;

/// Analyses already done, by the file they were done on.
///
/// A track with two stems would otherwise be listened to three times, and the
/// three would not necessarily agree — which is the whole thing this is here to
/// prevent.
#[derive(Default)]
struct Analyses(std::sync::Mutex<BTreeMap<PathBuf, Shared>>);

impl Analyses {
    /// The analysis of one file, doing the work only if nobody else has.
    fn of(&self, path: &Path, bpm: Option<f64>) -> Result<Shared> {
        if let Some(done) = self.0.lock().ok().and_then(|cache| cache.get(path).cloned()) {
            return Ok(done);
        }
        let audio = decode_file(path)?;
        let listened: Shared = std::sync::Arc::new(analysis::analyze_at(&audio, bpm));
        if let Ok(mut cache) = self.0.lock() {
            cache.insert(path.to_path_buf(), Shared::clone(&listened));
        }
        Ok(listened)
    }
}

fn prepare(
    args: &ExportArgs,
    path: &Path,
    id: u32,
    analyses: &Analyses,
    reporter: &dyn Reporter,
) -> Result<Prepared> {
    let name = short(path);
    let extension =
        path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).unwrap_or_default();
    if !PLAYABLE.contains(&extension.as_str()) {
        bail!("a player cannot open a .{extension} file");
    }

    let began = std::time::Instant::now();
    let audio = decode_file(path)?;
    report::detail(
        reporter,
        format!(
            "{name}: decoded {} Hz, {} ch, {:.1}s in {}",
            audio.sample_rate,
            audio.planes.len(),
            audio.duration_secs(),
            took(began)
        ),
    );
    if audio.sample_rate > 96_000 {
        bail!("{} Hz is above the 96 kHz a player will accept", audio.sample_rate);
    }
    let metadata = crate::tag::read_metadata(path).unwrap_or_default();

    // The track this file was cut from, when it is a stem of one, and what that
    // track is filed under. Read once: it decides both where the stem lands and
    // what its row says, and those two must not be worked out separately.
    let parent = args.companions.iter().find(|(stem, _)| stem == path).map(|(_, parent)| parent);
    let parent_tags = parent.map(|parent| crate::tag::read_metadata(parent).unwrap_or_default());

    let filename = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| anyhow::anyhow!("{} has no usable file name", path.display()))?;
    // Whether this file can reach the player this drive is for as it is. A
    // format it does not open, or a rate above its ceiling, is re-encoded on
    // the way on — an MP3 at the best rate and bitrate the hardware takes.
    //
    // Decided here, before the name is worked out, because the name is what
    // everything else hangs off: the analysis directory is a hash of the path
    // on the drive, so a file that goes on as an MP3 has to be called one from
    // this line onwards or the player looks for its grid in the wrong place.
    //
    // The library's own file is untouched. This is a copy bound for a stick,
    // and the grid, key, cues and phrases below are all measured from the
    // original rather than from what is written.
    let recode = args.player.must_recode(&extension, audio.sample_rate);
    let renamed = format!("{}.mp3", stem_of(filename));
    let filename = match recode {
        false => filename,
        true => renamed.as_str(),
    };
    // A stem with no name of its own takes its parent's, which is not a
    // nicety: a wav has nowhere to keep tags and `--no-tags` asks for none, and
    // either way the alternative is a row reading "Unknown Artist" next to the
    // record it was cut from.
    let artist = metadata
        .artist
        .clone()
        .or_else(|| parent_tags.as_ref().and_then(|tags| tags.artist.clone()))
        .unwrap_or_else(|| UNKNOWN_ARTIST.to_string());
    // Where it goes. A stem is filed with its parent — in the parent's folder,
    // worked out from the parent's tags rather than its own — so that the four
    // files of a record sit together on the drive however the stems were
    // written. Deriving it from the stem's tags instead only lands it beside
    // the parent when those tags happen to match, which is not something to
    // stake the layout of a drive on.
    let folder = match &parent_tags {
        Some(tags) => tags.artist.clone().unwrap_or_else(|| UNKNOWN_ARTIST.to_string()),
        None => artist.clone(),
    };
    let on_drive = on_drive_path(&folder, filename);
    if on_drive.len() > MAX_DRIVE_PATH {
        bail!("{} characters is longer than a player will follow", on_drive.len());
    }
    report::detail(
        reporter,
        format!("{name}: goes to {on_drive} ({} of {MAX_DRIVE_PATH} characters)", on_drive.len()),
    );

    // Named the way rekordbox names it, which is not a courtesy: the player
    // works this name out for itself from the audio path and looks nowhere
    // else. See [`anlz::analysis_dir`]. A second track hashing to the same
    // directory is given a numbered file instead, which is settled when the
    // drive is written rather than here, because it depends on what else is
    // going on.
    let analyze_dir = anlz::analysis_dir(&on_drive);
    let analyze_path = anlz::analysis_paths(&analyze_dir, 0)[0].clone();

    // Everything but the picture is the record's rather than the file's, so a
    // stem is looked up under its parent: its own answers would be measured
    // from audio with most of the track removed — a vocal with no drums under
    // it — and a cue that does not line up with the one on the parent is worse
    // than no cue at all.
    let source = parent.map(PathBuf::as_path).unwrap_or(path);
    let supplied = args
        .prepared
        .iter()
        .find(|(named, _)| named == source)
        .map(|(_, prep)| prep)
        .filter(|prep| !prep.is_empty());

    // A tempo the collection is sure of is passed to the tracker, which still
    // tracks the beats rather than assuming them: a tempo says how far apart
    // they are, not where they fall.
    let listening = std::time::Instant::now();
    let bpm = supplied.and_then(|prep| prep.bpm).or(args.bpm);
    let listened = match parent {
        Some(parent) => {
            report::detail(
                reporter,
                format!("{name}: takes its grid, cues, key and phrases from {}", short(parent)),
            );
            analyses.of(parent, bpm).with_context(|| {
                format!("analysing {} for its stem {}", parent.display(), path.display())
            })?
        }
        None => analyses.of(path, bpm)?,
    };

    // What actually goes on the drive: the collection's answers where it has
    // them, the measured ones where it does not. Measuring is still done —
    // the waveform is of the audio and nothing else can supply it, and the
    // grid is what a section in milliseconds has to be placed against — but a
    // cue somebody moved by hand is not something to measure over.
    // The collection's grid where it kept one. A grid that bends is the only
    // copy of work a player did, and re-measuring it would hand the drive a
    // flattened version of what the drive gave us.
    let grid = match supplied {
        Some(prep) if !prep.beat_ms.is_empty() => BeatGrid::from_beat_times(&prep.beat_ms),
        _ => listened.grid.clone(),
    };
    let beat_ms: Vec<u32> = grid.beats.iter().map(|beat| beat.time_ms).collect();
    let cues = match supplied {
        Some(prep) if !prep.cues.is_empty() => prep.cues.clone(),
        _ => listened.cues.clone(),
    };
    let structure = match supplied {
        Some(prep) if !prep.parts.is_empty() => {
            crate::analysis::structure::Structure::from_parts(&prep.parts, &beat_ms)
                .to_song_structure()
        }
        _ => listened.song_structure(),
    };
    let key = match supplied {
        Some(prep) if !prep.key.is_empty() => prep.key.clone(),
        _ => listened.camelot(),
    };
    if let Some(prep) = supplied {
        let mut kept = Vec::new();
        if !prep.cues.is_empty() {
            kept.push(report::plural(prep.cues.len(), "cue"));
        }
        if !prep.parts.is_empty() {
            kept.push(report::plural(prep.parts.len(), "phrase"));
        }
        if !prep.key.is_empty() {
            kept.push(format!("key {}", prep.key));
        }
        if let Some(bpm) = prep.bpm {
            kept.push(format!("{bpm:.2} BPM"));
        }
        if !prep.beat_ms.is_empty() {
            kept.push(format!("a bent grid of {}", report::plural(prep.beat_ms.len(), "beat")));
        }
        report::detail(reporter, format!("{name}: writes the collection's {}", kept.join(", ")));
    }
    report::detail(
        reporter,
        format!(
            "{name}: {:.2} BPM, {}, {} ({} hot) in {}",
            listened.bpm,
            report::plural(listened.grid.beats.len(), "beat"),
            report::plural(cues.len(), "cue"),
            cues.iter().filter(|c| c.is_hot()).count(),
            took(listening)
        ),
    );
    if !listened.found_beats() {
        bail!("no beat could be found; pass --bpm to say what the tempo is");
    }
    // The picture, though, is of this file: an acapella that drew the whole
    // track's waveform would be showing something that is not playing.
    let drawing = std::time::Instant::now();
    let waveforms = waveform::analyze(&audio);
    report::detail(reporter, format!("{name}: drew its own waveforms in {}", took(drawing)));
    let seek = seek_index(path);
    let files = anlz::Analysis {
        on_drive_path: &on_drive,
        grid: &grid,
        cues: &cues,
        waveforms: &waveforms,
        structure: structure.as_ref(),
        vbr: seek.as_ref(),
    };

    // The audio that actually goes on the drive, and what the row has to say
    // about it. A re-encode is the only case where either differs from the
    // source file; everything else is copied byte for byte.
    let carried = match recode {
        false => None,
        true => {
            let rate = args.player.mp3_rate_from(audio.sample_rate);
            let encoding = std::time::Instant::now();
            let bytes: Vec<u8> = crate::audio::encode::to_mp3(
                &audio,
                &crate::audio::encode::EncodeOptions {
                    mp3_bitrate: crate::compat::Player::MP3_KBPS,
                    mp3_vbr: None,
                    mp3_out_rate: Some(rate),
                    ..Default::default()
                },
            )?;
            report::detail(
                reporter,
                format!(
                    "{name}: a {} does not take this as it is, so it goes on as a {} kbps mp3 \
                     at {rate} Hz \u{2014} {} bytes in {}",
                    args.player.name(),
                    crate::compat::Player::MP3_KBPS,
                    bytes.len(),
                    took(encoding)
                ),
            );
            Some((bytes, rate))
        }
    };

    let file_size = match &carried {
        Some((bytes, _)) => bytes.len() as u32,
        None => std::fs::metadata(path).map(|m| m.len() as u32).unwrap_or(0),
    };
    let track = pdb::Track {
        id,
        title: metadata.title.clone().unwrap_or_else(|| stem_of(filename)),
        artist,
        album: metadata
            .album
            .clone()
            .or_else(|| parent_tags.as_ref().and_then(|tags| tags.album.clone()))
            .unwrap_or_default(),
        key: key.clone(),
        comment: supplied.map(|prep| prep.comment.clone()).unwrap_or_default(),
        file_path: on_drive.clone(),
        analyze_path,
        tempo_x100: (listened.bpm * 100.0).round() as u32,
        duration_secs: audio.duration_secs().round() as u16,
        // What the file on the drive is, not what the library's copy is: the
        // player reads this row about the file sitting beside it.
        sample_rate: match &carried {
            Some((_, rate)) => *rate,
            None => audio.sample_rate,
        },
        sample_depth: 16,
        file_size,
        track_number: metadata.track_number.unwrap_or(0),
        disc_number: metadata.disc_number.unwrap_or(0) as u16,
        year: metadata
            .date
            .as_deref()
            .and_then(|d| d.get(..4))
            .and_then(|y| y.parse().ok())
            .unwrap_or(0),
        ..pdb::Track::default()
    };

    let names = anlz::analysis_paths(&analyze_dir, 0);
    let analysis = vec![
        (names[0].clone(), files.dat()),
        (names[1].clone(), files.ext()),
        (names[2].clone(), files.two_ex()),
    ];

    Ok(Prepared {
        track: Some(track),
        on_drive,
        analysis,
        carried: carried.map(|(bytes, _)| bytes),
        beats: listened.grid.beats.len(),
        bpm: listened.bpm,
        key,
        phrases: structure.as_ref().map(|s| s.phrases.len()).unwrap_or(0),
        cues: cues.iter().filter(|c| c.is_hot()).count(),
    })
}

/// The name a file is filed under when it carries no artist of its own.
pub const UNKNOWN_ARTIST: &str = "Unknown Artist";

/// Where a track's audio lands on the drive.
///
/// Public because anything that wants to warn about a path before it is written
/// has to be able to work out the same path the writer will use. A preflight
/// that reimplements this rule is a preflight that will eventually disagree
/// with it, and pass a drive the writer then refuses.
///
/// A stem is filed under the artist of the track it came from rather than its
/// own, so a caller checking a stem's path should pass the parent's name here —
/// which is what puts the stem in the same folder as the record it was cut
/// from. See `prepare`.
pub fn on_drive_path(artist: &str, filename: &str) -> String {
    format!("/Contents/{}/{}", safe_component(artist), filename)
}

/// The longest on-drive path a player will follow, for callers checking one
/// before it is written.
pub const MAX_ON_DRIVE_PATH: usize = MAX_DRIVE_PATH;

/// Whether a player can open a file with this extension.
pub fn is_playable(extension: &str) -> bool {
    PLAYABLE.contains(&extension.to_ascii_lowercase().as_str())
}

/// A folder name a FAT filesystem and a player will both accept.
fn safe_component(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| if c.is_control() || "/\\:*?\"<>|".contains(c) { '_' } else { c })
        .collect();
    let trimmed = cleaned.trim().trim_end_matches('.');
    if trimmed.is_empty() {
        "Unknown".to_string()
    } else {
        trimmed.chars().take(60).collect()
    }
}

fn stem_of(filename: &str) -> String {
    filename.rsplit_once('.').map_or(filename, |(stem, _)| stem).to_string()
}

// -- analyze ---------------------------------------------------------------

pub fn analyze(args: &AnalyzeArgs, reporter: &dyn Reporter) -> Result<()> {
    let files = discover::collect(&args.input.inputs, args.input.recursive)?;
    analyze_files(args, &files, reporter)
}

pub fn analyze_files(args: &AnalyzeArgs, files: &[PathBuf], reporter: &dyn Reporter) -> Result<()> {
    let progress = Progress::new(reporter, files.len());

    let results: Vec<Result<Vec<String>>> = files
        .par_iter()
        .map(|path| {
            if progress.cancelled() {
                return Ok(Vec::new());
            }
            let outcome = (|| {
                let audio = decode_file(path)?;
                let measured = loudness::measure(&audio)?;
                Ok(vec![if args.json {
                    json_line(path, &measured, audio.duration_secs(), audio.channels())
                } else {
                    table_row(path, &measured, audio.duration_secs())
                }])
            })();
            progress.tick();
            outcome
        })
        .collect();

    let (lines, mut outcome) = partition(files, results);
    outcome.cancelled = reporter.cancelled();

    if !args.json && !lines.is_empty() {
        reporter.event(Event::Heading(format!(
            "{:<40} {:>8} {:>10} {:>8} {:>11}",
            "file", "length", "loudness", "range", "true peak"
        )));
    }
    for line in lines {
        reporter.event(Event::Line(line));
    }

    outcome.report(reporter)
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

pub fn normalize(args: &NormalizeArgs, reporter: &dyn Reporter) -> Result<()> {
    let files = discover::collect(&args.input.inputs, args.input.recursive)?;
    normalize_files(args, &files, reporter).map(|_| ())
}

/// Normalize an explicit list, and report which files anything downstream
/// should carry on with.
///
/// In ReplayGain mode that is the inputs, since the audio was left alone. When
/// re-encoding it is the files that were just written — a pipeline that went on
/// to tag the originals instead would tag the wrong copy. A file that was
/// skipped, or a dry run, produces nothing new, so the input stands in.
pub fn normalize_files(
    args: &NormalizeArgs,
    files: &[PathBuf],
    reporter: &dyn Reporter,
) -> Result<Vec<PathBuf>> {
    match args.mode {
        NormalizeMode::Reencode => normalize_reencode(args, files, reporter),
        NormalizeMode::Replaygain => {
            normalize_replaygain(args, files, reporter).map(|()| files.to_vec())
        }
    }
}

fn normalize_reencode(
    args: &NormalizeArgs,
    files: &[PathBuf],
    reporter: &dyn Reporter,
) -> Result<Vec<PathBuf>> {
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
        mp3_vbr: None,
        dither: !args.no_dither,
        // Never here: the library's own files keep the rate they came with.
        mp3_out_rate: None,
    };

    let progress = Progress::new(reporter, plans.len());
    let results: Vec<Result<Normalized>> = plans
        .par_iter()
        .map(|plan| {
            if progress.cancelled() {
                return Ok(Normalized::skipped(&plan.input));
            }
            let outcome = normalize_one(plan, args, &settings, &encode);
            progress.tick();
            outcome
        })
        .collect();

    let mut failures = Vec::new();
    let mut next = Vec::with_capacity(plans.len());
    for (plan, result) in plans.iter().zip(results) {
        match result {
            Ok(done) => {
                if let Some(line) = done.line {
                    reporter.event(Event::Line(line));
                }
                next.push(done.produced);
            }
            Err(e) => failures.push((plan.input.clone(), e)),
        }
    }

    let mut outcome = Outcome::new(plans.len(), failures);
    outcome.cancelled = reporter.cancelled();
    outcome.report(reporter)?;
    Ok(next)
}

/// What normalizing one file produced: something to report, and the file that
/// carries on to the next step.
struct Normalized {
    line: Option<String>,
    produced: PathBuf,
}

impl Normalized {
    /// Nothing was written, so the input is what carries on.
    fn skipped(input: &Path) -> Self {
        Self { line: None, produced: input.to_path_buf() }
    }
}

fn normalize_one(
    plan: &Plan,
    args: &NormalizeArgs,
    settings: &Settings,
    encode: &EncodeOptions,
) -> Result<Normalized> {
    let mut audio = decode_file(&plan.input)?;
    let report = normalize::apply(&mut audio, settings)?;

    if report.silent {
        return Ok(Normalized {
            line: Some(format!("{}: silent, skipped", plan.input.display())),
            // Nothing was written, so anything downstream uses the original.
            produced: plan.input.clone(),
        });
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
        line.push_str(&format!(", limiter took off up to {:.2} dB", -report.limiter_reduction_db));
    }

    let produced = if args.dry_run {
        line.push_str(" [dry run]");
        plan.input.clone()
    } else {
        let report = write_file(&plan.output, &audio, plan.codec, encode)?;
        line.push_str(&format!(" -> {}", plan.output.display()));
        if report.clipped_anything() {
            line.push_str(&format!(" (warning: {} samples clipped)", report.clipped));
        }
        plan.output.clone()
    };
    Ok(Normalized { line: Some(line), produced })
}

fn normalize_replaygain(
    args: &NormalizeArgs,
    files: &[PathBuf],
    reporter: &dyn Reporter,
) -> Result<()> {
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
    // Progress counts files rather than groups, so an --album run over one
    // directory still advances instead of sitting at zero until it finishes.
    let progress = Progress::new(reporter, files.len());
    let results: Vec<Result<Vec<String>>> = group_keys
        .par_iter()
        .map(|key| {
            let members = &groups[key];
            if progress.cancelled() {
                return Ok(Vec::new());
            }

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
                progress.tick();
            }
            Ok(lines)
        })
        .collect();

    let (mut lines, mut outcome) = partition(&group_keys, results);
    outcome.cancelled = reporter.cancelled();
    lines.sort();
    for line in lines {
        reporter.event(Event::Line(line));
    }
    outcome.report(reporter)
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

pub fn stems(args: &StemsArgs, reporter: &dyn Reporter) -> Result<()> {
    let files = discover::collect(&args.input.inputs, args.input.recursive)?;
    stems_files(args, &files, reporter)
}

pub fn stems_files(args: &StemsArgs, files: &[PathBuf], reporter: &dyn Reporter) -> Result<()> {
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

    let encode = EncodeOptions {
        bit_depth: args.bit_depth,
        mp3_bitrate: args.bitrate,
        // Stems are written VBR by default: they are long stretches of near
        // silence between phrases, which is exactly what a constant bitrate
        // spends the most on for the least.
        mp3_vbr: args.vbr(),
        dither: true,
        // A stem keeps its parent's rate: the two are compared on a deck, and
        // one resampled and one not is a comparison of two different files.
        mp3_out_rate: None,
    };

    let mut failures = Vec::new();
    let progress = Progress::new(reporter, files.len());

    // Separation is memory-hungry and already uses every core internally, so
    // files go one at a time. Each one reports as it finishes rather than at the
    // end, because a long batch would otherwise look like it had hung.
    for path in files {
        if progress.cancelled() {
            break;
        }
        match separate_one(path, args, &config, &encode, demucs_bin.as_deref(), reporter) {
            Ok(written) => {
                for line in written {
                    reporter.event(Event::Line(line));
                }
            }
            Err(e) => failures.push((path.clone(), e)),
        }
        progress.tick();
    }

    let mut outcome = Outcome::new(files.len(), failures);
    outcome.cancelled = reporter.cancelled();
    outcome.report(reporter)
}

fn separate_one(
    path: &Path,
    args: &StemsArgs,
    config: &dsp::Config,
    encode: &EncodeOptions,
    demucs_bin: Option<&std::ffi::OsStr>,
    reporter: &dyn Reporter,
) -> Result<Vec<String>> {
    let track = path
        .file_stem()
        .with_context(|| format!("{} has no file name", path.display()))?
        .to_string_lossy()
        .into_owned();

    // Never the source's format: a wav track would otherwise yield three wav
    // stems, which is a gigabyte a record for audio that gets played under
    // something else.
    let codec = args.format.codec();

    // Check the destinations before doing the expensive part.
    for stem in &args.only {
        let out = stem_path(&args.out_dir, &track, *stem, codec);
        if out.exists() && !args.force {
            bail!("{} already exists; pass --force to overwrite", out.display());
        }
    }

    let mut separated = match args.backend {
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
            demucs_config.model = args.model();
            demucs_config.device = args.demucs.demucs_device.clone();
            demucs_config.shifts = args.shifts();
            demucs_config.overlap = args.demucs.demucs_overlap;

            // Separating one track is minutes of work, so it reports from
            // inside rather than only when it finishes.
            let result = demucs::separate(path, &demucs_config, &|percent| {
                reporter.event(Event::Step { percent })
            });
            // Demucs' own output is an intermediate; the stems we write are the
            // deliverable. Clean up whether or not it succeeded.
            let _ = std::fs::remove_dir_all(&work_dir);
            result?
        }
    };

    // Bring the stems under full scale before writing, by one shared gain, so
    // an integer format does not clip a stem that peaks above the mix. A hair
    // under 0 dBFS, since the true peak between samples can sit a touch above
    // the highest sample.
    let attenuation = separated.fit_under(0.98);

    write_stems(&track, &separated, args, encode, codec, path, attenuation)
}

/// Where a stem goes: beside its siblings, named after the track it came from,
/// so `track.mp3` yields `track-vocals.mp3` next to `track-drums.mp3`.
fn stem_path(out_dir: &Path, track: &str, stem: Stem, codec: Codec) -> PathBuf {
    out_dir.join(format!("{}-{}.{}", track, stem.name(), codec.extension()))
}

#[allow(clippy::too_many_arguments)]
fn write_stems(
    track: &str,
    separated: &StemSet,
    args: &StemsArgs,
    encode: &EncodeOptions,
    codec: Codec,
    source: &Path,
    attenuation: Option<f32>,
) -> Result<Vec<String>> {
    let mut written = Vec::new();
    for stem in &args.only {
        let out = stem_path(&args.out_dir, track, *stem, codec);
        let report = write_file(&out, separated.get(*stem), codec, encode)?;

        // Tags go on after the audio, so the stem is identifiable in a library
        // rather than landing there as an untitled file by an unknown artist.
        if !args.no_tags {
            crate::tag::copy::copy_for_stem(source, &out, *stem)?;
        }

        let mut line = format!("wrote {}", out.display());
        if let Some(gain_db) = attenuation {
            // The stems were pulled down together to fit under full scale. Said
            // once, on the first line, rather than as a warning per stem: it is
            // expected, not a fault, and the shared gain keeps them in balance.
            if *stem == args.only[0] {
                line.push_str(&format!(
                    " (stems attenuated {gain_db:.1} dB to stay under full scale)"
                ));
            }
        } else if report.clipped_anything() {
            // With the shared attenuation this should not happen, but if a
            // format's own rounding pushes a sample over, say so rather than
            // hide it.
            line.push_str(&format!(" — warning: {} samples clipped", report.clipped));
        }
        written.push(line);
    }
    Ok(written)
}

// -- tag -------------------------------------------------------------------

pub fn tag(args: &TagArgs, reporter: &dyn Reporter) -> Result<()> {
    let files = discover::collect(&args.input.inputs, args.input.recursive)?;
    tag_files(args, &files, reporter)
}

pub fn tag_files(args: &TagArgs, files: &[PathBuf], reporter: &dyn Reporter) -> Result<()> {
    let progress = Progress::new(reporter, files.len());

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
        let (lines, outcome) = partition(files, results);
        for (path, line) in files.iter().zip(lines) {
            reporter.event(Event::Line(format!("{}\t{line}", path.display())));
        }
        return outcome.report(reporter);
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
        if progress.cancelled() {
            break;
        }
        let result = fingerprint.and_then(|fp| tag_one(path, &fp, args, &mut lookup));
        match result {
            Ok(Some(line)) => {
                matched += 1;
                reporter.event(Event::Line(line));
            }
            Ok(None) => {
                unmatched += 1;
                reporter.event(Event::Line(format!("{}: no confident match", path.display())));
            }
            Err(e) => failures.push((path.clone(), e)),
        }
        progress.tick();
    }

    reporter.event(Event::Summary(format!(
        "{matched} tagged, {unmatched} unmatched, {} failed",
        failures.len()
    )));
    let mut outcome = Outcome::new(files.len(), failures);
    outcome.cancelled = reporter.cancelled();
    outcome.report(reporter)
}

// -- run: the whole pipeline -----------------------------------------------

/// Everything the pipeline needs, in whatever detail the caller has.
///
/// The command line offers a curated subset of each step's options and builds
/// these from it; the window already holds all three argument structs and
/// passes them straight through. Both end up here, so there is one pipeline
/// rather than two that drift.
pub struct Pipeline<'a> {
    pub steps: &'a [Step],
    pub normalize: &'a NormalizeArgs,
    pub tag: &'a TagArgs,
    pub stems: &'a StemsArgs,
}

pub fn run(args: &RunArgs, reporter: &dyn Reporter) -> Result<()> {
    // The whole list is gathered before any work starts, so the run knows how
    // much there is to do and can say so, and so a directory is walked once
    // rather than once per step.
    let files = discover::collect(&args.input.inputs, args.input.recursive)?;

    let normalize = args.normalize_args();
    let tag = args.tag_args();
    let stems = args.stems_args();

    run_pipeline(
        &files,
        &Pipeline { steps: &args.steps, normalize: &normalize, tag: &tag, stems: &stems },
        reporter,
    )
}

/// Normalize, tag and separate one list of files, in that order.
///
/// Each step runs over every file before the next one starts. That is what
/// makes the ordering meaningful — the tags are on disk before anything is
/// separated — and it means a step that needs the network is not interleaved
/// with one that saturates the CPU.
pub fn run_pipeline(
    files: &[PathBuf],
    pipeline: &Pipeline<'_>,
    reporter: &dyn Reporter,
) -> Result<()> {
    // Requested order is ignored in favour of the order that makes sense; see
    // Step::ALL.
    let steps: Vec<Step> =
        Step::ALL.iter().copied().filter(|s| pipeline.steps.contains(s)).collect();

    if files.is_empty() {
        reporter.event(Event::Summary("nothing to do: no audio files found".into()));
        return Ok(());
    }
    if steps.is_empty() {
        reporter.event(Event::Summary("nothing to do: no steps selected".into()));
        return Ok(());
    }

    reporter.event(Event::Summary(format!(
        "{} file{} through {}",
        files.len(),
        if files.len() == 1 { "" } else { "s" },
        steps.iter().map(|s| s.name()).collect::<Vec<_>>().join(" -> "),
    )));

    // Each step works on what the one before it produced. Only normalizing
    // changes the set, and only when it re-encodes into new files.
    let mut working: Vec<PathBuf> = files.to_vec();
    let mut failed_steps = Vec::new();

    for (index, step) in steps.iter().enumerate() {
        if reporter.cancelled() {
            reporter.event(Event::Summary(format!("stopped before {}", step.name())));
            break;
        }

        reporter.event(Event::Stage {
            name: step.name().to_string(),
            index: index + 1,
            of: steps.len(),
        });

        let result = match step {
            Step::Normalize => normalize_step(pipeline.normalize, &mut working, reporter),
            Step::Tag => {
                // Every lookup needs the key, so without one this is a long
                // list of identical failures rather than a tagging run.
                if pipeline.tag.acoustid_key.as_deref().unwrap_or_default().trim().is_empty() {
                    reporter.event(Event::Summary(
                        "skipping tag: no AcoustID key (set ACOUSTID_API_KEY, or pass \
                         --acoustid-key; free from https://acoustid.org/new-application)"
                            .into(),
                    ));
                    continue;
                }
                tag_files(pipeline.tag, &working, reporter)
            }
            Step::Stems => stems_files(pipeline.stems, &working, reporter),
        };

        // One step failing does not cancel the rest: tagging that cannot reach
        // MusicBrainz is no reason to skip separating, and the files that did
        // work are still worth carrying forward.
        if let Err(e) = result {
            report::failed(reporter, Path::new(step.name()), &e);
            failed_steps.push(step.name());
        }
    }

    if failed_steps.is_empty() {
        Ok(())
    } else {
        bail!("{} did not finish cleanly", failed_steps.join(" and "))
    }
}

/// The normalize step of a pipeline, which is not quite the `normalize`
/// command.
///
/// Wav has nowhere to put a ReplayGain tag. On its own that is an error worth
/// stopping for, because tagging wavs is all you asked for. In a pipeline it is
/// not: the file still wants tagging and separating, so the wavs are set aside
/// with one message rather than one error each, and carry on to the next step
/// unchanged.
fn normalize_step(
    args: &NormalizeArgs,
    working: &mut Vec<PathBuf>,
    reporter: &dyn Reporter,
) -> Result<()> {
    if args.mode != NormalizeMode::Replaygain {
        *working = normalize_files(args, working, reporter)?;
        return Ok(());
    }

    let (taggable, wavs): (Vec<PathBuf>, Vec<PathBuf>) =
        working.iter().cloned().partition(|p| Codec::from_path(p) != Some(Codec::Wav));

    if !wavs.is_empty() {
        reporter.event(Event::Summary(format!(
            "{} wav file{} cannot carry ReplayGain tags, so their loudness is unchanged \
             (use --mode reencode to write new files instead)",
            wavs.len(),
            if wavs.len() == 1 { "" } else { "s" },
        )));
    }
    if taggable.is_empty() {
        return Ok(());
    }

    normalize_files(args, &taggable, reporter)?;
    Ok(())
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

// -- rekordbox's own libraries ---------------------------------------------

/// Find the database somebody meant, given a path that may be either the file
/// or the drive it sits on.
fn rekordbox_database(path: &Path) -> Result<PathBuf> {
    if path.is_file() {
        return Ok(path.to_path_buf());
    }
    if let Some(found) = crate::rekordbox::onelibrary::find(path) {
        return Ok(found);
    }
    for candidate in ["master.db", "rekordbox/master.db"] {
        let joined = path.join(candidate);
        if joined.is_file() {
            return Ok(joined);
        }
    }
    bail!("no rekordbox database under {}", path.display())
}

pub fn rekordbox(args: &crate::cli::RekordboxCommand, reporter: &dyn Reporter) -> Result<()> {
    use crate::cli::RekordboxCommand;
    use crate::report::Event;

    let (args, schema_only) = match args {
        RekordboxCommand::Read(args) => (args, false),
        RekordboxCommand::Schema(args) => (args, true),
    };
    let path = rekordbox_database(&args.path)?;
    let key = crate::rekordbox::resolve(args.key.as_deref())?;
    let connection = crate::rekordbox::open(&path, &key)?;
    reporter.event(Event::Heading(format!("{}", path.display())));

    if schema_only {
        let shapes = crate::rekordbox::onelibrary::describe(&connection)?;
        for line in crate::rekordbox::onelibrary::report(&shapes).lines() {
            reporter.event(Event::Line(line.to_string()));
        }
        return Ok(());
    }

    // A OneLibrary drive is a different schema, and reading it as a master.db
    // would report an empty library rather than the wrong one — which is worse,
    // because an empty answer looks like an answer.
    let tables = crate::rekordbox::tables(&connection)?;
    if !tables.iter().any(|(name, _)| name == "djmdContent") {
        reporter.event(Event::Summary(
            "this is not a rekordbox master.db. If it is a OneLibrary drive, nothing here \
             reads it as a library yet — `rekordbox schema` will describe its tables."
                .into(),
        ));
        return Ok(());
    }

    let collection = crate::rekordbox::master::read(&connection)?;
    reporter.event(Event::Line(format!(
        "{} tracks, {} playlists",
        collection.tracks.len(),
        collection.playlists.len()
    )));
    for track in &collection.tracks {
        let mut line = format!("{} — {}", track.artist, track.title);
        if track.bpm > 0.0 {
            line.push_str(&format!("  {:.2}", track.bpm));
        }
        if !track.key.is_empty() {
            line.push_str(&format!("  {}", track.key));
        }
        if !track.cues.is_empty() {
            line.push_str(&format!("  {} cues", track.cues.len()));
        }
        reporter.event(Event::Line(line));
    }
    for playlist in &collection.playlists {
        let where_ = match playlist.folder.is_empty() {
            true => playlist.name.clone(),
            false => format!("{}/{}", playlist.folder, playlist.name),
        };
        reporter.event(Event::Line(format!(
            "playlist {where_} ({} tracks){}",
            playlist.track_ids.len(),
            if playlist.was_smart { " — was smart" } else { "" }
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {

    #[test]
    fn playlists_become_a_tree_of_folders_and_lists() {
        use crate::cli::PlaylistSpec;

        let ids: HashMap<PathBuf, u32> = [("/a.flac", 1u32), ("/b.flac", 2), ("/c.flac", 3)]
            .into_iter()
            .map(|(p, id)| (PathBuf::from(p), id))
            .collect();
        let specs = vec![
            PlaylistSpec {
                name: "warm".into(),
                folder: "Sat 14/9".into(),
                tracks: vec!["/a.flac".into(), "/b.flac".into()],
            },
            PlaylistSpec {
                name: "peak".into(),
                folder: "Sat 14/9".into(),
                tracks: vec!["/c.flac".into()],
            },
            PlaylistSpec {
                name: "promos".into(),
                folder: String::new(),
                tracks: vec!["/a.flac".into()],
            },
        ];

        let rows = playlist_tree(&specs, &ids);
        let folders: Vec<&pdb::Playlist> = rows.iter().filter(|r| r.is_folder).collect();
        assert_eq!(folders.len(), 1, "one folder, named twice");
        assert_eq!(folders[0].name, "Sat 14/9");

        let by_name = |name: &str| rows.iter().find(|r| r.name == name).expect(name);
        assert_eq!(by_name("warm").parent_id, folders[0].id);
        assert_eq!(by_name("peak").parent_id, folders[0].id, "the second list joins the folder");
        assert_eq!(by_name("promos").parent_id, 0, "the top level is not a folder");
        assert_eq!(by_name("warm").track_ids, vec![1, 2]);

        let mut ids: Vec<u32> = rows.iter().map(|r| r.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), rows.len(), "a folder and a list shared an id");
    }

    #[test]
    fn a_track_that_did_not_reach_the_drive_leaves_the_playlist_that_named_it() {
        use crate::cli::PlaylistSpec;

        // The database refuses an entry for a track it has no row for, so one
        // unreadable file has to cost that file rather than the whole export.
        let ids: HashMap<PathBuf, u32> =
            [(PathBuf::from("/good.flac"), 1u32)].into_iter().collect();
        let specs = vec![PlaylistSpec {
            name: "set".into(),
            folder: String::new(),
            tracks: vec!["/good.flac".into(), "/broken.wav".into()],
        }];

        let rows = playlist_tree(&specs, &ids);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].track_ids, vec![1], "the missing one is dropped, the list survives");
    }

    #[test]
    fn a_playlist_whose_tracks_all_failed_is_still_written() {
        use crate::cli::PlaylistSpec;

        // Empty on the player is a truthful answer. Absent looks like the sync
        // forgot it.
        let rows = playlist_tree(
            &[PlaylistSpec {
                name: "set".into(),
                folder: String::new(),
                tracks: vec!["/x.wav".into()],
            }],
            &HashMap::new(),
        );
        assert_eq!(rows.len(), 1);
        assert!(rows[0].track_ids.is_empty());
    }
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
