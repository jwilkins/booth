//! Reading the frame structure of an MP3, to build a seek index.
//!
//! A variable-bitrate MP3 has no fixed relationship between a moment in the
//! music and a byte in the file: each frame holds the same amount of audio but
//! takes a different number of bytes. A player that wants to jump to a hot cue
//! has to know where in the file that moment lives, and it cannot work it out
//! by arithmetic the way it can for a constant-bitrate file. So rekordbox
//! writes a table of byte offsets — the `PVBR` section — and this builds it.
//!
//! Nothing here decodes audio. It walks the frame headers, which is enough to
//! know each frame's size and how much time it carries, and that is all a seek
//! table needs.

use std::path::Path;

use anyhow::{Context, Result};

/// The number of intervals a track is divided into. rekordbox uses 400, so the
/// table has 401 offsets: one at the start of each interval and one at the end.
const INTERVALS: usize = 400;

/// A seek index: where in the file each of 401 evenly-spaced moments lives.
pub struct SeekIndex {
    /// Byte offsets from the start of the file, one per boundary between the
    /// 400 equal-time intervals, plus the two ends. Monotonic non-decreasing.
    pub offsets: [u32; INTERVALS + 1],
    /// Whether the file actually varies its bitrate. A constant-bitrate file
    /// seeks fine by arithmetic and rekordbox leaves its table empty; this
    /// records which case produced the offsets.
    pub variable: bool,
}

impl SeekIndex {
    /// Build the index for an MP3 on disk. `None` for a file with no decodable
    /// frames — an empty file, or one that is not really an MP3.
    pub fn of_mp3(path: &Path) -> Result<Option<SeekIndex>> {
        let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        Ok(build(&bytes))
    }

    /// The 401 offsets as rekordbox writes them into `PVBR`.
    ///
    /// For a variable-bitrate file this is the real per-time byte map. For a
    /// constant-bitrate one rekordbox writes a stub — every offset zero except
    /// the last, which is the file length — because the player can seek a CBR
    /// file by arithmetic and does not need the table. Matching that exactly is
    /// the safe choice against a player that reads "all zero" as "this is CBR".
    pub fn pvbr_offsets(&self) -> [u32; INTERVALS + 1] {
        if self.variable {
            self.offsets
        } else {
            let mut stub = [0u32; INTERVALS + 1];
            stub[INTERVALS] = self.offsets[INTERVALS];
            stub
        }
    }
}

/// Samples of audio in one frame, which depends on the MPEG version and layer.
/// Layer 3 is by far the common case; the others are here so an odd file does
/// not produce a wrong duration.
fn samples_per_frame(version: Version, layer: Layer) -> u32 {
    match (version, layer) {
        (_, Layer::One) => 384,
        (Version::One, _) => 1152,
        // MPEG 2 and 2.5 halve the Layer 2/3 frame.
        (_, Layer::Two) => 1152,
        (_, Layer::Three) => 576,
    }
}

#[derive(Copy, Clone, PartialEq, Eq)]
enum Version {
    One,
    Two,
    TwoFive,
}

#[derive(Copy, Clone, PartialEq, Eq)]
enum Layer {
    One,
    Two,
    Three,
}

/// The bitrate tables, in kbps, indexed by the four-bit field in the frame
/// header. Index 0 is "free" and index 15 is invalid; both read as `None`.
/// MPEG 1 and MPEG 2/2.5 have different tables, and each version has one per
/// layer.
const BITRATE_V1_L3: [u32; 16] =
    [0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 0];
const BITRATE_V1_L2: [u32; 16] =
    [0, 32, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 384, 0];
const BITRATE_V1_L1: [u32; 16] =
    [0, 32, 64, 96, 128, 160, 192, 224, 256, 288, 320, 352, 384, 416, 448, 0];
const BITRATE_V2_L1: [u32; 16] =
    [0, 32, 48, 56, 64, 80, 96, 112, 128, 144, 160, 176, 192, 224, 256, 0];
const BITRATE_V2_L23: [u32; 16] = [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160, 0];

const SAMPLE_RATE_V1: [u32; 3] = [44_100, 48_000, 32_000];
const SAMPLE_RATE_V2: [u32; 3] = [22_050, 24_000, 16_000];
const SAMPLE_RATE_V25: [u32; 3] = [11_025, 12_000, 8_000];

