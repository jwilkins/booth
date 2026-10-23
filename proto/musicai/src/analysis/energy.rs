//! Where a record's drops and breaks are, by how much it is playing.
//!
//! The arrangement analysis names sections, and it can miss one: on a record
//! whose chorus came back after its break, it found an intro, the break and an
//! outro, and nothing where the chorus returned. A cue that has to land on the
//! main drop every time — button D, on every record — cannot rest on a name
//! that can be missing. So the drops and breaks are found again here, from how
//! much is being played bar by bar, and the names are used only to say exactly
//! where one starts.
//!
//! A drop is a stretch of at least [`RUN_BARS`] bars in the upper half of the
//! record's own range; a break is what lies between two drops, if it lasts as
//! long, so that the intro and the outro are not breaks. The range is the
//! record's own, between its quietest and loudest bars, because what makes a
//! drop is that it is the loud part of *this* record: a deep house drop is
//! quieter than a techno break.
//!
//! Both the analysis and the app read it off the three-band picture, which the
//! app keeps for every track, so that the main drop the command line suggests
//! and the one the app puts on D are the same bar.

use super::place::BEATS_PER_BAR;

/// The shortest stretch that counts as a drop or a break: eight bars, a phrase.
/// Shorter is a fill, a build's last bars or a drum drop for effect.
pub const RUN_BARS: usize = 8;

/// The longest dip inside a stretch that does not end it: a bar or two with the
/// kick taken out for a fill is still the drop.
const BRIDGE_BARS: usize = 2;

/// Which of a record's bars mark its quiet and its loud end. A tenth in from
/// either end rather than the extremes, so that one silent bar at the top of
/// the file or one clipped crash does not set the scale.
const QUIET_QUANTILE: f32 = 0.1;
const LOUD_QUANTILE: f32 = 0.9;

/// Columns of the three-band picture per second: see
/// [`crate::export::waveform`].
const COLUMNS_PER_SECOND: usize = 150;

/// How many columns the low band is averaged over before its rises are
/// counted: eight, about 50 ms.
///
/// A chord in the low mids beats — two notes a third apart near 200 Hz swell
/// and fade fifty-odd times a second — and the picture's 7 ms columns follow
/// it, so a pad held through a break rises and falls as often as a drop's
/// kick does, only less far. Measured on a made record with a pad on A and
/// C sharp, the low band's rises read 0.51 a column in the break against 1.0
/// in the drop: half. Averaged over eight columns the beating is gone and a
/// kick, which takes a tenth of a second to swell and fade, is not: 0.03
/// against 0.59.
const SMOOTH_COLUMNS: usize = 8;

/// A stretch of bars, `from` up to but not including `to`, counted from the
/// first bar of the grid, and how much is being played across it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Run {
    pub from: usize,
    pub to: usize,
    pub energy: f32,
}

impl Run {
    pub fn bars(&self) -> usize {
        self.to - self.from
    }

    pub fn holds(&self, bar: usize) -> bool {
        (self.from..self.to).contains(&bar)
    }
}

/// A record's drops and breaks, by energy.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Shape {
    pub drops: Vec<Run>,
    pub breaks: Vec<Run>,
}

impl Shape {
    /// The drop with the most energy in it: the one a DJ means by "the drop".
    /// Between two alike, the longer, then the earlier.
    pub fn main_drop(&self) -> Option<Run> {
        self.drops.iter().copied().reduce(|best, run| {
            match (run.energy, run.bars()) > (best.energy, best.bars()) {
                true => run,
                false => best,
            }
        })
    }

    /// The break with the least energy in it: the deepest, where a second
    /// record has the most room. Between two alike, the longer, then the
    /// earlier.
    pub fn main_break(&self) -> Option<Run> {
        self.breaks.iter().copied().reduce(|best, run| {
            match run.energy < best.energy
                || (run.energy == best.energy && run.bars() > best.bars())
            {
                true => run,
                false => best,
            }
        })
    }

    /// Whether a bar is inside one of the drops.
    pub fn in_a_drop(&self, bar: usize) -> bool {
        self.drops.iter().any(|run| run.holds(bar))
    }
}

