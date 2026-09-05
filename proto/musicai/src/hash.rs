//! What a file is, and what is in it: two hashes with different questions.
//!
//! [`file_sha256`] is the file as it sits on disk. Two of them agreeing means
//! the bytes are the same, which is the only case where deleting one is
//! obviously safe.
//!
//! [`audio_sha256`] is the encoded audio inside the container, with the tag
//! blocks skipped. It answers the question the file hash cannot: the same rip,
//! tagged twice — one from a shop, one after a fingerprint lookup wrote the
//! artist in — is the same recording taking up twice the disk, and its file
//! hashes disagree about that because they include the tags.
//!
//! Neither decodes. This reads the container's own structure to find where the
//! audio starts and stops, which is the difference between a question that
//! costs a moment and one that costs minutes a track.

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};

/// Read in blocks rather than whole: a library has files of a hundred
/// megabytes in it, and there is no reason for any of them to be in memory.
const BLOCK: usize = 64 * 1024;

/// The SHA-256 of the file exactly as it is on disk.
pub fn file_sha256(path: &Path) -> Result<String> {
    let file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut reader = BufReader::new(file);
    let mut hasher = Sha256::new();
    let mut block = vec![0u8; BLOCK];
    loop {
        let read =
            reader.read(&mut block).with_context(|| format!("reading {}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&block[..read]);
    }
    Ok(hex(hasher.finalize().as_slice()))
}

/// The SHA-256 of the encoded audio, with the container's metadata skipped.
///
/// Unchanged by writing a tag, so two copies of one rip that differ only in
/// what somebody typed into them still agree. A format whose layout this does
/// not know falls back to the whole file, which is the safe direction to be
/// wrong in: it can then only fail to notice a duplicate, never claim one that
/// is not there.
pub fn audio_sha256(path: &Path) -> Result<String> {
    let mut file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let spans = audio_spans(&mut file, path)?;

    let mut hasher = Sha256::new();
    let mut block = vec![0u8; BLOCK];
    for (start, length) in spans {
        file.seek(SeekFrom::Start(start))?;
        let mut left = length;
        while left > 0 {
            let want = left.min(BLOCK as u64) as usize;
            let read = file.read(&mut block[..want])?;
            if read == 0 {
                break;
            }
            hasher.update(&block[..read]);
            left -= read as u64;
        }
    }
    Ok(hex(hasher.finalize().as_slice()))
}

/// Where the audio lives, as `(offset, length)` pairs.
fn audio_spans(file: &mut File, path: &Path) -> Result<Vec<(u64, u64)>> {
    let size = file.metadata()?.len();
    let extension =
        path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();

    let spans = match extension.as_str() {
        "flac" => flac_spans(file, size)?,
        "mp3" => mp3_spans(file, size)?,
        "m4a" | "m4b" | "mp4" | "aac" => mp4_spans(file, size)?,
        // AIFF is chunked like WAV — `FORM`/`SSND` where WAV has
        // `RIFF`/`data` — and carries its tags in a chunk beside the samples.
        "wav" | "wave" | "aiff" | "aif" => riff_spans(file, size)?,
        _ => None,
    };
    // The whole file when the layout is not known, so an unrecognised format
    // still gets an answer — just a stricter one.
    Ok(spans.unwrap_or_else(|| vec![(0, size)]))
}

/// FLAC: `fLaC`, then metadata blocks, then frames to the end.
///
/// Tags live in those blocks, so everything after them is the audio and none
/// of it moves when a tag is rewritten.
fn flac_spans(file: &mut File, size: u64) -> Result<Option<Vec<(u64, u64)>>> {
    file.seek(SeekFrom::Start(0))?;
    let mut magic = [0u8; 4];
    if file.read_exact(&mut magic).is_err() || &magic != b"fLaC" {
        return Ok(None);
    }
    let mut at = 4u64;
    loop {
        let mut header = [0u8; 4];
        file.seek(SeekFrom::Start(at))?;
        if file.read_exact(&mut header).is_err() {
            return Ok(None);
        }
        let last = header[0] & 0x80 != 0;
        let length = u32::from_be_bytes([0, header[1], header[2], header[3]]) as u64;
        at += 4 + length;
        if at > size {
            return Ok(None);
        }
        if last {
            break;
        }
    }
    Ok(Some(vec![(at, size - at)]))
}

/// MP3: an ID3v2 block at the front, an ID3v1 or APE tag at the back, frames
/// in between.
fn mp3_spans(file: &mut File, size: u64) -> Result<Option<Vec<(u64, u64)>>> {
    let start = id3v2_length(file)?;
    let mut end = size;

    // ID3v1 is a fixed 128 bytes at the very end, starting `TAG`.
    if end >= 128 {
        file.seek(SeekFrom::Start(end - 128))?;
        let mut marker = [0u8; 3];
        if file.read_exact(&mut marker).is_ok() && &marker == b"TAG" {
            end -= 128;
        }
    }
    // APEv2 keeps its length in a footer that ends with `APETAGEX`.
    if end >= 32 {
        file.seek(SeekFrom::Start(end - 32))?;
        let mut footer = [0u8; 32];
        if file.read_exact(&mut footer).is_ok() && &footer[..8] == b"APETAGEX" {
            let length = u32::from_le_bytes([footer[12], footer[13], footer[14], footer[15]]);
            end = end.saturating_sub(length as u64);
        }
    }
    match end > start {
        true => Ok(Some(vec![(start, end - start)])),
        false => Ok(None),
    }
}

/// How long the ID3v2 block at the front is, or 0 when there is not one.
///
/// The length is stored as four seven-bit bytes, so that a length can never
/// contain the frame sync a decoder is scanning for.
fn id3v2_length(file: &mut File) -> Result<u64> {
    file.seek(SeekFrom::Start(0))?;
    let mut header = [0u8; 10];
    if file.read_exact(&mut header).is_err() || &header[..3] != b"ID3" {
        return Ok(0);
    }
    let synchsafe =
        header[6..10].iter().fold(0u64, |total, byte| (total << 7) | (*byte & 0x7f) as u64);
    // The ten-byte header itself, and a footer when the flag says so.
    let footer = if header[5] & 0x10 != 0 { 10 } else { 0 };
    Ok(10 + synchsafe + footer)
}

/// MP4: the `mdat` boxes hold the samples; `moov` holds the tags.
fn mp4_spans(file: &mut File, size: u64) -> Result<Option<Vec<(u64, u64)>>> {
    let mut spans = Vec::new();
    let mut at = 0u64;
    while at + 8 <= size {
        file.seek(SeekFrom::Start(at))?;
        let mut header = [0u8; 8];
        if file.read_exact(&mut header).is_err() {
            break;
        }
        let short = u32::from_be_bytes([header[0], header[1], header[2], header[3]]) as u64;
        let kind = &header[4..8];
        // 1 means the real length is in the next eight bytes; 0 means the box
        // runs to the end of the file.
        let (length, body) = match short {
            1 => {
                let mut extended = [0u8; 8];
                if file.read_exact(&mut extended).is_err() {
                    break;
                }
                (u64::from_be_bytes(extended), 16)
            }
            0 => (size - at, 8),
            n => (n, 8),
        };
        if length < body || at + length > size {
            break;
        }
        if kind == b"mdat" {
            spans.push((at + body, length - body));
        }
        at += length;
    }
    match spans.is_empty() {
        true => Ok(None),
        false => Ok(Some(spans)),
    }
}

/// WAV and AIFF: chunks, of which one holds the samples.
fn riff_spans(file: &mut File, size: u64) -> Result<Option<Vec<(u64, u64)>>> {
    file.seek(SeekFrom::Start(0))?;
    let mut magic = [0u8; 12];
    if file.read_exact(&mut magic).is_err() {
        return Ok(None);
    }
    let little = match &magic[..4] {
        b"RIFF" => true,
        b"FORM" => false,
        _ => return Ok(None),
    };
    let wanted: &[u8] = if little { b"data" } else { b"SSND" };

    let mut at = 12u64;
    while at + 8 <= size {
        file.seek(SeekFrom::Start(at))?;
        let mut header = [0u8; 8];
        if file.read_exact(&mut header).is_err() {
            break;
        }
        let raw = [header[4], header[5], header[6], header[7]];
        let length = if little { u32::from_le_bytes(raw) } else { u32::from_be_bytes(raw) } as u64;
        if &header[..4] == wanted {
            let length = length.min(size - at - 8);
            return Ok(Some(vec![(at + 8, length)]));
        }
        // Chunks are padded to an even length.
        at += 8 + length + (length % 2);
    }
    Ok(None)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
