//! Deciding where the cue points go.
//!
//! A cue is worth setting where something changes and where the voice comes in.
//! The first of those falls out of the phrase analysis; the second needs a
//! measurement of its own, and gets an honest one: energy that is both centred
//! in the stereo image and in the range a voice occupies. That finds a sung
//! line entering over a backing. It will also fire on a centred lead synth,
//! which is the limit of what can be told without separating the stems — and
//! separating them is minutes a track, not milliseconds.
//!
//! Everything lands on a beat, and on a downbeat where there is one close by,
//! because a cue half a beat early is worse than no cue at all.

use crate::export::{Cue, Rgb};

use super::features::Features;
use super::structure::{Kind, Structure};

/// A player has eight hot cues, A through H.
const HOT_CUES: usize = 8;
/// How long the voice has to keep going before its arrival counts, in seconds.
/// Shorter than this is a vocal stab, not an entry.
const MIN_VOCAL_SECONDS: f64 = 1.0;
/// And how long since the last one, so a phrase with gaps in it does not
/// produce a cue per breath.
const VOCAL_SPACING_SECONDS: f64 = 8.0;
/// How far a cue will be moved to land on a downbeat rather than a plain beat.
const SNAP_BEATS: usize = 2;

/// What a cue was set for, which decides its colour and what it is called.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Reason {
    Section(Kind),
    /// Where a voice arrives, measured off the mix.
    Vocal,
    /// The first landing of the line the track repeats most, read out of the
    /// words. See [`crate::transcribe`].
    Hook,
    /// And every landing after that.
    Refrain,
}

impl Reason {
    /// What to call it, when nothing better is known. A hook knows better: it
    /// carries the line itself.
    pub fn label(self) -> &'static str {
        match self {
            Reason::Section(kind) => kind.label(),
            Reason::Vocal => "vocal",
            Reason::Hook => "hook",
            Reason::Refrain => "hook again",
        }
    }

    /// Colours a DJ can read at a glance in a dark booth: the drop is red, the
    /// breakdown is blue, the voice is purple and the hook is the brightest
    /// thing on the waveform.
    pub fn color(self) -> Rgb {
        let (r, g, b) = match self {
            Reason::Section(Kind::Intro) => (0x30, 0x5a, 0xff),
            Reason::Section(Kind::Up) => (0xe2, 0xa0, 0x3f),
            Reason::Section(Kind::Chorus) => (0xc0, 0x3a, 0x22),
            Reason::Section(Kind::Down) => (0x2f, 0x6f, 0xd0),
            Reason::Section(Kind::Outro) => (0x2f, 0x7d, 0x52),
            Reason::Vocal => (0x9a, 0x6b, 0xd4),
            Reason::Hook => (0xe8, 0x3c, 0x9e),
            Reason::Refrain => (0xb4, 0x5c, 0xc8),
        };
        Rgb { r, g, b }
    }

    /// Which cues survive when there are more than eight candidates.
    ///
    /// The hook outranks everything, including the drop. A drop can be found by
    /// looking at the waveform — it is the loud part — and the line the crowd
    /// sings cannot be found by looking at anything.
    pub fn priority(self, first_of_its_kind: bool) -> u32 {
        match self {
            Reason::Hook => 110,
            Reason::Section(Kind::Chorus) => 100,
            Reason::Vocal if first_of_its_kind => 95,
            Reason::Section(Kind::Intro) => 90,
            Reason::Section(Kind::Down) => 70,
            Reason::Refrain => 65,
            Reason::Vocal => 60,
            Reason::Section(Kind::Outro) => 55,
            Reason::Section(Kind::Up) => 50,
        }
    }

    fn is_section(self) -> bool {
        matches!(self, Reason::Section(_))
    }
}

/// One moment that might be worth a cue.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub time_ms: u32,
    pub reason: Reason,
    /// What to write on the cue, when there is something better to say than the
    /// reason's own name — the words of a hook, say.
    pub label: Option<String>,
}

