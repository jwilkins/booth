//! Tag writing against real flac and mp3 files.
//!
//! These are offline: they exercise everything from `Metadata` onwards, which
//! is the part that touches the user's files. Identification (AcoustID) and
//! metadata retrieval (MusicBrainz) are covered by `live_services.rs`.

use std::path::PathBuf;

use musicai::audio::decode::decode_file;
use musicai::audio::encode::{write_file, Codec, EncodeOptions};
use musicai::audio::Audio;
use musicai::tag::{coverart::CoverArt, write_tags, Field, Metadata, OnExisting};

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("musicai-tagging-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    /// A short real file of the given codec, with no tags on it.
    fn track(&self, codec: Codec) -> PathBuf {
        let sample_rate = 44_100;
        let frames = sample_rate * 2;
        let plane: Vec<f32> = (0..frames)
            .map(|i| {
                let t = i as f32 / sample_rate as f32;
                0.3 * (2.0 * std::f32::consts::PI * 440.0 * t).sin()
            })
            .collect();
        let audio = Audio::new(sample_rate as u32, vec![plane.clone(), plane]).unwrap();

        let path = self.0.join(format!("track.{}", codec.extension()));
        write_file(&path, &audio, codec, &EncodeOptions::default()).unwrap();
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn metadata() -> Metadata {
    Metadata {
        title: Some("Creep".into()),
        artist: Some("Radiohead".into()),
        album: Some("Pablo Honey".into()),
        album_artist: Some("Radiohead".into()),
        date: Some("1993-02-22".into()),
        track_number: Some(2),
        total_tracks: Some(12),
        disc_number: Some(1),
        total_discs: Some(1),
        recording_mbid: Some("rec-mbid".into()),
        release_mbid: Some("rel-mbid".into()),
        release_group_mbid: Some("rg-mbid".into()),
        artist_mbid: Some("art-mbid".into()),
        acoustid: Some("acoust-id".into()),
    }
}

/// A small but structurally valid PNG, built through `identify` so the
/// dimensions are filled in the same way a fetched cover's would be.
fn cover() -> CoverArt {
    let mut data = b"\x89PNG\r\n\x1a\n".to_vec();
    data.extend_from_slice(&[0, 0, 0, 13]); // IHDR length
    data.extend_from_slice(b"IHDR");
    data.extend_from_slice(&600u32.to_be_bytes());
    data.extend_from_slice(&600u32.to_be_bytes());
    data.push(8); // bit depth
    data.push(2); // colour type: truecolour
    data.extend_from_slice(&[0; 512]);
    musicai::tag::coverart::identify(data).expect("not recognised as an image")
}

fn vorbis(path: &PathBuf, key: &str) -> Option<String> {
    metaflac::Tag::read_from_path(path)
        .unwrap()
        .get_vorbis(key)
        .and_then(|mut v| v.next().map(str::to_string))
}

#[test]
fn writes_every_field_to_a_flac() {
    let dir = Scratch::new("flac-write");
    let path = dir.track(Codec::Flac);

    let outcome = write_tags(&path, &metadata(), OnExisting::Keep, None).unwrap();
    assert_eq!(outcome.written.len(), Field::ALL.len());

    assert_eq!(vorbis(&path, "TITLE").as_deref(), Some("Creep"));
    assert_eq!(vorbis(&path, "ARTIST").as_deref(), Some("Radiohead"));
    assert_eq!(vorbis(&path, "ALBUM").as_deref(), Some("Pablo Honey"));
    assert_eq!(vorbis(&path, "ALBUMARTIST").as_deref(), Some("Radiohead"));
    assert_eq!(vorbis(&path, "DATE").as_deref(), Some("1993-02-22"));
    assert_eq!(vorbis(&path, "TRACKNUMBER").as_deref(), Some("2"));
    assert_eq!(vorbis(&path, "TOTALTRACKS").as_deref(), Some("12"));
    // Picard-compatible names, so other software recognises these.
    assert_eq!(vorbis(&path, "MUSICBRAINZ_TRACKID").as_deref(), Some("rec-mbid"));
    assert_eq!(vorbis(&path, "MUSICBRAINZ_ALBUMID").as_deref(), Some("rel-mbid"));
    assert_eq!(vorbis(&path, "ACOUSTID_ID").as_deref(), Some("acoust-id"));
}

#[test]
fn writes_every_field_to_an_mp3() {
    use id3::TagLike;

    let dir = Scratch::new("mp3-write");
    let path = dir.track(Codec::Mp3);

    write_tags(&path, &metadata(), OnExisting::Keep, None).unwrap();

    let tag = id3::Tag::read_from_path(&path).unwrap();
    assert_eq!(tag.title(), Some("Creep"));
    assert_eq!(tag.artist(), Some("Radiohead"));
    assert_eq!(tag.album(), Some("Pablo Honey"));
    assert_eq!(tag.album_artist(), Some("Radiohead"));
    assert_eq!(tag.track(), Some(2));
    assert_eq!(tag.total_tracks(), Some(12));
    assert_eq!(tag.disc(), Some(1));

    let recorded = tag.date_recorded().expect("no recording date");
    assert_eq!((recorded.year, recorded.month, recorded.day), (1993, Some(2), Some(22)));

    let txxx: Vec<_> = tag.extended_texts().collect();
    assert!(
        txxx.iter().any(|t| t.description == "MusicBrainz Album Id" && t.value == "rel-mbid"),
        "no MusicBrainz Album Id frame"
    );
    assert!(
        txxx.iter().any(|t| t.description == "Acoustid Id" && t.value == "acoust-id"),
        "no Acoustid Id frame"
    );
}

#[test]
fn tagging_leaves_the_audio_untouched() {
    let dir = Scratch::new("audio-intact");
    let path = dir.track(Codec::Flac);
    let before = decode_file(&path).unwrap();

    write_tags(&path, &metadata(), OnExisting::Keep, Some(&cover())).unwrap();

    let after = decode_file(&path).unwrap();
    assert_eq!(before.planes, after.planes, "tagging altered the audio");
    assert_eq!(before.sample_rate, after.sample_rate);
}

#[test]
fn keep_policy_does_not_clobber_existing_tags() {
    let dir = Scratch::new("keep");
    let path = dir.track(Codec::Flac);

    // Pretend the user already curated the title.
    let mut tag = metaflac::Tag::read_from_path(&path).unwrap();
    tag.set_vorbis("TITLE", vec!["My Careful Title"]);
    tag.save().unwrap();

    let outcome = write_tags(&path, &metadata(), OnExisting::Keep, None).unwrap();

    assert_eq!(vorbis(&path, "TITLE").as_deref(), Some("My Careful Title"));
    // Everything else still got filled in.
    assert_eq!(vorbis(&path, "ALBUM").as_deref(), Some("Pablo Honey"));
    assert!(outcome.unchanged.contains(&Field::Title));
    assert!(!outcome.written.contains(&Field::Title));
}

#[test]
fn overwrite_policy_replaces_existing_tags() {
    let dir = Scratch::new("overwrite");
    let path = dir.track(Codec::Flac);

    let mut tag = metaflac::Tag::read_from_path(&path).unwrap();
    tag.set_vorbis("TITLE", vec!["My Careful Title"]);
    tag.save().unwrap();

    write_tags(&path, &metadata(), OnExisting::Overwrite, None).unwrap();
    assert_eq!(vorbis(&path, "TITLE").as_deref(), Some("Creep"));
}

#[test]
fn report_policy_reports_conflicts_without_writing_them() {
    let dir = Scratch::new("report");
    let path = dir.track(Codec::Flac);

    let mut tag = metaflac::Tag::read_from_path(&path).unwrap();
    tag.set_vorbis("TITLE", vec!["My Careful Title"]);
    tag.set_vorbis("ALBUM", vec!["Pablo Honey"]); // already agrees
    tag.save().unwrap();

    let outcome = write_tags(&path, &metadata(), OnExisting::Report, None).unwrap();

    assert_eq!(vorbis(&path, "TITLE").as_deref(), Some("My Careful Title"));
    assert_eq!(outcome.conflicts.len(), 1);
    assert_eq!(outcome.conflicts[0].field, Field::Title);
    assert_eq!(outcome.conflicts[0].proposed, "Creep");
    // A field that already matches is not reported as a conflict.
    assert!(outcome.unchanged.contains(&Field::Album));
}

#[test]
fn embeds_cover_art_in_flac_without_duplicating_it() {
    let dir = Scratch::new("flac-art");
    let path = dir.track(Codec::Flac);
    let art = cover();

    let outcome = write_tags(&path, &metadata(), OnExisting::Keep, Some(&art)).unwrap();
    assert!(outcome.cover_art);

    let count = |p: &PathBuf| metaflac::Tag::read_from_path(p).unwrap().pictures().count();
    assert_eq!(count(&path), 1);

    // Re-tagging must replace the picture, not add a second one.
    write_tags(&path, &metadata(), OnExisting::Overwrite, Some(&art)).unwrap();
    assert_eq!(count(&path), 1);

    let tag = metaflac::Tag::read_from_path(&path).unwrap();
    let picture = tag.pictures().next().unwrap();
    assert_eq!(picture.mime_type, "image/png");
    assert_eq!(picture.data.len(), art.data.len());
    // A FLAC PICTURE block carries its own dimensions; leaving them zero
    // writes a block that is valid but incomplete.
    assert_eq!((picture.width, picture.height), (600, 600));
    assert_eq!(picture.depth, 24);
}

#[test]
fn embeds_cover_art_in_mp3_without_duplicating_it() {
    let dir = Scratch::new("mp3-art");
    let path = dir.track(Codec::Mp3);
    let art = cover();

    write_tags(&path, &metadata(), OnExisting::Keep, Some(&art)).unwrap();
    write_tags(&path, &metadata(), OnExisting::Overwrite, Some(&art)).unwrap();

    let tag = id3::Tag::read_from_path(&path).unwrap();
    let pictures: Vec<_> = tag.pictures().collect();
    assert_eq!(pictures.len(), 1, "cover art was duplicated");
    assert_eq!(pictures[0].mime_type, "image/png");
}

#[test]
fn re_tagging_an_already_tagged_file_changes_nothing() {
    let dir = Scratch::new("idempotent");
    let path = dir.track(Codec::Flac);

    write_tags(&path, &metadata(), OnExisting::Keep, None).unwrap();
    let second = write_tags(&path, &metadata(), OnExisting::Keep, None).unwrap();

    assert!(second.written.is_empty(), "rewrote {:?} on a second pass", second.written);
    assert!(second.conflicts.is_empty());
    assert_eq!(second.unchanged.len(), Field::ALL.len());
}

#[test]
fn partial_metadata_writes_only_what_it_has() {
    let dir = Scratch::new("partial");
    let path = dir.track(Codec::Flac);

    let sparse = Metadata {
        title: Some("Untitled".into()),
        artist: Some("Someone".into()),
        ..Default::default()
    };
    let outcome = write_tags(&path, &sparse, OnExisting::Keep, None).unwrap();

    assert_eq!(outcome.written.len(), 2);
    assert_eq!(vorbis(&path, "TITLE").as_deref(), Some("Untitled"));
    assert_eq!(vorbis(&path, "ALBUM"), None);
}

#[test]
fn refuses_to_tag_a_wav() {
    let dir = Scratch::new("wav");
    let path = dir.track(Codec::Wav);

    let err = write_tags(&path, &metadata(), OnExisting::Keep, None).unwrap_err();
    assert!(err.to_string().contains("no standard metadata tag"), "unhelpful error: {err}");
}

// -- copying a parent's tags onto its stems --------------------------------

/// Give a file the kind of tags a real library file carries, including some
/// this tool knows nothing about and some that must not be copied.
fn tag_like_a_library_file(path: &PathBuf) {
    match Codec::from_path(path).unwrap() {
        Codec::Flac => {
            let mut tag = metaflac::Tag::read_from_path(path).unwrap();
            {
                let c = tag.vorbis_comments_mut();
                c.set("TITLE", vec!["Some Song"]);
                c.set("ARTIST", vec!["Some Artist"]);
                c.set("ALBUM", vec!["Some Album"]);
                // Fields this tool has no model for, but a user curated.
                c.set("GENRE", vec!["Shoegaze"]);
                c.set("COMPOSER", vec!["Someone Else"]);
                // Measured on the mix, so meaningless for a stem.
                c.set("REPLAYGAIN_TRACK_GAIN", vec!["-7.32 dB"]);
                c.set("REPLAYGAIN_TRACK_PEAK", vec!["0.988"]);
            }
            tag.save().unwrap();
        }
        Codec::Mp3 => {
            use id3::frame::ExtendedText;
            use id3::TagLike;
            let mut tag = id3::Tag::new();
            tag.set_title("Some Song");
            tag.set_artist("Some Artist");
            tag.set_album("Some Album");
            tag.set_genre("Shoegaze");
            tag.add_frame(ExtendedText {
                description: "REPLAYGAIN_TRACK_GAIN".into(),
                value: "-7.32 dB".into(),
            });
            tag.write_to_path(path, id3::Version::Id3v24).unwrap();
        }
        Codec::Wav => {}
    }
}

#[test]
fn stems_inherit_the_parents_tags_in_flac() {
    use musicai::stems::Stem;
    use musicai::tag::copy::copy_for_stem;

    let dir = Scratch::new("copy-flac");
    let source = dir.track(Codec::Flac);
    tag_like_a_library_file(&source);

    let stem_path = dir.0.join("vocals.flac");
    std::fs::copy(&source, &stem_path).unwrap();
    // Start from a stem carrying nothing, as a freshly written one would.
    let mut bare = metaflac::Tag::read_from_path(&stem_path).unwrap();
    bare.remove_blocks(metaflac::BlockType::VorbisComment);
    bare.save().unwrap();

    copy_for_stem(&source, &stem_path, Stem::Vocals).unwrap();

    assert_eq!(vorbis(&stem_path, "ARTIST").as_deref(), Some("Some Artist"));
    assert_eq!(vorbis(&stem_path, "ALBUM").as_deref(), Some("Some Album"));
    // Same-format copies bring across fields this tool has no model for.
    assert_eq!(vorbis(&stem_path, "GENRE").as_deref(), Some("Shoegaze"));
    assert_eq!(vorbis(&stem_path, "COMPOSER").as_deref(), Some("Someone Else"));
    // The stem is identifiable rather than a duplicate of its siblings.
    assert_eq!(vorbis(&stem_path, "TITLE").as_deref(), Some("Some Song (vocals)"));
    assert_eq!(vorbis(&stem_path, "STEM").as_deref(), Some("vocals"));
}

#[test]
fn replaygain_is_not_carried_over_to_a_stem() {
    use musicai::stems::Stem;
    use musicai::tag::copy::copy_for_stem;

    let dir = Scratch::new("copy-no-rg");
    let source = dir.track(Codec::Flac);
    tag_like_a_library_file(&source);

    let stem_path = dir.0.join("drums.flac");
    std::fs::copy(&source, &stem_path).unwrap();
    copy_for_stem(&source, &stem_path, Stem::Drums).unwrap();

    // The parent's loudness was measured on the mix. A stem is quieter, so
    // copying these would have a player apply a figure from other audio.
    assert_eq!(vorbis(&stem_path, "REPLAYGAIN_TRACK_GAIN"), None);
    assert_eq!(vorbis(&stem_path, "REPLAYGAIN_TRACK_PEAK"), None);
    // Everything else still made it.
    assert_eq!(vorbis(&stem_path, "ARTIST").as_deref(), Some("Some Artist"));
}

#[test]
fn stems_inherit_the_parents_tags_in_mp3() {
    use id3::TagLike;
    use musicai::stems::Stem;
    use musicai::tag::copy::copy_for_stem;

    let dir = Scratch::new("copy-mp3");
    let source = dir.track(Codec::Mp3);
    tag_like_a_library_file(&source);

    let stem_path = dir.0.join("melody.mp3");
    std::fs::copy(&source, &stem_path).unwrap();
    copy_for_stem(&source, &stem_path, Stem::Melody).unwrap();

    let tag = id3::Tag::read_from_path(&stem_path).unwrap();
    assert_eq!(tag.artist(), Some("Some Artist"));
    assert_eq!(tag.album(), Some("Some Album"));
    assert_eq!(tag.genre(), Some("Shoegaze"));
    assert_eq!(tag.title(), Some("Some Song (melody)"));

    let txxx: Vec<_> = tag.extended_texts().collect();
    assert!(txxx.iter().any(|t| t.description == "STEM" && t.value == "melody"));
    assert!(
        !txxx.iter().any(|t| t.description.starts_with("REPLAYGAIN_")),
        "replaygain was copied onto the stem"
    );
}

#[test]
fn crossing_formats_carries_the_fields_both_dialects_share() {
    use musicai::stems::Stem;
    use musicai::tag::copy::copy_for_stem;

    let dir = Scratch::new("copy-cross");
    let source = dir.track(Codec::Flac);
    tag_like_a_library_file(&source);

    // A flac parent with mp3 stems, as `--format mp3` would produce.
    let stem_path = dir.0.join("vocals.mp3");
    write_file(&stem_path, &decode_file(&source).unwrap(), Codec::Mp3, &EncodeOptions::default())
        .unwrap();
    copy_for_stem(&source, &stem_path, Stem::Vocals).unwrap();

    use id3::TagLike;
    let tag = id3::Tag::read_from_path(&stem_path).unwrap();
    assert_eq!(tag.artist(), Some("Some Artist"));
    assert_eq!(tag.title(), Some("Some Song (vocals)"));
    assert!(tag.extended_texts().any(|t| t.description == "STEM"));
}

#[test]
fn a_wav_stem_is_left_alone_rather_than_failing() {
    use musicai::stems::Stem;
    use musicai::tag::copy::copy_for_stem;

    let dir = Scratch::new("copy-wav");
    let source = dir.track(Codec::Flac);
    tag_like_a_library_file(&source);
    let stem_path = dir.track(Codec::Wav);

    // Wav has no standard tag; that must not abort the separation.
    copy_for_stem(&source, &stem_path, Stem::Drums).unwrap();
    assert!(decode_file(&stem_path).is_ok());
}
