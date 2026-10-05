//! The tagged-section analysis files: `.DAT`, `.EXT` and `.2EX`.
//!
//! Every file is the four-character code `PMAI`, a header, and then a run of
//! sections. Each section is its own four-character code, the length of its
//! header, the length of the whole section, and a payload whose shape depends
//! on the code. Nothing is compressed and only one section is obfuscated (see
//! [`song_structure`]).
//!
//! Which sections go in which file is a compatibility decision rather than a
//! technical one: `.DAT` holds what a 2009 player can read, `.EXT` adds what
//! the nexus 2 line understands, and `.2EX` carries the CDJ-3000's three-band
//! waveforms. A player reads the richest file it knows about and ignores the
//! rest, which is why writing all three is the safe thing to do.

use anyhow::{bail, Result};

use super::waveform::WaveformData;
use super::{BeatGrid, Cue, CueKind, SongStructure};

/// The header length rekordbox writes at the top of every analysis file. The
/// sixteen bytes after `len_file` have no known purpose and are written as
/// zeroes.
const FILE_HEADER_LEN: u32 = 0x1c;

/// Where a track's analysis files go, worked out from where its audio went.
///
/// # Why it is a hash and not a number
///
/// Every track on a drive gets a directory of its own under
/// `PIONEER/USBANLZ`, named `P{three hex}/{eight hex}`, and it looked for a
/// long time as though the names could be anything as long as the database
/// pointed at them. They cannot. A player computes this name itself, from the
/// path of the audio file, and looks only there — so an analysis file the
/// database points at perfectly and that sits under any other name is a file
/// the player never opens.
///
/// That was this program's bug: it named these directories after the track id,
/// wrote grids, waveforms, cues and phrases into them, and a CDJ-3000X showed
/// none of it. Not slowly, not wrongly — it showed nothing at all and did not
/// pause to analyse, because as far as the player was concerned the track was
/// analysed and the analysis was simply missing.
///
/// The hash is `morizkraemer/fourfour`'s, disassembled out of rekordbox's own
/// `CreateAnlzFileFolderPath`. Two of the three worked examples they publish
/// reproduce exactly here; the third does not, which is recorded in
/// `docs/onelibrary.md` §5.1 along with the rest of what is known.
///
/// The eight hex digits are the hash modulo a prime, so two tracks can land in
/// one directory: about two hundred thousand of them exist, which a large
/// library will fill often enough to matter. That is what the numbered files
/// are for — `ANLZ0000`, `ANLZ0001` — and why the path section inside each
/// file has to name the track it belongs to exactly.
pub fn analysis_dir(on_drive: &str) -> String {
    let mut hash: u32 = 0;
    for unit in on_drive.encode_utf16() {
        let unit = u32::from(unit);
        hash = hash.wrapping_mul(0x5BC9).wrapping_add(unit);
        hash = hash.wrapping_mul(0x93B5).wrapping_add(unit);
    }
    let hash = hash % 200_003;
    // The first component is seven bits of the second, taken from positions
    // nobody has explained.
    let bucket = (hash & 0x01)
        | ((hash >> 1) & 0x02)
        | ((hash >> 4) & 0x04)
        | ((hash >> 4) & 0x08)
        | ((hash >> 5) & 0x10)
        | ((hash >> 8) & 0x20)
        | ((hash >> 10) & 0x40);
    format!("/PIONEER/USBANLZ/P{bucket:03X}/{hash:08X}")
}

/// The three analysis files for one track, by their paths on the drive.
///
/// `number` is 0 unless something else already occupies the directory, which
/// happens when two audio paths hash the same.
pub fn analysis_paths(dir: &str, number: u32) -> [String; 3] {
    ["DAT", "EXT", "2EX"].map(|ext| format!("{dir}/ANLZ{number:04}.{ext}"))
}

/// The three bytes that follow a cue's type in both cue formats. They are not
/// padding: every file seen in the wild holds a big-endian 1000 there, and
/// nobody knows why.
const THOUSAND: [u8; 3] = [0x00, 0x03, 0xE8];

fn put_u16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_be_bytes());
}

fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_be_bytes());
}

fn put_pad(out: &mut Vec<u8>, n: usize) {
    out.extend(std::iter::repeat_n(0u8, n));
}

/// Start a section: its code, the length of its header, and a placeholder for
/// the total length, which [`finish`] fills in once the body is known.
fn start(fourcc: &[u8; 4], len_header: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(64);
    out.extend_from_slice(fourcc);
    put_u32(&mut out, len_header);
    put_u32(&mut out, 0); // len_tag, patched by finish()
    out
}

fn finish(mut section: Vec<u8>) -> Vec<u8> {
    let len = section.len() as u32;
    section[8..12].copy_from_slice(&len.to_be_bytes());
    section
}

/// UTF-16 big-endian with a trailing NUL, which is how every string in this
/// format is stored.
fn utf16_nul(s: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len() * 2 + 2);
    for unit in s.encode_utf16() {
        put_u16(&mut out, unit);
    }
    put_u16(&mut out, 0);
    out
}

