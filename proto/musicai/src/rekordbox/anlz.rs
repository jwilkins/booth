//! Reading the analysis files back off a drive.
//!
//! [`crate::export::anlz`] writes these. This reads them, for the one job that
//! needs it: a CDJ-3000X can move a cue, re-grid a track or rename a phrase on
//! the deck and writes the result back to the stick, and a collection that
//! cannot read that can only ever overwrite it.
//!
//! # What it is not
//!
//! This is not an independent parser and does not pretend to be one. It was
//! written from the same understanding of the format as the writer, so the two
//! agreeing proves nothing about whether either matches rekordbox — that check
//! belongs to the round-trip tests through `rekordcrate`, which is a different
//! implementation by different people. What this is for is reading a file that
//! *this program wrote and a player has since edited*, which is a narrower job
//! and the one that matters here.
//!
//! It is deliberately forgiving all the same, because the second half of that
//! sentence means a player has had its hands on the bytes. Lengths are trusted
//! over layout, sections whose tag is not recognised are stepped over rather
//! than refused, and a section that runs off the end of the file ends the walk
//! instead of failing it. A file that has been half rewritten should give up
//! what it still has.

use anyhow::{bail, Result};

use crate::export::{BeatGrid, Cue, CueKind, Mood, Phrase, Rgb, SongStructure};

/// Everything worth having out of one track's analysis files.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Analysis {
    /// The audio file the analysis belongs to, as the player sees it.
    pub path: String,
    pub grid: BeatGrid,
    /// Memory cues and hot cues together, in the order they were found.
    pub cues: Vec<Cue>,
    pub structure: Option<SongStructure>,
}

impl Analysis {
    /// Fold a second file's sections into this one.
    ///
    /// The `.EXT` is read after the `.DAT` and wins where they overlap: it
    /// carries the same cues with their names and colours, which the older
    /// section has no room for.
    pub fn merge(&mut self, other: Analysis) {
        if !other.path.is_empty() {
            self.path = other.path;
        }
        if !other.grid.beats.is_empty() {
            self.grid = other.grid;
        }
        if !other.cues.is_empty() {
            self.cues = other.cues;
        }
        if other.structure.is_some() {
            self.structure = other.structure;
        }
    }

    /// The tempo the grid was written at, in BPM.
    ///
    /// The median rather than the mean: a grid holds a tempo per beat, and one
    /// beat out of place should not move the number a browser sorts on.
    pub fn bpm(&self) -> Option<f64> {
        let mut tempos: Vec<u16> = self.grid.beats.iter().map(|beat| beat.tempo_x100).collect();
        if tempos.is_empty() {
            return None;
        }
        tempos.sort_unstable();
        Some(tempos[tempos.len() / 2] as f64 / 100.0)
    }

    /// Where a beat number falls, in milliseconds. Beat 1 is the first.
    pub fn time_of(&self, beat: u16) -> Option<u32> {
        let index = (beat.max(1) - 1) as usize;
        self.grid.beats.get(index).map(|beat| beat.time_ms)
    }
}

/// Read one analysis file.
pub fn read(bytes: &[u8]) -> Result<Analysis> {
    if bytes.len() < 12 || &bytes[..4] != b"PMAI" {
        bail!("not an analysis file: it does not begin with PMAI");
    }
    let header_len = be_u32(bytes, 4).unwrap_or(0) as usize;
    if header_len < 12 || header_len > bytes.len() {
        bail!("the file header claims {header_len} bytes, which is not a length this file has");
    }

    let mut found = Analysis::default();
    for section in sections(bytes, header_len) {
        match &section.tag {
            b"PPTH" => found.path = path(section.whole).unwrap_or_default(),
            b"PQTZ" => found.grid = beat_grid(section.whole),
            // The plain list first and the extended one after, in the order
            // the file writes them, so the richer section wins by arriving
            // second.
            b"PCOB" => extend(&mut found.cues, plain_cues(section.whole)),
            b"PCO2" => replace_kind(&mut found.cues, extended_cues(section.whole)),
            b"PSSI" => found.structure = song_structure(section.whole),
            _ => {}
        }
    }
    Ok(found)
}

/// Read a track's `.DAT` and `.EXT` together, which is how a player reads them.
pub fn read_files(dat: &[u8], ext: Option<&[u8]>) -> Result<Analysis> {
    let mut found = read(dat)?;
    if let Some(ext) = ext {
        // A `.EXT` that will not parse is not a reason to throw away a `.DAT`
        // that did: the older file carries the grid and the cue positions, and
        // half an answer is worth more than none.
        if let Ok(extended) = read(ext) {
            found.merge(extended);
        }
    }
    Ok(found)
}

