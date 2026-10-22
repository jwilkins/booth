//! Timing somebody's words by what the recogniser heard.
//!
//! A lyrics server often has a record's words only as text, and a recogniser
//! has times only for words it half heard. Each has the half the other lacks,
//! and the karaoke tools that make timed lyrics out of a song put the two
//! together the same way: the words come from the people who wrote them down,
//! and the recogniser is asked only *when* things were sung. Measured on real
//! songs, a recogniser gets one sung word in three wrong; a lyric database gets
//! them right and knows nothing about time.
//!
//! The written words and the heard words are lined up as two sequences — the
//! longest common subsequence over normalised words, which keeps both in order
//! and so cannot put a repeated hook's second airing before its first — and
//! every written line takes the time of the first of its words that was heard.
//! A line none of whose words were heard is placed between its neighbours by
//! its share of the words between them. Where too little matches, nothing is
//! returned: a transcript that agrees with none of the words is either not
//! these words or not a transcript, and timing a lyric by it would be guessing.

use super::{words, Line, Transcript};

/// The share of the written words the recogniser has to have been heard to
/// sing before its times are trusted for the rest.
///
/// A third. A recogniser on a vocal stem gets about a third of sung words
/// wrong on the published benchmarks, and a remix drops verses the lyric has,
/// so the overlap between the two on a record they agree about is often not
/// much more than half. Below a third there is no saying whose words these
/// are.
const ENOUGH_HEARD: f32 = 0.34;

/// The fewest matched words worth anything. Two words in common is a
/// coincidence on any two songs in English.
const ENOUGH_WORDS: usize = 3;

/// How long the last line is reckoned to last, and the gap used to place a
/// line past the last one anybody was heard to sing.
const LAST_LINE_MS: u32 = 3_000;

/// One heard word, and when the recogniser's segment places it.
struct Heard {
    word: String,
    /// The segment it came from, so that the first word matched in a segment
    /// can take the segment's own start rather than a place inside it.
    segment: usize,
    at_ms: u32,
}

/// One written word, and the line it is in.
struct Written {
    word: String,
    line: usize,
}

/// Give the written lines the times the recogniser heard them at.
///
/// `written` is one line per entry, as a lyric file or a server has them;
/// `heard` is what the recogniser made of the stem, placed against the voice.
/// What comes back keeps every written line, as written, in order, with a
/// time each; or nothing, where the two agree about too few words to say
/// the heard times are these words' times.
pub fn time_words(written: &[String], heard: &Transcript) -> Option<Transcript> {
    let lines: Vec<&String> = written.iter().filter(|line| !line.trim().is_empty()).collect();
    if lines.is_empty() || heard.lines.is_empty() {
        return None;
    }

    let mut segments: Vec<&Line> = heard.lines.iter().collect();
    segments.sort_by_key(|line| line.start_ms);
    // Each word at its own time where the recogniser gave one: a segment can
    // be half a minute of a verse, and spread evenly across it a word lands
    // seconds from where it was sung. See `Transcript::words`.
    let timed = !heard.words.is_empty();
    let heard_words: Vec<Heard> = match timed {
        true => heard
            .words
            .iter()
            .flat_map(|word| words(&word.text).into_iter().map(|said| (said, word.start_ms)))
            .map(|(word, at_ms)| Heard { word, segment: 0, at_ms })
            .collect(),
        false => segments
            .iter()
            .enumerate()
            .flat_map(|(segment, line)| {
                let said = words(&line.text);
                let count = said.len().max(1) as u32;
                let span = line.end_ms.saturating_sub(line.start_ms);
                // The recogniser says when a segment starts and ends, not when
                // each word in it falls; spreading them evenly is the best
                // guess there is, and only the first word in a segment is
                // placed by anything better.
                said.into_iter().enumerate().map(move |(i, word)| Heard {
                    word,
                    segment,
                    at_ms: line.start_ms + span * i as u32 / count,
                })
            })
            .collect(),
    };
    let written_words: Vec<Written> = lines
        .iter()
        .enumerate()
        .flat_map(|(line, text)| words(text).into_iter().map(move |word| Written { word, line }))
        .collect();
    if written_words.is_empty() || heard_words.is_empty() {
        return None;
    }

    let matched = longest_common(&written_words, &heard_words);
    let share = matched.len() as f32 / written_words.len() as f32;
    if matched.len() < ENOUGH_WORDS || share < ENOUGH_HEARD {
        return None;
    }

    // Each line's time: the first of its words that was heard. Where that word
    // is the first one heard in its segment, the segment's own start is the
    // better answer — it is where the recogniser heard the phrase begin,
    // which is where a cue wants to be — and where an earlier line has
    // already taken that segment, the word's own place keeps the two lines
    // apart.
    let mut anchors: Vec<Option<u32>> = vec![None; lines.len()];
    let mut segment_taken = vec![false; segments.len()];
    for &(w, h) in &matched {
        let line = written_words[w].line;
        if anchors[line].is_some() {
            continue;
        }
        let heard = &heard_words[h];
        // A word with its own time is placed by it. Only a guessed place is
        // bettered by the segment's start.
        let at = match (timed, segment_taken.get(heard.segment).copied()) {
            (true, _) => heard.at_ms,
            (false, Some(false)) => segments[heard.segment].start_ms,
            (false, _) => heard.at_ms,
        };
        if let Some(taken) = segment_taken.get_mut(heard.segment) {
            *taken = true;
        }
        anchors[line] = Some(at);
    }
    // The anchors came from a monotonic alignment, but a segment start taken
    // for a later line can sit before a word-place taken for an earlier one;
    // a cue list has to run forwards.
    let mut last = 0u32;
    for anchor in anchors.iter_mut().flatten() {
        if *anchor < last {
            *anchor = last;
        }
        last = *anchor;
    }

    let starts = place_the_rest(&anchors, &lines);
    let timed = starts
        .iter()
        .enumerate()
        .map(|(i, &start_ms)| Line {
            start_ms,
            end_ms: starts.get(i + 1).copied().unwrap_or(start_ms + LAST_LINE_MS).max(start_ms),
            text: lines[i].trim().to_string(),
        })
        .collect();
    // Written lines, timed by ear: still somebody's lines.
    Some(Transcript {
        lines: timed,
        confidence: None,
        language: heard.language.clone(),
        written: true,
        words: Vec::new(),
    })
}