impl Candidate {
    pub fn new(time_ms: u32, reason: Reason) -> Self {
        Self { time_ms, reason, label: None }
    }

    pub fn named(time_ms: u32, reason: Reason, label: &str) -> Self {
        let label = label.trim();
        Self { time_ms, reason, label: (!label.is_empty()).then(|| label.to_string()) }
    }

    fn comment(&self) -> String {
        self.label.clone().unwrap_or_else(|| self.reason.label().to_string())
    }
}

/// The cue a moment in the words has earned.
pub fn from_moment(moment: &crate::transcribe::Moment) -> Candidate {
    use crate::transcribe::MomentKind;
    let reason = match moment.kind {
        MomentKind::VocalIn => Reason::Vocal,
        MomentKind::Hook => Reason::Hook,
        MomentKind::Refrain => Reason::Refrain,
    };
    Candidate::named(moment.time_ms, reason, &moment.text)
}

/// How close two moments have to be before they are one moment.
///
/// Half a second. A cue that near another one is not a second place to drop
/// in — it is the same place, found twice by two different measurements.
const TOGETHER_MS: u32 = 500;

/// Suggest cue points for a track.
///
/// Returns a memory cue at the first downbeat — which is where a player parks
/// when the track loads — followed by up to eight hot cues in time order.
pub fn suggest(features: &Features, beat_times: &[u32], structure: &Structure) -> Vec<Cue> {
    if beat_times.is_empty() {
        return Vec::new();
    }

    let mut candidates = sections(structure, beat_times);
    for at in vocal_entries(features, beat_times, structure) {
        candidates.push(Candidate::new(at, Reason::Vocal));
    }

    // The memory cue is where the track begins as far as a player is
    // concerned: the first downbeat, or the first beat if the grid has no bar
    // lines yet.
    let start = first_downbeat(beat_times, structure).unwrap_or(beat_times[0]);
    assemble(start, candidates)
}

/// A cue at the start of every section the phrase analysis found.
pub fn sections(structure: &Structure, beat_times: &[u32]) -> Vec<Candidate> {
    structure
        .sections
        .iter()
        .filter_map(|section| {
            beat_times
                .get(section.start_beat as usize - 1)
                .map(|&at| Candidate::new(at, Reason::Section(section.kind)))
        })
        .collect()
}

/// Turn everything worth cueing into the cues a player will actually hold.
///
/// One memory cue at `start_ms`, then the best eight of `candidates` in time
/// order, lettered A onwards. Moments that land on top of each other are folded
/// into one, and the survivors are ranked by [`Reason::priority`] — because a
/// player has eight hot cues and a busy track has more than eight moments, and
/// which eight it keeps is the whole difference between a useful set of cues
/// and a wall of markers.
pub fn assemble(start_ms: u32, candidates: Vec<Candidate>) -> Vec<Cue> {
    let mut kept = fold(rank(candidates));
    if kept.len() > HOT_CUES {
        kept.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.time_ms.cmp(&b.0.time_ms)));
        kept.truncate(HOT_CUES);
        kept.sort_by_key(|(candidate, _)| candidate.time_ms);
    }

    let mut cues = vec![Cue::memory(start_ms)];
    for (letter, (candidate, _)) in kept.into_iter().enumerate() {
        let color = candidate.reason.color();
        cues.push(
            Cue::hot(letter as u8 + 1, candidate.time_ms)
                .with_comment(&candidate.comment())
                .with_color(color.r, color.g, color.b),
        );
    }
    cues
}

/// Put the candidates in time order and price each one.
///
/// The first vocal is worth more than the ones after it, which is why this is a
/// pass over the whole list rather than a map: what a moment is worth depends
/// on what came before it.
fn rank(mut candidates: Vec<Candidate>) -> Vec<(Candidate, u32)> {
    candidates.sort_by_key(|candidate| candidate.time_ms);
    let mut seen_vocal = false;
    candidates
        .into_iter()
        .map(|candidate| {
            let first = candidate.reason == Reason::Vocal && !seen_vocal;
            if candidate.reason == Reason::Vocal {
                seen_vocal = true;
            }
            let priority = candidate.reason.priority(first);
            (candidate, priority)
        })
        .collect()
}

