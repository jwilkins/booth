//! Putting a transcript back where the singing actually is.
//!
//! # Why this exists
//!
//! Whisper's timestamps are not reliable on a vocal stem, and they are wrong in
//! a specific, measurable way: **it anchors its first segment at zero however
//! much silence comes before the first word.** Measured on a stem-shaped file —
//! speech, a long gap, more speech:
//!
//! | phrase | really at | whisper said | out by |
//! | --- | --- | --- | --- |
//! | first | 4,000 ms | 0 ms | −4,000 |
//! | second | 10,044 ms | 10,160 ms | +116 |
//!
//! Varying the leading silence moves the error with it exactly — 1 s of lead
//! gives a 1 s error, 6 s gives 6 s — while everything after the first phrase
//! lands within about a tenth of a second.
//!
//! A vocal stem is the worst possible input for that bug, because a stem *is*
//! mostly silence: the whole instrumental intro comes out as nothing, and the
//! hook is usually inside that first segment. So the cue for the line the crowd
//! sings lands seconds before anybody sings it.
//!
//! # What it does instead
//!
//! The stem is the voice, so where the voice comes in is not something to take
//! anybody's word for — it is the loudest thing in the file and it can be
//! measured. Each line is moved to the onset of the singing it belongs to:
//! back to the start of the phrase it lands inside, or forward to the next one
//! if it lands in a silence.
//!
//! Both halves of the measurement above come out right under that rule: 0 falls
//! in the silence before the first phrase and moves forward to 4,000; 10,160
//! falls inside the second phrase and moves back to its start at 10,044. The
//! second case is the one that stops a cue clipping its own first word.

use crate::audio::Audio;

use super::{Line, Transcript};

/// How long a stretch of sound has to last to be singing rather than a click.
///
/// Shorter than a syllable. The point is to throw away separation artefacts —
/// a stem carries a ghost of the snare that was taken out of it — without
/// losing a short sung word.
const MIN_VOICED_MS: u32 = 120;

/// A silence shorter than this is inside a phrase rather than between two.
///
/// Words have gaps between them, and a rule that started a new phrase at every
/// gap would put the onset of a line in the middle of it. Half a second is
/// longer than the gap between words and shorter than the gap between lines.
const BRIDGE_MS: u32 = 500;

/// How far below the loud parts something can be and still count as singing.
///
/// Measured against the track's own range rather than an absolute level,
/// because a stem is whatever the separator left and its floor is not zero.
const FLOOR_OF_PEAK: f32 = 0.04;

/// How far a line may be moved. Beyond this, the measurement and the transcript
/// disagree about something other than timing, and moving the cue that far
/// would be guessing at which phrase was meant.
const REACH_MS: u32 = 30_000;

/// The stretches of a stem that have singing in them, in milliseconds.
/// Whether a stem has any singing on it at all.
///
/// A separation of an instrumental still produces a vocal stem — what is in it
/// is bleed, a ghost of the snare, the tail of a reverb — and handing that to
/// a recogniser costs a minute or two for a transcript of things nobody sang.
/// Worse than nothing, in fact: a recogniser given something that is not
/// speech does not hand back nothing, it hallucinates, and "thanks for
/// watching" is the sentence it writes. There is already code downstream
/// whose whole job is to not believe that.
///
/// Measured absolutely rather than against the stem's own range, which is the
/// whole difference from [`voiced_spans`]: that asks which parts of a stem are
/// its loud ones and finds some in anything, where this asks whether the loud
/// ones are loud enough to be a voice.
pub fn has_singing(audio: &Audio) -> bool {
    loudest(audio) > QUIETEST_VOICE
}

/// How loud a stem gets, as a fraction of full scale.
///
/// Taken near the top rather than at the very top, so one separation artefact
/// cannot answer for the whole file.
fn loudest(audio: &Audio) -> f32 {
    let mono = audio.to_mono();
    if mono.is_empty() {
        return 0.0;
    }
    let mut levels: Vec<f32> = mono.iter().map(|s| s.abs()).collect();
    levels.sort_by(f32::total_cmp);
    levels[levels.len() * 999 / 1000]
}