/// Fill in the lines nobody was heard to sing, between the ones somebody was.
///
/// Between two anchored lines, by each line's share of the words between them:
/// a long line takes longer to sing. Before the first and after the last, by
/// the typical gap between anchored lines, so a lyric whose first verse the
/// recogniser missed still starts before its second rather than on top of it.
fn place_the_rest(anchors: &[Option<u32>], lines: &[&String]) -> Vec<u32> {
    let known: Vec<(usize, u32)> =
        anchors.iter().enumerate().filter_map(|(i, at)| at.map(|at| (i, at))).collect();
    let mut gaps: Vec<u32> = known.windows(2).map(|pair| pair[1].1 - pair[0].1).collect();
    gaps.sort_unstable();
    let typical = gaps.get(gaps.len() / 2).copied().unwrap_or(LAST_LINE_MS).max(500);

    let mut starts = vec![0u32; anchors.len()];
    for &(i, at) in &known {
        starts[i] = at;
    }
    let (first, _) = known[0];
    for i in (0..first).rev() {
        starts[i] = starts[i + 1].saturating_sub(typical);
    }
    for pair in known.windows(2) {
        let ((from, from_at), (to, to_at)) = (pair[0], pair[1]);
        let span = (to_at - from_at) as u64;
        let total: u64 = (from..to).map(|i| words(lines[i]).len().max(1) as u64).sum();
        let mut sung = 0u64;
        for i in from + 1..to {
            sung += words(lines[i - 1]).len().max(1) as u64;
            starts[i] = from_at + (span * sung / total.max(1)) as u32;
        }
    }
    let (last, _) = known[known.len() - 1];
    for i in last + 1..anchors.len() {
        starts[i] = starts[i - 1] + typical;
    }
    starts
}

