//! Looking a track's words up rather than listening for them.
//!
//! Reading the words off a record costs a stem separation and a pass through a
//! speech recogniser — minutes a track — and what comes back is a machine's
//! best guess at words somebody already wrote down. Where they have been
//! written down and can be found, that is a better answer and it arrives in
//! milliseconds.
//!
//! # What this is good for, and what it is not
//!
//! [LRCLIB](https://lrclib.net) is the one service that fits: free, no key, no
//! account, and it serves **line-by-line timestamps** rather than a wall of
//! text. Measured against it from here:
//!
//! | asked for | came back |
//! | --- | --- |
//! | Burial — Archangel | synced lyrics |
//! | Disclosure — Latch | synced lyrics |
//! | Floating Points — Last Bloom | `instrumental: true` |
//! | Peverelist — Roll With The Punches | known, no lyrics |
//! | Objekt — Ganzfeld | nothing |
//!
//! So for deep club records the hit rate is poor, and it should be: a
//! crowd-sourced database does not have the white labels and dubplates this
//! program is mostly for. Listening is still the answer there.
//!
//! The row that earns this its place is the third one. Being told a track is
//! an instrumental costs one request, where finding out by listening costs the
//! separation *and* the recogniser, and ends in "nothing was sung".
//!
//! Those were all asked for at the length the database holds them at. A DJ's
//! copy is usually some other length, and that is not a detail: `duration` on
//! this endpoint is a filter, so asking for a club mix by its own length is a
//! 404 even when the record is right there. Hence [`Client::lookup`] asking
//! twice.
//!
//! # The times are not this pressing's times
//!
//! A lyric is synced against one release. A DJ's library is extended mixes and
//! edits, and an LRC timed to a 3:30 radio cut is wrong for the 6:12 club mix
//! in every line but the first.
//!
//! Which is a problem this program has already solved for a different reason:
//! `crate::transcribe::align` exists because the recogniser anchors its first
//! segment at zero, and it moves lines whose *words* are right and whose
//! *times* are wrong onto singing measured off the stem. Server lyrics are the
//! same shape of problem, so they go through the same door — which is why what
//! is returned here is a transcript like any other, and why nothing in this
//! file tries to be clever about time.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};

use super::http::Http;
use crate::transcribe::{Line, Transcript};

/// Where to ask. Documented at <https://lrclib.net/docs>.
const GET_URL: &str = "https://lrclib.net/api/get";

/// One request a second. LRCLIB publishes no limit and asks only that clients
/// identify themselves, so this is the courtesy a service that costs nothing
/// is owed rather than a rule being obeyed.
pub const DEFAULT_MIN_INTERVAL: Duration = Duration::from_secs(1);

/// How far a track's length may be from the one a lyric was written against
/// and still be taken for the same pressing.
///
/// Ten seconds. Not a filter — a remix is a different length by design and is
/// still worth looking at — but the line between "these are this record's
/// words and its times" and "these are this record's words and somebody
/// else's times", which is what decides whether anybody is asked.
const SAME_LENGTH_SECS: f64 = 10.0;

/// What a lyrics server knows about a track.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Found {
    /// The words with their times, where the server had them timed. Empty when
    /// it only had them as text.
    pub synced: Transcript,
    /// The words as text, one line each, always present where there are any.
    pub plain: Vec<String>,
    /// The server saying outright that nothing is sung on this record.
    ///
    /// Worth more than it looks: this is the one answer that saves a stem
    /// separation and a recogniser pass, and it costs one request.
    pub instrumental: bool,
    /// What it thinks this is, for showing somebody who has to decide whether
    /// the match is real.
    pub artist: String,
    pub title: String,
    pub duration_secs: f64,
}

impl Found {
    /// Whether there is anything here worth keeping.
    pub fn is_empty(&self) -> bool {
        self.synced.lines.is_empty() && self.plain.is_empty() && !self.instrumental
    }

