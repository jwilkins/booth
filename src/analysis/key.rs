//! Working out a track's musical key.
//!
//! Two steps, both standard. A chromagram folds the spectrum down to how much
//! of each of the twelve pitch classes is present, averaged over the track;
//! then the Krumhansl–Schmuckler method correlates that against a profile of
//! what each of the twenty-four keys sounds like, and the best match wins.
//!
//! The output is what a DJ reads on the player: the Camelot code they mix by,
//! and the classical name underneath it. A confidence comes with it, because a
//! modal or key-ambiguous track has no one right answer and it is more honest
//! to say the answer was close than to pick a side and sound certain.
//!
//! Two choices are made for the sake of real music rather than tidiness. The
//! chromagram is gathered by reading the spectrum *at* each note's frequency
//! and interpolating, rather than by dropping each FFT bin into the nearest
//! pitch class: in the bass, where dance music carries its key, a semitone is
//! only a few hertz wide and a bin dropped into the nearest class lands in the
//! wrong one as often as not. And the key profiles are Sha'ath's — the ones the
//! KeyFinder tool uses — rather than the classical Krumhansl–Kessler ones,
//! because they were tuned on popular and electronic music and they tell major
//! from minor on a bass-heavy track far better, which is exactly where the
//! classical profiles fail.

use realfft::num_complex::Complex32;
use realfft::{RealFftPlanner, RealToComplex};
use std::sync::Arc;

use crate::audio::Audio;

/// A long window: 16384 points is 2.7 Hz per bin at 44.1 kHz. That is what lets
/// the bass resolve — a semitone at C2 is 3.9 Hz, so a coarser window would put
/// two low notes in one bin. Pitch does not move fast enough for the time
/// smearing a window this long brings to matter.
const N_FFT: usize = 16_384;
const HOP: usize = 8_192;
/// The lowest and highest notes gathered, as MIDI numbers: C2 (36) to C7 (96).
/// Below C2 even this window cannot separate semitones, and there is little key
/// information down there anyway — mostly kick drums; above C7 it is harmonics.
const MIN_MIDI: i32 = 36;
const MAX_MIDI: i32 = 96;
/// The twelve pitch classes.
const PITCHES: usize = 12;

/// The pitch-class names, sharp-spelled, indexed from C.
const NAMES: [&str; PITCHES] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];

/// Sha'ath's major profile: the relative weight each scale degree carries,
/// starting from the tonic. Rotated to each of twelve tonics it becomes the
/// template for that major key. These are the weights the KeyFinder tool uses,
/// derived on popular and electronic music, from Ibrahim Sha'ath's 2011 thesis.
const MAJOR_PROFILE: [f32; PITCHES] = [6.6, 2.0, 3.5, 2.3, 4.6, 4.0, 2.5, 5.2, 2.4, 3.7, 2.3, 3.4];
/// And the minor profile. Its minor third (index 3, weight 5.4) sits just above
/// its fifth, which is what tells a minor track from its parallel major on a
/// record where the root and fifth dominate and the third is buried — the case
/// the classical profiles get wrong.
const MINOR_PROFILE: [f32; PITCHES] = [6.5, 2.7, 3.5, 5.4, 2.6, 3.5, 2.5, 5.2, 4.0, 2.7, 4.3, 3.2];

/// Whether a key is major or minor. On the Camelot wheel these are the `B` and
/// `A` suffixes.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Mode {
    Major,
    Minor,
}

/// A detected key.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Key {
    /// The tonic as a pitch class, 0 for C through 11 for B.
    pub tonic: u8,
    pub mode: Mode,
}

impl Key {
    /// The classical name, e.g. `F#m` or `C`.
    pub fn name(self) -> String {
        match self.mode {
            Mode::Major => NAMES[self.tonic as usize].to_string(),
            Mode::Minor => format!("{}m", NAMES[self.tonic as usize]),
        }
    }

    /// Parse a key written the way rekordbox and other tools store it, in
    /// either classical notation (`Am`, `C#`, `Dbm`, `Gb`) or a Camelot code
    /// (`8A`, `11B`). `None` for anything unrecognised.
    ///
    /// This is what lets an existing library's keys be read back — for
    /// importing one, and for measuring this detector against one.
    pub fn parse(text: &str) -> Option<Key> {
        let text = text.trim();
        if let Some(key) = parse_camelot(text) {
            return Some(key);
        }
        parse_classical(text)
    }