/// `PPTH` — where the audio file sits, as a path on the drive.
///
/// This is the player's link back from the analysis to the audio, so it has to
/// be the path as the *player* will see it (`/Contents/…`), not the path on the
/// machine that wrote it.
pub fn path(on_drive: &str) -> Vec<u8> {
    let mut out = start(b"PPTH", 0x10);
    let text = utf16_nul(on_drive);
    put_u32(&mut out, text.len() as u32);
    out.extend_from_slice(&text);
    finish(out)
}

/// `PVBR` — the seek index for a variable-bitrate file.
///
/// A table of byte offsets, one per boundary of the 400 equal-time intervals a
/// track is divided into, so a player can jump to a moment in a VBR file
/// without decoding from the start. The four bytes after the section header,
/// then 401 big-endian offsets. See [`crate::audio::mp3`] for how the offsets
/// are found; a constant-bitrate file passes a stubbed table here, which is
/// what rekordbox writes.
pub fn vbr(offsets: &[u32; 401]) -> Vec<u8> {
    let mut out = start(b"PVBR", 0x10);
    put_u32(&mut out, 0); // unknown, always zero in a real export
    for &offset in offsets.iter() {
        put_u32(&mut out, offset);
    }
    finish(out)
}

/// `PQTZ` — the beat grid.
pub fn beat_grid(grid: &BeatGrid) -> Vec<u8> {
    let mut out = start(b"PQTZ", 0x18);
    put_u32(&mut out, 0);
    put_u32(&mut out, 0x0008_0000); // constant, purpose unknown
    put_u32(&mut out, grid.beats.len() as u32);
    for beat in &grid.beats {
        put_u16(&mut out, beat.number);
        put_u16(&mut out, beat.tempo_x100);
        put_u32(&mut out, beat.time_ms);
    }
    finish(out)
}

/// `PCOB` — the original cue list, as players before the nexus 2 read it.
///
/// It carries no colours and no comments; those arrived with [`cues_extended`].
/// Both are written, because an older player reads this one and ignores the
/// other.
pub fn cues(list: &[Cue], hot: bool) -> Vec<u8> {
    let chosen: Vec<&Cue> = list.iter().filter(|c| c.is_hot() == hot).collect();
    let mut out = start(b"PCOB", 0x18);
    put_u32(&mut out, if hot { 1 } else { 0 });
    put_pad(&mut out, 2);
    put_u16(&mut out, chosen.len() as u16);
    // `memory_count`, whose meaning nobody has worked out. A real rekordbox
    // export writes 0xffffffff here even for an empty list, which is what rules
    // out the obvious reading of it as a count.
    put_u32(&mut out, 0xffff_ffff);
    let last = chosen.len().saturating_sub(1);
    for (i, cue) in chosen.iter().enumerate() {
        let mut entry = Vec::with_capacity(56);
        entry.extend_from_slice(b"PCPT");
        put_u32(&mut entry, 0x1c);
        put_u32(&mut entry, 0x38);
        put_u32(&mut entry, cue.hot_cue as u32);
        // Status 1 is "enabled". The other documented value, 4, means a loop
        // that is *currently running*, which is a playback state rather than
        // something a saved loop should claim.
        put_u32(&mut entry, 1);
        put_u32(&mut entry, 0x0001_0000); // constant, purpose unknown
                                          // The two order fields chain the cues together; the ends of the chain
                                          // are marked with 0xffff.
        put_u16(&mut entry, if i == 0 { 0xffff } else { i as u16 });
        put_u16(&mut entry, if i == last { 0xffff } else { i as u16 + 1 });
        entry.push(match cue.kind {
            CueKind::Point => 1,
            CueKind::Loop { .. } => 2,
        });
        entry.extend_from_slice(&THOUSAND);
        put_u32(&mut entry, cue.time_ms);
        put_u32(
            &mut entry,
            match cue.kind {
                CueKind::Point => 0xffff_ffff,
                CueKind::Loop { end_ms } => end_ms,
            },
        );
        put_pad(&mut entry, 16);
        debug_assert_eq!(entry.len(), 0x38);
        out.extend_from_slice(&entry);
    }
    finish(out)
}

/// `PCO2` — the cue list as the nexus 2 line and everything after it reads it,
/// with hot cues D through H, per-cue colour, and a comment.
pub fn cues_extended(list: &[Cue], hot: bool) -> Vec<u8> {
    let chosen: Vec<&Cue> = list.iter().filter(|c| c.is_hot() == hot).collect();
    let mut out = start(b"PCO2", 0x14);
    put_u32(&mut out, if hot { 1 } else { 0 });
    put_u16(&mut out, chosen.len() as u16);
    put_pad(&mut out, 2);
    for cue in chosen {
        // Written even when there is no comment, as a bare NUL. rekordbox does
        // the same, and a reader that trusts the length rather than the content
        // rejects an entry that leaves it out.
        let comment = utf16_nul(cue.comment.as_deref().unwrap_or(""));
        let len_comment = comment.len() as u32;

        let mut entry = Vec::with_capacity(64);
        entry.extend_from_slice(b"PCP2");
        put_u32(&mut entry, 0x1c);
        // Always 68 bytes plus the comment. The colour fields are written even
        // when there is no colour, so every entry has the same shape, and the
        // twenty bytes on the end are what rekordbox itself leaves there — the
        // format documentation allows an entry to stop before them, but a
        // parser written from real files expects to find them.
        put_u32(&mut entry, 68 + len_comment);
        put_u32(&mut entry, cue.hot_cue as u32);
        entry.push(match cue.kind {
            CueKind::Point => 1,
            CueKind::Loop { .. } => 2,
        });
        entry.extend_from_slice(&THOUSAND);
        put_u32(&mut entry, cue.time_ms);
        put_u32(
            &mut entry,
            match cue.kind {
                CueKind::Point => 0xffff_ffff,
                CueKind::Loop { end_ms } => end_ms,
            },
        );
        entry.push(0); // colour table row, for memory cues that use one
        put_pad(&mut entry, 7);
        let (num, den) = cue.loop_beats.unwrap_or((0, 0));
        put_u16(&mut entry, num);
        put_u16(&mut entry, den);
        put_u32(&mut entry, len_comment);
        entry.extend_from_slice(&comment);
        let color = cue.color.unwrap_or(super::Rgb { r: 0, g: 0, b: 0 });
        entry.push(0); // colour code, a lookup rekordbox uses for its own palette
        entry.push(color.r);
        entry.push(color.g);
        entry.push(color.b);
        put_pad(&mut entry, 20);
        debug_assert_eq!(entry.len() as u32, 68 + len_comment);
        out.extend_from_slice(&entry);
    }
    finish(out)
}