    /// How far this track's length is from the one the words were written
    /// against, in seconds. A remix is a different length by design, so this
    /// is reported rather than enforced.
    pub fn apart_from(&self, duration_secs: f64) -> f64 {
        (self.duration_secs - duration_secs).abs()
    }
}

/// What a lyric file beside the track may be called, in the order they are
/// looked for.
///
/// `.lrc` first because it carries times and a `.txt` does not, so where both
/// are there the one that can place a cue wins.
const LYRIC_FILES: [&str; 2] = ["lrc", "txt"];

/// A lyric file sitting beside a track, if there is one.
///
/// Same folder, same name, different extension — which is where every tool
/// that writes one puts it, and how a DJ who has collected them has them
/// filed. Worth looking before asking anybody: a file somebody put next to
/// *this* file is better evidence than a name-and-length match against a
/// database, it costs no request, and it works for the white labels no
/// database has heard of.
pub fn beside(track: &Path) -> Option<PathBuf> {
    let folder = track.parent()?;
    let stem = track.file_stem()?;
    LYRIC_FILES.iter().find_map(|extension| {
        let named = folder.join(stem).with_extension(extension);
        // Read back from the directory rather than trusted as spelled, so a
        // case-insensitive volume does not hand back a path that then fails to
        // open on a case-sensitive one.
        named.is_file().then_some(named)
    })
}

/// Read a lyric file off disk.
///
/// An `.lrc` is parsed for its times; anything else is taken as one line per
/// line, which is what a `.txt` is. The track's own length is reported as the
/// found length, because a file somebody filed under this name *is* about this
/// pressing — there is no other record it could be about — and saying
/// otherwise would send it to be asked about for no reason.
pub fn read_beside(file: &Path, duration_secs: f64) -> Result<Found> {
    let text =
        std::fs::read_to_string(file).with_context(|| format!("reading {}", file.display()))?;
    let synced = parse_lrc(&text);
    let plain: Vec<String> = match synced.lines.is_empty() {
        true => {
            text.lines().map(|line| line.trim().to_string()).filter(|l| !l.is_empty()).collect()
        }
        false => synced.lines.iter().map(|line| line.text.clone()).collect(),
    };
    Ok(Found {
        synced,
        plain,
        instrumental: false,
        artist: String::new(),
        title: file.file_name().unwrap_or_default().to_string_lossy().into_owned(),
        duration_secs,
    })
}

/// A rate-limited LRCLIB client.
pub struct Client {
    http: Http,
}

impl Client {
    pub fn new(min_interval: Duration) -> Self {
        Self { http: Http::new(min_interval) }
    }

    /// Ask for one track's words: this pressing if the server has it, the
    /// record otherwise.
    ///
    /// The length is **not** a hint. `duration` on this endpoint is a filter,
    /// and a length a couple of seconds out is a 404 — so asking for a 6:12
    /// club mix by its own length is a miss even when the database has the
    /// record, and this went looking for a problem it had created:
    ///
    /// | asked | with its length | without |
    /// | --- | --- | --- |
    /// | Falco — Der Kommissar, 5:50 | miss | 62 lines |
    /// | Floating Points — Last Bloom, 7:00 | miss | `instrumental: true` |
    ///
    /// Both of those are the cases this exists for. The first is the record a
    /// recogniser makes a hash of; the second is the one answer that saves a
    /// separation and a recogniser pass outright.
    ///
    /// So: with the length first, because a hit there is *this* pressing and
    /// its times are this pressing's times; then without it, which finds the
    /// record a remix was built on. The second request is only ever made after
    /// a miss, and what comes back at another length is for
    /// [`verdict`] to be careful with rather than for this to refuse.
    pub fn lookup(
        &mut self,
        artist: &str,
        title: &str,
        duration_secs: Option<f64>,
    ) -> Result<Option<Found>> {
        if artist.trim().is_empty() || title.trim().is_empty() {
            return Ok(None);
        }
        if let Some(secs) = duration_secs.filter(|secs| *secs > 0.0) {
            if let Some(found) = self.ask(artist, title, Some(secs))? {
                return Ok(Some(found));
            }
        }
        self.ask(artist, title, None)
    }

