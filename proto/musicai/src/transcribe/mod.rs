//! Reading the words out of a vocal stem, and finding what the track keeps
//! coming back to.
//!
//! A DJ's most useful cue is rarely the loudest moment. It is the line the
//! crowd sings — the one that comes round three times and that everyone knows
//! is about to land. Energy analysis cannot see that: a hook and the verse
//! before it are the same loudness, the same instruments and the same key. The
//! words are the only thing that tells them apart.
//!
//! So the vocal stem is transcribed, and the transcript is read for repetition.
//! Two passes, neither of them clever: group the lines that say the same thing,
//! then take the group with the most separate airings. That is the hook, and
//! where it first lands is where the cue goes.
//!
//! The transcription itself is somebody else's program — see [`whisper`]. What
//! is here is everything after it, which is the part that has opinions.

pub mod whisper;

/// One stretch of speech, as the recogniser heard it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    pub start_ms: u32,
    pub end_ms: u32,
    pub text: String,
}

/// What was sung, and when.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Transcript {
    pub lines: Vec<Line>,
}

/// A line the track returns to, and every time it does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refrain {
    /// The wording it was heard with most often.
    pub text: String,
    /// Where it comes round, in milliseconds, in time order.
    pub at: Vec<u32>,
}

impl Refrain {
    /// How many separate times it is sung.
    pub fn times(&self) -> usize {
        self.at.len()
    }
}

/// Something worth cueing that the words found.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum MomentKind {
    /// The first sung word in the track.
    VocalIn,
    /// The first landing of the line the track repeats most.
    Hook,
    /// Every landing after that.
    Refrain,
}

/// One of those, placed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Moment {
    pub time_ms: u32,
    pub kind: MomentKind,
    /// The words, for a cue that can be read rather than guessed at.
    pub text: String,
}

/// How alike two lines have to be to count as the same line, on the Dice
/// measure below: 1.0 is word for word.
///
/// Well short of 1.0 on purpose. A recogniser hearing the same sung phrase four
/// times writes it four slightly different ways — a swallowed "the", a
/// homophone, a comma — and matching exactly would count a hook sung eight
/// times as eight different lines, which is the one failure that makes the
/// whole exercise pointless.
const SAME: f32 = 0.75;

/// The fewest words a line can have and still be a hook.
///
/// One word is an ad-lib. "Oh", "yeah" and "hey" are the most repeated tokens
/// in most vocal stems and mark nothing at all.
const MIN_WORDS: usize = 2;

/// How far apart two airings of a line have to be to count as two.
///
/// Within a chorus a line lands twice in a few seconds, and a recogniser that
/// loses its place repeats itself far faster than that. Neither is a second
/// cue: a cue four seconds after the last one is a cue nobody can use.
const APART_MS: u32 = 4_000;

/// How many times a line has to come round before it is the hook rather than
/// just a line.
const MIN_TIMES: usize = 2;

/// The longest comment worth putting on a cue. A player shows a line of text
/// under the cue, not a verse.
const COMMENT_CHARS: usize = 40;

impl Transcript {
    pub fn is_empty(&self) -> bool {
        self.lines.iter().all(|line| words(&line.text).is_empty())
    }

    /// The lines the track comes back to, most repeated first.
    ///
    /// Ties go to the longer line and then to the earlier one: between two
    /// lines sung three times each, the longer is the one somebody wrote a
    /// chorus around and the shorter is a tag on the end of it.
    pub fn refrains(&self) -> Vec<Refrain> {
        let mut groups: Vec<Group> = Vec::new();
        for line in &self.lines {
            let said = words(&line.text);
            if said.len() < MIN_WORDS {
                continue;
            }
            match groups.iter_mut().find(|group| group.matches(&said)) {
                Some(group) => group.add(line.start_ms, said, &line.text),
                None => groups.push(Group::new(line.start_ms, said, &line.text)),
            }
        }

        let mut refrains: Vec<Refrain> = groups.into_iter().map(Group::into_refrain).collect();
        refrains.sort_by(|a, b| {
            b.times()
                .cmp(&a.times())
                .then(words(&b.text).len().cmp(&words(&a.text).len()))
                .then(a.at.first().cmp(&b.at.first()))
        });
        refrains
    }

