//! Shared HTTP plumbing for the metadata services.
//!
//! Both AcoustID and MusicBrainz publish rate limits and both ask for a
//! User-Agent that identifies the application and gives them a way to get in
//! touch. MusicBrainz in particular will start returning 503 and eventually
//! block clients that ignore either rule, so the limiter here is not a
//! politeness nicety — it is what keeps the service usable.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};

/// When each host was last called, shared across every client in the process.
///
/// The limit belongs to the remote service, not to any one client object, so
/// tracking it per instance would let a caller sail past it just by
/// constructing a second client — which is exactly what happened the first
/// time these clients were tested. Keying on the host makes that impossible.
static LAST_REQUEST: Mutex<BTreeMap<String, Instant>> = Mutex::new(BTreeMap::new());

/// How many times to retry a request the service asked us to back off from.
const MAX_RETRIES: u32 = 3;

/// Identifies this application to the services, per MusicBrainz' requirement
/// that the agent name a real application and a contact address.
pub fn user_agent() -> String {
    format!(
        "{}/{} ( https://github.com/jwilkins/musicai )",
        env!("CARGO_PKG_NAME"),
        env!("CARGO_PKG_VERSION")
    )
}

/// A rate-limited HTTP client for one service.
pub struct Http {
    agent: ureq::Agent,
    user_agent: String,
    min_interval: Duration,
}

impl Http {
    /// Build a client that issues at most one request per `min_interval`.
    pub fn new(min_interval: Duration) -> Self {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            // Honour HTTPS_PROXY and friends, so this works behind a corporate
            // or sandboxed network the same way curl does.
            .proxy(ureq::Proxy::try_from_env())
            .timeout_global(Some(Duration::from_secs(30)))
            .build()
            .into();

        Self { agent, user_agent: user_agent(), min_interval }
    }

    /// Block until enough time has passed since the last call to this host.
    fn throttle(&self, url: &str) {
        let host = host_of(url);
        loop {
            let wait = {
                let mut last_request = LAST_REQUEST.lock().unwrap_or_else(|e| e.into_inner());
                let now = Instant::now();
                match last_request.get(&host) {
                    Some(last) if now.duration_since(*last) < self.min_interval => {
                        self.min_interval - now.duration_since(*last)
                    }
                    _ => {
                        last_request.insert(host, now);
                        return;
                    }
                }
            };
            // The lock is released before sleeping, so other threads waiting
            // on a different host are not held up by this one.
            std::thread::sleep(wait);
        }
    }

    /// GET a URL and parse the response as JSON.
    pub fn get_json(&mut self, url: &str) -> Result<serde_json::Value> {
        let body = self.get_string(url)?;
        serde_json::from_str(&body).with_context(|| format!("parsing the response from {url}"))
    }

    /// Issue a request, waiting out the rate limit and retrying with backoff
    /// when the service tells us to slow down.
    ///
    /// MusicBrainz answers with 503 rather than 429 when it wants a client to
    /// back off, and expects the client to wait and try again rather than
    /// treat it as a hard failure.
    fn with_retry<T>(
        &self,
        url: &str,
        mut attempt: impl FnMut() -> Result<T, ureq::Error>,
    ) -> Result<T> {
        let mut backoff = Duration::from_millis(500);
        let mut last_error = None;

        for _ in 0..=MAX_RETRIES {
            self.throttle(url);
            match attempt() {
                Ok(value) => return Ok(value),
                Err(e) if is_backoff(&e) => {
                    last_error = Some(e);
                    std::thread::sleep(backoff);
                    backoff *= 2;
                }
                Err(e) => return Err(describe(e, url)),
            }
        }

        Err(describe(last_error.expect("the loop only exits here after an error"), url))
    }

    /// GET a URL and return the body as text.
    pub fn get_string(&mut self, url: &str) -> Result<String> {
        let mut response = self.with_retry(url, || {
            self.agent
                .get(url)
                .header("User-Agent", &self.user_agent)
                .header("Accept", "application/json")
                .call()
        })?;
        response
            .body_mut()
            .read_to_string()
            .with_context(|| format!("reading the response body from {url}"))
    }

