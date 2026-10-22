//! Reading the record where the stem says the singing is.
//!
//! # Why the record is read and the stem is not
//!
//! The vocal stem is the right thing to *find* the singing with and the wrong
//! thing to read the words off. What a separator leaves is a voice with the
//! band carved out around it — breathy where a cymbal was taken away, hollow
//! where a chord shared its notes — and a recogniser trained on speech hears
//! those scars as syllables. Measured on Jam-ALT, the benchmark of whole songs
//! (Syed, Meresman Higgs, Cífka and Sandler, 2025), Whisper large-v2 got 23.0
//! percent of words wrong read off the mix and 22.9 off the separated vocals:
//! the separation bought nothing, and its artefacts were where the invented
//! words came from. What did buy something was cutting the **mix** where the
//! separated vocals' loudness said there was singing — 20.4 percent, the best
//! open result published against the benchmark — and cutting the vocals the
//! same way bought less. The stem knows where the singing is. The record is
//! what the singing sounds like.
//!
//! # Why cut at all
//!
//! Because a recogniser handed music with nobody singing on it does not write
//! nothing. It writes "thanks for watching", over and over, and there is code
//! downstream whose whole job is to disbelieve it. The stem's loudness says
//! exactly which stretches have a voice in them, so those are what the
//! recogniser is handed: end to end, each with a margin of the record either
//! side, and a short silence between them where it can hear a pause rather
//! than a splice. What comes back is timed against that cut, and is put back
//! on the record's clock here before anything downstream sees it.
//!
//! The published pipeline cuts where the vocals are quiet for a second and
//! hands the recogniser each piece on its own. This cuts at the same second —
//! see [`GAP_MS`] for how that comes about — and hands over the pieces end to
//! end instead, so the recogniser keeps the thread of a verse across them.
//!
//! # Measured here
//!
//! On a spoken lyric over a synthetic band — the only singing a container can
//! make — read with whisper.cpp `base.en`: the record cut by a stem that is
//! quiet between the lines came back as eight lines for eight spoken, each
//! within half a second of where it was spoken, at 19 percent of words wrong;
//! the same record read whole came back as three segments at 21 percent, two
//! of them where a line was; and its stem read on its own as two segments at
//! 30 percent. The cut also took the least time, since the recogniser heard a
//! third of the record. The built-in separator's stem is another matter: its
//! bleed carries the kick at the beat period, which the half-second bridge in
//! [`super::align`] joins, so it is never quiet and the record is read whole —
//! still the better reading, by the numbers above. The ignored test
//! `words_off_the_mix` is the measurement, and prints that table.
//!
//! One thing this gets for nothing: the recogniser's habit of anchoring its
//! first segment at zero whatever comes before the first word — see
//! [`super::align`] — now puts that segment a margin before the first sung
//! stretch instead of at the start of the record.

use crate::audio::Audio;

use super::Transcript;

/// How much of the record goes in on either side of a sung stretch, in ms.
///
/// The stretches are found on 20 ms frames of the stem's loudness against a
/// floor, and a line begins with a consonant quieter than the vowel it leads
/// to, often with a breath before that. A third of a second is longer than
/// either and shorter than the gap between two lines, so a line keeps its
/// first consonant without taking the end of the line before it. The authors
/// of the measurement above put the invented words they still saw down to
/// cuts that landed inside the singing, which is what this is for.
pub const MARGIN_MS: u32 = 300;

/// The silence between two stretches, in ms.
///
/// Whisper breaks its segments at pauses, and two lines spliced hard together
/// come back as one segment with one time. Long enough to be heard as a
/// pause; far shorter than the silence it starts describing — "[Music]",
/// "(pause)" — which is seconds.
///
/// It is also the shortest gap worth cutting. A gap no longer than the silence
/// that would stand in for it stays as it is: cutting it would take real audio
/// out to put the same length of nothing back. With the margins either side of
/// a stretch that comes to a second of quiet on the stem before the record is
/// cut, which is where the published pipeline cuts too.
pub const GAP_MS: u32 = 400;

