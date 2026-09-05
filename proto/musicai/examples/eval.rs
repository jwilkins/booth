//! Measure the analysers against a real, hand-labelled library.
//!
//! Point this at a `rekordbox.xml` — File → Export Collection in rekordbox —
//! and it decodes each track the file references, runs our analysis, and
//! compares the key and tempo we found against the ones a DJ (or rekordbox
//! itself) already assigned. That turns "passes the synthetic tests" into a
//! number on real music, which is what the spec's ANA-2 and ANA-4 ask for.
//!
//! ```sh
//! cargo run --release -p booth-core --example eval -- ~/rekordbox.xml
//! # or, if the audio has moved since the XML was written:
//! cargo run --release -p booth-core --example eval -- ~/rekordbox.xml --root ~/Music
//! ```
//!
//! Nothing here is a test in the `cargo test` sense: it needs files that live
//! on your disk, not in the repo, so it is a program you run by hand.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use booth_core::analysis;
use booth_core::analysis::key::Key;
use booth_core::audio::decode::decode_file;

/// One track's worth of what the library claims and what we found.
struct Row {
    title: String,
    ref_key: Option<Key>,
    got_key: Option<Key>,
    ref_bpm: Option<f64>,
    got_bpm: f64,
}

fn main() {
    let mut args = std::env::args().skip(1);
    let xml = match args.next() {
        Some(path) => PathBuf::from(path),
        None => {
            eprintln!("usage: eval <rekordbox.xml> [--root DIR] [--limit N]");
            std::process::exit(2);
        }
    };

    let mut root: Option<PathBuf> = None;
    let mut limit = usize::MAX;
    let mut dry_run = false;
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--root" => root = args.next().map(PathBuf::from),
            "--limit" => limit = args.next().and_then(|n| n.parse().ok()).unwrap_or(usize::MAX),
            "--dry-run" => dry_run = true,
            other => {
                eprintln!("unknown argument {other}");
                std::process::exit(2);
            }
        }
    }

    let text = std::fs::read_to_string(&xml)
        .unwrap_or_else(|e| fatal(&format!("reading {}: {e}", xml.display())));
    let tracks = parse_tracks(&text);
    if tracks.is_empty() {
        fatal("no <TRACK> elements found — is this a rekordbox collection export?");
    }
    eprintln!("{} tracks in {}", tracks.len(), xml.display());

    if dry_run {
        survey(&tracks, root.as_deref());
        return;
    }

    let mut rows = Vec::new();
    let mut missing = 0usize;
    for track in tracks.into_iter().take(limit) {
        let path = locate(&track.location, root.as_deref());
        let Some(path) = path else {
            missing += 1;
            continue;
        };
        let audio = match decode_file(&path) {
            Ok(audio) => audio,
            Err(e) => {
                eprintln!("  skip {}: {e}", track.title);
                continue;
            }
        };
        let listened = analysis::analyze(&audio);
        rows.push(Row {
            title: track.title,
            ref_key: track.tonality.as_deref().and_then(Key::parse),
            got_key: listened.key.as_ref().map(|k| k.key),
            ref_bpm: track.average_bpm.filter(|&b| b > 0.0),
            got_bpm: listened.bpm,
        });
        eprint!("\r  analysed {} tracks", rows.len());
    }
    eprintln!();
    if missing > 0 {
        eprintln!("{missing} tracks could not be found on disk (try --root)");
    }

    report(&rows);
}

/// Print the accuracy figures, and the disagreements, so a bad number can be
/// looked into rather than just noted.
fn report(rows: &[Row]) {
    let keyed: Vec<&Row> = rows.iter().filter(|r| r.ref_key.is_some()).collect();
    let mut exact = 0usize;
    let mut compatible = 0usize;
    let mut key_misses = Vec::new();
    for row in &keyed {
        let (Some(reference), Some(got)) = (row.ref_key, row.got_key) else {
            key_misses.push((row, "no key detected".to_string()));
            continue;
        };
        if got == reference {
            exact += 1;
        } else if harmonically_close(got, reference) {
            compatible += 1;
        } else {
            key_misses.push((row, format!("{} vs {}", got.camelot(), reference.camelot())));
        }
    }

    let bpmed: Vec<&Row> = rows.iter().filter(|r| r.ref_bpm.is_some()).collect();
    let mut bpm_exact = 0usize;
    let mut bpm_metrical = 0usize;
    let mut bpm_misses = Vec::new();
    // The metrical relationships a beat tracker legitimately confuses: a track
    // heard at half or double time, or at a triplet or dotted level (2/3, 3/2,
    // 3/4, 4/3). A DJ would still call these related, even if beatmatching at
    // the wrong one is a mistake.
    let ratios = [0.5, 2.0, 2.0 / 3.0, 3.0 / 2.0, 3.0 / 4.0, 4.0 / 3.0];
    for row in &bpmed {
        let reference = row.ref_bpm.unwrap();
        if close(row.got_bpm, reference, 0.02) {
            bpm_exact += 1;
        } else if ratios.iter().any(|r| close(row.got_bpm * r, reference, 0.02)) {
            bpm_metrical += 1;
        } else {
            bpm_misses.push((row, format!("{:.2} vs {:.2}", row.got_bpm, reference)));
        }
    }

    // How many of the outright misses are a track called the wrong mode on the
    // right root (A major for A minor). A cluster of these points at the key
    // profiles rather than the chromagram.
    let mode_flips = keyed
        .iter()
        .filter_map(|r| Some((r.ref_key?, r.got_key?)))
        .filter(|(reference, got)| got.tonic == reference.tonic && got.mode != reference.mode)
        .count();

    println!("\n== key ==  ({} tracks with a reference key)", keyed.len());
    if !keyed.is_empty() {
        percent("exact", exact, keyed.len());
        percent("exact or harmonically adjacent", exact + compatible, keyed.len());
        percent("same root, wrong mode", mode_flips, keyed.len());
        key_bias(&keyed);
    }
    println!("\n== tempo ==  ({} tracks with a reference tempo)", bpmed.len());
    if !bpmed.is_empty() {
        percent("within 2%", bpm_exact, bpmed.len());
        percent("within 2% at a metrical multiple", bpm_exact + bpm_metrical, bpmed.len());
    }

    print_misses("key disagreements", &key_misses);
    print_misses("tempo disagreements", &bpm_misses);
}