/// `PWAV` — the 400-column monochrome preview above the touch strip.
pub fn wave_preview(data: &[u8]) -> Vec<u8> {
    fixed_preview(b"PWAV", data)
}

/// `PWV2` — the 100-column preview a CDJ-900 shows.
pub fn wave_tiny(data: &[u8]) -> Vec<u8> {
    fixed_preview(b"PWV2", data)
}

fn fixed_preview(fourcc: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut out = start(fourcc, 0x14);
    put_u32(&mut out, data.len() as u32);
    put_u32(&mut out, 0x0010_0000); // constant, purpose unknown
    out.extend_from_slice(data);
    finish(out)
}

/// `PWV3` — the scrolling monochrome waveform, one byte per half-frame.
pub fn wave_detail(data: &[u8]) -> Vec<u8> {
    entry_section(b"PWV3", 1, data, Some(0x0096_0000))
}

/// `PWV4` — the 1,200-column colour preview, six bytes per column.
pub fn wave_color_preview(data: &[u8]) -> Vec<u8> {
    entry_section(b"PWV4", 6, data, Some(0x0096_0000))
}

/// `PWV5` — the scrolling colour waveform, two bytes per half-frame.
pub fn wave_color_detail(data: &[u8]) -> Vec<u8> {
    entry_section(b"PWV5", 2, data, Some(0x0096_0305))
}

/// `PWV6` — the CDJ-3000's 1,200-column three-band preview.
///
/// Alone among the waveform sections this one has no unknown word before its
/// entries, so its header is four bytes shorter than the others.
pub fn wave_3band_preview(data: &[u8]) -> Vec<u8> {
    entry_section(b"PWV6", 3, data, None)
}

/// `PWV7` — the CDJ-3000's scrolling three-band waveform.
pub fn wave_3band_detail(data: &[u8]) -> Vec<u8> {
    entry_section(b"PWV7", 3, data, Some(0x0096_0000))
}

/// `PWVC` — the twenty bytes that close a real `.2EX`.
///
/// Three small numbers whose meaning nobody has published; a capture of a real
/// export read them as something like `[88, 81, 127]`, which is the shape of a
/// per-band average and is what this writes. It goes last, after both
/// waveforms, so that a player which does not understand it has already read
/// everything that matters.
pub fn wave_3band_summary(bands: &[u8]) -> Vec<u8> {
    let mut out = start(b"PWVC", 0x0e);
    put_u16(&mut out, 0);
    for band in 0..3 {
        let mut total = 0u64;
        let mut count = 0u64;
        for entry in bands.chunks_exact(3) {
            total += u64::from(entry[band]);
            count += 1;
        }
        put_u16(&mut out, (total.checked_div(count).unwrap_or(0)) as u16);
    }
    finish(out)
}

fn entry_section(fourcc: &[u8; 4], entry_bytes: u32, data: &[u8], unknown: Option<u32>) -> Vec<u8> {
    let len_header = if unknown.is_some() { 0x18 } else { 0x14 };
    let mut out = start(fourcc, len_header);
    put_u32(&mut out, entry_bytes);
    put_u32(&mut out, data.len() as u32 / entry_bytes);
    if let Some(v) = unknown {
        put_u32(&mut out, v);
    }
    out.extend_from_slice(data);
    finish(out)
}

/// The nineteen-byte pattern rekordbox 6 and later use to obfuscate `PSSI`.
/// Each byte has the phrase count added to it before it is applied.
pub(crate) const PSSI_MASK: [u8; 19] = [
    0xCB, 0xE1, 0xEE, 0xFA, 0xE5, 0xEE, 0xAD, 0xEE, 0xE9, 0xD2, 0xE9, 0xEB, 0xE1, 0xE9, 0xF3, 0xE8,
    0xE9, 0xF4, 0xE1,
];

