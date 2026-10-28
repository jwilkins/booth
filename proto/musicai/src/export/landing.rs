//! Putting a file on a drive so that it is really there, and saying so when the
//! drive will not have it.
//!
//! Three things the plain library calls do not do. A file is written beside its
//! name and renamed into place, so a write that dies partway leaves the file
//! that was there before rather than half of a new one. It is flushed to the
//! stick and read back off the stick, not out of the memory that still holds
//! what was written — a read straight after a write is answered from the page
//! cache, and a stick that is failing passes that every time. And a failure on
//! the drive's side is told apart from a failure on the laptop's, because the
//! advice is different: a file that is not where the collection says is the
//! collection's to fix, and an input/output error is the stick or its port.

use std::fs::File;
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};

/// How a drive failed a write.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    /// The drive is not there any more: pulled out, or dropped off the bus.
    Gone,
    /// It ran out of room.
    Full,
    /// It will not be written to: locked, or mounted read-only after an error.
    ReadOnly,
    /// What came back off it is not what was put on it.
    Mismatch,
    /// Anything else it said, an input/output error above all.
    Failing,
}

/// A drive that would not take a file. Carried as an error so that a caller
/// can tell it from a file of its own that would not read.
#[derive(Debug)]
pub struct DriveFault {
    pub fault: Fault,
    /// The file on the drive it happened to.
    pub path: PathBuf,
    pub detail: String,
}

impl std::fmt::Display for DriveFault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let what = match self.fault {
            Fault::Gone => "the drive is not there any more — pulled out, or it dropped off the bus",
            Fault::Full => "the drive is full",
            Fault::ReadOnly => {
                "the drive will not be written to — locked, or the system made it read-only after an \
                 error"
            }
            Fault::Mismatch => "what was read back off the drive is not what was written to it",
            Fault::Failing => {
                "the drive failed the write — that is the stick or its port, not the music; try \
                 another port or another stick"
            }
        };
        write!(f, "{what}: {} ({})", self.path.display(), self.detail)
    }
}

impl std::error::Error for DriveFault {}

impl DriveFault {
    /// Name an error the drive gave. `root` is the drive, which is how one
    /// that has gone is told from one that refused.
    pub fn from_io(root: &Path, path: &Path, error: &std::io::Error) -> Self {
        let fault = match error.kind() {
            _ if !root.exists() => Fault::Gone,
            ErrorKind::StorageFull => Fault::Full,
            ErrorKind::ReadOnlyFilesystem => Fault::ReadOnly,
            _ => Fault::Failing,
        };
        Self { fault, path: path.to_path_buf(), detail: error.to_string() }
    }
}

/// Write `bytes` to `out` on the drive at `root`, and read them back off it.
pub fn put(root: &Path, out: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    land(root, out, |file| file.write_all(bytes).map_err(Side::Drive))?;
    let back = read_from_device(out).map_err(|e| DriveFault::from_io(root, out, &e))?;
    if back != bytes {
        let at =
            back.iter().zip(bytes).position(|(a, b)| a != b).unwrap_or(back.len().min(bytes.len()));
        return Err(mismatch(out, at, back.len() as u64, bytes.len() as u64).into());
    }
    Ok(())
}

/// Copy the file at `from` to `out` on the drive at `root`, and compare what
/// is on the drive with it.
pub fn copy(root: &Path, from: &Path, out: &Path) -> anyhow::Result<()> {
    let mut source = File::open(from).map_err(|e| source_error(from, e))?;
    land(root, out, |file| {
        let mut buffer = vec![0u8; CHUNK];
        loop {
            let read = source.read(&mut buffer).map_err(Side::Source)?;
            if read == 0 {
                return Ok(());
            }
            file.write_all(&buffer[..read]).map_err(Side::Drive)?;
        }
    })
    .map_err(|e| match e.downcast::<SourceError>() {
        Ok(source) => source_error(from, source.0),
        Err(other) => other,
    })?;
    compare(root, from, out)
}