fn print_misses(heading: &str, misses: &[(&&Row, String)]) {
    if misses.is_empty() {
        return;
    }
    println!("\n{heading} ({}):", misses.len());
    for (row, detail) in misses.iter().take(40) {
        println!("  {:40}  {detail}", truncate(&row.title, 40));
    }
    if misses.len() > 40 {
        println!("  … and {} more", misses.len() - 40);
    }
}

fn percent(label: &str, count: usize, total: usize) {
    let pct = 100.0 * count as f64 / total as f64;
    println!("  {label:32} {count:4}/{total:<4}  {pct:5.1}%");
}

/// Which keys the detector reaches for more (or less) often than the library
/// says it should. A key detected far more than it is referenced is a bias —
/// the C-minor lean a loud kick produces looks like a big positive next to 5A.
/// This is the objective version of squinting at the disagreement list.
fn key_bias(keyed: &[&Row]) {
    let mut detected: BTreeMap<String, i32> = BTreeMap::new();
    let mut referenced: BTreeMap<String, i32> = BTreeMap::new();
    for row in keyed {
        if let Some(got) = row.got_key {
            *detected.entry(got.camelot()).or_default() += 1;
        }
        if let Some(reference) = row.ref_key {
            *referenced.entry(reference.camelot()).or_default() += 1;
        }
    }
    let mut bias: Vec<(String, i32)> = detected
        .keys()
        .chain(referenced.keys())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .map(|code| {
            let net = detected.get(code).copied().unwrap_or(0)
                - referenced.get(code).copied().unwrap_or(0);
            (code.clone(), net)
        })
        .collect();
    bias.sort_by_key(|(_, net)| -net.abs());
    let notable: Vec<&(String, i32)> =
        bias.iter().filter(|(_, net)| net.abs() >= 5).take(8).collect();
    if notable.is_empty() {
        return;
    }
    println!("  key bias (detected − referenced, worst first):");
    for (code, net) in notable {
        let sign = if *net > 0 { "+" } else { "" };
        println!("    {code:<4} {sign}{net}");
    }
}

/// Two keys a DJ would consider a safe mix: the same key, its relative
/// major/minor, or a neighbour on the Camelot wheel. This is the "within a
/// neighbour" bar ANA-4 sets, expressed the way the wheel expresses it.
fn harmonically_close(a: Key, b: Key) -> bool {
    let (an, al) = split_camelot(a.camelot());
    let (bn, bl) = split_camelot(b.camelot());
    let step = ((an as i32 - bn as i32 + 6).rem_euclid(12) - 6).abs();
    (al == bl && step <= 1) || (an == bn && al != bl)
}

fn split_camelot(code: String) -> (u32, char) {
    let letter = code.chars().last().unwrap();
    let number: u32 = code[..code.len() - 1].parse().unwrap();
    (number, letter)
}

fn close(a: f64, b: f64, tolerance: f64) -> bool {
    (a - b).abs() <= b * tolerance
}

// -- reading the XML -------------------------------------------------------

struct Track {
    title: String,
    location: String,
    tonality: Option<String>,
    average_bpm: Option<f64>,
}

/// Pull the tracks out of a rekordbox collection export.
///
/// A deliberately small reader: it walks to each `<TRACK` opening tag and reads
/// its attributes, which is all the key and tempo comparison needs. It does not
/// try to be a general XML parser, because it does not have to be — rekordbox's
/// output is regular, and the alternative is a dependency for a program run by
/// hand a handful of times.
fn parse_tracks(xml: &str) -> Vec<Track> {
    let mut tracks = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find("<TRACK ") {
        rest = &rest[start + "<TRACK ".len()..];
        let end = match rest.find('>') {
            Some(end) => end,
            None => break,
        };
        let attrs = parse_attributes(&rest[..end]);
        // The collection's TRACK elements carry a Location; the ones inside
        // PLAYLISTS carry only a Key reference, so a missing Location is how the
        // two are told apart.
        if let Some(location) = attrs.get("Location") {
            tracks.push(Track {
                title: attrs.get("Name").cloned().unwrap_or_default(),
                location: location.clone(),
                tonality: attrs.get("Tonality").filter(|s| !s.is_empty()).cloned(),
                average_bpm: attrs.get("AverageBpm").and_then(|b| b.parse().ok()),
            });
        }
        rest = &rest[end..];
    }
    tracks
}

