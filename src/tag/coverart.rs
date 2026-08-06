//! Cover art from the Cover Art Archive.

use std::time::Duration;

use anyhow::Result;

use super::http::Http;

pub const BASE_URL: &str = "https://coverartarchive.org";

/// Refuse anything larger than this, so one unusually big scan cannot blow up
/// memory or the file it gets embedded into.
pub const DEFAULT_MAX_BYTES: usize = 8 * 1024 * 1024;

/// The archive sits behind a CDN and is not as strict as MusicBrainz, but
/// there is no reason to hammer it.
pub const DEFAULT_MIN_INTERVAL: Duration = Duration::from_millis(250);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CoverArt {
    pub data: Vec<u8>,
    pub mime_type: &'static str,
    /// Pixel dimensions, zero when they could not be determined.
    pub width: u32,
    pub height: u32,
    /// Colour depth in bits per pixel, zero when unknown.
    pub depth: u32,
}

pub struct Client {
    http: Http,
    max_bytes: usize,
}

impl Client {
    pub fn new(min_interval: Duration, max_bytes: usize) -> Self {
        Self { http: Http::new(min_interval), max_bytes }
    }

    /// Fetch the front cover for a release, if it has one.
    ///
    /// `Ok(None)` means the release genuinely has no front cover, which is
    /// common and not an error.
    pub fn front(&mut self, release_mbid: &str) -> Result<Option<CoverArt>> {
        let url = format!("{BASE_URL}/release/{release_mbid}/front");
        let Some(data) = self.http.get_bytes_optional(&url, self.max_bytes)? else {
            return Ok(None);
        };
        Ok(identify(data))
    }
}

/// Work out the image type from its leading bytes.
///
/// The archive's redirect loses the content type often enough that sniffing
/// the data is more reliable than trusting the header, and an image embedded
/// with the wrong MIME type will not display.
pub fn identify(data: Vec<u8>) -> Option<CoverArt> {
    let mime_type = if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
        "image/jpeg"
    } else if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        "image/png"
    } else if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") {
        "image/gif"
    } else if data.len() > 12 && data.starts_with(b"RIFF") && &data[8..12] == b"WEBP" {
        "image/webp"
    } else {
        return None;
    };

    let (width, height, depth) = measure(mime_type, &data).unwrap_or((0, 0, 0));
    Some(CoverArt { data, mime_type, width, height, depth })
}

/// Read pixel dimensions and colour depth out of an image header.
///
/// A FLAC `PICTURE` block carries these as fields of its own, and leaving them
/// zero writes a technically incomplete block. Only the headers are parsed —
/// enough for the formats the Cover Art Archive actually serves — and anything
/// unrecognised falls back to zeros, which is what the spec says to use when
/// the values are unknown.
fn measure(mime_type: &str, data: &[u8]) -> Option<(u32, u32, u32)> {
    match mime_type {
        "image/png" => measure_png(data),
        "image/jpeg" => measure_jpeg(data),
        "image/gif" => measure_gif(data),
        // WebP stores dimensions differently per compression mode; not worth
        // parsing for a format the archive rarely serves.
        _ => None,
    }
}

