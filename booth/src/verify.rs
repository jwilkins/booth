//! Checking the collection against the files it describes.
//!
//! A collection is a set of claims about files somebody else's programs can
//! also move, retag and delete. Every claim in it was true when it was written
//! and none of them is guaranteed still to be, so this reads the files back and
//! says where the two have come apart.
//!
//! Only what a container answers without being decoded: whether the file is
//! there, how big it is, and what its tags say. Length, sample rate and channel
//! count are not here, even though the collection holds them, because getting
//! them means decoding — the difference between a sweep of a whole library over
//! lunch and one over a weekend. They are checked by analysing, which is what
//! measured them in the first place.

use std::path::{Path, PathBuf};

use crate::library::{Field, Track};

/// One way a track and its file disagree.
#[derive(Clone, Debug, PartialEq)]
pub enum Trouble {
    /// The file is not where the collection says it is.
    ///
    /// Which is not the same as gone: an unplugged drive and a deleted file
    /// look identical from here, and telling somebody their music is deleted
    /// when the drive is merely asleep would be worse than saying neither.
    Missing,
    /// The file is there, and is not the file that was read: it is a different
    /// size now.
    Resized { was: u64, now: u64 },
    /// The same size, and different bytes. Only ever found by reading them, so
    /// only ever reported by a thorough check.
    Rewritten,
    /// The file's tags answer something the collection answers differently.
    Field { field: Field, stored: String, file: String },
    /// A stem kit lists a part that is not on disk.
    StemGone { part: &'static str },
}

impl Trouble {
    /// How bad it is, worst first, for sorting and for colour.
    pub fn rank(&self) -> u8 {
        match self {
            Trouble::Missing => 0,
            Trouble::Rewritten => 1,
            Trouble::Resized { .. } => 2,
            Trouble::StemGone { .. } => 3,
            Trouble::Field { .. } => 4,
        }
    }

    /// Whether this is something the collection can put right on its own.
    ///
    /// A file's own size and tags are facts about the file, so where they
    /// differ the collection is simply out of date and can be brought up to
    /// date. A missing file is not a disagreement to settle — there is nothing
    /// to read — and what to do about it is the user's call.
    pub fn is_fixable(&self) -> bool {
        !matches!(self, Trouble::Missing)
    }