/// The fade at each edge of a piece, in ms: long enough not to click, short
/// enough to take nothing audible away.
const FADE_MS: u32 = 10;

/// A record cut down to its sung stretches, and the way back.
#[derive(Clone, Debug)]
pub struct Cut {
    /// The stretches, end to end, with [`GAP_MS`] of silence between them.
    pub audio: Audio,
    pieces: Vec<Piece>,
    /// How long the record is, for [`Cut::share`].
    whole_ms: u32,
}

/// One stretch of the record and where it landed in the cut.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Piece {
    /// Where it starts in the cut audio.
    at_ms: u32,
    /// What it is of the record.
    from_ms: u32,
    to_ms: u32,
}

impl Piece {
    fn len_ms(&self) -> u32 {
        self.to_ms - self.from_ms
    }

    /// Where it ends in the cut audio.
    fn end_ms(&self) -> u32 {
        self.at_ms + self.len_ms()
    }
}

impl Cut {
    /// Cut `mix` down to `spans` — where its stem is singing, in ms on the
    /// record's clock, as [`super::align::voiced_spans`] measures it. `None`
    /// when there is nothing to keep.
    pub fn of(mix: &Audio, spans: &[(u32, u32)]) -> Option<Cut> {
        let rate = mix.sample_rate;
        let whole_ms = ms_of(mix.frames(), rate);
        let stretches = stretches(spans, whole_ms);
        if stretches.is_empty() {
            return None;
        }

        let gap = frames_of(GAP_MS, rate);
        let fade = frames_of(FADE_MS, rate);
        let mut planes: Vec<Vec<f32>> = vec![Vec::new(); mix.channels()];
        let mut pieces = Vec::with_capacity(stretches.len());
        let mut at_ms = 0;
        for (nth, &(from_ms, to_ms)) in stretches.iter().enumerate() {
            if nth > 0 {
                for plane in &mut planes {
                    plane.resize(plane.len() + gap, 0.0);
                }
                at_ms += GAP_MS;
            }
            let from = frames_of(from_ms, rate).min(mix.frames());
            let to = frames_of(to_ms, rate).min(mix.frames());
            for (plane, source) in planes.iter_mut().zip(&mix.planes) {
                let start = plane.len();
                plane.extend_from_slice(&source[from..to]);
                faded(&mut plane[start..], fade);
            }
            pieces.push(Piece { at_ms, from_ms, to_ms });
            at_ms += to_ms - from_ms;
        }
        let audio = Audio::new(rate, planes).ok()?;
        Some(Cut { audio, pieces, whole_ms })
    }

    /// The stretches of the record that are in the cut, in ms, in order.
    pub fn stretches(&self) -> Vec<(u32, u32)> {
        self.pieces.iter().map(|piece| (piece.from_ms, piece.to_ms)).collect()
    }

    /// How much of the record is in the cut, from 0 to 1.
    pub fn share(&self) -> f32 {
        if self.whole_ms == 0 {
            return 0.0;
        }
        let kept: u32 = self.pieces.iter().map(Piece::len_ms).sum();
        kept as f32 / self.whole_ms as f32
    }

    /// Put a transcript of the cut audio onto the record's clock.
    ///
    /// A moment inside a piece is that far into its stretch of the record. A
    /// moment in the silence between two pieces belongs to neither: as a
    /// line's start it is taken as the start of the piece after it, and as a
    /// line's end as the end of the piece before — a line heard beginning in
    /// the gap began where the music came back, and one heard ending there
    /// ended where it went. The mapping never runs backwards, so lines that
    /// were in order stay in order.
    pub fn put_back(&self, mut transcript: Transcript) -> Transcript {
        for line in &mut transcript.lines {
            let start = self.start_back(line.start_ms);
            let end = self.end_back(line.end_ms).max(start);
            line.start_ms = start;
            line.end_ms = end;
        }
        // And every word, by the same map: a word is a moment in the cut like
        // a line is.
        for word in &mut transcript.words {
            let start = self.start_back(word.start_ms);
            let end = self.end_back(word.end_ms).max(start);
            word.start_ms = start;
            word.end_ms = end;
        }
        transcript
    }