fn be_u32(bytes: &[u8]) -> u32 {
    u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

/// PNG puts an IHDR chunk immediately after the signature.
fn measure_png(data: &[u8]) -> Option<(u32, u32, u32)> {
    if data.len() < 26 || &data[12..16] != b"IHDR" {
        return None;
    }
    let width = be_u32(&data[16..20]);
    let height = be_u32(&data[20..24]);

    let bit_depth = u32::from(data[24]);
    let channels = match data[25] {
        0 => 1, // greyscale
        2 => 3, // truecolour
        3 => 1, // palette index
        4 => 2, // greyscale + alpha
        6 => 4, // truecolour + alpha
        _ => return None,
    };
    Some((width, height, bit_depth * channels))
}

/// JPEG keeps its dimensions in a start-of-frame segment, which has to be
/// found by walking the segment chain.
fn measure_jpeg(data: &[u8]) -> Option<(u32, u32, u32)> {
    let mut i = 2; // skip the SOI marker
    while i + 9 < data.len() {
        if data[i] != 0xFF {
            i += 1;
            continue;
        }
        let marker = data[i + 1];
        // SOF0 through SOF15 carry the frame header, except for three markers
        // in that range that mean something else entirely.
        let is_start_of_frame =
            (0xC0..=0xCF).contains(&marker) && !matches!(marker, 0xC4 | 0xC8 | 0xCC);
        if is_start_of_frame {
            let precision = u32::from(data[i + 4]);
            let height = u32::from(u16::from_be_bytes([data[i + 5], data[i + 6]]));
            let width = u32::from(u16::from_be_bytes([data[i + 7], data[i + 8]]));
            let components = u32::from(data[i + 9]);
            return Some((width, height, precision * components));
        }
        // Markers without a payload; everything else has a length to skip.
        if matches!(marker, 0xD8 | 0xD9 | 0x01) || (0xD0..=0xD7).contains(&marker) {
            i += 2;
        } else {
            let length = u16::from_be_bytes([data[i + 2], data[i + 3]]) as usize;
            if length < 2 {
                return None;
            }
            i += 2 + length;
        }
    }
    None
}

fn measure_gif(data: &[u8]) -> Option<(u32, u32, u32)> {
    if data.len() < 11 {
        return None;
    }
    let width = u32::from(u16::from_le_bytes([data[6], data[7]]));
    let height = u32::from(u16::from_le_bytes([data[8], data[9]]));
    // Low three bits of the packed field hold the palette size exponent.
    let depth = u32::from((data[10] & 0b111) + 1);
    Some((width, height, depth))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A PNG header describing a `width` x `height` 8-bit truecolour image.
    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut data = b"\x89PNG\r\n\x1a\n".to_vec();
        data.extend_from_slice(&[0, 0, 0, 13]); // IHDR length
        data.extend_from_slice(b"IHDR");
        data.extend_from_slice(&width.to_be_bytes());
        data.extend_from_slice(&height.to_be_bytes());
        data.push(8); // bit depth
        data.push(2); // colour type: truecolour
        data.extend_from_slice(&[0, 0, 0]);
        data
    }

    /// A JPEG with one filler segment before the start-of-frame, so the parser
    /// has to walk the chain rather than assume a fixed offset.
    fn jpeg(width: u16, height: u16) -> Vec<u8> {
        let mut data = vec![0xFF, 0xD8];
        // APP0 segment: marker, length 16, then 14 bytes of payload.
        data.extend_from_slice(&[0xFF, 0xE0, 0x00, 0x10]);
        data.extend_from_slice(&[0; 14]);
        // SOF0.
        data.extend_from_slice(&[0xFF, 0xC0, 0x00, 0x11, 8]);
        data.extend_from_slice(&height.to_be_bytes());
        data.extend_from_slice(&width.to_be_bytes());
        data.push(3); // components
        data.extend_from_slice(&[0; 9]);
        data
    }

    #[test]
    fn identifies_common_image_formats() {
        assert_eq!(identify(jpeg(10, 10)).unwrap().mime_type, "image/jpeg");
        assert_eq!(identify(png(10, 10)).unwrap().mime_type, "image/png");

        let mut webp = b"RIFF".to_vec();
        webp.extend_from_slice(&[0; 4]);
        webp.extend_from_slice(b"WEBPmore");
        assert_eq!(identify(webp).unwrap().mime_type, "image/webp");
    }

    #[test]
    fn measures_png_dimensions() {
        let art = identify(png(600, 400)).unwrap();
        assert_eq!((art.width, art.height), (600, 400));
        // 8 bits across three channels.
        assert_eq!(art.depth, 24);
    }

    #[test]
    fn measures_jpeg_dimensions_past_earlier_segments() {
        let art = identify(jpeg(1200, 1200)).unwrap();
        assert_eq!((art.width, art.height), (1200, 1200));
        assert_eq!(art.depth, 24);
    }

    #[test]
    fn measures_gif_dimensions() {
        let mut data = b"GIF89a".to_vec();
        data.extend_from_slice(&320u16.to_le_bytes());
        data.extend_from_slice(&240u16.to_le_bytes());
        data.push(0b1000_0111); // palette exponent 7 -> 8 bits
        data.extend_from_slice(&[0, 0]);
        let art = identify(data).unwrap();
        assert_eq!((art.width, art.height, art.depth), (320, 240, 8));
    }

    #[test]
    fn unmeasurable_images_fall_back_to_zero_rather_than_failing() {
        // A truncated PNG is still recognisably a PNG; it just cannot be
        // measured, and zeros are what the FLAC spec uses for unknown.
        let mut truncated = b"\x89PNG\r\n\x1a\n".to_vec();
        truncated.extend_from_slice(&[0; 8]);
        let art = identify(truncated).unwrap();
        assert_eq!(art.mime_type, "image/png");
        assert_eq!((art.width, art.height, art.depth), (0, 0, 0));
    }

    #[test]
    fn a_malformed_jpeg_segment_chain_terminates() {
        // A zero length would otherwise leave the scan stuck on one offset.
        let mut data = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x00];
        data.extend_from_slice(&[0; 32]);
        assert_eq!(identify(data).unwrap().width, 0);
    }

    #[test]
    fn rejects_data_that_is_not_an_image() {
        assert!(identify(b"<!DOCTYPE html><html>".to_vec()).is_none());
        assert!(identify(Vec::new()).is_none());
        // A short buffer that starts like a RIFF must not panic on the slice.
        assert!(identify(b"RIFF".to_vec()).is_none());
    }
}
