//! Working out how a track is put together.
//!
//! The method is Foote's: describe each bar by where its energy sits, compare
//! every bar with every other, and look for the moments where the music stops
//! resembling what came before and starts resembling what comes after. Those
//! moments are the phrase boundaries a CDJ-3000 draws under its waveform.
//!
//! Labelling them is cruder, and deliberately so. Rather than guess at verses
//! and choruses, the labels are the ones a DJ uses about a record — intro,
//! build, drop, break, outro — decided from how loud each section is and which
//! way it is heading. Those map onto the phrase types rekordbox calls a "high
//! mood" track, which is the vocabulary the format offers that fits dance music.

use crate::export::{Mood, Phrase, SongStructure};

use super::features::{Features, BANDS};

/// Bars either side of a candidate boundary that the comparison looks at. Eight
/// is a phrase in most dance music, so a boundary is a point where one
/// eight-bar stretch stops resembling the next.
const KERNEL_BARS: usize = 8;
/// The shortest section worth marking. Anything less is a fill, not a phrase.
const MIN_SECTION_BARS: usize = 8;
/// How much a band has to move across the track, in log energy, before the
/// movement counts as arrangement rather than as noise. One unit is about four
/// decibels; a track that stays inside that is a loop, not an arrangement.
const MIN_VARIATION: f32 = 1.0;
/// How far above the run of the novelty curve a peak has to stand before it is
/// called a boundary, in standard deviations. Some bar is always the most
/// novel; this is what stops that being enough.
const PEAK_PROMINENCE: f32 = 1.0;
/// Boundaries are snapped to this many bars, because arrangements are built in
/// fours and a phrase that starts three and a half bars in is a mistake.
const SNAP_BARS: usize = 4;
const BEATS_PER_BAR: usize = 4;

/// What a section of a track is doing.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    Intro,
    /// Energy on the way up: a build.
    Up,
    /// Energy dropped away: a breakdown.
    Down,
    /// The loud part. rekordbox calls it a chorus; a DJ calls it the drop.
    Chorus,
    Outro,
}

impl Kind {
    /// The value the format uses for this phrase in a high-mood track.
    fn id(self) -> u16 {
        match self {
            Kind::Intro => 1,
            Kind::Up => 2,
            Kind::Down => 3,
            Kind::Chorus => 5,
            Kind::Outro => 6,
        }
    }

    /// What to call it on a cue point.
    pub fn label(self) -> &'static str {
        match self {
            Kind::Intro => "intro",
            Kind::Up => "build",
            Kind::Down => "break",
            Kind::Chorus => "drop",
            Kind::Outro => "outro",
        }
    }
}

/// One stretch of a track, measured in beats from the first one.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Section {
    /// Beat number the section starts on, counting the first beat as 1.
    pub start_beat: u16,
    /// Beat number the next section starts on.
    pub end_beat: u16,
    pub kind: Kind,
    /// How much is happening: the mean onset strength over the section.
    ///
    /// Not the level. A pad can be louder than a kick drum and still be the
    /// quiet part of a record; what separates a drop from a breakdown is how
    /// much is being played, and that is what the onset envelope measures.
    pub intensity: f32,
}

/// How a whole track is put together.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Structure {
    pub sections: Vec<Section>,
}

impl Structure {
    pub fn is_empty(&self) -> bool {
        self.sections.is_empty()
    }

    /// The form the analysis files store, ready to be written into `PSSI`.
    pub fn to_song_structure(&self) -> Option<SongStructure> {
        let last = self.sections.last()?;
        Some(SongStructure {
            mood: Mood::High,
            end_beat: last.end_beat,
            bank: 0,
            phrases: self
                .sections
                .iter()
                .map(|s| Phrase { beat: s.start_beat, kind: s.kind.id() })
                .collect(),
        })
    }
}

