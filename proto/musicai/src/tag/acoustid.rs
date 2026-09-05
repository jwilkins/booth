//! AcoustID lookups: fingerprint in, MusicBrainz recording IDs out.
//!
//! AcoustID is the index that maps acoustic fingerprints to recordings. It is
//! the one part of this pipeline that needs an API key; keys are free from
//! <https://acoustid.org/new-application>. MusicBrainz itself needs no key.

use std::time::Duration;

use anyhow::{bail, Result};
use serde_json::Value;

use super::fingerprint::Fingerprint;
use super::http::{check_status_field, string_field, Http};

pub const LOOKUP_URL: &str = "https://api.acoustid.org/v2/lookup";

/// AcoustID asks for no more than three requests a second.
pub const DEFAULT_MIN_INTERVAL: Duration = Duration::from_millis(340);

/// One possible identification of a track.
#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    /// The AcoustID itself, which identifies the audio rather than the work.
    pub acoustid: String,
    /// Confidence from 0 to 1, as reported by AcoustID.
    pub score: f64,
    /// MusicBrainz recording this fingerprint matched.
    pub recording_mbid: String,
    /// Title as AcoustID knows it. MusicBrainz is asked for the authoritative
    /// version later, but this is enough to show the user a choice.
    pub title: Option<String>,
    pub artist: Option<String>,
    pub artist_mbid: Option<String>,
}

pub struct Client {
    http: Http,
    api_key: String,
}

impl Client {
    pub fn new(api_key: String, min_interval: Duration) -> Self {
        Self { http: Http::new(min_interval), api_key }
    }

    /// Identify a fingerprint, returning candidates best-first.
    pub fn lookup(&mut self, fingerprint: &Fingerprint) -> Result<Vec<Candidate>> {
        if self.api_key.trim().is_empty() {
            bail!(
                "no AcoustID API key. Get a free one at https://acoustid.org/new-application \
                 and pass it with --acoustid-key or $ACOUSTID_API_KEY"
            );
        }

        let duration = fingerprint.duration_secs.to_string();
        // Fingerprints run to several kilobytes, past what many servers accept
        // in a query string, so this goes in a POST body.
        let response = self
            .http
            .post_form_json(
                LOOKUP_URL,
                &[
                    ("client", self.api_key.as_str()),
                    ("meta", "recordings"),
                    ("duration", duration.as_str()),
                    ("fingerprint", fingerprint.compressed.as_str()),
                ],
            )
            .map_err(explain_key_rejection)?;

        parse_candidates(&response).map_err(explain_key_rejection)
    }
}

/// Add the explanation that AcoustID's own message leaves out.
///
/// AcoustID issues two unrelated keys and says only "invalid API key" when it
/// gets the wrong one. The *application* key identifies the program and is
/// what lookups need; the *user* key identifies the account and is only for
/// submitting fingerprints. Both are short alphanumeric strings, so there is
/// nothing about a user key that looks wrong until the server rejects it.
fn explain_key_rejection(error: anyhow::Error) -> anyhow::Error {
    if !error.to_string().contains("invalid API key") {
        return error;
    }
    error.context(
        "AcoustID rejected the API key. Lookups need an *application* API key from \
         https://acoustid.org/my-applications — not the *user* API key from your account \
         preferences, which is only used for submitting fingerprints",
    )
}

/// Pull candidates out of an AcoustID lookup response, best score first.
///
/// A result can name several recordings — the same audio released as different
/// MusicBrainz recordings — and each becomes its own candidate carrying the
/// result's score.
pub fn parse_candidates(response: &Value) -> Result<Vec<Candidate>> {
    check_status_field(response, "AcoustID")?;

    let mut candidates = Vec::new();
    let results = response.get("results").and_then(|r| r.as_array());
    for result in results.into_iter().flatten() {
        let Some(acoustid) = string_field(result, "id") else { continue };
        let score = result.get("score").and_then(|s| s.as_f64()).unwrap_or(0.0);

        let recordings = result.get("recordings").and_then(|r| r.as_array());
        for recording in recordings.into_iter().flatten() {
            let Some(recording_mbid) = string_field(recording, "id") else { continue };
            let (artist, artist_mbid) = first_artist(recording);

            candidates.push(Candidate {
                acoustid: acoustid.clone(),
                score,
                recording_mbid,
                title: string_field(recording, "title"),
                artist,
                artist_mbid,
            });
        }
    }

    // Highest confidence first. Scores can tie, so keep it a stable sort and
    // leave AcoustID's own ordering to break ties.
    candidates.sort_by(|a, b| b.score.total_cmp(&a.score));
    Ok(candidates)
}

