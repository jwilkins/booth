//! Playing a track, from a file on disk to the samples a device would get.
//!
//! No device is opened: what is checked is the path from a decoded file to an
//! output buffer, which is where the mistakes live. A test that needs a sound
//! card is a test that does not run on the machine that would catch the bug.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use booth::job::{Job, Runner, Update};
use booth::player::{fill, Sound};
use musicai::audio::encode::{write_file, Codec, EncodeOptions};
use musicai::audio::Audio;

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("booth-audio-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A tone whose two channels differ, so a swapped or collapsed channel shows up.
fn write_tone(path: &Path, secs: f32, rate: u32) {
    let frames = (rate as f32 * secs) as usize;
    let left: Vec<f32> = (0..frames)
        .map(|i| 0.5 * (std::f32::consts::TAU * 220.0 * i as f32 / rate as f32).sin())
        .collect();
    let right: Vec<f32> = left.iter().map(|s| -s).collect();
    let audio = Audio::new(rate, vec![left, right]).unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    write_file(path, &audio, Codec::Wav, &EncodeOptions::default()).unwrap();
}

#[test]
fn a_file_on_disk_becomes_samples_a_device_could_play() {
    let scratch = Scratch::new("decode");
    let path = scratch.0.join("tone.wav");
    write_tone(&path, 1.0, 44_100);

    let mut runner = Runner::start(Job::Decode { id: 7, sources: vec![path] }, Arc::new(|| {}));
    runner.join();

    let sound = runner
        .drain()
        .into_iter()
        .find_map(|update| match update {
            Update::Decoded { id: 7, sound } => Some(sound),
            _ => None,
        })
        .expect("the track should have decoded");

    assert_eq!(sound.rate, 44_100);
    assert_eq!(sound.channels, 2);
    assert_eq!(sound.frames(), 44_100);
    assert!((sound.duration_secs() - 1.0).abs() < 0.001);

    // The two channels are still two channels, and still opposite.
    let mut out = vec![0.0f32; 200];
    fill(&mut out, 2, &sound, 100.0, 1.0, 1.0);
    let pairs: Vec<(f32, f32)> = out.chunks(2).map(|c| (c[0], c[1])).collect();
    assert!(pairs.iter().any(|(l, _)| l.abs() > 0.1), "the decoded audio came out silent");
    for (left, right) in pairs {
        assert!((left + right).abs() < 1e-4, "the channels were mixed together: {left} {right}");
    }
}

#[test]
fn a_track_decoded_at_one_rate_plays_at_another() {
    // The common case on a Mac: a 44.1 kHz file on a 48 kHz device. It must
    // come out the same length in seconds, not the same number of frames.
    let scratch = Scratch::new("resample");
    let path = scratch.0.join("tone.wav");
    write_tone(&path, 1.0, 44_100);

    let mut runner = Runner::start(Job::Decode { id: 1, sources: vec![path] }, Arc::new(|| {}));
    runner.join();
    let sound = runner
        .drain()
        .into_iter()
        .find_map(|update| match update {
            Update::Decoded { sound, .. } => Some(sound),
            _ => None,
        })
        .unwrap();

    let out_rate = 48_000.0;
    let step = sound.rate as f64 / out_rate;
    let mut out = vec![0.0f32; 48_000];
    let end = fill(&mut out, 1, &sound, 0.0, step, 1.0);

    // One second of output consumed one second of source.
    assert!(
        (end - 44_100.0).abs() < 2.0,
        "a second at 48k should be a second at 44.1k, got {end} frames"
    );
}

#[test]
fn a_file_that_will_not_decode_reports_it_rather_than_playing_nothing() {
    let scratch = Scratch::new("broken");
    let path = scratch.0.join("broken.wav");
    std::fs::write(&path, b"not a wav").unwrap();

    let mut runner = Runner::start(Job::Decode { id: 1, sources: vec![path] }, Arc::new(|| {}));
    runner.join();

    let updates = runner.drain();
    assert!(
        !updates.iter().any(|u| matches!(u, Update::Decoded { .. })),
        "a broken file must not produce a sound"
    );
    // The job fails rather than the thread dying, so the window can say so.
    assert!(matches!(updates.last(), Some(Update::Done(Err(_)))), "{}", updates.len());
}

#[test]
fn a_long_track_does_not_have_to_be_decoded_twice_to_be_seeked() {
    // The reason the whole track is decoded up front: seeking anywhere has to
    // be free, because that is what placing and checking cues is made of.
    let sound =
        Sound { samples: (0..1_000).map(|i| i as f32).collect(), channels: 1, rate: 44_100 };
    let mut out = vec![0.0; 4];

    for from in [0.0, 500.0, 999.0] {
        let end = fill(&mut out, 1, &sound, from, 1.0, 1.0);
        assert!(end >= from, "seeking to {from} went backwards");
        assert_eq!(out[0], from.min(999.0) as f32);
    }
}

/// The deck really moves when it is told to play.
///
/// Skipped where there is no output device — CI, a container, an SSH session —
/// because the alternative is a suite that cannot run anywhere without a sound
/// card. Everything this would catch about *mixing* is covered above without a
/// device; what it adds is that the stream, the position and the transport are
/// wired to each other at all.
#[test]
fn a_deck_advances_while_it_plays_and_stops_when_paused() {
    use booth::player::Player;

    let Ok(mut player) = Player::open() else {
        eprintln!("no audio device here — skipping the live deck test");
        return;
    };

    let sound = Arc::new(Sound { samples: vec![0.0; 44_100 * 4], channels: 1, rate: 44_100 });
    player.load(1, sound);
    assert_eq!(player.loaded(), Some(1));
    assert_eq!(player.position_secs(), 0.0);
    assert!(!player.is_playing(), "loading a track must not start it");

    player.play();
    assert!(player.is_playing());
    std::thread::sleep(std::time::Duration::from_millis(300));
    let moved = player.position_secs();
    assert!(moved > 0.0, "the deck did not advance while playing");

    player.pause();
    std::thread::sleep(std::time::Duration::from_millis(150));
    let stopped = player.position_secs();
    std::thread::sleep(std::time::Duration::from_millis(150));
    assert_eq!(player.position_secs(), stopped, "the deck kept moving after pause");

    // And seeking lands where it was told, playing or not.
    player.seek_secs(2.0);
    assert!((player.position_secs() - 2.0).abs() < 0.01);
}

/// An instrumental has no file of its own: it is the melody and drum stems
/// summed. Playing one of the two would be an instrumental missing half of
/// itself, which is the kind of wrong that sounds plausible until you A/B it.
#[test]
fn an_instrumental_is_its_two_stems_added_together() {
    let scratch = Scratch::new("sum");
    let melody = scratch.0.join("Sirens-melody.wav");
    let drums = scratch.0.join("Sirens-drums.wav");
    write_tone(&melody, 1.0, 44_100);
    write_tone(&drums, 1.0, 44_100);

    let one = decode(vec![melody.clone()]);
    let both = decode(vec![melody, drums]);

    assert_eq!(both.samples.len(), one.samples.len(), "summing must not change the length");
    assert_eq!(both.rate, one.rate);
    assert_eq!(both.channels, one.channels);

    // Two copies of the same tone are twice the tone, brought back under full
    // scale together rather than each being halved on the way in.
    let peak = |sound: &Sound| sound.samples.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    assert!(peak(&both) <= 1.0, "the sum has to fit: {}", peak(&both));
    assert!(peak(&both) > 0.9, "and it should use the room it has: {}", peak(&both));

    // The shape is the shape of the parts, not something new: every sample of
    // the sum is the same multiple of the corresponding single-stem sample.
    let ratio = both.samples[100] / one.samples[100];
    for (index, (sum, single)) in both.samples.iter().zip(&one.samples).enumerate().take(4_000) {
        if single.abs() > 0.05 {
            assert!(
                (sum / single - ratio).abs() < 0.01,
                "sample {index} is not the same mix as the rest"
            );
        }
    }
}

/// Two stems of different lengths — a separator can round differently per
/// stem — must not truncate the longer one or read off the end of the shorter.
#[test]
fn stems_of_different_lengths_sum_to_the_longer_one() {
    let scratch = Scratch::new("ragged");
    let short = scratch.0.join("a-melody.wav");
    let long = scratch.0.join("a-drums.wav");
    write_tone(&short, 0.5, 44_100);
    write_tone(&long, 1.0, 44_100);

    let summed = decode(vec![short, long]);
    assert!(
        (summed.samples.len() as i64 - 44_100 * 2).abs() < 400,
        "expected about a second of stereo, got {}",
        summed.samples.len()
    );
}

fn decode(sources: Vec<PathBuf>) -> Sound {
    let mut runner = Runner::start(Job::Decode { id: 1, sources }, Arc::new(|| {}));
    runner.join();
    runner
        .drain()
        .into_iter()
        .find_map(|update| match update {
            Update::Decoded { sound, .. } => Some(sound),
            _ => None,
        })
        .map(|sound| Sound {
            samples: sound.samples.clone(),
            channels: sound.channels,
            rate: sound.rate,
        })
        .expect("it should have decoded")
}
