//! Correcting a beat grid by hand.
//!
//! # Why this exists
//!
//! A beat tracker is right about most records and wrong about a few, and the
//! few are not random. It reads a half-time hip-hop record at double, a
//! drum-and-bass record at half, and it puts the one on the snare of anything
//! with a strong backbeat. None of that is fixable by analysing again — the
//! same audio gives the same answer — and all of it is obvious to somebody
//! looking at the waveform with the track playing.
//!
//! A player has had these controls for twenty years for the same reason, and a
//! DJ who has used them on a CDJ knows what they do: the tempo is doubled or
//! halved, the grid is slid until the lines land on the kicks, and the one is
//! put where the one is.
//!
//! # What a grid is here
//!
//! Two numbers for almost every record — a tempo and where the first downbeat
//! falls — because a grid at one steady tempo is exactly that, and keeping
//! thousands of beat times for it would be keeping the same fact over and
//! over. A track whose grid genuinely bends keeps every beat instead, in
//! [`Track::beat_ms`], and these operations handle both: what a tempo cannot
//! describe, a list of times can.
//!
//! Every one of these returns whether it changed anything, so the window can
//! tell a correction from a no-op without comparing grids itself.

use crate::library::{Track, BEATS_PER_BAR};

/// The smallest nudge worth offering, in milliseconds.
///
/// Four. Below about this a grid line moves less than the width it is drawn
/// at, so the picture does not change and neither does what a player does with
/// it; above it, two presses are a noticeable shift. It is also roughly the
/// error a human ear can hear on a doubled kick, which is the thing being
/// corrected.
pub const NUDGE_MS: i32 = 4;

/// How far a nudge goes when it is asked for in a hurry.
///
/// Ten nudges. A grid that is out by a whole fortieth of a second is out by
/// more than a tracker's rounding — it has locked onto the wrong transient —
/// and stepping there four milliseconds at a time is ten presses of watching
/// nothing much happen.
pub const SHOVE_MS: i32 = NUDGE_MS * 10;

/// Double the tempo: a record read at half speed.
///
/// The commonest thing a tracker gets wrong on a record with space in it. A
/// half-time beat has kicks where the tracker expects them and nothing in
/// between, so it calls 140 BPM 70 and every bar line lands two beats apart.
///
/// Returns false where there is nothing to double — no grid, or a tempo so
/// high that doubling it is plainly not what anybody meant.
pub fn double(track: &mut Track) -> bool {
    if !track.has_grid || track.bpm <= 0.0 || track.bpm * 2.0 > MOST_BPM {
        return false;
    }
    track.bpm *= 2.0;
    // A grid that bends is a list of times, and doubling it means a beat
    // halfway between each pair. The last beat has no pair to sit between, so
    // the list grows by one short of twice its length.
    if !track.beat_ms.is_empty() {
        let was = std::mem::take(&mut track.beat_ms);
        let mut now = Vec::with_capacity(was.len() * 2);
        for pair in was.windows(2) {
            now.push(pair[0]);
            now.push(pair[0] + (pair[1] - pair[0]) / 2);
        }
        if let Some(last) = was.last() {
            now.push(*last);
        }
        track.beat_ms = now;
    }
    track.beats = count(track);
    true
}

/// Halve the tempo: a record read at double speed.
///
/// The other half of the same mistake. A tracker counting the hi-hats of a
/// drum-and-bass record calls 174 BPM 348, and every fourth line is where the
/// bar actually is.
pub fn halve(track: &mut Track) -> bool {
    if !track.has_grid || track.bpm / 2.0 < LEAST_BPM {
        return false;
    }
    track.bpm /= 2.0;
    // Every other beat, keeping the first so the downbeat stays a downbeat.
    if !track.beat_ms.is_empty() {
        track.beat_ms = track.beat_ms.iter().step_by(2).copied().collect();
    }
    track.beats = count(track);
    true
}

/// The fastest and slowest a record is allowed to be called.
///
/// Not a judgement about music: these are the rails that stop a doubling from
/// running away. Four presses of double on a 128 BPM record is 2048, which is
/// a grid of lines a millisecond apart, a picture that takes a second to draw
/// and a number nobody typed on purpose.
const MOST_BPM: f64 = 400.0;
const LEAST_BPM: f64 = 20.0;

