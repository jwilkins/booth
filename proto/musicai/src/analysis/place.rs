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
/// and `sung` is where the stem is singing, as `transcribe::align::voiced_spans`
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