/// The three flag bytes that spell out a high-mood phrase's numbered variant.
///
/// The number a player prints after "Chorus" or "Intro" is not in `kind`: it
/// is carried in three flags whose meaning depends on the kind, which is the
/// table below, published by Deep Symmetry and confirmed against a photograph
/// of a CDJ-1500X drawing a drive written here.
///
/// | kind | k1 | k2 | k3 | label |
/// | --- | --- | --- | --- | --- |
/// | 1 Intro | 1 | | | Intro 1 |
/// | 1 Intro | 0 | | | Intro 2 |
/// | 2 Up | | 0 | 0 | Up 1 |
/// | 2 Up | | 0 | 1 | Up 2 |
/// | 2 Up | | 1 | 0 | Up 3 |
/// | 3 Down | | | | Down |
/// | 5 Chorus | 1 | | | Chorus 1 |
/// | 5 Chorus | 0 | | | Chorus 2 |
/// | 6 Outro | 1 | | | Outro 1 |
/// | 6 Outro | 0 | | | Outro 2 |
///
/// Note what all-zero means: Intro **2**, Chorus **2**, Outro **2**. Writing
/// zeroes is not writing "no variant", it is writing the second one — which is
/// what a photograph of a real player showed, every chorus of the track
/// reading "CHORUS 2".
///
/// "Up 3" is never written. The format carries extra beat numbers inside an Up
/// 3 phrase, for lighting changes nobody here has worked out, and a phrase
/// that claims to be one without them is a lie a player may act on.
fn variant_flags(kind: u16, variant: u8) -> (u8, u8, u8) {
    match kind {
        // Intro, Chorus, Outro: one flag, and it reads the opposite way round
        // to the number printed.
        1 | 5 | 6 => (u8::from(variant <= 1), 0, 0),
        2 => match variant {
            0 | 1 => (0, 0, 0),
            _ => (0, 0, 1),
        },
        // Down has no variants, and anything else is a mood this does not
        // write, where these flags are not variant flags at all.
        _ => (0, 0, 0),
    }
}

/// `PSSI` — the phrase analysis the CDJ-3000 draws under its waveform.
///
/// Everything from the mood onwards is XOR-masked. The mask is not encryption
/// and is not treated as one here: it is a fixed nineteen-byte pattern offset
/// by the phrase count, documented in public, and it is applied because the
/// player expects to have to undo it.
pub fn song_structure(structure: &SongStructure) -> Vec<u8> {
    let count = structure.phrases.len() as u16;

    let mut body = Vec::with_capacity(20 + structure.phrases.len() * 24);
    put_u16(&mut body, structure.mood as u16);
    put_pad(&mut body, 6);
    put_u16(&mut body, structure.end_beat);
    put_pad(&mut body, 2);
    body.push(structure.bank);
    put_pad(&mut body, 1);
    for (i, phrase) in structure.phrases.iter().enumerate() {
        let (k1, k2, k3) = variant_flags(phrase.kind, phrase.variant);
        put_u16(&mut body, i as u16 + 1);
        put_u16(&mut body, phrase.beat);
        put_u16(&mut body, phrase.kind);
        put_pad(&mut body, 1);
        body.push(k1);
        put_pad(&mut body, 1);
        body.push(k2);
        put_pad(&mut body, 1);
        body.push(0); // b: extra beat numbers, only used by "Up 3" phrases
        put_u16(&mut body, 0); // beat2
        put_u16(&mut body, 0); // beat3
        put_u16(&mut body, 0); // beat4
        put_pad(&mut body, 1);
        body.push(k3);
        put_pad(&mut body, 1);
        body.push(0); // fill-in present
        put_u16(&mut body, 0); // beat at which the fill-in starts
    }

    let key = count as u8;
    for (i, byte) in body.iter_mut().enumerate() {
        *byte ^= PSSI_MASK[i % PSSI_MASK.len()].wrapping_add(key);
    }

    let mut out = start(b"PSSI", 0x20);
    put_u32(&mut out, 24); // bytes per phrase entry
    put_u16(&mut out, count);
    out.extend_from_slice(&body);
    finish(out)
}

/// Assemble sections into a complete analysis file.
pub fn file(sections: &[Vec<u8>]) -> Vec<u8> {
    let body_len: usize = sections.iter().map(|s| s.len()).sum();
    let mut out = Vec::with_capacity(FILE_HEADER_LEN as usize + body_len);
    out.extend_from_slice(b"PMAI");
    put_u32(&mut out, FILE_HEADER_LEN);
    put_u32(&mut out, FILE_HEADER_LEN + body_len as u32);
    let padding = FILE_HEADER_LEN as usize - out.len();
    put_pad(&mut out, padding);
    for section in sections {
        out.extend_from_slice(section);
    }
    out
}

/// Everything known about one track, ready to be written out.
pub struct Analysis<'a> {
    /// The path the *player* will use, e.g. `/Contents/Peverelist/track.flac`.
    pub on_drive_path: &'a str,
    pub grid: &'a BeatGrid,
    pub cues: &'a [Cue],
    pub waveforms: &'a WaveformData,
    pub structure: Option<&'a SongStructure>,
    /// The 401-entry seek index, for an MP3. `None` for a format that seeks
    /// without one, which is every format but MP3.
    pub vbr: Option<&'a [u32; 401]>,
}

