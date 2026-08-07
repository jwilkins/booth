//! Carrying a track's tags over to the stems separated out of it.
//!
//! A stem with no tags is an orphan: it lands in a library as an untitled file
//! by an unknown artist. Copying the parent's metadata keeps it identifiable.
//!
//! Two things are deliberately not a straight copy:
//!
//! - **ReplayGain tags are dropped.** They describe the loudness of the mix,
//!   and a stem is quieter than the mix it came from. Copying them would leave
//!   a player applying a gain figure that was measured on different audio.
//! - **The stem's name is appended to the title**, because three files all
//!   called the same thing is worse than useless in a library.

use std::path::Path;

use anyhow::{bail, Context, Result};

use crate::audio::encode::Codec;
use crate::stems::Stem;

use super::{read_metadata, write_tags, OnExisting};

/// Tag naming which stem a file is, in both tag dialects.
pub const STEM_KEY: &str = "STEM";

/// Copy the tags of `source` onto `dest`, adjusted for a stem.
pub fn copy_for_stem(source: &Path, dest: &Path, stem: Stem) -> Result<()> {
    let from = Codec::from_path(source);
    let to = Codec::from_path(dest)
        .ok_or_else(|| anyhow::anyhow!("cannot tell what kind of file {} is", dest.display()))?;

    // Wav has nowhere to put any of this, which is not an error — the stem is
    // written, it just cannot carry metadata.
    if to == Codec::Wav {
        return Ok(());
    }

    match (from, to) {
        // Same format: copy the whole tag, so genre, composer, comments and
        // anything else the user curated comes along too.
        (Some(Codec::Flac), Codec::Flac) => copy_vorbis(source, dest, stem),
        (Some(Codec::Mp3), Codec::Mp3) => copy_id3(source, dest, stem),
        // Crossing formats, or coming from a wav with no tags at all. Only the
        // fields with an agreed meaning in both dialects can survive.
        _ => copy_known_fields(source, dest, stem),
    }
    .with_context(|| format!("copying tags from {} to {}", source.display(), dest.display()))
}

/// True for the tag names ReplayGain uses, in either dialect.
fn is_replaygain(key: &str) -> bool {
    key.to_ascii_uppercase().starts_with("REPLAYGAIN_")
}

/// The title a stem should carry.
fn stem_title(original: Option<&str>, stem: Stem) -> Option<String> {
    original.map(|title| format!("{title} ({stem})"))
}

// -- same-format copies ----------------------------------------------------

fn copy_vorbis(source: &Path, dest: &Path, stem: Stem) -> Result<()> {
    let src = metaflac::Tag::read_from_path(source)?;
    let mut dst = metaflac::Tag::read_from_path(dest)?;

    let source_comments = src.vorbis_comments().cloned();
    let original_title = source_comments
        .as_ref()
        .and_then(|c| c.comments.get("TITLE"))
        .and_then(|values| values.first())
        .cloned();

    {
        let out = dst.vorbis_comments_mut();
        // The contract is that the stem carries no ReplayGain, so drop what the
        // destination already had as well as declining to copy the source's.
        out.comments.retain(|key, _| !is_replaygain(key));
        if let Some(comments) = &source_comments {
            for (key, values) in &comments.comments {
                if is_replaygain(key) {
                    continue;
                }
                out.comments.insert(key.clone(), values.clone());
            }
        }
        if let Some(title) = stem_title(original_title.as_deref(), stem) {
            out.set("TITLE", vec![title]);
        }
        out.set(STEM_KEY, vec![stem.name().to_string()]);
    }

    // Cover art belongs to the release, so it applies to the stems too. Clear
    // first, or a destination that already had art ends up with it twice.
    dst.remove_blocks(metaflac::BlockType::Picture);
    for picture in src.pictures() {
        dst.push_block(metaflac::Block::Picture(picture.clone()));
    }

    dst.save()?;
    Ok(())
}

fn copy_id3(source: &Path, dest: &Path, stem: Stem) -> Result<()> {
    use id3::frame::ExtendedText;
    use id3::{Tag, TagLike, Version};

    // A source with no tag at all is normal, not a failure.
    let src = Tag::read_from_path(source).unwrap_or_default();
    let mut dst = Tag::new();

    for frame in src.frames() {
        if frame_is_replaygain(frame) {
            continue;
        }
        dst.add_frame(frame.clone());
    }

    if let Some(title) = stem_title(src.title(), stem) {
        dst.set_title(title);
    }
    dst.remove_extended_text(Some(STEM_KEY), None);
    dst.add_frame(ExtendedText {
        description: STEM_KEY.to_string(),
        value: stem.name().to_string(),
    });

    dst.write_to_path(dest, Version::Id3v24)?;
    Ok(())
}

