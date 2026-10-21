//! Where a cue off the words goes, and which airing of a line gets it.
//!
//! A line's start is a time the recogniser gave and the stem corrected, and a
//! cue is a place a DJ presses. Between the two is the grid. A hot cue is
//! pressed live, in time with another deck, so it wants to be on the one; and
//! it must never fall inside a word, because a cue that clips its own first
//! syllable is one nobody presses twice. On a line that starts a beat and a
//! half after the bar line those two pull against each other, and this is the
//! order they are settled in — best first:
//!
//! 1. **On the one.** The bar line before the line, where the line starts
//!    within half a beat of it and nothing is sung between the two. Sung lines
//!    land late more often than early, and a line half a beat after the bar
//!    line is still that bar's line to anyone dancing.
//! 2. **The bar line before.** The same bar line, with a longer clear run-in:
//!    a line that starts on beat three after two beats of band. Pressed on
//!    the one of another deck, the words arrive in phrase, two beats later.
//! 3. **The beat before.** The beat at or before the line, where it does not
//!    fall inside the line before. What every word cue was until now.
//! 4. **The line itself.** Where even that beat is inside the previous line,
//!    and the stem measured singing starting where the line does, the one
//!    place that clips nothing is the line's own start, off the grid.
//! 5. **The beat before, anyway.** Where the line's time is itself inside
//!    singing — a line sung straight on from the one before, in a stretch the
//!    stem cannot split into lines — that time is an estimate: a lyric file's,
//!    or the recogniser's. Trusted to the millisecond, an estimate a word or two
//!    late cues the line a couple of words in, and the line's opening is the
//!    thing being cued. So the cue goes on the beat before, as every word cue
//!    once did: a beat early plays the tail of the line before and then the
//!    whole line, where a couple of words late loses the line's beginning.
//!
//! The line's start that the ladder works from is not taken on trust where it
//! can be found. A line's time is a lyric file's or the recogniser's, and
//! inside a verse the stem cannot split into stretches, an estimate: a lyric
//! timed against another pressing was seen a word or two late, line after
//! line, and every cue on it came in mid-line. So the ladder starts from
//! [`line_start`] — where the singing nearest that time starts after a
//! breath — and only where there is no breath to find does a line keep the
//! time it was given, and land on the fifth rung.
//!
//! Among the airings of a line the track comes back to, the button goes to the
//! airing that reaches the best rung; between airings on the same rung, to the
//! one on a phrase boundary, eight bars from the start of its section or four;
//! and between those, to the earliest. The hook's first airing used to get the
//! button whatever its placing, and a chorus that starts on a pickup the first
//! time round often starts clean the fourth.
//!
//! The half-beat and the two frames below were reasoned from one record's
//! account, not measured across a collection. The account under the cues
//! measures every airing of every line — see `booth`'s "What the words
//! decided" — and is where to set them from.

/// Beats in a bar. The grid starts on a downbeat, so the bar lines are every
/// fourth beat from its first.
pub const BEATS_PER_BAR: usize = 4;

/// How far after the one a line may start and still be cued on it, in beats.
///
/// Half a beat. A recogniser trims the breath before a word and singers lean
/// back on a phrase, so a line's measured start sits a little after the beat
/// it belongs to; half a beat is well clear of that and well short of the
/// next beat, which a line starting on would belong to.
pub const ON_THE_ONE_BEATS: f32 = 0.5;

/// How far into a sung stretch a cue can sit and still be at its onset rather
/// than inside its first word: two of the detector's twenty-millisecond frames.
pub const ONSET_SLACK_MS: u32 = 40;

/// The shortest silence that counts as the breath before a line: 150 ms.
///
/// Longer than the gap between two words of a line — a consonant, a few tens
/// of milliseconds, which the finer measurement mostly bridges anyway — and
/// shorter than the breath a singer takes between lines. Reasoned rather than
/// measured on a collection; the account under the cues says, for each line it
/// cues, where the stem had it starting and where its lyric said, which is
/// where to set it from.
pub const MIN_BREATH_MS: u32 = 150;

/// How far before the time a line was given its start is looked for: a second
/// and a half, a couple of words and then some. Never further than halfway to
/// the line before, whose start the breaths nearer it belong to.
pub const REACH_BACK_MS: u32 = 1_500;

/// And how far after: three quarters of a second, or halfway to the line
/// after. Less than back, because a time given early lands in the silence
/// before the line rather than inside the one before it.
pub const REACH_ON_MS: u32 = 750;

/// Bars in a phrase, as a DJ counts them. Half of it is the lesser boundary.
const PHRASE_BARS: usize = 8;

