//! Writing a drive as a disk image rather than into a directory.
//!
//! A player does not read a folder on your laptop; it reads a partitioned
//! block device with a FAT filesystem on it. Producing that directly is worth
//! doing for two reasons: it can be written to a stick with `dd` in one step,
//! and it can be handed straight to an emulator's USB slot, which is the
//! closest thing to a CDJ that exists without a CDJ.
//!
//! The layout is the one a rekordbox-formatted stick has: a master boot record
//! with a single FAT32 partition starting at the usual one-megabyte boundary.
//! FAT32 rather than exFAT because it is what every player back to 2009 reads,
//! and alignment at 2048 sectors because that is what everything else does.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::Mutex;

use anyhow::{bail, Context, Result};
use fatfs::{FatType, FileSystem, FormatVolumeOptions, FsOptions};

/// Everything here counts in 512-byte sectors, as partition tables do.
const SECTOR: u64 = 512;
/// Where the first partition starts. One megabyte in, which is what every
/// partitioning tool has done since drives stopped having real cylinders.
const FIRST_PARTITION_LBA: u64 = 2_048;
/// MBR partition type 0x0c: FAT32 with LBA addressing.
const TYPE_FAT32_LBA: u8 = 0x0c;
/// The smallest image worth making. FAT32 needs 65,525 clusters before it is
/// FAT32 at all, and a drive this size leaves room for the filesystem's own
/// overhead without arithmetic.
const MIN_CAPACITY: u64 = 64 * 1024 * 1024;

/// How big an image has to be to hold `content` bytes of files.
///
/// A tenth over, for the directory entries, the file allocation tables and the
/// slack every file leaves in its last cluster, rounded up to a megabyte.
pub fn capacity_for(content: u64) -> u64 {
    let wanted = content + content / 10 + FIRST_PARTITION_LBA * SECTOR;
    let megabyte = 1024 * 1024;
    wanted.next_multiple_of(megabyte).max(MIN_CAPACITY)
}

/// A disk image being written.
pub struct DriveImage {
    fs: FileSystem<Partition>,
}

impl DriveImage {
    /// Create an image of `capacity` bytes and format its partition.
    ///
    /// Anything already at `path` is replaced, so callers that care should ask
    /// first — this is the one destructive operation in the module.
    pub fn create(path: &Path, capacity: u64, label: &str) -> Result<Self> {
        if capacity < MIN_CAPACITY {
            bail!("{capacity} bytes is too small for a FAT32 drive");
        }
        let sectors = capacity / SECTOR;

        let file = File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)
            .with_context(|| format!("creating {}", path.display()))?;
        file.set_len(sectors * SECTOR)?;

        let mut image = file.try_clone()?;
        image.write_all(&master_boot_record(sectors))?;
        image.flush()?;

        let base = FIRST_PARTITION_LBA * SECTOR;
        let length = sectors * SECTOR - base;
        let options =
            FormatVolumeOptions::new().fat_type(FatType::Fat32).volume_label(volume_label(label));
        fatfs::format_volume(Partition::new(file.try_clone()?, base, length), options)
            .with_context(|| format!("formatting the partition in {}", path.display()))?;