/// AcoustID reports artists as a list; the first is the primary credit.
fn first_artist(recording: &Value) -> (Option<String>, Option<String>) {
    let Some(artists) = recording.get("artists").and_then(|a| a.as_array()) else {
        return (None, None);
    };
    let Some(first) = artists.first() else {
        return (None, None);
    };
    (string_field(first, "name"), string_field(first, "id"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shaped like a real AcoustID response, including a result whose score is
    /// lower and one recording that is missing an id.
    fn sample() -> Value {
        serde_json::json!({
            "status": "ok",
            "results": [
                {
                    "id": "aaaaaaaa-0000-0000-0000-000000000001",
                    "score": 0.72,
                    "recordings": [
                        {
                            "id": "rec-low",
                            "title": "Creep (live)",
                            "artists": [{"id": "art-1", "name": "Radiohead"}]
                        }
                    ]
                },
                {
                    "id": "aaaaaaaa-0000-0000-0000-000000000002",
                    "score": 0.98,
                    "recordings": [
                        {
                            "id": "rec-high",
                            "title": "Creep",
                            "artists": [{"id": "art-1", "name": "Radiohead"}]
                        },
                        {
                            "title": "no id here"
                        },
                        {
                            "id": "rec-alt",
                            "title": "Creep"
                        }
                    ]
                }
            ]
        })
    }

    #[test]
    fn orders_candidates_by_confidence() {
        let candidates = parse_candidates(&sample()).unwrap();
        assert_eq!(candidates[0].recording_mbid, "rec-high");
        assert!((candidates[0].score - 0.98).abs() < 1e-9);
        assert_eq!(candidates.last().unwrap().recording_mbid, "rec-low");
    }

    #[test]
    fn expands_every_recording_of_a_result() {
        let candidates = parse_candidates(&sample()).unwrap();
        // rec-high, rec-alt and rec-low; the entry without an id is dropped.
        assert_eq!(candidates.len(), 3);
        assert!(candidates.iter().all(|c| !c.recording_mbid.is_empty()));
    }

    #[test]
    fn reads_artist_credit_when_present() {
        let candidates = parse_candidates(&sample()).unwrap();
        let top = &candidates[0];
        assert_eq!(top.artist.as_deref(), Some("Radiohead"));
        assert_eq!(top.artist_mbid.as_deref(), Some("art-1"));

        // And copes with a recording that has no artists array at all.
        let alt = candidates.iter().find(|c| c.recording_mbid == "rec-alt").unwrap();
        assert_eq!(alt.artist, None);
    }

    #[test]
    fn surfaces_the_service_error_message() {
        let response = serde_json::json!({
            "status": "error",
            "error": {"code": 4, "message": "invalid API key"}
        });
        let err = parse_candidates(&response).unwrap_err();
        assert!(err.to_string().contains("invalid API key"), "{err}");
    }

    #[test]
    fn a_rejected_key_explains_which_of_the_two_keys_is_needed() {
        // "invalid API key" on its own sends people to check for a typo in a
        // key that is perfectly valid — just the wrong one of AcoustID's two.
        let rejected = anyhow::anyhow!("returned HTTP 400: invalid API key");
        let explained = explain_key_rejection(rejected);
        let message = format!("{explained:#}");

        assert!(message.contains("application"), "{message}");
        assert!(message.contains("my-applications"), "{message}");
        // The original cause is still there, not replaced.
        assert!(message.contains("invalid API key"), "{message}");
    }

    #[test]
    fn unrelated_errors_are_passed_through_untouched() {
        let other = anyhow::anyhow!("could not reach https://api.acoustid.org: timed out");
        let passed = explain_key_rejection(other);
        assert!(!format!("{passed:#}").contains("my-applications"));
    }

    #[test]
    fn handles_a_successful_lookup_with_no_matches() {
        let response = serde_json::json!({"status": "ok", "results": []});
        assert!(parse_candidates(&response).unwrap().is_empty());
    }

    #[test]
    fn missing_key_is_reported_before_any_request() {
        let mut client = Client::new("  ".to_string(), Duration::from_millis(1));
        let fingerprint = Fingerprint { compressed: "x".into(), duration_secs: 100 };
        let err = client.lookup(&fingerprint).unwrap_err();
        assert!(err.to_string().contains("acoustid.org/new-application"), "{err}");
    }
}
