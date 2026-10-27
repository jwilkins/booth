//! An analysis written out as text, with the numbers each decision was made on.
//!
//! For a developer looking at a track that came out wrong: a downbeat a beat
//! late, a D on the intro, a B on a two-bar fill. What the analysis decided is
//! in the collection already; what it decided it *from* is not, and without
//! that the only way to find out is to run it again under a debugger. So this
//! gives each stage's evidence beside its answer — above all the downbeat,
//! which is chosen from a few readings per position in the bar and is wrong in
//! a way nothing downstream can notice.

use std::fmt::Write;

use super::energy;
use super::place::BEATS_PER_BAR;
use super::structure::Kind;
use super::tempo::{DecidedBy, Downbeat};
use super::TrackAnalysis;
use crate::export::CueKind;

/// How many beats each line of the downbeat's stretch-by-stretch
/// account covers: eight bars, a phrase, which is the longest a record holds
/// one idea and so the shortest stretch whose answer means anything.
const STRETCH_BEATS: usize = 32;

/// How far ahead the winning position has to be, by whichever reading decided,
/// for the downbeat to be more than a guess: a tenth. Below it the log says so.
///
/// Measured on the sixteen CC records with a reference: the two decided by
/// less were Pere, wrong at 1.008, and tou207c, right at 1.07 but with fifteen
/// of its 29 eight-bar stretches favouring the other beat the backbeat left.
/// The twelve others it got right it led by 1.16 to 10.4. A lead is not a
/// guarantee: two were wrong by more, at 1.24 and 2.15, both where the
/// reference itself is less sure.
const CLEAR_LEAD: f32 = 1.1;

/// How wide the prose is wrapped: what a terminal shows without wrapping it
/// mid-word itself.
const WIDTH: usize = 80;

/// The account of one track.
///
/// `name` is what to call the track at the top. `bands` and `drums` are the
/// three-band pictures the analysis was given, so that the drops and breaks
/// are found again exactly as the cues found them.
pub fn explain(name: &str, analysis: &TrackAnalysis, bands: &[u8], drums: Option<&[u8]>) -> String {
    let mut out = String::new();
    let build = if cfg!(debug_assertions) { "a debug build" } else { "a release build" };
    let _ = writeln!(out, "Analysis of {name}");
    let _ = writeln!(out, "booth-cli {}, {build}", env!("CARGO_PKG_VERSION"));
    tempo(&mut out, analysis);
    downbeat(&mut out, analysis);
    key(&mut out, analysis);
    arrangement(&mut out, analysis);
    shape(&mut out, analysis, bands, drums);
    cues(&mut out, analysis);
    out
}

fn heading(out: &mut String, title: &str) {
    let _ = write!(out, "\n{title}\n{}\n", "-".repeat(title.chars().count()));
}

/// Prose, wrapped at [`WIDTH`]. The tables are left as they are, since a
/// table wrapped is a table no longer.
fn paragraph(out: &mut String, text: &str) {
    let mut line = 0;
    for word in text.split_whitespace() {
        let width = word.chars().count();
        if line > 0 && line + 1 + width > WIDTH {
            out.push('\n');
            line = 0;
        }
        if line > 0 {
            out.push(' ');
            line += 1;
        }
        out.push_str(word);
        line += width;
    }
    out.push('\n');
}