/// One parsed frame header: how big it is and how much audio it carries.
struct Frame {
    bytes: u32,
    samples: u32,
    bitrate_kbps: u32,
}

/// Parse the four header bytes at `at`, if they are a valid frame.
fn parse_frame(data: &[u8], at: usize) -> Option<Frame> {
    if at + 4 > data.len() {
        return None;
    }
    let h = u32::from_be_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]]);

    // Eleven sync bits set.
    if h & 0xffe0_0000 != 0xffe0_0000 {
        return None;
    }
    let version = match (h >> 19) & 0b11 {
        0b00 => Version::TwoFive,
        0b10 => Version::Two,
        0b11 => Version::One,
        _ => return None, // 0b01 is reserved
    };
    let layer = match (h >> 17) & 0b11 {
        0b01 => Layer::Three,
        0b10 => Layer::Two,
        0b11 => Layer::One,
        _ => return None, // 0b00 is reserved
    };
    let bitrate_index = ((h >> 12) & 0b1111) as usize;
    let sample_index = ((h >> 10) & 0b11) as usize;
    let padding = (h >> 9) & 0b1;
    if sample_index == 3 {
        return None; // reserved sample-rate index
    }

    let bitrate_table = match (version, layer) {
        (Version::One, Layer::One) => BITRATE_V1_L1,
        (Version::One, Layer::Two) => BITRATE_V1_L2,
        (Version::One, Layer::Three) => BITRATE_V1_L3,
        (_, Layer::One) => BITRATE_V2_L1,
        (_, _) => BITRATE_V2_L23,
    };
    let bitrate_kbps = bitrate_table[bitrate_index];
    if bitrate_kbps == 0 {
        return None; // free or invalid bitrate
    }
    let sample_rate = match version {
        Version::One => SAMPLE_RATE_V1,
        Version::Two => SAMPLE_RATE_V2,
        Version::TwoFive => SAMPLE_RATE_V25,
    }[sample_index];

    let samples = samples_per_frame(version, layer);
    // The frame size formula, in bytes. Layer 1 counts in four-byte slots; the
    // others in bytes. The division truncates, which is what the format
    // intends — the padding bit makes up the remainder.
    let bitrate = bitrate_kbps * 1000;
    let bytes = match layer {
        Layer::One => (12 * bitrate / sample_rate + padding) * 4,
        _ => samples / 8 * bitrate / sample_rate + padding,
    };
    if bytes < 4 {
        return None;
    }
    Some(Frame { bytes, samples, bitrate_kbps })
}

/// Where the audio frames begin: past an ID3v2 tag if there is one.
fn audio_start(data: &[u8]) -> usize {
    if data.len() >= 10 && &data[0..3] == b"ID3" {
        // The size is four sync-safe bytes: seven bits each, high bit clear.
        let size = ((data[6] as usize) << 21)
            | ((data[7] as usize) << 14)
            | ((data[8] as usize) << 7)
            | (data[9] as usize);
        (10 + size).min(data.len())
    } else {
        0
    }
}

/// Walk the frames and turn them into a seek index.
fn build(data: &[u8]) -> Option<SeekIndex> {
    let mut at = audio_start(data);

    // Find the first real frame, tolerating a little junk before it.
    let mut first = None;
    let scan_end = (at + 8192).min(data.len());
    while at < scan_end {
        if let Some(frame) = parse_frame(data, at) {
            first = Some(frame);
            break;
        }
        at += 1;
    }
    let first = first?;

    // Skip a Xing/Info/VBRI header frame: it is a frame-shaped block of
    // metadata, not audio, and counting it would shift every offset. It sits at
    // a known distance into the first frame.
    let xing = has_side_info_tag(data, at, &first);

    // A running map of (byte offset, cumulative samples) at each frame
    // boundary. Bitrate is watched to decide whether the file is really VBR.
    let mut offsets = Vec::new();
    let mut samples = Vec::new();
    let mut cumulative_samples: u64 = 0;
    let mut variable = false;
    let base_bitrate = first.bitrate_kbps;

    if xing {
        // The header frame is skipped, so the first audio frame is the next one.
        at += first.bytes as usize;
    }

    while at + 4 <= data.len() {
        let Some(frame) = parse_frame(data, at) else {
            // A run of frames can end at a trailing ID3v1 tag or padding; stop
            // rather than hunting, since a real stream is contiguous.
            break;
        };
        if frame.bitrate_kbps != base_bitrate {
            variable = true;
        }
        offsets.push(at as u64);
        samples.push(cumulative_samples);
        cumulative_samples += frame.samples as u64;
        at += frame.bytes as usize;
    }

    if offsets.is_empty() || cumulative_samples == 0 {
        return None;
    }
    // The end of the last frame is the end of the audio.
    offsets.push(at as u64);
    samples.push(cumulative_samples);

    Some(SeekIndex {
        offsets: sample_offsets(&offsets, &samples, cumulative_samples, data.len()),
        variable,
    })
}