/// How much is being played in each bar, from the three-band picture a
/// collection keeps of every track: three bytes a column — low, mid, high —
/// at [`COLUMNS_PER_SECOND`].
///
/// How loud the bar is, and how hard its low end hits, together. Neither is
/// enough alone. A pad held through a breakdown is nearly as loud as the drop
/// either side of it: on a made record, 3 dB under, and in the picture, whose
/// bands are each scaled against their own loudest, louder — the pad is the
/// loudest thing in the mids. And hi-hats on their own are as busy as a drop:
/// counted by onsets, a hats-only intro came out at 84 per cent of one. What a
/// drop has that neither has is the kick, which is the low band rising sharply
/// once a beat; so the measure is the geometric mean of the bar's power and of
/// how far, column by column, its low band climbs. The geometric mean so that
/// the loudest bar with no kick and the hardest kick with nothing round it
/// both come out low, and so that a drop twice as loud as another reads as
/// more than one with a bigger kick in it.
///
/// `grid` is every beat from the first downbeat; a bar the picture does not
/// reach, or a last bar without all its beats, is left out.
pub fn bars(bands: &[u8], grid: &[u32]) -> Vec<f32> {
    let columns = bands.len() / 3;
    let band = |column: usize, band: usize| f32::from(bands[column * 3 + band]) / 127.0;
    // A running mean over the last few columns, so that it can be read across
    // a bar line.
    let mut low = Vec::with_capacity(columns);
    let mut sum = 0.0;
    for column in 0..columns {
        sum += band(column, 0);
        if column >= SMOOTH_COLUMNS {
            sum -= band(column - SMOOTH_COLUMNS, 0);
        }
        low.push(sum / (column + 1).min(SMOOTH_COLUMNS) as f32);
    }
    let column = |ms: u32| ms as usize * COLUMNS_PER_SECOND / 1000;
    each_bar(grid, |from_ms, to_ms| {
        let (from, to) = (column(from_ms), column(to_ms));
        (to > from && to <= columns).then(|| {
            let width = (to - from) as f32;
            let power = (from..to)
                .map(|c| (0..3).map(|b| band(c, b).powi(2)).sum::<f32>() / 3.0)
                .sum::<f32>()
                / width;
            let rises = (from.max(1)..to).map(|c| (low[c] - low[c - 1]).max(0.0)).sum::<f32>();
            (power * rises / width).sqrt()
        })
    })
}

fn each_bar(grid: &[u32], measure: impl Fn(u32, u32) -> Option<f32>) -> Vec<f32> {
    let mut bars = Vec::new();
    for bar in grid.chunks(BEATS_PER_BAR) {
        if bar.len() < BEATS_PER_BAR {
            break;
        }
        // To the next bar line, or one beat past this bar's last beat where
        // the grid ends.
        let at = bars.len() * BEATS_PER_BAR;
        let to = grid
            .get(at + BEATS_PER_BAR)
            .copied()
            .unwrap_or_else(|| bar[3] + bar[3].saturating_sub(bar[2]));
        match measure(bar[0], to) {
            Some(energy) => bars.push(energy),
            None => break,
        }
    }
    bars
}