fn tempo(out: &mut String, analysis: &TrackAnalysis) {
    heading(out, "Tempo");
    let beats = &analysis.grid.beats;
    let (Some(first), Some(last)) = (beats.first(), beats.last()) else {
        paragraph(
            out,
            &format!(
                "No beat found (confidence {:.2}). There is no grid, and nothing below that \
                 counts in bars has anything to count in.",
                analysis.confidence
            ),
        );
        return;
    };
    let confidence = match analysis.confidence.is_finite() {
        true => {
            format!("confidence {:.2}; under about 2 the tracker was guessing", analysis.confidence)
        }
        false => "the tempo was given rather than found".to_string(),
    };
    paragraph(out, &format!("{:.2} BPM, {confidence}.", analysis.bpm));
    if let Some(moved) = analysis.moved {
        paragraph(
            out,
            &format!(
                "Heard first at {:.1}, and moved an octave: the genre model hears {}, which is \
                 counted from {:.0} to {:.0} BPM.",
                moved.heard_bpm,
                moved.octave.because,
                moved.octave.from_bpm,
                moved.octave.from_bpm * 2.0
            ),
        );
    }
    paragraph(
        out,
        match analysis.grid.bends() {
            false => "One tempo throughout: the beats were put on a straight line through them.",
            true => "The tempo moves, and the grid follows it beat by beat.",
        },
    );
    paragraph(
        out,
        &format!(
            "{} beats in the grid, {} to {}.",
            beats.len(),
            clock(first.time_ms),
            clock(last.time_ms)
        ),
    );
}

fn downbeat(out: &mut String, analysis: &TrackAnalysis) {
    heading(out, "Downbeat");
    let found = &analysis.downbeat;
    let heard = found.kicks.len();
    if heard == 0 {
        paragraph(out, "No beats, so no downbeat.");
        return;
    }
    paragraph(
        out,
        &format!(
            "The grid has {heard} beats. Which of every four is the one is read off what \
             happens at each of the four positions in the bar, averaged over every beat in it: \
             the kick, as the low band's sharpest onset; the clap or snare, as the sharpest \
             onset from 1 to 5 kHz; and how far the bass note and the chord move across the \
             beat. A clap is on the 2 and the 4, so where one pair of positions has clearly \
             more, the 1 is on the other pair. Of the positions left, the kick says which it \
             favours and by how much, and so does the harmony, which changes on the 1, and the \
             surer of the two decides. A kick on every beat is sure of nothing."
        ),
    );
    if found.by == DecidedBy::TooFew {
        paragraph(out, "Under eight beats is too few to tell, so the first is taken as the one.");
    } else {
        let (kick, claps) = (found.kick(), found.claps());
        let harmony = found.harmony_over(0..heard);
        let (bass, chords) = (found.bass(), found.chords());
        let scores = found.scores();
        let _ = writeln!(
            out,
            "\n  position  grid's beats           kick     clap     bass   chords  harmony"
        );
        for position in 0..4 {
            let beats = format!("{}, {}, {}, ...", position + 1, position + 5, position + 9);
            let ruled_out = found.by == DecidedBy::Harmony && scores[position] == 0.0;
            let harmony = match ruled_out {
                true => "       -".to_string(),
                false => format!("{:>8.3}", harmony[position]),
            };
            let mark = if position == found.phase { "  <- the one" } else { "" };
            let _ = writeln!(
                out,
                "  {:<8}  {beats:<18}  {:>8.2} {:>8.2} {:>8.4} {:>8.4} {harmony}{mark}",
                ordinal(position),
                kick[position],
                claps[position],
                bass[position],
                chords[position],
            );
        }
        out.push('\n');
        paragraph(out, &reading(found));
    }
    match first_downbeat(analysis) {
        Some((index, at)) => paragraph(
            out,
            &format!("The first downbeat is the grid's beat {}, at {}.", index + 1, clock(at)),
        ),
        None => paragraph(out, "The grid has no downbeat."),
    }

    if heard < STRETCH_BEATS * 2 {
        return;
    }
    out.push('\n');
    paragraph(
        out,
        &format!(
            "Eight bars at a time, the {} that decided, to show whether the whole track agrees \
             or one stretch of it is outvoted. A break, with the kick out or the chords held, \
             often is.",
            match found.by {
                DecidedBy::Harmony => "harmony",
                _ => "kick",
            }
        ),
    );
    let _ = writeln!(out, "\n  from        1st       2nd       3rd       4th     favours");
    for start in (0..heard).step_by(STRETCH_BEATS) {
        let sums = found.scores_over(start..start + STRETCH_BEATS);
        let favours = (0..4).fold(0, |best, position| match sums[position] > sums[best] {
            true => position,
            false => best,
        });
        // A stretch with none of the evidence in it — the kick out, the
        // chords held — favours nothing.
        let (favours, mark) = match sums.iter().all(|&sum| sum == 0.0) {
            true => ("-".to_string(), ""),
            false if favours == found.phase => (ordinal(favours), ""),
            false => (ordinal(favours), "  <- not the one"),
        };
        let _ = writeln!(
            out,
            "  {:<10}{:>8.2}  {:>8.2}  {:>8.2}  {:>8.2}   {favours}{mark}",
            clock(found.times_ms[start]),
            sums[0],
            sums[1],
            sums[2],
            sums[3],
        );
    }
}

