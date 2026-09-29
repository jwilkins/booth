//! How fast the disk under a track is going to answer.
//!
//! A collection is a list of paths, and two paths that look alike can be
//! seconds apart to open. A file macOS has quarantined is checked by Gatekeeper
//! on every open; a file in a folder a sync client owns may not be on the
//! machine at all until something asks for it, and the thing that asks is the
//! deck, two bars before the drop.
//!
//! None of this shows up while a library is being built, because building one
//! reads every file once and slowly anyway. It shows up on the night. So the
//! check is here, it runs with the rest of the file checks, and the one problem
//! that can be fixed from inside Booth — the quarantine flag — is fixed rather
//! than described.

use std::path::{Component, Path, PathBuf};

/// The extended attribute macOS puts on anything that came off the network.
pub const QUARANTINE: &str = "com.apple.quarantine";

/// Folders whose contents belong to something other than the filesystem.
///
/// Matched on a whole path component rather than a substring, so a record
/// called "Dropbox EP" is not mistaken for a sync folder. iCloud Drive is the
/// odd one out: its on-disk name is not the one anybody sees.
const SYNCED: [(&str, &str); 7] = [
    ("Dropbox", "Dropbox"),
    ("com~apple~CloudDocs", "iCloud Drive"),
    ("Mobile Documents", "iCloud Drive"),
    ("Google Drive", "Google Drive"),
    ("GoogleDrive", "Google Drive"),
    ("OneDrive", "OneDrive"),
    ("pCloud Drive", "pCloud"),
];

/// Why reaching a file is going to be slower than reaching its neighbour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slow {
    /// macOS has flagged the file as downloaded, so every open is a
    /// Gatekeeper check. Booth can take this off.
    Quarantined,
    /// Somewhere under the Downloads folder, which is where quarantine keeps
    /// coming back from and not where a library belongs.
    Downloads,
    /// Inside a folder a sync client owns. The file may be a placeholder that
    /// has to be fetched before it can be played, and nothing Booth does to it
    /// will change that — only moving it out will.
    Synced(&'static str),
}

impl Slow {
    /// What to say about it, in the sheet, as one line.
    pub fn text(self) -> String {
        match self {
            Slow::Quarantined => {
                "quarantined by macOS, so every play is a Gatekeeper check".to_string()
            }
            Slow::Downloads => {
                "in the Downloads folder, where macOS keeps re-flagging it".to_string()
            }
            Slow::Synced(service) => {
                format!("inside {service}, so playing it may mean downloading it first")
            }
        }
    }

    /// Whether Booth can do anything about it, as opposed to telling you.
    pub fn fixable(self) -> bool {
        self == Slow::Quarantined
    }
}

/// Everything slow about reaching this file.
pub fn slow(path: &Path) -> Vec<Slow> {
    let mut found = Vec::new();
    if quarantined(path) {
        found.push(Slow::Quarantined);
    }
    if in_downloads(path) {
        found.push(Slow::Downloads);
    }
    if let Some(service) = synced_under(path) {
        found.push(Slow::Synced(service));
    }
    found
}

/// The sync service whose folder this path is inside, if any.
pub fn synced_under(path: &Path) -> Option<&'static str> {
    path.components().find_map(|component| match component {
        Component::Normal(name) => {
            let name = name.to_str()?;
            SYNCED.iter().find(|(folder, _)| *folder == name).map(|(_, service)| *service)
        }
        _ => None,
    })
}

/// Whether the path is under the current user's Downloads folder.
///
/// Compared against the home directory rather than against the name, because
/// a crate called "Downloads" inside a music folder is somebody's filing and
/// not a problem.
pub fn in_downloads(path: &Path) -> bool {
    home().map(|home| path.starts_with(home.join("Downloads"))).unwrap_or(false)
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from).filter(|home| !home.as_os_str().is_empty())
}