/// Which rung of the ladder a cue reached, best first.
///
/// Last is the one where nobody knows where the line begins, which is also
/// what puts an airing whose start was measured ahead of one whose start was
/// estimated when [`best_airing`] chooses between them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Rung {
    OnTheOne,
    BarBefore,
    BeatBefore,
    Onset,
    InSinging,
}

impl Rung {
    /// How it reads in the account of what the words decided.
    pub fn said(self) -> &'static str {
        match self {
            Rung::OnTheOne => "on the one, with a clear way in",
            Rung::BarBefore => "on the bar line before, with a clear way in",
            Rung::BeatBefore => "on the beat before",
            Rung::Onset => "on the line itself, there being no clear beat before it",
            Rung::InSinging => {
                "on the beat before, the line being sung straight on from the one before it"
            }
        }
    }
}

/// Where a cue went, and how.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placed {
    pub time_ms: u32,
    pub rung: Rung,
}

/// Where a cue for a line starting at `onset_ms` goes.
///
/// `grid` is every beat of the track from its first downbeat, in milliseconds,
/// and `sung` is where the stem is singing, as `transcribe::align::sounds`
/// measures it. With no grid, or a line before the grid's first beat, the cue
/// is the line itself.
pub fn place(onset_ms: u32, grid: &[u32], sung: &[(u32, u32)]) -> Placed {
    let Some(beat) = grid.partition_point(|at| *at <= onset_ms).checked_sub(1) else {
        return Placed { time_ms: onset_ms, rung: Rung::Onset };
    };
    let one = beat - beat % BEATS_PER_BAR;
    if !sung_between(sung, grid[one], onset_ms) {
        let late = (onset_ms - grid[one]) as f32 / period_at(grid, beat);
        let rung = match late <= ON_THE_ONE_BEATS {
            true => Rung::OnTheOne,
            false => Rung::BarBefore,
        };
        return Placed { time_ms: grid[one], rung };
    }
    if !inside_a_word(sung, grid[beat]) {
        return Placed { time_ms: grid[beat], rung: Rung::BeatBefore };
    }
    // The beat before is inside singing too. Only where singing starts at the
    // line's time is that singing the line before, and the line's time the
    // place to cue; a time inside the singing is an estimate, and the cue goes
    // early rather than late on it — see the ladder's fifth rung.
    if inside_a_word(sung, onset_ms) {
        return Placed { time_ms: grid[beat], rung: Rung::InSinging };
    }
    Placed { time_ms: onset_ms, rung: Rung::Onset }
}

/// Where a line's singing starts, near the time it was given.
///
/// The sound in `sounds` that follows the longest silence within reach of
/// `at_ms`, where that silence is a breath — [`MIN_BREATH_MS`] or more. Within
/// [`REACH_BACK_MS`] before and [`REACH_ON_MS`] after, and never past halfway
/// to the lines either side, `before_ms` and `after_ms`, whose starts the
/// breaths nearer them are. The longest rather than the nearest, because a
/// singer breathes between lines and pauses inside them, and the deeper of two
/// silences is the line break more often than not; between two the same
/// length, the nearer.
///
/// `None` where no breath is in reach: a verse delivered without one, or a
/// stem whose reverb never lets the voice drop below the floor. The line keeps
/// the time it was given, which [`place`] then treats as the estimate it is.
pub fn line_start(
    at_ms: u32,
    before_ms: Option<u32>,
    after_ms: Option<u32>,
    sounds: &[(u32, u32)],
) -> Option<u32> {
    let back =
        before_ms.map_or(REACH_BACK_MS, |b| (at_ms.saturating_sub(b) / 2).min(REACH_BACK_MS));
    let on = after_ms.map_or(REACH_ON_MS, |a| (a.saturating_sub(at_ms) / 2).min(REACH_ON_MS));
    let (from, to) = (at_ms.saturating_sub(back), at_ms.saturating_add(on));
    sounds
        .iter()
        .enumerate()
        .filter(|(_, (start, _))| *start >= from && *start <= to)
        .map(|(i, &(start, _))| {
            let quiet_since = i.checked_sub(1).map_or(0, |before| sounds[before].1);
            (start.saturating_sub(quiet_since), start)
        })
        .filter(|(silence, _)| *silence >= MIN_BREATH_MS)
        .max_by_key(|(silence, start)| (*silence, std::cmp::Reverse(start.abs_diff(at_ms))))
        .map(|(_, start)| start)
}

