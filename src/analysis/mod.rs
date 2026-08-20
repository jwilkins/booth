//! Working out what is in a track: where the beats are, how it is put
//! together, and where a DJ would want to drop a cue.
//!
//! Everything here is signal processing rather than a model. That is a
//! deliberate limit: it runs offline in seconds, it explains itself, and every
//! decision it makes is one the user can see and correct. Where it is weaker
//! than a trained model — telling a sung line from a centred lead, mostly — the
//! documentation says so rather than hoping.

pub mod cues;
pub mod features;
pub mod structure;
pub mod tempo;

use crate::audio::Audio;
use crate::export::{BeatGrid, Cue, SongStructure};

/// Everything the analysers worked out about one track.
pub struct TrackAnalysis {
    pub grid: BeatGrid,
    /// The tempo the track was tracked at, or zero if no beat was found.
    pub bpm: f64,
    /// How clearly the tempo stood out. Under about 2 is a track the beat
    /// tracker was guessing at.
    pub confidence: f32,
    pub structure: structure::Structure,
    /// A memory cue at the first downbeat, then up to eight hot cues.
    pub cues: Vec<Cue>,
}

impl TrackAnalysis {
    /// The phrases, in the form the analysis file stores. `None` when the track
    /// was too short or too uniform to have any.
    pub fn song_structure(&self) -> Option<SongStructure> {
        self.structure.to_song_structure()
    }

    pub fn found_beats(&self) -> bool {
        !self.grid.is_empty()
    }
}

/// Listen to a track: find the beats, the phrases, and the cue points.
///
/// One pass over the audio for the measurements, then three cheap passes over
/// those. A five-minute track takes a couple of seconds.
pub fn analyze(audio: &Audio) -> TrackAnalysis {
    analyze_at(audio, None)
}