/// Find the sections of a track, given where its beats are.
///
/// `beat_times` is in milliseconds, as the grid stores them. Returns an empty
/// structure when there is not enough track to say anything: fewer than a
/// couple of phrases, or no beats at all.
pub fn detect(features: &Features, beat_times: &[u32]) -> Structure {
    let (bars, variation) = bar_features(features, beat_times);
    if bars.len() < KERNEL_BARS * 2 {
        return Structure::default();
    }

    // A track whose spectrum never moves has one section, however long it is.
    // Standardising the bars would otherwise magnify the difference between
    // one identical bar and the next into a convincing arrangement.
    let novelty =
        if variation < MIN_VARIATION { vec![0.0; bars.len()] } else { novelty_curve(&bars) };
    let boundaries = pick_boundaries(&novelty, bars.len());
    let intensities = bar_intensity(features, beat_times, bars.len());

    let mut sections = Vec::with_capacity(boundaries.len());
    for (i, &start) in boundaries.iter().enumerate() {
        let end = boundaries.get(i + 1).copied().unwrap_or(bars.len());
        let intensity = mean(&intensities[start..end]);
        sections.push(Section {
            start_beat: (start * BEATS_PER_BAR) as u16 + 1,
            end_beat: (end * BEATS_PER_BAR) as u16 + 1,
            // Provisional; the pass below needs every section measured first.
            kind: Kind::Down,
            intensity,
        });
    }
    label(&mut sections);
    Structure { sections }
}

/// One vector per bar: mean band energy across its beats, then standardised per
/// band so that a comparison is about the shape of the spectrum rather than how
/// loud the bar happened to be.
fn bar_features(features: &Features, beat_times: &[u32]) -> (Vec<[f32; BANDS]>, f32) {
    if beat_times.len() < BEATS_PER_BAR + 1 || features.frames() == 0 {
        return (Vec::new(), 0.0);
    }
    let mut bars = Vec::new();
    for bar in beat_times.chunks(BEATS_PER_BAR) {
        let (Some(&first), Some(&last)) = (bar.first(), bar.last()) else { continue };
        if bar.len() < BEATS_PER_BAR {
            break;
        }
        let from = features.frame_at(first as f64 / 1000.0);
        let to = features.frame_at(last as f64 / 1000.0).max(from + 1);
        let mut mean = [0.0f32; BANDS];
        let mut counted = 0usize;
        for frame in from..to.min(features.frames()) {
            for (band, value) in features.band_frame(frame).iter().enumerate() {
                mean[band] += value;
            }
            counted += 1;
        }
        if counted == 0 {
            continue;
        }
        for value in &mut mean {
            *value /= counted as f32;
        }
        bars.push(mean);
    }
    let variation = standardise(&mut bars);
    (bars, variation)
}

/// Put every band on the same footing across the track, so a band that barely
/// moves cannot dominate the comparison and one that swings wildly cannot be
/// drowned out.
///
/// Returns how far the most variable band moved before that was done, which is
/// the only measure of whether there was anything to standardise.
fn standardise(bars: &mut [[f32; BANDS]]) -> f32 {
    if bars.is_empty() {
        return 0.0;
    }
    let mut widest = 0.0f32;
    for band in 0..BANDS {
        let values: Vec<f32> = bars.iter().map(|b| b[band]).collect();
        let mean = values.iter().sum::<f32>() / values.len() as f32;
        let deviation =
            (values.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / values.len() as f32).sqrt();
        widest = widest.max(deviation);
        let scale = if deviation > f32::EPSILON { 1.0 / deviation } else { 0.0 };
        for bar in bars.iter_mut() {
            bar[band] = (bar[band] - mean) * scale;
        }
    }
    widest
}