    fn start_back(&self, ms: u32) -> u32 {
        for piece in &self.pieces {
            if ms < piece.at_ms {
                return piece.from_ms;
            }
            if ms < piece.end_ms() {
                return piece.from_ms + (ms - piece.at_ms);
            }
        }
        self.pieces.last().map_or(0, |piece| piece.to_ms)
    }

    fn end_back(&self, ms: u32) -> u32 {
        for piece in self.pieces.iter().rev() {
            if ms >= piece.end_ms() {
                return piece.to_ms;
            }
            if ms >= piece.at_ms {
                return piece.from_ms + (ms - piece.at_ms);
            }
        }
        self.pieces.first().map_or(0, |piece| piece.from_ms)
    }
}

/// The stretches of the record worth handing over: the spans with their
/// margins, clipped to the record, and run together where the gap between
/// two is not worth cutting.
fn stretches(spans: &[(u32, u32)], whole_ms: u32) -> Vec<(u32, u32)> {
    let mut padded: Vec<(u32, u32)> = spans
        .iter()
        .map(|&(from, to)| {
            (from.saturating_sub(MARGIN_MS), to.saturating_add(MARGIN_MS).min(whole_ms))
        })
        .filter(|(from, to)| from < to)
        .collect();
    padded.sort_unstable();

    let mut joined: Vec<(u32, u32)> = Vec::with_capacity(padded.len());
    for (from, to) in padded {
        match joined.last_mut() {
            Some(last) if from <= last.1.saturating_add(GAP_MS) => last.1 = last.1.max(to),
            _ => joined.push((from, to)),
        }
    }
    joined
}

/// Fade a piece in at its start and out at its end, so that the splice onto
/// silence either side of it does not click.
fn faded(piece: &mut [f32], fade: usize) {
    let fade = fade.min(piece.len() / 2);
    if fade == 0 {
        return;
    }
    let len = piece.len();
    for i in 0..fade {
        // Raised cosine: 0 at the edge, 1 a fade in, with no corner at either.
        let gain = 0.5 - 0.5 * (std::f32::consts::PI * (i as f32 + 0.5) / fade as f32).cos();
        piece[i] *= gain;
        piece[len - 1 - i] *= gain;
    }
}

fn frames_of(ms: u32, rate: u32) -> usize {
    (ms as u64 * rate as u64 / 1000) as usize
}