/// Add cues for a kind — memory or hot — that the list has none of yet.
fn extend(cues: &mut Vec<Cue>, found: Vec<Cue>) {
    for cue in found {
        if !cues.iter().any(|have| have.is_hot() == cue.is_hot() && have.time_ms == cue.time_ms) {
            cues.push(cue);
        }
    }
    cues.sort_by_key(|cue| (cue.hot_cue, cue.time_ms));
}

/// Replace every cue of the same kind — memory or hot — with these.
///
/// The extended section is the whole of that kind's list, not an addition to
/// it, so a cue the player deleted has to go rather than survive in the older
/// section that still mentions it.
fn replace_kind(cues: &mut Vec<Cue>, found: Vec<Cue>) {
    let Some(hot) = found.first().map(Cue::is_hot) else { return };
    cues.retain(|cue| cue.is_hot() != hot);
    cues.extend(found);
    cues.sort_by_key(|cue| (cue.hot_cue, cue.time_ms));
}

/// One tagged section, as it sits in the file.
struct Section<'a> {
    tag: [u8; 4],
    /// The whole section, from its tag to the end of its body, so a reader can
    /// use the offsets the format documentation gives.
    whole: &'a [u8],
}

/// Walk the sections, stopping at the first one that does not fit.
fn sections(bytes: &[u8], from: usize) -> Vec<Section<'_>> {
    let mut found = Vec::new();
    let mut at = from;
    while at + 12 <= bytes.len() {
        let total = match be_u32(bytes, at + 8) {
            Some(total) if total >= 12 => total as usize,
            // A section with no length is the end of what can be read, not a
            // reason to reject everything before it.
            _ => break,
        };
        let end = match at.checked_add(total) {
            Some(end) if end <= bytes.len() => end,
            _ => break,
        };
        let mut tag = [0u8; 4];
        tag.copy_from_slice(&bytes[at..at + 4]);
        found.push(Section { tag, whole: &bytes[at..end] });
        at = end;
    }
    found
}

/// `PPTH` — the audio file's path on the drive.
fn path(section: &[u8]) -> Option<String> {
    let len = be_u32(section, 12)? as usize;
    let text = section.get(16..16 + len)?;
    Some(utf16_text(text))
}

/// `PQTZ` — the beat grid.
fn beat_grid(section: &[u8]) -> BeatGrid {
    let count = be_u32(section, 20).unwrap_or(0) as usize;
    let mut beats = Vec::with_capacity(count.min(1 << 20));
    let mut at = 24;
    for _ in 0..count {
        let (Some(number), Some(tempo), Some(time_ms)) =
            (be_u16(section, at), be_u16(section, at + 2), be_u32(section, at + 4))
        else {
            break;
        };
        beats.push(crate::export::Beat { number, tempo_x100: tempo, time_ms });
        at += 8;
    }
    BeatGrid { beats }
}

/// `PCOB` — cue positions, with no names and no colours.
fn plain_cues(section: &[u8]) -> Vec<Cue> {
    let count = be_u16(section, 18).unwrap_or(0) as usize;
    let mut found = Vec::with_capacity(count.min(1 << 12));
    let mut at = 24;
    for _ in 0..count {
        let Some(entry) = section.get(at..at + 0x38) else { break };
        if &entry[..4] != b"PCPT" {
            break;
        }
        let hot_cue = be_u32(entry, 12).unwrap_or(0) as u8;
        let time_ms = be_u32(entry, 32).unwrap_or(0);
        let loop_end = be_u32(entry, 36).unwrap_or(u32::MAX);
        found.push(placed(hot_cue, time_ms, entry.get(28).copied().unwrap_or(1), loop_end));
        at += 0x38;
    }
    found
}

/// `PCO2` — the same cues with their names and their colours.
fn extended_cues(section: &[u8]) -> Vec<Cue> {
    let count = be_u16(section, 16).unwrap_or(0) as usize;
    let mut found = Vec::with_capacity(count.min(1 << 12));
    let mut at = 20;
    for _ in 0..count {
        // The entry says its own length, because the comment makes every one a
        // different size.
        let Some(len) = be_u32(section, at + 8).map(|len| len as usize) else { break };
        let Some(entry) = section.get(at..at + len.max(44)) else { break };
        if &entry[..4] != b"PCP2" {
            break;
        }
        let hot_cue = be_u32(entry, 12).unwrap_or(0) as u8;
        let kind = entry.get(16).copied().unwrap_or(1);
        let time_ms = be_u32(entry, 20).unwrap_or(0);
        let loop_end = be_u32(entry, 24).unwrap_or(u32::MAX);
        let mut cue = placed(hot_cue, time_ms, kind, loop_end);

        let comment_len = be_u32(entry, 40).unwrap_or(0) as usize;
        if let Some(text) = entry.get(44..44 + comment_len) {
            let comment = utf16_text(text);
            if !comment.is_empty() {
                cue.comment = Some(comment);
            }
        }
        // The colour follows the comment, one byte of rekordbox's own palette
        // index and then the three that say what it actually looks like.
        if let Some(rgb) = entry.get(45 + comment_len..48 + comment_len) {
            if rgb != [0, 0, 0] {
                cue.color = Some(Rgb { r: rgb[0], g: rgb[1], b: rgb[2] });
            }
        }
        found.push(cue);
        at += len.max(44);
    }
    found
}