/// Slide the whole grid, keeping its tempo and its phase.
///
/// What the nudge buttons do. Positive is later, which is what a grid drawn
/// ahead of the kicks needs.
pub fn nudge(track: &mut Track, by_ms: i32) -> bool {
    if !track.has_grid || by_ms == 0 {
        return false;
    }
    match track.beat_ms.is_empty() {
        true => {
            // An even grid is its downbeat, so moving the grid is moving that.
            // Where it has never been set, it is wherever the first beat is,
            // and the shift has to start from there rather than from zero.
            let from = first_beat(track) as i64 + by_ms as i64;
            // A downbeat repeats every bar, so one shifted before the start of
            // the track is the same grid as the one a bar later — and that is
            // the one that can be written down. Without this a grid anchored
            // at zero could not be nudged earlier at all: the subtraction
            // saturated, the anchor stayed on zero, and the press did nothing.
            // Which is how a fit that has to slide the grid back a little
            // silently did not.
            let bar = 60_000.0 / track.bpm.max(f64::EPSILON) * BEATS_PER_BAR as f64;
            let mut at = from as f64;
            if bar > 0.0 {
                while at < 0.0 {
                    at += bar;
                }
            }
            track.downbeat_ms = Some(at.max(0.0).round() as u32);
        }
        false => {
            // A beat shifted before the start of the track is a beat that is
            // not in the track any more. Dropped rather than clamped, because
            // clamping stacks them all onto zero and that is a grid claiming
            // several beats in the same millisecond.
            for beat in &mut track.beat_ms {
                *beat = beat.saturating_add_signed(by_ms);
            }
            if by_ms < 0 {
                let first_kept =
                    track.beat_ms.iter().position(|beat| *beat > 0).unwrap_or(track.beat_ms.len());
                track.beat_ms.drain(..first_kept.saturating_sub(1).min(track.beat_ms.len()));
            }
        }
    }
    track.beats = count(track);
    true
}

/// Put a beat exactly on `at_ms`, moving the grid to meet it.
///
/// The coarse version of a nudge, and the one a DJ reaches for first: park the
/// playhead on a kick that the grid is plainly missing, and the grid comes to
/// it. The phase is kept — whichever beat of the bar was nearest stays that
/// beat of the bar — because moving the grid and moving the one are different
/// corrections and doing both at once is how a fixed grid comes out a beat off.
pub fn move_to(track: &mut Track, at_ms: u32) -> bool {
    let Some(nearest) = nearest_beat(track, at_ms) else { return false };
    let by = at_ms as i64 - nearest as i64;
    nudge(track, by.clamp(i32::MIN as i64, i32::MAX as i64) as i32)
}

/// Say that the beat nearest `at_ms` is a one.
///
/// The other correction, and the one a tracker gets wrong on anything with a
/// strong backbeat: the beats are right and the bar lines are on the two. This
/// moves the phase by up to three beats and does not move a single beat, so a
/// grid that was landing on the kicks still lands on them afterwards.
pub fn set_downbeat(track: &mut Track, at_ms: u32) -> bool {
    let Some(nearest) = nearest_beat(track, at_ms) else { return false };
    match track.beat_ms.is_empty() {
        true => {
            if track.downbeat_ms == Some(nearest) {
                return false;
            }
            track.downbeat_ms = Some(nearest);
        }
        false => {
            // A bent grid is kept from its first downbeat on, so saying "the
            // one is here" is dropping the beats before it.
            let Some(at) = track.beat_ms.iter().position(|beat| *beat == nearest) else {
                return false;
            };
            if at == 0 {
                return false;
            }
            track.beat_ms.drain(..at);
        }
    }
    track.beats = count(track);
    true
}

/// Set the tempo to exactly `bpm`, keeping the first beat where it is.
///
/// What the typed field and the two tenth-of-a-BPM buttons do. A tempo a DJ
/// has from the record sleeve, from another program, or from counting it
/// themselves beats one this program measured, and a tenth of a BPM is the
/// unit a long mix drifts by — ten minutes at 128.1 against 128.0 is five
/// beats apart by the end.
///
/// On a grid that bends, this stretches the whole shape about its first beat
/// rather than straightening it: the bends are a measurement of how the record
/// actually moves, and throwing them away to honour a round number would lose
/// the thing the list of times is kept for. So the ratio between the beats is
/// held and the span between them scales.
pub fn set_bpm(track: &mut Track, bpm: f64) -> bool {
    if !track.has_grid || !bpm.is_finite() || !(LEAST_BPM..=MOST_BPM).contains(&bpm) {
        return false;
    }
    if track.bpm <= 0.0 || (track.bpm - bpm).abs() < f64::EPSILON {
        return false;
    }
    let factor = track.bpm / bpm;
    track.bpm = bpm;
    if !track.beat_ms.is_empty() {
        let first = track.beat_ms[0] as f64;
        for beat in &mut track.beat_ms {
            *beat = (first + (*beat as f64 - first) * factor).round().max(0.0) as u32;
        }
    }
    track.beats = count(track);
    true
}