fn parse_attributes(tag: &str) -> BTreeMap<String, String> {
    let mut attrs = BTreeMap::new();
    let bytes = tag.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // name
        while i < bytes.len() && (bytes[i] as char).is_whitespace() {
            i += 1;
        }
        let name_start = i;
        while i < bytes.len() && bytes[i] != b'=' && !(bytes[i] as char).is_whitespace() {
            i += 1;
        }
        let name = &tag[name_start..i];
        while i < bytes.len() && bytes[i] != b'"' {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        i += 1; // opening quote
        let value_start = i;
        while i < bytes.len() && bytes[i] != b'"' {
            i += 1;
        }
        if i > bytes.len() {
            break;
        }
        let value = unescape(&tag[value_start..i.min(bytes.len())]);
        if !name.is_empty() {
            attrs.insert(name.to_string(), value);
        }
        i += 1; // closing quote
    }
    attrs
}

fn unescape(text: &str) -> String {
    text.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
}

/// Turn a rekordbox `Location` (a `file://localhost/...` URL, percent-encoded)
/// into a path on this machine, honouring `--root` when the audio has moved.
fn locate(location: &str, root: Option<&Path>) -> Option<PathBuf> {
    let path = file_url_to_path(location)?;
    if let Some(root) = root {
        // Try the path as-is, then the filename under the given root, then a
        // deeper suffix match, so a library exported on another machine still
        // resolves.
        if path.exists() {
            return Some(path);
        }
        if let Some(name) = path.file_name() {
            let candidate = root.join(name);
            if candidate.exists() {
                return Some(candidate);
            }
        }
        return find_under(root, &path);
    }
    path.exists().then_some(path)
}

fn file_url_to_path(location: &str) -> Option<PathBuf> {
    let rest =
        location.strip_prefix("file://localhost").or_else(|| location.strip_prefix("file://"))?;
    Some(PathBuf::from(percent_decode(rest)))
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(byte) = u8::from_str_radix(&text[i + 1..i + 3], 16) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Look for a file by name somewhere under `root`, shallow first. Bounded so a
/// huge library does not turn into an unbounded walk.
fn find_under(root: &Path, wanted: &Path) -> Option<PathBuf> {
    let name = wanted.file_name()?;
    let mut queue = std::collections::VecDeque::from([root.to_path_buf()]);
    let mut budget = 20_000;
    while let Some(dir) = queue.pop_front() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            budget -= 1;
            if budget == 0 {
                return None;
            }
            let path = entry.path();
            if path.is_dir() {
                queue.push_back(path);
            } else if path.file_name() == Some(name) {
                return Some(path);
            }
        }
    }
    None
}

fn truncate(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        text.to_string()
    } else {
        format!("{}…", text.chars().take(width - 1).collect::<String>())
    }
}

/// Report what an eval run would cover, without decoding anything: how many
/// tracks carry a reference key, how many carry a tempo, how many parse, and
/// how many can actually be found on disk. Run it first to see how much of a
/// library is going to be measured before waiting for the decode.
fn survey(tracks: &[Track], root: Option<&Path>) {
    let with_key = tracks.iter().filter(|t| t.tonality.is_some()).count();
    let with_bpm = tracks.iter().filter(|t| t.average_bpm.is_some()).count();

    let mut unparsed = Vec::new();
    for track in tracks {
        if let Some(label) = &track.tonality {
            if Key::parse(label).is_none() {
                unparsed.push((track.title.clone(), label.clone()));
            }
        }
    }

    let local = tracks
        .iter()
        .filter(|t| file_url_to_path(&t.location).is_some_and(|p| p.is_absolute()))
        .count();
    let found = tracks.iter().filter(|t| locate(&t.location, root).is_some()).count();

    println!(
        "
== survey =="
    );
    percent("with a reference key", with_key, tracks.len());
    percent("with a reference tempo", with_bpm, tracks.len());
    percent("reference keys that parse", with_key - unparsed.len(), with_key.max(1));
    percent("locations that are file paths", local, tracks.len());
    percent("audio found on this machine", found, tracks.len());

    if found == 0 {
        println!(
            "
no audio is reachable from here — run this on the machine the library lives on,"
        );
        println!("or pass --root to point at where the files moved to.");
    }
    if !unparsed.is_empty() {
        println!(
            "
key labels that did not parse ({}):",
            unparsed.len()
        );
        for (title, label) in unparsed.iter().take(20) {
            println!("  {:40}  {label:?}", truncate(title, 40));
        }
    }
}

fn fatal(message: &str) -> ! {
    eprintln!("eval: {message}");
    std::process::exit(1);
}