/// What the downbeat was decided on, and by how much, in words.
fn reading(found: &Downbeat) -> String {
    let lead = found.lead().unwrap_or(1.0);
    let guess = match lead < CLEAR_LEAD {
        true => " That is near enough a tie to be a guess.",
        false => "",
    };
    let one = ordinal(found.phase);
    let claps = found.claps();
    let (even, odd) = (claps[0] + claps[2], claps[1] + claps[3]);
    let apart = even.max(odd) / even.min(odd).max(f32::MIN_POSITIVE);
    let (backbeat, among) = match found.backbeat() {
        Some(pair) => (
            format!(
                "The claps are on the {} and {} positions, {apart:.2} times the other pair, so \
                 the 1 is on the {} or the {}.",
                ordinal(pair),
                ordinal(pair + 2),
                ordinal(1 - pair),
                ordinal(3 - pair)
            ),
            "Between those",
        ),
        None => (
            format!(
                "The claps favour neither pair of positions ({apart:.2} to 1), so they rule \
                 nothing out."
            ),
            "Of all four",
        ),
    };
    let lead_of = |by| match found.lead_of(by) {
        Some(lead) => format!("{lead:.2}"),
        None => "nothing".to_string(),
    };
    let (kick, harmony) = (lead_of(DecidedBy::Kick), lead_of(DecidedBy::Harmony));
    let surer = match found.by {
        DecidedBy::Harmony => "the harmony",
        _ if found.lead_of(DecidedBy::Harmony).is_none() => {
            "the kick, there being no harmony to speak of,"
        }
        _ => "the kick",
    };
    format!(
        "{backbeat} {among}, the kick's strongest position leads by {kick}, and the harmony's \
         — each position's share of the bass's change plus its share of the chords' — by \
         {harmony}. The surer is {surer} and says the {one}.{guess}"
    )
}

fn key(out: &mut String, analysis: &TrackAnalysis) {
    heading(out, "Key");
    let Some(found) = &analysis.key else {
        paragraph(out, "No tonal centre to find.");
        return;
    };
    let close = found
        .alternative
        .map(|other| format!("; {} ({}) came within a hair of it", other.camelot(), other.name()))
        .unwrap_or_default();
    paragraph(
        out,
        &format!(
            "{} ({}), confidence {:.2}{close}.",
            found.key.camelot(),
            found.key.name(),
            found.confidence
        ),
    );
}

fn arrangement(out: &mut String, analysis: &TrackAnalysis) {
    heading(out, "Arrangement");
    let sections = &analysis.structure.sections;
    if sections.is_empty() {
        paragraph(out, "No sections: the track was too short or too even to have any.");
        return;
    }
    let times: Vec<u32> = analysis.grid.beats.iter().map(|beat| beat.time_ms).collect();
    let at = |beat: u16| times.get(usize::from(beat).saturating_sub(1)).copied();
    let _ = writeln!(out, "  from        beats      kind     bars  busyness");
    for section in sections {
        let beats = section.end_beat.saturating_sub(section.start_beat);
        let _ = writeln!(
            out,
            "  {:<10}  {:>4}-{:<4}  {:<7}  {:>4}  {:.3}",
            at(section.start_beat).map(clock).unwrap_or_default(),
            section.start_beat,
            section.end_beat,
            section.kind.label(),
            beats as usize / BEATS_PER_BAR,
            section.intensity
        );
    }
    out.push('\n');
    paragraph(out, Kind::MEASURE);
    let mut explained = Vec::new();
    for section in sections {
        if !explained.contains(&section.kind) {
            explained.push(section.kind);
            out.push('\n');
            paragraph(out, &format!("{}: {}", section.kind.label(), section.kind.rules()));
        }
    }
}

