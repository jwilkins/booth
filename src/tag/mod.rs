//! Identifying tracks by sound and writing what we learn into their tags.
//!
//! The pipeline is: fingerprint the audio locally, ask AcoustID which
//! MusicBrainz recording that fingerprint belongs to, ask MusicBrainz for the
//! details, and write those into the file.
//!
//! Unlike the rest of this tool, this is **not** an offline operation. The
//! fingerprint is computed locally, but identifying it means sending it to
//! AcoustID, which needs a free API key. MusicBrainz needs no key but does
//! require a descriptive User-Agent and no more than one request per second.

pub mod acoustid;
pub mod coverart;
pub mod fingerprint;
pub mod http;
pub mod musicbrainz;

use std::fmt;
use std::path::Path;

use anyhow::{bail, Context, Result};

use crate::audio::encode::Codec;

/// The metadata fields this tool knows how to write.
///
/// Keeping them as one enum means the "keep or overwrite" policy is applied
/// identically to every field and to both tag formats, rather than being
/// re-implemented per field.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Field {
    Title,
    Artist,
    Album,
    AlbumArtist,
    Date,
    TrackNumber,
    TotalTracks,
    DiscNumber,
    TotalDiscs,
    RecordingMbid,
    ReleaseMbid,
    ReleaseGroupMbid,
    ArtistMbid,
    AcoustId,
}

impl Field {
    pub const ALL: [Field; 14] = [
        Field::Title,
        Field::Artist,
        Field::Album,
        Field::AlbumArtist,
        Field::Date,
        Field::TrackNumber,
        Field::TotalTracks,
        Field::DiscNumber,
        Field::TotalDiscs,
        Field::RecordingMbid,
        Field::ReleaseMbid,
        Field::ReleaseGroupMbid,
        Field::ArtistMbid,
        Field::AcoustId,
    ];

    /// Vorbis comment key, as MusicBrainz Picard writes it.
    pub fn vorbis_key(self) -> &'static str {
        match self {
            Field::Title => "TITLE",
            Field::Artist => "ARTIST",
            Field::Album => "ALBUM",
            Field::AlbumArtist => "ALBUMARTIST",
            Field::Date => "DATE",
            Field::TrackNumber => "TRACKNUMBER",
            Field::TotalTracks => "TOTALTRACKS",
            Field::DiscNumber => "DISCNUMBER",
            Field::TotalDiscs => "TOTALDISCS",
            Field::RecordingMbid => "MUSICBRAINZ_TRACKID",
            Field::ReleaseMbid => "MUSICBRAINZ_ALBUMID",
            Field::ReleaseGroupMbid => "MUSICBRAINZ_RELEASEGROUPID",
            Field::ArtistMbid => "MUSICBRAINZ_ARTISTID",
            Field::AcoustId => "ACOUSTID_ID",
        }
    }

    /// Human-readable name for reports.
    pub fn label(self) -> &'static str {
        match self {
            Field::Title => "title",
            Field::Artist => "artist",
            Field::Album => "album",
            Field::AlbumArtist => "album artist",
            Field::Date => "date",
            Field::TrackNumber => "track number",
            Field::TotalTracks => "total tracks",
            Field::DiscNumber => "disc number",
            Field::TotalDiscs => "total discs",
            Field::RecordingMbid => "recording MBID",
            Field::ReleaseMbid => "release MBID",
            Field::ReleaseGroupMbid => "release group MBID",
            Field::ArtistMbid => "artist MBID",
            Field::AcoustId => "AcoustID",
        }
    }
}

impl fmt::Display for Field {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// Everything we learned about one track.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Metadata {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub date: Option<String>,
    pub track_number: Option<u32>,
    pub total_tracks: Option<u32>,
    pub disc_number: Option<u32>,
    pub total_discs: Option<u32>,
    pub recording_mbid: Option<String>,
    pub release_mbid: Option<String>,
    pub release_group_mbid: Option<String>,
    pub artist_mbid: Option<String>,
    pub acoustid: Option<String>,
}