impl Analysis<'_> {
    /// The `.DAT` file: what a player from 2009 onwards can read.
    pub fn dat(&self) -> Vec<u8> {
        // PVBR sits right after the path, as it does in a real export, and only
        // in the .DAT — the .EXT and .2EX do not carry it. A file with no
        // variable bitrate to index still gets the section, as a table of
        // zeroes: every real export has one, and a section a player expects and
        // does not find is not a risk worth taking for 1.6 kB a track.
        let mut sections = vec![path(self.on_drive_path)];
        sections.push(vbr(self.vbr.unwrap_or(&[0; 401])));
        sections.extend([
            beat_grid(self.grid),
            wave_preview(&self.waveforms.preview),
            wave_tiny(&self.waveforms.tiny),
            cues(self.cues, false),
            cues(self.cues, true),
        ]);
        file(&sections)
    }

    /// The `.EXT` file: colour waveforms, named and coloured cues, phrases.
    ///
    /// The order and the duplication follow a real rekordbox export rather than
    /// taste: the extended file repeats the old cue lists alongside the new
    /// ones, so a player that reads `.EXT` but predates the nexus 2 cue format
    /// still finds cues.
    pub fn ext(&self) -> Vec<u8> {
        // No beat grid here. rekordbox writes one — `PQT2`, a second, terser
        // encoding of the same beats — and this used to write a copy of the
        // `.DAT`'s `PQTZ` in its place, which is a section that does not belong
        // in this file. A CDJ-3000X handed one drew the monochrome preview from
        // the `.DAT` and none of the colour: everything after the wrong tag,
        // which is exactly the colour waveforms and the phrases, went unread.
        //
        // `PQT2`'s layout is not published beyond "two bytes a beat", so
        // nothing is written in its place rather than something invented. The
        // grid the player uses is the `.DAT`'s, which it already reads.
        let mut sections = vec![
            path(self.on_drive_path),
            wave_detail(&self.waveforms.detail),
            cues(self.cues, false),
            cues(self.cues, true),
            cues_extended(self.cues, false),
            cues_extended(self.cues, true),
            wave_color_detail(&self.waveforms.color_detail),
            wave_color_preview(&self.waveforms.color_preview),
        ];
        if let Some(structure) = self.structure {
            sections.push(song_structure(structure));
        }
        file(&sections)
    }

    /// The `.2EX` file: the CDJ-3000's three-band waveforms, and the summary
    /// that closes one.
    ///
    /// Preview before detail, which is the order a real export writes them in
    /// and the opposite of what this used to do.
    pub fn two_ex(&self) -> Vec<u8> {
        file(&[
            path(self.on_drive_path),
            wave_3band_preview(&self.waveforms.band_preview),
            wave_3band_detail(&self.waveforms.band_detail),
            wave_3band_summary(&self.waveforms.band_preview),
        ])
    }
}

/// What a reader found in one section of an analysis file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SectionInfo {
    pub fourcc: String,
    pub len_header: u32,
    pub len_tag: u32,
    /// What the section turned out to contain: a beat count, a cue count, a
    /// number of waveform columns, a path.
    pub summary: String,
}