/// Whether the first frame carries a Xing, Info or VBRI header rather than
/// audio. The Xing/Info tag sits after the side information, whose size depends
/// on the version and channel mode; VBRI sits at a fixed offset.
fn has_side_info_tag(data: &[u8], frame_at: usize, first: &Frame) -> bool {
    let _ = first;
    // VBRI is always 32 bytes past the frame header.
    if frame_at + 36 <= data.len() && &data[frame_at + 32..frame_at + 36] == b"VBRI" {
        return true;
    }
    // Xing/Info can sit at one of a few offsets depending on the side-info
    // size; check the ones that occur, which covers mono and stereo for both
    // MPEG versions.
    for offset in [21, 36, 13, 21] {
        let start = frame_at + 4 + offset;
        if start + 4 <= data.len()
            && (&data[start..start + 4] == b"Xing" || &data[start..start + 4] == b"Info")
        {
            return true;
        }
    }
    false
}

/// Turn the per-frame map into 401 offsets at evenly spaced times.
fn sample_offsets(
    offsets: &[u64],
    samples: &[u64],
    total_samples: u64,
    file_len: usize,
) -> [u32; INTERVALS + 1] {
    let mut out = [0u32; INTERVALS + 1];
    for (i, slot) in out.iter_mut().enumerate() {
        let target = total_samples * i as u64 / INTERVALS as u64;
        // The offset of the last frame that starts at or before `target`
        // samples. A binary search over the cumulative-sample map.
        let frame = match samples.binary_search(&target) {
            Ok(exact) => exact,
            Err(next) => next.saturating_sub(1),
        };
        let byte = offsets.get(frame).copied().unwrap_or(file_len as u64);
        *slot = byte.min(u32::MAX as u64) as u32;
    }
    // The final entry is the end of the file, which is where a seek to the very
    // end lands. A real rekordbox export puts the file length here.
    out[INTERVALS] = file_len.min(u32::MAX as usize) as u32;
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a synthetic MPEG-1 Layer 3 stream. Each entry in `bitrates` is one
    /// frame's bitrate in kbps; a repeated value is a CBR file, varied values a
    /// VBR one. Returns the bytes and the true byte offset of each frame.
    fn stream(bitrates: &[u32]) -> (Vec<u8>, Vec<usize>) {
        let sample_rate = 44_100u32;
        let mut data = Vec::new();
        let mut starts = Vec::new();
        for &kbps in bitrates {
            starts.push(data.len());
            let index = BITRATE_V1_L3.iter().position(|&b| b == kbps).unwrap() as u32;
            let bytes = 1152 / 8 * (kbps * 1000) / sample_rate; // no padding
                                                                // 0xFFFB = sync + MPEG1 + Layer3 + no CRC. Then bitrate index and a
                                                                // 44.1 kHz sample-rate index (00).
            let header: u32 = 0xfffb_0000 | (index << 12);
            data.extend_from_slice(&header.to_be_bytes());
            data.resize(data.len() + bytes as usize - 4, 0);
        }
        starts.push(data.len());
        (data, starts)
    }

    #[test]
    fn a_constant_bitrate_file_is_recognised_as_such() {
        let (data, _) = stream(&[320; 100]);
        let index = build(&data).expect("no frames found");
        assert!(!index.variable, "constant bitrate misread as variable");
    }

    #[test]
    fn a_varying_bitrate_file_is_recognised_as_variable() {
        let (data, _) = stream(&[128, 320, 96, 256, 192, 320, 128, 64]);
        let index = build(&data).expect("no frames found");
        assert!(index.variable, "varying bitrate misread as constant");
    }

    #[test]
    fn the_offsets_are_monotonic_and_end_at_the_file_length() {
        let (data, _) = stream(&[128, 320, 96, 256, 192, 320, 128, 64, 224, 160]);
        let index = build(&data).unwrap();
        for pair in index.offsets.windows(2) {
            assert!(pair[1] >= pair[0], "offsets went backwards: {pair:?}");
        }
        assert_eq!(index.offsets[0], 0, "the first offset is the start of the file");
        assert_eq!(index.offsets[400] as usize, data.len(), "the last offset is the file length");
    }

    #[test]
    fn an_offset_lands_in_the_right_frame_for_a_vbr_file() {
        // Front-loaded: five fat frames then five thin ones, so the byte
        // midpoint of the file falls nowhere near its time midpoint.
        let bitrates = [320, 320, 320, 320, 320, 32, 32, 32, 32, 32];
        let (data, starts) = stream(&bitrates);
        let index = build(&data).unwrap();

        // Halfway through the track by time is the start of the sixth frame,
        // because every frame carries the same number of samples.
        let midpoint = index.offsets[200];
        assert_eq!(midpoint as usize, starts[5], "the halfway offset is not at frame 5");

        // And a linear byte guess would have been badly wrong, which is the
        // whole reason the table exists.
        let linear = data.len() / 2;
        assert!(
            (midpoint as i64 - linear as i64).unsigned_abs() > 500,
            "this file is not varied enough to be a real test"
        );
    }

    #[test]
    fn an_id3v2_tag_is_skipped() {
        let (audio, starts) = stream(&[256; 20]);
        // A 100-byte ID3v2 tag: "ID3", two version bytes, a flags byte, and a
        // sync-safe size of 90 (the 10-byte header is not counted).
        let mut data = Vec::new();
        data.extend_from_slice(b"ID3\x04\x00\x00");
        data.extend_from_slice(&[0, 0, 0, 90]);
        data.resize(10 + 90, 0);
        let tag_len = data.len();
        data.extend_from_slice(&audio);

        let index = build(&data).unwrap();
        // The first real frame is now 100 bytes into the file, not at zero.
        assert_eq!(index.offsets[0] as usize, tag_len + starts[0]);
    }

    #[test]
    fn a_cbr_index_is_stubbed_the_way_rekordbox_stubs_it() {
        let (data, _) = stream(&[256; 40]);
        let index = build(&data).unwrap();
        let pvbr = index.pvbr_offsets();
        // Every middle entry zero, the last the file length — the shape a real
        // rekordbox CBR export has.
        assert!(pvbr[..400].iter().all(|&o| o == 0), "a CBR stub should be all zeros");
        assert_eq!(pvbr[400] as usize, data.len());
    }

    #[test]
    fn a_vbr_index_keeps_its_real_offsets() {
        let (data, _) = stream(&[320, 32, 320, 32, 320, 32, 320, 32]);
        let index = build(&data).unwrap();
        let pvbr = index.pvbr_offsets();
        assert!(pvbr[100] > 0, "a VBR table should be populated, not stubbed");
        assert_eq!(pvbr, index.offsets);
    }

    #[test]
    fn something_that_is_not_an_mp3_produces_no_index() {
        assert!(build(b"this is not audio, it is a sentence.").is_none());
        assert!(build(&[]).is_none());
    }

    #[test]
    fn frame_sizes_follow_the_format_formula() {
        // A 128 kbps MPEG-1 Layer 3 frame at 44.1 kHz is 417 bytes with no
        // padding: 1152/8 * 128000 / 44100 = 417.9, truncated.
        let frame = parse_frame(&[0xff, 0xfb, 0x90, 0x00], 0).unwrap();
        assert_eq!(frame.bytes, 417);
        assert_eq!(frame.samples, 1152);
        assert_eq!(frame.bitrate_kbps, 128);
    }
}