/// Read a file off the device it is on rather than out of the page cache,
/// which still holds whatever was just written to it.
pub fn read_from_device(path: &Path) -> std::io::Result<Vec<u8>> {
    let mut file = uncached(path)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// How much is copied and compared at a time.
const CHUNK: usize = 1 << 20;

/// Which end of a copy failed.
enum Side {
    Source(std::io::Error),
    Drive(std::io::Error),
}

/// A source file that would not read, carried out of [`land`] so that it is
/// not taken for the drive's fault.
#[derive(Debug)]
struct SourceError(std::io::Error);

impl std::fmt::Display for SourceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl std::error::Error for SourceError {}

fn source_error(from: &Path, error: std::io::Error) -> anyhow::Error {
    match error.kind() {
        ErrorKind::NotFound => {
            anyhow::anyhow!("{} is not where it was: {error}", from.display())
        }
        _ => anyhow::anyhow!("reading {}: {error}", from.display()),
    }
}

/// Write a file beside `out` with `fill`, flush it to the device, and rename
/// it into place.
fn land(
    root: &Path,
    out: &Path,
    fill: impl FnOnce(&mut File) -> Result<(), Side>,
) -> anyhow::Result<()> {
    let drive = |e: std::io::Error| anyhow::Error::from(DriveFault::from_io(root, out, &e));
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent).map_err(drive)?;
    }
    let partial = partial_of(out);
    let mut file = File::create(&partial).map_err(drive)?;
    let filled = fill(&mut file).and_then(|()| file.sync_all().map_err(Side::Drive));
    drop(file);
    match filled {
        Ok(()) => {}
        Err(side) => {
            let _ = std::fs::remove_file(&partial);
            return Err(match side {
                Side::Source(e) => SourceError(e).into(),
                Side::Drive(e) => drive(e),
            });
        }
    }
    std::fs::rename(&partial, out).map_err(drive)
}

/// Where a file is written before it is renamed to `out`: beside it, so the
/// rename never crosses a filesystem.
fn partial_of(out: &Path) -> PathBuf {
    let mut name = out.file_name().unwrap_or_default().to_os_string();
    name.push(".partial");
    out.with_file_name(name)
}

/// Whether the file at `out` on the drive holds what `from` does, read off
/// the device.
fn compare(root: &Path, from: &Path, out: &Path) -> anyhow::Result<()> {
    let drive = |e: std::io::Error| anyhow::Error::from(DriveFault::from_io(root, out, &e));
    let mut source = File::open(from).map_err(|e| source_error(from, e))?;
    let mut landed = uncached(out).map_err(drive)?;
    let (mut a, mut b) = (vec![0u8; CHUNK], vec![0u8; CHUNK]);
    let mut at = 0u64;
    loop {
        let read = fill(&mut source, &mut a).map_err(|e| source_error(from, e))?;
        let back = fill(&mut landed, &mut b).map_err(drive)?;
        if a[..read] != b[..back] {
            let offset = a[..read].iter().zip(&b[..back]).position(|(x, y)| x != y);
            let offset = offset.unwrap_or(read.min(back)) as u64;
            let (from_len, out_len) = (len_of(from), len_of(out));
            return Err(mismatch(out, (at + offset) as usize, out_len, from_len).into());
        }
        if read == 0 {
            return Ok(());
        }
        at += read as u64;
    }
}

fn len_of(path: &Path) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

/// Read until `buffer` is full or the file ends, so two files read side by
/// side stay in step however each one's reads come back.
fn fill(file: &mut File, buffer: &mut [u8]) -> std::io::Result<usize> {
    let mut filled = 0;
    while filled < buffer.len() {
        match file.read(&mut buffer[filled..])? {
            0 => break,
            read => filled += read,
        }
    }
    Ok(filled)
}

fn mismatch(out: &Path, at: usize, landed: u64, meant: u64) -> DriveFault {
    DriveFault {
        fault: Fault::Mismatch,
        path: out.to_path_buf(),
        detail: format!("first different byte at {at}; {landed} bytes on the drive, {meant} meant"),
    }
}