fn shape(out: &mut String, analysis: &TrackAnalysis, bands: &[u8], drums: Option<&[u8]>) {
    heading(out, "Drops and breaks");
    let Some((downbeat, _)) = first_downbeat(analysis) else {
        paragraph(out, "No grid, so no bars to measure.");
        return;
    };
    // From the first downbeat, as the cues count them, so bar 1 here is bar 1
    // there.
    let grid: Vec<u32> = analysis.grid.beats[downbeat..].iter().map(|beat| beat.time_ms).collect();
    let mix = energy::bars(bands, &grid);
    let drum_bars = drums.map(|drums| energy::bars(drums, &grid));
    let shape = energy::read(bands, drums, &grid);
    let bar_at = |bar: usize| grid.get(bar * BEATS_PER_BAR).copied().map(clock).unwrap_or_default();

    paragraph(
        out,
        &format!(
            "How much each bar plays: the geometric mean of its power and of how hard its low \
             band climbs, from the three-band picture{}. A drop is {} bars or more in the upper \
             half of the record's own range; a break is the gap between two drops, as long.",
            match drums {
                Some(_) => ", and from the drum stem's, which adds the stretches the drums are out",
                None => "; the track has no drum stem",
            },
            energy::RUN_BARS
        ),
    );
    let main_drop = shape.main_drop();
    let main_break = shape.main_break();
    let run = |kind: &str, run: &energy::Run, main: Option<energy::Run>, letter: &str| {
        format!(
            "  {kind:<6} bars {:>3}-{:<3} from {:<10} {:>3} bars, energy {:.4}{}",
            run.from + 1,
            run.to,
            bar_at(run.from),
            run.bars(),
            run.energy,
            if main == Some(*run) { letter } else { "" }
        )
    };
    out.push('\n');
    if shape.drops.is_empty() {
        let _ = writeln!(out, "  No drops.");
    }
    for drop in &shape.drops {
        let _ = writeln!(out, "{}", run("drop", drop, main_drop, "  <- the main drop, D"));
    }
    if shape.breaks.is_empty() {
        let _ = writeln!(out, "  No breaks.");
    }
    for quiet in &shape.breaks {
        let _ = writeln!(out, "{}", run("break", quiet, main_break, "  <- the main break, B"));
    }

    let _ = writeln!(
        out,
        "\n  bar  from         mix{}",
        if drum_bars.is_some() { "     drums" } else { "" }
    );
    for (bar, energy) in mix.iter().enumerate() {
        let drums = drum_bars
            .as_ref()
            .and_then(|drums| drums.get(bar))
            .map(|energy| format!("  {energy:>8.4}"))
            .unwrap_or_default();
        let part = if shape.drops.iter().any(|run| run.holds(bar)) {
            "  drop"
        } else if shape.breaks.iter().any(|run| run.holds(bar)) {
            "  break"
        } else {
            ""
        };
        let _ = writeln!(out, "  {:>3}  {:<10}{energy:>8.4}{drums}{part}", bar + 1, bar_at(bar));
    }
}