        let fs = FileSystem::new(Partition::new(file, base, length), FsOptions::new())
            .with_context(|| format!("opening the filesystem in {}", path.display()))?;
        Ok(Self { fs })
    }

    /// Open an image that already exists, for reading it back.
    pub fn open(path: &Path) -> Result<Self> {
        let file = File::options()
            .read(true)
            .write(true)
            .open(path)
            .with_context(|| format!("opening {}", path.display()))?;
        let length = file.metadata()?.len();
        let base = FIRST_PARTITION_LBA * SECTOR;
        if length <= base {
            bail!("{} is too small to hold a partition", path.display());
        }
        let fs = FileSystem::new(Partition::new(file, base, length - base), FsOptions::new())
            .with_context(|| format!("reading the filesystem in {}", path.display()))?;
        Ok(Self { fs })
    }

    /// Write a file at a path the player will see, e.g.
    /// `/PIONEER/rekordbox/export.pdb`. Parent directories are made as needed.
    pub fn write(&self, on_drive: &str, bytes: &[u8]) -> Result<()> {
        let mut file = self.create_file(on_drive)?;
        file.write_all(bytes).with_context(|| format!("writing {on_drive}"))?;
        file.flush()?;
        Ok(())
    }

    /// Copy a file from the host into the image, without holding it in memory.
    pub fn copy_in(&self, on_drive: &str, from: &Path) -> Result<u64> {
        let mut source = File::open(from).with_context(|| format!("reading {}", from.display()))?;
        let mut destination = self.create_file(on_drive)?;
        let copied = std::io::copy(&mut source, &mut destination)
            .with_context(|| format!("copying {} into the image", from.display()))?;
        destination.flush()?;
        Ok(copied)
    }

    /// Read a file back out.
    pub fn read(&self, on_drive: &str) -> Result<Vec<u8>> {
        let mut file = self
            .fs
            .root_dir()
            .open_file(on_drive.trim_start_matches('/'))
            .with_context(|| format!("{on_drive} is not on the drive"))?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).with_context(|| format!("reading {on_drive}"))?;
        Ok(bytes)
    }

    /// Everything in a directory, by name.
    pub fn list(&self, on_drive: &str) -> Result<Vec<String>> {
        let trimmed = on_drive.trim_start_matches('/').trim_end_matches('/');
        let dir = if trimmed.is_empty() {
            self.fs.root_dir()
        } else {
            self.fs
                .root_dir()
                .open_dir(trimmed)
                .with_context(|| format!("{on_drive} is not a directory on the drive"))?
        };
        let mut names = Vec::new();
        for entry in dir.iter() {
            let entry = entry?;
            let name = entry.file_name();
            if name != "." && name != ".." {
                names.push(name);
            }
        }
        names.sort();
        Ok(names)
    }

    /// Flush everything and close the image.
    pub fn finish(self) -> Result<()> {
        self.fs.unmount().context("closing the filesystem")?;
        Ok(())
    }

    fn create_file(&self, on_drive: &str) -> Result<fatfs::File<'_, Partition>> {
        let path = on_drive.trim_start_matches('/');
        let root = self.fs.root_dir();
        // Directories have to exist before a file inside one can be made, and
        // only the last component of a path is created for you.
        if let Some((parents, _)) = path.rsplit_once('/') {
            let mut so_far = String::new();
            for part in parents.split('/') {
                if !so_far.is_empty() {
                    so_far.push('/');
                }
                so_far.push_str(part);
                root.create_dir(&so_far)
                    .with_context(|| format!("creating the folder {so_far} on the drive"))?;
            }
        }
        root.create_file(path).with_context(|| format!("creating {on_drive}"))
    }
}

/// A FAT volume label: eleven bytes, upper case, padded with spaces.
fn volume_label(label: &str) -> [u8; 11] {
    let mut out = [b' '; 11];
    for (slot, byte) in out.iter_mut().zip(
        label
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
            .map(|c| c.to_ascii_uppercase() as u8),
    ) {
        *slot = byte;
    }
    out
}

/// A master boot record with one FAT32 partition filling the disk.
fn master_boot_record(sectors: u64) -> [u8; 512] {
    let mut mbr = [0u8; 512];
    let count = (sectors - FIRST_PARTITION_LBA).min(u32::MAX as u64) as u32;

    let entry = 0x1be;
    mbr[entry] = 0x00; // not bootable; a player does not boot from the stick
    mbr[entry + 1..entry + 4].copy_from_slice(&chs(FIRST_PARTITION_LBA));
    mbr[entry + 4] = TYPE_FAT32_LBA;
    mbr[entry + 5..entry + 8].copy_from_slice(&chs(FIRST_PARTITION_LBA + count as u64 - 1));
    mbr[entry + 8..entry + 12].copy_from_slice(&(FIRST_PARTITION_LBA as u32).to_le_bytes());
    mbr[entry + 12..entry + 16].copy_from_slice(&count.to_le_bytes());

    mbr[510] = 0x55;
    mbr[511] = 0xaa;
    mbr
}

/// The cylinder/head/sector form of a block address, for the three bytes of
/// each partition entry that predate anything caring. Addresses past what the
/// geometry can express are pinned to the maximum, which is what every tool
/// does and what every reader ignores.
fn chs(lba: u64) -> [u8; 3] {
    const HEADS: u64 = 255;
    const SECTORS: u64 = 63;
    let cylinder = lba / (HEADS * SECTORS);
    if cylinder > 1023 {
        return [0xfe, 0xff, 0xff];
    }
    let head = (lba / SECTORS) % HEADS;
    let sector = (lba % SECTORS) + 1;
    [head as u8, (sector as u8) | (((cylinder >> 2) as u8) & 0xc0), cylinder as u8]
}

/// A window onto part of a file, so the filesystem inside a partition cannot
/// see — or scribble on — the partition table in front of it.
struct Partition {
    file: File,
    base: u64,
    length: u64,
    at: u64,
}

impl Partition {
    fn new(file: File, base: u64, length: u64) -> Self {
        Self { file, base, length, at: 0 }
    }
}

impl Read for Partition {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let remaining = self.length.saturating_sub(self.at) as usize;
        let take = buffer.len().min(remaining);
        if take == 0 {
            return Ok(0);
        }
        self.file.seek(SeekFrom::Start(self.base + self.at))?;
        let read = self.file.read(&mut buffer[..take])?;
        self.at += read as u64;
        Ok(read)
    }
}