/// Open a file for reading past the page cache: flushed to the device, and its
/// cached pages dropped, so that what is read is what the device holds.
fn uncached(path: &Path) -> std::io::Result<File> {
    let file = File::open(path)?;
    file.sync_all()?;
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::io::AsRawFd;
        // SAFETY: the descriptor is open for the length of the call. Advice is
        // only advice, so a refusal is not an error: the read then comes from
        // the cache, which is what happened before.
        unsafe {
            libc::posix_fadvise(file.as_raw_fd(), 0, 0, libc::POSIX_FADV_DONTNEED);
        }
    }
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::io::AsRawFd;
        // SAFETY: as above. Reads on this descriptor bypass the cache.
        unsafe {
            libc::fcntl(file.as_raw_fd(), libc::F_NOCACHE, 1);
        }
    }
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("musicai-landing-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_copy_lands_under_its_own_name_and_leaves_nothing_beside_it() {
        let dir = scratch("copy");
        let from = dir.join("source.flac");
        std::fs::write(&from, vec![7u8; 3 * CHUNK + 5]).unwrap();
        let drive = dir.join("USB");
        let out = drive.join("Contents").join("source.flac");
        copy(&drive, &from, &out).unwrap();
        assert_eq!(std::fs::read(&out).unwrap(), std::fs::read(&from).unwrap());
        assert!(!partial_of(&out).exists(), "the partial file was left behind");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_file_on_the_drive_that_differs_from_its_source_is_named_with_where() {
        let dir = scratch("differs");
        let from = dir.join("source.flac");
        std::fs::write(&from, b"abcdefgh").unwrap();
        let out = dir.join("landed.flac");
        std::fs::write(&out, b"abcdXfgh").unwrap();
        let error = compare(&dir, &from, &out).unwrap_err();
        let fault = error.downcast_ref::<DriveFault>().expect("not taken for the drive's fault");
        assert_eq!(fault.fault, Fault::Mismatch);
        assert!(fault.detail.contains("byte at 4"), "{}", fault.detail);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_shorter_file_on_the_drive_is_a_mismatch_too() {
        let dir = scratch("short");
        let from = dir.join("source.flac");
        std::fs::write(&from, vec![1u8; CHUNK + 10]).unwrap();
        let out = dir.join("landed.flac");
        std::fs::write(&out, vec![1u8; CHUNK]).unwrap();
        let error = compare(&dir, &from, &out).unwrap_err();
        assert_eq!(error.downcast_ref::<DriveFault>().unwrap().fault, Fault::Mismatch);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_source_that_is_not_there_is_the_collections_problem_not_the_drives() {
        let dir = scratch("missing");
        let error = copy(&dir, &dir.join("gone.flac"), &dir.join("USB/gone.flac")).unwrap_err();
        assert!(error.downcast_ref::<DriveFault>().is_none(), "{error:#}");
        assert!(format!("{error:#}").contains("is not where it was"), "{error:#}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    // The error numbers are Unix's; Windows numbers its errors differently.
    #[cfg(unix)]
    #[test]
    fn a_drive_error_is_named_for_what_it_means() {
        let dir = scratch("faults");
        let at = dir.join("x");
        let io = |raw: i32| std::io::Error::from_raw_os_error(raw);
        let fault = |e: &std::io::Error| DriveFault::from_io(&dir, &at, e).fault;
        assert_eq!(fault(&io(libc::EIO)), Fault::Failing);
        assert_eq!(fault(&io(libc::ENOSPC)), Fault::Full);
        assert_eq!(fault(&io(libc::EROFS)), Fault::ReadOnly);
        // A drive whose folder has gone has gone, whatever the error said.
        let gone = DriveFault::from_io(&dir.join("not-there"), &at, &io(libc::EIO)).fault;
        assert_eq!(gone, Fault::Gone);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn written_bytes_are_read_back_and_land_whole() {
        let dir = scratch("put");
        let out = dir.join("PIONEER").join("export.pdb");
        put(&dir, &out, b"a database").unwrap();
        assert_eq!(read_from_device(&out).unwrap(), b"a database");
        assert!(!partial_of(&out).exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