/// The folders worth clearing in one go, given a set of quarantined files.
///
/// Clearing a flag file by file is a syscall each and leaves the folder to
/// re-flag the next thing put in it; `xattr -r -d` on the folder is what a
/// person would run. This picks the folders that hold more than one of them,
/// which is the case that is worth offering.
pub fn roots(paths: &[PathBuf]) -> Vec<PathBuf> {
    let mut counted: Vec<(PathBuf, usize)> = Vec::new();
    for parent in paths.iter().filter_map(|path| path.parent()) {
        match counted.iter_mut().find(|(seen, _)| seen == parent) {
            Some((_, count)) => *count += 1,
            None => counted.push((parent.to_path_buf(), 1)),
        }
    }
    counted.retain(|(_, count)| *count > 1);
    counted.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    counted.into_iter().map(|(path, _)| path).collect()
}

/// The command that does what [`clear`] does, for saying so out loud.
///
/// Booth calls the syscall rather than the tool, but a person who wants to know
/// what was done to their files — or to do it to a folder Booth has no business
/// walking — should be told in the form they can paste.
pub fn command_for(path: &Path, recursive: bool) -> String {
    let flags = match recursive {
        true => "-r -d",
        false => "-d",
    };
    format!("xattr {flags} {QUARANTINE} {}", shell_quoted(path))
}

fn shell_quoted(path: &Path) -> String {
    let text = path.to_string_lossy();
    match text.chars().all(|c| c.is_alphanumeric() || "._-/".contains(c)) {
        true => text.into_owned(),
        false => format!("'{}'", text.replace('\'', r"'\''")),
    }
}

#[cfg(target_os = "macos")]
mod xattr {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;

    fn c_path(path: &Path) -> Option<CString> {
        CString::new(path.as_os_str().as_bytes()).ok()
    }

    pub fn present(path: &Path, name: &str) -> bool {
        let (Some(path), Ok(name)) = (c_path(path), CString::new(name)) else {
            return false;
        };
        // Asking for zero bytes asks whether the attribute is there at all,
        // which is the whole question and costs no allocation.
        let size =
            unsafe { libc::getxattr(path.as_ptr(), name.as_ptr(), std::ptr::null_mut(), 0, 0, 0) };
        size >= 0
    }

