//! Booth's beats held up against two trackers that are not Booth's.
//!
//! madmom and Essentia, run by `scripts/reference/beats.py` over a folder of
//! records, each say what the tempo is and where the beats are, and madmom
//! where the bar starts. Neither is a listener, and neither is the dynamic
//! program Booth uses, so a record where both agree with each other and not
//! with Booth is one Booth has to answer for; a record where they disagree with
//! each other is reported and not counted.
//!
//! ```sh
//! BOOTH_REFERENCE_RECORDS=<records> BOOTH_REFERENCE_BEATS=<the script's output> \
//!     cargo test --release -p booth-cli --test reference_beats -- --ignored --nocapture
//! ```
//!
//! See `scripts/reference/README.md` for the environment the script needs.
//!
//! The records it was measured on are 41 CC records. Eighteen are from the
//! archive.org items unfound51, mtrnc003, RAR032_TTC_Auf_Den_Punkt_EP, efp006,
//! Jara_NosceTeIpsum, mne013-gartner-phobos, audcst031Blacksquare-Upside,
//! tou207, tou350, vault-kid-into-the-vault and ca015_va_cs, and from Jamendo
//! (Anton Vlasov's Hard Epic Dubstep, VogelAudio's Circuit Breaker). The other 23, one track from each, are from the netlabel
//! items Hfr011-mizukisLastChance-tacticalAssault, tkep009, tkep010, tkep017,
//! s27-043WeAreOne-Section27Netlabel, xgn028_-_martsman, mstk001, DWK123,
//! DWK127, mia049, gt536Maelstrom-ThePassage, quiz050, tou355, onmp225,
//! dig017_tromenia_ep, pan025, exp036, SCL160,
//! rest037-kay_grove_-_samba_440-night_walk, YkYk015, rfr020,
//! mz001_Mademoi_Selle and phoke33, picked for the genres where the octave is
//! in question: dubstep, drum and bass, hip hop, trance and garage.

use std::path::Path;

use booth_cli::analysis::analyze;
use booth_cli::audio::decode::decode_file;
use booth_cli::export::Beat;

/// How close two beats have to be to be the same beat: the window the
/// downbeat reference in `explain.rs` uses, and well inside the 117 ms of a
/// sixteenth at 128 BPM, so a beat a sixteenth off is not counted as on.
const WITHIN_MS: f64 = 70.0;

/// How near two tempos, or a ratio of them, have to be to count as the same.
///
/// Generous for two trackers on one steady record: of the 35 of the 41 that
/// madmom and Essentia agreed on, 29 were within half a per cent of each other
/// and five within one and a half. It is there for the one that drifts, Jet
/// Streams at 2.8, and it is still well short of the twelve per cent between
/// the nearest two of the slips below, 2/3 and 3/4.
const SAME: f64 = 0.03;

/// The ratios a tracker is known to slip by, each with how it reads.
const SLIPS: [(f64, &str); 7] = [
    (0.5, "half"),
    (2.0 / 3.0, "2/3"),
    (0.75, "3/4"),
    (1.0, "same"),
    (4.0 / 3.0, "4/3"),
    (1.5, "3/2"),
    (2.0, "double"),
];

/// The records Booth is known to read at another tempo than the two agree on,
/// by a piece of the file's name, with how it reads.
///
/// Six are the octave, which is a convention as much as a measurement: hip hop
/// at 86 is counted at 86 and drum and bass at 172 at 172, though a record of
/// either can repeat as strongly at the other or more so (The Werewolf half as
/// much again at 173), and the two are as far either side of the 120 Booth
/// leans towards. Telling them apart by where the snare falls, on the 2 and
/// the 4, fixed five of these six; but the band that hears the snare hears the
/// hats on the off-beat too, and it doubled 02_-_Get_Out from 95 and halved
/// 1-02FractureDsordre from 160.
///
/// The other five are a rhythm in threes over the four, or near one, taken for
/// the beat.
const KNOWN: [(&str, &str); 11] = [
    // Dubstep, half-time: it repeats most strongly every two beats of 140.
    ("Hard_Epic_Dubstep", "half"),
    // Hard techno at 142, with the clap on every other beat.
    ("dillerkopp", "half"),
    ("Procedure238", "double"),
    ("The_Werewolf", "double"),
    ("thats_just_what_its_for", "double"),
    ("Oldvision", "half"),
    // A bass stab every three half-beats.
    ("Circuit_Breaker", "2/3"),
    ("Phobos", "4/3"),
    ("Jet_Streams", "4/3"),
    ("bedes_ii", "0.89x"),
    // 120 read as 80: it repeats half as much again every three half-beats
    // as every beat.
    ("ryan_l", "2/3"),
];