/// Fold moments that land together into one.
///
/// A section start wins any such argument, whatever it is worth: when the voice
/// arrives exactly where the drop does, that is the drop, and a second marker
/// half a second away from the first is a marker nobody can use. But the loser's
/// words are kept — a drop that is also where the hook lands should say so,
/// because "drop" is something a DJ can see on the waveform and the line the
/// crowd sings is not.
///
/// Takes the list already in time order, as [`rank`] leaves it.
fn fold(ranked: Vec<(Candidate, u32)>) -> Vec<(Candidate, u32)> {
    let mut kept: Vec<(Candidate, u32)> = Vec::with_capacity(ranked.len());
    for (candidate, priority) in ranked {
        let Some((last, last_priority)) = kept.last_mut() else {
            kept.push((candidate, priority));
            continue;
        };
        if last.time_ms.abs_diff(candidate.time_ms) >= TOGETHER_MS {
            kept.push((candidate, priority));
            continue;
        }
        let beaten = match (last.reason.is_section(), candidate.reason.is_section()) {
            (true, false) => false,
            (false, true) => true,
            _ => priority > *last_priority,
        };
        match beaten {
            true => {
                let words = last.label.take();
                *last = candidate;
                *last_priority = priority;
                last.label = last.label.take().or(words);
            }
            false => last.label = last.label.take().or(candidate.label),
        }
    }
    kept
}

fn first_downbeat(beat_times: &[u32], structure: &Structure) -> Option<u32> {
    structure
        .sections
        .first()
        .and_then(|s| beat_times.get(s.start_beat as usize - 1))
        .or_else(|| beat_times.first())
        .copied()
}

/// Where a voice arrives, in milliseconds, snapped to the grid.
fn vocal_entries(features: &Features, beat_times: &[u32], structure: &Structure) -> Vec<u32> {
    let curve = smoothed_voice(features);
    if curve.is_empty() {
        return Vec::new();
    }

    let mut sorted = curve.clone();
    sorted.sort_by(f32::total_cmp);
    let quiet = sorted[sorted.len() / 5];
    let loud = sorted[sorted.len() * 9 / 10];
    // Nothing stands out from the backing, so nothing is singing over it.
    if loud - quiet < 0.02 {
        return Vec::new();
    }
    let threshold = quiet + (loud - quiet) * 0.5;

    let sustain = (MIN_VOCAL_SECONDS * features.frame_rate) as usize;
    let mut entries = Vec::new();
    let mut last_entry = f64::NEG_INFINITY;
    let mut frame = 1;
    while frame + sustain < curve.len() {
        let rising = curve[frame] >= threshold && curve[frame - 1] < threshold;
        if rising && curve[frame..frame + sustain].iter().all(|&v| v >= threshold * 0.7) {
            let seconds = features.seconds_at(frame);
            if seconds - last_entry >= VOCAL_SPACING_SECONDS {
                entries.push(snap(seconds, beat_times, structure));
                last_entry = seconds;
            }
            frame += sustain;
            continue;
        }
        frame += 1;
    }
    entries
}

