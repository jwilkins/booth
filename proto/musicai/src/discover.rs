//! Turning command-line paths into a list of audio files.

use std::collections::BTreeSet;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{bail, Result};
use walkdir::WalkDir;

/// Extensions the command-line pipelines take: ones they can decode and write
/// back in the same format.
pub const SUPPORTED: [&str; 7] = ["mp3", "flac", "wav", "m4a", "aac", "aiff", "aif"];

/// Extensions the library takes: everything above, and the formats no player
/// opens that are decoded to be converted or carried onto a drive as an mp3.
///
/// Kept apart from [`SUPPORTED`] because `normalize` writes its output in the
/// format it was given, and has no way to write an Ogg, an Opus or a WMA: a
/// folder with one in it would stop the whole command.
pub const LIBRARY: [&str; 11] =
    ["mp3", "flac", "wav", "m4a", "aac", "aiff", "aif", "ogg", "oga", "opus", "wma"];

pub fn is_supported(path: &Path) -> bool {
    has_extension(path, &SUPPORTED)
}

/// Whether the library takes a file with this extension, whether or not it
/// can decode it on this machine: see [`decodable`].
pub fn is_library(path: &Path) -> bool {
    has_extension(path, &LIBRARY)
}

/// Whether a file with this extension can be decoded here and now: a WMA only
/// when ffmpeg is installed.
pub fn decodable(extension: &str) -> bool {
    let extension = extension.to_ascii_lowercase();
    LIBRARY.contains(&extension.as_str())
        && (!crate::audio::ffmpeg::needed_for(&extension) || crate::audio::ffmpeg::find().is_some())
}

fn has_extension(path: &Path, extensions: &[&str]) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| extensions.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

/// The four bytes an AppleDouble file opens with: RFC 1740's magic number.
///
/// Measured on one written by a Mac, a `._.DS_Store` in a 2009 netlabel zip on
/// archive.org: `00 05 16 07`, then the format's version, `00 02 00 00`.
const APPLE_DOUBLE_MAGIC: [u8; 4] = [0x00, 0x05, 0x16, 0x07];

/// Under this, a file named like an AppleDouble one is taken for one even
/// without its magic number: 64 KiB.
///
/// They hold a file's Finder information and extended attributes, which is
/// bytes to a few kilobytes — the one measured above is 82 bytes — where one
/// second of a 128 kbps MP3 is 16 KiB and a track is megabytes.
pub const APPLE_DOUBLE_SMALL: u64 = 64 * 1024;

/// Whether `path` is named like the `._` companion macOS writes beside every
/// file it touches on a drive that cannot keep a Mac's metadata itself: FAT32,
/// exFAT, a network share.
pub fn named_apple_double(path: &Path) -> bool {
    path.file_name().and_then(|name| name.to_str()).is_some_and(|name| name.starts_with("._"))
}

/// Whether `path` is one of those companions rather than audio.
///
/// It keeps the real file's name, extension and all — `._Sirens.flac` beside
/// `Sirens.flac` — so anything going by the extension takes it for a track: a
/// row that will not play, and a second copy of the record in the duplicate
/// search. The name alone is not enough, since a real file can be called
/// anything; one that also opens with the AppleDouble magic number, or is too
/// small to be a track, holds no audio.
pub fn is_apple_double(path: &Path) -> bool {
    if !named_apple_double(path) {
        return false;
    }
    let mut head = [0u8; 4];
    let opened = std::fs::File::open(path).and_then(|mut file| file.read_exact(&mut head));
    (opened.is_ok() && head == APPLE_DOUBLE_MAGIC)
        || std::fs::metadata(path).is_ok_and(|meta| meta.len() < APPLE_DOUBLE_SMALL)
}

/// Expand `inputs` into a sorted, de-duplicated list of audio files.
///
/// Files named explicitly are taken as given, even if the extension is
/// unfamiliar — if you point at a file you mean it. Directories are scanned,
/// and there only recognised extensions are picked up. Either way a macOS `._`
/// companion is left out: there is no audio in one however it is asked for.
pub fn collect(inputs: &[PathBuf], recursive: bool) -> Result<Vec<PathBuf>> {
    collect_of(inputs, recursive, &SUPPORTED)
}

/// The same, for the library: every format it takes, including the ones it
/// can only convert. See [`LIBRARY`].
pub fn collect_library(inputs: &[PathBuf], recursive: bool) -> Result<Vec<PathBuf>> {
    collect_of(inputs, recursive, &LIBRARY)
}