/// ID3 carries ReplayGain either as `TXXX` frames or as the dedicated `RVA2`
/// relative-volume frame. Both are about the mix, not the stem.
fn frame_is_replaygain(frame: &id3::Frame) -> bool {
    if frame.id() == "RVA2" {
        return true;
    }
    frame.content().extended_text().is_some_and(|t| is_replaygain(&t.description))
}

// -- crossing formats ------------------------------------------------------

fn copy_known_fields(source: &Path, dest: &Path, stem: Stem) -> Result<()> {
    // Nothing to copy from a source with no tag dialect of its own.
    if !matches!(Codec::from_path(source), Some(Codec::Flac) | Some(Codec::Mp3)) {
        return Ok(());
    }

    let mut metadata = read_metadata(source)?;
    metadata.title = stem_title(metadata.title.as_deref(), stem);

    // Overwrite: the stem was just written and holds nothing worth keeping.
    write_tags(dest, &metadata, OnExisting::Overwrite, None)?;
    finish_stem(dest, stem)
}

/// Record which stem a file is and make sure no ReplayGain survives on it,
/// whatever the file's format.
fn finish_stem(path: &Path, stem: Stem) -> Result<()> {
    match Codec::from_path(path) {
        Some(Codec::Flac) => {
            let mut tag = metaflac::Tag::read_from_path(path)?;
            let comments = tag.vorbis_comments_mut();
            comments.comments.retain(|key, _| !is_replaygain(key));
            comments.set(STEM_KEY, vec![stem.name().to_string()]);
            tag.save()?;
            Ok(())
        }
        Some(Codec::Mp3) => {
            use id3::frame::ExtendedText;
            use id3::{Tag, TagLike, Version};

            let existing = Tag::read_from_path(path).unwrap_or_default();
            let mut tag = Tag::new();
            for frame in existing.frames() {
                if !frame_is_replaygain(frame) {
                    tag.add_frame(frame.clone());
                }
            }
            tag.remove_extended_text(Some(STEM_KEY), None);
            tag.add_frame(ExtendedText {
                description: STEM_KEY.to_string(),
                value: stem.name().to_string(),
            });
            tag.write_to_path(path, Version::Id3v24)?;
            Ok(())
        }
        Some(Codec::Wav) => Ok(()),
        None => bail!("cannot tell what kind of file {} is", path.display()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_replaygain_tags_in_either_case() {
        assert!(is_replaygain("REPLAYGAIN_TRACK_GAIN"));
        assert!(is_replaygain("replaygain_album_peak"));
        assert!(is_replaygain("REPLAYGAIN_REFERENCE_LOUDNESS"));

        // Things that merely mention gain are not ReplayGain tags.
        assert!(!is_replaygain("TITLE"));
        assert!(!is_replaygain("GAIN"));
        assert!(!is_replaygain("MY_REPLAYGAIN_NOTES"));
    }

    #[test]
    fn appends_the_stem_to_the_title() {
        assert_eq!(
            stem_title(Some("Some Song"), Stem::Vocals).as_deref(),
            Some("Some Song (vocals)")
        );
        assert_eq!(
            stem_title(Some("Some Song"), Stem::Drums).as_deref(),
            Some("Some Song (drums)")
        );
        // Nothing to build on means nothing is invented.
        assert_eq!(stem_title(None, Stem::Melody), None);
    }

    #[test]
    fn wav_stems_are_skipped_rather_than_failing() {
        // Wav has no standard tag, but that must not fail the separation.
        let result = copy_for_stem(Path::new("a.flac"), Path::new("b.wav"), Stem::Vocals);
        assert!(result.is_ok(), "{result:?}");
    }

    #[test]
    fn an_unknown_destination_is_an_error() {
        let err = copy_for_stem(Path::new("a.flac"), Path::new("b.ogg"), Stem::Vocals).unwrap_err();
        assert!(err.to_string().contains("cannot tell what kind of file"), "{err}");
    }
}
