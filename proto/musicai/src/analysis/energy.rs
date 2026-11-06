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

/// How many bars apart two breaks' lengths can be and still count as the same
/// length: two. A stretch is measured in whole bars and bridged over a dip of
/// up to [`BRIDGE_BARS`], so its edges can each land a bar or two either way of
/// where an ear would put them.
pub const ALIKE_BARS: usize = 2;

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

/// How many bars before a drop it is measured against: a phrase, the eight a
/// build or the end of a breakdown takes.
pub const BEFORE_BARS: usize = 8;

/// How little of a drop's kick the bars before it can keep and still count as
/// the drums having dropped out: about a third.
///
/// In the gap the records leave. Before the drops of 31 records, where the
/// drums had gone the bars kept 0.05 to 0.30 of the drop's kick (Upside,
/// tou207c, Aku, Blunted My Transistor); where the kick played on and the bass
/// or the chords were taken out instead, 0.38 to 0.9 (Signal, tou207a,
/// toulp2d). The low band's rises are the kick and the bassline both, so a
/// bassline playing through a drum break keeps some.
pub const DRUMS_OUT: f32 = 0.35;

/// A stretch of bars, `from` up to but not including `to`, counted from the
/// first bar of the grid, and how much is being played across it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Run {
    pub from: usize,
    pub to: usize,
    pub energy: f32,
    /// For a drop, how many times harder it plays than the [`BEFORE_BARS`]
    /// before it. `None` for a break, and for a drop with fewer bars than that
    /// before it, which has no section of its own to climb out of.
    pub lift: Option<f32>,
    /// For a drop, how much of its kick those bars kept, against its own first
    /// [`BEFORE_BARS`]: near nothing where the drums dropped out, near one
    /// where the kick played on and something else was taken out. `None` where
    /// the kick was not measured.
    pub kept_kick: Option<f32>,
}

impl Run {
    pub fn bars(&self) -> usize {
        self.to - self.from
    }

    pub fn holds(&self, bar: usize) -> bool {
        (self.from..self.to).contains(&bar)
    }
}

/// Where a kind of record puts its main drop, how far that drop climbs over
/// the bars before it, and what those bars take out.
///
/// A drop comes after a stretch that holds back, and plays a good deal harder
/// than it: that much is every kind of club record. What differs is where the
/// one a set is built round falls, and what the stretch before it leaves out.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Arranged {
    /// Which part of the record the main drop is in, and of two drops that
    /// climb about as far, which is the main one.
    pub half: Half,
    /// How many times harder than the [`BEFORE_BARS`] before it a drop has to
    /// play to count as climbing out of anything.
    pub lift: f32,
    /// Whether the main drop is one the drums drop out before, where the
    /// record has one.
    pub drums_out: bool,
    /// The genre this is for, as the genre tempo table names it, or empty.
    pub genre: &'static str,
}

/// Which part of a record its main drop is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Half {
    /// Before halfway, and of two as big, the earlier. Drum and bass and
    /// dubstep put their drop after an intro of a phrase or more, and that
    /// drop is the one a DJ cues: what follows the breakdown halfway through
    /// is the same drop again, often with less above it.
    First,
    /// After the first third, and of two as big, the later: house, techno,
    /// trance, where the intro's first drop is the groove setting out and the
    /// drop the record builds to comes after it, at a third of the way in or
    /// later — on the records here, from 0.37 of the way through.
    Later,
}

impl Half {
    /// Whether a drop starting at `bar` is in this part of a record `bars`
    /// long.
    fn holds(self, bar: usize, bars: usize) -> bool {
        match self {
            Half::First => bar * 2 < bars,
            Half::Later => bar * 3 >= bars,
        }
    }
}

impl Arranged {
    /// A record nobody has said the kind of: as house and techno arrange it,
    /// which is most of what a club DJ plays.
    ///
    /// A lift of one and a half. The drops that answer a breakdown on the 31
    /// records climbed 1.8 to 33 times over the eight bars before; the
    /// stretches of records that never hold back, which the shape still reads
    /// as drops, 0.99 to 1.45.
    pub const UNKNOWN: Arranged =
        Arranged { half: Half::Later, lift: 1.5, drums_out: false, genre: "" };

    /// How a genre arranges its main drop.
    ///
    /// By genre rather than by the table's family where the two part: the
    /// family it files dubstep under also holds breakbeat, and the two
    /// progressive breaks records here are built as house is, their drop
    /// after a long breakdown.
    pub fn of(genre: &super::tempo::Genre) -> Arranged {
        let arranged =
            |half, drums_out| Arranged { half, drums_out, genre: genre.name, ..Self::UNKNOWN };
        match (genre.family, genre.id) {
            ("dnb", _) | (_, "dubstep" | "trap-edm" | "future-bass") => {
                arranged(Half::First, false)
            }
            // A trance breakdown takes the drums out and keeps the melody,
            // and the drop it builds to brings them back.
            ("trance", _) => arranged(Half::Later, true),
            _ => arranged(Half::Later, false),
        }
    }

    /// How the surest of `genres` arranges its main drop, or
    /// [`Arranged::UNKNOWN`] where there are none. Of two as sure, the first:
    /// a tag reading "Jungle, Dubstep" is filed under jungle before anything.
    pub fn of_genres(genres: &[super::tempo::Genre]) -> Arranged {
        genres
            .iter()
            .reduce(|surest, genre| if genre.sure > surest.sure { genre } else { surest })
            .map_or(Self::UNKNOWN, Self::of)
    }
}

impl Default for Arranged {
    fn default() -> Self {
        Self::UNKNOWN
    }
}

/// How the main drop was chosen, for the log to say.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Chosen {
    /// The drop that climbs furthest, in the part of the record its kind puts
    /// the main drop in.
    Biggest,
    /// Nothing climbs far enough where it should, so as before: the drop
    /// after the breakdown, or the one with the most energy.
    AsBefore,
}

/// A lift within a tenth of the biggest counts as as big, and then the kind of
/// record says which is the main drop: see [`Half`]. Three drops each after an
/// eight-bar break, at 6.11, 6.69 and 6.48, are three answers to the same
/// question, and which measures a hair higher is not something the record
/// chose.
const ALIKE_LIFT: f32 = 0.9;

/// A record's drops and breaks, by energy.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Shape {
    pub drops: Vec<Run>,
    pub breaks: Vec<Run>,
    /// How many bars the record runs to, for where in it a drop falls.
    pub bars: usize,
    /// The record's average energy a bar, intro and outro and all, which the
    /// main drop has to play harder than: see [`Shape::choose`].
    pub average: f32,
    /// How its kind arranges its main drop: [`Arranged::UNKNOWN`] until
    /// somebody says what kind of record it is.
    pub arranged: Arranged,
}

