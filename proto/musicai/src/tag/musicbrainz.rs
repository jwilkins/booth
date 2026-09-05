//! MusicBrainz lookups.
//!
//! AcoustID tells us *which* recording a file is; MusicBrainz tells us
//! everything about it. No API key is needed — the service is open for
//! non-commercial use — but it does require a User-Agent that identifies the
//! application and no more than one request per second. Both are handled by
//! [`Http`].

use std::time::Duration;

use anyhow::{Context, Result};
use serde_json::Value;

use super::http::{string_field, u32_field, Http};

pub const BASE_URL: &str = "https://musicbrainz.org/ws/2";

/// MusicBrainz permits one request per second averaged over time. The extra
/// tenth of a second keeps us clear of the limit rather than riding it.
pub const DEFAULT_MIN_INTERVAL: Duration = Duration::from_millis(1_100);

/// A recording, with the release we picked to represent it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Recording {
    pub mbid: String,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub artist_mbid: Option<String>,
    /// Earliest known release date of the recording itself, used when the
    /// chosen release has no date of its own.
    pub first_release_date: Option<String>,
    pub release: Option<Release>,
}

/// The release (album) a recording appears on.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Release {
    pub mbid: String,
    pub title: Option<String>,
    pub date: Option<String>,
    pub album_artist: Option<String>,
    pub release_group_mbid: Option<String>,
    pub track_number: Option<u32>,
    pub total_tracks: Option<u32>,
    pub disc_number: Option<u32>,
    pub total_discs: Option<u32>,
}

pub struct Client {
    http: Http,
}

impl Client {
    pub fn new(min_interval: Duration) -> Self {
        Self { http: Http::new(min_interval) }
    }

    /// Look up a recording by MBID, including enough of its releases to work
    /// out album, date and track number.
    pub fn lookup_recording(&mut self, mbid: &str) -> Result<Recording> {
        let url = format!(
            "{BASE_URL}/recording/{mbid}?fmt=json\
             &inc=artist-credits+releases+release-groups+media"
        );
        let response = self
            .http
            .get_json(&url)
            .with_context(|| format!("looking up MusicBrainz recording {mbid}"))?;
        Ok(parse_recording(&response))
    }
}

/// Build a [`Recording`] from a MusicBrainz recording lookup response.
pub fn parse_recording(value: &Value) -> Recording {
    let (artist, artist_mbid) = artist_credit(value);

    Recording {
        mbid: string_field(value, "id").unwrap_or_default(),
        title: string_field(value, "title"),
        artist,
        artist_mbid,
        first_release_date: string_field(value, "first-release-date"),
        release: value
            .get("releases")
            .and_then(|r| r.as_array())
            .and_then(|releases| choose_release(releases))
            .map(parse_release),
    }
}

/// Pick the release that best represents a recording.
///
/// Preferring an official release keeps bootlegs and promos from supplying the
/// album name, and among equally official ones the earliest is the original
/// rather than a later reissue or compilation.
fn choose_release(releases: &[Value]) -> Option<&Value> {
    releases.iter().min_by(|a, b| rank(a).cmp(&rank(b)).then_with(|| date_key(a).cmp(&date_key(b))))
}

/// Lower ranks sort first.
fn rank(release: &Value) -> u8 {
    let official = matches!(string_field(release, "status").as_deref(), Some("Official"));
    // A compilation is a real release but a poor description of where a track
    // originally came from, so it loses to an official album.
    let compilation = release
        .get("release-group")
        .and_then(|g| g.get("secondary-types"))
        .and_then(|t| t.as_array())
        .is_some_and(|types| types.iter().any(|t| t.as_str() == Some("Compilation")));

    match (official, compilation) {
        (true, false) => 0,
        (true, true) => 1,
        (false, false) => 2,
        (false, true) => 3,
    }
}

/// Sort key that puts dated releases before undated ones.
fn date_key(release: &Value) -> (bool, String) {
    match string_field(release, "date") {
        Some(date) => (false, date),
        None => (true, String::new()),
    }
}

fn parse_release(value: &Value) -> Release {
    let (album_artist, _) = artist_credit(value);
    let media = value.get("media").and_then(|m| m.as_array());

    // The lookup returns only the medium and track matching our recording, so
    // the first entry is the one we want.
    let medium = media.and_then(|m| m.first());
    let track =
        medium.and_then(|m| m.get("tracks")).and_then(|t| t.as_array()).and_then(|t| t.first());

    Release {
        mbid: string_field(value, "id").unwrap_or_default(),
        title: string_field(value, "title"),
        date: string_field(value, "date"),
        album_artist,
        release_group_mbid: value.get("release-group").and_then(|g| string_field(g, "id")),
        // `position` is a plain number; the sibling `number` field can be
        // something like "A1" on a vinyl release, so it is no use as an
        // integer track number.
        track_number: track.and_then(|t| u32_field(t, "position")),
        total_tracks: medium.and_then(|m| u32_field(m, "track-count")),
        disc_number: medium.and_then(|m| u32_field(m, "position")),
        total_discs: media.map(|m| m.len() as u32),
    }
}