impl Write for Partition {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        let remaining = self.length.saturating_sub(self.at) as usize;
        let take = buffer.len().min(remaining);
        if take == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::WriteZero,
                "the partition is full",
            ));
        }
        self.file.seek(SeekFrom::Start(self.base + self.at))?;
        let written = self.file.write(&buffer[..take])?;
        self.at += written as u64;
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

impl Seek for Partition {
    fn seek(&mut self, to: SeekFrom) -> std::io::Result<u64> {
        let at = match to {
            SeekFrom::Start(offset) => offset as i64,
            SeekFrom::End(offset) => self.length as i64 + offset,
            SeekFrom::Current(offset) => self.at as i64 + offset,
        };
        if at < 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "seek before the start of the partition",
            ));
        }
        self.at = at as u64;
        Ok(self.at)
    }
}

/// Somewhere for a drive to be written: a folder, or an image.
///
/// The two are interchangeable to everything upstream, which is what lets the
/// same export be tried in an emulator and then written to a stick without a
/// second code path deciding what goes where.
pub enum Destination {
    Directory(std::path::PathBuf),
    Image(Mutex<DriveImage>),
}

impl Destination {
    pub fn write(&self, on_drive: &str, bytes: &[u8]) -> Result<()> {
        match self {
            Destination::Directory(root) => {
                let out = root.join(on_drive.trim_start_matches('/'));
                if let Some(parent) = out.parent() {
                    std::fs::create_dir_all(parent)
                        .with_context(|| format!("creating {}", parent.display()))?;
                }
                std::fs::write(&out, bytes).with_context(|| format!("writing {}", out.display()))
            }
            Destination::Image(image) => image.lock().unwrap().write(on_drive, bytes),
        }
    }

    pub fn copy_in(&self, on_drive: &str, from: &Path) -> Result<()> {
        match self {
            Destination::Directory(root) => {
                let out = root.join(on_drive.trim_start_matches('/'));
                if let Some(parent) = out.parent() {
                    std::fs::create_dir_all(parent)
                        .with_context(|| format!("creating {}", parent.display()))?;
                }
                std::fs::copy(from, &out)
                    .with_context(|| format!("copying {} to the drive", from.display()))?;
                Ok(())
            }
            Destination::Image(image) => {
                image.lock().unwrap().copy_in(on_drive, from)?;
                Ok(())
            }
        }
    }

    /// Read a file back off the drive, which is how an export checks itself.
    pub fn read(&self, on_drive: &str) -> Result<Vec<u8>> {
        match self {
            Destination::Directory(root) => {
                let at = root.join(on_drive.trim_start_matches('/'));
                std::fs::read(&at).with_context(|| format!("reading back {}", at.display()))
            }
            Destination::Image(image) => image.lock().unwrap().read(on_drive),
        }
    }