/// How much of a change each bar is, by Foote's checkerboard: the similarity
/// within the bars before and within the bars after, minus the similarity
/// across the divide. High where the music turns a corner.
fn novelty_curve(bars: &[[f32; BANDS]]) -> Vec<f32> {
    let mut novelty = vec![0.0f32; bars.len()];
    #[allow(clippy::needless_range_loop)] // `centre` is a bar number, not just an index
    for centre in KERNEL_BARS..bars.len().saturating_sub(KERNEL_BARS) {
        let before = centre - KERNEL_BARS..centre;
        let after = centre..centre + KERNEL_BARS;
        let mut same = 0.0f32;
        let mut across = 0.0f32;
        for i in before.clone() {
            for j in before.clone() {
                same += similarity(&bars[i], &bars[j]);
            }
            for j in after.clone() {
                across += similarity(&bars[i], &bars[j]);
            }
        }
        for i in after.clone() {
            for j in after.clone() {
                same += similarity(&bars[i], &bars[j]);
            }
        }
        novelty[centre] = (same - 2.0 * across) / (KERNEL_BARS * KERNEL_BARS) as f32;
    }
    novelty
}

fn similarity(a: &[f32; BANDS], b: &[f32; BANDS]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a < f32::EPSILON || norm_b < f32::EPSILON {
        0.0
    } else {
        dot / (norm_a * norm_b)
    }
}

/// Turn the novelty curve into a list of bars where a section starts. Always
/// includes bar zero: a track begins with a section whether or not anything
/// changed to start it.
fn pick_boundaries(novelty: &[f32], bars: usize) -> Vec<usize> {
    // The kernel cannot reach the ends of the track, so those bars are zero and
    // would drag the threshold down if they were counted.
    let measured: Vec<f32> =
        novelty[KERNEL_BARS..novelty.len().saturating_sub(KERNEL_BARS)].to_vec();
    if measured.is_empty() {
        return vec![0];
    }
    let mean = measured.iter().sum::<f32>() / measured.len() as f32;
    let deviation =
        (measured.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / measured.len() as f32).sqrt();
    let threshold = mean + PEAK_PROMINENCE * deviation;

    let mut candidates: Vec<(usize, f32)> = (1..novelty.len().saturating_sub(1))
        .filter(|&i| novelty[i] > novelty[i - 1] && novelty[i] >= novelty[i + 1])
        .map(|i| (i, novelty[i]))
        .collect();
    candidates.sort_by(|a, b| b.1.total_cmp(&a.1));

    let mut chosen = vec![0usize];
    for (bar, strength) in candidates {
        if strength <= 0.0 || strength < threshold {
            continue;
        }
        let snapped = (bar + SNAP_BARS / 2) / SNAP_BARS * SNAP_BARS;
        if snapped == 0 || snapped + MIN_SECTION_BARS > bars {
            continue;
        }
        if chosen.iter().any(|&other| snapped.abs_diff(other) < MIN_SECTION_BARS) {
            continue;
        }
        chosen.push(snapped);
    }
    chosen.sort_unstable();
    chosen
}

fn bar_intensity(features: &Features, beat_times: &[u32], bars: usize) -> Vec<f32> {
    (0..bars)
        .map(|bar| {
            let first = beat_times[bar * BEATS_PER_BAR];
            let last = beat_times
                .get((bar + 1) * BEATS_PER_BAR)
                .copied()
                .unwrap_or_else(|| *beat_times.last().unwrap());
            let from = features.frame_at(first as f64 / 1000.0);
            let to = features.frame_at(last as f64 / 1000.0).max(from + 1);
            let frames = to.min(features.frames());
            // Onset strength, with the bottom of the spectrum counted twice:
            // a section with drums in it is a different section.
            mean(&features.flux[from..frames]) + mean(&features.low_flux[from..frames])
        })
        .collect()
}