impl Shape {
    /// The same shape, for a kind of record.
    pub fn arranged_as(self, arranged: Arranged) -> Shape {
        Shape { arranged, ..self }
    }

    /// The drop a DJ means by "the drop", for the kind of record this is: see
    /// [`Arranged`] and [`Shape::choose`].
    pub fn main_drop(&self) -> Option<Run> {
        self.choose().0
    }

    /// The main drop, and how it was chosen.
    ///
    /// The biggest drop: the one that climbs furthest over the bars before it,
    /// of those in the part of the record its kind puts the main drop in that
    /// climb at least [`Arranged::lift`] times and play harder than the
    /// record's [`Shape::average`] — and where the kind wants the drums out
    /// before its drop and some of those have them out, of those. Where nothing
    /// there does all that, as before — see [`Shape::after_the_breakdown`] —
    /// because a record that never holds back still has its loud part.
    ///
    /// Harder than the average as well as harder than the bars before it,
    /// because a climb is measured against whatever came before, and a stretch
    /// that comes out of near-silence climbs a long way without being loud. A
    /// drop that plays softer than the record does on the whole is not the
    /// part of it a set is built round, however quiet the bars before it.
    ///
    /// Not the first drop after the breakdown, which is what this was, nor the
    /// first drop that climbs far enough. Either is a place in the arrangement
    /// rather than how hard a drop hits, and the drop a DJ cues is the one that
    /// hits hardest. On the records here the drop after the longest break is
    /// often not that one: Brain Dance climbs 2.35 times out of its 32-bar
    /// breakdown and 2.94 out of the eight-bar break that takes the drums out
    /// later. Measured over the whole stretch since the drop before rather than
    /// the last eight bars, the breakdown's drop climbs less still, 1.86.
    pub fn choose(&self) -> (Option<Run>, Chosen) {
        let Arranged { half, lift: need, drums_out, .. } = self.arranged;
        let mut climbing: Vec<Run> = self
            .drops
            .iter()
            .copied()
            .filter(|run| half.holds(run.from, self.bars))
            .filter(|run| run.lift.is_some_and(|lift| lift >= need))
            .filter(|run| self.louder_than_average(run))
            .collect();
        let dropped_out = |run: &Run| run.kept_kick.is_some_and(|kept| kept <= DRUMS_OUT);
        if drums_out && climbing.iter().any(dropped_out) {
            climbing.retain(dropped_out);
        }
        let Some(biggest) = climbing.iter().filter_map(|run| run.lift).reduce(f32::max) else {
            return (self.after_the_breakdown(), Chosen::AsBefore);
        };
        let as_big = |run: &&Run| run.lift.is_some_and(|lift| lift >= ALIKE_LIFT * biggest);
        let run = match half {
            Half::First => climbing.iter().find(as_big),
            Half::Later => climbing.iter().rfind(as_big),
        };
        (run.copied(), Chosen::Biggest)
    }

    /// The drop after the record's breakdown, where it has one: what the main
    /// drop was for every kind of record before kinds were told apart, and
    /// still is where none climbs far enough.
    ///
    /// Not the drop with the most energy in it, which is what this was. A club
    /// record is mastered so that its drops play about as hard as each other,
    /// and which measures a hair louder is not a choice the record makes: on
    /// four records of eighteen it put D on the intro, on a drop before the
    /// breakdown rather than after it, or on a stretch with the drums barely
    /// in, by margins as thin as 0.96 against 0.95. What the arrangement does
    /// say is where it holds back, and the drop that answers that is the one
    /// a set is built round.
    ///
    /// The breakdown is the main break, if it is quieter than every drop. One
    /// that is not is a dip between two loud stretches rather than a record
    /// holding back — on one record of the eighteen, the drop after it began
    /// with its drums barely in — and then, as with no break at all, the main
    /// drop is the one with the most energy, between two alike the longer,
    /// then the earlier.
    ///
    /// The first drop after the breakdown that plays harder than the record's
    /// [`Shape::average`], that is, passing over one that does not, for the
    /// reason [`Shape::choose`] gives. On Get Free this put D on an eight-bar
    /// stretch at 0.83 of the record's average straight after its breakdown,
    /// and now puts it on the 33 bars at 1.17 that come in sixteen seconds
    /// later.
    pub fn after_the_breakdown(&self) -> Option<Run> {
        let after_the_breakdown = self
            .main_break()
            .filter(|quiet| self.drops.iter().all(|drop| quiet.energy < drop.energy))
            .and_then(|quiet| {
                self.drops
                    .iter()
                    .find(|drop| drop.from >= quiet.from && self.louder_than_average(drop))
                    .copied()
            });
        after_the_breakdown.or_else(|| self.loudest_drop())
    }

    /// Whether a drop plays harder than the record does on average.
    fn louder_than_average(&self, run: &Run) -> bool {
        run.energy > self.average
    }

    /// The drop with the most energy in it. Between two alike, the longer,
    /// then the earlier.
    fn loudest_drop(&self) -> Option<Run> {
        self.drops.iter().copied().reduce(|best, run| {
            match (run.energy, run.bars()) > (best.energy, best.bars()) {
                true => run,
                false => best,
            }
        })
    }

    /// The break a DJ means by "the break": the longest, and of those about
    /// as long — within [`ALIKE_BARS`] of it — the latest.
    ///
    /// Not the deepest, which is what this was. A short break near the top of
    /// a record is often its quietest, and it is not the one a set is built
    /// round: that is the long breakdown later on, the one the record builds
    /// to, where a second record has the most time to be laid over and which
    /// the main drop answers.
    pub fn main_break(&self) -> Option<Run> {
        let longest = self.breaks.iter().map(Run::bars).max()?;
        self.breaks.iter().copied().rfind(|run| run.bars() + ALIKE_BARS >= longest)
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
    measured(bands, grid).into_iter().map(|(energy, _)| energy).collect()
}

/// How hard each bar's kick hits: how far, column by column, its low band
/// climbs, the half of [`bars`] that is the drums. For telling a break that
/// took the drums out from one that kept them: see [`DRUMS_OUT`].
pub fn kicks(bands: &[u8], grid: &[u32]) -> Vec<f32> {
    measured(bands, grid).into_iter().map(|(_, kick)| kick).collect()
}

/// Each bar's [`bars`] and its [`kicks`].
fn measured(bands: &[u8], grid: &[u32]) -> Vec<(f32, f32)> {
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
            ((power * rises / width).sqrt(), rises / width)
        })
    })
}