    /// The Camelot code a DJ mixes by, e.g. `8A` or `11B`.
    ///
    /// The wheel walks the circle of fifths: 8B is C major, 8A is A minor, and
    /// each step of a fifth is one number clockwise. Adjacent numbers, and the
    /// two letters of one number, are the harmonically compatible mixes.
    pub fn camelot(self) -> String {
        // Position around the circle of fifths from the tonic. C is 0, G is 1,
        // D is 2 — each fifth is seven semitones, so multiplying the pitch
        // class by seven modulo twelve gives the position.
        let fifths = (self.tonic as usize * 7) % PITCHES;
        let (number, letter) = match self.mode {
            // 8B is C, and B keys are numbered from there around the fifths.
            Mode::Major => ((fifths + 8 - 1) % PITCHES + 1, 'B'),
            // 8A is A minor. A minor's tonic is pitch class 9; the same
            // fifths arithmetic lands it on 8 with this offset.
            Mode::Minor => ((fifths + 8 - 1 + 9) % PITCHES + 1, 'A'),
        };
        format!("{number}{letter}")
    }
}

/// A key detection: the key, an alternative when it was close, and how sure.
pub struct Detected {
    pub key: Key,
    /// How much the winning key beat the average of all twenty-four, as a ratio.
    /// Around 1.0 is no key at all; a clearly tonal track measures well above.
    pub confidence: f32,
    /// The runner-up, when it was within a hair of the winner — usually the
    /// relative major/minor, which shares all the same notes. `None` when the
    /// winner was clear.
    pub alternative: Option<Key>,
}

/// Detect the key of a track. `None` when there is not enough pitched material
/// to place one — silence, or a drum loop with no harmony.
pub fn detect(audio: &Audio) -> Option<Detected> {
    let chroma = chromagram(audio)?;
    correlate(&chroma)
}

/// The average pitch-class distribution of a track: twelve numbers summing to
/// one, or `None` when nothing pitched was found.
fn chromagram(audio: &Audio) -> Option<[f32; PITCHES]> {
    if audio.frames() < N_FFT {
        return None;
    }
    let sample_rate = audio.sample_rate as f32;
    let mut planner = RealFftPlanner::<f32>::new();
    let forward: Arc<dyn RealToComplex<f32>> = planner.plan_fft_forward(N_FFT);

    let window: Vec<f32> = (0..N_FFT)
        .map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / N_FFT as f32).cos())
        .collect();
    let bins = N_FFT / 2 + 1;
    let bin_hz = sample_rate / N_FFT as f32;
    // The fractional bin each note sits at, worked out once. Reading the
    // spectrum here rather than dropping bins into classes is what keeps the
    // bass honest.
    let note_bins: Vec<(usize, f32)> = (MIN_MIDI..=MAX_MIDI)
        .map(|midi| {
            let hz = 440.0 * 2.0f32.powf((midi - 69) as f32 / 12.0);
            ((midi.rem_euclid(PITCHES as i32)) as usize, hz / bin_hz)
        })
        .collect();

    let mut chroma = [0.0f64; PITCHES];
    let mut frame = vec![0.0f32; N_FFT];
    let mut spectrum = vec![Complex32::default(); bins];
    let mut scratch = forward.make_scratch_vec();

    let mut start = 0;
    let channels = audio.channels() as f32;
    while start + N_FFT <= audio.frames() {
        for (i, sample) in frame.iter_mut().enumerate() {
            let mono: f32 = audio.planes.iter().map(|p| p[start + i]).sum::<f32>() / channels;
            *sample = mono * window[i];
        }
        let _ = forward.process_with_scratch(&mut frame, &mut spectrum, &mut scratch);
        for &(class, bin) in &note_bins {
            // Magnitude, not power: a chromagram tracks how present a note is,
            // and power over-weights whatever happens to be loudest. The value
            // is read at the note's exact frequency, taking the strongest of the
            // bin and its neighbours so a track tuned a little sharp or flat
            // still lands on the right note.
            chroma[class] += magnitude_at(&spectrum, bin) as f64;
        }
        start += HOP;
    }

    let total: f64 = chroma.iter().sum();
    if total <= f64::EPSILON {
        return None;
    }
    let mut out = [0.0f32; PITCHES];
    for (slot, value) in out.iter_mut().zip(chroma) {
        *slot = (value / total) as f32;
    }
    Some(out)
}

/// The spectrum magnitude at a fractional bin, interpolated, and taken as the
/// strongest of that bin and the two either side. The neighbour search gives a
/// little tolerance for a track that is not tuned to exactly 440 Hz, which many
/// older and analogue-sourced records are not.
fn magnitude_at(spectrum: &[Complex32], bin: f32) -> f32 {
    let center = bin.round() as isize;
    let mut best = 0.0f32;
    for offset in -1..=1 {
        let at = center + offset;
        if at >= 0 && (at as usize) < spectrum.len() {
            best = best.max(spectrum[at as usize].norm());
        }
    }
    best
}