fn ms_of(frames: usize, rate: u32) -> u32 {
    (frames as u64 * 1000 / rate as u64) as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcribe::Line;

    /// A low rate keeps the buffers small; nothing here cares about the sound.
    const RATE: u32 = 8_000;

    /// A record whose every sample says what time it is, so a piece of it can
    /// be told from any other piece.
    fn record(seconds: u32) -> Audio {
        let frames = (RATE * seconds) as usize;
        let plane: Vec<f32> = (0..frames).map(|i| i as f32 / frames as f32).collect();
        Audio::new(RATE, vec![plane.clone(), plane]).unwrap()
    }

    fn said(times: &[(u32, u32)]) -> Transcript {
        Transcript {
            lines: times
                .iter()
                .map(|&(start_ms, end_ms)| Line { start_ms, end_ms, text: "something".into() })
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn a_word_goes_back_onto_the_records_clock_the_way_a_line_does() {
        let cut = Cut::of(&record(60), &[(10_000, 12_000), (40_000, 42_000)]).unwrap();
        // A word in the second piece of the cut, a second into its singing.
        let second = cut.pieces[1];
        let in_cut = second.at_ms + (40_000 - second.from_ms) + 1_000;
        let mut transcript = Transcript::default();
        transcript.words.push(crate::transcribe::Word {
            start_ms: in_cut,
            end_ms: in_cut + 300,
            text: "closer".into(),
        });
        let back = cut.put_back(transcript);
        assert_eq!((back.words[0].start_ms, back.words[0].end_ms), (41_000, 41_300));
    }

    #[test]
    fn the_cut_is_the_sung_stretches_with_their_margins_and_nothing_else() {
        let cut = Cut::of(&record(20), &[(4_000, 5_000), (10_000, 11_500)]).unwrap();
        assert_eq!(cut.stretches(), [(3_700, 5_300), (9_700, 11_800)]);
        // Two pieces and one gap between them, and not a gap after the last.
        let expected_ms = 1_600 + GAP_MS + 2_100;
        assert_eq!(cut.audio.frames(), frames_of(expected_ms, RATE));
        assert_eq!(cut.audio.channels(), 2, "the record's channels, not a mono mixdown");
        assert!((cut.share() - 0.185).abs() < 0.001, "{}", cut.share());
    }

    #[test]
    fn a_piece_is_the_record_and_not_a_copy_of_the_stem_or_silence() {
        let mix = record(20);
        let cut = Cut::of(&mix, &[(4_000, 5_000)]).unwrap();
        // Well inside the margin and past the fade: the record at 3.85 s.
        let in_cut = frames_of(150, RATE);
        let in_record = frames_of(3_850, RATE);
        assert_eq!(cut.audio.planes[0][in_cut], mix.planes[0][in_record]);
    }

    #[test]
    fn a_moment_in_a_piece_goes_back_to_where_it_was_in_the_record() {
        let cut = Cut::of(&record(20), &[(4_000, 5_000), (10_000, 11_500)]).unwrap();
        // The second piece starts after the first (1,600 ms) and the gap.
        let heard = said(&[(100, 900), (2_300, 3_000)]);
        let back = cut.put_back(heard);
        assert_eq!((back.lines[0].start_ms, back.lines[0].end_ms), (3_800, 4_600));
        assert_eq!((back.lines[1].start_ms, back.lines[1].end_ms), (10_000, 10_700));
    }

    #[test]
    fn a_line_heard_beginning_in_the_silence_began_where_the_music_came_back() {
        let cut = Cut::of(&record(20), &[(4_000, 5_000), (10_000, 11_500)]).unwrap();
        // Starts 100 ms into the gap, ends 500 ms into the second piece.
        let back = cut.put_back(said(&[(1_700, 2_500)]));
        assert_eq!(back.lines[0].start_ms, 9_700);
        assert_eq!(back.lines[0].end_ms, 10_200);
    }

    #[test]
    fn a_line_heard_ending_in_the_silence_ended_where_the_music_went() {
        let cut = Cut::of(&record(20), &[(4_000, 5_000), (10_000, 11_500)]).unwrap();
        let back = cut.put_back(said(&[(1_000, 1_800)]));
        assert_eq!((back.lines[0].start_ms, back.lines[0].end_ms), (4_700, 5_300));
    }

    #[test]
    fn a_line_that_runs_past_the_cut_ends_where_the_cut_does_on_the_record() {
        let cut = Cut::of(&record(20), &[(4_000, 5_000), (10_000, 11_500)]).unwrap();
        let back = cut.put_back(said(&[(2_300, 9_000)]));
        assert_eq!((back.lines[0].start_ms, back.lines[0].end_ms), (10_000, 11_800));
    }

    #[test]
    fn a_line_cannot_end_before_it_starts_on_the_way_back() {
        let cut = Cut::of(&record(20), &[(4_000, 5_000), (10_000, 11_500)]).unwrap();
        // A segment entirely inside the gap, which whisper can emit for a
        // breath: its start goes forward and its end goes back.
        let back = cut.put_back(said(&[(1_650, 1_950)]));
        assert_eq!((back.lines[0].start_ms, back.lines[0].end_ms), (9_700, 9_700));
    }

    #[test]
    fn stretches_closer_than_the_gap_are_one_piece() {
        // Padded, these are (700, 2300) and (2800, 4300): a 500 ms gap, which
        // is longer than the silence that would replace it, so two pieces.
        let two = Cut::of(&record(10), &[(1_000, 2_000), (3_100, 4_000)]).unwrap();
        assert_eq!(two.stretches(), [(700, 2_300), (2_800, 4_300)]);
        // (700, 2300) and (2700, 4300): a 400 ms gap, no longer than the
        // silence, so the record is kept as it is.
        let one = Cut::of(&record(10), &[(1_000, 2_000), (3_000, 4_000)]).unwrap();
        assert_eq!(one.stretches(), [(700, 4_300)]);
        // Overlapping margins, likewise.
        let overlapping = Cut::of(&record(10), &[(1_000, 2_000), (2_400, 3_000)]).unwrap();
        assert_eq!(overlapping.stretches(), [(700, 3_300)]);
    }

    #[test]
    fn the_stretches_stop_at_the_ends_of_the_record() {
        let cut = Cut::of(&record(20), &[(100, 1_000), (19_900, 25_000)]).unwrap();
        assert_eq!(cut.stretches(), [(0, 1_300), (19_600, 20_000)]);
        assert_eq!(cut.audio.frames(), frames_of(1_300 + GAP_MS + 400, RATE));
    }

    #[test]
    fn nothing_sung_is_nothing_to_cut() {
        assert!(Cut::of(&record(20), &[]).is_none());
        // A span past the end of the record is nothing of the record.
        assert!(Cut::of(&record(20), &[(30_000, 31_000)]).is_none());
    }

    #[test]
    fn the_edges_of_a_piece_are_faded_so_a_splice_does_not_click() {
        let loud = Audio::new(RATE, vec![vec![1.0; (RATE * 10) as usize]]).unwrap();
        let cut = Cut::of(&loud, &[(2_000, 3_000), (6_000, 7_000)]).unwrap();
        let plane = &cut.audio.planes[0];
        let fade = frames_of(FADE_MS, RATE);
        let first_len = frames_of(1_600, RATE);
        assert!(plane[0] < 0.1, "the first sample is {}", plane[0]);
        assert!(
            (plane[fade] - 1.0).abs() < 1e-6,
            "a fade in later it is the record: {}",
            plane[fade]
        );
        assert!(
            plane[first_len - 1] < 0.1,
            "the last sample of the piece is {}",
            plane[first_len - 1]
        );
        assert_eq!(plane[first_len], 0.0, "then the gap");
        assert_eq!(plane[first_len + frames_of(GAP_MS, RATE) - 1], 0.0);
        assert!(plane[first_len + frames_of(GAP_MS, RATE)] < 0.1, "and the next piece fades in");
    }

    #[test]
    fn the_way_back_and_the_placing_rule_together_put_a_line_on_its_onset() {
        // The recogniser anchors its first segment at zero. In the cut, zero
        // is a margin before the first sung stretch, so once put back the
        // line lands in that margin — and the placing rule, which moves a
        // line in a silence forward to the next onset, finishes the job.
        let spans = [(4_000, 5_000), (10_000, 11_500)];
        let cut = Cut::of(&record(20), &spans).unwrap();
        let mut back = cut.put_back(said(&[(0, 1_000), (2_300, 3_200)]));
        assert_eq!(back.lines[0].start_ms, 3_700, "a margin before the singing");
        crate::transcribe::align::to_the_voice(&mut back, &spans);
        assert_eq!(back.lines[0].start_ms, 4_000);
        assert_eq!(back.lines[1].start_ms, 10_000);
    }
}