/// The longest common subsequence of the two word lists, as index pairs in
/// order. Two words match when they are the same, or one letter apart and
/// long enough for that to mean a slip rather than another word.
fn longest_common(written: &[Written], heard: &[Heard]) -> Vec<(usize, usize)> {
    let (n, m) = (written.len(), heard.len());
    let mut table = vec![vec![0u16; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            table[i][j] = match alike(&written[i].word, &heard[j].word) {
                true => table[i + 1][j + 1] + 1,
                false => table[i + 1][j].max(table[i][j + 1]),
            };
        }
    }
    let mut pairs = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if alike(&written[i].word, &heard[j].word) {
            pairs.push((i, j));
            i += 1;
            j += 1;
        } else if table[i + 1][j] >= table[i][j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    pairs
}

/// Whether two normalised words are the same word, give or take a slip.
fn alike(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    // Short words are too alike to begin with: "no" and "so" are two words.
    if a.chars().count() < 5 || b.chars().count() < 5 {
        return false;
    }
    one_edit_apart(a, b)
}

/// Whether one substitution, insertion or deletion turns one into the other.
fn one_edit_apart(a: &str, b: &str) -> bool {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.len().abs_diff(b.len()) > 1 {
        return false;
    }
    let (mut i, mut j, mut slips) = (0, 0, 0);
    while i < a.len() && j < b.len() {
        if a[i] == b[j] {
            i += 1;
            j += 1;
            continue;
        }
        slips += 1;
        if slips > 1 {
            return false;
        }
        match a.len().cmp(&b.len()) {
            std::cmp::Ordering::Equal => {
                i += 1;
                j += 1;
            }
            std::cmp::Ordering::Greater => i += 1,
            std::cmp::Ordering::Less => j += 1,
        }
    }
    slips + (a.len() - i) + (b.len() - j) <= 1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn heard(lines: &[(u32, u32, &str)]) -> Transcript {
        Transcript {
            lines: lines
                .iter()
                .map(|&(start_ms, end_ms, text)| Line { start_ms, end_ms, text: text.into() })
                .collect(),
            confidence: Some(0.8),
            language: Some("en".into()),
            written: false,
            words: Vec::new(),
        }
    }

    fn written(lines: &[&str]) -> Vec<String> {
        lines.iter().map(|line| line.to_string()).collect()
    }

    fn starts(transcript: &Transcript) -> Vec<u32> {
        transcript.lines.iter().map(|line| line.start_ms).collect()
    }

    #[test]
    fn written_lines_inside_one_long_segment_are_timed_by_the_words_heard_in_it() {
        // The report: a verse the recogniser heard as one segment from 3 s to
        // 33 s, with the hook sung at 7 s inside it. Spread evenly across the
        // segment its words land seconds from where they were sung; at their
        // own times, the hook's line is where the hook is.
        let sung = [
            (3_200, "hey there bankroll"),
            (5_000, "slow it down"),
            (7_000, "hold me closer now tonight"),
            (9_500, "shake it all around the room"),
            (20_000, "never coming for the drama"),
            (30_000, "slow it down"),
        ];
        let mut heard = heard(&[(
            3_000,
            33_000,
            &sung.iter().map(|(_, said)| *said).collect::<Vec<_>>().join(" "),
        )]);
        let written: Vec<String> = sung.iter().map(|(_, said)| said.to_string()).collect();
        let spread = time_words(&written, &heard).unwrap();
        heard.words = sung
            .iter()
            .flat_map(|&(at, said)| {
                said.split(' ').enumerate().map(move |(i, word)| crate::transcribe::Word {
                    start_ms: at + i as u32 * 250,
                    end_ms: at + i as u32 * 250 + 200,
                    text: word.to_string(),
                })
            })
            .collect();
        let timed = time_words(&written, &heard).unwrap();

        let hook = |transcript: &Transcript| {
            transcript
                .lines
                .iter()
                .find(|line| line.text == "hold me closer now tonight")
                .unwrap()
                .start_ms
        };
        assert_eq!(hook(&timed), 7_000, "{timed:?}");
        assert!(hook(&spread).abs_diff(7_000) > 2_000, "the fixture shows nothing: {spread:?}");
        let starts: Vec<u32> = timed.lines.iter().map(|line| line.start_ms).collect();
        assert_eq!(starts, sung.map(|(at, _)| at), "{timed:?}");
    }

    #[test]
    fn the_written_words_take_the_times_the_recogniser_heard_them_at() {
        let timed = time_words(
            &written(&["Hold me closer now", "And then home"]),
            &heard(&[(40_000, 42_000, "hold me closer now"), (100_000, 101_500, "and then home")]),
        )
        .unwrap();
        assert_eq!(starts(&timed), [40_000, 100_000]);
        // The words stay as written: the recogniser only lent its times.
        assert_eq!(timed.lines[0].text, "Hold me closer now");
        assert_eq!(timed.lines[0].end_ms, 100_000, "a line runs to the next");
        assert_eq!(timed.lines[1].end_ms, 103_000);
        assert_eq!(timed.language.as_deref(), Some("en"));
        assert_eq!(timed.confidence, None, "nobody's doubt attaches to written words");
    }

    #[test]
    fn a_line_half_misheard_still_lands_where_its_phrase_began() {
        // The measured case: "hold me closer now" came back as "Though we'll
        // meet closer now". Two of its words were heard, and they are enough
        // to place the written line at the start of the phrase they were in.
        let timed = time_words(
            &written(&["hold me closer now", "and then home"]),
            &heard(&[
                (40_000, 42_000, "Though we'll meet closer now"),
                (100_000, 101_500, "and then home"),
            ]),
        )
        .unwrap();
        assert_eq!(starts(&timed), [40_000, 100_000]);
    }

    #[test]
    fn two_lines_heard_as_one_segment_are_told_apart_by_their_words() {
        // The recogniser runs two lines together often. The first takes the
        // segment's start; the second the place of its first word within it.
        let timed = time_words(
            &written(&["hold me closer now", "and then home"]),
            &heard(&[(40_000, 47_000, "hold me closer now and then home")]),
        )
        .unwrap();
        // Seven words over seven seconds: "and" is the fifth, at 44 s.
        assert_eq!(starts(&timed), [40_000, 44_000]);
    }

    #[test]
    fn a_line_nobody_was_heard_to_sing_is_placed_between_its_neighbours() {
        let timed = time_words(
            &written(&["hold me closer now", "something quite different here", "and then home"]),
            &heard(&[(40_000, 42_000, "hold me closer now"), (60_000, 61_500, "and then home")]),
        )
        .unwrap();
        // Four words sung, then four more before the next anchor: halfway.
        assert_eq!(starts(&timed), [40_000, 50_000, 60_000]);
    }

    #[test]
    fn lines_before_the_first_one_heard_come_before_it_rather_than_on_top_of_it() {
        let timed = time_words(
            &written(&[
                "an intro nobody heard",
                "hold me closer now",
                "and then home",
                "closing line here",
            ]),
            &heard(&[(40_000, 42_000, "hold me closer now"), (50_000, 51_500, "and then home")]),
        )
        .unwrap();
        // The typical gap between what was heard is ten seconds, so the lines
        // nobody heard are a gap before and a gap after.
        assert_eq!(starts(&timed), [30_000, 40_000, 50_000, 60_000]);
    }

    #[test]
    fn a_hook_sung_twice_keeps_both_airings_in_order() {
        let timed = time_words(
            &written(&["get down", "the verse goes here", "get down"]),
            &heard(&[
                (10_000, 11_000, "get down"),
                (20_000, 23_000, "the verse goes here"),
                (30_000, 31_000, "get down"),
            ]),
        )
        .unwrap();
        assert_eq!(starts(&timed), [10_000, 20_000, 30_000]);
    }

    #[test]
    fn a_transcript_that_agrees_with_none_of_the_words_times_nothing() {
        // Either not this record, or not a transcript; timing the lyric by it
        // would be guessing, and nothing is what a caller can act on.
        assert!(time_words(
            &written(&["hold me closer now", "and then home"]),
            &heard(&[
                (40_000, 42_000, "thanks for watching"),
                (60_000, 61_000, "see you next time")
            ]),
        )
        .is_none());
        assert!(time_words(&written(&["hold me closer now"]), &heard(&[])).is_none());
        assert!(time_words(&written(&[]), &heard(&[(0, 1_000, "hold me")])).is_none());
    }

    #[test]
    fn a_slip_of_one_letter_is_the_same_word_where_the_word_is_long_enough() {
        assert!(alike("closer", "closet"));
        assert!(alike("watching", "watchin"));
        assert!(!alike("no", "so"), "short words are different words");
        assert!(!alike("closer", "chosen"), "two letters is another word");
        assert!(!alike("home", "hose"));
    }
}