    pub fn remove(path: &Path, name: &str) -> std::io::Result<()> {
        let (Some(c), Ok(name)) = (c_path(path), CString::new(name)) else {
            return Err(std::io::Error::other(format!("{} is not a usable name", path.display())));
        };
        if unsafe { libc::removexattr(c.as_ptr(), name.as_ptr(), 0) } == 0 {
            return Ok(());
        }
        let failure = std::io::Error::last_os_error();
        // Already gone is the outcome that was wanted. Another pass over a
        // folder, or two tracks in it sharing a parent, must not read as a
        // failure.
        match failure.raw_os_error() {
            Some(libc::ENOATTR) => Ok(()),
            _ => Err(failure),
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod xattr {
    use std::path::Path;

    pub fn present(_path: &Path, _name: &str) -> bool {
        false
    }

    pub fn remove(_path: &Path, _name: &str) -> std::io::Result<()> {
        Ok(())
    }
}

/// Whether macOS has this file flagged as downloaded.
///
/// Always false anywhere else: the flag is a macOS idea, and a check that
/// invented one on Linux would put a fix button next to a problem that cannot
/// exist.
pub fn quarantined(path: &Path) -> bool {
    xattr::present(path, QUARANTINE)
}

/// Take the quarantine flag off one file. Not being flagged counts as done.
pub fn clear(path: &Path) -> std::io::Result<()> {
    xattr::remove(path, QUARANTINE)
}

/// Take it off a folder and everything under it, the way `xattr -r -d` does.
///
/// Returns how many entries were cleared and what refused. It keeps going past
/// a refusal: one file somebody else owns is not a reason to leave the other
/// four hundred flagged.
pub fn clear_tree(root: &Path) -> (usize, Vec<(PathBuf, std::io::Error)>) {
    let mut cleared = 0;
    let mut refused = Vec::new();
    let mut walking = vec![root.to_path_buf()];
    while let Some(at) = walking.pop() {
        match clear(&at) {
            Ok(()) => cleared += 1,
            Err(e) => refused.push((at.clone(), e)),
        }
        // Symlinks are not followed: a link into somebody else's folder is not
        // part of this tree, and a link that points at its own parent is a walk
        // that never ends.
        let Ok(entries) = std::fs::read_dir(&at) else { continue };
        for entry in entries.flatten() {
            if entry.file_type().is_ok_and(|kind| !kind.is_symlink()) {
                walking.push(entry.path());
            }
        }
    }
    (cleared, refused)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sync_folder_is_recognised_by_a_whole_name_and_not_a_substring() {
        assert_eq!(
            synced_under(Path::new("/Users/dj/Dropbox/Music/a.flac")),
            Some("Dropbox"),
            "a file inside Dropbox was not spotted"
        );
        assert_eq!(
            synced_under(Path::new(
                "/Users/dj/Library/Mobile Documents/com~apple~CloudDocs/a.flac"
            )),
            Some("iCloud Drive")
        );
        assert_eq!(
            synced_under(Path::new("/Users/dj/Music/Dropbox EP/a.flac")),
            None,
            "a record named after the service is not the service"
        );
    }

    #[test]
    fn the_downloads_folder_is_this_users_rather_than_any_folder_so_named() {
        let home = Path::new("/Users/dj");
        // Set for this test only; the check reads it rather than guessing.
        temp_env("HOME", "/Users/dj", || {
            assert!(in_downloads(&home.join("Downloads/mix.flac")));
            assert!(in_downloads(&home.join("Downloads/2024/mix.flac")));
            assert!(
                !in_downloads(Path::new("/Volumes/Music/Downloads/mix.flac")),
                "a crate somebody called Downloads is somebody's filing"
            );
        });
    }

    #[test]
    fn a_folder_holding_more_than_one_flagged_file_is_worth_clearing_whole() {
        let paths = [
            PathBuf::from("/m/incoming/a.flac"),
            PathBuf::from("/m/incoming/b.flac"),
            PathBuf::from("/m/one-off/c.flac"),
        ];
        assert_eq!(
            roots(&paths),
            vec![PathBuf::from("/m/incoming")],
            "a folder with a single file in it is a file, not a folder to sweep"
        );
    }

    #[test]
    fn the_command_shown_is_the_one_a_person_would_type() {
        assert_eq!(
            command_for(Path::new("/m/a.flac"), false),
            "xattr -d com.apple.quarantine /m/a.flac"
        );
        assert_eq!(
            command_for(Path::new("/m/Friday Night"), true),
            "xattr -r -d com.apple.quarantine '/m/Friday Night'"
        );
    }

    #[test]
    fn clearing_a_file_that_was_never_flagged_is_not_a_failure() {
        let dir = std::env::temp_dir().join(format!("booth-xattr-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("plain.flac");
        std::fs::write(&file, b"").unwrap();

        assert!(!quarantined(&file));
        clear(&file).expect("clearing a flag that is not there is the outcome that was wanted");

        let (cleared, refused) = clear_tree(&dir);
        assert_eq!(cleared, 2, "the folder and the file in it");
        assert!(refused.is_empty(), "{refused:?}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Run `body` with an environment variable set, and put it back after.
    ///
    /// Tests share a process, so this is not safe to run beside another test
    /// that reads the same variable — none does.
    fn temp_env(name: &str, value: &str, body: impl FnOnce()) {
        let before = std::env::var_os(name);
        unsafe { std::env::set_var(name, value) };
        body();
        match before {
            Some(was) => unsafe { std::env::set_var(name, was) },
            None => unsafe { std::env::remove_var(name) },
        }
    }
}