impl Metadata {
    /// Assemble metadata from a MusicBrainz recording and the AcoustID that
    /// led us to it.
    pub fn from_musicbrainz(recording: &musicbrainz::Recording, acoustid: Option<&str>) -> Self {
        let release = recording.release.as_ref();
        Self {
            title: recording.title.clone(),
            artist: recording.artist.clone(),
            album: release.and_then(|r| r.title.clone()),
            // Fall back to the recording's own artist when the release does not
            // name one, which is the common case for a single-artist album.
            album_artist: release
                .and_then(|r| r.album_artist.clone())
                .or_else(|| recording.artist.clone()),
            // A release date describes this particular pressing; the
            // recording's first release date is the better answer when the
            // chosen release has none.
            date: release
                .and_then(|r| r.date.clone())
                .or_else(|| recording.first_release_date.clone()),
            track_number: release.and_then(|r| r.track_number),
            total_tracks: release.and_then(|r| r.total_tracks),
            disc_number: release.and_then(|r| r.disc_number),
            total_discs: release.and_then(|r| r.total_discs),
            recording_mbid: Some(recording.mbid.clone()).filter(|s| !s.is_empty()),
            release_mbid: release.map(|r| r.mbid.clone()).filter(|s| !s.is_empty()),
            release_group_mbid: release.and_then(|r| r.release_group_mbid.clone()),
            artist_mbid: recording.artist_mbid.clone(),
            acoustid: acoustid.map(str::to_string),
        }
    }

    /// The value for `field`, if we have one.
    pub fn get(&self, field: Field) -> Option<String> {
        match field {
            Field::Title => self.title.clone(),
            Field::Artist => self.artist.clone(),
            Field::Album => self.album.clone(),
            Field::AlbumArtist => self.album_artist.clone(),
            Field::Date => self.date.clone(),
            Field::TrackNumber => self.track_number.map(|n| n.to_string()),
            Field::TotalTracks => self.total_tracks.map(|n| n.to_string()),
            Field::DiscNumber => self.disc_number.map(|n| n.to_string()),
            Field::TotalDiscs => self.total_discs.map(|n| n.to_string()),
            Field::RecordingMbid => self.recording_mbid.clone(),
            Field::ReleaseMbid => self.release_mbid.clone(),
            Field::ReleaseGroupMbid => self.release_group_mbid.clone(),
            Field::ArtistMbid => self.artist_mbid.clone(),
            Field::AcoustId => self.acoustid.clone(),
        }
    }

    /// A one-line summary for progress output.
    pub fn describe(&self) -> String {
        let artist = self.artist.as_deref().unwrap_or("unknown artist");
        let title = self.title.as_deref().unwrap_or("unknown title");
        match &self.album {
            Some(album) => format!("{artist} - {title} ({album})"),
            None => format!("{artist} - {title}"),
        }
    }
}

/// What to do about a field that already has a value.
#[derive(Copy, Clone, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum OnExisting {
    /// Leave it alone. Only empty fields are filled.
    Keep,
    /// Replace it with the MusicBrainz value.
    Overwrite,
    /// Leave it alone, but report where MusicBrainz disagrees.
    Report,
}

/// What writing tags to one file actually did.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TagOutcome {
    /// Fields that were written.
    pub written: Vec<Field>,
    /// Fields left alone because they already had a matching value.
    pub unchanged: Vec<Field>,
    /// Fields left alone whose existing value differs from MusicBrainz.
    pub conflicts: Vec<Conflict>,
    /// Whether cover art was embedded.
    pub cover_art: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conflict {
    pub field: Field,
    pub existing: String,
    pub proposed: String,
}

impl TagOutcome {
    pub fn changed_anything(&self) -> bool {
        !self.written.is_empty() || self.cover_art
    }
}

/// A tag container we can read fields from and write fields to.
///
/// The keep/overwrite decision is identical for Vorbis comments and ID3, so it
/// lives in [`plan`] and works through this trait rather than being written
/// twice.
trait TagTarget {
    fn get(&self, field: Field) -> Option<String>;
    fn set(&mut self, field: Field, value: &str);
    fn set_cover_art(&mut self, art: &coverart::CoverArt) -> Result<()>;
}

/// Decide what to do with each field, without touching the file.
fn plan<T: TagTarget>(
    target: &T,
    metadata: &Metadata,
    policy: OnExisting,
) -> (Vec<(Field, String)>, TagOutcome) {
    let mut writes = Vec::new();
    let mut outcome = TagOutcome::default();

    for field in Field::ALL {
        let Some(proposed) = metadata.get(field) else { continue };
        match target.get(field) {
            // Empty fields are always filled, whatever the policy: there is
            // nothing there to protect.
            None => writes.push((field, proposed)),
            Some(existing) if existing.trim().is_empty() => writes.push((field, proposed)),
            Some(existing) if existing == proposed => outcome.unchanged.push(field),
            Some(existing) => match policy {
                OnExisting::Overwrite => writes.push((field, proposed)),
                OnExisting::Keep => outcome.unchanged.push(field),
                OnExisting::Report => {
                    outcome.conflicts.push(Conflict { field, existing, proposed })
                }
            },
        }
    }

    outcome.written = writes.iter().map(|(field, _)| *field).collect();
    (writes, outcome)
}

