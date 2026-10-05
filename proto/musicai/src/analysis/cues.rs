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
/// How many of those each kind of cue gets before the other kind may have the
/// rest. Half each: the arrangement and the words are two different ways of
/// finding your place in a record, and a set that is all of one of them is
/// only half a set.
const SHARE: usize = HOT_CUES / 2;
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
    /// Where another line the track keeps coming back to first lands.
    Refrain,
    /// A line sung once and never again.
    ///
    /// Only ever fills a button nothing better wanted — see [`to_the_buttons`]
    /// — which on a track with no chorus is most of them.
    Line,
}

impl Reason {
    /// What to call it, when nothing better is known. A hook knows better: it
    /// carries the line itself.
    pub fn label(self) -> &'static str {
        match self {
            Reason::Section(kind) => kind.label(),
            Reason::Vocal => "vocal",
            Reason::Hook => "hook",
            Reason::Refrain => "refrain",
            Reason::Line => "line",
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
            Reason::Line => (0x74, 0x55, 0xa4),
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
            Reason::Line => 45,
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
    /// How many bars the section runs for, where this is one.
    ///
    /// On the marker rather than only in the window, because a player's own
    /// phrase strip cannot say it: the label there is chosen from ten fixed
    /// strings by the phrase kind and its flag bytes, and there is no text
    /// field in the format to put a number in. A memory cue's comment is text,
    /// and a player shows it.
    pub bars: Option<u16>,
    /// Whether this may only ever be a memory cue.
    ///
    /// A player has eight buttons and as many memory cues as a track needs, so
    /// a moment that is worth marking but not worth a button — a line coming
    /// round for the fourth time — says so here rather than being ranked
    /// against the drops and losing every time by a different margin.
    pub memory_only: bool,
}

impl Candidate {
    pub fn new(time_ms: u32, reason: Reason) -> Self {
        Self { time_ms, reason, label: None, bars: None, memory_only: false }
    }

    pub fn named(time_ms: u32, reason: Reason, label: &str) -> Self {
        let label = label.trim();
        Self {
            time_ms,
            reason,
            label: (!label.is_empty()).then(|| label.to_string()),
            bars: None,
            memory_only: false,
        }
    }

