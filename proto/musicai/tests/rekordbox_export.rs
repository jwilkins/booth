//! Checking the analysis files we write against somebody else's parser.
//!
//! Our own reader agreeing with our own writer proves nothing: a
//! misunderstanding of the format would be symmetrical and invisible. So these
//! tests parse what we produce with [`rekordcrate`], an independent
//! implementation built from the same public documentation by different people,
//! and assert that what comes back out is what went in.
//!
//! This is the smallest form of the check every exported drive is meant to get.

use std::io::Cursor;

use binrw::BinRead;
use booth_cli::audio::Audio;
use booth_cli::export::{anlz, waveform, BeatGrid, Cue, Mood, Phrase, SongStructure};
use rekordcrate::anlz::{Content, ContentKind, CueListType, CueType, ANLZ};

const SAMPLE_RATE: u32 = 44_100;

/// Four bars of something with content in every band, so the waveforms are not
/// all zero.
fn track(seconds: f32) -> Audio {
    let frames = (SAMPLE_RATE as f32 * seconds) as usize;
    let plane: Vec<f32> = (0..frames)
        .map(|i| {
            let t = i as f32 / SAMPLE_RATE as f32;
            let two_pi = 2.0 * std::f32::consts::PI;
            0.5 * (two_pi * 60.0 * t).sin()
                + 0.25 * (two_pi * 700.0 * t).sin()
                + 0.1 * (two_pi * 9_000.0 * t).sin()
        })
        .collect();
    Audio::new(SAMPLE_RATE, vec![plane.clone(), plane]).unwrap()
}

fn parse(bytes: &[u8]) -> ANLZ {
    ANLZ::read(&mut Cursor::new(bytes)).expect("rekordcrate could not parse what we wrote")
}

fn kinds(file: &ANLZ) -> Vec<ContentKind> {
    file.sections.iter().map(|s| s.header.kind.clone()).collect()
}

struct Fixture {
    grid: BeatGrid,
    cues: Vec<Cue>,
    waveforms: waveform::WaveformData,
    structure: SongStructure,
}

impl Fixture {
    fn new() -> Self {
        let audio = track(8.0);
        Self {
            grid: BeatGrid::constant(128.0, 412, 8_000),
            // Loops throughout, rather than the point cues a DJ would
            // actually set. See `a_point_cue_uses_the_type_the_format_docs_give`
            // for why: the two parsers disagree about what a point cue is, and
            // everything else here is meant to be checking *our* work.
            cues: vec![
                Cue::memory(412).looping(2_287),
                Cue::hot(1, 412).looping(2_287).with_comment("intro").with_color(226, 160, 63),
                Cue::hot(2, 4_000)
                    .looping(5_875)
                    .with_comment("first drop")
                    .with_color(47, 111, 208),
                Cue::hot(3, 6_000).looping(7_875),
            ],
            waveforms: waveform::analyze(&audio),
            structure: SongStructure {
                mood: Mood::Mid,
                end_beat: 16,
                bank: 0,
                phrases: vec![
                    Phrase { beat: 1, kind: 1, ..Phrase::default() },
                    Phrase { beat: 9, kind: 9, ..Phrase::default() },
                    Phrase { beat: 13, kind: 10, ..Phrase::default() },
                ],
            },
        }
    }

    fn analysis(&self) -> anlz::Analysis<'_> {
        anlz::Analysis {
            on_drive_path: "/Contents/Peverelist/Roll With The Punches.flac",
            grid: &self.grid,
            cues: &self.cues,
            waveforms: &self.waveforms,
            structure: Some(&self.structure),
            vbr: None,
        }
    }
}

#[test]
fn a_dat_file_holds_what_a_2009_player_looks_for() {
    let fixture = Fixture::new();
    let file = parse(&fixture.analysis().dat());
    assert_eq!(
        kinds(&file),
        [
            ContentKind::Path,
            // Written even for a file with no variable bitrate to index, as a
            // table of zeroes, because every real export has one.
            ContentKind::VBR,
            ContentKind::BeatGrid,
            ContentKind::WaveformPreview,
            ContentKind::TinyWaveformPreview,
            ContentKind::CueList,
            ContentKind::CueList,
        ]
    );
}