fn each_bar<T>(grid: &[u32], measure: impl Fn(u32, u32) -> Option<T>) -> Vec<T> {
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
            Some(measured) => bars.push(measured),
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

    let run = |from: usize, to: usize| stretch(bars, from, to);
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
            .map(|(from, to)| as_drop(bars, from, to))
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
    let average = bars.iter().sum::<f32>() / bars.len() as f32;
    Shape { drops, breaks, bars: bars.len(), average, ..Shape::default() }
}

/// Bars `from` to `to`, and how much they play.
fn stretch(bars: &[f32], from: usize, to: usize) -> Run {
    Run {
        from,
        to,
        energy: bars[from..to].iter().sum::<f32>() / (to - from) as f32,
        lift: None,
        kept_kick: None,
    }
}

/// Bars `from` to `to` as a drop: how much they play, and how far that climbs
/// over the [`BEFORE_BARS`] before them where there are that many.
fn as_drop(bars: &[f32], from: usize, to: usize) -> Run {
    let run = stretch(bars, from, to);
    let lift = (from >= BEFORE_BARS).then(|| {
        let before = bars[from - BEFORE_BARS..from].iter().sum::<f32>() / BEFORE_BARS as f32;
        run.energy / before.max(f32::MIN_POSITIVE)
    });
    Run { lift, ..run }
}

/// How much of each drop's kick the bars before it kept: see
/// [`Run::kept_kick`].
fn with_kicks(shape: Shape, kicks: &[f32]) -> Shape {
    let mean = |from: usize, to: usize| {
        let to = to.min(kicks.len());
        (to > from).then(|| kicks[from..to].iter().sum::<f32>() / (to - from) as f32)
    };
    let drops = shape
        .drops
        .into_iter()
        .map(|run| {
            let kept_kick = (run.from >= BEFORE_BARS)
                .then(|| {
                    let before = mean(run.from - BEFORE_BARS, run.from)?;
                    let inside = mean(run.from, run.to.min(run.from + BEFORE_BARS))?;
                    Some(before / inside.max(f32::MIN_POSITIVE))
                })
                .flatten();
            Run { kept_kick, ..run }
        })
        .collect();
    Shape { drops, ..shape }
}

/// How far up the drum stem's own range a bar has to come for its drums to
/// count as playing: a quarter of the way from its quiet bars to its loud.
///
/// Low, because the question is whether the drums are there at all. A fill, a
/// bar of hats or a kick played half as hard is still drums; a breakdown is
/// the stem near silence.
const DRUMS_PLAYING: f32 = 0.25;

/// The share of a record's bars its drums have to play in before the drum
/// stem is let say where the breaks are. Half.
///
/// A record whose drums come and go in bursts has no groove for them to drop
/// out of. On three experimental records from archive.org the drums played in
/// 17, 26 and 64 per cent of the bars, and in the first two the gaps between
/// bursts read as breaks that are nothing of the kind. On four club records
/// they played in 56 to 87 per cent.
const DRUMS_MOSTLY: f32 = 0.5;

/// A record's drops and breaks, with its drum stem saying where the drums drop
/// out.
///
/// `mix` is [`bars`] of the record and `drums` of its drum stem, on the same
/// grid. The mix finds the drops and breaks as [`shape`] does, and decides
/// which is the main one. The drum stem adds the breaks the mix cannot see:
/// eight bars or more with the drums out, between bars with drums in. Measured
/// from the mix, a bassline plucked through a breakdown rises in the low band
/// the way a kick does, and a breakdown of loud synths is as loud as a drop, so
/// either reads as part of the drop around it; in the stem, only the drums can
/// make a bar sound like drums. A drop the mix found is cut where such a break
/// runs through it.
///
/// Only the breaks come from the stem. The drums of an intro are often the
/// drop's loop, so a drop found from the stem can begin at bar one, and the
/// drums of two drops are often the same, so they cannot say which is bigger.
///
/// A record whose drums play in less than half its bars has no groove to drop
/// out of, and its drum stem is not asked: the shape is the mix's.
pub fn shape_with_drums(mix: &[f32], drums: &[f32]) -> Shape {
    let from_mix = shape(mix);
    let out = drums_out(&drums[..drums.len().min(mix.len())]);
    if out.is_empty() {
        return from_mix;
    }
    let mut drops = Vec::new();
    for drop in &from_mix.drops {
        let mut pieces = vec![(drop.from, drop.to)];
        for &(from, to) in &out {
            pieces = pieces
                .into_iter()
                .flat_map(|(start, end)| match to <= start || from >= end {
                    true => vec![(start, end)],
                    false => vec![(start, from.max(start)), (to.min(end), end)],
                })
                .collect();
        }
        drops.extend(
            pieces
                .into_iter()
                .filter(|(from, to)| to - from >= RUN_BARS)
                .map(|(f, t)| as_drop(mix, f, t)),
        );
    }

    // Both kinds of break, where they overlap as one: a stretch the mix had as
    // a break and the drums dropping out inside it are the same breakdown.
    let mut spans: Vec<(usize, usize)> =
        from_mix.breaks.iter().map(|b| (b.from, b.to)).chain(out).collect();
    spans.sort_unstable();
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (from, to) in spans {
        match merged.last_mut() {
            Some(last) if from < last.1 => last.1 = last.1.max(to),
            _ => merged.push((from, to)),
        }
    }
    let breaks = merged.into_iter().map(|(f, t)| stretch(mix, f, t)).collect();
    Shape { drops, breaks, ..from_mix }
}

/// A record's drops and breaks from its picture, and from its drum stem's
/// where it has one. `grid` is every beat from the first downbeat.
pub fn read(bands: &[u8], drums: Option<&[u8]>, grid: &[u32]) -> Shape {
    let (mix, kicks): (Vec<f32>, Vec<f32>) = measured(bands, grid).into_iter().unzip();
    let shape = match drums {
        Some(drums) => shape_with_drums(&mix, &bars(drums, grid)),
        None => shape(&mix),
    };
    with_kicks(shape, &kicks)
}