    /// The same moment, marked as one that never takes a button.
    pub fn memory_only(mut self) -> Self {
        self.memory_only = true;
        self
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
        MomentKind::Hook | MomentKind::Return => Reason::Hook,
        MomentKind::Refrain => Reason::Refrain,
        MomentKind::Line => Reason::Line,
    };
    let candidate = Candidate::named(moment.time_ms, reason, &moment.text);
    match moment.kind {
        MomentKind::Return => candidate.memory_only(),
        _ => candidate,
    }
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
    // The last beat the grid names, which is the last bar a player will let you
    // park on rather than wherever the file happens to stop.
    let end = beat_times.last().copied();
    assemble(start, end, candidates)
}

/// A cue at the start of every section the phrase analysis found.
pub fn sections(structure: &Structure, beat_times: &[u32]) -> Vec<Candidate> {
    structure
        .sections
        .iter()
        .filter_map(|section| {
            beat_times.get(section.start_beat as usize - 1).map(|&at| Candidate {
                // Whole bars, rounded down: a boundary dragged into the middle
                // of a bar loses that bar rather than claiming it, which is
                // what the window's own strip counts and shows.
                bars: Some(section.end_beat.saturating_sub(section.start_beat) / 4)
                    .filter(|bars| *bars > 0),
                ..Candidate::new(at, Reason::Section(section.kind))
            })
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
pub fn assemble(start_ms: u32, end_ms: Option<u32>, candidates: Vec<Candidate>) -> Vec<Cue> {
    let mut kept = fold(rank(candidates));

    // Every moment worth cueing becomes a memory cue, whether or not it also
    // gets one of the eight buttons. A player holds as many of these as the
    // track needs, and what they cost is a line in the list rather than a hot
    // cue somebody wanted for something else.
    let mut cues = memory_cues(start_ms, end_ms, &kept);

    // A hot cue where the memory cue already is, is a button that does what
    // loading the track already did — and eight is not many. The name is worth
    // keeping: "intro" on the cue a player parks at says something, and it
    // costs no slot to say it there.
    //
    // This is nearly always the first section: an arrangement starts at the
    // top of the record, and so does the memory cue. It was quietly taking hot
    // cue A on every track, where it sat invisibly underneath the memory marker
    // and looked like a cue that had not been set.
    //
    // Taken out before the eight are chosen, not after, or the track would
    // come out with seven.
    if let Some(at) =
        kept.iter().position(|(candidate, _)| candidate.time_ms.abs_diff(start_ms) < TOGETHER_MS)
    {
        kept.remove(at);
    }
    kept.retain(|(candidate, _)| !candidate.memory_only);

    for (letter, (candidate, _)) in to_the_buttons(kept).into_iter().enumerate() {
        let color = candidate.reason.color();
        cues.push(
            Cue::hot(letter as u8 + 1, candidate.time_ms)
                .with_comment(&candidate.comment())
                .with_color(color.r, color.g, color.b),
        );
    }
    cues
}

/// Which of the candidates get the eight buttons, in time order.
///
/// Half to the arrangement and half to the words, and then whatever the other
/// kind did not want. The two are different ways of finding your place in a
/// record — the drop is where the floor moves and the hook is where the room
/// sings — and ranking them against each other produced sets that were all of
/// one or all of the other: a busy arrangement took every button and left a
/// track's words uncued, and a wordy record took them all back.
///
/// Neither half is held open. A track with nine sections and no words gets
/// eight sections; an instrumental gets its eight and nothing is left blank
/// that could have been filled. What *is* left blank is what nothing was found
/// for, which is the honest answer — an empty button says "nothing here" and a
/// made-up one says the wrong thing in a booth.
fn to_the_buttons(kept: Vec<(Candidate, u32)>) -> Vec<(Candidate, u32)> {
    let (mut sections, mut words): (Vec<_>, Vec<_>) =
        kept.into_iter().partition(|(candidate, _)| candidate.reason.is_section());

    // Best first within each kind, so taking from the front of each takes the
    // ones worth having. Ties go to the earlier moment.
    let worth = |a: &(Candidate, u32), b: &(Candidate, u32)| {
        b.1.cmp(&a.1).then(a.0.time_ms.cmp(&b.0.time_ms))
    };
    sections.sort_by(worth);
    words.sort_by(worth);

    let from_sections = SHARE.max(HOT_CUES.saturating_sub(words.len())).min(sections.len());
    let from_words = (HOT_CUES - from_sections).min(words.len());
    sections.truncate(from_sections);
    words.truncate(from_words);

    let mut out = sections;
    out.extend(words);
    out.sort_by_key(|(candidate, _)| candidate.time_ms);
    out
}

/// What a section is called on a marker, in the words a DJ uses about a record.
fn section_word(kind: Kind) -> &'static str {
    match kind {
        Kind::Intro => "Intro",
        Kind::Up => "Build",
        Kind::Down => "Break",
        Kind::Chorus => "Drop",
        Kind::Outro => "Outro",
    }
}

/// A named memory cue for every moment, in time order.
///
/// The names are what a DJ would write on the markers themselves: Start, then
/// each section numbered within its own kind — Build 1, Drop 1, Break 1, Drop 2
/// — and End on the last bar. Numbering within the kind rather than across all
/// of them is the whole point: six markers all called "drop" say nothing that
/// looking at the waveform does not.
///
/// A sung line gets a verse number and its words, every time it lands: "V1 Get
/// Down" wherever that line comes round. The number says which line of the
/// record this is without having to read it, and the words say which line it
/// is at a glance — a CDJ-3000X shows the whole comment, so there is no reason
/// to make somebody remember what V1 was.
///
/// `kept` is the folded, time-ordered list [`assemble`] works from.
fn memory_cues(start_ms: u32, end_ms: Option<u32>, kept: &[(Candidate, u32)]) -> Vec<Cue> {
    let mut out = vec![Cue::memory(start_ms).with_comment("Start")];
    let mut sections: Vec<(&'static str, usize)> = Vec::new();
    let mut lines: Vec<String> = Vec::new();
    let mut plain_vocals = 0usize;

    for (candidate, _) in kept {
        // The section that sits on the start is what Start is: naming it twice
        // would put two markers on the same bar, which is one to step over
        // every time the track is loaded. It is still counted, so a second
        // intro reads "Intro 2" rather than starting again at one.
        let at_start = candidate.time_ms.abs_diff(start_ms) < TOGETHER_MS;
        let label = match candidate.reason {
            Reason::Section(kind) => {
                let word = section_word(kind);
                let count = match sections.iter_mut().find(|(seen, _)| *seen == word) {
                    Some((_, count)) => {
                        *count += 1;
                        *count
                    }
                    None => {
                        sections.push((word, 1));
                        1
                    }
                };
                // "Drop 1 · 40 bars". The ordinal says which drop this is
                // and the bars say what kind of drop it is — a 40-bar one is
                // the record's centre and a 16-bar one is a passing lift, and
                // on a deck you are reading the marker rather than counting
                // bars off the waveform.
                match candidate.bars {
                    Some(bars) => format!("{word} {count} · {bars} bars"),
                    None => format!("{word} {count}"),
                }
            }
            _ => match candidate.label.as_deref() {
                Some(text) => {
                    // Numbered by the line it is, not by how many have been
                    // seen: a line that comes round four times is V1 all four
                    // times, and the one after it is V2 rather than V5.
                    let at = match lines
                        .iter()
                        .position(|seen| crate::transcribe::same_line(seen, text))
                    {
                        Some(at) => at,
                        None => {
                            lines.push(text.to_string());
                            lines.len() - 1
                        }
                    };
                    // The words every time, not only the first. A player with
                    // room for them is a player that should show them, and
                    // "V1" on its own asks somebody in a booth to remember
                    // what V1 was.
                    format!("V{} {}", at + 1, lines[at])
                }
                // A voice the mix found and the words did not. Numbered on its
                // own, because calling it V1 would claim it is a line that has
                // been read when nothing has read it.
                None => {
                    plain_vocals += 1;
                    format!("Vocal {plain_vocals}")
                }
            },
        };
        if !at_start {
            out.push(Cue::memory(candidate.time_ms).with_comment(&label));
        }
    }

    // The last bar, so that running out of record is a marker rather than a
    // surprise. Only when it is clear of everything else: a marker half a
    // second after the outro cue is two markers on one moment.
    if let Some(end) = end_ms {
        let clear = out.iter().all(|cue| cue.time_ms + TOGETHER_MS <= end);
        if clear {
            out.push(Cue::memory(end).with_comment("End"));
        }
    }
    out
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
        // A moment that may only ever be a memory cue never takes the place of
        // one that could have been a button, whatever it is worth: the line
        // coming round for the fourth time is worth a marker and is not worth
        // the drop's slot.
        let beaten = match (last.memory_only, candidate.memory_only) {
            (true, false) => true,
            (false, true) => false,
            _ => match (last.reason.is_section(), candidate.reason.is_section()) {
                (true, false) => false,
                (false, true) => true,
                _ => priority > *last_priority,
            },
        };
        // Nor does it lend its words to whatever swallowed it. A drop that
        // happens to land on a hook coming round should still read "drop",
        // because that is what a DJ is reaching for when they are not
        // reaching for the words.
        let (winner_only, loser) = match beaten {
            true => (candidate.memory_only, last.memory_only),
            false => (last.memory_only, candidate.memory_only),
        };
        let lend = !loser && !winner_only;
        match beaten {
            true => {
                let words = last.label.take().filter(|_| lend);
                *last = candidate;
                *last_priority = priority;
                last.label = last.label.take().or(words);
            }
            false => {
                let words = candidate.label.filter(|_| lend);
                last.label = last.label.take().or(words);
            }
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

        // The first section is the memory cue rather than a hot one: the
        // player already parks at the top of the record, so a hot cue there is
        // a button that does what loading the track did. It keeps the name.
        assert!(!cues[0].is_hot());
        assert_eq!(cues[0].time_ms, 0);
        assert_eq!(cues[0].comment.as_deref(), Some("Start"));

        let hot: Vec<&Cue> = cues.iter().filter(|c| c.is_hot()).collect();
        assert_eq!(hot.len(), 2);
        assert_eq!(hot[0].time_ms, 8 * 4 * BEAT_MS);
        assert_eq!(hot[1].time_ms, 16 * 4 * BEAT_MS);
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

        // The intro is on the memory cue, where the player parks anyway.
        assert_eq!(cues[0].comment.as_deref(), Some("Start"));
        assert_eq!(hot[0].comment.as_deref(), Some("break"));
        assert_eq!(hot[1].comment.as_deref(), Some("drop"));
        // The drop is red and the breakdown is blue, and they are not the same.
        assert_ne!(hot[0].color, hot[1].color);
        assert_eq!(hot[1].color, Some(Rgb { r: 0xc0, g: 0x3a, b: 0x22 }));
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

    /// The memory cues a set came out with, in the order they were written.
    fn marks(cues: &[Cue]) -> Vec<String> {
        cues.iter()
            .filter(|cue| !cue.is_hot())
            .map(|cue| cue.comment.clone().unwrap_or_default())
            .collect()
    }

    #[test]
    fn a_section_marker_says_how_long_the_section_runs() {
        // What a player's own phrase strip cannot say. The label there comes
        // from ten fixed strings chosen by the phrase kind and its flag bytes,
        // and the format has no text field to put a number in — so a deck
        // draws "CHORUS 2" for a forty-bar drop and a sixteen-bar one alike.
        // A memory cue's comment is text, and a player shows it.
        let structure = Structure {
            sections: vec![
                section(0, 16, Kind::Intro, -20.0),
                section(16, 56, Kind::Chorus, -6.0),
                section(56, 72, Kind::Chorus, -8.0),
            ],
        };
        let marked = marks(&assemble(0, Some(200_000), sections(&structure, &beats(400))));

        assert!(marked.contains(&"Drop 1 · 40 bars".to_string()), "{marked:?}");
        assert!(marked.contains(&"Drop 2 · 16 bars".to_string()), "{marked:?}");
    }

    #[test]
    fn a_marker_with_no_length_behind_it_is_left_as_it_was() {
        // A moment that is not a section — a hook, a vocal coming in — has no
        // length to report, and inventing one would be worse than the number
        // being absent.
        let candidates = vec![Candidate::new(30_000, Reason::Section(Kind::Chorus))];
        assert_eq!(marks(&assemble(0, Some(200_000), candidates)), ["Start", "Drop 1", "End"]);
    }

    #[test]
    fn sections_are_numbered_within_their_own_kind() {
        // Six markers all reading "drop" say nothing the waveform does not.
        // Numbered within the kind, the list is the arrangement.
        let kinds = [Kind::Intro, Kind::Up, Kind::Chorus, Kind::Down, Kind::Up, Kind::Chorus];
        let candidates = kinds
            .iter()
            .enumerate()
            .map(|(i, &kind)| Candidate::new(i as u32 * 30_000, Reason::Section(kind)))
            .collect();

        assert_eq!(
            marks(&assemble(0, Some(200_000), candidates)),
            ["Start", "Build 1", "Drop 1", "Break 1", "Build 2", "Drop 2", "End"]
        );
    }

    #[test]
    fn the_first_section_is_start_and_is_still_counted() {
        // Start is the intro, so a second intro has to read "Intro 2" — one
        // that started again at one would say there were two first sections.
        let candidates = vec![
            Candidate::new(0, Reason::Section(Kind::Intro)),
            Candidate::new(60_000, Reason::Section(Kind::Intro)),
        ];
        assert_eq!(marks(&assemble(0, None, candidates)), ["Start", "Intro 2"]);
    }

    #[test]
    fn a_sung_line_is_named_wherever_it_lands_and_keeps_its_number() {
        // What a player shows while a track is loaded is the comment on the
        // marker, and a CDJ-3000X shows the whole of it. So every landing says
        // which line it is and what the line was, and the number stays with
        // the line rather than counting the landings.
        let words = crate::transcribe::Transcript {
            lines: vec![
                line(30_000, "get down"),
                line(60_000, "everybody in the room"),
                line(90_000, "get down"),
                line(120_000, "everybody in the room"),
                line(150_000, "get down"),
            ],
            ..Default::default()
        };
        let mut candidates = vec![Candidate::new(0, Reason::Section(Kind::Intro))];
        candidates.extend(words.moments().iter().map(from_moment));

        let marks = marks(&assemble(0, None, candidates));
        assert_eq!(marks[0], "Start");
        assert_eq!(
            marks.iter().filter(|mark| *mark == "V1 get down").count(),
            3,
            "every landing of the line should say which line it is: {marks:?}"
        );
        assert!(
            !marks.iter().any(|mark| *mark == "V1"),
            "a number with no words asks somebody in a booth to remember: {marks:?}"
        );
        assert!(
            marks.iter().any(|mark| mark.starts_with("V2 ")),
            "the other line is another verse: {marks:?}"
        );
    }

    #[test]
    fn a_verse_marker_carries_the_whole_line() {
        let words = crate::transcribe::Transcript {
            lines: vec![
                line(30_000, "everybody in the room put your hands up"),
                line(90_000, "everybody in the room put your hands up"),
            ],
            ..Default::default()
        };
        let mut candidates = vec![Candidate::new(0, Reason::Section(Kind::Intro))];
        candidates.extend(words.moments().iter().map(from_moment));

        let marks = marks(&assemble(0, None, candidates));
        assert!(
            marks.iter().all(|mark| !mark.starts_with('V')
                || mark == "V1 everybody in the room put your hands up"),
            "{marks:?}"
        );
    }

    #[test]
    fn a_line_the_recogniser_wrote_two_ways_is_labelled_one_way() {
        // Close enough to be the same line is close enough to carry the same
        // marker: two spellings of one hook read as two hooks in a booth.
        let words = crate::transcribe::Transcript {
            lines: vec![
                line(30_000, "hold me closer now"),
                line(90_000, "hold me closer, now"),
                line(150_000, "hold me closer now"),
            ],
            ..Default::default()
        };
        let mut candidates = vec![Candidate::new(0, Reason::Section(Kind::Intro))];
        candidates.extend(words.moments().iter().map(from_moment));

        let marks = marks(&assemble(0, None, candidates));
        let verses: Vec<&String> = marks.iter().filter(|mark| mark.starts_with('V')).collect();
        assert!(verses.len() >= 2, "{marks:?}");
        assert!(
            verses.iter().all(|mark| *mark == verses[0]),
            "one line should read one way: {verses:?}"
        );
    }

    #[test]
    fn the_end_marker_is_only_written_where_nothing_else_is() {
        // A marker half a second after the outro cue is two markers on one
        // moment, which is one to step over every time.
        let candidates = vec![Candidate::new(200_000, Reason::Section(Kind::Outro))];
        assert_eq!(marks(&assemble(0, Some(200_100), candidates)), ["Start", "Outro 1"]);
    }

    #[test]
    fn the_hook_is_cued_with_the_words_that_make_it_one() {
        let words = crate::transcribe::Transcript {
            lines: vec![
                line(30_000, "hold me closer now"),
                line(90_000, "hold me closer now"),
                line(150_000, "hold me closer now"),
            ],
            ..Default::default()
        };
        let mut candidates = vec![Candidate::new(0, Reason::Section(Kind::Intro))];
        candidates.extend(words.moments().iter().map(from_moment));
        let cues = assemble(0, None, candidates);

        let hot: Vec<&Cue> = cues.iter().filter(|c| c.is_hot()).collect();
        let hook = hot.iter().find(|c| c.time_ms == 30_000).expect("no cue on the hook");
        assert_eq!(hook.comment.as_deref(), Some("hold me closer now"));
        // Once, not once per airing. Eight cues spent three times over on the
        // same words leaves the drops nowhere to go.
        assert_eq!(
            hot.iter().filter(|c| c.comment.as_deref() == hook.comment.as_deref()).count(),
            1,
            "{hot:?}"
        );
    }

    #[test]
    fn the_slots_the_words_give_back_go_to_the_arrangement() {
        // The point of the whole change. A track whose hook comes round five
        // times, with an arrangement to match: before, the words took six of
        // the eight and the breaks and builds were pushed out.
        let words = crate::transcribe::Transcript {
            lines: (0..5).map(|i| line(30_000 + i * 60_000, "hold me closer now")).collect(),
            ..Default::default()
        };
        let kinds = [
            Kind::Intro,
            Kind::Up,
            Kind::Chorus,
            Kind::Down,
            Kind::Chorus,
            Kind::Down,
            Kind::Outro,
        ];
        let mut candidates: Vec<Candidate> = kinds
            .iter()
            .enumerate()
            .map(|(i, &kind)| Candidate::new(i as u32 * 45_000, Reason::Section(kind)))
            .collect();
        candidates.extend(words.moments().iter().map(from_moment));

        let hot: Vec<Cue> = assemble(0, None, candidates).into_iter().filter(Cue::is_hot).collect();
        let named = |what: &str| hot.iter().filter(|c| c.comment.as_deref() == Some(what)).count();

        assert_eq!(named("hold me closer now"), 1, "the hook took more than one slot: {hot:?}");
        // And the arrangement got the rest, saying what it is: a drop that
        // happened to land on a hook return still reads "drop", because that
        // is what a DJ is reaching for when they are not reaching for the
        // words.
        assert_eq!(named("drop"), 2, "{hot:?}");
        assert_eq!(named("break"), 2, "{hot:?}");
    }

    #[test]
    fn the_hook_survives_a_track_with_more_moments_than_a_player_has_cues() {
        // Ten sections, all of them loud, plus a hook halfway through. Eight
        // slots; the hook takes one of them whatever else is going on.
        let mut candidates: Vec<Candidate> =
            (0..10).map(|i| Candidate::new(i * 20_000, Reason::Section(Kind::Chorus))).collect();
        candidates.push(Candidate::named(95_000, Reason::Hook, "hold me closer now"));

        let cues = assemble(0, None, candidates);
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
        let cues = assemble(0, None, candidates);

        let hot: Vec<&Cue> = cues.iter().filter(|c| c.is_hot()).collect();
        assert_eq!(hot.len(), 1, "two markers half a second apart: {hot:?}");
        // The section keeps the moment — it is on the grid — and the words
        // come with it.
        assert_eq!(hot[0].time_ms, 60_000);
        assert_eq!(hot[0].comment.as_deref(), Some("hold me closer now"));
        assert_eq!(hot[0].color, Some(Reason::Section(Kind::Chorus).color()));
    }

    /// Six lines with nothing in common, so the grouping keeps them apart and
    /// the track genuinely repeats nothing. Lines that differ by one word do
    /// not do: the whole point of the grouping is that those are one line.
    const VERSE: [&str; 6] = [
        "walking through the city at night",
        "nobody told me it would end",
        "a coat on the back of a chair",
        "every window on the eighteenth floor",
        "she said wait for the rain",
        "counting the stops to the river",
    ];

    /// The hot cues a set came out with, in time order.
    fn buttons(cues: &[Cue]) -> Vec<String> {
        cues.iter()
            .filter(|cue| cue.is_hot())
            .map(|cue| cue.comment.clone().unwrap_or_default())
            .collect()
    }

    #[test]
    fn a_track_where_nothing_repeats_still_comes_back_with_lyric_cues() {
        // The fault: a rap, a live take, a record with one verse and no
        // chorus has no hook and no refrains, so the words came back with one
        // marker for the voice arriving and nothing else. Its lines are what
        // it has.
        let words = crate::transcribe::Transcript {
            lines: VERSE
                .iter()
                .enumerate()
                .map(|(i, text)| line(20_000 + i as u32 * 40_000, text))
                .collect(),
            ..Default::default()
        };
        let mut candidates = vec![
            Candidate::new(0, Reason::Section(Kind::Intro)),
            Candidate::new(100_000, Reason::Section(Kind::Chorus)),
        ];
        candidates.extend(words.moments().iter().map(from_moment));

        let hot = buttons(&assemble(0, None, candidates));
        let sung = hot.iter().filter(|name| VERSE.contains(&name.as_str())).count();
        assert!(sung >= 4, "the spare buttons should have gone to the words: {hot:?}");
        assert!(hot.len() <= HOT_CUES, "{hot:?}");
    }

    #[test]
    fn the_buttons_are_shared_between_the_arrangement_and_the_words() {
        // Four and four where there are four of each to be had. Ranking them
        // against one another gave sets that were all of one or all of the
        // other: a busy arrangement took every button and left the words
        // uncued.
        let words = crate::transcribe::Transcript {
            lines: VERSE
                .iter()
                .enumerate()
                .map(|(i, text)| line(25_000 + i as u32 * 40_000, text))
                .collect(),
            ..Default::default()
        };
        let kinds = [Kind::Intro, Kind::Up, Kind::Chorus, Kind::Down, Kind::Chorus, Kind::Down];
        let mut candidates: Vec<Candidate> = kinds
            .iter()
            .enumerate()
            .map(|(i, &kind)| Candidate::new(i as u32 * 45_000 + 2_000, Reason::Section(kind)))
            .collect();
        candidates.extend(words.moments().iter().map(from_moment));

        let hot = buttons(&assemble(0, None, candidates));
        let sung = hot.iter().filter(|name| VERSE.contains(&name.as_str())).count();
        assert_eq!(hot.len(), HOT_CUES, "{hot:?}");
        assert_eq!(sung, SHARE, "the words should have had half: {hot:?}");
        assert_eq!(hot.len() - sung, SHARE, "and the arrangement the other half: {hot:?}");
    }

    #[test]
    fn an_instrumental_gets_every_button_for_its_arrangement() {
        // Neither half is held open. There is nothing to put in the words'
        // share, so the arrangement has it.
        let kinds = [
            Kind::Intro,
            Kind::Up,
            Kind::Chorus,
            Kind::Down,
            Kind::Up,
            Kind::Chorus,
            Kind::Down,
            Kind::Chorus,
            Kind::Outro,
        ];
        let candidates: Vec<Candidate> = kinds
            .iter()
            .enumerate()
            .map(|(i, &kind)| Candidate::new(i as u32 * 30_000 + 1_000, Reason::Section(kind)))
            .collect();

        let hot = buttons(&assemble(0, None, candidates));
        assert_eq!(hot.len(), HOT_CUES, "{hot:?}");
    }

    #[test]
    fn a_track_with_little_to_cue_leaves_the_rest_of_the_buttons_empty() {
        // Rather than filling them with something made up. An empty button
        // says "nothing here"; a made-up one says the wrong thing in a booth.
        let candidates = vec![
            Candidate::new(30_000, Reason::Section(Kind::Chorus)),
            Candidate::new(90_000, Reason::Section(Kind::Down)),
        ];
        let hot = buttons(&assemble(0, None, candidates));
        assert_eq!(hot.len(), 2, "{hot:?}");
    }

    #[test]
    fn a_line_only_fills_a_button_nothing_better_wanted() {
        // A hook and two refrains is already a full share, so the one-off
        // lines stay markers.
        let mut lines: Vec<crate::transcribe::Line> =
            (0..3).map(|i| line(20_000 + i * 90_000, "hold me closer now")).collect();
        lines.extend((0..3).map(|i| line(50_000 + i * 90_000, "and the night comes down")));
        lines.push(line(300_000, "something said once and never again"));
        let words = crate::transcribe::Transcript { lines, ..Default::default() };

        let mut candidates = vec![Candidate::new(0, Reason::Section(Kind::Intro))];
        candidates.extend(words.moments().iter().map(from_moment));

        let hot = buttons(&assemble(0, None, candidates));
        assert!(hot.iter().any(|name| name == "hold me closer now"), "{hot:?}");
        assert!(hot.iter().any(|name| name == "and the night comes down"), "{hot:?}");
        // And it is still a marker, which costs nothing.
        let marks = marks(&assemble(0, None, {
            let mut again = vec![Candidate::new(0, Reason::Section(Kind::Intro))];
            again.extend(words.moments().iter().map(from_moment));
            again
        }));
        assert!(marks.iter().any(|mark| mark.contains("said once")), "{marks:?}");
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