    /// The line the track repeats most, if any line is repeated at all.
    pub fn hook(&self) -> Option<Refrain> {
        self.refrains().into_iter().find(|refrain| refrain.times() >= MIN_TIMES)
    }

    /// Where the singing starts.
    pub fn vocal_in(&self) -> Option<u32> {
        self.lines.iter().find(|line| !words(&line.text).is_empty()).map(|line| line.start_ms)
    }

    /// Everything in the words that is worth a cue, in time order.
    ///
    /// The hook wins any argument with the entry: on a track that opens on its
    /// chorus they are the same moment, and calling it "the voice comes in"
    /// when it is the hook throws away the more useful of the two names.
    pub fn moments(&self) -> Vec<Moment> {
        let mut moments = Vec::new();
        let hook = self.hook();

        if let Some(hook) = &hook {
            for (index, &at) in hook.at.iter().enumerate() {
                let kind = match index {
                    0 => MomentKind::Hook,
                    _ => MomentKind::Refrain,
                };
                moments.push(Moment { time_ms: at, kind, text: comment(&hook.text) });
            }
        }

        if let Some(at) = self.vocal_in() {
            let claimed = moments.iter().any(|m| m.time_ms.abs_diff(at) < APART_MS);
            if !claimed {
                moments.push(Moment {
                    time_ms: at,
                    kind: MomentKind::VocalIn,
                    text: String::new(),
                });
            }
        }

        moments.sort_by_key(|moment| moment.time_ms);
        moments
    }
}

/// A line under construction: every wording it has been heard with, every time
/// it was heard, and which of those count as separate airings.
struct Group {
    /// The words of each hearing, with the text it was written down as.
    heard: Vec<(Vec<String>, String)>,
    /// When it was last heard at all, which is what decides whether the next
    /// hearing is a new airing or the same one still going.
    last_seen: u32,
    at: Vec<u32>,
}

impl Group {
    fn new(at: u32, said: Vec<String>, text: &str) -> Self {
        Self { heard: vec![(said, tidy(text))], last_seen: at, at: vec![at] }
    }

    /// Whether a line belongs here. Compared against every wording already in
    /// the group rather than one representative, so that a group joined by a
    /// half-heard line still recognises the full one.
    fn matches(&self, said: &[String]) -> bool {
        self.heard.iter().any(|(known, _)| similarity(known, said) >= SAME)
    }

    fn add(&mut self, at: u32, said: Vec<String>, text: &str) {
        self.heard.push((said, tidy(text)));
        // Measured from the last time the line was heard at all, not from the
        // last airing that was counted. A chorus that sings its line twice over
        // is one airing; so is a recogniser that has lost its place and is
        // emitting the same line every second, which would otherwise read as an
        // airing every four seconds for as long as it kept going.
        if at.saturating_sub(self.last_seen) >= APART_MS {
            self.at.push(at);
        }
        self.last_seen = at;
    }

    fn into_refrain(self) -> Refrain {
        Refrain { text: wording(&self.heard), at: self.at }
    }
}

/// Which of a group's wordings to show: the one heard most, and the longest of
/// those when there is no clear winner.
///
/// Heard most rather than first, because the first time a line goes past is
/// often the time it is buried under the arrangement and half-heard. Shown as
/// it was written down rather than as it was reduced to words, so that a cue
/// reads like a lyric instead of like a search query.
fn wording(heard: &[(Vec<String>, String)]) -> String {
    let mut best: Option<(usize, &(Vec<String>, String))> = None;
    for candidate in heard {
        let times = heard.iter().filter(|(other, _)| *other == candidate.0).count();
        let better = match best {
            None => true,
            Some((most, current)) => {
                times > most || (times == most && candidate.0.len() > current.0.len())
            }
        };
        if better {
            best = Some((times, candidate));
        }
    }
    best.map(|(_, (_, text))| text.clone()).unwrap_or_default()
}

/// A line of transcript with the recogniser's leading space and stray
/// punctuation taken off, ready to be read on a player.
fn tidy(text: &str) -> String {
    text.trim().trim_matches(|c: char| c == '"' || c == '\'').trim().to_string()
}