    /// One request, for exactly the length asked for or for any length.
    fn ask(&mut self, artist: &str, title: &str, secs: Option<f64>) -> Result<Option<Found>> {
        let mut url = format!(
            "{GET_URL}?artist_name={}&track_name={}",
            urlencoding(artist.trim()),
            urlencoding(title.trim())
        );
        if let Some(secs) = secs {
            url.push_str(&format!("&duration={}", secs.round() as u64));
        }

        let value = match self.http.get_json(&url) {
            Ok(value) => value,
            // A track the database has never heard of is the ordinary case,
            // not a failure: most of a crate of white labels will be misses.
            Err(e) if is_not_found(&e) => return Ok(None),
            Err(e) => return Err(e).context("asking lrclib for the words"),
        };
        Ok(Some(read(&value)?))
    }
}

/// Whether an error is the service saying it has never heard of the track.
fn is_not_found(error: &anyhow::Error) -> bool {
    let said = error.to_string();
    said.contains("404") || said.contains("not found")
}

/// Turn one of the server's answers into what this program keeps.
fn read(value: &serde_json::Value) -> Result<Found> {
    let text =
        |name: &str| value.get(name).and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    let plain: Vec<String> = value
        .get("plainLyrics")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .lines()
        .map(|line| line.trim().to_string())
        .filter(|line| !line.is_empty())
        .collect();

    Ok(Found {
        synced: parse_lrc(value.get("syncedLyrics").and_then(|v| v.as_str()).unwrap_or("")),
        plain,
        instrumental: value.get("instrumental").and_then(|v| v.as_bool()).unwrap_or(false),
        artist: text("artistName"),
        title: text("trackName"),
        duration_secs: value.get("duration").and_then(|v| v.as_f64()).unwrap_or(0.0),
    })
}

/// Read an LRC: `[mm:ss.cc] the words`, one line each.
///
/// Lines with no timestamp are the file's own header — the title, the person
/// who synced it — and are not words anybody sang. A line's end is the next
/// line's start, because an LRC says when a line begins and never when it
/// stops; the last one is given a couple of seconds, which is what a line
/// lasts and is long enough for nothing that reads this to care.
pub fn parse_lrc(text: &str) -> Transcript {
    let mut lines: Vec<Line> = Vec::new();
    for raw in text.lines() {
        let Some((stamp, said)) =
            raw.trim().strip_prefix('[').and_then(|rest| rest.split_once(']'))
        else {
            continue;
        };
        let Some(start_ms) = parse_stamp(stamp) else { continue };
        let said = said.trim();
        if said.is_empty() {
            continue;
        }
        lines.push(Line { start_ms, end_ms: start_ms, text: said.to_string() });
    }
    lines.sort_by_key(|line| line.start_ms);
    for i in 0..lines.len() {
        let ends = match lines.get(i + 1) {
            Some(next) => next.start_ms,
            None => lines[i].start_ms + LAST_LINE_MS,
        };
        lines[i].end_ms = ends.max(lines[i].start_ms);
    }
    Transcript { lines, ..Default::default() }
}

/// How long the last line of an LRC is reckoned to last.
const LAST_LINE_MS: u32 = 3_000;