#[test]
fn the_path_survives() {
    let fixture = Fixture::new();
    let file = parse(&fixture.analysis().dat());
    match &file.sections[0].content {
        Content::Path(path) => {
            assert_eq!(path.path.to_string(), "/Contents/Peverelist/Roll With The Punches.flac")
        }
        other => panic!("expected a path, got {other:?}"),
    }
}

#[test]
fn every_beat_comes_back_with_its_bar_position_tempo_and_time() {
    let fixture = Fixture::new();
    let file = parse(&fixture.analysis().dat());
    let Some(Content::BeatGrid(grid)) =
        file.sections.iter().map(|s| &s.content).find(|c| matches!(c, Content::BeatGrid(_)))
    else {
        panic!("expected a beat grid");
    };

    assert_eq!(grid.beats.len(), fixture.grid.beats.len());
    for (ours, theirs) in fixture.grid.beats.iter().zip(&grid.beats) {
        assert_eq!(theirs.beat_number, ours.number);
        assert_eq!(theirs.tempo, ours.tempo_x100);
        assert_eq!(theirs.time, ours.time_ms);
    }

    // 128 BPM from 412 ms into an eight-second track.
    assert_eq!(grid.beats[0].time, 412);
    assert_eq!(grid.beats[0].beat_number, 1);
    assert_eq!(grid.beats[0].tempo, 12_800);
    assert_eq!(grid.beats[4].beat_number, 1, "bar lines fall every four beats");
}

#[test]
fn hot_cues_and_memory_cues_land_in_their_own_lists() {
    let fixture = Fixture::new();
    let file = parse(&fixture.analysis().dat());

    let lists: Vec<_> = file
        .sections
        .iter()
        .filter_map(|s| match &s.content {
            Content::CueList(list) => Some(list),
            _ => None,
        })
        .collect();

    let memory = lists.iter().find(|l| l.list_type == CueListType::MemoryCues).unwrap();
    let hot = lists.iter().find(|l| l.list_type == CueListType::HotCues).unwrap();
    assert_eq!(memory.cues.len(), 1);
    assert_eq!(hot.cues.len(), 3);

    assert_eq!(hot.cues[0].hot_cue, 1);
    assert_eq!(hot.cues[0].time, 412);
    assert_eq!(hot.cues[2].cue_type, CueType::Loop);
    assert_eq!(hot.cues[2].time, 6_000);
    assert_eq!(hot.cues[2].loop_time, 7_875);
}

#[test]
fn an_ext_file_carries_the_colour_waveforms_and_the_named_cues() {
    let fixture = Fixture::new();
    let file = parse(&fixture.analysis().ext());
    assert_eq!(
        kinds(&file),
        [
            ContentKind::Path,
            ContentKind::WaveformDetail,
            ContentKind::CueList,
            ContentKind::CueList,
            ContentKind::ExtendedCueList,
            ContentKind::ExtendedCueList,
            // No beat grid: rekordbox puts a PQT2 here, whose layout is not
            // published, and a copy of the .DAT's PQTZ in its place cost a
            // CDJ-3000X everything after it — the colour waveforms and the
            // phrases.
            ContentKind::WaveformColorDetail,
            ContentKind::WaveformColorPreview,
            ContentKind::SongStructure,
        ]
    );
}

#[test]
fn comments_and_colours_survive_the_extended_cue_format() {
    let fixture = Fixture::new();
    let file = parse(&fixture.analysis().ext());

    let hot = file
        .sections
        .iter()
        .find_map(|s| match &s.content {
            Content::ExtendedCueList(list) if list.list_type == CueListType::HotCues => Some(list),
            _ => None,
        })
        .expect("no hot cue list");

    assert_eq!(hot.cues.len(), 3);
    assert_eq!(hot.cues[0].comment.to_string(), "intro");
    assert_eq!(hot.cues[1].comment.to_string(), "first drop");
    assert_eq!(hot.cues[1].time, 4_000);
    // A loop with no comment still writes the empty string rather than nothing.
    assert_eq!(hot.cues[2].comment.to_string(), "");
    assert_eq!(hot.cues[2].loop_time, 7_875);
}