/// The quietest a stem's loud parts can be and still hold a voice: −40 dBFS.
///
/// A sung vocal separated out of a mix sits far above this — it is the loudest
/// thing in the file by design, which is what a separator was asked for. What
/// sits below it is what the separator could not remove.
///
/// Picked rather than measured against a library of real instrumentals, which
/// is the honest limit of it: it is well clear of both cases as synthesised
/// here, and it is one number to move if a real stem lands on the wrong side.
const QUIETEST_VOICE: f32 = 0.01;

pub fn voiced_spans(audio: &Audio) -> Vec<(u32, u32)> {
    const FRAME_MS: u32 = 20;
    let mono = audio.to_mono();
    if mono.is_empty() || audio.sample_rate == 0 {
        return Vec::new();
    }
    let per_frame = (audio.sample_rate as usize * FRAME_MS as usize / 1000).max(1);

    let levels: Vec<f32> = mono
        .chunks(per_frame)
        .map(|frame| {
            let sum: f32 = frame.iter().map(|s| s * s).sum();
            (sum / frame.len() as f32).sqrt()
        })
        .collect();

    // The threshold is a fraction of how loud this stem gets, taken near the
    // top rather than at the very top so one clipped frame cannot set it.
    let mut sorted = levels.clone();
    sorted.sort_by(f32::total_cmp);
    let loud = sorted[sorted.len() * 95 / 100];
    if loud <= f32::EPSILON {
        return Vec::new();
    }
    let floor = loud * FLOOR_OF_PEAK;

    let mut spans: Vec<(u32, u32)> = Vec::new();
    let mut open: Option<u32> = None;
    for (at, level) in levels.iter().enumerate() {
        let time = at as u32 * FRAME_MS;
        match (*level > floor, open) {
            (true, None) => open = Some(time),
            (false, Some(from)) => {
                spans.push((from, time));
                open = None;
            }
            _ => {}
        }
    }
    if let Some(from) = open {
        spans.push((from, levels.len() as u32 * FRAME_MS));
    }

    // Join what is one phrase with gaps in it, then drop what is too short to
    // be singing at all. In that order: three clicks in a row are still not a
    // phrase, and a word split by a breath is.
    let mut joined: Vec<(u32, u32)> = Vec::with_capacity(spans.len());
    for (from, to) in spans {
        match joined.last_mut() {
            Some(last) if from.saturating_sub(last.1) < BRIDGE_MS => last.1 = to,
            _ => joined.push((from, to)),
        }
    }
    joined.retain(|(from, to)| to.saturating_sub(*from) >= MIN_VOICED_MS);
    joined
}

/// Move every line to the onset of the singing it belongs to.
///
/// A line inside a phrase goes back to that phrase's start, which is what stops
/// a cue clipping its own first word. A line in a silence goes forward to the
/// next phrase, which is what fixes the first segment's missing lead-in. A line
/// past the last phrase is left where it is: there is nothing to move it to.
pub fn to_the_voice(transcript: &mut Transcript, spans: &[(u32, u32)]) {
    if spans.is_empty() {
        return;
    }
    for line in &mut transcript.lines {
        let Some(moved) = onset_for(line.start_ms, spans) else { continue };
        if moved.abs_diff(line.start_ms) > REACH_MS {
            continue;
        }
        // The line keeps its length rather than its end, so a line moved
        // forward does not come out as one that ends before it starts.
        let ran_for = line.end_ms.saturating_sub(line.start_ms);
        line.start_ms = moved;
        line.end_ms = moved.saturating_add(ran_for);
    }
    // Moving lines can put them out of order where two landed in one phrase.
    transcript.lines.sort_by_key(|line: &Line| line.start_ms);
}

