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
    Some(CoverArt { data, mime_type })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifies_common_image_formats() {
        let jpeg = identify(vec![0xFF, 0xD8, 0xFF, 0xE0, 0, 0]).unwrap();
        assert_eq!(jpeg.mime_type, "image/jpeg");

        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend_from_slice(&[0; 8]);
        assert_eq!(identify(png).unwrap().mime_type, "image/png");

        let mut webp = b"RIFF".to_vec();
        webp.extend_from_slice(&[0; 4]);
        webp.extend_from_slice(b"WEBPmore");
        assert_eq!(identify(webp).unwrap().mime_type, "image/webp");
    }

    #[test]
    fn rejects_data_that_is_not_an_image() {
        assert!(identify(b"<!DOCTYPE html><html>".to_vec()).is_none());
        assert!(identify(Vec::new()).is_none());
        // A short buffer that starts like a RIFF must not panic on the slice.
        assert!(identify(b"RIFF".to_vec()).is_none());
    }
}