/// The voice measurement, smoothed over half a second so that one syllable does
/// not read as an entry and one gap does not read as a departure.
fn smoothed_voice(features: &Features) -> Vec<f32> {
    let window = ((features.frame_rate / 2.0) as usize).max(1);
    if features.voice.len() < window * 2 {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(features.voice.len());
    let mut sum = 0.0f64;
    for (i, &value) in features.voice.iter().enumerate() {
        sum += value as f64;
        if i >= window {
            sum -= features.voice[i - window] as f64;
        }
        out.push((sum / window.min(i + 1) as f64) as f32);
    }
    out
}

/// Move a moment onto the grid: the nearest downbeat if one is within a beat or
/// two, otherwise the nearest beat.
fn snap(seconds: f64, beat_times: &[u32], structure: &Structure) -> u32 {
    let target = (seconds * 1000.0).round() as u32;
    let nearest = beat_times
        .iter()
        .enumerate()
        .min_by_key(|(_, &at)| at.abs_diff(target))
        .map(|(i, _)| i)
        .unwrap_or(0);

    // Bar lines are wherever the phrase analysis says a section starts, four
    // beats apart from there.
    let bar_phase =
        structure.sections.first().map(|s| (s.start_beat as usize - 1) % 4).unwrap_or(0);
    let from = nearest.saturating_sub(SNAP_BEATS);
    let to = (nearest + SNAP_BEATS + 1).min(beat_times.len());
    let downbeat = (from..to).find(|i| i % 4 == bar_phase);
    beat_times[downbeat.unwrap_or(nearest)]
}

#[cfg(test)]
mod tests {
    use super::super::features;
    use super::super::structure::Section;
    use super::*;
    use crate::audio::Audio;
    use crate::export::CueKind;

    const RATE: u32 = 44_100;
    const BEAT_MS: u32 = 500;

    fn beats(count: usize) -> Vec<u32> {
        (0..count).map(|i| i as u32 * BEAT_MS).collect()
    }

    fn line(start_ms: u32, text: &str) -> crate::transcribe::Line {
        crate::transcribe::Line { start_ms, end_ms: start_ms + 2_000, text: text.to_string() }
    }

    fn section(start_bar: usize, end_bar: usize, kind: Kind, intensity: f32) -> Section {
        Section {
            start_beat: (start_bar * 4) as u16 + 1,
            end_beat: (end_bar * 4) as u16 + 1,
            kind,
            intensity,
        }
    }

    /// A track with a backing throughout and a centred "voice" over part of it.
    fn with_voice(total_secs: f32, voice_from: f32, voice_to: f32) -> Audio {
        let frames = (RATE as f32 * total_secs) as usize;
        let mut left = vec![0.0f32; frames];
        let mut right = vec![0.0f32; frames];
        for i in 0..frames {
            let t = i as f32 / RATE as f32;
            let two_pi = 2.0 * std::f32::consts::PI;
            // A wide backing: the same note, out of phase, so it sits nowhere
            // near the middle.
            let backing = 0.3 * (two_pi * 300.0 * t).sin();
            left[i] = backing;
            right[i] = -backing;
            // Plus a kick down the middle, which is centred but too low to be
            // mistaken for a voice.
            let kick = 0.4 * (two_pi * 55.0 * t).sin();
            left[i] += kick;
            right[i] += kick;
            if t >= voice_from && t < voice_to {
                // A centred, wobbling tone in the vocal range.
                let vibrato = (two_pi * 5.0 * t).sin() * 20.0;
                let voice = 0.35 * (two_pi * (900.0 + vibrato) * t).sin();
                left[i] += voice;
                right[i] += voice;
            }
        }
        Audio::new(RATE, vec![left, right]).unwrap()
    }

    #[test]
    fn every_section_start_gets_a_cue() {
        let structure = Structure {
            sections: vec![
                section(0, 8, Kind::Intro, -20.0),
                section(8, 16, Kind::Up, -14.0),
                section(16, 24, Kind::Chorus, -8.0),
            ],
        };
        let audio = with_voice(4.0, 10.0, 10.0);
        let cues = suggest(&features::extract(&audio), &beats(128), &structure);

        let hot: Vec<&Cue> = cues.iter().filter(|c| c.is_hot()).collect();
        assert_eq!(hot.len(), 3);
        assert_eq!(hot[0].time_ms, 0);
        assert_eq!(hot[1].time_ms, 8 * 4 * BEAT_MS);
        assert_eq!(hot[2].time_ms, 16 * 4 * BEAT_MS);
    }

    #[test]
    fn cues_are_named_and_coloured_by_what_they_are() {
        let structure = Structure {
            sections: vec![
                section(0, 8, Kind::Intro, -20.0),
                section(8, 16, Kind::Down, -18.0),
                section(16, 24, Kind::Chorus, -8.0),
            ],
        };
        let cues =
            suggest(&features::extract(&with_voice(4.0, 10.0, 10.0)), &beats(128), &structure);
        let hot: Vec<&Cue> = cues.iter().filter(|c| c.is_hot()).collect();

        assert_eq!(hot[0].comment.as_deref(), Some("intro"));
        assert_eq!(hot[1].comment.as_deref(), Some("break"));
        assert_eq!(hot[2].comment.as_deref(), Some("drop"));
        // The drop is red and the breakdown is blue, and they are not the same.
        assert_ne!(hot[1].color, hot[2].color);
        assert_eq!(hot[2].color, Some(Rgb { r: 0xc0, g: 0x3a, b: 0x22 }));
    }

    #[test]
    fn the_first_cue_a_player_parks_on_is_a_memory_cue() {
        let structure = Structure { sections: vec![section(0, 8, Kind::Intro, -20.0)] };
        let cues =
            suggest(&features::extract(&with_voice(4.0, 10.0, 10.0)), &beats(64), &structure);
        assert!(!cues[0].is_hot());
        assert_eq!(cues[0].time_ms, 0);
        assert_eq!(cues[0].kind, CueKind::Point);
    }

    #[test]
    fn a_voice_coming_in_gets_its_own_cue() {
        // Twenty seconds of backing, with a voice from ten seconds in.
        let audio = with_voice(20.0, 10.0, 20.0);
        let f = features::extract(&audio);
        let structure = Structure { sections: vec![section(0, 10, Kind::Intro, -20.0)] };
        let cues = suggest(&f, &beats(80), &structure);

        let vocal = cues.iter().find(|c| c.comment.as_deref() == Some("vocal"));
        let vocal = vocal.unwrap_or_else(|| panic!("no vocal cue in {cues:?}"));
        let seconds = vocal.time_ms as f64 / 1000.0;
        assert!((seconds - 10.0).abs() < 1.5, "the voice cue landed at {seconds:.2}s");
    }

    #[test]
    fn an_instrumental_gets_no_vocal_cue() {
        let audio = with_voice(20.0, 100.0, 100.0);
        let f = features::extract(&audio);
        let structure = Structure { sections: vec![section(0, 10, Kind::Intro, -20.0)] };
        let cues = suggest(&f, &beats(80), &structure);
        assert!(cues.iter().all(|c| c.comment.as_deref() != Some("vocal")), "{cues:?}");
    }

    #[test]
    fn a_vocal_cue_lands_on_a_beat() {
        let audio = with_voice(20.0, 10.3, 20.0);
        let f = features::extract(&audio);
        let structure = Structure { sections: vec![section(0, 10, Kind::Intro, -20.0)] };
        let cues = suggest(&f, &beats(80), &structure);
        for cue in &cues {
            assert_eq!(cue.time_ms % BEAT_MS, 0, "a cue at {} ms is off the grid", cue.time_ms);
        }
    }

    #[test]
    fn a_track_with_more_than_eight_moments_keeps_the_useful_ones() {
        let kinds = [
            Kind::Intro,
            Kind::Up,
            Kind::Chorus,
            Kind::Down,
            Kind::Up,
            Kind::Chorus,
            Kind::Down,
            Kind::Up,
            Kind::Chorus,
            Kind::Outro,
        ];
        let sections = kinds
            .iter()
            .enumerate()
            .map(|(i, &kind)| section(i * 8, (i + 1) * 8, kind, -10.0))
            .collect();
        let structure = Structure { sections };
        let cues =
            suggest(&features::extract(&with_voice(4.0, 10.0, 10.0)), &beats(400), &structure);

        let hot: Vec<&Cue> = cues.iter().filter(|c| c.is_hot()).collect();
        assert_eq!(hot.len(), 8, "a player has eight hot cues");
        // All three drops survived; the builds are what got dropped.
        assert_eq!(hot.iter().filter(|c| c.comment.as_deref() == Some("drop")).count(), 3);
        assert!(hot.iter().filter(|c| c.comment.as_deref() == Some("build")).count() < 3);
        // And they are lettered in time order.
        for pair in hot.windows(2) {
            assert!(pair[0].hot_cue < pair[1].hot_cue);
            assert!(pair[0].time_ms <= pair[1].time_ms);
        }
    }

    #[test]
    fn the_hook_is_cued_with_the_words_that_make_it_one() {
        let words = crate::transcribe::Transcript {
            lines: vec![
                line(30_000, "hold me closer now"),
                line(90_000, "hold me closer now"),
                line(150_000, "hold me closer now"),
            ],
        };
        let mut candidates = vec![Candidate::new(0, Reason::Section(Kind::Intro))];
        candidates.extend(words.moments().iter().map(from_moment));
        let cues = assemble(0, candidates);

        let hot: Vec<&Cue> = cues.iter().filter(|c| c.is_hot()).collect();
        let hook = hot.iter().find(|c| c.time_ms == 30_000).expect("no cue on the hook");
        assert_eq!(hook.comment.as_deref(), Some("hold me closer now"));
        // And its returns are cued too, with the same words.
        assert_eq!(
            hot.iter().filter(|c| c.comment.as_deref() == hook.comment.as_deref()).count(),
            3
        );
    }

    #[test]
    fn the_hook_survives_a_track_with_more_moments_than_a_player_has_cues() {
        // Ten sections, all of them loud, plus a hook halfway through. Eight
        // slots; the hook takes one of them whatever else is going on.
        let mut candidates: Vec<Candidate> =
            (0..10).map(|i| Candidate::new(i * 20_000, Reason::Section(Kind::Chorus))).collect();
        candidates.push(Candidate::named(95_000, Reason::Hook, "hold me closer now"));

        let cues = assemble(0, candidates);
        let hot: Vec<&Cue> = cues.iter().filter(|c| c.is_hot()).collect();
        assert_eq!(hot.len(), 8);
        assert!(
            hot.iter().any(|c| c.comment.as_deref() == Some("hold me closer now")),
            "the hook was dropped for a drop: {hot:?}"
        );
    }

    #[test]
    fn a_drop_that_is_also_the_hook_is_one_cue_that_says_the_words() {
        let candidates = vec![
            Candidate::new(60_000, Reason::Section(Kind::Chorus)),
            Candidate::named(60_200, Reason::Hook, "hold me closer now"),
        ];
        let cues = assemble(0, candidates);

        let hot: Vec<&Cue> = cues.iter().filter(|c| c.is_hot()).collect();
        assert_eq!(hot.len(), 1, "two markers half a second apart: {hot:?}");
        // The section keeps the moment — it is on the grid — and the words
        // come with it.
        assert_eq!(hot[0].time_ms, 60_000);
        assert_eq!(hot[0].comment.as_deref(), Some("hold me closer now"));
        assert_eq!(hot[0].color, Some(Reason::Section(Kind::Chorus).color()));
    }

    #[test]
    fn a_hook_is_worth_more_than_a_build() {
        assert!(Reason::Hook.priority(false) > Reason::Section(Kind::Chorus).priority(false));
        assert!(Reason::Refrain.priority(false) > Reason::Section(Kind::Up).priority(false));
        assert!(Reason::Hook.color() != Reason::Section(Kind::Chorus).color());
    }

    #[test]
    fn a_track_with_no_beats_gets_no_cues() {
        let cues =
            suggest(&features::extract(&with_voice(4.0, 1.0, 3.0)), &[], &Structure::default());
        assert!(cues.is_empty());
    }
}
