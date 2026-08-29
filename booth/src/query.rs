//! The query bar, which is the browser.
//!
//! Everything the list can be filtered by is expressible as text. That is the
//! whole idea: a filter that is text can be saved, pasted into a message, kept
//! in a file, and diffed — and a saved query is then a smart playlist without a
//! second concept, a separate rule editor, or a ceiling on how many tracks it
//! may match.
//!
//! Parsing produces two things from one pass: the [`Term`]s the browser filters
//! with, and the [`Token`]s the command bar paints. They come from the same
//! parse so that what is highlighted is what is being matched — a bar that
//! colours `bpm:` as a field it did not actually understand is worse than one
//! that does not colour anything.

use std::collections::HashMap;
use std::ops::Range;

use crate::library::{Role, Track};

/// A parsed query: what to match, and how to draw the text that said so.
#[derive(Clone, Debug, Default)]
pub struct Query {
    pub terms: Vec<Term>,
    pub tokens: Vec<Token>,
}

/// One condition. Every term must hold for a track to be listed.
#[derive(Clone, Debug, PartialEq)]
pub struct Term {
    /// Whether the term was written with a leading `-` or `!`.
    pub negated: bool,
    pub test: Test,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Test {
    /// A bare word: matches artist, title or album.
    Text(String),
    Bpm(Compare),
    /// `key:8A`, or `key:~8A` for anything that would mix with it.
    Key {
        camelot: String,
        harmonic: bool,
    },
    Tag(String),
    /// Days since the track was added, e.g. `added:<14d`.
    Added(Compare),
    Played(Played),
    Missing(Missing),
    /// `has:stems` — the other way round from `missing:`, so that the common
    /// question does not have to be asked backwards.
    Has(Missing),
    OnDrive(String),
    InPlaylist(String),
    Format(String),
    Bitrate(Compare),
    Energy(Compare),
    /// `dupes:title+artist` — tracks whose named fields are shared with another.
    Duplicates(Vec<DupeKey>),
    /// Something that did not parse. It matches nothing, so a typo shows up as
    /// an empty list and a red token rather than as a silently wider search.
    Invalid,
}

/// A numeric comparison, in whatever unit the field is measured in.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum Compare {
    Less(f64),
    Greater(f64),
    /// `>=4`, which on a scale of five means four or five and `>4` does not.
    AtLeast(f64),
    /// `<=4`.
    AtMost(f64),
    Between(f64, f64),
    /// Written as a bare number. Ranges rather than equality, because a tempo
    /// of 128 means 128 as printed, not 128.000000.
    About(f64),
}

/// How far either side of a written number still counts as that number.
///
/// Half of the last digit shown. A grid is never exactly 128, so `bpm:128` has
/// to find 128.02 to be worth typing — and a range carries the same tolerance
/// at each end, so that `bpm:124-128` finds the same track that `bpm:128` does.
/// Two forms of the same question must not give different answers.
const TOLERANCE: f64 = 0.5;