/// Walk an analysis file and report what is in it.
///
/// This is deliberately written against the format documentation rather than
/// against the writers above: it decodes the bytes from scratch and shares no
/// code with them. That is the point. A drive is only verified if something
/// other than the thing that wrote it can read it back, and this is the first
/// piece of that verifier.
pub fn inspect(bytes: &[u8]) -> Result<Vec<SectionInfo>> {
    if bytes.len() < 12 || &bytes[0..4] != b"PMAI" {
        bail!("not an analysis file: missing the PMAI signature");
    }
    let be32 = |at: usize| -> u32 {
        u32::from_be_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
    };
    let len_header = be32(4) as usize;
    let len_file = be32(8) as usize;
    if len_file != bytes.len() {
        bail!("file header claims {len_file} bytes but the file is {}", bytes.len());
    }
    if len_header > bytes.len() {
        bail!("file header claims to be {len_header} bytes, longer than the file");
    }

    let mut found = Vec::new();
    let mut at = len_header;
    while at + 12 <= bytes.len() {
        let fourcc = String::from_utf8_lossy(&bytes[at..at + 4]).into_owned();
        let sec_header = be32(at + 4);
        let sec_len = be32(at + 8);
        if sec_len < 12 || at + sec_len as usize > bytes.len() {
            bail!("section {fourcc} at offset {at} claims a length of {sec_len} bytes");
        }
        let body = &bytes[at + 12..at + sec_len as usize];
        let summary = match fourcc.as_str() {
            "PQTZ" => format!("{} beats", u32::from_be_bytes(body[8..12].try_into()?)),
            "PPTH" => {
                let len = u32::from_be_bytes(body[0..4].try_into()?) as usize;
                let units: Vec<u16> = body[4..4 + len.saturating_sub(2)]
                    .chunks_exact(2)
                    .map(|c| u16::from_be_bytes([c[0], c[1]]))
                    .collect();
                String::from_utf16_lossy(&units)
            }
            "PCOB" => format!(
                "{} {} cues",
                u16::from_be_bytes(body[6..8].try_into()?),
                if body[3] == 1 { "hot" } else { "memory" }
            ),
            "PCO2" => format!(
                "{} {} cues",
                u16::from_be_bytes(body[4..6].try_into()?),
                if body[3] == 1 { "hot" } else { "memory" }
            ),
            "PWAV" | "PWV2" => {
                format!("{} columns", u32::from_be_bytes(body[0..4].try_into()?))
            }
            "PVBR" => {
                // Four unknown bytes, then the offsets. Report the last, which
                // is the file length and the one entry a CBR stub fills in.
                let last = body.len().saturating_sub(4);
                format!(
                    "seek index, ends at {}",
                    u32::from_be_bytes(body[last..last + 4].try_into()?)
                )
            }
            "PWV3" | "PWV4" | "PWV5" | "PWV6" | "PWV7" => format!(
                "{} entries of {} bytes",
                u32::from_be_bytes(body[4..8].try_into()?),
                u32::from_be_bytes(body[0..4].try_into()?)
            ),
            "PSSI" => format!("{} phrases", u16::from_be_bytes(body[4..6].try_into()?)),
            _ => format!("{} bytes", body.len()),
        };
        found.push(SectionInfo { fourcc, len_header: sec_header, len_tag: sec_len, summary });
        at += sec_len as usize;
    }
    if at != bytes.len() {
        bail!("{} trailing bytes after the last section", bytes.len() - at);
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::{Mood, Phrase};

    fn codes(sections: &[SectionInfo]) -> Vec<&str> {
        sections.iter().map(|s| s.fourcc.as_str()).collect()
    }

    #[test]
    fn a_section_declares_its_own_length() {
        let grid = BeatGrid::constant(120.0, 0, 4_000);
        let section = beat_grid(&grid);
        assert_eq!(&section[0..4], b"PQTZ");
        assert_eq!(u32::from_be_bytes(section[8..12].try_into().unwrap()), section.len() as u32);
        // Twelve bytes of section header, twelve of grid header, eight per beat.
        assert_eq!(section.len(), 24 + grid.beats.len() * 8);
    }

    #[test]
    fn beats_carry_bar_position_tempo_and_time() {
        let grid = BeatGrid::constant(120.0, 0, 1_000);
        let s = beat_grid(&grid);
        assert_eq!(u16::from_be_bytes(s[24..26].try_into().unwrap()), 1); // first beat of the bar
        assert_eq!(u16::from_be_bytes(s[26..28].try_into().unwrap()), 12_000); // 120.00 BPM
        assert_eq!(u32::from_be_bytes(s[28..32].try_into().unwrap()), 0);
        assert_eq!(u32::from_be_bytes(s[36..40].try_into().unwrap()), 500); // second beat
    }

    #[test]
    fn paths_are_utf16_with_a_trailing_nul() {
        let s = path("/Contents/a.flac");
        let len = u32::from_be_bytes(s[12..16].try_into().unwrap()) as usize;
        assert_eq!(len, ("/Contents/a.flac".len() + 1) * 2);
        assert_eq!(&s[s.len() - 2..], &[0, 0]);
    }

    #[test]
    fn a_non_ascii_path_survives_the_round_trip() {
        let name = "/Contents/Björk/Jóga.flac";
        let bytes = file(&[path(name)]);
        let sections = inspect(&bytes).unwrap();
        assert_eq!(sections[0].summary, name);
    }

    #[test]
    fn cues_are_split_into_hot_and_memory_lists() {
        let list = [Cue::memory(1_000), Cue::hot(1, 2_000), Cue::hot(2, 3_000)];
        let memory = inspect(&file(&[cues(&list, false)])).unwrap();
        let hot = inspect(&file(&[cues(&list, true)])).unwrap();
        assert_eq!(memory[0].summary, "1 memory cues");
        assert_eq!(hot[0].summary, "2 hot cues");
    }

    #[test]
    fn every_extended_cue_entry_is_68_bytes_plus_its_comment() {
        let list = [Cue::hot(1, 1_000).with_comment("first drop").with_color(226, 160, 63)];
        let s = cues_extended(&list, true);
        // 20 bytes of section header, then the entry.
        let len_entry = u32::from_be_bytes(s[28..32].try_into().unwrap());
        assert_eq!(len_entry as usize, 68 + ("first drop".len() + 1) * 2);
        assert_eq!(cues_extended(&[Cue::hot(1, 0)], true).len(), 20 + 68 + 2);
        assert_eq!(s.len(), 20 + len_entry as usize);
        // The colour sits just before the twenty trailing bytes.
        assert_eq!(&s[s.len() - 23..s.len() - 20], &[226, 160, 63]);
    }

    #[test]
    fn a_loop_records_where_it_returns_to() {
        let list = [Cue::hot(1, 1_000).looping(3_000)];
        let s = cues_extended(&list, true);
        assert_eq!(u32::from_be_bytes(s[40..44].try_into().unwrap()), 1_000);
        assert_eq!(u32::from_be_bytes(s[44..48].try_into().unwrap()), 3_000);
    }

    #[test]
    fn a_point_cue_has_no_loop_end() {
        let s = cues_extended(&[Cue::hot(1, 1_000)], true);
        assert_eq!(u32::from_be_bytes(s[44..48].try_into().unwrap()), 0xffff_ffff);
    }

    #[test]
    fn a_phrases_numbered_variant_reaches_the_flag_bytes_it_lives_in() {
        // The number a player prints after "Chorus" is not in `kind`: it is in
        // k1, k2 and k3. Writing them zeroed is not writing "no variant", it is
        // writing the *second* one — which is what a photograph of a CDJ-1500X
        // showed, every chorus of a real track reading "CHORUS 2".
        assert_eq!(variant_flags(1, 1), (1, 0, 0), "Intro 1");
        assert_eq!(variant_flags(1, 2), (0, 0, 0), "Intro 2");
        assert_eq!(variant_flags(5, 1), (1, 0, 0), "Chorus 1");
        assert_eq!(variant_flags(5, 2), (0, 0, 0), "Chorus 2");
        assert_eq!(variant_flags(6, 1), (1, 0, 0), "Outro 1");
        assert_eq!(variant_flags(6, 2), (0, 0, 0), "Outro 2");
        assert_eq!(variant_flags(2, 1), (0, 0, 0), "Up 1");
        assert_eq!(variant_flags(2, 2), (0, 0, 1), "Up 2");
        // Down has none, and nothing asks for Up 3: the format carries extra
        // beat numbers inside one, and this writes none of them.
        assert_eq!(variant_flags(3, 1), (0, 0, 0), "Down");
        assert_eq!(variant_flags(2, 3), (0, 0, 1), "never Up 3");
    }

    #[test]
    fn song_structure_is_masked_and_unmasks_to_what_went_in() {
        let structure = SongStructure {
            mood: Mood::Mid,
            end_beat: 512,
            bank: 0,
            phrases: vec![
                Phrase { beat: 1, kind: 1, ..Phrase::default() },
                Phrase { beat: 65, kind: 9, ..Phrase::default() },
            ],
        };
        let s = song_structure(&structure);
        assert_eq!(u16::from_be_bytes(s[16..18].try_into().unwrap()), 2);

        // The mood is not readable until the mask comes off.
        let masked_mood = u16::from_be_bytes(s[18..20].try_into().unwrap());
        assert_ne!(masked_mood, Mood::Mid as u16);
        assert!(masked_mood > 20, "an unmasked file is recognised by a mood under 20");

        let key = 2u8;
        let unmasked: Vec<u8> = s[18..]
            .iter()
            .enumerate()
            .map(|(i, b)| b ^ PSSI_MASK[i % PSSI_MASK.len()].wrapping_add(key))
            .collect();
        assert_eq!(u16::from_be_bytes(unmasked[0..2].try_into().unwrap()), Mood::Mid as u16);
        assert_eq!(u16::from_be_bytes(unmasked[8..10].try_into().unwrap()), 512);
        // The entries start fourteen bytes into the masked body, after the
        // mood, the end beat and the lighting bank.
        // First phrase: index 1, beat 1, kind 1 (Intro).
        assert_eq!(u16::from_be_bytes(unmasked[14..16].try_into().unwrap()), 1);
        assert_eq!(u16::from_be_bytes(unmasked[16..18].try_into().unwrap()), 1);
        assert_eq!(u16::from_be_bytes(unmasked[18..20].try_into().unwrap()), 1);
        // Second phrase, one 24-byte entry later, starts at beat 65 and is a
        // chorus.
        assert_eq!(u16::from_be_bytes(unmasked[38..40].try_into().unwrap()), 2);
        assert_eq!(u16::from_be_bytes(unmasked[40..42].try_into().unwrap()), 65);
        assert_eq!(u16::from_be_bytes(unmasked[42..44].try_into().unwrap()), 9);
    }

    #[test]
    fn phrase_entries_are_24_bytes_each() {
        let one = song_structure(&SongStructure {
            mood: Mood::Mid,
            end_beat: 4,
            bank: 0,
            phrases: vec![Phrase { beat: 1, kind: 1, ..Phrase::default() }],
        });
        let two = song_structure(&SongStructure {
            mood: Mood::Mid,
            end_beat: 4,
            bank: 0,
            phrases: vec![
                Phrase { beat: 1, kind: 1, ..Phrase::default() },
                Phrase { beat: 5, kind: 2, ..Phrase::default() },
            ],
        });
        assert_eq!(two.len() - one.len(), 24);
        assert_eq!(one.len(), 0x20 + 24);
    }

    #[test]
    fn a_file_declares_its_own_length() {
        let bytes =
            file(&[path("/Contents/a.flac"), beat_grid(&BeatGrid::constant(128.0, 0, 2_000))]);
        assert_eq!(&bytes[0..4], b"PMAI");
        assert_eq!(u32::from_be_bytes(bytes[4..8].try_into().unwrap()), FILE_HEADER_LEN);
        assert_eq!(u32::from_be_bytes(bytes[8..12].try_into().unwrap()), bytes.len() as u32);
    }

    #[test]
    fn inspect_rejects_a_truncated_file() {
        let bytes = file(&[path("/Contents/a.flac")]);
        let err = inspect(&bytes[..bytes.len() - 4]).unwrap_err().to_string();
        assert!(err.contains("bytes"), "unhelpful error: {err}");
    }

    #[test]
    fn inspect_rejects_something_that_is_not_an_analysis_file() {
        assert!(inspect(b"this is an mp3, actually").is_err());
    }

    #[test]
    fn the_three_files_carry_the_sections_their_players_expect() {
        let grid = BeatGrid::constant(128.0, 0, 3_000);
        let waveforms = WaveformData::silent(3.0);
        let cue_list = [Cue::memory(0), Cue::hot(1, 1_000)];
        let structure = SongStructure {
            mood: Mood::Mid,
            end_beat: 6,
            bank: 0,
            phrases: vec![Phrase { beat: 1, kind: 1, ..Phrase::default() }],
        };
        let analysis = Analysis {
            on_drive_path: "/Contents/a.flac",
            grid: &grid,
            cues: &cue_list,
            waveforms: &waveforms,
            structure: Some(&structure),
            vbr: None,
        };

        // Measured across some seven hundred tracks of two real rekordbox
        // exports, and worth holding to exactly: a section in the wrong file,
        // or in the wrong place in the right file, is not something a player
        // reports. It draws what it managed to read and says nothing about the
        // rest.
        assert_eq!(
            codes(&inspect(&analysis.dat()).unwrap()),
            ["PPTH", "PVBR", "PQTZ", "PWAV", "PWV2", "PCOB", "PCOB"]
        );
        assert_eq!(
            codes(&inspect(&analysis.ext()).unwrap()),
            ["PPTH", "PWV3", "PCOB", "PCOB", "PCO2", "PCO2", "PWV5", "PWV4", "PSSI"],
            "rekordbox has a PQT2 between the cues and the colour waveforms; \
             nothing goes there until its layout is known, and never a PQTZ"
        );
        assert_eq!(codes(&inspect(&analysis.two_ex()).unwrap()), ["PPTH", "PWV6", "PWV7", "PWVC"]);
    }

    #[test]
    fn a_seek_index_lands_right_after_the_path_in_the_dat() {
        let grid = BeatGrid::constant(128.0, 0, 3_000);
        let waveforms = WaveformData::silent(3.0);
        let offsets = [0u32; 401];
        let analysis = Analysis {
            on_drive_path: "/Contents/a.mp3",
            grid: &grid,
            cues: &[],
            waveforms: &waveforms,
            structure: None,
            vbr: Some(&offsets),
        };
        let dat = analysis.dat();
        assert_eq!(
            codes(&inspect(&dat).unwrap()),
            ["PPTH", "PVBR", "PQTZ", "PWAV", "PWV2", "PCOB", "PCOB"]
        );
        // And it is only in the .DAT, never the .EXT.
        let ext = analysis.ext();
        assert!(!codes(&inspect(&ext).unwrap()).contains(&"PVBR"));
    }

    #[test]
    fn a_track_with_no_phrase_analysis_simply_has_no_pssi() {
        let grid = BeatGrid::constant(128.0, 0, 3_000);
        let waveforms = WaveformData::silent(3.0);
        let analysis = Analysis {
            on_drive_path: "/Contents/a.flac",
            grid: &grid,
            cues: &[],
            waveforms: &waveforms,
            structure: None,
            vbr: None,
        };
        assert!(!codes(&inspect(&analysis.ext()).unwrap()).contains(&"PSSI"));
    }

    #[test]
    fn the_analysis_directory_is_the_one_rekordbox_would_have_used() {
        // Worked examples published from a disassembly of rekordbox's own
        // path-naming, checked against the directory names on real drives. If
        // this drifts, a player stops finding anything a drive was prepared
        // with — silently, because it has no reason to look anywhere else.
        assert_eq!(
            analysis_dir("/Contents/Leo Portela/Bon Vibrant - Leo Portela.flac"),
            "/PIONEER/USBANLZ/P00E/000281CE"
        );
        assert_eq!(
            analysis_dir("/Contents/Daniela Cast/Jazzy - Daniela Cast.flac"),
            "/PIONEER/USBANLZ/P00A/0000CC9C"
        );
    }

    #[test]
    fn every_directory_name_is_one_a_player_could_have_computed() {
        // The first component is seven bits, so it never runs past P07F, and
        // the second is a hash modulo a prime. A name outside that range is a
        // name rekordbox would never write, which is the cheap version of the
        // check above for paths nobody has published an answer for.
        for name in ["/Contents/a.flac", "/Contents/Someone/A Long Title Goes Here.mp3", "/x"] {
            let dir = analysis_dir(name);
            let (bucket, hash) =
                dir.trim_start_matches("/PIONEER/USBANLZ/P").split_once('/').unwrap();
            assert!(u32::from_str_radix(bucket, 16).unwrap() <= 0x7F, "{dir}");
            assert!(u32::from_str_radix(hash, 16).unwrap() < 200_003, "{dir}");
            assert_eq!(bucket.len(), 3);
            assert_eq!(hash.len(), 8);
        }
    }

    #[test]
    fn the_same_track_always_lands_in_the_same_place() {
        // Which is what makes a second sync able to leave a track alone.
        assert_eq!(analysis_dir("/Contents/a.flac"), analysis_dir("/Contents/a.flac"));
        assert_ne!(analysis_dir("/Contents/a.flac"), analysis_dir("/Contents/b.flac"));
    }

    #[test]
    fn a_directory_holds_more_than_one_track_by_numbering_the_files() {
        let dir = analysis_dir("/Contents/a.flac");
        assert_eq!(
            analysis_paths(&dir, 0),
            [
                format!("{dir}/ANLZ0000.DAT"),
                format!("{dir}/ANLZ0000.EXT"),
                format!("{dir}/ANLZ0000.2EX")
            ]
        );
        assert_eq!(analysis_paths(&dir, 1)[0], format!("{dir}/ANLZ0001.DAT"));
        assert_eq!(analysis_paths(&dir, 12)[0], format!("{dir}/ANLZ0012.DAT"));
    }
}