/// Which airing of a line the button goes to, and where.
///
/// The airing that reaches the best rung; between airings on the same rung,
/// the one on a phrase boundary — see [`off_the_phrase`] — and between those,
/// the earliest, which is what the button was before this existed. `sections`
/// are where the arrangement's sections start, on the grid. Returns the index
/// into `airings` with the placing; `None` for no airings.
pub fn best_airing(
    airings: &[u32],
    grid: &[u32],
    sung: &[(u32, u32)],
    sections: &[u32],
) -> Option<(usize, Placed)> {
    airings
        .iter()
        .enumerate()
        .map(|(i, &at)| (i, place(at, grid, sung)))
        .min_by_key(|(i, placed)| (placed.rung, off_the_phrase(placed.time_ms, grid, sections), *i))
}

/// How far a moment is from a phrase boundary: 0 on one, 1 on a half-phrase,
/// 2 anywhere else.
///
/// Phrases are counted in bars from the start of the section the moment is
/// in, because that is where a DJ starts counting — a drop that lands on bar
/// seventeen of the record is bar one of the drop. Without sections, from the
/// grid's first bar.
fn off_the_phrase(at_ms: u32, grid: &[u32], sections: &[u32]) -> u8 {
    let Some(bar) = bar_index(grid, at_ms) else { return 2 };
    let from = sections
        .iter()
        .filter(|&&start| start <= at_ms)
        .max()
        .and_then(|&start| bar_index(grid, start))
        .unwrap_or(0);
    let bars = bar.saturating_sub(from);
    match (bars % PHRASE_BARS, bars % (PHRASE_BARS / 2)) {
        (0, _) => 0,
        (_, 0) => 1,
        _ => 2,
    }
}

/// Which bar of the grid a moment falls in, counting the first as zero.
fn bar_index(grid: &[u32], at_ms: u32) -> Option<usize> {
    let beat = grid.partition_point(|at| *at <= at_ms).checked_sub(1)?;
    Some(beat / BEATS_PER_BAR)
}

/// The length of the beat at `i`, from the grid itself: the interval to the
/// next beat, or from the one before where it is the last the grid names.
fn period_at(grid: &[u32], i: usize) -> f32 {
    let next = grid.get(i + 1).map(|&next| next.saturating_sub(grid[i]));
    let before = i.checked_sub(1).map(|before| grid[i].saturating_sub(grid[before]));
    next.or(before).unwrap_or(1).max(1) as f32
}

/// How far into its bar a moment falls, in beats after the bar line: 0.0 is on
/// the one, 3.8 is a pickup a fifth of a beat before the next one.
///
/// `None` off the grid — before its first beat, or where there is no grid to
/// speak of. Past the last beat the grid names, the last interval carries on.
pub fn in_the_bar(grid: &[u32], at_ms: u32) -> Option<f32> {
    let i = grid.partition_point(|beat| *beat <= at_ms).checked_sub(1)?;
    if grid.len() < 2 {
        return None;
    }
    let fraction = ((at_ms - grid[i]) as f32 / period_at(grid, i)).min(0.99);
    Some((i % BEATS_PER_BAR) as f32 + fraction)
}

/// The bar line at or before a moment, where the grid reaches it.
pub fn bar_line_before(grid: &[u32], at_ms: u32) -> Option<u32> {
    let i = grid.partition_point(|beat| *beat <= at_ms).checked_sub(1)?;
    grid.get(i - i % BEATS_PER_BAR).copied()
}

/// Whether anything is sung between two moments.
pub fn sung_between(sung: &[(u32, u32)], from_ms: u32, to_ms: u32) -> bool {
    sung.iter().any(|&(from, to)| from < to_ms && to > from_ms)
}