/// Where a drum stem's drums are out for [`RUN_BARS`] or more, between bars
/// where they play: never the intro before they first come in or the outro
/// after they last stop. Nothing, for a record whose drums do not play in
/// most of its bars.
fn drums_out(drums: &[f32]) -> Vec<(usize, usize)> {
    if drums.len() < RUN_BARS * 2 {
        return Vec::new();
    }
    let mut sorted = drums.to_vec();
    sorted.sort_by(f32::total_cmp);
    let at = |quantile: f32| sorted[((sorted.len() - 1) as f32 * quantile).round() as usize];
    let (quiet, loud) = (at(QUIET_QUANTILE), at(LOUD_QUANTILE));
    if loud <= quiet {
        return Vec::new();
    }
    let playing: Vec<bool> =
        drums.iter().map(|&bar| bar >= quiet + DRUMS_PLAYING * (loud - quiet)).collect();
    let count = playing.iter().filter(|&&on| on).count();
    if (count as f32) < DRUMS_MOSTLY * drums.len() as f32 {
        return Vec::new();
    }
    let (Some(first), Some(last)) =
        (playing.iter().position(|&on| on), playing.iter().rposition(|&on| on))
    else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut bar = first;
    while bar < last {
        if playing[bar] {
            bar += 1;
            continue;
        }
        let from = bar;
        while !playing[bar] {
            bar += 1;
        }
        if bar - from >= RUN_BARS {
            out.push((from, bar));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Bars at these levels, `count` of each, in order.
    fn record(parts: &[(f32, usize)]) -> Vec<f32> {
        parts.iter().flat_map(|&(level, count)| std::iter::repeat_n(level, count)).collect()
    }

    /// A genre, as the table files it, for a test.
    fn genre(id: &'static str, family: &'static str, sure: f32) -> super::super::tempo::Genre {
        super::super::tempo::Genre { id, name: id, family, from_bpm: 0.0, to_bpm: 0.0, sure }
    }

    /// Intro 32, drop 32, break 16, drop 32 a little louder, outro 16: drum
    /// and bass as it is usually built, with the drop after a long intro.
    fn after_a_long_intro() -> Vec<f32> {
        record(&[(20.0, 32), (80.0, 32), (25.0, 16), (90.0, 32), (30.0, 16)])
    }

    #[test]
    fn a_drum_and_bass_record_has_its_main_drop_before_halfway_and_house_after_its_first_third() {
        let shape = shape(&after_a_long_intro());
        assert_eq!(shape.main_drop().map(|run| run.from), Some(80), "with no genre");
        let dnb = shape.clone().arranged_as(Arranged::of(&genre("drum-and-bass", "dnb", 1.0)));
        assert_eq!(dnb.choose().0.map(|run| run.from), Some(32));
        assert_eq!(dnb.choose().1, Chosen::Biggest);
        let house = shape.arranged_as(Arranged::of(&genre("house", "house", 1.0)));
        assert_eq!(house.choose(), (house.after_the_breakdown(), Chosen::Biggest));
    }

    #[test]
    fn of_the_drops_before_halfway_drum_and_bass_cues_the_biggest_not_the_first() {
        // Intro, a drop that climbs three times out of it, a short break, a
        // drop that climbs four and a half, the breakdown, the drop again.
        let bars = |second| {
            record(&[
                (20.0, 16),
                (60.0, 16),
                (20.0, 8),
                (second, 24),
                (25.0, 16),
                (90.0, 32),
                (20.0, 16),
            ])
        };
        let dnb = Arranged::of(&genre("drum-and-bass", "dnb", 1.0));
        let shape = shape(&bars(90.0)).arranged_as(dnb);
        assert_eq!(shape.choose().0.map(|run| run.from), Some(40));
        // Climbing 3.2 times is within a tenth of the first's 3.0, so as big,
        // and of two as big drum and bass cues the earlier.
        let shape = super::shape(&bars(64.0)).arranged_as(dnb);
        assert_eq!(shape.choose().0.map(|run| run.from), Some(16));
    }

    #[test]
    fn of_the_drops_after_its_first_third_house_cues_the_biggest_not_the_one_after_its_breakdown() {
        // Intro, drop, a 32-bar breakdown ending in a build that leaves little
        // for the drop after it to climb over, then a short break with
        // everything out and the drop that climbs furthest out of it.
        let bars = record(&[
            (20.0, 8),
            (85.0, 24),
            (20.0, 24),
            (50.0, 8),
            (85.0, 32),
            (40.0, 8),
            (90.0, 32),
            (20.0, 8),
        ]);
        let shape = shape(&bars);
        assert_eq!(shape.main_break().map(|run| (run.from, run.to)), Some((32, 64)));
        assert_eq!(shape.after_the_breakdown().map(|run| run.from), Some(64));
        assert_eq!(shape.choose().0.map(|run| run.from), Some(104));
    }

    #[test]
    fn a_record_with_no_breakdown_has_its_main_drop_where_it_climbs_furthest_after_its_first_third()
    {
        // No break of eight bars, so before kinds were told apart the main
        // drop was the loudest: the second, which comes out of a dip it barely
        // climbs over. The third comes out of a dip it climbs well out of, and
        // the first, out of the intro, is the groove setting out.
        let bars = record(&[
            (10.0, 8),
            (80.0, 16),
            (45.0, 4),
            (88.0, 40),
            (20.0, 4),
            (84.0, 60),
            (10.0, 8),
        ]);
        let shape = shape(&bars);
        assert!(shape.breaks.is_empty());
        assert_eq!(shape.after_the_breakdown().map(|run| run.from), Some(28), "the loudest");
        assert_eq!(shape.choose(), (shape.drops.get(2).copied(), Chosen::Biggest));
        assert_eq!(shape.drops[2].from, 72);
    }

    #[test]
    fn a_record_whose_drops_never_climb_far_enough_keeps_its_main_drop_as_it_was() {
        // A record that plays about as hard all the way through, as a
        // hardcore one with its kick going under every break does.
        let bars = record(&[(60.0, 16), (80.0, 40), (62.0, 16), (82.0, 40), (60.0, 16)]);
        let shape = shape(&bars);
        assert!(shape.drops.iter().all(|run| run.lift.is_some_and(|lift| lift < 1.5)));
        assert_eq!(shape.choose(), (shape.after_the_breakdown(), Chosen::AsBefore));
    }

    #[test]
    fn a_trance_record_has_its_main_drop_after_the_drums_drop_out_though_another_climbs_further() {
        // Intro, drop, a long break with the kick still going and the bass
        // out, drop, a short break with the drums out but more left above
        // them, drop. Without a genre the drop out of the long break is the
        // biggest; a trance breakdown is where the drums go.
        let bars = record(&[
            (20.0, 16),
            (80.0, 32),
            (30.0, 16),
            (85.0, 32),
            (40.0, 8),
            (85.0, 32),
            (20.0, 16),
        ]);
        let kicks =
            record(&[(1.0, 16), (1.0, 32), (0.9, 16), (1.0, 32), (0.1, 8), (1.0, 32), (1.0, 16)]);
        let shape = with_kicks(shape(&bars), &kicks);
        assert_eq!(shape.main_drop().map(|run| run.from), Some(64));
        let trance = shape.arranged_as(Arranged::of(&genre("trance", "trance", 1.0)));
        assert_eq!(trance.choose(), (trance.drops.get(2).copied(), Chosen::Biggest));
        assert_eq!(trance.drops[2].from, 104);
        assert!(trance.drops[2].kept_kick.is_some_and(|kept| kept <= DRUMS_OUT));
    }

    #[test]
    fn a_drop_with_too_few_bars_before_it_has_nothing_to_climb_out_of() {
        let shape = shape(&record(&[(10.0, 4), (80.0, 32), (20.0, 16), (85.0, 32), (10.0, 8)]));
        assert_eq!(shape.drops[0].from, 4);
        assert_eq!(shape.drops[0].lift, None);
        assert!(shape.drops[1].lift.is_some_and(|lift| lift > 4.0));
        assert_eq!(shape.breaks[0].lift, None, "a break does not climb");
    }

    #[test]
    fn the_surest_genre_says_how_a_record_is_arranged_and_the_first_of_two_as_sure() {
        let (house, jungle) = (genre("house", "house", 0.4), genre("jungle", "dnb", 0.7));
        assert_eq!(Arranged::of_genres(&[]), Arranged::UNKNOWN);
        assert_eq!(Arranged::of_genres(&[house, jungle]).half, Half::First);
        let (house, jungle) = (genre("house", "house", 1.0), genre("jungle", "dnb", 1.0));
        assert_eq!(Arranged::of_genres(&[jungle, house]).genre, "jungle");
        assert_eq!(Arranged::of_genres(&[house, jungle]).genre, "house");
    }

    #[test]
    fn dubstep_cues_a_drop_before_halfway_and_breakbeat_filed_beside_it_one_after_its_first_third()
    {
        assert_eq!(Arranged::of(&genre("dubstep", "bass", 1.0)).half, Half::First);
        assert_eq!(Arranged::of(&genre("breakbeat", "bass", 1.0)).half, Half::Later);
        assert!(Arranged::of(&genre("trance", "trance", 1.0)).drums_out);
    }

    #[test]
    fn a_record_with_two_drops_and_a_break_between_has_the_drop_after_it_as_its_main_one() {
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
    fn of_drops_as_hard_as_each_other_the_one_after_the_breakdown_is_d() {
        // Drums from the first bar, as a DJ-friendly intro has them, and every
        // drop mastered to the same level: the most energy would be the intro.
        let bars = record(&[(90.0, 64), (88.0, 8), (90.0, 72), (30.0, 48), (90.0, 56)]);
        let shape = shape(&bars);
        assert_eq!(shape.main_break().map(|run| run.from), Some(144), "{shape:?}");
        assert_eq!(shape.main_drop().map(|run| run.from), Some(192), "{shape:?}");
    }

    #[test]
    fn a_drop_a_little_louder_before_the_breakdown_is_not_d() {
        let bars = record(&[(20.0, 16), (95.0, 32), (20.0, 32), (92.0, 32), (20.0, 16)]);
        assert_eq!(shape(&bars).main_drop().map(|run| run.from), Some(80));
    }

    #[test]
    fn a_break_as_loud_as_a_drop_is_no_breakdown_and_d_is_the_loudest_drop() {
        // Read off the mix alone a break is quieter than every drop, since
        // its bars are all below the line the drops' are above. Read with a
        // drum stem it can be a stretch with the drums out and the synths
        // loud: as on a record whose drop after such a stretch began with its
        // drums barely in.
        let run = |from, to, energy| Run { from, to, energy, lift: None, kept_kick: None };
        let shape = Shape {
            drops: vec![run(34, 56, 0.78), run(61, 96, 0.99), run(104, 150, 0.91)],
            breaks: vec![run(96, 104, 0.86)],
            ..Shape::default()
        };
        assert_eq!(shape.main_drop().map(|run| run.from), Some(61));
    }

    #[test]
    fn a_drop_that_climbs_furthest_but_plays_softer_than_the_record_on_average_is_not_d() {
        // Intro, drop, a short break that keeps something going, a long loud
        // drop out of it, a break of near-silence, and a soft stretch that
        // climbs a long way out of the silence and is still quieter than the
        // record as a whole, then the outro.
        let bars = record(&[
            (10.0, 8),
            (90.0, 24),
            (40.0, 8),
            (90.0, 40),
            (10.0, 8),
            (52.0, 16),
            (10.0, 8),
        ]);
        let shape = shape(&bars);
        let (loud, soft) = (shape.drops[1], shape.drops[2]);
        assert_eq!((loud.from, soft.from), (40, 88));
        assert!(soft.lift > loud.lift, "{soft:?} climbs further than {loud:?}");
        assert!(soft.energy < shape.average, "{soft:?} against {}", shape.average);
        assert_eq!(shape.choose(), (Some(loud), Chosen::Biggest));
        assert_eq!(shape.after_the_breakdown(), Some(loud), "nor is it D as before");
    }

    #[test]
    fn after_the_breakdown_a_stretch_softer_than_the_record_on_average_is_passed_over() {
        // As on Get Free: eight bars at 0.83 of the record's average straight
        // after the breakdown, and the drop proper after them.
        let run = |from, to, energy| Run { from, to, energy, lift: None, kept_kick: None };
        let shape = Shape {
            drops: vec![run(32, 64, 0.9), run(80, 88, 0.5), run(92, 124, 0.7)],
            breaks: vec![run(64, 80, 0.3)],
            bars: 140,
            average: 0.6,
            ..Shape::default()
        };
        assert_eq!(shape.after_the_breakdown().map(|run| run.from), Some(92));
    }

    #[test]
    fn a_record_with_no_break_has_its_loudest_drop_as_d() {
        let bars = record(&[(20.0, 16), (80.0, 32), (60.0, 4), (90.0, 32), (20.0, 16)]);
        let shape = shape(&bars);
        assert!(shape.breaks.is_empty(), "{shape:?}");
        assert_eq!(shape.main_drop().map(|run| run.from), Some(16), "{shape:?}");
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
    fn of_two_breaks_the_longer_is_the_main_one() {
        let bars = record(&[(80.0, 16), (40.0, 8), (80.0, 16), (15.0, 16), (80.0, 16)]);
        let shape = shape(&bars);
        assert_eq!(shape.breaks.len(), 2, "{shape:?}");
        assert_eq!(shape.main_break().map(|run| run.from), Some(40));
    }

    #[test]
    fn a_long_break_later_on_is_the_main_one_over_a_short_deeper_one_near_the_top() {
        let bars = record(&[(20.0, 8), (80.0, 16), (5.0, 8), (80.0, 32), (30.0, 24), (85.0, 32)]);
        let shape = shape(&bars);
        assert_eq!(shape.breaks.len(), 2, "{shape:?}");
        assert!(shape.breaks[0].energy < shape.breaks[1].energy, "the early one is not deeper");
        assert_eq!(shape.main_break().map(|run| run.from), Some(64), "{shape:?}");
    }

    #[test]
    fn of_two_breaks_about_as_long_the_later_is_the_main_one() {
        // Ten bars and nine: the same length, as edges are measured.
        let bars = record(&[(80.0, 16), (5.0, 10), (80.0, 32), (30.0, 9), (85.0, 32)]);
        let shape = shape(&bars);
        assert_eq!(shape.breaks.len(), 2, "{shape:?}");
        assert_eq!(shape.main_break().map(|run| run.from), Some(58), "{shape:?}");
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
        made(&ARRANGED, Keep::All)
    }

    /// Which instruments of a made record to write: all of them for the mix,
    /// or only the drums, for the stem a perfect separator would give.
    #[derive(Clone, Copy, PartialEq)]
    enum Keep {
        All,
        Drums,
    }

    /// (bars, kick and bass, hats, pad, lead, rolling bass)
    type Part = (usize, bool, bool, bool, bool, bool);

    const ARRANGED: [Part; 5] = [
        (16, false, true, false, false, false),
        (32, true, true, false, false, false),
        (16, false, false, true, false, false),
        (32, true, true, false, true, false),
        (16, false, true, false, false, false),
    ];

    fn made(parts: &[Part], keep: Keep) -> crate::audio::Audio {
        const RATE: u32 = 22_050;
        let bar = 2.0f32;
        let total: usize = parts.iter().map(|part| part.0).sum();
        let mut plane = vec![0.0f32; (total as f32 * bar * RATE as f32) as usize];
        let mut start = 0usize;
        let tau = 2.0 * std::f32::consts::PI;
        for &(bars, low, hats, pad, lead, rolling) in parts {
            let drums = keep == Keep::Drums;
            let from = (start as f32 * bar * RATE as f32) as usize;
            let to = ((start + bars) as f32 * bar * RATE as f32) as usize;
            for (i, sample) in plane.iter_mut().enumerate().take(to).skip(from) {
                let t = i as f32 / RATE as f32;
                let in_beat = t % 0.5;
                let mut x = 0.0;
                if low {
                    x += 0.6 * (tau * 55.0 * t).sin() * (-in_beat * 12.0).exp();
                    if !drums {
                        x += 0.3 * (tau * 41.2 * t).sin();
                    }
                }
                // A bassline plucked every sixteenth, with no kick under it:
                // the low band rising four times a beat.
                if rolling && !drums {
                    let in_sixteenth = t % 0.125;
                    x += 0.5 * (tau * 49.0 * t).sin() * (-in_sixteenth * 30.0).exp();
                }
                if hats {
                    let off = (t + 0.25) % 0.5;
                    x += 0.15
                        * ((i as f32 * 12.9898).sin() * 43_758.545).fract()
                        * (-off * 60.0).exp();
                }
                if pad && !drums {
                    x += 0.2 * (tau * 220.0 * t).sin() + 0.15 * (tau * 277.2 * t).sin();
                }
                if lead && !drums {
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
        // the drop after the break starts at bar 64 and the break at 48, and the
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

    /// Bars from a made record, on its two-second grid.
    fn measured(parts: &[Part], keep: Keep) -> Vec<f32> {
        let total: u32 = parts.iter().map(|part| part.0 as u32).sum();
        let grid: Vec<u32> = (0..total * 4).map(|beat| beat * 500).collect();
        bars(&crate::export::waveform::analyze(&made(parts, keep)).band_detail, &grid)
    }

    fn spans(runs: &[Run]) -> Vec<(usize, usize)> {
        runs.iter().map(|run| (run.from, run.to)).collect()
    }

    #[test]
    fn a_bassline_rolling_through_a_break_hides_it_from_the_mix_but_not_from_the_drum_stem() {
        // The made record again, with a bass plucked every sixteenth under the
        // pad in its break and no kick: the low band rising four times a beat.
        let rolling: [Part; 5] = [
            (16, false, true, false, false, false),
            (32, true, true, false, false, false),
            (16, false, false, true, false, true),
            (32, true, true, false, true, false),
            (16, false, true, false, false, false),
        ];
        let mix = measured(&rolling, Keep::All);
        let drums = measured(&rolling, Keep::Drums);

        // What this is here for: read from the mix, the break is part of the
        // main drop, which runs straight through it.
        let from_mix = shape(&mix);
        assert!(from_mix.breaks.is_empty(), "the mix saw the break after all: {from_mix:?}");
        assert!(from_mix.main_drop().unwrap().holds(56), "{from_mix:?}");

        let with_drums = shape_with_drums(&mix, &drums);
        assert_eq!(spans(&with_drums.breaks), [(48, 64)], "{with_drums:?}");
        assert_eq!(with_drums.main_break().map(|run| run.from), Some(48));
        assert_eq!(with_drums.main_drop().map(|run| run.from), Some(64), "{with_drums:?}");
    }

    #[test]
    fn a_record_the_mix_already_reads_is_read_the_same_with_its_drum_stem() {
        let mix = measured(&ARRANGED, Keep::All);
        let drums = measured(&ARRANGED, Keep::Drums);
        let (from_mix, with_drums) = (shape(&mix), shape_with_drums(&mix, &drums));
        assert_eq!(spans(&with_drums.drops), spans(&from_mix.drops));
        assert_eq!(spans(&with_drums.breaks), spans(&from_mix.breaks));
        assert_eq!(with_drums.main_drop(), from_mix.main_drop());
    }

    #[test]
    fn the_mix_still_says_which_drop_is_the_main_one() {
        // The same drum loop in both drops, so the stem cannot tell them
        // apart; the second is louder in the mix.
        let mix = record(&[(20.0, 16), (60.0, 32), (20.0, 16), (90.0, 32), (20.0, 16)]);
        let drums = record(&[(0.0, 16), (1.0, 32), (0.0, 16), (1.0, 32), (0.0, 16)]);
        assert_eq!(shape_with_drums(&mix, &drums).main_drop().map(|run| run.from), Some(64));
    }

    #[test]
    fn a_drop_the_mix_found_is_cut_where_the_drums_drop_out_inside_it() {
        // A breakdown as loud as the drops either side, which the mix reads as
        // one long drop.
        let mix = record(&[(20.0, 16), (80.0, 32), (75.0, 16), (90.0, 32), (20.0, 16)]);
        let drums = record(&[(0.0, 16), (1.0, 32), (0.0, 16), (1.0, 32), (0.0, 16)]);
        assert_eq!(spans(&shape(&mix).drops), [(16, 96)], "the mix saw the break after all");

        let with_drums = shape_with_drums(&mix, &drums);
        assert_eq!(spans(&with_drums.drops), [(16, 48), (64, 96)]);
        assert_eq!(spans(&with_drums.breaks), [(48, 64)]);
    }

    #[test]
    fn before_the_drums_first_come_in_is_an_intro_and_after_they_stop_an_outro() {
        let drums = record(&[(0.0, 16), (1.0, 32), (0.0, 16), (1.0, 32), (0.0, 16)]);
        assert_eq!(drums_out(&drums), [(48, 64)]);
    }

    #[test]
    fn a_record_whose_drums_come_in_bursts_keeps_the_mixs_shape() {
        // Four bars of drums every sixteen: a quarter of the record, with
        // twelve-bar gaps that would otherwise each be a break.
        let drums: Vec<f32> = (0..128).map(|bar| if bar % 16 < 4 { 1.0 } else { 0.0 }).collect();
        let mix = record(&[(20.0, 16), (80.0, 48), (25.0, 16), (90.0, 32), (30.0, 16)]);
        assert!(drums_out(&drums).is_empty());
        assert_eq!(shape_with_drums(&mix, &drums), shape(&mix));
    }

    #[test]
    fn a_dip_in_the_drums_shorter_than_a_phrase_is_not_a_break() {
        let drums = record(&[(1.0, 32), (0.0, 7), (1.0, 32)]);
        assert!(drums_out(&drums).is_empty());
    }

    /// The comparison behind reading breaks off the drum stem, on real
    /// records: for each `<name>.mp3` in `BOOTH_DRUM_RECORDS`, its stem at
    /// `BOOTH_DRUM_STEMS/<name>/drums.wav` (the layout demucs writes), the
    /// drops and breaks read from the mix alone and with the drum stem.
    ///
    /// ```sh
    /// demucs -n htdemucs --out <stems> <records>/*.mp3
    /// BOOTH_DRUM_RECORDS=<records> BOOTH_DRUM_STEMS=<stems>/htdemucs \
    ///     cargo test -p booth-cli --release --lib with_the_drum_stem -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore = "needs records and their drum stems: BOOTH_DRUM_RECORDS, BOOTH_DRUM_STEMS"]
    fn real_records_read_with_the_drum_stem() {
        use crate::audio::decode::decode_file;
        let records = std::path::PathBuf::from(std::env::var("BOOTH_DRUM_RECORDS").unwrap());
        let stems = std::path::PathBuf::from(std::env::var("BOOTH_DRUM_STEMS").unwrap());
        let mut names: Vec<std::path::PathBuf> =
            std::fs::read_dir(&records).unwrap().map(|entry| entry.unwrap().path()).collect();
        names.sort();
        let said = |shape: &Shape| {
            format!(
                "drops {:?} breaks {:?} D {:?} B {:?}",
                spans(&shape.drops),
                spans(&shape.breaks),
                shape.main_drop().map(|run| run.from),
                shape.main_break().map(|run| run.from)
            )
        };
        for path in names {
            let name = path.file_stem().unwrap().to_string_lossy().to_string();
            let Ok(drums) = decode_file(&stems.join(&name).join("drums.wav")) else { continue };
            let audio = decode_file(&path).unwrap();
            let bands = crate::export::waveform::analyze(&audio).band_detail;
            let analysed = crate::analysis::analyze_drawn(&audio, None, &bands);
            let times: Vec<u32> = analysed.grid.beats.iter().map(|beat| beat.time_ms).collect();
            let phase = analysed.grid.beats.iter().position(|beat| beat.number == 1).unwrap_or(0);
            let grid = &times[phase..];
            let mix = bars(&bands, grid);
            let drum_bars = bars(&crate::export::waveform::analyze(&drums).band_detail, grid);
            eprintln!("{name} ({:.0} BPM, {} bars)", analysed.bpm, mix.len());
            eprintln!("  mix:        {}", said(&shape(&mix)));
            eprintln!("  with drums: {}", said(&shape_with_drums(&mix, &drum_bars)));
        }
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

    /// The comparison behind choosing the main drop by the kind of record, on
    /// real records: for each in `BOOTH_D_RECORDS`, every drop with how far it
    /// climbs over the eight bars before it and how much of its kick those
    /// kept, and the main drop as it was chosen before kinds were told apart
    /// and as it is now. With `BOOTH_GENRE_MODEL`, each record is heard first
    /// and placed by the genre it is heard as; without, by no genre at all.
    ///
    /// ```sh
    /// BOOTH_D_RECORDS=<a folder of records> BOOTH_GENRE_MODEL=<model.onnx> \
    ///     cargo test -p booth-cli --release --lib d_by_each_rule -- --ignored --nocapture
    /// ```
    ///
    /// On the 26 records from archive.org behind the downbeat and six more
    /// for drum and bass, dubstep and hardcore, heard by the model, six of 32
    /// moved. I Need House went to the drop at 2:27 that the downbeat's
    /// reference heard as everything coming in, off one at 0:46 a hair louder.
    /// The two drum and bass records went to the drop after their intro,
    /// Get Free from 4:00 to 0:43 and Swords from 4:00 to 1:36. And three went
    /// off the drop after their longest break to a bigger one: Brain Dance
    /// from 2:50 to 4:54, Blunted My Transistor from 4:21 to 2:33, and Nebula
    /// from 2:39 to 1:15. Without the model, five: the same less Swords, and
    /// Get Free to its last drop, at 5:05, rather than its first.
    ///
    /// Requiring D to play harder than the record's average moved none of
    /// those. It changed D as before on one record, Get Free, from the
    /// eight bars at 4:00 to the drop at 4:16, and the biggest drop overrules
    /// that there either way.
    #[test]
    #[ignore = "needs records: BOOTH_D_RECORDS"]
    fn real_records_d_by_each_rule() {
        let records = std::path::PathBuf::from(std::env::var("BOOTH_D_RECORDS").unwrap());
        let classifier = super::super::style::locate(None)
            .filter(|_| std::env::var_os("BOOTH_GENRE_MODEL").is_some())
            .map(|model| super::super::style::Classifier::load(&model).unwrap());
        let mut paths: Vec<std::path::PathBuf> =
            std::fs::read_dir(&records).unwrap().map(|entry| entry.unwrap().path()).collect();
        paths.sort();
        let (mut differ, mut total) = (0, 0);
        for path in paths {
            let audio = crate::audio::decode::decode_file(&path).unwrap();
            let bands = crate::export::waveform::analyze(&audio).band_detail;
            let analysed = crate::analysis::analyze_drawn(&audio, None, &bands);
            let times: Vec<u32> = analysed.grid.beats.iter().map(|beat| beat.time_ms).collect();
            let phase = analysed.grid.beats.iter().position(|beat| beat.number == 1).unwrap_or(0);
            let grid = &times[phase..];
            let heard = classifier
                .as_ref()
                .map(|model| super::super::genre_tempo::heard(&model.classify(&audio).unwrap()))
                .unwrap_or_default();
            let shape = read(&bands, None, grid).arranged_as(Arranged::of_genres(&heard));
            let at = |run: &Run| {
                let s = grid.get(run.from * 4).copied().unwrap_or(0) / 1000;
                format!("bar {} ({}:{:02})", run.from + 1, s / 60, s % 60)
            };
            let said = |run: Option<Run>| run.as_ref().map_or("none".to_string(), at);
            let (before, (now, how)) = (shape.after_the_breakdown(), shape.choose());
            total += 1;
            differ += usize::from(before != now);
            eprintln!(
                "{} [{}]: before {}; now {} ({how:?}){}",
                path.file_stem().unwrap().to_string_lossy(),
                shape.arranged.genre,
                said(before),
                said(now),
                if before != now { "  <- differ" } else { "" }
            );
            for run in &shape.drops {
                let number =
                    |value: Option<f32>| value.map_or("-".to_string(), |v| format!("{v:.2}"));
                eprintln!(
                    "    drop {} for {} bars: lift {}, kick kept {}",
                    at(run),
                    run.bars(),
                    number(run.lift),
                    number(run.kept_kick)
                );
            }
        }
        eprintln!("{differ} of {total} differ");
    }

    /// The comparison behind choosing the main break by length rather than by
    /// depth, on real records: for each record in `BOOTH_DRUM_RECORDS`, read
    /// with its drum stem from `BOOTH_DRUM_STEMS/<name>/drums.wav` where there
    /// is one, the breaks, the deepest, and the one now taken, with the main
    /// drop that answers each.
    ///
    /// ```sh
    /// BOOTH_DRUM_RECORDS=<records> BOOTH_DRUM_STEMS=<demucs out>/htdemucs \
    ///     cargo test -p booth-cli --release --lib b_by_each_rule -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore = "needs records: BOOTH_DRUM_RECORDS, and their stems in BOOTH_DRUM_STEMS"]
    fn real_records_b_by_each_rule() {
        use crate::audio::decode::decode_file;
        let records = std::path::PathBuf::from(std::env::var("BOOTH_DRUM_RECORDS").unwrap());
        let stems = std::path::PathBuf::from(std::env::var("BOOTH_DRUM_STEMS").unwrap_or_default());
        let mut paths: Vec<std::path::PathBuf> =
            std::fs::read_dir(&records).unwrap().map(|entry| entry.unwrap().path()).collect();
        paths.sort();
        let (mut differ, mut total) = (0, 0);
        for path in paths {
            let name = path.file_stem().unwrap().to_string_lossy().to_string();
            let audio = decode_file(&path).unwrap();
            let bands = crate::export::waveform::analyze(&audio).band_detail;
            let analysed = crate::analysis::analyze_drawn(&audio, None, &bands);
            let times: Vec<u32> = analysed.grid.beats.iter().map(|beat| beat.time_ms).collect();
            let phase = analysed.grid.beats.iter().position(|beat| beat.number == 1).unwrap_or(0);
            let grid = &times[phase..];
            let drums = decode_file(&stems.join(&name).join("drums.wav"))
                .ok()
                .map(|stem| crate::export::waveform::analyze(&stem).band_detail);
            let shape = read(&bands, drums.as_deref(), grid);
            let deepest = shape.breaks.iter().copied().reduce(|best, run| {
                match run.energy < best.energy
                    || (run.energy == best.energy && run.bars() > best.bars())
                {
                    true => run,
                    false => best,
                }
            });
            let answering = |quiet: Option<Run>| {
                quiet
                    .filter(|quiet| shape.drops.iter().all(|drop| quiet.energy < drop.energy))
                    .and_then(|quiet| {
                        shape.drops.iter().find(|drop| drop.from >= quiet.from).copied()
                    })
                    .or_else(|| shape.loudest_drop())
            };
            let at = |run: Option<Run>| match run {
                Some(run) => {
                    let s = grid.get(run.from * 4).copied().unwrap_or(0) / 1000;
                    format!("{}-{} ({}:{:02})", run.from, run.to, s / 60, s % 60)
                }
                None => "none".to_string(),
            };
            let now = shape.main_break();
            // Off the mix alone, as the analysis reads it, and where that finds
            // no break the arrangement's sections named a break: the first of
            // them, which B used to fall back to, and the one it falls back to
            // now.
            let mix_only = read(&bands, None, grid);
            if mix_only.breaks.is_empty() {
                let named: Vec<(u32, u16)> = analysed
                    .structure
                    .sections
                    .iter()
                    .filter(|section| section.kind == super::super::structure::Kind::Down)
                    .filter_map(|section| {
                        let at = times.get(section.start_beat as usize - 1)?;
                        Some((*at, section.end_beat.saturating_sub(section.start_beat) / 4))
                    })
                    .collect();
                let b = analysed
                    .cues
                    .iter()
                    .find(|cue| cue.is_hot() && cue.hot_cue == 2)
                    .map(|cue| cue.time_ms);
                let clock = |ms: u32| format!("{}:{:02}", ms / 60_000, ms / 1000 % 60);
                eprintln!(
                    "  mix alone finds no break; sections named a break {:?}; first {}; B now {}",
                    named
                        .iter()
                        .map(|(at, bars)| format!("{} ({bars} bars)", clock(*at)))
                        .collect::<Vec<_>>(),
                    named.first().map_or("none".into(), |(at, bars)| format!(
                        "{} ({bars} bars)",
                        clock(*at)
                    )),
                    b.map_or("empty".into(), clock)
                );
            }
            total += 1;
            differ += usize::from(now != deepest);
            eprintln!(
                "{name}{}: breaks {:?}\n  deepest {} -> D {}\n  now     {} -> D {}{}",
                if drums.is_some() { " (with drums)" } else { "" },
                shape.breaks.iter().map(|run| (run.from, run.to)).collect::<Vec<_>>(),
                at(deepest),
                at(answering(deepest)),
                at(now),
                at(shape.main_drop()),
                if now != deepest { "  <- differ" } else { "" }
            );
        }
        eprintln!("{differ} of {total} differ");
    }
}