/// Write `metadata` into the tags of an existing file.
///
/// The audio itself is untouched — only the tag blocks are rewritten.
pub fn write_tags(
    path: &Path,
    metadata: &Metadata,
    policy: OnExisting,
    cover: Option<&coverart::CoverArt>,
) -> Result<TagOutcome> {
    match Codec::from_path(path) {
        Some(Codec::Flac) => write_flac(path, metadata, policy, cover),
        Some(Codec::Mp3) => write_mp3(path, metadata, policy, cover),
        Some(Codec::Wav) => {
            bail!("wav has no standard metadata tag; {} cannot be tagged", path.display())
        }
        None => bail!("cannot tell what kind of file {} is", path.display()),
    }
    .with_context(|| format!("writing tags to {}", path.display()))
}

// -- FLAC ------------------------------------------------------------------

struct VorbisTarget<'a>(&'a mut metaflac::Tag);

impl TagTarget for VorbisTarget<'_> {
    fn get(&self, field: Field) -> Option<String> {
        self.0
            .get_vorbis(field.vorbis_key())
            .and_then(|mut values| values.next().map(str::to_string))
    }

    fn set(&mut self, field: Field, value: &str) {
        self.0.set_vorbis(field.vorbis_key(), vec![value.to_string()]);
    }

    fn set_cover_art(&mut self, art: &coverart::CoverArt) -> Result<()> {
        use metaflac::block::PictureType;
        // Replace rather than accumulate: re-tagging a file should not leave
        // it carrying three front covers.
        self.0.remove_picture_type(PictureType::CoverFront);
        self.0.add_picture(art.mime_type, PictureType::CoverFront, art.data.clone());
        Ok(())
    }
}

fn write_flac(
    path: &Path,
    metadata: &Metadata,
    policy: OnExisting,
    cover: Option<&coverart::CoverArt>,
) -> Result<TagOutcome> {
    let mut tag = metaflac::Tag::read_from_path(path)?;
    let (writes, mut outcome) = {
        let target = VorbisTarget(&mut tag);
        plan(&target, metadata, policy)
    };

    let mut target = VorbisTarget(&mut tag);
    for (field, value) in &writes {
        target.set(*field, value);
    }
    if let Some(art) = cover {
        target.set_cover_art(art)?;
        outcome.cover_art = true;
    }

    if outcome.changed_anything() {
        tag.save()?;
    }
    Ok(outcome)
}

// -- MP3 -------------------------------------------------------------------

struct Id3Target<'a>(&'a mut id3::Tag);

/// ID3 has dedicated frames for the common fields and a free-form `TXXX`
/// frame for everything else. Picard's `TXXX` descriptions are the de facto
/// standard, so other software recognises what we write.
fn id3_txxx_description(field: Field) -> Option<&'static str> {
    match field {
        Field::RecordingMbid => Some("MusicBrainz Release Track Id"),
        Field::ReleaseMbid => Some("MusicBrainz Album Id"),
        Field::ReleaseGroupMbid => Some("MusicBrainz Release Group Id"),
        Field::ArtistMbid => Some("MusicBrainz Artist Id"),
        Field::AcoustId => Some("Acoustid Id"),
        _ => None,
    }
}

impl TagTarget for Id3Target<'_> {
    fn get(&self, field: Field) -> Option<String> {
        use id3::TagLike;
        if let Some(description) = id3_txxx_description(field) {
            return self
                .0
                .extended_texts()
                .find(|t| t.description == description)
                .map(|t| t.value.clone());
        }
        match field {
            Field::Title => self.0.title().map(str::to_string),
            Field::Artist => self.0.artist().map(str::to_string),
            Field::Album => self.0.album().map(str::to_string),
            Field::AlbumArtist => self.0.album_artist().map(str::to_string),
            Field::Date => self.0.date_recorded().map(|d| d.to_string()),
            Field::TrackNumber => self.0.track().map(|n| n.to_string()),
            Field::TotalTracks => self.0.total_tracks().map(|n| n.to_string()),
            Field::DiscNumber => self.0.disc().map(|n| n.to_string()),
            Field::TotalDiscs => self.0.total_discs().map(|n| n.to_string()),
            _ => None,
        }
    }

    fn set(&mut self, field: Field, value: &str) {
        use id3::frame::ExtendedText;
        use id3::TagLike;

        if let Some(description) = id3_txxx_description(field) {
            self.0.remove_extended_text(Some(description), None);
            self.0.add_frame(ExtendedText {
                description: description.to_string(),
                value: value.to_string(),
            });
            return;
        }

        match field {
            Field::Title => self.0.set_title(value),
            Field::Artist => self.0.set_artist(value),
            Field::Album => self.0.set_album(value),
            Field::AlbumArtist => self.0.set_album_artist(value),
            Field::Date => {
                if let Some(timestamp) = parse_id3_date(value) {
                    self.0.set_date_recorded(timestamp);
                }
            }
            Field::TrackNumber => {
                if let Ok(n) = value.parse() {
                    self.0.set_track(n);
                }
            }
            Field::TotalTracks => {
                if let Ok(n) = value.parse() {
                    self.0.set_total_tracks(n);
                }
            }
            Field::DiscNumber => {
                if let Ok(n) = value.parse() {
                    self.0.set_disc(n);
                }
            }
            Field::TotalDiscs => {
                if let Ok(n) = value.parse() {
                    self.0.set_total_discs(n);
                }
            }
            _ => {}
        }
    }

    fn set_cover_art(&mut self, art: &coverart::CoverArt) -> Result<()> {
        use id3::frame::{Picture, PictureType};
        use id3::TagLike;

        self.0.remove_picture_by_type(PictureType::CoverFront);
        self.0.add_frame(Picture {
            mime_type: art.mime_type.to_string(),
            picture_type: PictureType::CoverFront,
            description: String::new(),
            data: art.data.clone(),
        });
        Ok(())
    }
}