/// What a ratio of two tempos reads as, or the number where it is none of them.
fn slip(ratio: f64) -> String {
    match SLIPS.iter().find(|(at, _)| (ratio / at - 1.0).abs() < SAME) {
        Some((_, name)) => name.to_string(),
        None => format!("{ratio:.2}x"),
    }
}

/// A tempo read off beat times in seconds: the median span of eight beats.
///
/// Eight rather than one, because madmom's beats come in hundredths of a
/// second and at 128 BPM a single gap of 0.47 s can only read as 0.46 or 0.47,
/// which is two per cent either way.
fn tempo_of(beats: &[f64]) -> Option<f64> {
    let mut spans: Vec<f64> = beats.windows(9).map(|w| (w[8] - w[0]) / 8.0).collect();
    if spans.is_empty() {
        return None;
    }
    spans.sort_by(f64::total_cmp);
    Some(60.0 / spans[spans.len() / 2])
}

/// The beat of Booth's nearest to a moment in seconds, if it is within reach.
fn nearest(grid: &[Beat], seconds: f64) -> Option<&Beat> {
    let at = seconds * 1000.0;
    let i = grid.partition_point(|beat| (beat.time_ms as f64) < at);
    [i.checked_sub(1), Some(i)]
        .into_iter()
        .flatten()
        .filter_map(|i| grid.get(i))
        .min_by(|a, b| (a.time_ms as f64 - at).abs().total_cmp(&(b.time_ms as f64 - at).abs()))
        .filter(|beat| (beat.time_ms as f64 - at).abs() <= WITHIN_MS)
}

/// A file name short enough for a column and still told from its neighbours,
/// which on an album share the start.
fn short(name: &str) -> String {
    let chars: Vec<char> = name.chars().collect();
    match chars.len() > 28 {
        true => {
            let head: String = chars[..12].iter().collect();
            let tail: String = chars[chars.len() - 15..].iter().collect();
            format!("{head}~{tail}")
        }
        false => name.to_string(),
    }
}

/// One record, as each tracker saw it.
struct Compared {
    booth: f64,
    madmom: f64,
    essentia: f64,
    /// Of madmom's beats, how many have one of Booth's on them.
    on_booth: f64,
    /// Of madmom's ones that land on a beat of Booth's, the share on each of
    /// Booth's four beats of the bar.
    ones_on: [f64; 4],
}

fn compare(record: &Path, reference: &Path) -> Option<Compared> {
    let text = std::fs::read_to_string(reference).ok()?;
    let found: serde_json::Value = serde_json::from_str(&text).unwrap();
    let madmom: Vec<(f64, u64)> = found["madmom"]["beats"]
        .as_array()?
        .iter()
        .map(|beat| (beat[0].as_f64().unwrap(), beat[1].as_u64().unwrap()))
        .collect();
    let essentia = found["essentia"]["bpm"].as_f64()?;

    let analysis = analyze(&decode_file(record).unwrap());
    let grid = &analysis.grid.beats;
    let times: Vec<f64> = madmom.iter().map(|&(t, _)| t).collect();
    let on = times.iter().filter(|&&t| nearest(grid, t).is_some()).count();
    let mut ones = [0usize; 4];
    for &(t, position) in &madmom {
        if let (1, Some(beat)) = (position, nearest(grid, t)) {
            ones[(beat.number as usize).clamp(1, 4) - 1] += 1;
        }
    }
    let landed = ones.iter().sum::<usize>().max(1) as f64;
    Some(Compared {
        booth: analysis.bpm,
        madmom: tempo_of(&times)?,
        essentia,
        on_booth: on as f64 / times.len().max(1) as f64,
        ones_on: ones.map(|n| n as f64 / landed),
    })
}

