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
    /// Where the line the track repeats most first lands.
    Hook,
    /// Where one of the other lines it keeps coming back to first lands.
    Refrain,
    /// A line coming back round, after the airing that named it.
    ///
    /// Never worth a hot cue — eight buttons spent on one line is the
    /// complaint this whole arrangement exists to answer — and always worth a
    /// memory cue, which costs nothing and is what turns a list of markers
    /// into a map of the record.
    Return,
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

/// How many of the track's repeated lines get a cue of their own.
///
/// One each, and not many. A player has eight hot cues and a record's
/// arrangement wants most of them: the hook, a second line and a tag is already
/// generous, and everything past that is a slot taken from a drop.
const LINES: usize = 3;

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
    /// One *hot* cue per line, not one per airing. A hook sung six times used
    /// to be six hot cues saying the same thing, which is a player that can
    /// jump to one moment of a record — and the drops and the breakdowns, the
    /// things a DJ reaches for between the words, had nowhere left to go. So
    /// each of the track's repeated lines is worth a button once, where it
    /// first lands, and the returns come back as [`MomentKind::Return`], which
    /// takes a memory cue and never a button.
    ///
    /// Nor are they quietly borrowed as names for whatever section they land
    /// on. That was tried, and it puts the same words on three differently
    /// coloured cues, which is the complaint again in another form: a drop
    /// should say "drop", because that is what a DJ is reaching for when they
    /// are not reaching for the words.
    ///
    /// The hook wins any argument with the entry: on a track that opens on its
    /// chorus they are the same moment, and calling it "the voice comes in"
    /// when it is the hook throws away the more useful of the two names.
    pub fn moments(&self) -> Vec<Moment> {
        let mut moments = Vec::new();

        // Most repeated first, so the top line is the hook and the next ones
        // are the lesser ones — which is the order they should lose their
        // slots in when the arrangement wants them.
        let repeated = self.refrains().into_iter().filter(|line| line.times() >= MIN_TIMES);
        for (rank, line) in repeated.take(LINES).enumerate() {
            let text = comment(&line.text);
            let kind = match rank {
                0 => MomentKind::Hook,
                _ => MomentKind::Refrain,
            };
            let Some((&first, again)) = line.at.split_first() else { continue };
            moments.push(Moment { time_ms: first, kind, text: text.clone() });
            // Every time it comes round after that. The words are on the first
            // one, so these carry the same text for the cue set to recognise
            // them by and are marked as returns so they stay off the buttons.
            for &at in again {
                moments.push(Moment { time_ms: at, kind: MomentKind::Return, text: text.clone() });
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

/// Whether two written-out lines are the same sung line.
///
/// The question a cue set asks about its own labels: a hook heard four times is
/// four moments, and they have to be recognised as one line so that the second
/// and third can say "V1" rather than repeating the words.
pub fn same_line(a: &str, b: &str) -> bool {
    similarity(&words(a), &words(b)) >= SAME
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
    fn a_line_gets_one_cue_however_often_it_comes_round() {
        // The complaint this answers: a hook sung six times was six cues
        // saying the same thing, so a player could jump to one moment of the
        // record and the drops had nowhere left to go.
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
            vec![MomentKind::VocalIn, MomentKind::Hook, MomentKind::Return, MomentKind::Return],
            "the line is named once and comes back as returns"
        );
        assert_eq!(moments[0].time_ms, 10_000);
        assert_eq!(moments[1].time_ms, 30_000, "where the line first lands");
        assert_eq!(
            moments.iter().filter(|m| m.kind == MomentKind::Hook).count(),
            1,
            "only the first landing is worth a button"
        );
        assert_eq!(moments[1].text, "hold me closer now");
    }

    #[test]
    fn a_second_line_the_track_keeps_returning_to_gets_a_cue_of_its_own() {
        // One cue each, which is the other half of the rule: the slots saved
        // by not repeating the hook are worth spending on a different line.
        let transcript = said(&[
            (20_000, "hold me closer now"),
            (40_000, "and I don't want to go home"),
            (80_000, "hold me closer now"),
            (100_000, "and I don't want to go home"),
            (140_000, "hold me closer now"),
        ]);

        let moments = transcript.moments();
        let cued: Vec<(MomentKind, &str)> = moments
            .iter()
            .filter(|m| matches!(m.kind, MomentKind::Hook | MomentKind::Refrain))
            .map(|m| (m.kind, m.text.as_str()))
            .collect();
        // In time order, and the one sung three times is the hook.
        assert_eq!(
            cued,
            vec![
                (MomentKind::Hook, "hold me closer now"),
                (MomentKind::Refrain, "and I don't want to go home")
            ]
        );
        // And each comes back, carrying the same words so a cue set can tell
        // which line it is without reading it again.
        assert_eq!(
            moments.iter().filter(|m| m.kind == MomentKind::Return).count(),
            3,
            "{moments:?}"
        );
    }

    #[test]
    fn only_the_handful_of_lines_a_track_leans_on_get_cues() {
        // A player has eight hot cues and the arrangement wants most of them.
        // A very repetitive vocal must not take the lot.
        // Six distinct lines, each sung twice, in the order a recogniser
        // would emit them — which is always ascending, and which the grouping
        // relies on to tell one airing from the next.
        // Six lines with nothing in common, each sung twice, in the order a
        // recogniser emits them — always ascending, which is what the grouping
        // relies on to tell one airing from the next.
        const VERSES: [&str; 6] = [
            "hold me closer now",
            "walking through the city at night",
            "and I don't want to go home",
            "the rain has not stopped falling",
            "take me all the way down",
            "nothing here was ever ours",
        ];
        let mut lines: Vec<(u32, &str)> = Vec::new();
        for round in 0..2u32 {
            for (index, verse) in VERSES.iter().enumerate() {
                lines.push((round * 120_000 + index as u32 * 10_000, verse));
            }
        }
        let transcript = said(&lines);

        let cued = transcript
            .moments()
            .iter()
            .filter(|m| matches!(m.kind, MomentKind::Hook | MomentKind::Refrain))
            .count();
        assert_eq!(cued, LINES, "every repeated line took a slot: {cued}");
    }

    #[test]
    fn a_track_that_opens_on_its_hook_gets_one_cue_not_two() {
        let transcript = said(&[(8_000, "hold me closer now"), (68_000, "hold me closer now")]);
        let moments = transcript.moments();
        // The hook, at the moment the voice also comes in, and no second
        // moment for the entry — calling it "the voice comes in" when it is
        // the hook throws away the more useful of the two names.
        assert_eq!(moments[0].kind, MomentKind::Hook, "the hook outranks the entry");
        assert!(
            !moments.iter().any(|m| m.kind == MomentKind::VocalIn),
            "the entry was cued twice: {moments:?}"
        );
        // The line coming round again is a marker, and only a marker.
        assert_eq!(moments.len(), 2, "{moments:?}");
        assert_eq!(moments[1].kind, MomentKind::Return);
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