/// Whether a moment falls inside a word rather than at the start of one: in a
/// sung stretch, and further into it than [`ONSET_SLACK_MS`].
pub fn inside_a_word(sung: &[(u32, u32)], at_ms: u32) -> bool {
    sung.iter().any(|&(from, to)| at_ms > from + ONSET_SLACK_MS && at_ms < to)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 120 BPM from zero: a beat is 500 ms and the one is every two seconds.
    fn grid() -> Vec<u32> {
        (0..400).map(|i| i * 500).collect()
    }

    #[test]
    fn a_line_just_after_the_one_with_a_clear_way_in_is_cued_on_the_one() {
        let placed = place(40_100, &grid(), &[(40_100, 42_000)]);
        assert_eq!(placed, Placed { time_ms: 40_000, rung: Rung::OnTheOne });
    }

    #[test]
    fn a_line_well_into_its_bar_with_a_clear_way_in_is_cued_on_the_bar_line_before() {
        // Two and a half beats of band before the words, so pressed on the
        // one they arrive in phrase.
        let placed = place(41_250, &grid(), &[(41_250, 43_000)]);
        assert_eq!(placed, Placed { time_ms: 40_000, rung: Rung::BarBefore });
    }

    #[test]
    fn a_line_with_the_line_before_still_going_at_the_one_is_cued_on_the_beat_before() {
        // The previous line runs over the bar line and stops on beat two; the
        // beat before the words is clear of it.
        let sung = [(38_000, 40_400), (41_200, 43_000)];
        let placed = place(41_200, &grid(), &sung);
        assert_eq!(placed, Placed { time_ms: 41_000, rung: Rung::BeatBefore });
    }

    #[test]
    fn a_line_the_stem_measured_starting_right_after_another_is_cued_on_itself() {
        // The stem says singing starts where the line does, and the beat before
        // is inside the line before: the line's own start clips neither.
        let sung = [(38_600, 40_600), (40_600, 42_600)];
        let placed = place(40_600, &grid(), &sung);
        assert_eq!(placed, Placed { time_ms: 40_600, rung: Rung::Onset });
    }

    #[test]
    fn a_line_whose_time_falls_inside_continuous_singing_is_cued_on_the_beat_before_it() {
        // One stretch of singing from 38 s to 44 s holds this line and the one
        // before it, so 41.2 s is a lyric file's or the recogniser's estimate
        // of where the line starts, not a measurement. Cued at the estimate, a
        // line timed a word or two late starts a couple of words in; the beat
        // before keeps its beginning.
        let placed = place(41_200, &grid(), &[(38_000, 44_000)]);
        assert_eq!(placed, Placed { time_ms: 41_000, rung: Rung::InSinging });
    }

    #[test]
    fn an_airing_whose_start_was_measured_wins_over_one_whose_start_was_estimated() {
        // The first airing is inside a stretch with the line before it; the
        // second starts its own stretch. Both have singing on the beat before,
        // but only the second's beginning is known.
        let sung = [(38_000, 44_000), (98_600, 100_600), (100_600, 102_600)];
        let (which, placed) = best_airing(&[41_200, 100_600], &grid(), &sung, &[]).unwrap();
        assert_eq!((which, placed.rung), (1, Rung::Onset));
    }

    #[test]
    fn a_pickup_with_a_clear_bar_before_it_is_cued_on_that_bar_line() {
        // A fifth of a beat before the next one: cueing on that one would clip
        // it, and the bar before is clear.
        let placed = place(41_900, &grid(), &[(41_900, 44_000)]);
        assert_eq!(placed, Placed { time_ms: 40_000, rung: Rung::BarBefore });
    }

    #[test]
    fn without_a_grid_the_cue_is_the_line_itself() {
        assert_eq!(place(40_100, &[], &[]), Placed { time_ms: 40_100, rung: Rung::Onset });
        // And before the grid's first beat.
        assert_eq!(place(500, &grid()[4..], &[]), Placed { time_ms: 500, rung: Rung::Onset });
    }

    #[test]
    fn the_button_goes_to_the_airing_that_places_best_and_otherwise_to_the_first() {
        // First airing straight after another line, second clean on the one.
        let sung = [(38_600, 40_600), (40_600, 42_600), (100_100, 102_000)];
        let (which, placed) = best_airing(&[40_600, 100_100], &grid(), &sung, &[]).unwrap();
        assert_eq!((which, placed.time_ms, placed.rung), (1, 100_000, Rung::OnTheOne));

        // Both clean on the one: the first.
        let sung = [(40_100, 42_000), (100_100, 102_000)];
        let (which, _) = best_airing(&[40_100, 100_100], &grid(), &sung, &[]).unwrap();
        assert_eq!(which, 0);
        assert!(best_airing(&[], &grid(), &sung, &[]).is_none());
    }

    #[test]
    fn between_airings_placed_alike_the_one_on_a_phrase_boundary_wins() {
        // Both on the one with a clear way in. The drop starts at bar 16
        // (32 s); the second airing is eight bars into it and the first is
        // four, so the second is the phrase and wins.
        let sung = [(40_000, 42_000), (48_000, 50_000)];
        let (which, _) = best_airing(&[40_000, 48_000], &grid(), &sung, &[0, 32_000]).unwrap();
        assert_eq!(which, 1);
        // Counted from the grid's first bar when nothing says where the
        // sections are: bar 20 is a half-phrase and bar 24 a phrase.
        let (which, _) = best_airing(&[40_000, 48_000], &grid(), &sung, &[]).unwrap();
        assert_eq!(which, 1);
        assert_eq!(off_the_phrase(40_000, &grid(), &[]), 1);
        assert_eq!(off_the_phrase(42_000, &grid(), &[]), 2);
    }

    #[test]
    fn a_line_given_a_late_time_starts_after_the_breath_before_it() {
        // The line before stops at 40 s, a quarter-second breath, and this one
        // starts at 40.25 s. Its lyric says 40.65 s: a word or two late.
        let sounds = [(38_000, 40_000), (40_250, 43_000)];
        assert_eq!(line_start(40_650, None, None, &sounds), Some(40_250));
        // And given early, in the breath itself, it is the same place.
        assert_eq!(line_start(40_100, None, None, &sounds), Some(40_250));
    }

    #[test]
    fn a_gap_between_two_words_is_not_a_breath_and_is_not_taken_for_a_line_start() {
        // Eighty milliseconds is a consonant, not a breath. Nothing in reach,
        // so the line keeps the time it was given.
        let sounds = [(38_000, 40_000), (40_080, 43_000)];
        assert_eq!(line_start(40_400, None, None, &sounds), None);
    }

    #[test]
    fn a_breath_nearer_the_line_before_belongs_to_the_line_before() {
        // The line before starts at 38.8 s after a half-second breath; this one
        // at 39.8 s after a short one, and its lyric says 40.2 s. Looked for
        // without a bound, the deeper breath wins and this line is put on the
        // line before's start. Halfway to the line before is as far back as
        // this line's start is looked for.
        let sounds = [(36_000, 38_300), (38_800, 39_600), (39_800, 43_000)];
        assert_eq!(line_start(40_200, Some(38_800), None, &sounds), Some(39_800));
        assert_eq!(
            line_start(40_200, None, None, &sounds),
            Some(38_800),
            "with nothing to bound it"
        );
    }

    #[test]
    fn of_two_breaths_in_reach_the_deeper_is_the_line_break() {
        // A pause inside the line, at 41.2 s, and the breath before it, at
        // 40.25 s, which is longer.
        let sounds = [(38_000, 39_900), (40_250, 41_050), (41_220, 43_000)];
        assert_eq!(line_start(41_400, None, None, &sounds), Some(40_250));
    }

    #[test]
    fn a_line_cued_from_where_its_singing_starts_is_cued_before_it_and_not_in_it() {
        // The whole path: a lyric half a second late inside a verse, a breath
        // before the line, and a cue on the beat before the breath's end
        // rather than on the beat before the lyric's time — which was inside
        // the line.
        let sounds = [(36_000, 40_400), (40_700, 43_400)];
        let begins = line_start(41_200, Some(36_000), None, &sounds).unwrap();
        assert_eq!(begins, 40_700);
        assert_eq!(
            place(begins, &grid(), &sounds),
            Placed { time_ms: 40_500, rung: Rung::BeatBefore }
        );
        assert_eq!(place(41_200, &grid(), &sounds).time_ms, 41_000, "inside the line it is for");
    }

    #[test]
    fn a_moment_is_placed_in_its_bar_in_beats_after_the_one() {
        let grid = grid();
        let close = |a: Option<f32>, b: f32| a.is_some_and(|a| (a - b).abs() < 0.01);
        assert!(close(in_the_bar(&grid, 2_000), 0.0));
        assert!(close(in_the_bar(&grid, 2_250), 0.5));
        assert!(close(in_the_bar(&grid, 3_900), 3.8));
        // Past the last beat the grid names, the last interval carries on.
        assert!(close(in_the_bar(&grid, 199_900), 3.8));
        assert_eq!(in_the_bar(&[], 1_000), None);
        assert_eq!(in_the_bar(&grid[2..], 500), None);
        assert_eq!(bar_line_before(&grid, 3_900), Some(2_000));
    }

    #[test]
    fn a_cue_is_inside_a_word_when_it_falls_in_singing_and_not_at_its_onset() {
        let sung = [(1_000, 2_000)];
        assert!(!inside_a_word(&sung, 900));
        assert!(!inside_a_word(&sung, 1_000));
        assert!(!inside_a_word(&sung, 1_040));
        assert!(inside_a_word(&sung, 1_500));
        assert!(!inside_a_word(&sung, 2_000));
        assert!(sung_between(&sung, 500, 1_200));
        assert!(!sung_between(&sung, 500, 1_000));
        assert!(!sung_between(&sung, 2_000, 2_500));
    }
}