/// The onset this moment belongs to: the phrase it is inside, or the next one.
fn onset_for(at: u32, spans: &[(u32, u32)]) -> Option<u32> {
    if let Some((from, _)) = spans.iter().find(|(from, to)| at >= *from && at < *to) {
        return Some(*from);
    }
    spans.iter().map(|(from, _)| *from).find(|from| *from > at)
}

/// Read the words off a stem and put them where the singing is, in one step.
pub fn aligned(mut transcript: Transcript, audio: &Audio) -> Transcript {
    to_the_voice(&mut transcript, &voiced_spans(audio));
    transcript
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stem-shaped file: a burst of sound at the given level, then silence.
    fn stem_at(level: f32) -> Audio {
        const RATE: u32 = 16_000;
        let mut plane = vec![0.0f32; RATE as usize * 4];
        for (i, sample) in plane.iter_mut().enumerate().take(RATE as usize * 2) {
            let t = i as f32 / RATE as f32;
            *sample = level * (2.0 * std::f32::consts::PI * 220.0 * t).sin();
        }
        Audio::new(RATE, vec![plane.clone(), plane]).unwrap()
    }

    #[test]
    fn a_stem_with_a_voice_on_it_is_worth_a_recogniser() {
        // What a separated vocal looks like: the loudest thing in the file,
        // because that is what the separator was asked for.
        assert!(has_singing(&stem_at(0.5)));
        assert!(has_singing(&stem_at(0.1)));
    }

    #[test]
    fn a_stem_that_is_only_what_the_separator_could_not_remove_is_not() {
        // An instrumental still produces a vocal stem. What is in it is bleed
        // — a ghost of the snare, the tail of a reverb — tens of dB below a
        // voice, and a recogniser handed it does not return nothing: it
        // hallucinates.
        assert!(!has_singing(&stem_at(0.003)));
        assert!(!has_singing(&stem_at(0.0)));
    }

    #[test]
    fn the_question_is_how_loud_and_not_which_parts_are_loudest() {
        // The difference from `voiced_spans`, which measures against the
        // stem's own range and so finds something in anything — including in
        // bleed, which is why it cannot answer this on its own.
        let bleed = stem_at(0.003);
        assert!(!voiced_spans(&bleed).is_empty(), "the relative measure finds spans in bleed");
        assert!(!has_singing(&bleed), "and the absolute one does not call it a voice");
    }

    /// Tone bursts and silence, which is the shape of a vocal stem.
    fn stem(bursts: &[(f32, f32)], seconds: f32) -> Audio {
        const RATE: u32 = 16_000;
        let mut plane = vec![0.0f32; (RATE as f32 * seconds) as usize];
        for (from, to) in bursts {
            let a = (from * RATE as f32) as usize;
            let b = ((to * RATE as f32) as usize).min(plane.len());
            for (i, sample) in plane[a..b].iter_mut().enumerate() {
                let t = i as f32 / RATE as f32;
                *sample = 0.5 * (t * 440.0 * std::f32::consts::TAU).sin();
            }
        }
        Audio::new(RATE, vec![plane]).unwrap()
    }

    fn said(times: &[(u32, u32)]) -> Transcript {
        Transcript {
            lines: times
                .iter()
                .map(|(start, end)| Line {
                    start_ms: *start,
                    end_ms: *end,
                    text: "something".into(),
                })
                .collect(),
        }
    }

    #[test]
    fn the_singing_is_found_where_it_is_and_not_where_it_is_not() {
        let spans = voiced_spans(&stem(&[(4.0, 5.0), (10.0, 11.5)], 13.0));
        assert_eq!(spans.len(), 2, "{spans:?}");
        // Within a frame of where the bursts really are.
        assert!(spans[0].0.abs_diff(4_000) <= 40, "{spans:?}");
        assert!(spans[1].0.abs_diff(10_000) <= 40, "{spans:?}");
    }

    #[test]
    fn words_inside_one_phrase_are_one_phrase() {
        // Gaps between words are not gaps between lines, so a phrase spoken
        // with breaths in it has one onset and not four.
        let spans = voiced_spans(&stem(&[(2.0, 2.4), (2.6, 3.0), (3.2, 3.6), (9.0, 9.8)], 11.0));
        assert_eq!(spans.len(), 2, "{spans:?}");
        assert!(spans[0].0.abs_diff(2_000) <= 40, "{spans:?}");
    }

    #[test]
    fn a_click_is_not_singing() {
        // A stem carries a ghost of what was taken out of it. One frame of it
        // is not a phrase, and a cue placed on one is a cue on a snare.
        let spans = voiced_spans(&stem(&[(1.0, 1.03), (5.0, 6.0)], 8.0));
        assert_eq!(spans.len(), 1, "{spans:?}");
        assert!(spans[0].0.abs_diff(5_000) <= 40, "{spans:?}");
    }

    #[test]
    fn the_first_line_is_moved_to_where_the_singing_starts() {
        // The measured failure, in the numbers it was measured with: whisper
        // anchors its first segment at zero whatever the lead-in, so the cue
        // for the hook lands seconds before anybody sings.
        let spans = [(4_000, 5_044), (10_044, 11_088)];
        let mut transcript = said(&[(0, 1_044), (10_160, 11_200)]);
        to_the_voice(&mut transcript, &spans);

        assert_eq!(transcript.lines[0].start_ms, 4_000, "the lead-in was not put back");
        // And the second one, which whisper had a tenth of a second late,
        // comes back to the start of its own phrase rather than clipping it.
        assert_eq!(transcript.lines[1].start_ms, 10_044);
    }

    #[test]
    fn a_line_keeps_how_long_it_ran_for() {
        let mut transcript = said(&[(0, 900)]);
        to_the_voice(&mut transcript, &[(4_000, 5_000)]);
        assert_eq!((transcript.lines[0].start_ms, transcript.lines[0].end_ms), (4_000, 4_900));
    }

    #[test]
    fn a_line_nothing_can_be_found_for_is_left_alone() {
        // Past the last phrase there is nothing to move it to, and more than
        // half a minute away the two are disagreeing about something other
        // than timing — moving it that far would be picking a phrase at random.
        let mut transcript = said(&[(90_000, 91_000), (200, 1_200)]);
        to_the_voice(&mut transcript, &[(1_000, 2_000), (60_000, 61_000)]);
        assert_eq!(transcript.lines[1].start_ms, 90_000, "nothing comes after it");
        assert_eq!(transcript.lines[0].start_ms, 1_000, "and this one is close enough to move");
    }

    /// Run the detector over a real recording, for checking it against ears.
    ///
    /// Ignored because it needs a file nobody else has. Point it at anything
    /// with singing on it and it prints where it thinks each phrase begins:
    ///
    ///     BOOTH_STEM_WAV=/path/to/vocals.wav \
    ///         cargo test -p booth-cli -- --ignored where_the_singing_is
    #[test]
    #[ignore = "needs a vocal recording to point at"]
    fn where_the_singing_is_in_a_real_recording() {
        let path = std::env::var("BOOTH_STEM_WAV").expect("set BOOTH_STEM_WAV to a vocal file");
        let audio = crate::audio::decode::decode_file(std::path::Path::new(&path))
            .expect("that file should decode");
        let spans = voiced_spans(&audio);
        println!("{} phrases in {}", spans.len(), path);
        for (from, to) in &spans {
            println!("  {from:>8} ms \u{2192} {to:>8} ms  ({} ms)", to - from);
        }
        assert!(!spans.is_empty(), "nothing was heard at all");
    }

    #[test]
    fn a_stem_with_nothing_on_it_changes_nothing() {
        let mut transcript = said(&[(1_000, 2_000)]);
        to_the_voice(&mut transcript, &voiced_spans(&stem(&[], 5.0)));
        assert_eq!(transcript.lines[0].start_ms, 1_000);
    }
}