/// Name each section from how busy it is and which way the track is going.
///
/// Everything is relative to the track's own range, because a record that never
/// takes the drums out still has a loudest part and a quietest one.
fn label(sections: &mut [Section]) {
    let Some(least) = sections.iter().map(|s| s.intensity).reduce(f32::min) else { return };
    let most = sections.iter().map(|s| s.intensity).fold(f32::MIN, f32::max);
    let range = (most - least).max(f32::EPSILON);
    let place = |section: &Section| (section.intensity - least) / range;

    let last = sections.len() - 1;
    for i in 0..sections.len() {
        let position = place(&sections[i]);
        let rising = i > 0 && position > place(&sections[i - 1]) + 0.1;
        let next_is_busier = i < last && place(&sections[i + 1]) > position + 0.2;

        // The last section is an outro only if it sounds like one. A track that
        // ends on its loudest passage ends on a drop, and calling that an outro
        // would put the wrong colour on the wrong cue.
        let ends_quietly = position < 0.5;

        sections[i].kind = if i == 0 {
            Kind::Intro
        } else if i == last && ends_quietly {
            Kind::Outro
        } else if position > 0.66 {
            Kind::Chorus
        } else if position < 0.33 {
            // The quietest parts are breakdowns even when a drop follows, which
            // is what a DJ means by the word: the interesting thing about that
            // section is that everything went away.
            Kind::Down
        } else if next_is_busier || rising {
            Kind::Up
        } else {
            Kind::Down
        };
    }
}

fn mean(values: &[f32]) -> f32 {
    if values.is_empty() {
        0.0
    } else {
        values.iter().sum::<f32>() / values.len() as f32
    }
}

#[cfg(test)]
mod tests {
    use super::super::{features, tempo};
    use super::*;
    use crate::audio::Audio;

    const RATE: u32 = 44_100;
    const BPM: f64 = 120.0;

    /// Build a track section by section. Each entry is a number of bars and a
    /// description of what plays: `(bars, kick_gain, bass_gain, lead_gain,
    /// hat_gain)`. That is enough for the spectrum to change shape between
    /// sections, which is what the detector looks for.
    fn arrangement(sections: &[(usize, f32, f32, f32, f32)]) -> Audio {
        let beat = 60.0 / BPM;
        let total_bars: usize = sections.iter().map(|s| s.0).sum();
        let lead_in = RATE as usize / 2;
        let len = lead_in + (RATE as f64 * beat * (total_bars * 4) as f64) as usize;
        let mut plane = vec![0.0f32; len];

        let mut bar = 0usize;
        for &(bars, kick, bass, lead, hat) in sections {
            for _ in 0..bars {
                for beat_in_bar in 0..4 {
                    let start =
                        lead_in + (RATE as f64 * beat * (bar * 4 + beat_in_bar) as f64) as usize;
                    for i in 0..(RATE as usize / 4) {
                        let at = start + i;
                        if at >= len {
                            break;
                        }
                        let t = i as f32 / RATE as f32;
                        let two_pi = 2.0 * std::f32::consts::PI;
                        let mut sample = kick * (-30.0 * t).exp() * (two_pi * 55.0 * t).sin();
                        sample += bass * (two_pi * 110.0 * t).sin() * (-2.0 * t).exp();
                        sample += lead * (two_pi * 1_400.0 * t).sin() * (-3.0 * t).exp();
                        sample += hat * (-60.0 * t).exp() * (two_pi * 9_000.0 * t).sin();
                        plane[at] += sample * 0.5;
                    }
                }
                bar += 1;
            }
        }
        Audio::new(RATE, vec![plane.clone(), plane]).unwrap()
    }

    fn analyse(audio: &Audio) -> (Structure, Vec<u32>) {
        let f = features::extract(audio);
        let beats = tempo::detect(&f);
        let times: Vec<u32> = beats.grid.beats.iter().map(|b| b.time_ms).collect();
        (detect(&f, &times), times)
    }

    /// Which bar each section starts on, counting from the first beat.
    fn section_bars(structure: &Structure) -> Vec<usize> {
        structure.sections.iter().map(|s| (s.start_beat as usize - 1) / 4).collect()
    }

    #[test]
    fn a_track_that_never_changes_is_one_section() {
        let (structure, _) = analyse(&arrangement(&[(32, 1.0, 0.5, 0.3, 0.2)]));
        assert_eq!(structure.sections.len(), 1, "{:?}", section_bars(&structure));
    }

