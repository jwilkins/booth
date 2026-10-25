//! What a drive has room for, asked of the filesystem it is on.
//!
//! Shared by the window's preflight and the command line's export, so that the
//! two cannot come to disagree about whether a write fits.

use std::path::{Path, PathBuf};

/// Room for the analysis files written for each track, which are not part of
/// any audio file's size. Two megabytes is generous for anything under about
/// half an hour long, and it is what an image is sized with.
pub const ANALYSIS_ALLOWANCE: u64 = 2 * 1024 * 1024;

/// Free bytes on the filesystem holding `path`, if it can be found out.
///
/// There is no portable way to ask, so on the platforms that answer the answer
/// is used, and on the ones that do not the check says it does not know rather
/// than guessing. A preflight that invents a free-space number is worse than
/// one that admits it has none.
#[cfg(unix)]
pub fn free_space(path: &Path) -> Option<u64> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let existing = nearest_existing(path)?;
    let c_path = CString::new(existing.as_os_str().as_bytes()).ok()?;
    let mut stat = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: the path is a valid NUL-terminated string that outlives the call,
    // and `statvfs` either fills the struct and returns zero or returns non-zero
    // and leaves it alone — which is why it is only read on success.
    let filled = unsafe { libc::statvfs(c_path.as_ptr(), stat.as_mut_ptr()) };
    if filled != 0 {
        return None;
    }
    let stat = unsafe { stat.assume_init() };
    // The casts are redundant on 64-bit and not on 32-bit, where these fields
    // are `c_ulong`.
    #[allow(clippy::unnecessary_cast)]
    (stat.f_bavail as u64).checked_mul(stat.f_frsize as u64)
}

#[cfg(not(unix))]
pub fn free_space(_path: &Path) -> Option<u64> {
    None
}

/// The path itself if it exists, else the nearest parent that does — a drive
/// folder that has not been created yet still sits on a filesystem with a size.
pub fn nearest_existing(path: &Path) -> Option<PathBuf> {
    let mut at = path;
    loop {
        if at.exists() {
            return Some(at.to_path_buf());
        }
        at = at.parent()?;
    }
}

/// A size, in the units a person reads.
pub fn bytes(count: u64) -> String {
    const UNITS: [(&str, u64); 4] =
        [("TB", 1 << 40), ("GB", 1 << 30), ("MB", 1 << 20), ("KB", 1 << 10)];
    for (unit, size) in UNITS {
        if count >= size {
            return format!("{:.1} {unit}", count as f64 / size as f64);
        }
    }
    format!("{count} B")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn free_space_is_reported_when_the_platform_will_say() {
        // On the platforms that answer, a real directory has a real size; on
        // the ones that do not, the check says so instead of guessing.
        let answer = free_space(Path::new("."));
        if cfg!(unix) {
            assert!(answer.is_some_and(|free| free > 0), "{answer:?}");
        }
    }

    #[test]
    fn a_folder_not_made_yet_is_asked_about_through_the_one_it_will_be_made_in() {
        let here = std::env::current_dir().unwrap();
        let later = here.join("not-made-yet").join("PIONEER");
        assert_eq!(nearest_existing(&later), Some(here));
    }

    #[test]
    fn sizes_read_the_way_a_person_says_them() {
        assert_eq!(bytes(512), "512 B");
        assert_eq!(bytes(2 * 1024 * 1024), "2.0 MB");
        assert_eq!(bytes(3 * 1024 * 1024 * 1024 + 512 * 1024 * 1024), "3.5 GB");
    }
}