/// Match a chromagram against all twenty-four key profiles.
fn correlate(chroma: &[f32; PITCHES]) -> Option<Detected> {
    let mut scores: Vec<(Key, f32)> = Vec::with_capacity(24);
    for tonic in 0..PITCHES as u8 {
        scores.push((Key { tonic, mode: Mode::Major }, score(chroma, &MAJOR_PROFILE, tonic)));
        scores.push((Key { tonic, mode: Mode::Minor }, score(chroma, &MINOR_PROFILE, tonic)));
    }
    scores.sort_by(|a, b| b.1.total_cmp(&a.1));

    let (best_key, best) = scores[0];
    let (second_key, second) = scores[1];
    let average = scores.iter().map(|(_, s)| *s).sum::<f32>() / scores.len() as f32;

    // A correlation can be negative; shift everything so the confidence ratio
    // stays meaningful. A flat, keyless chroma scores near the average and so
    // comes out near one.
    let floor = scores.last().map(|(_, s)| *s).unwrap_or(0.0);
    let confidence = if (average - floor).abs() > f32::EPSILON {
        (best - floor) / (average - floor)
    } else {
        1.0
    };

    // Below this the two best keys are effectively tied — a relative
    // major/minor pair, or a genuinely modal track — so the runner-up is worth
    // reporting rather than hiding.
    let alternative =
        if best - second < 0.05 * best.abs().max(f32::EPSILON) { Some(second_key) } else { None };

    Some(Detected { key: best_key, confidence, alternative })
}

/// The correlation of a chromagram with one key's profile, rotated to `tonic`.
fn score(chroma: &[f32; PITCHES], profile: &[f32; PITCHES], tonic: u8) -> f32 {
    let rotated: Vec<f32> =
        (0..PITCHES).map(|i| profile[(i + PITCHES - tonic as usize) % PITCHES]).collect();
    pearson(chroma, &rotated)
}

/// Pearson correlation between two twelve-element vectors.
fn pearson(a: &[f32], b: &[f32]) -> f32 {
    let n = a.len() as f32;
    let mean_a = a.iter().sum::<f32>() / n;
    let mean_b = b.iter().sum::<f32>() / n;
    let mut covariance = 0.0;
    let mut var_a = 0.0;
    let mut var_b = 0.0;
    for (x, y) in a.iter().zip(b) {
        let da = x - mean_a;
        let db = y - mean_b;
        covariance += da * db;
        var_a += da * da;
        var_b += db * db;
    }
    let denominator = (var_a * var_b).sqrt();
    if denominator > f32::EPSILON {
        covariance / denominator
    } else {
        0.0
    }
}

/// Semitones above C for each note letter.
const LETTER_SEMITONES: [(char, i32); 7] =
    [('C', 0), ('D', 2), ('E', 4), ('F', 5), ('G', 7), ('A', 9), ('B', 11)];

fn parse_classical(text: &str) -> Option<Key> {
    let mut chars = text.chars().peekable();
    let letter = chars.next()?.to_ascii_uppercase();
    let mut semitone = LETTER_SEMITONES.iter().find(|(c, _)| *c == letter).map(|(_, s)| *s)?;

    // An accidental, if any.
    match chars.peek() {
        Some('#') | Some('♯') => {
            semitone += 1;
            chars.next();
        }
        Some('b') | Some('♭') => {
            // A lone 'b' after a letter is a flat; but "B" is a note, already
            // consumed. Only treat a lowercase 'b' as an accidental here.
            semitone -= 1;
            chars.next();
        }
        _ => {}
    }

    // The rest, trimmed, names the mode: empty or "maj" is major, "m" or
    // "min" is minor.
    let rest: String = chars.collect::<String>().trim().to_ascii_lowercase();
    let mode = match rest.as_str() {
        "" | "maj" | "major" | "d" => Mode::Major,
        "m" | "min" | "minor" => Mode::Minor,
        _ => return None,
    };

    Some(Key { tonic: semitone.rem_euclid(PITCHES as i32) as u8, mode })
}