fn collect_of(inputs: &[PathBuf], recursive: bool, extensions: &[&str]) -> Result<Vec<PathBuf>> {
    let mut found = BTreeSet::new();

    for input in inputs {
        if input.is_file() {
            if !is_apple_double(input) {
                found.insert(input.clone());
            }
        } else if input.is_dir() {
            let walker =
                if recursive { WalkDir::new(input) } else { WalkDir::new(input).max_depth(1) };
            for entry in walker.into_iter().filter_map(|e| e.ok()) {
                if entry.file_type().is_file()
                    && has_extension(entry.path(), extensions)
                    && !is_apple_double(entry.path())
                {
                    found.insert(entry.into_path());
                }
            }
        } else {
            bail!("{} does not exist", input.display());
        }
    }

    if found.is_empty() {
        bail!("no audio files found (looked for {})", extensions.join(", "));
    }
    Ok(found.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempTree(PathBuf);

    impl TempTree {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir().join(format!("musicai-discover-{name}"));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(root.join("nested")).unwrap();
            for path in [
                root.join("a.mp3"),
                root.join("b.FLAC"),
                root.join("notes.txt"),
                root.join("nested").join("c.wav"),
            ] {
                std::fs::write(path, b"").unwrap();
            }
            Self(root)
        }
    }

    impl Drop for TempTree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn recognises_extensions_case_insensitively() {
        assert!(is_supported(Path::new("x.MP3")));
        assert!(is_supported(Path::new("x.flac")));
        assert!(!is_supported(Path::new("x.ogg")));
        assert!(!is_supported(Path::new("x")));
    }

    #[test]
    fn the_library_takes_the_formats_no_player_opens_and_the_pipelines_do_not() {
        for name in ["x.ogg", "x.OPUS", "x.oga", "x.wma"] {
            assert!(is_library(Path::new(name)), "{name}");
            assert!(!is_supported(Path::new(name)), "{name}");
        }
        assert!(decodable("ogg") && decodable("Opus"));
        assert_eq!(decodable("wma"), crate::audio::ffmpeg::find().is_some());
        assert!(!decodable("ape"));
    }

    #[test]
    fn scans_a_directory_without_descending() {
        let tree = TempTree::new("shallow");
        let files = collect(std::slice::from_ref(&tree.0), false).unwrap();
        assert_eq!(files.len(), 2, "{files:?}");
        assert!(files.iter().all(|f| f.parent() == Some(tree.0.as_path())));
    }

    #[test]
    fn descends_when_asked() {
        let tree = TempTree::new("deep");
        let files = collect(std::slice::from_ref(&tree.0), true).unwrap();
        assert_eq!(files.len(), 3, "{files:?}");
    }

    #[test]
    fn takes_named_files_regardless_of_extension() {
        let tree = TempTree::new("explicit");
        let files = collect(&[tree.0.join("notes.txt")], false).unwrap();
        assert_eq!(files, vec![tree.0.join("notes.txt")]);
    }

    #[test]
    fn deduplicates_overlapping_inputs() {
        let tree = TempTree::new("dupes");
        let files = collect(&[tree.0.clone(), tree.0.join("a.mp3")], false).unwrap();
        assert_eq!(files.len(), 2);
    }

    /// The companion macOS writes beside `name`: the AppleDouble header and
    /// some Finder information, as a Mac leaves on a FAT32 or exFAT stick.
    fn companion(dir: &Path, name: &str) -> PathBuf {
        let mut bytes = vec![0x00, 0x05, 0x16, 0x07, 0x00, 0x02, 0x00, 0x00];
        bytes.resize(4096, 0);
        let path = dir.join(format!("._{name}"));
        std::fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn a_macos_companion_beside_a_track_is_not_picked_up() {
        let tree = TempTree::new("companions");
        companion(&tree.0, "a.mp3");
        companion(&tree.0.join("nested"), "c.wav");
        let files = collect(std::slice::from_ref(&tree.0), true).unwrap();
        assert_eq!(files.len(), 3, "{files:?}");
        assert!(files.iter().all(|f| !named_apple_double(f)), "{files:?}");
    }

    #[test]
    fn a_companion_named_on_its_own_is_not_taken_either() {
        let tree = TempTree::new("named-companion");
        let path = companion(&tree.0, "a.mp3");
        let err = collect(&[path], false).unwrap_err();
        assert!(err.to_string().contains("no audio files"), "{err}");
    }

    #[test]
    fn a_small_file_named_like_a_companion_is_one_without_its_magic_number() {
        let tree = TempTree::new("small-companion");
        let path = tree.0.join("._b.mp3");
        std::fs::write(&path, [0u8; 82]).unwrap();
        assert!(is_apple_double(&path));
    }

    #[test]
    fn a_track_whose_name_happens_to_start_with_dot_underscore_is_kept() {
        let tree = TempTree::new("real-dot-underscore");
        let path = tree.0.join("._intro.mp3");
        std::fs::write(&path, vec![0xffu8; 200 * 1024]).unwrap();
        assert!(!is_apple_double(&path));
        assert!(collect(std::slice::from_ref(&tree.0), false).unwrap().contains(&path));
    }

    #[test]
    fn errors_on_a_missing_path() {
        let err = collect(&[PathBuf::from("/no/such/place")], false).unwrap_err();
        assert!(err.to_string().contains("does not exist"));
    }

    #[test]
    fn errors_when_a_directory_holds_no_audio() {
        let root = std::env::temp_dir().join("musicai-discover-barren");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let err = collect(std::slice::from_ref(&root), true).unwrap_err();
        assert!(err.to_string().contains("no audio files"));
        std::fs::remove_dir_all(&root).unwrap();
    }
}
