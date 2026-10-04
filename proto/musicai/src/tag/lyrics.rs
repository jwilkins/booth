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

/// How far a track's length may be from the one a lyric was written against.
///
/// Ten seconds. The point is not to be strict — a remix is a different length
/// by design and is still worth looking at — but to stop a radio edit being
/// served up as though it were the record. LRCLIB matches on duration itself
/// when one is given, so this is what decides whether to give it one.
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

/// A rate-limited LRCLIB client.
pub struct Client {
    http: Http,
}

impl Client {
    pub fn new(min_interval: Duration) -> Self {
        Self { http: Http::new(min_interval) }
    }

    /// Ask for one track's words.
    ///
    /// `duration_secs` is passed to the server where it is known, because that
    /// is how it tells one pressing from another — but only as a hint, since a
    /// miss on the duration is still a hit on the words.
    pub fn lookup(
        &mut self,
        artist: &str,
        title: &str,
        duration_secs: Option<f64>,
    ) -> Result<Option<Found>> {
        if artist.trim().is_empty() || title.trim().is_empty() {
            return Ok(None);
        }
        let mut url = format!(
            "{GET_URL}?artist_name={}&track_name={}",
            urlencoding(artist.trim()),
            urlencoding(title.trim())
        );
        if let Some(secs) = duration_secs.filter(|s| *s > 0.0) {
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
/// `heard` is what the recogniser made of the stem, which may be nothing: a
/// track nobody has listened to has no evidence either way, and the answer
/// then rests on the identification alone.
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
    // Not identified, or barely. The words are all there is.
    match agrees {
        Some(agrees) if agrees >= PLAINLY_THE_SAME => Verdict::Keep,
        Some(agrees) if agrees >= WORTH_ASKING => Verdict::Ask,
        Some(_) => Verdict::No,
        None if sure >= WORTH_ASKING_IDENTIFIED => Verdict::Ask,
        None => Verdict::No,
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

/// How sure a fingerprint has to be to be worth asking about on its own, with
/// no words heard yet to weigh against it.
const WORTH_ASKING_IDENTIFIED: f64 = 0.5;

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
    fn words_that_agree_with_nothing_heard_are_not_this_record() {
        let found = written(&["something else entirely", "nothing like it at all"], 240.0);
        let heard = sung(&["hold me closer now", "and then home", "walking through the city"]);
        assert_eq!(verdict(&found, &heard, Some(0.4), 240.0), Verdict::No);
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