    #[test]
    fn a_breakdown_is_found_where_it_happens() {
        // Sixteen bars of everything, sixteen with the drums gone, sixteen back.
        let (structure, _) = analyse(&arrangement(&[
            (16, 1.0, 0.5, 0.3, 0.25),
            (16, 0.0, 0.4, 0.3, 0.0),
            (16, 1.0, 0.5, 0.3, 0.25),
        ]));

        let bars = section_bars(&structure);
        assert!(structure.sections.len() >= 3, "found only {bars:?}");
        // The two boundaries should be close to bars 16 and 32. The beat
        // tracker starts its grid a little before the first click, so allow a
        // bar of slack either way.
        assert!(bars.iter().any(|&b| b.abs_diff(16) <= 4), "no boundary near bar 16: {bars:?}");
        assert!(bars.iter().any(|&b| b.abs_diff(32) <= 4), "no boundary near bar 32: {bars:?}");
    }

    #[test]
    fn the_first_section_is_an_intro_and_the_last_an_outro() {
        let (structure, _) = analyse(&arrangement(&[
            (16, 0.4, 0.2, 0.0, 0.1),
            (16, 1.0, 0.6, 0.4, 0.3),
            (16, 0.3, 0.2, 0.0, 0.1),
        ]));
        assert_eq!(structure.sections.first().unwrap().kind, Kind::Intro);
        assert_eq!(structure.sections.last().unwrap().kind, Kind::Outro);
    }

    #[test]
    fn the_loudest_middle_section_is_the_drop() {
        let (structure, _) = analyse(&arrangement(&[
            (16, 0.3, 0.2, 0.0, 0.1),
            (16, 1.0, 0.8, 0.5, 0.4),
            (16, 0.2, 0.2, 0.0, 0.05),
            (16, 1.0, 0.8, 0.5, 0.4),
            (16, 0.3, 0.2, 0.0, 0.1),
        ]));
        let drops = structure.sections.iter().filter(|s| s.kind == Kind::Chorus).count();
        assert!(drops >= 1, "no drop in {:?}", structure.sections);
        // And the quiet stretch between them is not one.
        let quietest =
            structure.sections.iter().min_by(|a, b| a.intensity.total_cmp(&b.intensity)).unwrap();
        assert_ne!(quietest.kind, Kind::Chorus);
    }

    #[test]
    fn sections_are_whole_bars_and_do_not_overlap() {
        let (structure, times) = analyse(&arrangement(&[
            (16, 1.0, 0.5, 0.3, 0.25),
            (16, 0.0, 0.4, 0.3, 0.0),
            (16, 1.0, 0.5, 0.3, 0.25),
        ]));
        for pair in structure.sections.windows(2) {
            assert_eq!(pair[0].end_beat, pair[1].start_beat);
        }
        for section in &structure.sections {
            assert_eq!((section.start_beat - 1) % 4, 0, "a phrase started mid-bar");
            assert!(section.end_beat > section.start_beat);
            assert!(section.end_beat as usize <= times.len() + 4);
        }
    }

    #[test]
    fn a_track_with_no_beats_has_no_structure() {
        let f = features::extract(&arrangement(&[(8, 1.0, 0.5, 0.3, 0.2)]));
        assert!(detect(&f, &[]).is_empty());
    }

    #[test]
    fn a_track_too_short_to_have_phrases_has_none() {
        let (structure, _) = analyse(&arrangement(&[(4, 1.0, 0.5, 0.3, 0.2)]));
        assert!(structure.is_empty());
    }

    #[test]
    fn the_song_structure_carries_every_phrase_in_order() {
        let (structure, _) = analyse(&arrangement(&[
            (16, 0.3, 0.2, 0.0, 0.1),
            (16, 1.0, 0.8, 0.5, 0.4),
            (16, 0.3, 0.2, 0.0, 0.1),
        ]));
        let song = structure.to_song_structure().unwrap();

        assert_eq!(song.mood, Mood::High);
        assert_eq!(song.phrases.len(), structure.sections.len());
        assert_eq!(song.phrases[0].beat, 1);
        assert_eq!(song.end_beat, structure.sections.last().unwrap().end_beat);
        for pair in song.phrases.windows(2) {
            assert!(pair[1].beat > pair[0].beat);
        }
    }
}