#[test]
fn the_phrase_analysis_unmasks_to_what_we_put_in() {
    let fixture = Fixture::new();
    let file = parse(&fixture.analysis().ext());

    let Some(Content::SongStructure(structure)) =
        file.sections.iter().map(|s| &s.content).find(|c| matches!(c, Content::SongStructure(_)))
    else {
        panic!("no song structure section");
    };

    // rekordcrate detects the mask, removes it, and reads the phrases. If our
    // masking were wrong this would fail to parse rather than fail an assert.
    let data = format!("{structure:?}");
    assert!(data.contains("Mid"), "mood did not survive: {data}");
    assert!(data.contains("end_beat: 16"), "end beat did not survive: {data}");
    for phrase in &fixture.structure.phrases {
        assert!(
            data.contains(&format!("beat: {}", phrase.beat)),
            "phrase at beat {} is missing: {data}",
            phrase.beat
        );
    }
}

#[test]
fn the_waveforms_are_the_length_the_track_is() {
    let fixture = Fixture::new();
    let ext = parse(&fixture.analysis().ext());

    // Eight seconds at 150 columns a second.
    let expected = 8 * 150;
    for section in &ext.sections {
        match &section.content {
            Content::WaveformDetail(w) => assert_eq!(w.data.len(), expected),
            Content::WaveformColorDetail(w) => assert_eq!(w.data.len(), expected),
            Content::WaveformColorPreview(w) => assert_eq!(w.data.len(), 1_200),
            _ => {}
        }
    }

    let dat = parse(&fixture.analysis().dat());
    for section in &dat.sections {
        match &section.content {
            Content::WaveformPreview(w) => assert_eq!(w.data.len(), 400),
            Content::TinyWaveformPreview(w) => assert_eq!(w.data.len(), 100),
            _ => {}
        }
    }
}

#[test]
fn a_waveform_column_carries_both_a_height_and_a_colour() {
    let fixture = Fixture::new();
    let ext = parse(&fixture.analysis().ext());

    let Some(Content::WaveformColorDetail(detail)) = ext
        .sections
        .iter()
        .map(|s| &s.content)
        .find(|c| matches!(c, Content::WaveformColorDetail(_)))
    else {
        panic!("no colour detail waveform");
    };

    let middle = &detail.data[detail.data.len() / 2];
    assert!(middle.height() > 0, "a track with sound in it drew an empty column");
    assert!(
        middle.red() > 0 || middle.green() > 0 || middle.blue() > 0,
        "a column with height should have a colour"
    );
}

#[test]
fn a_2ex_file_frames_the_three_band_waveforms() {
    let fixture = Fixture::new();
    let bytes = fixture.analysis().two_ex();
    let file = parse(&bytes);

    // rekordcrate 0.3 predates the CDJ-3000's three-band sections, so it reads
    // them as unknown — which is exactly the check that matters here: the
    // section framing is right even to a parser that has never heard of them.
    //
    // Detail, then preview, then the twenty-byte summary. This test asserted
    // the opposite for three releases, on a comment rather than a file; see
    // `the_section_order_is_the_one_two_real_exports_use` for where the order
    // now comes from.
    assert_eq!(file.sections.len(), 4);
    assert_eq!(file.sections[0].header.kind, ContentKind::Path);
    assert_eq!(file.sections[1].header.kind, ContentKind::Unknown(*b"PWV7"));
    assert_eq!(file.sections[2].header.kind, ContentKind::Unknown(*b"PWV6"));
    assert_eq!(file.sections[3].header.kind, ContentKind::Unknown(*b"PWVC"));

    // And our own reader, which does know about them, agrees on the sizes.
    let ours = anlz::inspect(&bytes).unwrap();
    assert_eq!(ours[1].summary, format!("{} entries of 3 bytes", 8 * 150));
    assert_eq!(ours[2].summary, "1200 entries of 3 bytes");
}