/// `mm:ss.cc`, `mm:ss.ccc` or `mm:ss` into milliseconds.
fn parse_stamp(stamp: &str) -> Option<u32> {
    let (minutes, rest) = stamp.split_once(':')?;
    let minutes: u32 = minutes.trim().parse().ok()?;
    let (seconds, fraction) = match rest.split_once(['.', ':']) {
        Some((seconds, fraction)) => (seconds, fraction),
        None => (rest, ""),
    };
    let seconds: u32 = seconds.trim().parse().ok()?;
    // Hundredths in almost every file, thousandths in some. Read as whatever
    // it is rather than assumed, because reading thousandths as hundredths
    // puts every line ten times too far into the track.
    let fraction = fraction.trim();
    let sub_ms = match fraction.len() {
        0 => 0,
        1 => fraction.parse::<u32>().ok()? * 100,
        2 => fraction.parse::<u32>().ok()? * 10,
        _ => fraction.get(..3)?.parse::<u32>().ok()?,
    };
    Some(minutes * 60_000 + seconds * 1_000 + sub_ms)
}

/// Percent-encode a query parameter.
///
/// Written out rather than pulled in: the only thing needed is one parameter's
/// worth, and a dependency for it would be a dependency to keep up to date.
fn urlencoding(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// Whether a track's length is close enough to be the same pressing.
pub fn same_pressing(found: &Found, duration_secs: f64) -> bool {
    found.apart_from(duration_secs) <= SAME_LENGTH_SECS
}

/// How far to trust a lyric that was found, given what else is known.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Take it. The track was identified by its sound and the words line up
    /// with what is already known, or there was nothing to contradict them.
    Keep,
    /// Plausible, and not sure enough to write in without being looked at.
    Ask,
    /// Not this record.
    No,
}

/// Whether a found lyric belongs to this track.
///
/// Three things are weighed, and the first is worth more than the other two
/// together: **was the track identified by its sound.** A fingerprint match
/// above `sure_enough` means the artist and title were not guessed off a file
/// name, so a lyric found under them is for this record and the only question
/// left is which pressing.
///
/// Where the identification is weaker, the words themselves are the evidence.
/// A remix keeps the hook and drops the verses, so what is already heard off
/// the stem will overlap what was written down without matching it line for
/// line — which is exactly what [`crate::transcribe::same_line`] measures, and
/// a few lines in common is a signal nothing else explains.
///
/// `heard` is what the recogniser made of the stem, which may be nothing, and
/// most often is: the tracks worth looking up are the ones nobody has spent
/// the minutes listening to. What is left to weigh then is the name the server
/// was asked under and the length it answered with — which agree or they do
/// not, and were not derived from one another.
pub fn verdict(
    found: &Found,
    heard: &Transcript,
    identified: Option<f64>,
    duration_secs: f64,
) -> Verdict {
    if found.is_empty() {
        return Verdict::No;
    }
    let sure = identified.unwrap_or(0.0);
    let agrees = how_much_agrees(found, heard);

    // Identified by its sound, and the right length: nothing left to doubt.
    if sure >= SURE_ENOUGH && same_pressing(found, duration_secs) {
        return Verdict::Keep;
    }
    // Identified by its sound but a different length — a remix, an extended
    // mix, an edit. The words are probably still the words, and whether they
    // are is exactly what the lines already heard can say.
    if sure >= SURE_ENOUGH {
        return match agrees {
            Some(agrees) if agrees >= PLAINLY_THE_SAME => Verdict::Keep,
            Some(_) => Verdict::Ask,
            // Nothing heard to compare against. A fingerprint is still a
            // fingerprint, so this is worth putting to somebody rather than
            // dropping.
            None => Verdict::Ask,
        };
    }
    // Not identified, or barely. The words are the evidence where there are
    // any, and the name and the length where there are not.
    match agrees {
        Some(agrees) if agrees >= PLAINLY_THE_SAME => Verdict::Keep,
        Some(agrees) if agrees >= WORTH_ASKING => Verdict::Ask,
        // Heard plenty and they are not these words — but the length says
        // this is the record it was asked about, so the two cannot both be
        // right and somebody should say which. A recogniser handed a thick
        // accent or a language nobody told it about writes a transcript that
        // agrees with nothing, the real lyric included.
        Some(_) if same_pressing(found, duration_secs) => Verdict::Ask,
        Some(_) => Verdict::No,
        // Nothing heard, which is most of a library and the case this whole
        // feature exists for: it is the track nobody has spent the minutes on
        // that has the most to gain from somebody else having written the
        // words down.
        //
        // What there is to go on is real. The server was asked under this
        // track's artist and title, and answered with a record of this
        // track's length — two things agreeing that were not derived from
        // each other, which is how a lyrics service is meant to be used.
        // Refusing here is what made the lookup look like it was not running:
        // the words came back and were thrown away.
        None if same_pressing(found, duration_secs) => Verdict::Keep,
        // The same name at a different length: an edit, an extended mix, or a
        // different record that happens to share a title. Real evidence and
        // partial evidence at once, which is exactly what the sheet is for.
        None => Verdict::Ask,
    }
}