impl Compare {
    fn holds(self, value: f64) -> bool {
        match self {
            Compare::Less(limit) => value < limit,
            Compare::Greater(limit) => value > limit,
            // The same tolerance the other forms carry, so that `bpm:>=128`
            // takes a grid that reads 128 and happens to sit at 127.99.
            Compare::AtLeast(limit) => value > limit - TOLERANCE,
            Compare::AtMost(limit) => value < limit + TOLERANCE,
            Compare::Between(low, high) => value > low - TOLERANCE && value < high + TOLERANCE,
            Compare::About(target) => (value - target).abs() < TOLERANCE,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum Played {
    /// Bought and never touched.
    Never,
    /// Played at some point within this many days.
    Within(f64),
    /// Played, but not for this many days — the "what have I been neglecting"
    /// question.
    ///
    /// A track that was never played is not one that was played a long time
    /// ago, so this does not include them; `played:never` is that question and
    /// keeping the two apart is what makes either of them mean anything.
    NotFor(f64),
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Missing {
    Grid,
    Key,
    Stems,
    Tags,
    Cues,
}

impl Missing {
    fn parse(word: &str) -> Option<Self> {
        Some(match word {
            "grid" | "beatgrid" | "beats" => Missing::Grid,
            "key" => Missing::Key,
            "stems" => Missing::Stems,
            "tags" | "tag" => Missing::Tags,
            "cues" | "cue" => Missing::Cues,
            _ => return None,
        })
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum DupeKey {
    Title,
    Artist,
    Duration,
}

impl DupeKey {
    fn parse(word: &str) -> Option<Self> {
        Some(match word {
            "title" => DupeKey::Title,
            "artist" => DupeKey::Artist,
            "length" | "duration" => DupeKey::Duration,
            _ => return None,
        })
    }
}

/// A stretch of the query text, and what it turned out to be. The command bar
/// paints from these.
#[derive(Clone, Debug, PartialEq)]
pub struct Token {
    pub at: Range<usize>,
    pub role: Paint,
}

/// What a stretch of query text is, for the purpose of colouring it.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Paint {
    /// The `bpm:` part of `bpm:124-128`.
    Field,
    /// The `124-128` part.
    Value,
    /// A whole term written with a leading `-`.
    Negated,
    /// A bare word.
    Text,
    /// A term that did not parse.
    Bad,
}

/// What the collection as a whole knows, which a single track does not.
pub struct Context<'a> {
    /// Now, as seconds since the Unix epoch.
    pub now: u64,
    /// Track ids on each drive, by the drive's label.
    pub drives: &'a HashMap<String, Vec<u32>>,
    /// Track ids in each playlist, by name.
    pub playlists: &'a HashMap<String, Vec<u32>>,
    /// Track ids that share a duplicate group, worked out once per query.
    pub duplicates: &'a [u32],
}

impl Query {
    /// Parse a query. Never fails: an unparseable term becomes [`Test::Invalid`],
    /// which matches nothing and is painted red.
    pub fn parse(text: &str) -> Self {
        let mut query = Query::default();
        for (at, word) in words(text) {
            // Either mark, because both are what people reach for: `-` is
            // what search boxes use and `!` is what everything else does.
            let stripped = word.strip_prefix('-').or_else(|| word.strip_prefix('!'));
            let (negated, body_at) = match stripped {
                Some(rest) if !rest.is_empty() => (true, at.start + 1..at.end),
                _ => (false, at.clone()),
            };
            let body = &text[body_at.clone()];

            let (test, spans) = parse_term(body, body_at.start);
            let bad = test == Test::Invalid;
            query.terms.push(Term { negated, test });

            // A negated term is coloured as one thing, so that the eye finds
            // the exclusions — they are the part of a query that is easy to
            // forget is there.
            query.tokens.push(match (bad, negated) {
                (true, _) => Token { at, role: Paint::Bad },
                (false, true) => Token { at, role: Paint::Negated },
                (false, false) => {
                    query.tokens.extend(spans);
                    continue;
                }
            });
        }
        query
    }

    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
    }

    /// Whether any term failed to parse.
    pub fn has_errors(&self) -> bool {
        self.terms.iter().any(|t| t.test == Test::Invalid)
    }

    /// Whether a track should be listed.
    pub fn matches(&self, track: &Track, context: &Context<'_>) -> bool {
        self.terms.iter().all(|term| term.holds(track, context) != term.negated)
    }
}

impl Term {
    fn holds(&self, track: &Track, context: &Context<'_>) -> bool {
        match &self.test {
            Test::Invalid => false,
            Test::Text(needle) => {
                let hit = |field: &str| field.to_lowercase().contains(needle);
                hit(&track.artist) || hit(&track.title) || hit(&track.album)
            }
            Test::Bpm(compare) => track.bpm > 0.0 && compare.holds(track.bpm),
            Test::Key { camelot, harmonic } => match (track.key.as_str(), harmonic) {
                ("", _) => false,
                (key, false) => key.eq_ignore_ascii_case(camelot),
                (key, true) => mixes_with(camelot, key),
            },
            Test::Tag(needle) => track.tags.iter().any(|t| t.eq_ignore_ascii_case(needle)),
            Test::Added(compare) => compare.holds(days_since(track.added, context.now)),
            Test::Played(Played::Never) => track.last_played.is_none(),
            Test::Played(Played::Within(days)) => {
                track.last_played.is_some_and(|at| days_since(at, context.now) <= *days)
            }
            Test::Played(Played::NotFor(days)) => {
                track.last_played.is_some_and(|at| days_since(at, context.now) > *days)
            }
            Test::Missing(what) => lacks(track, *what),
            Test::Has(what) => !lacks(track, *what),
            Test::OnDrive(label) => context
                .drives
                .iter()
                .any(|(name, ids)| matches_name(name, label) && ids.contains(&track.id)),
            Test::InPlaylist(name) => context
                .playlists
                .iter()
                .any(|(playlist, ids)| matches_name(playlist, name) && ids.contains(&track.id)),
            Test::Format(extension) => track.format.eq_ignore_ascii_case(extension),
            Test::Bitrate(compare) => compare.holds(track.bitrate_kbps as f64),
            Test::Energy(compare) => compare.holds(track.energy as f64),
            Test::Duplicates(_) => context.duplicates.contains(&track.id),
        }
    }
}

/// Case-insensitive, and a prefix is enough: drives and playlists are picked
/// from a list the user can see, so `in:drive:SANDISK` should not need the
/// `-64`.
fn matches_name(full: &str, written: &str) -> bool {
    full.to_lowercase().starts_with(&written.to_lowercase())
}

fn days_since(then: u64, now: u64) -> f64 {
    now.saturating_sub(then) as f64 / 86_400.0
}

/// Which tracks share a duplicate group, for `dupes:`.
///
/// Computed over the whole collection once per query rather than per track,
/// because the question "is anything else like this one" is not one a track can
/// answer about itself.
pub fn duplicate_ids(tracks: &[Track], keys: &[DupeKey]) -> Vec<u32> {
    let mut groups: HashMap<String, Vec<u32>> = HashMap::new();
    for track in tracks {
        // Stem companions are duplicates of their parent by construction;
        // listing them would bury the ones worth merging.
        if track.role != Role::Track {
            continue;
        }
        let mut key = String::new();
        for part in keys {
            key.push('\u{1}');
            match part {
                DupeKey::Title => key.push_str(&track.title.to_lowercase()),
                DupeKey::Artist => key.push_str(&track.artist.to_lowercase()),
                // To the second: two rips of the same track rarely differ by
                // less, and two different tracks rarely agree that closely.
                DupeKey::Duration => key.push_str(&format!("{}", track.duration_secs.round())),
            }
        }
        groups.entry(key).or_default().push(track.id);
    }
    groups.into_values().filter(|ids| ids.len() > 1).flatten().collect()
}

/// One line of the help: what to type, and what it does.
pub struct Help {
    pub example: &'static str,
    pub means: &'static str,
}

/// What can be typed in the query bar, as the help shows it.
///
/// Every example here is parsed by a test, so the help cannot come to describe
/// a grammar the parser does not have. A field that is renamed and not
/// documented fails the build's tests rather than quietly lying to whoever
/// reads this.
pub const HELP: &[(&str, &[Help])] = &[
    (
        "Finding a record",
        &[
            Help { example: "peverelist", means: "artist, title or album contains it" },
            Help { example: "\"roll with\"", means: "several words as one" },
            Help { example: "format:flac", means: "by file type — also mp3, wav, m4a" },
            Help { example: "bitrate:<256", means: "under 256 kbps" },
        ],
    ),
    (
        "Mixing",
        &[
            Help { example: "bpm:128", means: "around 128, as the grid reads" },
            Help { example: "bpm:124-128", means: "anywhere in the range" },
            Help { example: "key:8A", means: "that key exactly" },
            Help { example: "key:~8A", means: "that key and everything that mixes with it" },
            Help { example: "energy:>=4", means: "how much is going on, 1 to 5" },
        ],
    ),
    (
        "Preparation",
        &[
            Help { example: "has:stems", means: "a rendered kit — also grid, key, cues, tags" },
            Help { example: "missing:stems", means: "the other way round" },
            Help { example: "no:grid", means: "the same as missing:" },
            Help { example: "!missing:cues", means: "not missing them, i.e. has them" },
            Help { example: "-has:key", means: "`-` and `!` both mean not" },
        ],
    ),
    (
        "Where it lives",
        &[
            Help { example: "in:\"Sat 14/9\"", means: "in that playlist" },
            Help { example: "in:drive:SANDISK", means: "written to that drive" },
            Help { example: "tag:peak", means: "carries that tag" },
            Help { example: "dupes:title+artist", means: "shares both with another track" },
        ],
    ),
    (
        "History",
        &[
            Help { example: "added:<14d", means: "added in the last fortnight" },
            Help { example: "played:never", means: "never played" },
            Help { example: "played:>30d", means: "not for a month" },
        ],
    ),
];

/// Whether a track is without the thing named.
///
/// One function for both `missing:` and `has:`, so the pair cannot come to
/// disagree about what counts as having stems.
fn lacks(track: &Track, what: Missing) -> bool {
    match what {
        Missing::Grid => !track.has_grid,
        Missing::Key => track.key.is_empty(),
        // A stem is not missing its own stems; only a whole track can be
        // waiting for a kit.
        Missing::Stems => track.role == Role::Track && track.stems.is_empty(),
        Missing::Tags => track.tags.is_empty(),
        Missing::Cues => track.cues.is_empty(),
    }
}

/// Whether two Camelot keys would mix.
///
/// The wheel's own rule: the same key, its neighbours either side, and its
/// relative major or minor. Nothing here is a matter of taste — it is what the
/// numbering was invented to express — so it is worth having as one function
/// rather than as a mental step at the decks.
pub fn mixes_with(a: &str, b: &str) -> bool {
    let Some((number_a, letter_a)) = camelot_parts(a) else { return false };
    let Some((number_b, letter_b)) = camelot_parts(b) else { return false };

    if letter_a == letter_b {
        // 12 wraps to 1: the wheel is a circle.
        let apart = (number_a as i32 - number_b as i32).rem_euclid(12);
        return apart <= 1 || apart >= 11;
    }
    number_a == number_b
}

fn camelot_parts(key: &str) -> Option<(u8, char)> {
    let key = key.trim();
    let letter = key.chars().last()?.to_ascii_uppercase();
    if letter != 'A' && letter != 'B' {
        return None;
    }
    let number: u8 = key[..key.len() - 1].parse().ok()?;
    (1..=12).contains(&number).then_some((number, letter))
}

/// Split on whitespace, keeping where each word was.
fn words(text: &str) -> Vec<(Range<usize>, &str)> {
    let mut out = Vec::new();
    let mut start = None;
    for (at, ch) in text.char_indices() {
        match (ch.is_whitespace(), start) {
            (false, None) => start = Some(at),
            (true, Some(from)) => {
                out.push((from..at, &text[from..at]));
                start = None;
            }
            _ => {}
        }
    }
    if let Some(from) = start {
        out.push((from..text.len(), &text[from..]));
    }
    out
}

/// One term, and the spans that say how to paint it.
fn parse_term(body: &str, offset: usize) -> (Test, Vec<Token>) {
    let Some(colon) = body.find(':') else {
        let test = Test::Text(body.to_lowercase());
        return (test, vec![Token { at: offset..offset + body.len(), role: Paint::Text }]);
    };

    let field = &body[..colon];
    let value = &body[colon + 1..];
    let test = parse_field(&field.to_lowercase(), value);
    if test == Test::Invalid {
        return (test, Vec::new());
    }

    // The colon belongs with the field: it is what makes it a field.
    let spans = vec![
        Token { at: offset..offset + colon + 1, role: Paint::Field },
        Token { at: offset + colon + 1..offset + body.len(), role: Paint::Value },
    ];
    (test, spans)
}

fn parse_field(field: &str, value: &str) -> Test {
    if value.is_empty() {
        return Test::Invalid;
    }
    match field {
        "bpm" | "tempo" => compare(value).map(Test::Bpm).unwrap_or(Test::Invalid),
        "key" => {
            let harmonic = value.starts_with('~');
            let camelot = value.trim_start_matches('~').to_ascii_uppercase();
            match camelot_parts(&camelot) {
                Some(_) => Test::Key { camelot, harmonic },
                None => Test::Invalid,
            }
        }
        "tag" => Test::Tag(value.to_string()),
        "added" => compare(value).map(Test::Added).unwrap_or(Test::Invalid),
        "played" => match value {
            "never" => Test::Played(Played::Never),
            // `>30d` is "not for a month" and `<30d` is "within a month"; a
            // bare `30d` is the second, because that is what it reads as.
            other => {
                let (make, rest): (fn(f64) -> Played, &str) = match other {
                    _ if other.starts_with('>') => (Played::NotFor, &other[1..]),
                    _ if other.starts_with('<') => (Played::Within, &other[1..]),
                    _ => (Played::Within, other),
                };
                match duration_days(rest) {
                    Some(days) => Test::Played(make(days)),
                    None => Test::Invalid,
                }
            }
        },
        "missing" | "no" => Missing::parse(value).map(Test::Missing).unwrap_or(Test::Invalid),
        "has" | "with" => Missing::parse(value).map(Test::Has).unwrap_or(Test::Invalid),
        "in" => match value.split_once(':') {
            Some(("drive", name)) if !name.is_empty() => Test::OnDrive(name.to_string()),
            Some(("playlist", name)) if !name.is_empty() => Test::InPlaylist(name.to_string()),
            // A bare `in:name` means a playlist, which is what it usually is.
            None => Test::InPlaylist(value.to_string()),
            _ => Test::Invalid,
        },
        "format" | "type" => Test::Format(value.trim_start_matches('.').to_string()),
        "bitrate" => compare(value).map(Test::Bitrate).unwrap_or(Test::Invalid),
        "energy" => compare(value).map(Test::Energy).unwrap_or(Test::Invalid),
        "dupes" | "duplicates" => {
            let keys: Option<Vec<DupeKey>> = value.split('+').map(DupeKey::parse).collect();
            match keys {
                Some(keys) if !keys.is_empty() => Test::Duplicates(keys),
                _ => Test::Invalid,
            }
        }
        _ => Test::Invalid,
    }
}

/// `<14d`, `>128`, `124-128` or `128`, in whatever the field's own unit is.
fn compare(value: &str) -> Option<Compare> {
    // The two-character forms first: `>=` starts with `>`, so testing the
    // shorter one first would read `>=4` as "greater than nothing".
    if let Some(rest) = value.strip_prefix(">=") {
        return Some(Compare::AtLeast(number(rest)?));
    }
    if let Some(rest) = value.strip_prefix("<=") {
        return Some(Compare::AtMost(number(rest)?));
    }
    if let Some(rest) = value.strip_prefix('<') {
        return Some(Compare::Less(number(rest)?));
    }
    if let Some(rest) = value.strip_prefix('>') {
        return Some(Compare::Greater(number(rest)?));
    }
    // Split on a hyphen that is not a leading minus sign, so a range reads as a
    // range and a negative number still reads as a number.
    if let Some(at) = value[1..].find('-').map(|i| i + 1) {
        let (low, high) = (number(&value[..at])?, number(&value[at + 1..])?);
        return Some(Compare::Between(low.min(high), low.max(high)));
    }
    Some(Compare::About(number(value)?))
}

/// A number, with a trailing unit letter allowed: `14d` is 14.
fn number(text: &str) -> Option<f64> {
    let digits = text.trim_end_matches(|c: char| c.is_ascii_alphabetic());
    let parsed: f64 = digits.parse().ok()?;
    parsed.is_finite().then_some(parsed)
}

/// A span of time in days, written `30d`, `6w` or `1y`.
fn duration_days(text: &str) -> Option<f64> {
    let amount = number(text)?;
    let unit = text.chars().last()?;
    Some(match unit {
        'd' | 'D' => amount,
        'w' | 'W' => amount * 7.0,
        'm' | 'M' => amount * 30.0,
        'y' | 'Y' => amount * 365.0,
        // A bare number of days, for `played:30`.
        c if c.is_ascii_digit() => amount,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {

    #[test]
    fn every_example_in_the_help_actually_parses() {
        // The help is the only description of the grammar there is, so it has
        // to be checked against the grammar rather than trusted. A field
        // renamed without the help following fails here.
        for (heading, lines) in HELP {
            for help in *lines {
                let query = Query::parse(help.example);
                assert!(!query.terms.is_empty(), "{heading}: {:?} parsed to nothing", help.example);
                assert!(
                    query.terms.iter().all(|t| t.test != Test::Invalid),
                    "{heading}: {:?} does not parse — {:?}",
                    help.example,
                    query.terms
                );
            }
        }
    }

    #[test]
    fn a_bang_negates_as_well_as_a_dash() {
        // Both, because both are what people reach for.
        for text in ["-missing:stems", "!missing:stems"] {
            let query = Query::parse(text);
            assert_eq!(query.terms.len(), 1, "{text}");
            assert!(query.terms[0].negated, "{text} was not read as a negation");
            assert_eq!(query.terms[0].test, Test::Missing(Missing::Stems), "{text}");
        }

        // A lone mark is not a negation of nothing; it is a search for it.
        assert!(!Query::parse("!").terms[0].negated);
        assert!(!Query::parse("-").terms[0].negated);
    }

    #[test]
    fn has_is_missing_the_other_way_round() {
        let bare = track(1);
        let with = {
            let mut track = track(2);
            track.stems.vocals = Some("/s/a-vocals.mp3".into());
            track.stems.drums = Some("/s/a-drums.mp3".into());
            track.stems.melody = Some("/s/a-melody.mp3".into());
            track
        };

        assert!(matches("has:stems", &with));
        assert!(!matches("has:stems", &bare));
        assert!(matches("missing:stems", &bare));
        assert!(!matches("missing:stems", &with));

        // The two ways of asking the same question must agree.
        for track in [&bare, &with] {
            assert_eq!(
                matches("has:stems", track),
                matches("!missing:stems", track),
                "has: and !missing: disagreed about #{}",
                track.id
            );
        }
    }

    #[test]
    fn at_least_and_at_most_include_the_number_written() {
        // On a scale of five, `>=4` means four or five and `>4` means five.
        // Both were documented; only the strict pair was implemented, and
        // `energy:>=4` quietly matched nothing at all.
        let mut four = track(1);
        four.energy = 4;
        let mut five = track(2);
        five.energy = 5;
        let mut three = track(3);
        three.energy = 3;

        assert!(matches("energy:>=4", &four), "the number written was excluded");
        assert!(matches("energy:>=4", &five));
        assert!(!matches("energy:>=4", &three));

        assert!(!matches("energy:>4", &four), "the strict form still excludes it");
        assert!(matches("energy:<=4", &four));
        assert!(!matches("energy:<=4", &five));
    }

    #[test]
    fn played_reads_both_directions_of_a_span() {
        let day = 24.0 * 60.0 * 60.0;
        let mut recent = track(1);
        recent.last_played = Some(NOW - (5.0 * day) as u64);
        let mut ages_ago = track(2);
        ages_ago.last_played = Some(NOW - (90.0 * day) as u64);
        let never = track(3);

        assert!(matches("played:>30d", &ages_ago), "not for a month");
        assert!(!matches("played:>30d", &recent));
        assert!(matches("played:30d", &recent), "a bare span is within it");
        assert!(matches("played:<30d", &recent));
        assert!(!matches("played:<30d", &ages_ago));

        // Never played is its own question, and answering it here would make
        // both terms mean less.
        assert!(!matches("played:>30d", &never), "never played is not played long ago");
        assert!(matches("played:never", &never));
    }

    #[test]
    fn no_is_another_word_for_missing() {
        assert_eq!(Query::parse("no:grid").terms[0].test, Test::Missing(Missing::Grid));
        assert_eq!(Query::parse("with:cues").terms[0].test, Test::Has(Missing::Cues));
    }
    use super::*;
    use crate::library::StemKit;

    const DAY: u64 = 86_400;
    const NOW: u64 = 1_700_000_000;

    fn track(id: u32) -> Track {
        Track {
            id,
            artist: "Peverelist".into(),
            title: "Roll With The Punches".into(),
            album: "Livity Sound".into(),
            bpm: 128.02,
            key: "8A".into(),
            energy: 4,
            format: "flac".into(),
            bitrate_kbps: 940,
            added: NOW - 3 * DAY,
            has_grid: true,
            ..Track::placeholder(id)
        }
    }

    fn empty_context() -> (HashMap<String, Vec<u32>>, HashMap<String, Vec<u32>>) {
        (HashMap::new(), HashMap::new())
    }

    fn matches(query: &str, track: &Track) -> bool {
        let (drives, playlists) = empty_context();
        let context = Context { now: NOW, drives: &drives, playlists: &playlists, duplicates: &[] };
        Query::parse(query).matches(track, &context)
    }

    #[test]
    fn an_empty_query_lists_everything() {
        let query = Query::parse("   ");
        assert!(query.is_empty());
        assert!(matches("", &track(1)));
    }

    #[test]
    fn every_term_has_to_hold() {
        let track = track(1);
        assert!(matches("bpm:124-128 key:8A", &track));
        assert!(!matches("bpm:124-128 key:4A", &track), "one failing term should exclude it");
    }

    #[test]
    fn a_bare_word_searches_the_names() {
        let track = track(1);
        assert!(matches("punches", &track), "the title, case-insensitively");
        assert!(matches("peverelist", &track), "the artist");
        assert!(matches("livity", &track), "the album");
        assert!(!matches("batu", &track));
    }

    #[test]
    fn a_leading_minus_excludes() {
        let track = track(1);
        assert!(!matches("-key:8A", &track));
        assert!(matches("-key:4A", &track));
    }

    #[test]
    fn a_tempo_range_includes_its_ends() {
        let mut track = track(1);
        track.bpm = 124.0;
        assert!(matches("bpm:124-128", &track));
        track.bpm = 128.0;
        assert!(matches("bpm:124-128", &track));
        track.bpm = 129.0;
        assert!(!matches("bpm:124-128", &track));
        track.bpm = 123.0;
        assert!(!matches("bpm:124-128", &track));
    }

    #[test]
    fn a_range_and_a_bare_number_agree_about_the_same_track() {
        // A grid is never exactly 128. If `bpm:128` finds this track, so must
        // any range that ends at 128 — otherwise the two forms of the same
        // question give different answers.
        let mut track = track(1);
        track.bpm = 128.02;
        assert!(matches("bpm:128", &track));
        assert!(matches("bpm:124-128", &track));
        assert!(matches("bpm:128-132", &track));

        track.bpm = 127.96;
        assert!(matches("bpm:128", &track));
        assert!(matches("bpm:128-132", &track));
    }

    #[test]
    fn a_bare_tempo_means_the_number_as_printed() {
        // A grid is never exactly 128, and a DJ asking for 128 means this one.
        let track = track(1);
        assert_eq!(track.bpm, 128.02);
        assert!(matches("bpm:128", &track));
        assert!(!matches("bpm:129", &track));
    }

    #[test]
    fn an_ungridded_track_has_no_tempo_to_match() {
        let mut track = track(1);
        track.bpm = 0.0;
        track.has_grid = false;
        assert!(!matches("bpm:>0", &track), "zero is the absence of a tempo, not a slow one");
        assert!(matches("missing:grid", &track));
    }

    #[test]
    fn the_harmonic_key_search_is_the_camelot_wheel() {
        // 8A mixes with 7A, 9A and 8B, and with nothing else.
        for compatible in ["8A", "7A", "9A", "8B"] {
            assert!(mixes_with("8A", compatible), "8A should mix with {compatible}");
        }
        for incompatible in ["6A", "10A", "7B", "9B", "4A"] {
            assert!(!mixes_with("8A", incompatible), "8A should not mix with {incompatible}");
        }
    }

    #[test]
    fn the_wheel_wraps_around() {
        assert!(mixes_with("12A", "1A"), "12 and 1 are neighbours");
        assert!(mixes_with("1A", "12A"));
        assert!(!mixes_with("12A", "2A"));
    }

    #[test]
    fn a_plain_key_search_is_exact() {
        let mut track = track(1);
        track.key = "9A".into();
        assert!(!matches("key:8A", &track));
        assert!(matches("key:~8A", &track), "the tilde is what widens it");
    }

    #[test]
    fn ages_are_counted_in_days() {
        let mut track = track(1);
        track.added = NOW - 3 * DAY;
        assert!(matches("added:<14d", &track));
        assert!(!matches("added:>14d", &track));

        track.added = NOW - 40 * DAY;
        assert!(!matches("added:<14d", &track));
        assert!(matches("added:>14d", &track));
    }

    #[test]
    fn never_played_is_not_the_same_as_played_long_ago() {
        let mut track = track(1);
        track.last_played = None;
        assert!(matches("played:never", &track));
        assert!(!matches("played:30d", &track));

        track.last_played = Some(NOW - 90 * DAY);
        assert!(!matches("played:never", &track));
        assert!(!matches("played:30d", &track));
        // The spec's own example: not played in the last month.
        assert!(matches("-played:30d", &track));

        track.last_played = Some(NOW - 10 * DAY);
        assert!(matches("played:30d", &track));
        assert!(!matches("-played:30d", &track));
    }

    #[test]
    fn weeks_and_years_are_days_too() {
        assert_eq!(duration_days("2w"), Some(14.0));
        assert_eq!(duration_days("1y"), Some(365.0));
        assert_eq!(duration_days("30"), Some(30.0));
        assert_eq!(duration_days("soon"), None);
    }

    #[test]
    fn a_stem_is_not_missing_its_own_stems() {
        let mut parent = track(1);
        parent.stems = StemKit::default();
        assert!(matches("missing:stems", &parent));

        let mut stem = track(2);
        stem.role = Role::Vocals;
        stem.parent = Some(1);
        assert!(!matches("missing:stems", &stem), "a stem would always match, and drown the list");
    }

    #[test]
    fn drives_and_playlists_come_from_the_collection() {
        let track = track(7);
        let drives = HashMap::from([("SANDISK-64".to_string(), vec![7, 9])]);
        let playlists = HashMap::from([("peak".to_string(), vec![9])]);
        let context = Context { now: NOW, drives: &drives, playlists: &playlists, duplicates: &[] };

        assert!(Query::parse("in:drive:SANDISK-64").matches(&track, &context));
        assert!(Query::parse("in:drive:sandisk").matches(&track, &context), "a prefix is enough");
        assert!(!Query::parse("in:drive:OTHER").matches(&track, &context));
        assert!(!Query::parse("in:playlist:peak").matches(&track, &context));
        // A bare `in:` names a playlist, so it does not find a drive.
        assert!(!Query::parse("in:SANDISK-64").matches(&track, &context));
    }

    #[test]
    fn duplicates_are_groups_rather_than_a_property_of_one_track() {
        let mut a = track(1);
        let mut b = track(2);
        let mut c = track(3);
        c.title = "Marius".into();
        c.artist = "Batu".into();
        // Same title and artist, different files.
        a.duration_secs = 300.0;
        b.duration_secs = 301.0;

        let dupes = duplicate_ids(&[a, b, c], &[DupeKey::Title, DupeKey::Artist]);
        assert_eq!(dupes.len(), 2, "the pair, and not the odd one out: {dupes:?}");
        assert!(dupes.contains(&1) && dupes.contains(&2));
    }

    #[test]
    fn duplicates_can_be_told_apart_by_length() {
        let mut a = track(1);
        let mut b = track(2);
        a.duration_secs = 300.0;
        b.duration_secs = 420.0;
        // Same names, but one is an extended mix — which is not a duplicate.
        let dupes = duplicate_ids(&[a, b], &[DupeKey::Title, DupeKey::Artist, DupeKey::Duration]);
        assert!(dupes.is_empty(), "{dupes:?}");
    }

    #[test]
    fn files_that_will_not_survive_the_booth_are_findable() {
        let mut track = track(1);
        track.format = "mp3".into();
        track.bitrate_kbps = 192;
        assert!(matches("format:mp3 bitrate:<256", &track));
        assert!(!matches("format:flac bitrate:<256", &track));
    }

    #[test]
    fn a_term_that_does_not_parse_matches_nothing() {
        let track = track(1);
        // Rather than being ignored, which would quietly widen the search to
        // everything and look like it had worked.
        assert!(!matches("nonsense:12", &track));
        assert!(!matches("bpm:fast", &track));
        assert!(!matches("key:99Z", &track));
        assert!(Query::parse("nonsense:12").has_errors());
        assert!(!Query::parse("bpm:128").has_errors());
    }

    #[test]
    fn the_bar_is_painted_from_the_same_parse_that_filters() {
        let text = "bpm:124-128 key:~8A -played:30d peverelist wat:1";
        let query = Query::parse(text);

        let roles: Vec<Paint> = query.tokens.iter().map(|t| t.role).collect();
        assert_eq!(
            roles,
            vec![
                Paint::Field,   // bpm:
                Paint::Value,   // 124-128
                Paint::Field,   // key:
                Paint::Value,   // ~8A
                Paint::Negated, // -played:30d
                Paint::Text,    // peverelist
                Paint::Bad,     // wat:1
            ]
        );
        // Every token points at the text it came from.
        assert_eq!(&text[query.tokens[0].at.clone()], "bpm:");
        assert_eq!(&text[query.tokens[1].at.clone()], "124-128");
        assert_eq!(&text[query.tokens[4].at.clone()], "-played:30d");
        assert_eq!(&text[query.tokens[6].at.clone()], "wat:1");
    }

    #[test]
    fn a_lone_minus_is_a_word_rather_than_a_negation() {
        let query = Query::parse("-");
        assert_eq!(query.terms.len(), 1);
        assert!(!query.terms[0].negated);
    }
}