/// Find the drops and the breaks in a record's bars.
pub fn shape(bars: &[f32]) -> Shape {
    if bars.len() < RUN_BARS * 2 {
        return Shape::default();
    }
    let mut sorted = bars.to_vec();
    sorted.sort_by(f32::total_cmp);
    let at = |quantile: f32| sorted[((sorted.len() - 1) as f32 * quantile).round() as usize];
    let (quiet, loud) = (at(QUIET_QUANTILE), at(LOUD_QUANTILE));
    if loud <= quiet {
        // A record that never changes how much it plays has no drop to find.
        return Shape::default();
    }
    let middle = (quiet + loud) / 2.0;

    let run = |from: usize, to: usize| Run {
        from,
        to,
        energy: bars[from..to].iter().sum::<f32>() / (to - from) as f32,
    };
    let runs = || -> Vec<Run> {
        let mut runs: Vec<(usize, usize)> = Vec::new();
        for (bar, &energy) in bars.iter().enumerate() {
            if energy < middle {
                continue;
            }
            match runs.last_mut() {
                Some((_, to)) if bar - *to <= BRIDGE_BARS => *to = bar + 1,
                _ => runs.push((bar, bar + 1)),
            }
        }
        runs.into_iter()
            .filter(|(from, to)| to - from >= RUN_BARS)
            .map(|(from, to)| run(from, to))
            .collect()
    };
    let drops = runs();
    // The gaps between drops rather than runs of quiet bars of their own: a
    // bar of the drop that dips, which the drop bridges, would otherwise start
    // a quiet run inside the drop, and the break after it would seem to begin
    // before the drop ended.
    let breaks = drops
        .windows(2)
        .map(|pair| (pair[0].to, pair[1].from))
        .filter(|(from, to)| to - from >= RUN_BARS)
        .map(|(from, to)| run(from, to))
        .collect();
    Shape { drops, breaks }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Bars at these levels, `count` of each, in order.
    fn record(parts: &[(f32, usize)]) -> Vec<f32> {
        parts.iter().flat_map(|&(level, count)| std::iter::repeat_n(level, count)).collect()
    }

    #[test]
    fn a_record_with_two_drops_and_a_break_between_has_the_louder_drop_as_its_main_one() {
        // Intro 16, drop 32, break 16, drop 32 a little louder, outro 16.
        let bars = record(&[(20.0, 16), (80.0, 32), (25.0, 16), (90.0, 32), (30.0, 16)]);
        let shape = shape(&bars);
        assert_eq!(
            shape.drops.iter().map(|run| (run.from, run.to)).collect::<Vec<_>>(),
            [(16, 48), (64, 96)]
        );
        assert_eq!(shape.main_drop().map(|run| run.from), Some(64));
        assert_eq!(
            shape.breaks.iter().map(|run| (run.from, run.to)).collect::<Vec<_>>(),
            [(48, 64)],
            "the intro and the outro are not breaks"
        );
        assert_eq!(shape.main_break().map(|run| run.from), Some(48));
    }

    #[test]
    fn a_bar_or_two_with_the_kick_out_does_not_end_a_drop() {
        let bars = record(&[(20.0, 16), (80.0, 15), (30.0, 2), (80.0, 15), (20.0, 16)]);
        assert_eq!(
            shape(&bars).drops.iter().map(|run| (run.from, run.to)).collect::<Vec<_>>(),
            [(16, 48)]
        );
    }

    #[test]
    fn four_loud_bars_are_not_a_drop() {
        let bars = record(&[(20.0, 32), (80.0, 4), (20.0, 32)]);
        assert!(shape(&bars).drops.is_empty());
    }

    #[test]
    fn of_two_breaks_the_deepest_is_the_main_one() {
        let bars = record(&[(80.0, 16), (40.0, 8), (80.0, 16), (15.0, 16), (80.0, 16)]);
        let shape = shape(&bars);
        assert_eq!(shape.breaks.len(), 2, "{shape:?}");
        assert_eq!(shape.main_break().map(|run| run.from), Some(40));
    }

    #[test]
    fn a_record_that_never_changes_has_no_drop_and_no_break() {
        assert_eq!(shape(&[50.0; 64]), Shape::default());
    }

    /// A record made for the measurement, at 120 BPM so a bar is two
    /// seconds: 16 bars of hi-hats, a 32-bar drop of kick, bass and hats, a
    /// 16-bar break of a pad alone, a 32-bar drop with a lead over it as
    /// well, and 16 bars of hats out.
    fn arranged() -> crate::audio::Audio {
        const RATE: u32 = 22_050;
        let bar = 2.0f32;
        let parts: [(usize, bool, bool, bool, bool); 5] = [
            // (bars, kick and bass, hats, pad, lead)
            (16, false, true, false, false),
            (32, true, true, false, false),
            (16, false, false, true, false),
            (32, true, true, false, true),
            (16, false, true, false, false),
        ];
        let total: usize = parts.iter().map(|part| part.0).sum();
        let mut plane = vec![0.0f32; (total as f32 * bar * RATE as f32) as usize];
        let mut start = 0usize;
        let tau = 2.0 * std::f32::consts::PI;
        for (bars, low, hats, pad, lead) in parts {
            let from = (start as f32 * bar * RATE as f32) as usize;
            let to = ((start + bars) as f32 * bar * RATE as f32) as usize;
            for (i, sample) in plane.iter_mut().enumerate().take(to).skip(from) {
                let t = i as f32 / RATE as f32;
                let in_beat = t % 0.5;
                let mut x = 0.0;
                if low {
                    x += 0.6 * (tau * 55.0 * t).sin() * (-in_beat * 12.0).exp();
                    x += 0.3 * (tau * 41.2 * t).sin();
                }
                if hats {
                    let off = (t + 0.25) % 0.5;
                    x += 0.15
                        * ((i as f32 * 12.9898).sin() * 43_758.545).fract()
                        * (-off * 60.0).exp();
                }
                if pad {
                    x += 0.2 * (tau * 220.0 * t).sin() + 0.15 * (tau * 277.2 * t).sin();
                }
                if lead {
                    x += 0.25 * (tau * 880.0 * t).sin() * (tau * 2.0 * t).sin().abs();
                }
                *sample = x;
            }
            start += bars;
        }
        crate::audio::Audio::new(RATE, vec![plane]).unwrap()
    }

    #[test]
    fn on_a_made_record_the_picture_puts_the_main_drop_and_break_where_they_were_made() {
        // The measurement behind reading drops off the collection's picture:
        // the louder drop starts at bar 64 and the break at bar 48, and the
        // hat-only intro and outro are neither drops nor breaks, though the
        // hats are as busy as a drop and the pad in the break is the loudest
        // thing in the mids.
        let audio = arranged();
        let grid: Vec<u32> = (0..448).map(|beat| beat * 500).collect();
        let bands = crate::export::waveform::analyze(&audio).band_detail;
        let shape = shape(&bars(&bands, &grid));
        let spans = |runs: &[Run]| runs.iter().map(|run| (run.from, run.to)).collect::<Vec<_>>();
        assert_eq!(spans(&shape.drops), [(16, 48), (64, 96)], "{shape:?}");
        assert_eq!(spans(&shape.breaks), [(48, 64)], "{shape:?}");
        assert_eq!(shape.main_drop().map(|run| run.from), Some(64), "{shape:?}");
    }

    /// A picture `bars` bars long at 120 BPM, a bar 300 columns, with every
    /// band at `level` and the low band also kicking to `kick` for the first
    /// tenth of each beat.
    fn picture(bars: usize, level: u8, kick: u8) -> Vec<u8> {
        (0..bars * 300)
            .flat_map(|column| {
                let low = if column % 75 < 8 { kick.max(level) } else { level };
                [low, level, level]
            })
            .collect()
    }

    #[test]
    fn a_bar_with_a_kick_in_it_has_more_energy_than_a_louder_bar_without() {
        // The loud bar first, so that stepping up to it from the quiet one is
        // not a rise in it.
        let grid: Vec<u32> = (0..8).map(|beat| beat * 500).collect();
        let mut bands = picture(1, 90, 0);
        bands.extend(picture(1, 30, 100));
        let measured = bars(&bands, &grid);
        assert_eq!(measured.len(), 2);
        assert!(measured[1] > measured[0], "{measured:?}");
    }

    #[test]
    fn a_bar_held_at_one_level_has_no_energy_however_loud() {
        let grid: Vec<u32> = (0..12).map(|beat| beat * 500).collect();
        assert_eq!(bars(&picture(3, 120, 0), &grid), [0.0; 3]);
    }

    #[test]
    fn a_bar_past_the_end_of_the_picture_is_not_measured() {
        let grid: Vec<u32> = (0..12).map(|beat| beat * 500).collect();
        let bands = vec![30u8; 450 * 3];
        assert_eq!(bars(&bands, &grid).len(), 1);
    }
}