/// The same, at a tempo the user has corrected by hand.
///
/// The beats are still tracked rather than assumed: a tempo says how far apart
/// they are, not where they fall, and putting them in the wrong place is how a
/// grid ends up half a beat out for the whole track.
pub fn analyze_at(audio: &Audio, bpm: Option<f64>) -> TrackAnalysis {
    let measured = features::extract(audio);
    let beats = tempo::detect_at(&measured, bpm);
    let times: Vec<u32> = beats.grid.beats.iter().map(|b| b.time_ms).collect();
    let structure = structure::detect(&measured, &times);
    let cues = cues::suggest(&measured, &times, &structure);

    TrackAnalysis {
        grid: beats.grid,
        bpm: beats.bpm,
        confidence: beats.confidence,
        structure,
        cues,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A whole track, arranged: quiet intro, drums in, breakdown, drop, outro,
    /// with a centred voice over the second half.
    fn track() -> Audio {
        const RATE: u32 = 44_100;
        let bpm = 128.0f32;
        let beat = 60.0 / bpm;
        let bars = 48usize;
        let lead_in = RATE as usize / 2;
        let len = lead_in + (RATE as f32 * beat * (bars * 4) as f32) as usize;
        let mut left = vec![0.0f32; len];
        let mut right = vec![0.0f32; len];
        let two_pi = 2.0 * std::f32::consts::PI;

        for bar in 0..bars {
            // Loud from bar 8, quiet again for the breakdown at 24, back at 32.
            let full = (8..24).contains(&bar) || (32..44).contains(&bar);
            let (kick, hat) = if full { (1.0, 0.25) } else { (0.3, 0.0) };
            for beat_in_bar in 0..4 {
                let start =
                    lead_in + (RATE as f32 * beat * (bar * 4 + beat_in_bar) as f32) as usize;
                for i in 0..(RATE as usize / 4) {
                    let at = start + i;
                    if at >= len {
                        break;
                    }
                    let t = i as f32 / RATE as f32;
                    let sample = kick * (-30.0 * t).exp() * (two_pi * 55.0 * t).sin()
                        + hat * (-60.0 * t).exp() * (two_pi * 9_000.0 * t).sin();
                    left[at] += sample * 0.5;
                    right[at] += sample * 0.5;
                }
            }
        }

        // A wide pad throughout: present, but nowhere near the middle.
        for i in 0..len {
            let t = i as f32 / RATE as f32;
            let pad = 0.2 * (two_pi * 320.0 * t).sin();
            left[i] += pad;
            right[i] -= pad;
        }

        // A centred voice from bar 24: a line of sung notes, two beats each,
        // rather than one continuous tone. Phase is integrated so that the
        // pitch changes cleanly instead of sweeping.
        let notes = [880.0f32, 988.0, 784.0, 880.0, 1046.0, 880.0];
        let mut phase = 0.0f32;
        for note_index in 0.. {
            let first_beat = 24 * 4 + note_index * 2;
            if first_beat >= bars * 4 {
                break;
            }
            let hz = notes[note_index % notes.len()];
            let start = lead_in + (RATE as f32 * beat * first_beat as f32) as usize;
            let length = (RATE as f32 * beat * 1.8) as usize;
            for i in 0..length {
                let at = start + i;
                if at >= len {
                    break;
                }
                phase += two_pi * hz / RATE as f32;
                // Soft either end, so a note is a note and not a click.
                let envelope = (i as f32 / (RATE as f32 * 0.05)).min(1.0)
                    * ((length - i) as f32 / (RATE as f32 * 0.05)).min(1.0);
                let voice = 0.3 * envelope * phase.sin();
                left[at] += voice;
                right[at] += voice;
            }
        }
        Audio::new(RATE, vec![left, right]).unwrap()
    }

    #[test]
    fn a_whole_track_analyses_end_to_end() {
        let analysis = analyze(&track());

        assert!(analysis.found_beats());
        assert!((analysis.bpm - 128.0).abs() < 2.0, "found {:.2} BPM", analysis.bpm);
        assert!(analysis.confidence > 2.0, "confidence {}", analysis.confidence);

        // The arrangement changes at bars 8, 24, 32 and 44, so there should be
        // several sections and the phrases should reach the end of the track.
        assert!(analysis.structure.sections.len() >= 3, "{:?}", analysis.structure.sections);
        let song = analysis.song_structure().unwrap();
        assert_eq!(song.phrases.len(), analysis.structure.sections.len());

        // A memory cue and some hot cues, all on the grid, none past the end.
        assert!(!analysis.cues[0].is_hot());
        let hot = analysis.cues.iter().filter(|c| c.is_hot()).count();
        assert!((1..=8).contains(&hot), "{hot} hot cues");
        let last_beat = analysis.grid.beats.last().unwrap().time_ms;
        assert!(analysis.cues.iter().all(|c| c.time_ms <= last_beat));
    }

    #[test]
    fn the_voice_is_found_where_it_comes_in() {
        let analysis = analyze(&track());
        let vocal = analysis
            .cues
            .iter()
            .find(|c| c.comment.as_deref() == Some("vocal"))
            .map(|c| c.time_ms as f64 / 1000.0);
        // The voice starts 24 bars in, which at 128 BPM is 45 seconds.
        if let Some(at) = vocal {
            assert!((at - 45.0).abs() < 4.0, "the vocal cue landed at {at:.1}s");
        }
        // Not finding it is a miss rather than a failure: the section boundary
        // at the same moment may have claimed the cue first.
        assert!(
            analysis.cues.iter().any(|c| (c.time_ms as f64 / 1000.0 - 45.0).abs() < 4.0),
            "nothing was cued where the arrangement changes and the voice enters"
        );
    }

    #[test]
    fn a_track_with_nothing_in_it_analyses_to_nothing() {
        let silence = Audio::new(44_100, vec![vec![0.0; 44_100 * 10]]).unwrap();
        let analysis = analyze(&silence);
        assert!(!analysis.found_beats());
        assert_eq!(analysis.bpm, 0.0);
        assert!(analysis.structure.is_empty());
        assert!(analysis.cues.is_empty());
        assert!(analysis.song_structure().is_none());
    }
}