/// Whether this track's tempo moves.
///
/// The question a DJ asks before trusting a loop to stay in time across eight
/// bars, and the one the collection answers by what it chose to keep: a grid a
/// tempo and a downbeat could describe is not kept beat by beat, so a track
/// holding its own beat times is one whose tempo a single number could not
/// describe. The window rebuilds the even case from those two numbers when it
/// draws, which is the other half of the same bargain.
pub fn is_dynamic(track: &Track) -> bool {
    !track.beat_ms.is_empty()
}

/// The most beats a DJ is taken to have skipped between two marks.
///
/// Two marks a gap apart do not say how many beats are in the gap: a mark on
/// every kick of a 128 BPM record and a mark on every bar of a 32 BPM one put
/// the marks in the same places. So the gap is tried as one beat, two, and so
/// on, and the reading whose tempo lands nearest what the track already says
/// wins. Sixteen is four bars, which is as far apart as anybody marks two
/// points they expect a grid to be fitted through.
const MOST_BEATS_BETWEEN_MARKS: u32 = 16;

/// How far a mark may sit from the grid fitted through it before that grid is
/// called a bad explanation of the marks, in milliseconds.
///
/// Twelve. A kick is audibly early or late against the beat before it at about
/// twenty, and a DJ clicking a waveform is not placing marks to the
/// millisecond, so this has to be loose enough to forgive the pointing and
/// tight enough to notice a record that genuinely moves. It decides one thing:
/// whether the marks can be explained by a single tempo, and so whether the
/// answer is a plain grid or one that bends.
const MARK_SLACK_MS: f64 = 12.0;

/// Fit a grid to the places somebody has marked a beat.
///
/// A plain grid if one explains the marks, and one that bends only if none
/// does — because a tempo and a downbeat are the better answer wherever they
/// are true. They survive a re-analysis, they are what the picture is rebuilt
/// from, and eight beats from anywhere in such a track are eight beats from
/// anywhere else, which is what stops two loops drifting apart on two players.
/// A list of times says all of that only by accident.
///
/// One mark cannot say a tempo, so it slides the grid onto that mark and keeps
/// the tempo there is. Two or more fit a tempo as well as a phase.
///
/// The first mark becomes the downbeat. Somebody marking a record to fix its
/// grid is marking the one; where they were not, [`set_downbeat`] moves the
/// phase afterwards without moving a beat.
pub fn fit(track: &mut Track, marks: &[u32]) -> bool {
    if !track.has_grid || marks.is_empty() {
        return false;
    }
    let mut marks: Vec<u32> = marks.to_vec();
    marks.sort_unstable();
    marks.dedup();

    if marks.len() == 1 {
        return move_to(track, marks[0]);
    }

    match even_period(&marks, track.bpm) {
        Some(period) => {
            let bpm = 60_000.0 / period;
            if !(LEAST_BPM..=MOST_BPM).contains(&bpm) {
                return false;
            }
            track.bpm = bpm;
            track.downbeat_ms = Some(marks[0]);
            // The whole point of preferring a plain grid: a track that was
            // being kept beat by beat stops being, because a tempo and a
            // downbeat now say it.
            track.beat_ms.clear();
            track.beats = count(track);
            true
        }
        // No single tempo passes through the marks, so the marks are the grid:
        // beats spaced evenly between each neighbouring pair, which is the
        // most that can be said about the stretches nobody marked.
        None => {
            track.beat_ms = bent_through(&marks);
            if track.beat_ms.len() < 2 {
                return false;
            }
            track.bpm = nominal_bpm(&track.beat_ms);
            track.beats = count(track);
            true
        }
    }
}