/// What fraction of the lines heard off the stem appear in what was found.
///
/// `None` when nothing has been heard, which is not the same as nothing
/// agreeing: a track that has never been through the recogniser has no opinion
/// rather than a dissenting one, and treating the two alike would reject every
/// lyric for a track nobody has listened to yet.
pub fn how_much_agrees(found: &Found, heard: &Transcript) -> Option<f32> {
    let mine: Vec<&str> = heard.lines.iter().map(|line| line.text.as_str()).collect();
    if mine.is_empty() {
        return None;
    }
    let theirs: Vec<&str> = found
        .plain
        .iter()
        .map(String::as_str)
        .chain(found.synced.lines.iter().map(|line| line.text.as_str()))
        .collect();
    if theirs.is_empty() {
        return Some(0.0);
    }
    let found_in_theirs = mine
        .iter()
        .filter(|line| theirs.iter().any(|other| crate::transcribe::same_line(line, other)))
        .count();
    Some(found_in_theirs as f32 / mine.len() as f32)
}

/// How sure a fingerprint match has to be before the words found under its
/// name are this record's words.
///
/// The same bar the collection uses before writing a name in unasked, because
/// it is the same question: is this identification good enough to act on
/// without being looked at.
const SURE_ENOUGH: f64 = 0.9;

/// How much of what was heard has to turn up in what was found before the two
/// are plainly the same song.
///
/// Half. A remix keeps the hook and throws away the verses, and a recogniser
/// mishears a fair share of what is left — so agreeing on half the lines is a
/// great deal of agreement, and nothing but the same song explains it.
const PLAINLY_THE_SAME: f32 = 0.5;

