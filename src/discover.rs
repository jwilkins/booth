//! Turning command-line paths into a list of audio files.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use anyhow::{bail, Result};
use walkdir::WalkDir;

/// Extensions we can decode.
pub const SUPPORTED: [&str; 3] = ["mp3", "flac", "wav"];

pub fn is_supported(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| SUPPORTED.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

/// Expand `inputs` into a sorted, de-duplicated list of audio files.
///
/// Files named explicitly are taken as given, even if the extension is
/// unfamiliar — if you point at a file you mean it. Directories are scanned,
/// and there only recognised extensions are picked up.
pub fn collect(inputs: &[PathBuf], recursive: bool) -> Result<Vec<PathBuf>> {
    let mut found = BTreeSet::new();

    for input in inputs {
        if input.is_file() {
            found.insert(input.clone());
        } else if input.is_dir() {
            let walker =
                if recursive { WalkDir::new(input) } else { WalkDir::new(input).max_depth(1) };
            for entry in walker.into_iter().filter_map(|e| e.ok()) {
                if entry.file_type().is_file() && is_supported(entry.path()) {
                    found.insert(entry.into_path());
                }
            }
        } else {
            bail!("{} does not exist", input.display());
        }
    }

    if found.is_empty() {
        bail!("no audio files found (looked for {})", SUPPORTED.join(", "));
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
