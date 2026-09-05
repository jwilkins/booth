//! Reading just enough of an MP4 header to answer two questions.
//!
//! Is this actually an MP4, and is it a protected purchase? Both come out of
//! the first box in the file, which makes them cheap enough to ask about every
//! file walked to during an import.
//!
//! Nothing here decodes anything or gets past anything. A FairPlay file is
//! identified so that it can be reported as one — a track that cannot be
//! converted and has to be replaced is worth knowing about when it is added,
//! not on the night.

use std::fs::File;
use std::io::Read;
use std::path::Path;

/// What the first box of a file says it is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Container {
    /// An MP4 of some kind, with its major brand.
    Mp4 { brand: String, protected: bool },
    /// The file begins with something that is not an MP4 `ftyp` box.
    NotMp4,
    /// It could not be read, or is shorter than a header.
    Unreadable,
}

/// The brands Apple uses for FairPlay-protected audio and video.
///
/// A protected file is not a broken file: it plays perfectly in the program it
/// was bought in. It is simply not something a CDJ can open, and not something
/// any amount of re-encoding here will change.
const PROTECTED_BRANDS: [&str; 3] = ["M4P", "M4B", "mp42-drm"];

/// Read the `ftyp` box at the start of a file.
pub fn sniff(path: &Path) -> Container {
    let mut header = [0u8; 16];
    let Ok(mut file) = File::open(path) else { return Container::Unreadable };
    if file.read_exact(&mut header).is_err() {
        return Container::Unreadable;
    }
    if &header[4..8] != b"ftyp" {
        return Container::NotMp4;
    }
    // The major brand is the four bytes after the box type. It is padded with
    // spaces rather than nulls, so `M4A ` and `M4P ` both trim to three.
    let brand = String::from_utf8_lossy(&header[8..12]).trim().to_string();
    let protected = PROTECTED_BRANDS.iter().any(|p| p.eq_ignore_ascii_case(&brand));
    Container::Mp4 { brand, protected }
}

/// Whether a file is a FairPlay purchase, which nothing here can convert.
///
/// The `.m4p` extension is checked as well as the brand: a file renamed to
/// `.m4a` is still protected, and a file named `.m4p` is worth refusing to
/// spend a decode on either way.
pub fn is_protected(path: &Path) -> bool {
    let named_protected =
        path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("m4p"));
    named_protected || matches!(sniff(path), Container::Mp4 { protected: true, .. })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("musicai-mp4-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn with_brand(dir: &Path, name: &str, brand: &[u8; 4]) -> std::path::PathBuf {
        let mut payload = brand.to_vec();
        payload.extend_from_slice(&0u32.to_be_bytes());
        payload.extend_from_slice(b"isom");
        let mut file = ((8 + payload.len()) as u32).to_be_bytes().to_vec();
        file.extend_from_slice(b"ftyp");
        file.extend_from_slice(&payload);
        let path = dir.join(name);
        std::fs::write(&path, file).unwrap();
        path
    }

    #[test]
    fn an_ordinary_m4a_is_an_mp4_and_is_not_protected() {
        let dir = scratch("plain");
        let path = with_brand(&dir, "track.m4a", b"M4A ");
        assert_eq!(sniff(&path), Container::Mp4 { brand: "M4A".into(), protected: false });
        assert!(!is_protected(&path));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_purchase_is_recognised_even_when_it_has_been_renamed() {
        let dir = scratch("drm");
        // The brand says what it is whatever the file is called.
        let renamed = with_brand(&dir, "track.m4a", b"M4P ");
        assert!(is_protected(&renamed), "a renamed purchase is still a purchase");

        // And the name is taken at its word even before the file is opened.
        let named = dir.join("bought.m4p");
        std::fs::write(&named, b"").unwrap();
        assert!(is_protected(&named));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn something_that_is_not_an_mp4_says_so_rather_than_guessing() {
        let dir = scratch("bogus");
        let path = dir.join("track.m4a");
        std::fs::write(&path, b"ID3\x04this is an mp3 that was renamed").unwrap();
        assert_eq!(sniff(&path), Container::NotMp4);
        assert!(!is_protected(&path), "unreadable is not the same as protected");

        // Too short to hold a header at all.
        let stub = dir.join("stub.m4a");
        std::fs::write(&stub, b"abc").unwrap();
        assert_eq!(sniff(&stub), Container::Unreadable);

        assert_eq!(sniff(&dir.join("nothing-here.m4a")), Container::Unreadable);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
