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

use booth_cli::tag::{acoustid, coverart, lyrics, musicbrainz, Metadata};

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

    // Synthetic audio matches nothing in the index, so this proves the request
    // is well formed — key, duration and above all the fingerprint encoding —
    // rather than that identification works.
    //
    // Identification itself was verified by hand against Nine Inch Nails' "The
    // Slip" (Creative Commons, from archive.org), which came back as
    // "1,000,000" at score 0.99 and "Letting You" at 0.98 with correct MBIDs
    // and track numbers. That cannot live here: it needs both an API key and a
    // real recording, and neither belongs in the repository.
    let audio = synthetic_audio(30);
    let fingerprint = booth_cli::tag::fingerprint::fingerprint(&audio).unwrap();

    let mut client = acoustid::Client::new(key, acoustid::DEFAULT_MIN_INTERVAL);
    let candidates =
        client.lookup(&fingerprint).expect("AcoustID rejected a fingerprint we generated");

    eprintln!("AcoustID returned {} candidates for synthetic audio", candidates.len());
}

/// Real music would be better, but there is none in the repository, and this
/// is enough to exercise the encoding.
fn synthetic_audio(seconds: usize) -> booth_cli::audio::Audio {
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
    booth_cli::audio::Audio::new(sample_rate as u32, vec![plane.clone(), plane]).unwrap()
}

#[test]
#[ignore = "hits the live lrclib service"]
fn lrclib_returns_words_with_times_on_them() {
    let mut client = lyrics::Client::new(lyrics::DEFAULT_MIN_INTERVAL);
    let found = client
        .lookup("Radiohead", "Creep", Some(239.0))
        .expect("the request failed")
        .expect("Creep should be in any lyrics database");

    assert_eq!(found.artist, "Radiohead");
    assert!(!found.plain.is_empty(), "no plain lyrics");
    assert!(!found.synced.lines.is_empty(), "no synced lyrics, which is the point of lrclib");
    assert!(found.synced.lines[0].start_ms > 0, "the first line should not be at zero");
    assert!(!found.instrumental);
}

#[test]
#[ignore = "hits the live lrclib service"]
fn lrclib_says_outright_when_a_record_is_an_instrumental() {
    // The answer worth the most here: it saves a stem separation and a pass
    // through the recogniser, and costs one request.
    let mut client = lyrics::Client::new(lyrics::DEFAULT_MIN_INTERVAL);
    let found = client
        .lookup("Floating Points", "Last Bloom", None)
        .expect("the request failed")
        .expect("the track should be in the database");
    assert!(found.instrumental, "{found:?}");
}

#[test]
#[ignore = "hits the live lrclib service"]
fn a_remix_finds_the_record_it_was_built_on() {
    // The case the whole lookup was asked for: a record a recogniser makes a
    // hash of, held as a club mix nothing like the length the database has it
    // at. `duration` on this endpoint is a filter rather than a hint, so
    // asking by the mix's own length is a 404 — and this used to stop there,
    // which made every remix in a library a miss.
    let mut client = lyrics::Client::new(lyrics::DEFAULT_MIN_INTERVAL);
    let found = client
        .lookup("Falco", "Der Kommissar", Some(350.0))
        .expect("the request failed")
        .expect("the record should be found at its own length even when the mix is not");

    assert!(!found.plain.is_empty(), "{found:?}");
    assert!(
        found.apart_from(350.0) > 60.0,
        "this is meant to be the other pressing, not a lucky exact match: {found:?}"
    );
    // Another pressing's words and another pressing's times, which is a
    // question for somebody rather than something to write in quietly.
    let verdict =
        lyrics::verdict(&found, &booth_cli::transcribe::Transcript::default(), None, 350.0);
    assert_eq!(verdict, lyrics::Verdict::Ask, "{found:?}");
}

#[test]
#[ignore = "hits the live lrclib service"]
fn a_track_at_its_own_length_is_taken_without_anybody_being_asked() {
    // The ordinary case, and the one that was broken: no stem, no words heard,
    // no fingerprint — just a name and a length, both of which agree with what
    // came back. Every track in a fresh library looks like this.
    let mut client = lyrics::Client::new(lyrics::DEFAULT_MIN_INTERVAL);
    let found = client
        .lookup("Burial", "Archangel", Some(239.0))
        .expect("the request failed")
        .expect("the track should be in the database");

    let verdict =
        lyrics::verdict(&found, &booth_cli::transcribe::Transcript::default(), None, 239.0);
    assert_eq!(verdict, lyrics::Verdict::Keep, "{found:?}");
    assert!(!found.synced.lines.is_empty(), "{found:?}");
}

#[test]
#[ignore = "hits the live lrclib service"]
fn a_track_the_database_has_never_heard_of_is_a_miss_and_not_a_failure() {
    // Most of a crate of white labels will land here, so it has to be an
    // ordinary answer rather than an error.
    let mut client = lyrics::Client::new(lyrics::DEFAULT_MIN_INTERVAL);
    let found = client
        .lookup("A Label Nobody Pressed", "A Track Nobody Cut", None)
        .expect("a miss should not be an error");
    assert!(found.is_none(), "{found:?}");
}