fn cues(out: &mut String, analysis: &TrackAnalysis) {
    heading(out, "Cues the analysis suggested");
    if analysis.cues.is_empty() {
        paragraph(out, "None.");
        return;
    }
    // In time order, which is how a track is read; the list itself has the
    // memory cues first.
    let mut cues: Vec<_> = analysis.cues.iter().collect();
    cues.sort_by_key(|cue| cue.time_ms);
    for cue in cues {
        let button = match cue.hot_cue {
            0 => "memory".to_string(),
            letter => char::from(b'A' + letter - 1).to_string(),
        };
        let looped = match cue.kind {
            CueKind::Loop { end_ms } => format!(", a loop to {}", clock(end_ms)),
            CueKind::Point => String::new(),
        };
        let _ = writeln!(
            out,
            "  {button:<6}  {:<10}  {}{looped}",
            clock(cue.time_ms),
            cue.comment.as_deref().unwrap_or("")
        );
    }
}

/// Where in the grid the first downbeat is: its index and its time.
fn first_downbeat(analysis: &TrackAnalysis) -> Option<(usize, u32)> {
    analysis
        .grid
        .beats
        .iter()
        .enumerate()
        .find(|(_, beat)| beat.number == 1)
        .map(|(index, beat)| (index, beat.time_ms))
}

