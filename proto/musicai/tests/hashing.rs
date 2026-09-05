//! Two hashes, and the difference between them.
//!
//! Written against real files rather than made-up bytes, and by retagging them
//! through the same writer the window uses: the whole claim is that a tag
//! write does not disturb the audio hash, and the only way to know that is to
//! write a tag.

use std::path::{Path, PathBuf};

use musicai::audio::encode::{write_file, Codec, EncodeOptions};
use musicai::audio::Audio;
use musicai::hash::{audio_sha256, file_sha256};
use musicai::tag::{Metadata, OnExisting};

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("booth-hash-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn tone(secs: f32, rate: u32, freq: f32) -> Audio {
    let frames = (rate as f32 * secs) as usize;
    let samples: Vec<f32> = (0..frames)
        .map(|i| 0.4 * (std::f32::consts::TAU * freq * i as f32 / rate as f32).sin())
        .collect();
    Audio::new(rate, vec![samples.clone(), samples]).unwrap()
}

fn write(path: &Path, audio: &Audio, codec: Codec) {
    write_file(path, audio, codec, &EncodeOptions::default()).unwrap();
}

fn retag(path: &Path, artist: &str) {
    let metadata = Metadata {
        artist: Some(artist.to_string()),
        title: Some("A tone".to_string()),
        album: Some("Tones".to_string()),
        ..Default::default()
    };
    musicai::tag::write_tags(path, &metadata, OnExisting::Overwrite, None).unwrap();
}

#[test]
fn a_tag_changes_the_file_but_not_the_audio() {
    // The whole point of the second hash. The same rip tagged twice — once
    // from a shop, once after a lookup wrote the artist in — is one recording
    // taking up twice the disk, and the file hash cannot say so.
    // Not WAV: it has nowhere to put a tag, which is its own answer. Its
    // spans are checked below by giving it a chunk it did not have.
    for (name, codec) in [("tone.flac", Codec::Flac), ("tone.mp3", Codec::Mp3)] {
        let scratch = Scratch::new(&format!("retag-{}", codec.extension()));
        let path = scratch.path(name);
        write(&path, &tone(1.0, 44_100, 440.0), codec);

        retag(&path, "First Answer");
        let (file_before, audio_before) =
            (file_sha256(&path).unwrap(), audio_sha256(&path).unwrap());

        retag(&path, "A Longer Second Answer Entirely");
        let (file_after, audio_after) = (file_sha256(&path).unwrap(), audio_sha256(&path).unwrap());

        assert_ne!(file_before, file_after, "{name}: the file did not change at all");
        assert_eq!(audio_before, audio_after, "{name}: retagging moved the audio hash");
    }
}

#[test]
fn a_wav_is_hashed_by_its_samples_and_not_its_chunks() {
    // WAV carries what metadata it has in chunks beside the samples, so the
    // same test in a different shape: give the file a chunk it did not have,
    // and the audio hash should not notice.
    let scratch = Scratch::new("wavchunk");
    let path = scratch.path("tone.wav");
    write(&path, &tone(1.0, 44_100, 440.0), Codec::Wav);
    let before = audio_sha256(&path).unwrap();

    // A `LIST` chunk, appended the way a tagger would, with the RIFF length
    // brought up to match so the file stays well formed.
    let mut bytes = std::fs::read(&path).unwrap();
    let addition: Vec<u8> = b"LIST\x0c\x00\x00\x00INFOIART\x00\x00".to_vec();
    bytes.extend_from_slice(&addition);
    let riff_length = (bytes.len() - 8) as u32;
    bytes[4..8].copy_from_slice(&riff_length.to_le_bytes());
    std::fs::write(&path, &bytes).unwrap();

    assert_ne!(before, file_sha256(&path).unwrap(), "the file really did change");
    assert_eq!(before, audio_sha256(&path).unwrap(), "a chunk moved the audio hash");
}

#[test]
fn different_audio_hashes_differently() {
    // The other half: it has to be a hash of something, not a constant.
    let scratch = Scratch::new("different");
    let a = scratch.path("a.flac");
    let b = scratch.path("b.flac");
    write(&a, &tone(1.0, 44_100, 440.0), Codec::Flac);
    write(&b, &tone(1.0, 44_100, 660.0), Codec::Flac);

    assert_ne!(audio_sha256(&a).unwrap(), audio_sha256(&b).unwrap());
}

#[test]
fn the_same_bytes_hash_the_same_both_ways() {
    let scratch = Scratch::new("copy");
    let a = scratch.path("a.flac");
    let b = scratch.path("b.flac");
    write(&a, &tone(1.0, 44_100, 440.0), Codec::Flac);
    std::fs::copy(&a, &b).unwrap();

    assert_eq!(file_sha256(&a).unwrap(), file_sha256(&b).unwrap());
    assert_eq!(audio_sha256(&a).unwrap(), audio_sha256(&b).unwrap());
}

#[test]
fn a_hash_is_the_real_sha_256() {
    // Against a known answer, so that a change to the reading of a container
    // cannot quietly become a change to what the hash means.
    let scratch = Scratch::new("known");
    let path = scratch.path("empty.unknown");
    std::fs::write(&path, b"").unwrap();
    assert_eq!(
        file_sha256(&path).unwrap(),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );

    let path = scratch.path("abc.unknown");
    std::fs::write(&path, b"abc").unwrap();
    assert_eq!(
        file_sha256(&path).unwrap(),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    // An unknown container falls back to the whole file, which is the safe
    // direction: it can fail to notice a duplicate, never invent one.
    assert_eq!(audio_sha256(&path).unwrap(), file_sha256(&path).unwrap());
}