    pub fn finish(self) -> Result<()> {
        match self {
            Destination::Directory(_) => Ok(()),
            Destination::Image(image) => image.into_inner().unwrap().finish(),
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Destination::Directory(root) => root.display().to_string(),
            Destination::Image(_) => "the image".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Scratch(std::path::PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("musicai-image-{name}"));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn path(&self, name: &str) -> std::path::PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn capacity_is_generous_but_never_tiny() {
        assert_eq!(capacity_for(0), MIN_CAPACITY);
        assert_eq!(capacity_for(1_000), MIN_CAPACITY);
        let big = capacity_for(900 * 1024 * 1024);
        assert!(big > 990 * 1024 * 1024, "{big} leaves no room for the filesystem");
        assert_eq!(big % (1024 * 1024), 0, "images are a whole number of megabytes");
    }

    #[test]
    fn the_partition_table_says_what_a_reader_expects() {
        let scratch = Scratch::new("mbr");
        let path = scratch.path("drive.img");
        DriveImage::create(&path, MIN_CAPACITY, "REKORDBOX").unwrap().finish().unwrap();

        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(bytes.len() as u64, MIN_CAPACITY);
        assert_eq!(&bytes[510..512], &[0x55, 0xaa], "no boot signature");
        assert_eq!(bytes[0x1be + 4], TYPE_FAT32_LBA);
        assert_eq!(
            u32::from_le_bytes(bytes[0x1be + 8..0x1be + 12].try_into().unwrap()),
            FIRST_PARTITION_LBA as u32
        );
        assert_eq!(
            u32::from_le_bytes(bytes[0x1be + 12..0x1be + 16].try_into().unwrap()) as u64,
            MIN_CAPACITY / SECTOR - FIRST_PARTITION_LBA
        );
        // The three other partition entries are empty.
        assert!(bytes[0x1ce..0x1fe].iter().all(|&b| b == 0));
    }

    #[test]
    fn the_partition_holds_a_fat32_filesystem() {
        let scratch = Scratch::new("fat");
        let path = scratch.path("drive.img");
        DriveImage::create(&path, MIN_CAPACITY, "REKORDBOX").unwrap().finish().unwrap();

        let bytes = std::fs::read(&path).unwrap();
        let boot = (FIRST_PARTITION_LBA * SECTOR) as usize;
        // The boot sector of the partition, not of the disk.
        assert_eq!(&bytes[boot + 510..boot + 512], &[0x55, 0xaa]);
        // FAT32 names itself in its extended boot record.
        assert_eq!(&bytes[boot + 0x52..boot + 0x57], b"FAT32");
        assert_eq!(&bytes[boot + 0x47..boot + 0x52], b"REKORDBOX  ");
    }

    #[test]
    fn a_file_written_into_the_image_reads_back_the_same() {
        let scratch = Scratch::new("roundtrip");
        let path = scratch.path("drive.img");
        let contents: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8).collect();

        let image = DriveImage::create(&path, MIN_CAPACITY, "REKORDBOX").unwrap();
        image.write("/PIONEER/rekordbox/export.pdb", &contents).unwrap();
        image.finish().unwrap();

        let reopened = DriveImage::open(&path).unwrap();
        assert_eq!(reopened.read("/PIONEER/rekordbox/export.pdb").unwrap(), contents);
    }

    #[test]
    fn folders_are_made_as_deep_as_the_path_goes() {
        let scratch = Scratch::new("folders");
        let path = scratch.path("drive.img");

        let image = DriveImage::create(&path, MIN_CAPACITY, "USB").unwrap();
        image.write("/PIONEER/USBANLZ/P001/00000001/ANLZ0000.DAT", b"analysis").unwrap();
        image.write("/PIONEER/USBANLZ/P001/00000002/ANLZ0000.DAT", b"another").unwrap();
        image.finish().unwrap();

        let reopened = DriveImage::open(&path).unwrap();
        assert_eq!(reopened.list("/PIONEER").unwrap(), ["USBANLZ"]);
        assert_eq!(reopened.list("/PIONEER/USBANLZ/P001").unwrap(), ["00000001", "00000002"]);
        assert_eq!(
            reopened.read("/PIONEER/USBANLZ/P001/00000002/ANLZ0000.DAT").unwrap(),
            b"another"
        );
    }

    #[test]
    fn a_long_file_name_survives() {
        let scratch = Scratch::new("longname");
        let path = scratch.path("drive.img");
        let name = "/Contents/Peverelist/Roll With The Punches (original mix).flac";

        let image = DriveImage::create(&path, MIN_CAPACITY, "USB").unwrap();
        image.write(name, b"audio").unwrap();
        image.finish().unwrap();

        let reopened = DriveImage::open(&path).unwrap();
        assert_eq!(
            reopened.list("/Contents/Peverelist").unwrap(),
            ["Roll With The Punches (original mix).flac"]
        );
    }

    #[test]
    fn copying_a_file_in_does_not_change_it() {
        let scratch = Scratch::new("copy");
        let source = scratch.path("track.flac");
        let contents: Vec<u8> = (0..300_000u32).map(|i| (i % 97) as u8).collect();
        std::fs::write(&source, &contents).unwrap();

        let path = scratch.path("drive.img");
        let image = DriveImage::create(&path, MIN_CAPACITY, "USB").unwrap();
        let copied = image.copy_in("/Contents/track.flac", &source).unwrap();
        image.finish().unwrap();

        assert_eq!(copied as usize, contents.len());
        assert_eq!(
            DriveImage::open(&path).unwrap().read("/Contents/track.flac").unwrap(),
            contents
        );
    }

    #[test]
    fn an_image_too_small_for_fat32_is_refused() {
        let scratch = Scratch::new("tiny");
        let made = DriveImage::create(&scratch.path("drive.img"), 1024, "USB");
        let error = match made {
            Ok(_) => panic!("a 1 kB FAT32 drive was accepted"),
            Err(e) => e.to_string(),
        };
        assert!(error.contains("too small"), "{error}");
    }

    #[test]
    fn a_missing_file_is_reported_by_name() {
        let scratch = Scratch::new("missing");
        let path = scratch.path("drive.img");
        DriveImage::create(&path, MIN_CAPACITY, "USB").unwrap().finish().unwrap();

        let error = DriveImage::open(&path).unwrap().read("/PIONEER/rekordbox/export.pdb");
        assert!(error.unwrap_err().to_string().contains("export.pdb"));
    }
}