fn ordinal(index: usize) -> String {
    let n = index + 1;
    let suffix = match (n % 10, n % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{n}{suffix}")
}

/// A time as minutes, seconds and milliseconds: `4:07.250`.
fn clock(ms: u32) -> String {
    format!("{}:{:02}.{:03}", ms / 60_000, ms / 1000 % 60, ms % 1000)
}

#[cfg(test)]
mod tests {
    use super::super::tempo::{DecidedBy, Downbeat};
    use super::super::{analyze, tests::track};
    use super::*;
    use crate::audio::Audio;

    fn explained(analysis: &TrackAnalysis) -> String {
        let bands = crate::export::waveform::analyze(&track()).band_detail;
        explain("track.wav", analysis, &bands, None)
    }

    #[test]
    fn every_stage_is_explained_under_its_own_heading() {
        let text = explained(&analyze(&track()));
        for heading in
            ["Tempo", "Downbeat", "Key", "Arrangement", "Drops and breaks", "Cues the analysis"]
        {
            assert!(text.contains(&format!("\n{heading}")), "no {heading:?} in:\n{text}");
        }
        assert!(text.starts_with("Analysis of track.wav\n"), "{text}");
    }

    #[test]
    fn the_position_marked_as_the_one_is_the_largest_sum_and_the_grid_agrees() {
        let analysis = analyze(&track());
        let text = explained(&analysis);
        let sums = analysis.downbeat.scores();
        let largest = (0..4).fold(0, |best, i| if sums[i] > sums[best] { i } else { best });
        assert_eq!(analysis.downbeat.phase, largest);

        let marked: Vec<&str> = text.lines().filter(|line| line.ends_with("<- the one")).collect();
        assert_eq!(marked.len(), 1, "{text}");
        assert!(marked[0].trim_start().starts_with(&ordinal(largest)), "{}", marked[0]);

        let (index, at) = first_downbeat(&analysis).unwrap();
        let said =
            format!("The first downbeat is the grid's beat {}, at {}.", index + 1, clock(at));
        assert!(text.contains(&said), "no {said:?} in:\n{text}");
    }

    #[test]
    fn a_stretch_that_favours_another_beat_is_pointed_out() {
        // Two phrases of kicks on the first beat of four, then one on the third:
        // a two-beat fill the tracker kept, say. The whole track still says
        // the first, and the third stretch is the one to look at.
        let beats = super::STRETCH_BEATS * 3;
        let kicks = (0..beats)
            .map(|i| {
                let on = if i < super::STRETCH_BEATS * 2 { 0 } else { 2 };
                if i % 4 == on {
                    1.0
                } else {
                    0.1
                }
            })
            .collect();
        let mut analysis = analyze(&track());
        analysis.downbeat = Downbeat {
            times_ms: (0..beats as u32).map(|i| i * 500).collect(),
            kicks,
            by: DecidedBy::Kick,
            ..Default::default()
        };
        let text = explained(&analysis);
        let pointed: Vec<&str> =
            text.lines().filter(|line| line.ends_with("<- not the one")).collect();
        assert_eq!(pointed.len(), 1, "{text}");
        assert!(pointed[0].starts_with("  0:32.000"), "{}", pointed[0]);
        assert!(pointed[0].contains(" 3rd"), "{}", pointed[0]);
    }

    #[test]
    fn silence_is_explained_as_having_no_beat_rather_than_a_guess() {
        let silence = Audio::new(44_100, vec![vec![0.0; 44_100 * 10]; 2]).unwrap();
        let analysis = analyze(&silence);
        let bands = crate::export::waveform::analyze(&silence).band_detail;
        let text = explain("silence.wav", &analysis, &bands, None);
        assert!(text.contains("No beat found"), "{text}");
        assert!(text.contains("No beats, so no downbeat."), "{text}");
        assert!(text.contains("No grid, so no bars to measure."), "{text}");
    }

    /// Writes the explanation of every record in `BOOTH_DRUM_RECORDS` into
    /// `BOOTH_EXPLAIN_TO`, with its drum stem from `BOOTH_DRUM_STEMS` where
    /// there is one, to be read: whether it says what was decided and why.
    #[test]
    #[ignore = "needs records on disk"]
    fn real_records_explained() {
        use crate::audio::decode::decode_file;
        let var = |name: &str| std::env::var_os(name).map(std::path::PathBuf::from);
        let (Some(records), Some(to)) = (var("BOOTH_DRUM_RECORDS"), var("BOOTH_EXPLAIN_TO")) else {
            return;
        };
        let stems = var("BOOTH_DRUM_STEMS");
        std::fs::create_dir_all(&to).unwrap();
        let mut names: Vec<_> = std::fs::read_dir(&records)
            .unwrap()
            .filter_map(|entry| Some(entry.ok()?.path()))
            .filter(|path| path.extension().is_some_and(|ext| ext == "mp3" || ext == "flac"))
            .collect();
        names.sort();
        for path in names {
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            let stem = path.file_stem().unwrap().to_string_lossy().to_string();
            let drums = stems
                .as_ref()
                .and_then(|stems| decode_file(&stems.join(&stem).join("drums.wav")).ok())
                .map(|drums| crate::export::waveform::analyze(&drums).band_detail);
            let audio = decode_file(&path).unwrap();
            let bands = crate::export::waveform::analyze(&audio).band_detail;
            let analysis =
                super::super::analyze_with_drums(&audio, None, &bands, drums.as_deref(), None);
            let text = explain(&name, &analysis, &bands, drums.as_deref());
            std::fs::write(to.join(format!("{name}.log")), text).unwrap();
            eprintln!("{name}");
        }
    }

    /// Where the analysis puts the bar on the records the downbeat was
    /// measured against, from `BOOTH_DRUM_RECORDS`: for each, the beat of the
    /// bar it gives a moment the reference calls a 1.
    ///
    /// The reference is the arrangement, not a listener or rekordbox: on each
    /// record's straight grid, where its biggest changes in energy fall in the
    /// bar, two and four bars either side, whether they sit whole phrases
    /// apart, and whether the backbeat agrees. `true` where all of that is
    /// consistent, `false` where it is less sure. A confident one off the 1
    /// fails the test; the rest are reported.
    ///
    /// The records are on archive.org: mne013-gartner-phobos,
    /// audcst031Blacksquare-Upside, INeedHouseByBlacksquareaudcst016, tou207,
    /// vault-kid-into-the-vault, vault-kid-summer-wars, ca015_va_cs (Buben,
    /// Pere), 45rpm028_trib2010Buben-MorningEcho (Next Time), unfound51,
    /// mtrnc003, audcst037Aku-Stories, maladafneep, 1bit_011 (Markus Masuhr).
    #[test]
    #[ignore = "needs records on disk"]
    fn real_records_put_the_bar_where_the_reference_does() {
        use crate::audio::decode::decode_file;
        let Some(records) = std::env::var_os("BOOTH_DRUM_RECORDS") else {
            return;
        };
        // A distinctive piece of each title, a moment the reference calls a
        // 1 in milliseconds, and whether the reference is confident of it.
        let reference = [
            ("Signal", 60_043, true),
            ("Brain Dance", 59_994, false),
            ("Deep Into Another", 60_485, true),
            ("Groovielaugh", 59_513, true),
            ("I Need House", 59_520, true),
            ("Upside", 59_999, true),
            ("Next Time", 60_041, true),
            ("Pere", 60_542, false),
            ("Dafne", 59_785, true),
            ("Blunted", 59_515, true),
            ("Orquidea", 59_517, true),
            ("Prova", 59_541, true),
            ("Summer Wars", 61_348, false),
            ("Trainjam", 59_135, false),
            ("Treasure", 59_085, false),
            ("tou207c", 59_169, true),
        ];
        let mut wrong = Vec::new();
        for entry in std::fs::read_dir(records).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            let lower = name.to_lowercase();
            let Some(&(title, at, confident)) =
                reference.iter().find(|(title, ..)| lower.contains(&title.to_lowercase()))
            else {
                continue;
            };
            let analysis = analyze(&decode_file(&path).unwrap());
            let beats = &analysis.grid.beats;
            let Some(nearest) = beats.iter().min_by_key(|beat| beat.time_ms.abs_diff(at)) else {
                continue;
            };
            let off = nearest.time_ms.abs_diff(at);
            let said = match off < 70 {
                true => format!("beat {}", nearest.number),
                false => format!("no beat within {off} ms"),
            };
            eprintln!(
                "{title:<18} {}  the reference's 1 at {} is {said}",
                if confident { "confident" } else { "less sure" },
                clock(at)
            );
            if confident && (off >= 70 || nearest.number != 1) {
                wrong.push(title);
            }
        }
        assert!(wrong.is_empty(), "off the 1 where the reference is confident: {wrong:?}");
    }

    #[test]
    fn a_downbeat_won_by_a_hair_is_called_a_guess_and_a_clear_one_is_not() {
        let mut analysis = analyze(&track());
        let said = |kicks: [f32; 4], analysis: &mut TrackAnalysis| {
            analysis.downbeat = Downbeat {
                times_ms: (0..16).map(|i| i * 500).collect(),
                kicks: (0..16).map(|i| kicks[i % 4]).collect(),
                by: DecidedBy::Kick,
                ..Default::default()
            };
            // Read as prose, whatever line the wrapping broke it over.
            let words: Vec<String> =
                explained(analysis).split_whitespace().map(str::to_string).collect();
            words.join(" ").contains("near enough a tie to be a guess")
        };
        assert!(said([1.02, 1.0, 1.0, 1.0], &mut analysis));
        assert!(!said([2.0, 1.0, 1.0, 1.0], &mut analysis));
    }

    #[test]
    fn a_time_reads_as_minutes_seconds_and_milliseconds() {
        assert_eq!(clock(247_250), "4:07.250");
        assert_eq!(clock(5), "0:00.005");
    }

    #[test]
    fn positions_in_the_bar_are_named_as_a_count_would_name_them() {
        let named: Vec<String> = (0..4).map(ordinal).collect();
        assert_eq!(named, ["1st", "2nd", "3rd", "4th"]);
        assert_eq!(ordinal(10), "11th");
    }

    #[test]
    fn the_prose_is_wrapped_at_eighty_columns_and_the_tables_are_not() {
        let text = explained(&analyze(&track()));
        let prose = |line: &&str| !line.starts_with("  ");
        let long: Vec<&str> =
            text.lines().filter(prose).filter(|line| line.chars().count() > WIDTH).collect();
        assert!(long.is_empty(), "{long:#?}");
        // A table row is one line however long, so its columns stay columns.
        assert!(text.lines().any(|line| line.starts_with("  1st ")), "{text}");
    }
}
