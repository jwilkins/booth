//! Tests that talk to the real MusicBrainz, Cover Art Archive and AcoustID
//! services.
//!
//! These are `#[ignore]`d so the normal test run stays offline and
//! deterministic. Run them deliberately when changing the clients:
//!
//! ```sh
//! cargo test --release --test live_services -- --ignored --test-threads=1
//! ```
//!
//! `--test-threads=1` matters: the rate limiters are per-client, so running
//! these concurrently would exceed what MusicBrainz permits. The AcoustID test
//! additionally needs `ACOUSTID_API_KEY` set and skips itself without one.

use std::time::Duration;

use musicai::tag::{acoustid, coverart, musicbrainz, Metadata};

/// Radiohead, "Creep". A long-established recording, unlikely to be merged
/// away and so stable enough to assert against.
const KNOWN_RECORDING: &str = "4bc78586-56ba-4473-bf35-87c1d79eff35";

#[test]
#[ignore = "hits the live MusicBrainz service"]
fn musicbrainz_lookup_returns_usable_metadata() {
    let mut client = musicbrainz::Client::new(musicbrainz::DEFAULT_MIN_INTERVAL);
    let recording = client.lookup_recording(KNOWN_RECORDING).unwrap();

    assert_eq!(recording.mbid, KNOWN_RECORDING);
    assert_eq!(recording.title.as_deref(), Some("Creep"));
    assert_eq!(recording.artist.as_deref(), Some("Radiohead"));
    assert!(recording.artist_mbid.is_some(), "no artist MBID");

    // And it turns into metadata we would actually write.
    let metadata = Metadata::from_musicbrainz(&recording, Some("test-acoustid"));
    assert_eq!(metadata.title.as_deref(), Some("Creep"));
    assert!(metadata.date.is_some(), "no date on {metadata:?}");
    assert_eq!(metadata.acoustid.as_deref(), Some("test-acoustid"));
}

#[test]
#[ignore = "hits the live MusicBrainz service"]
fn musicbrainz_reports_a_missing_recording_clearly() {
    let mut client = musicbrainz::Client::new(musicbrainz::DEFAULT_MIN_INTERVAL);

    // A well-formed UUID that is not in the database. MusicBrainz
    // distinguishes this from a malformed one: a random UUID gets 404, while
    // something like the all-zeros MBID is rejected as invalid with a 400.
    let err = client.lookup_recording("0b46839c-618f-4a05-8379-6e2ddb7a1943").unwrap_err();
    let message = format!("{err:#}");
    assert!(message.contains("404"), "expected a not-found error, got: {message}");
    // And the message says which recording failed, not just that something did.
    assert!(message.contains("0b46839c"), "error does not name the recording: {message}");
}

#[test]
#[ignore = "hits the live Cover Art Archive"]
fn cover_art_archive_returns_a_real_image_or_a_clean_miss() {
    let mut client =
        coverart::Client::new(coverart::DEFAULT_MIN_INTERVAL, coverart::DEFAULT_MAX_BYTES);

    // Look up a release that definitely exists, via its recording.
    let mut mb = musicbrainz::Client::new(musicbrainz::DEFAULT_MIN_INTERVAL);
    let recording = mb.lookup_recording(KNOWN_RECORDING).unwrap();
    let release = recording.release.expect("no release on the recording");

    match client.front(&release.mbid).unwrap() {
        Some(art) => {
            assert!(art.data.len() > 1_000, "suspiciously small image");
            assert!(art.mime_type.starts_with("image/"), "not an image: {}", art.mime_type);
        }
        // Plenty of releases have no front cover; that must not be an error.
        None => eprintln!("release {} has no front cover, which is fine", release.mbid),
    }
}

#[test]
#[ignore = "hits the live Cover Art Archive"]
fn cover_art_for_an_unknown_release_is_absence_not_failure() {
    let mut client = coverart::Client::new(Duration::from_millis(10), 1_000_000);
    let art = client.front("00000000-0000-0000-0000-000000000000").unwrap();
    assert!(art.is_none(), "expected no art for a nonexistent release");
}

#[test]
#[ignore = "hits the live AcoustID service and needs ACOUSTID_API_KEY"]
fn acoustid_accepts_a_fingerprint_we_generated() {
    let Ok(key) = std::env::var("ACOUSTID_API_KEY") else {
        eprintln!("ACOUSTID_API_KEY is not set; skipping");
        return;
    };

    // A fingerprint of synthetic audio will match nothing in the index, but a
    // successful empty result still proves the whole request is well formed:
    // the key, the duration, and above all the fingerprint encoding.
    let audio = synthetic_audio(30);
    let fingerprint = musicai::tag::fingerprint::fingerprint(&audio).unwrap();

    let mut client = acoustid::Client::new(key, acoustid::DEFAULT_MIN_INTERVAL);
    let candidates =
        client.lookup(&fingerprint).expect("AcoustID rejected a fingerprint we generated");

    eprintln!("AcoustID returned {} candidates for synthetic audio", candidates.len());
}

/// Real music would be better, but there is none in the repository, and this
/// is enough to exercise the encoding.
fn synthetic_audio(seconds: usize) -> musicai::audio::Audio {
    let sample_rate = 44_100usize;
    let frames = sample_rate * seconds;
    let mut plane = Vec::with_capacity(frames);
    for i in 0..frames {
        let t = i as f32 / sample_rate as f32;
        let step = (t as usize % 4) as f32;
        let root = 220.0 * (1.0 + 0.06 * step);
        let mut v = 0.0;
        for harmonic in [1.0, 1.26, 1.5, 2.0] {
            v += 0.2 * (2.0 * std::f32::consts::PI * root * harmonic * t).sin();
        }
        plane.push(v * 0.5);
    }
    musicai::audio::Audio::new(sample_rate as u32, vec![plane.clone(), plane]).unwrap()
}