/// Join a MusicBrainz artist credit into a display string.
///
/// Credits are a list of parts with join phrases, so "Artist A feat. Artist B"
/// arrives as three pieces that have to be concatenated in order.
fn artist_credit(value: &Value) -> (Option<String>, Option<String>) {
    let Some(credits) = value.get("artist-credit").and_then(|c| c.as_array()) else {
        return (None, None);
    };
    if credits.is_empty() {
        return (None, None);
    }

    let mut name = String::new();
    for credit in credits {
        if let Some(part) = string_field(credit, "name") {
            name.push_str(&part);
        }
        if let Some(join) = string_field(credit, "joinphrase") {
            name.push_str(&join);
        }
    }

    let first_mbid =
        credits.first().and_then(|c| c.get("artist")).and_then(|a| string_field(a, "id"));

    (if name.is_empty() { None } else { Some(name) }, first_mbid)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Trimmed from a real response for Radiohead's "Creep".
    fn recording_json() -> Value {
        serde_json::json!({
            "id": "4bc78586-56ba-4473-bf35-87c1d79eff35",
            "title": "Creep",
            "first-release-date": "1992-09-21",
            "artist-credit": [{
                "name": "Radiohead",
                "joinphrase": "",
                "artist": {"id": "a74b1b7f-71a5-4011-9441-d0b5e4122711", "name": "Radiohead"}
            }],
            "releases": [
                {
                    "id": "bootleg-release",
                    "title": "Coup d'État",
                    "date": "2001-01-22",
                    "status": "Bootleg",
                    "release-group": {"id": "rg-boot", "secondary-types": ["Compilation"]},
                    "media": [{"position": 1, "track-count": 16,
                               "tracks": [{"position": 5, "number": "5", "title": "Creep"}]}]
                },
                {
                    "id": "official-release",
                    "title": "Pablo Honey",
                    "date": "1993-02-22",
                    "status": "Official",
                    "artist-credit": [{
                        "name": "Radiohead", "joinphrase": "",
                        "artist": {"id": "a74b1b7f-71a5-4011-9441-d0b5e4122711"}
                    }],
                    "release-group": {"id": "rg-pablo", "primary-type": "Album"},
                    "media": [{"position": 1, "track-count": 12,
                               "tracks": [{"position": 2, "number": "2", "title": "Creep"}]}]
                }
            ]
        })
    }

    #[test]
    fn reads_the_recording_fields() {
        let rec = parse_recording(&recording_json());
        assert_eq!(rec.title.as_deref(), Some("Creep"));
        assert_eq!(rec.artist.as_deref(), Some("Radiohead"));
        assert_eq!(rec.artist_mbid.as_deref(), Some("a74b1b7f-71a5-4011-9441-d0b5e4122711"));
        assert_eq!(rec.first_release_date.as_deref(), Some("1992-09-21"));
    }

    #[test]
    fn prefers_an_official_album_over_a_bootleg_compilation() {
        let rec = parse_recording(&recording_json());
        let release = rec.release.unwrap();
        assert_eq!(release.mbid, "official-release");
        assert_eq!(release.title.as_deref(), Some("Pablo Honey"));
        assert_eq!(release.track_number, Some(2));
        assert_eq!(release.total_tracks, Some(12));
        assert_eq!(release.disc_number, Some(1));
        assert_eq!(release.release_group_mbid.as_deref(), Some("rg-pablo"));
    }

    #[test]
    fn prefers_the_earliest_among_equally_ranked_releases() {
        let value = serde_json::json!({
            "id": "r", "title": "t",
            "releases": [
                {"id": "reissue", "title": "Later", "date": "2009-01-01", "status": "Official"},
                {"id": "original", "title": "First", "date": "1993-02-22", "status": "Official"}
            ]
        });
        let rec = parse_recording(&value);
        assert_eq!(rec.release.unwrap().mbid, "original");
    }

    #[test]
    fn undated_releases_lose_to_dated_ones() {
        let value = serde_json::json!({
            "id": "r", "title": "t",
            "releases": [
                {"id": "undated", "title": "No date", "status": "Official"},
                {"id": "dated", "title": "Dated", "date": "1995-01-01", "status": "Official"}
            ]
        });
        let rec = parse_recording(&value);
        assert_eq!(rec.release.unwrap().mbid, "dated");
    }

    #[test]
    fn joins_multi_artist_credits_with_their_join_phrases() {
        let value = serde_json::json!({
            "id": "r",
            "artist-credit": [
                {"name": "Artist A", "joinphrase": " feat. ", "artist": {"id": "mbid-a"}},
                {"name": "Artist B", "joinphrase": "", "artist": {"id": "mbid-b"}}
            ]
        });
        let (name, mbid) = artist_credit(&value);
        assert_eq!(name.as_deref(), Some("Artist A feat. Artist B"));
        // The MBID is the primary artist's, not the featured one's.
        assert_eq!(mbid.as_deref(), Some("mbid-a"));
    }

    #[test]
    fn copes_with_a_recording_that_has_no_releases() {
        let value = serde_json::json!({"id": "r", "title": "Untethered"});
        let rec = parse_recording(&value);
        assert_eq!(rec.title.as_deref(), Some("Untethered"));
        assert!(rec.release.is_none());
        assert!(rec.artist.is_none());
    }

    #[test]
    fn ignores_a_vinyl_style_track_number_in_favour_of_position() {
        let value = serde_json::json!({
            "id": "r",
            "releases": [{
                "id": "vinyl", "status": "Official", "date": "1980-01-01",
                "media": [{"position": 1, "track-count": 10,
                           "tracks": [{"position": 1, "number": "A1"}]}]
            }]
        });
        let rec = parse_recording(&value);
        assert_eq!(rec.release.unwrap().track_number, Some(1));
    }
}