#[test]
#[ignore = "needs records and their reference beats: BOOTH_REFERENCE_RECORDS, BOOTH_REFERENCE_BEATS"]
fn booth_finds_every_tempo_two_other_trackers_agree_on_but_the_known_misses() {
    let var = |name: &str| std::env::var_os(name).map(std::path::PathBuf::from);
    let (Some(records), Some(beats)) =
        (var("BOOTH_REFERENCE_RECORDS"), var("BOOTH_REFERENCE_BEATS"))
    else {
        return;
    };
    let mut paths: Vec<_> = std::fs::read_dir(&records)
        .unwrap()
        .filter_map(|entry| Some(entry.ok()?.path()))
        .filter(|path| {
            path.extension().is_some_and(|ext| ext == "mp3" || ext == "flac" || ext == "wav")
        })
        .collect();
    paths.sort();

    eprintln!(
        "{:<28} {:>7} {:>7} {:>7}  {:<14} {:>5}  madmom's 1s on Booth's 1 2 3 4",
        "record", "Booth", "madmom", "Essent.", "Booth/others", "beats"
    );
    let (mut counted, mut right, mut wrong, mut mended) = (0, 0, Vec::new(), Vec::new());
    for path in paths {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let stem = path.file_stem().unwrap().to_string_lossy().to_string();
        let Some(r) = compare(&path, &beats.join(format!("{stem}.json"))) else {
            continue;
        };
        let agreed = slip(r.madmom / r.essentia) == "same";
        let against = match agreed {
            true => slip(r.booth / r.madmom),
            false => format!("{}, {}", slip(r.booth / r.madmom), slip(r.booth / r.essentia)),
        };
        let ones = r.ones_on.map(|share| format!("{:>3.0}", share * 100.0)).join(" ");
        eprintln!(
            "{:<28} {:>7.2} {:>7.2} {:>7.2}  {:<14} {:>4.0}%  {ones}",
            short(&stem),
            r.booth,
            r.madmom,
            r.essentia,
            against,
            r.on_booth * 100.0
        );
        if !agreed {
            continue;
        }
        counted += 1;
        let known = KNOWN.iter().find(|(piece, _)| name.contains(piece)).map(|(_, slip)| *slip);
        match (against == "same", known) {
            (true, None) => right += 1,
            (true, Some(_)) => mended.push(short(&stem)),
            (false, Some(slip)) if slip == against => {}
            (false, _) => wrong.push(format!("{} ({against})", short(&stem))),
        }
    }
    eprintln!("\nof {counted} records the two agree on, Booth has the same tempo on {right}");
    if !mended.is_empty() {
        eprintln!("now right, and to come off the known misses: {mended:?}");
    }
    assert!(
        wrong.is_empty(),
        "Booth's tempo where madmom and Essentia agree on another: {wrong:?}"
    );
}

#[test]
fn a_ratio_of_tempos_is_named_by_the_slip_it_is_near() {
    assert_eq!(slip(0.502), "half");
    assert_eq!(slip(1.49), "3/2");
    assert_eq!(slip(1.0), "same");
    assert_eq!(slip(1.31), "4/3");
    assert_eq!(slip(1.2), "1.20x");
}

#[test]
fn a_tempo_read_off_beats_in_hundredths_is_not_two_per_cent_out() {
    // 128 BPM, each beat rounded to the hundredth madmom reports it in.
    let beats: Vec<f64> =
        (0..64).map(|i| (i as f64 * 60.0 / 128.0 * 100.0).round() / 100.0).collect();
    let tempo = tempo_of(&beats).unwrap();
    assert!((tempo / 128.0 - 1.0).abs() < 0.005, "{tempo}");
}

#[test]
fn a_long_name_keeps_its_start_and_its_end() {
    let a = short("Vault_Kid_-_Into_the_Vault_-_01_Trainjam_feat._DEFENSE_MECHANISM");
    let b = short("Vault_Kid_-_Into_the_Vault_-_02_Treasure");
    assert_ne!(a, b);
    assert!(a.chars().count() <= 28 && b.chars().count() <= 28, "{a} {b}");
    assert_eq!(short("sundown"), "sundown");
}