/// How alike two lines are, from 0 to 1: Dice's measure over their words,
/// counting repeats.
///
/// Word overlap rather than character distance, because what changes between
/// two hearings of a sung line is whole words — an article dropped, a name
/// misheard — and a measure that reads "don't stop" and "dont stop" as
/// different has missed the point. Counting repeats matters for the same
/// reason: "no no no no" and "no" are not the same line.
fn similarity(a: &[String], b: &[String]) -> f32 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let mut spare: Vec<&String> = b.iter().collect();
    let mut shared = 0usize;
    for word in a {
        if let Some(found) = spare.iter().position(|other| *other == word) {
            spare.swap_remove(found);
            shared += 1;
        }
    }
    2.0 * shared as f32 / (a.len() + b.len()) as f32
}

/// The words in a line, lower-cased and stripped of everything that is not one.
///
/// Apostrophes are kept inside a word so that "don't" stays one word, and
/// dropped at the edges so that a quoted line matches an unquoted one.
fn words(text: &str) -> Vec<String> {
    text.split(|c: char| !(c.is_alphanumeric() || c == '\''))
        .map(|word| word.trim_matches('\'').to_lowercase())
        .filter(|word| !word.is_empty())
        .collect()
}

/// A line cut to what a player will show, on a word boundary where it can be.
fn comment(text: &str) -> String {
    if text.chars().count() <= COMMENT_CHARS {
        return text.to_string();
    }
    let cut: String = text.chars().take(COMMENT_CHARS).collect();
    let trimmed = match cut.rsplit_once(' ') {
        Some((head, _)) if head.chars().count() >= COMMENT_CHARS / 2 => head,
        _ => cut.trim_end(),
    };
    format!("{}…", trimmed.trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn said(lines: &[(u32, &str)]) -> Transcript {
        Transcript {
            lines: lines
                .iter()
                .map(|&(start_ms, text)| Line {
                    start_ms,
                    end_ms: start_ms + 2_000,
                    text: text.to_string(),
                })
                .collect(),
        }
    }

    #[test]
    fn the_most_repeated_line_is_the_hook() {
        let transcript = said(&[
            (10_000, "walking through the city at night"),
            (20_000, "and I don't want to go home"),
            (30_000, "hold me closer now"),
            (60_000, "hold me closer now"),
            (90_000, "the rain has not stopped falling"),
            (120_000, "hold me closer now"),
        ]);

        let hook = transcript.hook().expect("a line sung three times is a hook");
        assert_eq!(hook.text, "hold me closer now");
        assert_eq!(hook.times(), 3);
        assert_eq!(hook.at, vec![30_000, 60_000, 120_000]);
    }

    #[test]
    fn a_line_heard_slightly_differently_each_time_is_still_one_line() {
        // What a recogniser actually does to the same sung phrase.
        let transcript = said(&[
            (30_000, "Hold me closer now"),
            (60_000, "hold me, closer now"),
            (90_000, "hold me closer, now!"),
            (120_000, "Oh hold me closer now"),
        ]);

        let hook = transcript.hook().expect("four hearings of one line");
        assert_eq!(hook.times(), 4, "{:?}", transcript.refrains());
    }

    #[test]
    fn two_different_lines_are_not_merged() {
        let transcript = said(&[
            (10_000, "hold me closer now"),
            (40_000, "let me go tonight"),
            (70_000, "hold me closer now"),
        ]);

        let refrains = transcript.refrains();
        assert_eq!(refrains.len(), 2, "{refrains:?}");
        assert_eq!(refrains[0].text, "hold me closer now");
        assert_eq!(refrains[0].times(), 2);
    }

    #[test]
    fn an_ad_lib_is_not_a_hook() {
        // "yeah" on its own is the most repeated thing in half the vocal stems
        // ever recorded and marks nothing.
        let transcript = said(&[
            (10_000, "yeah"),
            (20_000, "yeah!"),
            (30_000, "yeah"),
            (40_000, "yeah"),
            (50_000, "take me all the way down"),
            (80_000, "take me all the way down"),
        ]);

        let hook = transcript.hook().unwrap();
        assert_eq!(hook.text, "take me all the way down");
    }

    #[test]
    fn a_recogniser_stuck_in_a_loop_does_not_invent_a_hook() {
        // Whisper's worst habit: the same line emitted over and over, a second
        // apart, where there is really nothing being sung at all.
        let mut lines: Vec<(u32, &str)> = Vec::new();
        for i in 0..40 {
            lines.push((60_000 + i * 900, "thanks for watching"));
        }
        let transcript = said(&lines);

        let hook = transcript.hook();
        // Forty emissions inside a minute are one airing, so it never reaches
        // the two separate landings a hook needs.
        assert!(hook.is_none(), "{hook:?}");
    }

    #[test]
    fn a_line_sung_twice_inside_a_chorus_is_one_landing() {
        let transcript = said(&[
            (30_000, "hold me closer now"),
            (32_000, "hold me closer now"),
            (90_000, "hold me closer now"),
            (92_000, "hold me closer now"),
        ]);
        let hook = transcript.hook().unwrap();
        assert_eq!(hook.at, vec![30_000, 90_000], "a cue two seconds later is no use");
    }

    #[test]
    fn a_line_said_once_is_not_a_hook() {
        let transcript = said(&[(10_000, "a single spoken sample about nothing")]);
        assert!(transcript.hook().is_none());
        assert_eq!(transcript.refrains().len(), 1, "it is still a line");
    }

    #[test]
    fn an_instrumental_has_nothing_to_say() {
        let transcript = Transcript::default();
        assert!(transcript.is_empty());
        assert!(transcript.hook().is_none());
        assert!(transcript.vocal_in().is_none());
        assert!(transcript.moments().is_empty());
    }

    #[test]
    fn moments_name_the_entry_the_hook_and_its_returns() {
        let transcript = said(&[
            (10_000, "walking through the city at night"),
            (30_000, "hold me closer now"),
            (60_000, "hold me closer now"),
            (120_000, "hold me closer now"),
        ]);

        let moments = transcript.moments();
        let kinds: Vec<MomentKind> = moments.iter().map(|m| m.kind).collect();
        assert_eq!(
            kinds,
            vec![MomentKind::VocalIn, MomentKind::Hook, MomentKind::Refrain, MomentKind::Refrain]
        );
        assert_eq!(moments[0].time_ms, 10_000);
        assert_eq!(moments[1].time_ms, 30_000);
        assert_eq!(moments[1].text, "hold me closer now");
        // And they come out in time order.
        assert!(moments.windows(2).all(|pair| pair[0].time_ms <= pair[1].time_ms));
    }

    #[test]
    fn a_track_that_opens_on_its_hook_gets_one_cue_not_two() {
        let transcript = said(&[(8_000, "hold me closer now"), (68_000, "hold me closer now")]);
        let moments = transcript.moments();
        assert_eq!(moments.len(), 2, "{moments:?}");
        assert_eq!(moments[0].kind, MomentKind::Hook, "the hook outranks the entry");
    }

    #[test]
    fn a_long_line_is_cut_to_something_a_player_can_show() {
        let long = "and we will keep on running until the morning comes around again";
        let transcript = said(&[(10_000, long), (70_000, long)]);
        let hook = transcript.moments().into_iter().find(|m| m.kind == MomentKind::Hook).unwrap();

        assert!(hook.text.chars().count() <= COMMENT_CHARS + 1, "{:?}", hook.text);
        assert!(hook.text.ends_with('…'));
        // Cut between words rather than through one.
        assert!(long.starts_with(hook.text.trim_end_matches('…')), "{:?}", hook.text);
    }

    #[test]
    fn similarity_counts_repeats() {
        let no = words("no no no no");
        let one = words("no");
        assert!(similarity(&no, &one) < SAME, "{}", similarity(&no, &one));
        assert_eq!(similarity(&no, &no), 1.0);
    }

    #[test]
    fn words_are_case_and_punctuation_blind_but_keep_contractions() {
        assert_eq!(words("Don't Stop — the Music!"), vec!["don't", "stop", "the", "music"]);
        assert_eq!(words("'quoted'"), vec!["quoted"]);
        assert_eq!(words("   "), Vec::<String>::new());
    }
}