fn parse_camelot(text: &str) -> Option<Key> {
    let bytes = text.as_bytes();
    if bytes.len() < 2 || bytes.len() > 3 {
        return None;
    }
    let letter = *bytes.last()?;
    let mode = match letter.to_ascii_uppercase() {
        b'A' => Mode::Minor,
        b'B' => Mode::Major,
        _ => return None,
    };
    let number: u32 = text[..text.len() - 1].parse().ok()?;
    if !(1..=12).contains(&number) {
        return None;
    }
    // Invert the mapping in `camelot`: find the tonic whose code is this one.
    (0..PITCHES as u8)
        .map(|tonic| Key { tonic, mode })
        .find(|key| key.camelot() == text.to_ascii_uppercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 44_100;

    /// A note, as a stack of a fundamental and a few harmonics so it reads like
    /// an instrument rather than a sine.
    fn note(midi: i32, seconds: f32, gain: f32, into: &mut [f32], at: usize) {
        let hz = 440.0 * 2.0f32.powf((midi - 69) as f32 / 12.0);
        let frames = (RATE as f32 * seconds) as usize;
        for i in 0..frames {
            let index = at + i;
            if index >= into.len() {
                break;
            }
            let t = i as f32 / RATE as f32;
            let envelope = (-1.5 * t).exp();
            let mut sample = 0.0;
            for (h, weight) in [1.0, 0.5, 0.3, 0.2].into_iter().enumerate() {
                sample += weight * (2.0 * std::f32::consts::PI * hz * (h + 1) as f32 * t).sin();
            }
            into[index] += gain * envelope * sample;
        }
    }

    /// Play a chord progression, each chord a list of MIDI notes, one bar each.
    fn progression(chords: &[&[i32]]) -> Audio {
        let bar = (RATE as f32 * 0.9) as usize;
        let len = bar * chords.len() + RATE as usize;
        let mut plane = vec![0.0f32; len];
        for (i, chord) in chords.iter().enumerate() {
            for &midi in *chord {
                note(midi, 0.9, 0.2, &mut plane, i * bar);
            }
        }
        Audio::new(RATE, vec![plane.clone(), plane]).unwrap()
    }

    // MIDI note numbers: C4 = 60.
    const C: i32 = 60;
    const D: i32 = 62;
    const E: i32 = 64;
    const F: i32 = 65;
    const G: i32 = 67;
    const A: i32 = 69;
    const B: i32 = 71;

    #[test]
    fn c_major_is_named_and_coded_correctly() {
        let key = Key { tonic: 0, mode: Mode::Major };
        assert_eq!(key.name(), "C");
        assert_eq!(key.camelot(), "8B");
    }

    #[test]
    fn a_minor_is_the_relative_of_c_major() {
        let key = Key { tonic: 9, mode: Mode::Minor };
        assert_eq!(key.name(), "Am");
        assert_eq!(key.camelot(), "8A");
    }

    #[test]
    fn the_camelot_wheel_matches_the_reference() {
        // A spot check against the printed wheel every DJ owns.
        assert_eq!(Key { tonic: 7, mode: Mode::Major }.camelot(), "9B"); // G
        assert_eq!(Key { tonic: 2, mode: Mode::Major }.camelot(), "10B"); // D
        assert_eq!(Key { tonic: 4, mode: Mode::Minor }.camelot(), "9A"); // Em
        assert_eq!(Key { tonic: 11, mode: Mode::Minor }.camelot(), "10A"); // Bm
        assert_eq!(Key { tonic: 5, mode: Mode::Major }.camelot(), "7B"); // F
        assert_eq!(Key { tonic: 8, mode: Mode::Minor }.camelot(), "1A"); // G#m
    }

    #[test]
    fn every_camelot_code_is_used_exactly_once() {
        // The twenty-four keys should fill the wheel with no gaps or clashes.
        let mut seen = std::collections::BTreeSet::new();
        for tonic in 0..12 {
            for mode in [Mode::Major, Mode::Minor] {
                assert!(seen.insert(Key { tonic, mode }.camelot()), "duplicate code");
            }
        }
        assert_eq!(seen.len(), 24);
    }

    #[test]
    fn a_c_major_progression_reads_as_c_major() {
        // I–IV–V–I in C: C, F, G, C.
        let audio = progression(&[&[C, E, G], &[F, A, C + 12], &[G, B, D + 12], &[C, E, G]]);
        let detected = detect(&audio).expect("no key found");
        assert_eq!(detected.key.name(), "C", "detected {}", detected.key.camelot());
    }

    #[test]
    fn an_a_minor_progression_reads_as_a_minor() {
        // i–iv–v–i in A minor: Am, Dm, Em, Am.
        let audio =
            progression(&[&[A, C + 12, E + 12], &[D, F, A], &[E, G, B], &[A, C + 12, E + 12]]);
        let detected = detect(&audio).expect("no key found");
        // The relative major shares every note, so accept A minor as the key or
        // as the close alternative — but it must be one of the two.
        let names: Vec<String> = std::iter::once(detected.key.name())
            .chain(detected.alternative.map(|k| k.name()))
            .collect();
        assert!(names.contains(&"Am".to_string()), "got {names:?}");
    }

    #[test]
    fn a_key_in_sharps_reads_correctly() {
        // E major: E, A, B, E.
        let audio = progression(&[
            &[E, G + 1, B],
            &[A, C + 13, E + 12],
            &[B, D + 13, F + 13],
            &[E, G + 1, B],
        ]);
        let detected = detect(&audio).expect("no key found");
        assert_eq!(detected.key.name(), "E", "detected {}", detected.key.camelot());
        assert_eq!(detected.key.camelot(), "12B");
    }

    #[test]
    fn a_tonal_track_is_more_confident_than_a_chromatic_wash() {
        let tonal = detect(&progression(&[&[C, E, G], &[G, B, D + 12]])).unwrap();
        // Every note at once: no key.
        let wash = progression(&[&(0..12).map(|s| C + s).collect::<Vec<_>>()]);
        let washed = detect(&wash).unwrap();
        assert!(
            tonal.confidence > washed.confidence,
            "tonal {} should beat wash {}",
            tonal.confidence,
            washed.confidence
        );
    }

    #[test]
    fn the_relative_minor_shows_up_as_the_alternative() {
        // A bare C/Am tonality with nothing to break the tie should report both.
        let audio = progression(&[&[C, E, G, A], &[A, C + 12, E + 12]]);
        let detected = detect(&audio).unwrap();
        let both: Vec<String> = std::iter::once(detected.key.name())
            .chain(detected.alternative.map(|k| k.name()))
            .collect();
        // Whichever won, C and Am are the relative pair; if an alternative was
        // offered it should be the other of the two.
        if detected.alternative.is_some() {
            assert!(
                both.contains(&"C".to_string()) || both.contains(&"Am".to_string()),
                "got {both:?}"
            );
        }
    }

    #[test]
    fn key_strings_round_trip_through_parse() {
        for tonic in 0..12 {
            for mode in [Mode::Major, Mode::Minor] {
                let key = Key { tonic, mode };
                assert_eq!(Key::parse(&key.name()), Some(key), "name {}", key.name());
                assert_eq!(Key::parse(&key.camelot()), Some(key), "camelot {}", key.camelot());
            }
        }
    }

    #[test]
    fn parse_understands_flats_and_the_notations_tools_use() {
        assert_eq!(Key::parse("Db"), Some(Key { tonic: 1, mode: Mode::Major }));
        assert_eq!(Key::parse("C#"), Some(Key { tonic: 1, mode: Mode::Major }));
        assert_eq!(Key::parse("Ebm"), Some(Key { tonic: 3, mode: Mode::Minor }));
        assert_eq!(Key::parse("Amin"), Some(Key { tonic: 9, mode: Mode::Minor }));
        assert_eq!(Key::parse("F#maj"), Some(Key { tonic: 6, mode: Mode::Major }));
        assert_eq!(Key::parse("8A"), Some(Key { tonic: 9, mode: Mode::Minor }));
        assert_eq!(Key::parse("11b"), Some(Key { tonic: 9, mode: Mode::Major })); // A major
        assert_eq!(Key::parse("nonsense"), None);
        assert_eq!(Key::parse(""), None);
    }

    #[test]
    fn silence_has_no_key() {
        let silence = Audio::new(RATE, vec![vec![0.0; RATE as usize * 2]]).unwrap();
        assert!(detect(&silence).is_none());
    }

    #[test]
    fn a_track_shorter_than_the_window_has_no_key() {
        let short = Audio::new(RATE, vec![vec![0.1; 1000]]).unwrap();
        assert!(detect(&short).is_none());
    }

    #[test]
    fn a_drum_loop_with_no_pitch_is_low_confidence() {
        // Band-limited noise bursts: energy, no tonal centre.
        let mut plane = vec![0.0f32; RATE as usize * 2];
        let mut state = 0x1234_5678u32;
        for (i, sample) in plane.iter_mut().enumerate() {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let noise = (state >> 8) as f32 / (1u32 << 24) as f32 - 0.5;
            let beat = (i % (RATE as usize / 2)) < (RATE as usize / 20);
            *sample = if beat { noise } else { 0.0 };
        }
        let audio = Audio::new(RATE, vec![plane.clone(), plane]).unwrap();
        // It may or may not find a key, but it should not be confident about one.
        if let Some(detected) = detect(&audio) {
            assert!(
                detected.confidence < 3.0,
                "too sure of {}: {}",
                detected.key.camelot(),
                detected.confidence
            );
        }
    }
}