/// The beat period that explains every mark, or `None` where none does.
///
/// Each mark is some whole number of beats from the first. The gap between the
/// first two says what that number might be — it is one beat, or two, or any
/// count up to [`MOST_BEATS_BETWEEN_MARKS`] — and each guess gives a period to
/// try. The guess closest to the tempo the track already carries is tried
/// first, so a record already read at about the right speed is not re-read at
/// half of it on a tie.
fn even_period(marks: &[u32], now_bpm: f64) -> Option<f64> {
    let first = marks[0] as f64;
    let gap = marks[1] as f64 - first;
    if gap <= 0.0 {
        return None;
    }
    let now_period = match now_bpm > 0.0 {
        true => 60_000.0 / now_bpm,
        false => gap,
    };

    let mut guesses: Vec<f64> =
        (1..=MOST_BEATS_BETWEEN_MARKS).map(|beats| gap / beats as f64).collect();
    // Nearest the tempo already there first, so the tie between a reading and
    // half of it goes to the one nobody has to correct again.
    guesses.sort_by(|a, b| {
        (a - now_period)
            .abs()
            .partial_cmp(&(b - now_period).abs())
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    for guess in guesses {
        if guess <= 0.0 {
            continue;
        }
        let Some(period) = settle(marks, guess) else { continue };
        let worst = marks
            .iter()
            .map(|mark| {
                let from_first = *mark as f64 - first;
                let beats = (from_first / period).round();
                (from_first - beats * period).abs()
            })
            .fold(0.0f64, f64::max);
        if worst <= MARK_SLACK_MS {
            return Some(period);
        }
    }
    None
}

/// The period that best fits the marks, given a guess at how many beats apart
/// they are.
///
/// Least squares through the origin: each mark is assigned the whole number of
/// beats the guess puts it at, and the period is then the one that minimises
/// how far the marks sit from their beats. A guess only has to be close enough
/// to assign the right counts; the arithmetic does the rest.
fn settle(marks: &[u32], guess: f64) -> Option<f64> {
    let first = marks[0] as f64;
    let mut top = 0.0f64;
    let mut bottom = 0.0f64;
    for mark in marks {
        let from_first = *mark as f64 - first;
        let beats = (from_first / guess).round();
        if beats <= 0.0 {
            continue;
        }
        top += beats * from_first;
        bottom += beats * beats;
    }
    match bottom > 0.0 {
        true => Some(top / bottom),
        false => None,
    }
}

/// A grid that passes through every mark, with beats spread evenly between
/// each neighbouring pair.
///
/// For the records a single tempo cannot describe. How many beats go in a gap
/// is read from the gap itself against the tightest pair of marks, because
/// that pair is the one most likely to be two beats a DJ meant as neighbours.
fn bent_through(marks: &[u32]) -> Vec<u32> {
    let tightest =
        marks.windows(2).map(|pair| pair[1] - pair[0]).filter(|gap| *gap > 0).min().unwrap_or(0)
            as f64;
    if tightest <= 0.0 {
        return Vec::new();
    }
    let mut beats = Vec::new();
    for pair in marks.windows(2) {
        let (from, to) = (pair[0] as f64, pair[1] as f64);
        let steps = ((to - from) / tightest).round().max(1.0);
        for step in 0..steps as usize {
            beats.push((from + (to - from) * step as f64 / steps).round() as u32);
        }
    }
    if let Some(last) = marks.last() {
        beats.push(*last);
    }
    beats
}

/// The one tempo that best describes a list of beat times.
fn nominal_bpm(beats: &[u32]) -> f64 {
    match beats.len() >= 2 {
        true => {
            let span = *beats.last().unwrap() as f64 - beats[0] as f64;
            match span > 0.0 {
                true => 60_000.0 * (beats.len() - 1) as f64 / span,
                false => 0.0,
            }
        }
        false => 0.0,
    }
}

/// Turn a grid that is kept beat by beat into a tempo and a downbeat.
///
/// For a track whose beats were tracked untidily rather than because the
/// record moves: the list is thousands of numbers saying what two would say,
/// it cannot be rebuilt if it is lost, and a loop taken from one end of such a
/// track does not match one taken from the other. A plain grid fixes all
/// three.
///
/// The tempo is the one that best fits the beats that were kept, so the result
/// agrees with them at both ends rather than drifting away from one.
///
/// Returns false where there is nothing to convert.
pub fn flatten(track: &mut Track) -> bool {
    if !track.has_grid || track.beat_ms.len() < 2 {
        return false;
    }
    let bpm = nominal_bpm(&track.beat_ms);
    if !(LEAST_BPM..=MOST_BPM).contains(&bpm) {
        return false;
    }
    track.downbeat_ms = Some(track.beat_ms[0]);
    track.bpm = bpm;
    track.beat_ms.clear();
    track.beats = count(track);
    true
}

/// Where the grid's first beat falls, however the grid is kept./// Where the grid's first beat falls, however the grid is kept.
fn first_beat(track: &Track) -> u32 {
    track
        .beat_ms
        .first()
        .copied()
        .or(track.downbeat_ms)
        .or_else(|| track.cues.first().map(|cue| cue.time_ms))
        .unwrap_or(0)
}

/// The beat time closest to `at_ms`, or `None` where there is no grid.
fn nearest_beat(track: &Track, at_ms: u32) -> Option<u32> {
    if !track.has_grid || track.bpm <= 0.0 {
        return None;
    }
    if !track.beat_ms.is_empty() {
        return track.beat_ms.iter().copied().min_by_key(|beat| beat.abs_diff(at_ms));
    }
    // An even grid has a beat every period from its first one, and the nearest
    // is found rather than searched for.
    let period = 60_000.0 / track.bpm;
    let first = first_beat(track) as f64;
    let steps = ((at_ms as f64 - first) / period).round();
    Some((first + steps * period).max(0.0).round() as u32)
}

/// How many beats the grid now has across the track.
fn count(track: &Track) -> usize {
    if !track.beat_ms.is_empty() {
        return track.beat_ms.len();
    }
    if track.bpm <= 0.0 || track.duration_secs <= 0.0 {
        return 0;
    }
    let period = 60_000.0 / track.bpm;
    let first = first_beat(track) as f64 % (period * BEATS_PER_BAR as f64);
    ((track.duration_secs * 1000.0 - first) / period).floor().max(0.0) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A four-minute record at 128, gridded from the top.
    fn gridded() -> Track {
        let mut track = Track::placeholder(1);
        track.duration_secs = 240.0;
        track.bpm = 128.0;
        track.has_grid = true;
        track.downbeat_ms = Some(0);
        track.beats = count(&track);
        track
    }

    #[test]
    fn a_half_time_record_is_doubled_back_to_its_own_tempo() {
        // The commonest thing a tracker gets wrong: kicks where it expects
        // them and nothing between, so 140 reads as 70.
        let mut track = gridded();
        track.bpm = 70.0;
        assert!(double(&mut track));
        assert_eq!(track.bpm, 140.0);
    }

    #[test]
    fn a_record_counted_on_the_hats_is_halved() {
        let mut track = gridded();
        track.bpm = 348.0;
        assert!(halve(&mut track));
        assert_eq!(track.bpm, 174.0);
    }

    #[test]
    fn doubling_runs_out_before_the_number_stops_meaning_anything() {
        // Four presses on a club record is two thousand BPM and a grid of
        // lines a millisecond apart. The button stops rather than drawing it.
        let mut track = gridded();
        let mut doubled = 0;
        while double(&mut track) {
            doubled += 1;
            assert!(doubled < 10, "it never stopped: {} BPM", track.bpm);
        }
        assert!(track.bpm <= MOST_BPM, "{} BPM", track.bpm);
    }

    #[test]
    fn halving_runs_out_too() {
        let mut track = gridded();
        while halve(&mut track) {}
        assert!(track.bpm >= LEAST_BPM, "{} BPM", track.bpm);
    }

    #[test]
    fn a_nudge_moves_the_grid_and_leaves_the_tempo_alone() {
        let mut track = gridded();
        assert!(nudge(&mut track, NUDGE_MS));
        assert_eq!(track.downbeat_ms, Some(NUDGE_MS as u32));
        assert_eq!(track.bpm, 128.0, "a nudge is not a tempo change");
    }

    #[test]
    fn a_nudge_before_the_start_of_the_file_writes_the_same_grid_a_bar_later() {
        // Nothing happens before zero, and a grid that wrapped round to the
        // end of a u32 would be a track with no beats at all. This used to
        // clamp the anchor to zero for that reason, which kept both of those
        // true and made the press do nothing at all — the grid did not move.
        //
        // A downbeat repeats every bar, so the grid that was asked for can be
        // written down a bar later instead. Same beats, same bar lines, and
        // the nudge takes effect.
        let mut track = gridded();
        let bar = BEATS_PER_BAR as f64 * 60_000.0 / 128.0;
        assert!(nudge(&mut track, -1_000));
        assert_eq!(track.downbeat_ms, Some((bar - 1_000.0).round() as u32));

        // The same grid, not merely a legal one: every beat sits where asking
        // for a second earlier would have put it.
        let period = 60_000.0 / 128.0;
        let landed = nearest_beat(&track, 30_000).unwrap() as f64;
        let asked = -1_000.0 + (31_000.0f64 / period).round() * period;
        assert!((landed - asked).abs() < 1.0, "landed on {landed}, asked for {asked}");
    }

    #[test]
    fn the_grid_comes_to_the_playhead_without_changing_phase() {
        // A beat every 468.75 ms. Parked 20 ms after the fourth beat, the grid
        // moves 20 ms rather than jumping a beat.
        let mut track = gridded();
        let fourth = (3.0f64 * 60_000.0 / 128.0).round() as u32;
        assert!(move_to(&mut track, fourth + 20));

        let moved = nearest_beat(&track, fourth + 20).unwrap();
        assert_eq!(moved, fourth + 20, "the grid did not come to the playhead");
        assert_eq!(track.bpm, 128.0);
    }

    #[test]
    fn saying_the_one_is_here_moves_no_beat() {
        // The backbeat case: the beats are right and the bar lines are on the
        // two. Every beat has to stay exactly where it is.
        let mut track = gridded();
        let before: Vec<u32> = (0..16).map(|i| beat(&track, i)).collect();

        let second = beat(&track, 1);
        assert!(set_downbeat(&mut track, second));
        assert_eq!(track.downbeat_ms, Some(second));

        // Within a millisecond, which is the whole of the error this can have:
        // a downbeat is kept as whole milliseconds, so making a beat the
        // origin rounds it, and every beat after it is measured from the
        // rounded value. Half a millisecond is a fortieth of the width a grid
        // line is drawn at and well under what a player resolves.
        let after: Vec<u32> = (0..16).map(|i| beat(&track, i)).collect();
        for (was, now) in before[1..].iter().zip(&after) {
            assert!(was.abs_diff(*now) <= 1, "a beat moved from {was} to {now}");
        }
    }

    /// The nth beat of an even grid, in milliseconds.
    fn beat(track: &Track, n: usize) -> u32 {
        let first = first_beat(track) as f64;
        (first + n as f64 * 60_000.0 / track.bpm).round() as u32
    }

    #[test]
    fn a_typed_tempo_is_taken_exactly() {
        // The case it is for: a DJ with the tempo from the sleeve, or from
        // another program, or counted by hand, against one this measured.
        let mut track = gridded();
        assert!(set_bpm(&mut track, 127.33));
        assert_eq!(track.bpm, 127.33);
    }

    #[test]
    fn a_tempo_outside_the_rails_or_unchanged_is_refused() {
        // The same rails doubling and halving run into, for the same reason:
        // a grid of lines a millisecond apart is not what anybody typed on
        // purpose. And an unchanged tempo is not an edit — the window reads
        // the answer to tell a correction from a press that did nothing.
        let mut track = gridded();
        assert!(!set_bpm(&mut track, 0.0));
        assert!(!set_bpm(&mut track, f64::NAN));
        assert!(!set_bpm(&mut track, MOST_BPM + 1.0));
        assert!(!set_bpm(&mut track, LEAST_BPM - 1.0));
        assert!(!set_bpm(&mut track, 128.0), "that is the tempo it already had");
        assert_eq!(track.bpm, 128.0, "a refused tempo left the grid alone");
    }

    #[test]
    fn a_tenth_up_and_a_tenth_back_is_where_it_started() {
        // Ten minutes at 128.1 against 128.0 ends five beats apart, so a tenth
        // is the unit worth a button — and a button worth pressing is one that
        // can be unpressed.
        let mut track = gridded();
        assert!(set_bpm(&mut track, 128.1));
        assert!(set_bpm(&mut track, 128.0));
        assert_eq!(track.bpm, 128.0);
    }

    #[test]
    fn a_tempo_set_on_a_bent_grid_stretches_it_rather_than_straightening_it() {
        // The bends are a measurement of how the record actually moves.
        // Honouring a round number by throwing them away would lose the thing
        // the list of times is kept for, so the shape is held and the span
        // between the beats scales.
        let mut track = bent();
        let was = track.beat_ms.clone();
        let gaps = |beats: &[u32]| -> Vec<i64> {
            beats.windows(2).map(|pair| pair[1] as i64 - pair[0] as i64).collect()
        };
        let before = gaps(&was);

        assert!(set_bpm(&mut track, 120.0 * 2.0));
        assert_eq!(track.beat_ms[0], was[0], "the first beat is the one it turns about");
        assert_eq!(track.beat_ms.len(), was.len(), "no beat was added or dropped");

        // Twice the tempo, so every gap is half what it was — and still
        // shrinking beat by beat the way it was measured to.
        let after = gaps(&track.beat_ms);
        for (was, now) in before.iter().zip(&after) {
            assert!((was / 2 - now).abs() <= 1, "a gap of {was} became {now}");
        }
        assert!(after.windows(2).all(|pair| pair[1] <= pair[0]), "the shape was straightened");
    }

    #[test]
    fn a_track_that_keeps_its_own_beats_is_the_dynamic_one() {
        // How the collection already records the answer: a grid a tempo and a
        // downbeat could describe is not kept beat by beat.
        assert!(!is_dynamic(&gridded()));
        assert!(is_dynamic(&bent()));
    }

    /// Marks on the beat of an even grid at `bpm`, starting at `from`, every
    /// `every` beats, `count` of them — with `off_by` milliseconds of human
    /// aim added to each in turn.
    fn marks_at(from: u32, bpm: f64, every: u32, count: usize, off_by: &[i32]) -> Vec<u32> {
        let period = 60_000.0 / bpm;
        (0..count)
            .map(|n| {
                let at = from as f64 + n as f64 * every as f64 * period;
                let slip = off_by.get(n % off_by.len().max(1)).copied().unwrap_or(0);
                (at.round() as i64 + slip as i64).max(0) as u32
            })
            .collect()
    }

    #[test]
    fn two_marks_a_bar_apart_give_the_tempo_between_them() {
        // The case this is for: a tracker reading a record at the wrong
        // tempo, and a DJ who can see two kicks it should have landed on.
        let mut track = gridded();
        track.bpm = 126.0;
        let marks = marks_at(500, 124.0, 4, 2, &[0]);
        assert!(fit(&mut track, &marks));
        assert!((track.bpm - 124.0).abs() < 0.5, "fitted {:.2}", track.bpm);
        assert_eq!(track.downbeat_ms, Some(500), "the first mark is the one");
    }

    #[test]
    fn a_fit_prefers_a_plain_grid_and_throws_the_kept_beats_away() {
        // The whole point of preferring one: a tempo and a downbeat survive a
        // re-analysis, rebuild the picture, and make eight beats from one end
        // of a record the same as eight from the other. A list of times says
        // that only by accident, so a track that no longer needs one should
        // stop carrying it.
        let mut track = bent();
        let marks = marks_at(0, 120.0, 4, 6, &[0]);
        assert!(fit(&mut track, &marks));
        assert!(!is_dynamic(&track), "it kept a list of beats a tempo could say");
        assert!((track.bpm - 120.0).abs() < 0.5, "fitted {:.2}", track.bpm);
    }

    #[test]
    fn a_record_no_single_tempo_explains_is_fitted_with_one_that_bends() {
        // And only then. These marks drift further apart every bar, which is
        // a record slowing down — no tempo passes through all of them, so the
        // marks themselves become the grid.
        let mut track = gridded();
        let marks: Vec<u32> = vec![0, 2_000, 4_100, 6_400, 8_900, 11_600];
        assert!(fit(&mut track, &marks));
        assert!(is_dynamic(&track), "a slowing record was flattened to one tempo");
        for mark in &marks {
            assert!(
                track.beat_ms.iter().any(|beat| beat.abs_diff(*mark) <= 1),
                "the grid misses the mark at {mark}: {:?}",
                track.beat_ms
            );
        }
    }

    #[test]
    fn a_fit_forgives_the_aim_of_somebody_clicking_a_waveform() {
        // Marks a few milliseconds either side of the beat are somebody
        // pointing, not a record that moves. A fit that called this dynamic
        // would hand back a bent grid for every track anybody ever marked.
        let mut track = gridded();
        track.bpm = 126.0;
        let marks = marks_at(1_000, 128.0, 4, 8, &[0, 7, -6, 3, -9, 5]);
        assert!(fit(&mut track, &marks));
        assert!(!is_dynamic(&track), "human aim was read as a bending record");
        assert!((track.bpm - 128.0).abs() < 0.5, "fitted {:.2}", track.bpm);
    }

    #[test]
    fn one_mark_moves_the_grid_without_guessing_a_tempo() {
        // One point cannot say how fast a record is. It can say where a beat
        // falls, which is what the grid is slid to.
        let mut track = gridded();
        let was = track.bpm;
        assert!(fit(&mut track, &[1_234]));
        assert_eq!(track.bpm, was, "one mark invented a tempo");
        assert!(nearest_beat(&track, 1_234) == Some(1_234), "the grid did not come to the mark");
    }

    #[test]
    fn a_tie_between_a_tempo_and_half_of_it_goes_to_the_one_already_read() {
        // Marks every two beats at 128 are also marks every beat at 64. Both
        // explain them perfectly, so the tempo the track already carries
        // breaks the tie — otherwise fitting a correct grid could halve it.
        let mut track = gridded();
        track.bpm = 128.0;
        let marks = marks_at(0, 128.0, 2, 5, &[0]);
        assert!(fit(&mut track, &marks));
        assert!((track.bpm - 128.0).abs() < 0.5, "fitted {:.2}", track.bpm);
    }

    #[test]
    fn marks_out_of_order_or_repeated_are_still_a_fit() {
        // They arrive in the order somebody clicked, which is not necessarily
        // along the track, and clicking the same place twice is a slip rather
        // than a second beat.
        let mut track = gridded();
        let tidy = marks_at(0, 120.0, 4, 4, &[0]);
        let mut untidy = tidy.clone();
        untidy.reverse();
        untidy.push(tidy[2]);

        let mut other = track.clone();
        assert!(fit(&mut track, &tidy));
        assert!(fit(&mut other, &untidy));
        assert_eq!(track.bpm, other.bpm);
        assert_eq!(track.downbeat_ms, other.downbeat_ms);
    }

    #[test]
    fn marks_can_only_be_read_as_beats_or_as_several_of_them() {
        // What the marks can and cannot say, measured rather than assumed —
        // the first version of this test asserted the ambiguity ran both ways
        // and the arithmetic proved it does not.
        //
        // A gap is only ever divided, never multiplied: marks a gap apart say
        // the beat is that gap, or half of it, or a third. So marks on
        // consecutive beats pin the tempo whatever the track was read at,
        // because no reading of them is slower than the gap.
        let every_beat = marks_at(0, 128.0, 1, 8, &[0]);
        let mut badly_read = gridded();
        badly_read.bpm = 128.0 / 3.0;
        assert!(fit(&mut badly_read, &every_beat));
        assert!(
            (badly_read.bpm - 128.0).abs() < 0.5,
            "fitted {:.2} from a track read at a third of its tempo",
            badly_read.bpm
        );

        // The ambiguity is the other way. Marks every four beats at 128 are
        // also marks on consecutive beats at 32, and nothing in them says
        // which — so the tempo the track already carries decides.
        let every_bar = marks_at(0, 128.0, 4, 6, &[0]);
        let mut slow = gridded();
        slow.bpm = 32.0;
        assert!(fit(&mut slow, &every_bar));
        assert!(
            (slow.bpm - 32.0).abs() < 0.5,
            "fitted {:.2}, so something other than the prior broke the tie",
            slow.bpm
        );

        // Which is the case to know about: marking bar lines on a record read
        // at a quarter of its tempo confirms the wrong reading. Marking beats
        // does not, and ÷2 and ×2 are the tools for a gross misreading.
        let mut right = gridded();
        right.bpm = 126.0;
        assert!(fit(&mut right, &every_bar));
        assert!((right.bpm - 128.0).abs() < 0.5, "fitted {:.2} from 126", right.bpm);
    }

    #[test]
    fn a_grid_kept_beat_by_beat_can_be_converted_to_a_tempo_and_a_downbeat() {
        // For beats tracked untidily rather than a record that moves: the
        // list is thousands of numbers saying what two would say, it cannot
        // be rebuilt if it is lost, and loops from the two ends of such a
        // track do not match.
        let mut track = gridded();
        track.beat_ms = (0..64).map(|n| (n as f64 * 500.0).round() as u32).collect();
        track.bpm = 119.0;
        assert!(flatten(&mut track));
        assert!(!is_dynamic(&track));
        assert_eq!(track.bpm, 120.0, "500 ms a beat is 120 BPM");
        assert_eq!(track.downbeat_ms, Some(0));
    }

    #[test]
    fn converting_a_grid_that_is_already_plain_changes_nothing() {
        let mut track = gridded();
        assert!(!flatten(&mut track), "there was nothing to convert");
        assert_eq!(track.bpm, 128.0);
    }

    #[test]
    fn a_track_with_no_grid_refuses_every_one_of_them() {
        // Nothing to correct, and inventing a grid from a tempo of zero would
        // be worse than the buttons doing nothing.
        let mut track = Track::placeholder(1);
        track.duration_secs = 240.0;
        assert!(!double(&mut track));
        assert!(!halve(&mut track));
        assert!(!nudge(&mut track, NUDGE_MS));
        assert!(!move_to(&mut track, 1_000));
        assert!(!set_downbeat(&mut track, 1_000));
        assert!(!set_bpm(&mut track, 128.0));
        assert!(!fit(&mut track, &[1_000, 2_000]));
        assert!(!flatten(&mut track));
    }

    /// A grid that bends: beats that creep, kept one by one.
    fn bent() -> Track {
        let mut track = gridded();
        let (mut at, mut gap) = (0.0f64, 500.0f64);
        track.beat_ms = (0..64)
            .map(|_| {
                let was = at.round() as u32;
                at += gap;
                gap -= 1.0;
                was
            })
            .collect();
        track.bpm = 120.0;
        track.beats = track.beat_ms.len();
        track
    }

    #[test]
    fn doubling_a_bent_grid_puts_a_beat_between_each_pair() {
        let mut track = bent();
        let was = track.beat_ms.clone();
        assert!(double(&mut track));

        assert_eq!(track.beat_ms.len(), was.len() * 2 - 1);
        assert_eq!(track.beat_ms[0], was[0]);
        assert_eq!(track.beat_ms[2], was[1], "the original beats moved");
        assert_eq!(track.beat_ms[1], was[0] + (was[1] - was[0]) / 2);
    }

    #[test]
    fn halving_a_bent_grid_keeps_every_other_beat_from_the_first() {
        let mut track = bent();
        let was = track.beat_ms.clone();
        assert!(halve(&mut track));

        assert_eq!(track.beat_ms[0], was[0], "the downbeat stopped being one");
        assert_eq!(track.beat_ms[1], was[2]);
        assert_eq!(track.beats, track.beat_ms.len());
    }

    #[test]
    fn nudging_a_bent_grid_moves_every_beat_of_it() {
        let mut track = bent();
        let was = track.beat_ms.clone();
        assert!(nudge(&mut track, SHOVE_MS));

        for (before, after) in was.iter().zip(&track.beat_ms) {
            assert_eq!(*after, before + SHOVE_MS as u32);
        }
    }

    #[test]
    fn the_one_of_a_bent_grid_is_set_by_dropping_what_came_before_it() {
        // A bent grid is kept from its first downbeat on, so there is nowhere
        // to record a phase: the beats before the new one are not beats of
        // this grid any more.
        let mut track = bent();
        let third = track.beat_ms[2];
        assert!(set_downbeat(&mut track, third));

        assert_eq!(track.beat_ms[0], third);
        assert_eq!(track.beats, track.beat_ms.len());
    }
}