fn placed(hot_cue: u8, time_ms: u32, kind: u8, loop_end: u32) -> Cue {
    let cue = match hot_cue {
        0 => Cue::memory(time_ms),
        letter => Cue::hot(letter, time_ms),
    };
    match (kind, loop_end) {
        (2, end) if end != u32::MAX => Cue { kind: CueKind::Loop { end_ms: end }, ..cue },
        _ => cue,
    }
}

/// `PSSI` — the phrases, which are XOR-masked on the way out and back.
fn song_structure(section: &[u8]) -> Option<SongStructure> {
    let entry_len = be_u32(section, 12).unwrap_or(0) as usize;
    let count = be_u16(section, 16)? as usize;
    if entry_len == 0 {
        return None;
    }

    // Everything from the mood onwards is masked, by a fixed pattern offset by
    // the phrase count. See [`crate::export::anlz::song_structure`].
    let masked = section.get(18..)?;
    let key = count as u8;
    let body: Vec<u8> = masked
        .iter()
        .enumerate()
        .map(|(i, byte)| byte ^ crate::export::anlz::PSSI_MASK[i % 19].wrapping_add(key))
        .collect();

    let mood = match be_u16(&body, 0)? {
        1 => Mood::High,
        2 => Mood::Mid,
        _ => Mood::Low,
    };
    let end_beat = be_u16(&body, 8)?;
    let bank = body.get(12).copied().unwrap_or(0);

    let mut phrases = Vec::with_capacity(count.min(1 << 12));
    let mut at = 14;
    for _ in 0..count {
        let (Some(beat), Some(kind)) = (be_u16(&body, at + 2), be_u16(&body, at + 4)) else {
            break;
        };
        phrases.push(Phrase { beat, kind });
        at += entry_len;
    }
    Some(SongStructure { mood, end_beat, bank, phrases })
}