/// Every section's four-character code and header words, against the two real
/// rekordbox exports in `rekordcrate`'s `data/complete_export`.
///
/// Read out of those files with a twenty-line script rather than taken from
/// anybody's documentation, because two of the numbers below had been wrong
/// since they were first written and no amount of reading the format notes was
/// going to say so. The fixtures are not reachable from here — they live in
/// the registry copy of a dependency, at a path that is this machine's — so
/// what they said is transcribed, and the transcription is what this pins.
///
/// What a header word means is still unknown. That it differs from a real
/// export is the whole of the claim.
#[test]
fn the_section_order_is_the_one_two_real_exports_use() {
    let fixture = Fixture::new();

    // `(fourcc, len_header, [words after fourcc/len_header/len_tag])`. The
    // entry counts and `len_tag`s are the track's own and are not compared;
    // everything else here is byte-for-byte what both fixtures carry.
    let expected: [(&[u8], &[&str], u32); 3] = [
        (b"DAT", &["PPTH", "PVBR", "PQTZ", "PWAV", "PWV2", "PCOB", "PCOB"], 0),
        (
            b"EXT",
            // No `PQT2`, which both fixtures have between the cue lists and
            // `PWV5`. Leaving it out is deliberate — its layout is a 0x38
            // header of eleven words that nobody has published — and the
            // CDJ-3000X result says a section simply missing is skipped,
            // where a wrong one costs everything behind it.
            &["PPTH", "PWV3", "PCOB", "PCOB", "PCO2", "PCO2", "PWV5", "PWV4", "PSSI"],
            0,
        ),
        (b"2EX", &["PPTH", "PWV7", "PWV6", "PWVC"], 0),
    ];

    let analysis = fixture.analysis();
    for (which, fourccs, _) in expected {
        let bytes = match which {
            b"DAT" => analysis.dat(),
            b"EXT" => analysis.ext(),
            _ => analysis.two_ex(),
        };
        let found: Vec<String> = sections(&bytes).into_iter().map(|(code, ..)| code).collect();
        let want: Vec<String> = fourccs.iter().map(|s| s.to_string()).collect();
        assert_eq!(found, want, "{} section order", String::from_utf8_lossy(which));
    }

    // The header words that differ between sections, and the two that were
    // wrong. `PWV6` alone has no third word at all, so its header is 0x14
    // where the other scrolling and preview sections are 0x18; `PWV4`'s third
    // word is zero where `PWV3`, `PWV5` and `PWV7` carry 0x00960000.
    let ext = sections(&analysis.ext());
    let pwv3 = ext.iter().find(|(code, ..)| code == "PWV3").unwrap();
    let pwv4 = ext.iter().find(|(code, ..)| code == "PWV4").unwrap();
    let pwv5 = ext.iter().find(|(code, ..)| code == "PWV5").unwrap();
    assert_eq!((pwv3.1, pwv3.2[0], pwv3.2[2]), (0x18, 1, 0x0096_0000));
    assert_eq!((pwv4.1, pwv4.2[0], pwv4.2[2]), (0x18, 6, 0x0000_0000), "PWV4's third word");
    assert_eq!((pwv5.1, pwv5.2[0], pwv5.2[2]), (0x18, 2, 0x0096_0305));

    let two_ex = sections(&analysis.two_ex());
    let pwv6 = two_ex.iter().find(|(code, ..)| code == "PWV6").unwrap();
    let pwv7 = two_ex.iter().find(|(code, ..)| code == "PWV7").unwrap();
    assert_eq!((pwv6.1, pwv6.2.len()), (0x14, 2), "PWV6 carries no third word");
    assert_eq!((pwv7.1, pwv7.2[2]), (0x18, 0x0096_0000));
    let pwvc = two_ex.iter().find(|(code, ..)| code == "PWVC").unwrap();
    assert_eq!((pwvc.1, pwvc.0.len()), (0x0e, 4), "PWVC is a 0x0e header and twenty bytes");
}