    /// GET a URL and return the raw bytes, for cover art.
    ///
    /// `max_bytes` caps what will be read, so a surprisingly large image
    /// cannot exhaust memory.
    pub fn get_bytes(&mut self, url: &str, max_bytes: usize) -> Result<Vec<u8>> {
        let mut response = self.with_retry(url, || {
            self.agent.get(url).header("User-Agent", &self.user_agent).call()
        })?;

        let bytes = response
            .body_mut()
            .with_config()
            .limit(max_bytes as u64)
            .read_to_vec()
            .with_context(|| format!("reading the response body from {url}"))?;
        Ok(bytes)
    }

    /// GET raw bytes, treating "not found" as an ordinary absence.
    ///
    /// Plenty of releases simply have no cover art, and that is not a failure
    /// worth aborting a run over.
    pub fn get_bytes_optional(&mut self, url: &str, max_bytes: usize) -> Result<Option<Vec<u8>>> {
        match self.get_bytes(url, max_bytes) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if is_not_found(&e) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// POST form fields and parse the response as JSON.
    ///
    /// Fingerprints run to several kilobytes, which is past what many servers
    /// accept in a query string, so the AcoustID lookup goes in a body.
    pub fn post_form_json(
        &mut self,
        url: &str,
        fields: &[(&str, &str)],
    ) -> Result<serde_json::Value> {
        let mut response = self.with_retry(url, || {
            self.agent
                .post(url)
                .header("User-Agent", &self.user_agent)
                .header("Accept", "application/json")
                .send_form(fields.iter().copied())
        })?;

        let body = response
            .body_mut()
            .read_to_string()
            .with_context(|| format!("reading the response body from {url}"))?;
        serde_json::from_str(&body).with_context(|| format!("parsing the response from {url}"))
    }
}

/// Marker put on 404 errors so callers can treat them as absence.
const NOT_FOUND_MARKER: &str = "HTTP 404";

fn is_not_found(error: &anyhow::Error) -> bool {
    error.to_string().contains(NOT_FOUND_MARKER)
}

/// Statuses that mean "try again later" rather than "this failed".
fn is_backoff(error: &ureq::Error) -> bool {
    matches!(error, ureq::Error::StatusCode(429) | ureq::Error::StatusCode(503))
}

/// Turn a transport error into something that says what to do about it.
fn describe(error: ureq::Error, url: &str) -> anyhow::Error {
    match &error {
        ureq::Error::StatusCode(code) if is_backoff(&error) => anyhow::anyhow!(
            "{url} is still rate limiting us (HTTP {code}) after {MAX_RETRIES} retries; \
             raise the interval between requests and try again"
        ),
        // Formatted so `is_not_found` can recognise a 404 downstream.
        ureq::Error::StatusCode(code) => anyhow::anyhow!("{url} returned HTTP {code}"),
        _ => anyhow::anyhow!("could not reach {url}: {error}"),
    }
}

/// Host part of a URL, used as the rate-limiting key. Anything unparseable
/// falls back to the whole string, which is conservative: it just means that
/// URL gets its own bucket.
fn host_of(url: &str) -> String {
    url.split_once("://")
        .map(|(_, rest)| rest.split(['/', '?', '#']).next().unwrap_or(rest))
        .unwrap_or(url)
        .to_ascii_lowercase()
}

/// Read a JSON string field, treating an explicit null the same as absent.
pub fn string_field(value: &serde_json::Value, key: &str) -> Option<String> {
    value.get(key)?.as_str().map(str::to_string)
}

/// Read a JSON integer field.
pub fn u32_field(value: &serde_json::Value, key: &str) -> Option<u32> {
    value.get(key)?.as_u64().and_then(|n| u32::try_from(n).ok())
}

/// Check a service response that reports failure in the body rather than the
/// status code, as AcoustID does.
pub fn check_status_field(response: &serde_json::Value, service: &str) -> Result<()> {
    match response.get("status").and_then(|s| s.as_str()) {
        Some("ok") => Ok(()),
        _ => {
            let message = response
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .unwrap_or("no error message");
            bail!("{service} rejected the request: {message}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_agent_names_the_app_and_a_contact() {
        let agent = user_agent();
        assert!(agent.starts_with("musicai/"), "{agent}");
        // MusicBrainz requires a contact URL or address in parentheses.
        assert!(agent.contains("( https://"), "{agent}");
    }

    #[test]
    fn throttle_spaces_requests_out() {
        let http = Http::new(Duration::from_millis(120));
        let url = "https://throttle-one.example/x";
        let start = Instant::now();
        http.throttle(url);
        http.throttle(url);
        http.throttle(url);
        // The first call is free; the next two each wait.
        assert!(
            start.elapsed() >= Duration::from_millis(240),
            "requests were not spaced out: {:?}",
            start.elapsed()
        );
    }

    #[test]
    fn throttle_is_shared_between_clients() {
        // The limit belongs to the service, so a second client must not get a
        // fresh allowance. This is the bug that made MusicBrainz return 503
        // when the live tests each built their own client.
        let url = "https://throttle-shared.example/x";
        Http::new(Duration::from_millis(150)).throttle(url);
        let start = Instant::now();
        Http::new(Duration::from_millis(150)).throttle(url);
        assert!(
            start.elapsed() >= Duration::from_millis(100),
            "a second client skipped the rate limit: {:?}",
            start.elapsed()
        );
    }

    #[test]
    fn throttle_is_per_host() {
        // One slow service must not hold up requests to a different one.
        Http::new(Duration::from_secs(30)).throttle("https://throttle-a.example/x");
        let start = Instant::now();
        Http::new(Duration::from_secs(30)).throttle("https://throttle-b.example/y");
        assert!(start.elapsed() < Duration::from_millis(50));
    }

    #[test]
    fn extracts_the_host_from_a_url() {
        assert_eq!(host_of("https://musicbrainz.org/ws/2/recording/x?fmt=json"), "musicbrainz.org");
        assert_eq!(host_of("https://API.AcoustID.org/v2/lookup"), "api.acoustid.org");
        assert_eq!(host_of("https://coverartarchive.org"), "coverartarchive.org");
        // Anything unparseable still yields a usable key.
        assert_eq!(host_of("not a url"), "not a url");
    }

    #[test]
    fn recognises_a_not_found_error() {
        let err = describe(ureq::Error::StatusCode(404), "https://x.example/a");
        assert!(is_not_found(&err), "404 was not recognised: {err}");

        let other = describe(ureq::Error::StatusCode(500), "https://x.example/a");
        assert!(!is_not_found(&other));
    }

    #[test]
    fn rate_limit_errors_explain_what_to_do() {
        let err = describe(ureq::Error::StatusCode(503), "https://x.example/a");
        assert!(err.to_string().contains("raise the interval"), "{err}");
        assert!(is_backoff(&ureq::Error::StatusCode(429)));
        assert!(!is_backoff(&ureq::Error::StatusCode(404)));
    }

    #[test]
    fn detects_a_failed_service_response() {
        let bad = serde_json::json!({
            "status": "error",
            "error": {"code": 4, "message": "invalid API key"}
        });
        let err = check_status_field(&bad, "AcoustID").unwrap_err();
        assert!(err.to_string().contains("invalid API key"), "{err}");

        let good = serde_json::json!({"status": "ok", "results": []});
        assert!(check_status_field(&good, "AcoustID").is_ok());
    }

    #[test]
    fn json_helpers_treat_null_as_absent() {
        let value = serde_json::json!({"a": "x", "b": null, "n": 7});
        assert_eq!(string_field(&value, "a"), Some("x".to_string()));
        assert_eq!(string_field(&value, "b"), None);
        assert_eq!(string_field(&value, "missing"), None);
        assert_eq!(u32_field(&value, "n"), Some(7));
        assert_eq!(u32_field(&value, "a"), None);
    }
}