    pub fn what(&self) -> String {
        match self {
            Trouble::Missing => "the file is not there".to_string(),
            Trouble::Resized { was, now } => format!(
                "{} on disk, {} in the collection",
                crate::sync::bytes(*now),
                crate::sync::bytes(*was)
            ),
            Trouble::Rewritten => "same size, different bytes".to_string(),
            Trouble::Field { field, .. } => format!("{} disagrees with the file", field.name()),
            Trouble::StemGone { part } => format!("the {part} stem is gone"),
        }
    }
}

/// What one track's check found.
#[derive(Clone, Debug, PartialEq)]
pub struct Report {
    pub id: u32,
    pub path: PathBuf,
    pub troubles: Vec<Trouble>,
    /// The file as it reads now, for the fixes to be applied from. Absent when
    /// there was nothing to read.
    pub fresh: Option<Box<Track>>,
}

/// Read a track's file back and say where it and the collection disagree.
///
/// `deep` reads every byte to compare the hashes, which catches a file edited
/// in place without changing its length — a retag by another program, most
/// often. Without it the check is a stat and a tag read per file.
pub fn check(track: &Track, deep: bool) -> Report {
    let mut troubles = Vec::new();

    if !track.path.exists() {
        return Report {
            id: track.id,
            path: track.path.clone(),
            troubles: vec![Trouble::Missing],
            fresh: None,
        };
    }

    let fresh = crate::job::read_record(track.id, &track.path, deep);

    if fresh.bytes != track.bytes {
        troubles.push(Trouble::Resized { was: track.bytes, now: fresh.bytes });
    } else if deep
        && !track.file_hash.is_empty()
        && !fresh.file_hash.is_empty()
        && fresh.file_hash != track.file_hash
    {
        troubles.push(Trouble::Rewritten);
    }

    // Only where the file itself says something. A blank tag is not the file
    // disagreeing: it is the file having no opinion, and a name that came from
    // a fingerprint rather than from the tags is the collection knowing more
    // than the file does, which is the arrangement working rather than failing.
    let mut said = |field: Field, stored: &str, from_file: &str| {
        let (stored, from_file) = (stored.trim(), from_file.trim());
        if !from_file.is_empty() && stored != from_file {
            troubles.push(Trouble::Field {
                field,
                stored: stored.to_string(),
                file: from_file.to_string(),
            });
        }
    };
    said(Field::Artist, &track.artist, &fresh.artist);
    said(Field::Album, &track.album, &fresh.album);
    // The title only when the file was tagged with one. `read_record` falls
    // back to the file name for a track with no title tag, and reporting that
    // as the file's answer would turn every renamed file into a disagreement.
    if fresh.from_tags {
        said(Field::Title, &track.title, &fresh.title);
    }
    said(
        Field::Year,
        &track.year.map(|y| y.to_string()).unwrap_or_default(),
        &fresh.year.map(|y| y.to_string()).unwrap_or_default(),
    );

    for (part, at) in track.stems.each() {
        if at.is_some_and(|path| !path.exists()) {
            troubles.push(Trouble::StemGone { part });
        }
    }

    troubles.sort_by_key(|trouble| trouble.rank());
    match troubles.is_empty() {
        true => crate::debug!("#{} is as the collection describes it", track.id),
        false => crate::debug!(
            "#{} {}: {}",
            track.id,
            track.path.display(),
            troubles.iter().map(Trouble::what).collect::<Vec<_>>().join(", ")
        ),
    }
    Report { id: track.id, path: track.path.clone(), troubles, fresh: Some(Box::new(fresh)) }
}

/// Playable files in the library folder that no track points at.
///
/// The other half of the question: the checks above ask whether every track
/// still has its file, and this asks whether every file is still somebody's
/// track. A rip that failed halfway through the import, or an album copied in
/// by hand, is in the folder and in no playlist and would never be played.
pub fn orphans(library: &Path, known: &[PathBuf]) -> Vec<PathBuf> {
    if library.as_os_str().is_empty() || !library.is_dir() {
        return Vec::new();
    }
    let Ok(found) = booth_core::discover::collect(&[library.to_path_buf()], true) else {
        return Vec::new();
    };
    // Compared as the collection stores them, which is how they were written
    // when the file was imported from this same folder.
    let known: std::collections::HashSet<&Path> = known.iter().map(|p| p.as_path()).collect();
    found.into_iter().filter(|path| !known.contains(path.as_path())).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::Library;

    /// A directory with one real file in it, and the track that describes it.
    fn a_file(name: &str, bytes: &[u8]) -> (PathBuf, Library, u32) {
        let dir = std::env::temp_dir().join(format!("booth-verify-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("track.flac");
        std::fs::write(&path, bytes).unwrap();

        let mut library = Library::new();
        let id = library.add(&path);
        let track = library.get_mut(id).unwrap();
        track.bytes = bytes.len() as u64;
        (dir, library, id)
    }

    #[test]
    fn a_collection_that_matches_its_files_has_nothing_to_report() {
        let (dir, library, id) = a_file("clean", b"some audio");
        let report = check(library.get(id).unwrap(), false);
        assert!(report.troubles.is_empty(), "{:?}", report.troubles);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_that_is_not_there_is_the_only_thing_reported_about_it() {
        // Nothing else can be true of a file that cannot be read, and a list
        // saying the tags disagree with a file that is not there would be
        // noise on top of the one thing worth knowing.
        let mut library = Library::new();
        let id = library.add(Path::new("/nowhere/at/all/missing.flac"));
        let report = check(library.get(id).unwrap(), false);
        assert_eq!(report.troubles, vec![Trouble::Missing]);
        assert!(report.fresh.is_none(), "there was nothing to read");
        assert!(!report.troubles[0].is_fixable(), "nothing here can be put right by reading");
    }

    #[test]
    fn a_file_that_changed_size_is_not_the_file_that_was_read() {
        let (dir, mut library, id) = a_file("resized", b"some audio");
        library.get_mut(id).unwrap().bytes = 999_999;

        let report = check(library.get(id).unwrap(), false);
        assert_eq!(report.troubles, vec![Trouble::Resized { was: 999_999, now: 10 }]);
        assert!(report.troubles[0].is_fixable());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_rewritten_at_the_same_length_is_only_found_by_reading_it() {
        let (dir, mut library, id) = a_file("rewritten", b"some audio");
        {
            let track = library.get_mut(id).unwrap();
            track.file_hash = "NOT-THE-HASH-OF-THAT".into();
        }

        let shallow = check(library.get(id).unwrap(), false);
        assert!(shallow.troubles.is_empty(), "a stat cannot see this: {:?}", shallow.troubles);

        let deep = check(library.get(id).unwrap(), true);
        assert_eq!(deep.troubles, vec![Trouble::Rewritten]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_blank_tag_is_the_file_having_no_opinion_rather_than_a_disagreement() {
        // The file has no tags at all. The collection knowing the artist — from
        // a fingerprint, or because somebody typed it — is the arrangement
        // working, not a fault to report.
        let (dir, mut library, id) = a_file("quiet", b"not really a flac");
        {
            let track = library.get_mut(id).unwrap();
            track.artist = "Peverelist".into();
            track.album = "Livity Sound".into();
            track.year = Some(2019);
        }

        let report = check(library.get(id).unwrap(), false);
        assert!(report.troubles.is_empty(), "{:?}", report.troubles);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_stem_the_kit_lists_and_the_disk_does_not_have_is_reported() {
        let (dir, mut library, id) = a_file("stems", b"some audio");
        {
            let track = library.get_mut(id).unwrap();
            track.stems.vocals = Some(dir.join("gone-vocals.flac"));
            track.stems.drums = Some(dir.join("track.flac"));
        }

        let report = check(library.get(id).unwrap(), false);
        assert_eq!(report.troubles, vec![Trouble::StemGone { part: "vocals" }]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_in_the_library_folder_that_nobody_knows_about_is_found() {
        let dir = std::env::temp_dir().join(format!("booth-verify-orphans-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Peverelist")).unwrap();
        let known = dir.join("Peverelist").join("known.flac");
        let stray = dir.join("Peverelist").join("stray.flac");
        std::fs::write(&known, b"a").unwrap();
        std::fs::write(&stray, b"b").unwrap();

        let found = orphans(&dir, std::slice::from_ref(&known));
        assert_eq!(found, vec![stray.clone()], "the file nobody points at was not found");

        assert!(orphans(&dir, &[known, stray]).is_empty(), "everything is known");
        assert!(orphans(Path::new(""), &[]).is_empty(), "no library folder, nothing to walk");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