/// Every section of an analysis file as `(fourcc, len_header, header words)`.
///
/// Deliberately not `anlz::inspect`: that reads a section's payload and says
/// what it holds, and what is wanted here is the framing a player walks, down
/// to the words whose meaning nobody knows.
fn sections(bytes: &[u8]) -> Vec<(String, u32, Vec<u32>)> {
    let be32 = |at: usize| u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap());
    let mut out = Vec::new();
    let mut at = be32(4) as usize;
    while at + 12 <= bytes.len() {
        let code = String::from_utf8_lossy(&bytes[at..at + 4]).into_owned();
        let len_header = be32(at + 4);
        let len_tag = be32(at + 8) as usize;
        let words =
            (12..len_header as usize).step_by(4).map(|off| be32(at + off)).collect::<Vec<_>>();
        out.push((code, len_header, words));
        if len_tag == 0 {
            break;
        }
        at += len_tag;
    }
    out
}

#[test]
fn a_seek_index_round_trips_through_the_independent_parser() {
    use rekordcrate::anlz::ContentKind;

    let fixture = Fixture::new();
    // A believable VBR table: monotonic offsets ending at a file length.
    let mut offsets = [0u32; 401];
    for (i, slot) in offsets.iter_mut().enumerate() {
        *slot = (i as u32) * 5_000 + (i as u32 % 7) * 137;
    }
    offsets[400] = 2_048_000;

    let analysis = anlz::Analysis {
        on_drive_path: "/Contents/track.mp3",
        grid: &fixture.grid,
        cues: &fixture.cues,
        waveforms: &fixture.waveforms,
        structure: Some(&fixture.structure),
        vbr: Some(&offsets),
    };
    let file = parse(&analysis.dat());

    // rekordcrate reads PVBR as a VBR section; it sits right after the path.
    assert_eq!(file.sections[0].header.kind, ContentKind::Path);
    assert_eq!(file.sections[1].header.kind, ContentKind::VBR);
    // The whole file parses, which is the point — a malformed section length
    // would have stopped it at PVBR.
    assert!(kinds(&file).contains(&ContentKind::BeatGrid));
}

#[test]
fn a_track_with_no_cues_or_phrases_still_produces_readable_files() {
    let audio = track(2.0);
    let grid = BeatGrid::constant(174.0, 0, 2_000);
    let waveforms = waveform::analyze(&audio);
    let analysis = anlz::Analysis {
        on_drive_path: "/Contents/empty.flac",
        grid: &grid,
        cues: &[],
        waveforms: &waveforms,
        structure: None,
        vbr: None,
    };

    let dat = parse(&analysis.dat());
    let ext = parse(&analysis.ext());
    assert_eq!(kinds(&dat).len(), 7);
    assert!(!kinds(&ext).contains(&ContentKind::SongStructure));
}

/// A disagreement between two independent readings of the format, recorded
/// rather than papered over.
///
/// Deep Symmetry's format documentation says a cue entry's type is 1 for a
/// point and 2 for a loop, and says it twice — once for each cue format. The
/// Kaitai structures behind `crate-digger` say the same. `rekordcrate` 0.3
/// instead defines the point as 0 and has no variant for 1, so it cannot read a
/// point cue at all.
///
/// We follow the documentation. The rekordbox export that ships with
/// `rekordcrate` has no cues in it, so neither reading can be confirmed from a
/// real file, and this is on the list of things to settle against a player. The
/// test is written to survive being right *or* being wrong: what it pins down
/// is that we know which value we write and why.
#[test]
fn a_point_cue_uses_the_type_the_format_docs_give() {
    let list = [Cue::hot(1, 1_000)];
    let bytes = anlz::cues_extended(&list, true);
    // Twenty bytes of section header, then a cue entry whose type sits sixteen
    // bytes in, after the entry's own header and its hot cue number.
    assert_eq!(bytes[20 + 16], 1, "a point cue should be type 1");

    let file = booth_cli::export::anlz::file(&[bytes]);
    match ANLZ::read(&mut Cursor::new(&file)) {
        Ok(parsed) => {
            // rekordcrate has come round to the documented value.
            let Content::ExtendedCueList(list) = &parsed.sections[0].content else {
                panic!("expected a cue list");
            };
            assert_eq!(list.cues[0].cue_type, CueType::Point);
        }
        Err(e) => assert!(
            e.to_string().contains("Unexpected value for enum: 1"),
            "expected the known disagreement about cue types, got: {e}"
        ),
    }
}