/// MusicBrainz dates are `YYYY`, `YYYY-MM` or `YYYY-MM-DD`; ID3 timestamps
/// take the same shape but have to be built field by field.
fn parse_id3_date(value: &str) -> Option<id3::Timestamp> {
    let mut parts = value.split('-');
    let year = parts.next()?.parse().ok()?;
    let month = parts.next().and_then(|m| m.parse::<u8>().ok());
    let day = parts.next().and_then(|d| d.parse::<u8>().ok());
    Some(id3::Timestamp { year, month, day, hour: None, minute: None, second: None })
}

fn write_mp3(
    path: &Path,
    metadata: &Metadata,
    policy: OnExisting,
    cover: Option<&coverart::CoverArt>,
) -> Result<TagOutcome> {
    // A file with no tag yet is normal, not an error.
    let mut tag = id3::Tag::read_from_path(path).unwrap_or_default();
    let (writes, mut outcome) = {
        let target = Id3Target(&mut tag);
        plan(&target, metadata, policy)
    };

    let mut target = Id3Target(&mut tag);
    for (field, value) in &writes {
        target.set(*field, value);
    }
    if let Some(art) = cover {
        target.set_cover_art(art)?;
        outcome.cover_art = true;
    }

    if outcome.changed_anything() {
        tag.write_to_path(path, id3::Version::Id3v24)?;
    }
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_metadata() -> Metadata {
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

    /// An in-memory target, so the policy logic can be tested without files.
    #[derive(Default)]
    struct FakeTarget(std::collections::BTreeMap<Field, String>);

    impl TagTarget for FakeTarget {
        fn get(&self, field: Field) -> Option<String> {
            self.0.get(&field).cloned()
        }
        fn set(&mut self, field: Field, value: &str) {
            self.0.insert(field, value.to_string());
        }
        fn set_cover_art(&mut self, _: &coverart::CoverArt) -> Result<()> {
            Ok(())
        }
    }

    #[test]
    fn builds_metadata_from_a_musicbrainz_recording() {
        let recording = musicbrainz::Recording {
            mbid: "rec".into(),
            title: Some("Creep".into()),
            artist: Some("Radiohead".into()),
            artist_mbid: Some("art".into()),
            first_release_date: Some("1992-09-21".into()),
            release: Some(musicbrainz::Release {
                mbid: "rel".into(),
                title: Some("Pablo Honey".into()),
                date: Some("1993-02-22".into()),
                album_artist: None,
                release_group_mbid: Some("rg".into()),
                track_number: Some(2),
                total_tracks: Some(12),
                disc_number: Some(1),
                total_discs: Some(1),
            }),
        };

        let meta = Metadata::from_musicbrainz(&recording, Some("acoustid"));
        assert_eq!(meta.album.as_deref(), Some("Pablo Honey"));
        // The release date wins over the recording's first release date.
        assert_eq!(meta.date.as_deref(), Some("1993-02-22"));
        // With no album artist on the release, the track artist stands in.
        assert_eq!(meta.album_artist.as_deref(), Some("Radiohead"));
        assert_eq!(meta.acoustid.as_deref(), Some("acoustid"));
    }

    #[test]
    fn falls_back_to_the_first_release_date() {
        let recording = musicbrainz::Recording {
            mbid: "rec".into(),
            first_release_date: Some("1992-09-21".into()),
            release: Some(musicbrainz::Release { mbid: "rel".into(), ..Default::default() }),
            ..Default::default()
        };
        assert_eq!(
            Metadata::from_musicbrainz(&recording, None).date.as_deref(),
            Some("1992-09-21")
        );
    }

    #[test]
    fn fills_every_empty_field() {
        let target = FakeTarget::default();
        let (writes, outcome) = plan(&target, &sample_metadata(), OnExisting::Keep);
        assert_eq!(writes.len(), Field::ALL.len());
        assert_eq!(outcome.written.len(), Field::ALL.len());
        assert!(outcome.conflicts.is_empty());
    }

    #[test]
    fn keep_policy_protects_existing_values() {
        let mut target = FakeTarget::default();
        target.set(Field::Title, "My Careful Title");

        let (writes, outcome) = plan(&target, &sample_metadata(), OnExisting::Keep);
        assert!(!writes.iter().any(|(f, _)| *f == Field::Title));
        assert!(outcome.unchanged.contains(&Field::Title));
        assert!(outcome.conflicts.is_empty());
    }

    #[test]
    fn overwrite_policy_replaces_existing_values() {
        let mut target = FakeTarget::default();
        target.set(Field::Title, "My Careful Title");

        let (writes, outcome) = plan(&target, &sample_metadata(), OnExisting::Overwrite);
        assert!(writes.iter().any(|(f, v)| *f == Field::Title && v == "Creep"));
        assert!(outcome.written.contains(&Field::Title));
    }

    #[test]
    fn report_policy_records_disagreements_without_writing() {
        let mut target = FakeTarget::default();
        target.set(Field::Title, "My Careful Title");
        target.set(Field::Album, "Pablo Honey"); // agrees

        let (writes, outcome) = plan(&target, &sample_metadata(), OnExisting::Report);
        assert!(!writes.iter().any(|(f, _)| *f == Field::Title));
        assert_eq!(outcome.conflicts.len(), 1);
        assert_eq!(outcome.conflicts[0].field, Field::Title);
        assert_eq!(outcome.conflicts[0].existing, "My Careful Title");
        assert_eq!(outcome.conflicts[0].proposed, "Creep");
        // A field that already agrees is not a conflict.
        assert!(outcome.unchanged.contains(&Field::Album));
    }

    #[test]
    fn a_whitespace_only_value_counts_as_empty() {
        let mut target = FakeTarget::default();
        target.set(Field::Title, "   ");
        let (writes, _) = plan(&target, &sample_metadata(), OnExisting::Keep);
        assert!(writes.iter().any(|(f, _)| *f == Field::Title));
    }

    #[test]
    fn fields_we_have_no_value_for_are_skipped() {
        let sparse = Metadata { title: Some("Only a title".into()), ..Default::default() };
        let (writes, _) = plan(&FakeTarget::default(), &sparse, OnExisting::Keep);
        assert_eq!(writes.len(), 1);
        assert_eq!(writes[0].0, Field::Title);
    }

    #[test]
    fn parses_partial_musicbrainz_dates() {
        let full = parse_id3_date("1993-02-22").unwrap();
        assert_eq!((full.year, full.month, full.day), (1993, Some(2), Some(22)));

        let year_only = parse_id3_date("1993").unwrap();
        assert_eq!((year_only.year, year_only.month), (1993, None));

        let year_month = parse_id3_date("1993-02").unwrap();
        assert_eq!((year_month.year, year_month.month, year_month.day), (1993, Some(2), None));

        assert!(parse_id3_date("not a date").is_none());
    }

    #[test]
    fn vorbis_keys_match_the_picard_convention() {
        assert_eq!(Field::RecordingMbid.vorbis_key(), "MUSICBRAINZ_TRACKID");
        assert_eq!(Field::ReleaseMbid.vorbis_key(), "MUSICBRAINZ_ALBUMID");
        assert_eq!(Field::AlbumArtist.vorbis_key(), "ALBUMARTIST");
    }

    #[test]
    fn refuses_wav() {
        let err =
            write_tags(Path::new("x.wav"), &sample_metadata(), OnExisting::Keep, None).unwrap_err();
        assert!(err.to_string().contains("no standard metadata tag"), "{err}");
    }

    #[test]
    fn describes_a_track_for_display() {
        assert_eq!(sample_metadata().describe(), "Radiohead - Creep (Pablo Honey)");
        let partial = Metadata { title: Some("Untitled".into()), ..Default::default() };
        assert_eq!(partial.describe(), "unknown artist - Untitled");
    }
}