fn be_u16(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

fn be_u32(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

/// UTF-16 big-endian, with the trailing NUL every string in this format has.
fn utf16_text(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
        .take_while(|unit| *unit != 0)
        .collect();
    String::from_utf16_lossy(&units)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::anlz;
    use crate::export::{waveform::WaveformData, Beat};

    fn grid() -> BeatGrid {
        BeatGrid {
            beats: (0..16)
                .map(|i| Beat {
                    number: (i % 4) as u16 + 1,
                    tempo_x100: 12_800,
                    time_ms: i as u32 * 469,
                })
                .collect(),
        }
    }

    fn cues() -> Vec<Cue> {
        vec![
            Cue::memory(0),
            Cue::hot(1, 4_000).with_comment("hold me closer now").with_color(0xe8, 0x3c, 0x9e),
            Cue::hot(2, 9_500),
            Cue::hot(3, 12_000).looping(13_875),
        ]
    }

    fn structure() -> SongStructure {
        SongStructure {
            mood: Mood::High,
            end_beat: 16,
            bank: 0,
            phrases: vec![Phrase { beat: 1, kind: 1 }, Phrase { beat: 9, kind: 5 }],
        }
    }

    fn written() -> (Vec<u8>, Vec<u8>) {
        let waveforms = WaveformData::silent(8.0);
        let structure = structure();
        let cues = cues();
        let grid = grid();
        let files = anlz::Analysis {
            on_drive_path: "/Contents/Artist/Track.flac",
            grid: &grid,
            cues: &cues,
            waveforms: &waveforms,
            structure: Some(&structure),
            vbr: None,
        };
        (files.dat(), files.ext())
    }

    #[test]
    fn a_grid_comes_back_beat_for_beat() {
        let (dat, _) = written();
        let found = read(&dat).unwrap();
        assert_eq!(found.grid, grid());
        assert_eq!(found.bpm(), Some(128.0));
        assert_eq!(found.time_of(1), Some(0));
        assert_eq!(found.time_of(5), Some(4 * 469));
    }

    #[test]
    fn the_path_says_which_track_the_file_belongs_to() {
        let (dat, _) = written();
        assert_eq!(read(&dat).unwrap().path, "/Contents/Artist/Track.flac");
    }

    #[test]
    fn cues_come_back_with_their_names_and_colours() {
        let (dat, ext) = written();
        let found = read_files(&dat, Some(&ext)).unwrap();

        assert_eq!(found.cues.len(), 4, "{:?}", found.cues);
        let memory = found.cues.iter().find(|cue| !cue.is_hot()).unwrap();
        assert_eq!(memory.time_ms, 0);

        let named = found.cues.iter().find(|cue| cue.hot_cue == 1).unwrap();
        assert_eq!(named.time_ms, 4_000);
        assert_eq!(named.comment.as_deref(), Some("hold me closer now"));
        assert_eq!(named.color, Some(Rgb { r: 0xe8, g: 0x3c, b: 0x9e }));

        // A cue with no name of its own comes back without one rather than
        // with an empty string, which is a different thing to show.
        let plain = found.cues.iter().find(|cue| cue.hot_cue == 2).unwrap();
        assert_eq!(plain.comment, None);
    }

    #[test]
    fn a_saved_loop_is_still_a_loop() {
        let (dat, ext) = written();
        let found = read_files(&dat, Some(&ext)).unwrap();
        let looping = found.cues.iter().find(|cue| cue.hot_cue == 3).unwrap();
        assert_eq!(looping.kind, CueKind::Loop { end_ms: 13_875 });
    }

    #[test]
    fn the_phrases_unmask_to_what_was_written() {
        let (_, ext) = written();
        let found = read(&ext).unwrap().structure.expect("no phrases came back");
        assert_eq!(found, structure());
    }

    #[test]
    fn a_whole_track_survives_the_round_trip() {
        let (dat, ext) = written();
        let found = read_files(&dat, Some(&ext)).unwrap();
        assert_eq!(found.cues, cues());
        assert_eq!(found.grid, grid());
        assert_eq!(found.structure, Some(structure()));
    }

    #[test]
    fn a_cue_the_player_deleted_does_not_survive_in_the_older_section() {
        // The extended list is the whole of its kind, not an addition to it.
        // Reading the two as a union would put a deleted cue back.
        let grid = grid();
        let waveforms = WaveformData::silent(8.0);
        let all = cues();
        let fewer: Vec<Cue> = all.iter().filter(|cue| cue.hot_cue != 2).cloned().collect();

        let mut sections = vec![
            anlz::path("/Contents/Artist/Track.flac"),
            anlz::cues(&all, false),
            anlz::cues(&all, true),
            anlz::cues_extended(&fewer, false),
            anlz::cues_extended(&fewer, true),
        ];
        sections.push(anlz::beat_grid(&grid));
        let _ = waveforms;

        let found = read(&anlz::file(&sections)).unwrap();
        assert!(
            found.cues.iter().all(|cue| cue.hot_cue != 2),
            "a deleted cue came back: {:?}",
            found.cues
        );
        assert_eq!(found.cues.len(), 3);
    }

    #[test]
    fn a_file_that_is_not_one_says_so() {
        assert!(read(b"not an analysis file at all").is_err());
        assert!(read(&[]).is_err());
        let err = read(b"PMAI\x00\x00\xff\xff\x00\x00\x00\x20").unwrap_err();
        assert!(err.to_string().contains("not a length"), "{err}");
    }

    #[test]
    fn a_file_cut_short_gives_up_what_it_still_has() {
        // A stick pulled out mid-write. The grid is in the first half and
        // should still be readable.
        let (dat, _) = written();
        let half = &dat[..dat.len() * 3 / 4];
        let found = read(half).expect("a truncated file should still open");
        assert!(!found.path.is_empty(), "the path is at the front and should have survived");
    }

    #[test]
    fn a_section_nobody_here_knows_about_is_stepped_over() {
        let mut sections = vec![anlz::path("/Contents/A/B.flac")];
        // Something from a future player, between two sections we do read.
        let mut invented = b"PZZZ".to_vec();
        invented.extend_from_slice(&0x10u32.to_be_bytes());
        invented.extend_from_slice(&0x20u32.to_be_bytes());
        invented.extend(std::iter::repeat_n(0xabu8, 0x20 - 12));
        sections.push(invented);
        sections.push(anlz::beat_grid(&grid()));

        let found = read(&anlz::file(&sections)).unwrap();
        assert_eq!(found.path, "/Contents/A/B.flac");
        assert_eq!(found.grid.beats.len(), 16, "the grid after it was not reached");
    }

    #[test]
    fn a_track_with_no_cues_reads_as_having_none() {
        let grid = grid();
        let waveforms = WaveformData::silent(8.0);
        let files = anlz::Analysis {
            on_drive_path: "/Contents/A/B.flac",
            grid: &grid,
            cues: &[],
            waveforms: &waveforms,
            structure: None,
            vbr: None,
        };
        let found = read_files(&files.dat(), Some(&files.ext())).unwrap();
        assert!(found.cues.is_empty());
        assert!(found.structure.is_none());
        assert_eq!(found.grid.beats.len(), 16);
    }
}