/// And how much makes it worth somebody's glance rather than nothing.
const WORTH_ASKING: f32 = 0.2;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_timestamp_is_read_as_whatever_precision_it_was_written_in() {
        // Thousandths read as hundredths put every line ten times too far
        // into the record, which is the kind of wrong that looks like a
        // feature until somebody loads the track.
        assert_eq!(parse_stamp("00:19.16"), Some(19_160));
        assert_eq!(parse_stamp("00:19.160"), Some(19_160));
        assert_eq!(parse_stamp("01:02"), Some(62_000));
        assert_eq!(parse_stamp("02:03.5"), Some(123_500));
        assert_eq!(parse_stamp("nonsense"), None);
    }

    #[test]
    fn an_lrc_becomes_a_transcript_with_each_line_running_to_the_next() {
        // An LRC says when a line begins and never when it stops.
        let lrc = "[00:19.16] When you were here before\n\
                   [00:24.09] Couldn't look you in the eye\n\
                   [00:29.24] You're just like an angel";
        let heard = parse_lrc(lrc);

        assert_eq!(heard.lines.len(), 3);
        assert_eq!(heard.lines[0].start_ms, 19_160);
        assert_eq!(heard.lines[0].end_ms, 24_090, "a line runs to the next one");
        assert_eq!(heard.lines[0].text, "When you were here before");
        assert_eq!(heard.lines[2].end_ms, 29_240 + LAST_LINE_MS);
    }

    #[test]
    fn an_lrcs_own_header_is_not_something_anybody_sang() {
        let lrc = "[ar: Radiohead]\n[ti: Creep]\nby somebody\n[00:19.16] When you were here before";
        let heard = parse_lrc(lrc);
        assert_eq!(heard.lines.len(), 1, "{:?}", heard.lines);
        assert_eq!(heard.lines[0].text, "When you were here before");
    }

    /// The shape of a real answer, trimmed. Captured from the service.
    const ANSWER: &str = r#"{
        "id": 496,
        "trackName": "Creep",
        "artistName": "Radiohead",
        "albumName": "Pablo Honey",
        "duration": 239.0,
        "instrumental": false,
        "plainLyrics": "When you were here before\nCouldn't look you in the eye\n",
        "syncedLyrics": "[00:19.16] When you were here before\n[00:24.09] Couldn't look you in the eye"
    }"#;

    #[test]
    fn an_answer_comes_back_as_words_both_ways() {
        let found = read(&serde_json::from_str(ANSWER).unwrap()).unwrap();
        assert_eq!(found.artist, "Radiohead");
        assert_eq!(found.plain.len(), 2);
        assert_eq!(found.synced.lines.len(), 2);
        assert!(!found.instrumental);
        assert!(!found.is_empty());
    }

    #[test]
    fn a_record_the_server_calls_an_instrumental_is_not_empty_handed() {
        // The answer that pays for this whole exercise: one request instead
        // of a separation and a recogniser pass.
        let said = r#"{ "trackName": "Last Bloom", "artistName": "Floating Points",
                        "duration": 420.0, "instrumental": true }"#;
        let found = read(&serde_json::from_str(said).unwrap()).unwrap();
        assert!(found.instrumental);
        assert!(!found.is_empty(), "being told it is an instrumental is an answer");
    }

    #[test]
    fn a_pressing_is_told_from_another_by_how_long_it_runs() {
        let found = Found { duration_secs: 239.0, ..Found::default() };
        assert!(same_pressing(&found, 240.0), "a second either way is the same record");
        assert!(!same_pressing(&found, 372.0), "a club mix is not the radio edit");
        assert_eq!(found.apart_from(372.0), 133.0);
    }

    fn sung(lines: &[&str]) -> Transcript {
        Transcript {
            lines: lines
                .iter()
                .enumerate()
                .map(|(i, text)| Line {
                    start_ms: i as u32 * 10_000,
                    end_ms: i as u32 * 10_000 + 2_000,
                    text: (*text).to_string(),
                })
                .collect(),
            ..Default::default()
        }
    }

    fn written(lines: &[&str], duration_secs: f64) -> Found {
        Found {
            plain: lines.iter().map(|l| (*l).to_string()).collect(),
            duration_secs,
            ..Found::default()
        }
    }

    /// A scratch folder of this test's own, made fresh. The crate keeps no
    /// temp-directory dependency and makes its own, as `discover` and
    /// `audio::encode` do.
    fn scratch(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("musicai-lyrics-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_lyric_file_is_found_beside_the_track_by_name() {
        let dir = scratch("beside");
        let track = dir.join("Peverelist - Roll With The Punches.wav");
        std::fs::write(&track, b"not really a wav").unwrap();
        assert!(beside(&track).is_none(), "nothing is there yet");

        let lyric = track.with_extension("lrc");
        std::fs::write(&lyric, "[00:10.00] hold me closer now\n").unwrap();
        assert_eq!(beside(&track), Some(lyric));
    }

    #[test]
    fn an_lrc_beside_the_track_wins_over_a_txt() {
        // Both filed, and only one of them can place a cue.
        let dir = scratch("lrc-wins");
        let track = dir.join("track.flac");
        std::fs::write(&track, b"not really a flac").unwrap();
        std::fs::write(track.with_extension("txt"), "hold me closer now\n").unwrap();
        std::fs::write(track.with_extension("lrc"), "[00:10.00] hold me closer now\n").unwrap();

        assert_eq!(beside(&track), Some(track.with_extension("lrc")));
    }

    #[test]
    fn a_lyric_file_is_read_as_this_pressing_rather_than_another() {
        // The length reported back is the track's own. A file somebody filed
        // under this name is about this record and no other, so sending it off
        // to be asked about would be asking a question already answered.
        let dir = scratch("read-lrc");
        let file = dir.join("track.lrc");
        std::fs::write(&file, "[00:10.00] hold me closer now\n[00:20.00] and then home\n").unwrap();

        let found = read_beside(&file, 372.0).unwrap();
        assert_eq!(found.plain, ["hold me closer now", "and then home"]);
        assert_eq!(found.synced.lines.len(), 2);
        assert_eq!(found.synced.lines[0].start_ms, 10_000);
        assert_eq!(found.duration_secs, 372.0);
        assert_eq!(verdict(&found, &Transcript::default(), None, 372.0), Verdict::Keep);
    }

    #[test]
    fn a_plain_text_lyric_has_its_lines_and_no_times() {
        let dir = scratch("read-txt");
        let file = dir.join("track.txt");
        std::fs::write(&file, "hold me closer now\n\n   and then home  \n").unwrap();

        let found = read_beside(&file, 240.0).unwrap();
        assert_eq!(found.plain, ["hold me closer now", "and then home"]);
        assert!(found.synced.lines.is_empty(), "a .txt has no times to find");
    }

    #[test]
    fn a_track_identified_by_its_sound_takes_the_words_found_under_its_name() {
        // The fingerprint did the work. Artist and title were not guessed off
        // a file name, so the words under them are this record's.
        let found = written(&["hold me closer now", "and then home"], 240.0);
        assert_eq!(verdict(&found, &Transcript::default(), Some(0.95), 240.0), Verdict::Keep);
    }

    #[test]
    fn a_remix_keeps_the_hook_and_that_is_enough_to_recognise_it_by() {
        // Der Kommissar is the case: a fingerprint match, a length nothing
        // like the original, and a recogniser that made a hash of the German.
        // What is left is the lines it did get, turning up in what was
        // written down.
        let found = written(
            &[
                "Der Kommissar ist in der Stadt",
                "alles klar",
                "zwei drei vier",
                "and the music is on",
            ],
            190.0,
        );
        let heard = sung(&["alles klar", "and the music is on"]);
        assert_eq!(verdict(&found, &heard, Some(0.95), 400.0), Verdict::Keep);
    }

    #[test]
    fn a_remix_with_nothing_heard_off_it_yet_is_put_to_somebody() {
        // A fingerprint and a length that does not match, with no words to
        // weigh. Worth a glance, not worth writing in unasked.
        let found = written(&["alles klar", "zwei drei vier"], 190.0);
        assert_eq!(verdict(&found, &Transcript::default(), Some(0.95), 400.0), Verdict::Ask);
    }

    #[test]
    fn words_that_agree_with_nothing_heard_at_another_length_are_not_this_record() {
        // Nothing in common and not even the same length. Two records that
        // share a name, which is the ordinary way a lyrics database is wrong.
        let found = written(&["something else entirely", "nothing like it at all"], 190.0);
        let heard = sung(&["hold me closer now", "and then home", "walking through the city"]);
        assert_eq!(verdict(&found, &heard, Some(0.4), 400.0), Verdict::No);
    }

    #[test]
    fn words_that_agree_with_nothing_heard_at_this_length_are_put_to_somebody() {
        // The same name and the same length, and a transcript that matches
        // none of it. One of the two is wrong and it is not decidable from
        // here: a recogniser given a thick accent, a language nobody told it
        // about, or a vocal buried in a club mix writes something that agrees
        // with nothing, the real lyric included. Dropping it quietly would
        // throw away the right words on the word of the wrong ones.
        let found = written(&["something else entirely", "nothing like it at all"], 240.0);
        let heard = sung(&["hold me closer now", "and then home", "walking through the city"]);
        assert_eq!(verdict(&found, &heard, Some(0.4), 240.0), Verdict::Ask);
    }

    #[test]
    fn a_track_nobody_has_listened_to_takes_a_lyric_of_its_own_length() {
        // The ordinary case, and the one the whole feature is for: a track
        // with no stem, no fingerprint and no words yet. The server was asked
        // under its artist and title and answered with a record of its length.
        // This used to come back `No`, which meant the words were fetched and
        // thrown away on nearly every track in a library — a lookup that
        // looked from the outside like a lookup that never ran.
        let found = written(&["hold me closer now", "and then home"], 240.0);
        assert_eq!(verdict(&found, &Transcript::default(), None, 240.0), Verdict::Keep);
        assert_eq!(verdict(&found, &Transcript::default(), None, 236.0), Verdict::Keep);
    }

    #[test]
    fn a_track_nobody_has_listened_to_is_asked_about_at_another_length() {
        // Same name, nothing heard, and eight minutes against four: an
        // extended mix of the right record, or a different record that shares
        // a title, and nothing here can tell which.
        let found = written(&["hold me closer now", "and then home"], 240.0);
        assert_eq!(verdict(&found, &Transcript::default(), None, 480.0), Verdict::Ask);
    }

    #[test]
    fn a_server_calling_a_record_an_instrumental_is_believed_at_the_same_length() {
        // The answer that earns the lookup its place: one request against a
        // separation and a recogniser pass that end in "nothing was sung".
        let found = Found { instrumental: true, duration_secs: 240.0, ..Found::default() };
        assert_eq!(verdict(&found, &Transcript::default(), None, 240.0), Verdict::Keep);
    }

    #[test]
    fn words_that_agree_without_any_identification_are_still_the_words() {
        // No fingerprint at all — a white label with tags off the file name.
        // Half the lines heard turning up in what was written down is not
        // something coincidence explains.
        let found = written(&["hold me closer now", "and then home", "a third line"], 240.0);
        let heard = sung(&["hold me closer now", "and then home"]);
        assert_eq!(verdict(&found, &heard, None, 240.0), Verdict::Keep);
    }

    #[test]
    fn a_little_agreement_is_asked_about_rather_than_taken_or_dropped() {
        let found = written(&["hold me closer now", "a", "b", "c", "d"], 240.0);
        let heard = sung(&["hold me closer now", "x y z", "p q r", "l m n"]);
        assert_eq!(verdict(&found, &heard, None, 240.0), Verdict::Ask);
    }

    #[test]
    fn nothing_found_is_never_kept() {
        assert_eq!(verdict(&Found::default(), &sung(&["a b"]), Some(1.0), 240.0), Verdict::No);
    }

    #[test]
    fn a_track_nobody_has_listened_to_has_no_opinion_rather_than_a_dissenting_one() {
        // The difference matters: treating "nothing heard" as "nothing agrees"
        // would reject every lyric for every track not yet transcribed, which
        // is most of the point of looking them up.
        let found = written(&["hold me closer now"], 240.0);
        assert_eq!(how_much_agrees(&found, &Transcript::default()), None);
        assert_eq!(how_much_agrees(&found, &sung(&["nothing like it"])), Some(0.0));
    }

    #[test]
    fn a_name_with_anything_awkward_in_it_survives_the_query() {
        assert_eq!(urlencoding("Falco"), "Falco");
        assert_eq!(urlencoding("Der Kommissar"), "Der%20Kommissar");
        assert_eq!(urlencoding("A&B / C?"), "A%26B%20%2F%20C%3F");
        assert_eq!(urlencoding("Bj\u{f6}rk"), "Bj%C3%B6rk");
    }
}
