//! Music that is not in the library, and what happens when it is reached for.
//!
//! The rule this is testing is one sentence: a track referenced from somewhere
//! that might not be there is a track that will be missing on the night. What
//! follows checks that the default takes a copy, that a copy is a copy and not
//! a move, and that the collection stops depending on the original once it has
//! one.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use booth::config::{copy_in, Config, OnExternal};
use booth::job::{Adoptable, Job, Runner, Update};
use booth::library::{Library, Track};

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("booth-copy-{name}-{}", std::process::id()));
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

fn write(path: &Path, bytes: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

#[test]
fn the_default_takes_a_copy_and_the_original_is_left_alone() {
    let scratch = Scratch::new("adopt");
    let elsewhere = scratch.0.join("borrowed-usb").join("Marius.flac");
    write(&elsewhere, b"the audio");

    let config = Config {
        library_path: scratch.0.join("library"),
        on_external: OnExternal::default(),
        ..Config::default()
    };
    assert_eq!(config.on_external, OnExternal::Copy);
    assert!(!config.holds(&elsewhere));

    let mut collection = Library::new();
    let id = collection.add(&elsewhere);
    collection.get_mut(id).unwrap().artist = "Batu".into();

    let job = Job::Adopt {
        tracks: vec![Adoptable { id, path: elsewhere.clone(), artist: "Batu".into() }],
        config: Box::new(config.clone()),
    };
    let mut runner = Runner::start(job, Arc::new(|| {}));
    runner.join();

    let landed = runner
        .drain()
        .into_iter()
        .find_map(|update| match update {
            Update::Adopted { id: reported, to } if reported == id => Some(to),
            _ => None,
        })
        .expect("the copy should have been reported");

    assert_eq!(landed, config.library_path.join("Batu").join("Marius.flac"));
    assert_eq!(std::fs::read(&landed).unwrap(), b"the audio");
    assert!(config.holds(&landed));

    // The whole point of a copy: someone who pointed at borrowed music gets it
    // back exactly as they lent it.
    assert!(elsewhere.exists(), "the original was moved rather than copied");
    assert_eq!(std::fs::read(&elsewhere).unwrap(), b"the audio");
}

#[test]
fn a_track_already_in_the_library_is_not_copied_again() {
    let scratch = Scratch::new("already");
    let config = Config { library_path: scratch.0.join("library"), ..Config::default() };
    let inside = config.library_path.join("Batu").join("Marius.flac");
    write(&inside, b"the audio");

    // The window only ever adopts what `holds` says is outside, so the two have
    // to agree — this is the check that they do.
    assert!(config.holds(&inside));
    let landed = copy_in(&config, "Batu", &inside).unwrap();
    assert_eq!(landed, inside, "copying a file onto itself made a second one");
    let count = std::fs::read_dir(config.library_path.join("Batu")).unwrap().count();
    assert_eq!(count, 1);
}

#[test]
fn a_track_whose_file_is_gone_is_not_something_to_copy() {
    let scratch = Scratch::new("missing");
    let config = Config { library_path: scratch.0.join("library"), ..Config::default() };
    let vanished = scratch.0.join("gone.flac");

    // It is outside the library, but there is nothing to take a copy of; the
    // answer is the inspector's "forget this track", not a failed copy.
    assert!(!config.holds(&vanished));
    assert!(!vanished.exists());
    assert!(copy_in(&config, "X", &vanished).is_err());
}

#[test]
fn a_failed_copy_does_not_take_the_rest_of_the_batch_with_it() {
    let scratch = Scratch::new("partial-batch");
    let config = Config { library_path: scratch.0.join("library"), ..Config::default() };
    let good = scratch.0.join("good.flac");
    write(&good, b"audio");
    let missing = scratch.0.join("missing.flac");

    let job = Job::Adopt {
        tracks: vec![
            Adoptable { id: 1, path: missing, artist: "X".into() },
            Adoptable { id: 2, path: good, artist: "X".into() },
        ],
        config: Box::new(config),
    };
    let mut runner = Runner::start(job, Arc::new(|| {}));
    runner.join();

    let updates = runner.drain();
    assert!(updates.iter().any(|u| matches!(u, Update::Failed { .. })), "the missing one");
    assert!(
        updates.iter().any(|u| matches!(u, Update::Adopted { id: 2, .. })),
        "the good one should still have been copied"
    );
    assert!(matches!(updates.last(), Some(Update::Done(Ok(())))));
}

#[test]
fn the_policy_decides_and_the_default_is_to_copy() {
    // Stated as a test because it is the behaviour that was asked for, and a
    // default that quietly flips is a default nobody notices has changed.
    assert_eq!(Config::default().on_external, OnExternal::Copy);
    assert_eq!(OnExternal::ALL.len(), 3);
    for policy in OnExternal::ALL {
        assert!(!policy.label().is_empty());
        assert!(!policy.blurb().is_empty());
    }
}

#[test]
fn editing_a_cue_makes_the_drive_out_of_date() {
    // Cue editing is only worth having if the drive notices. This is the join
    // between the two: an edited cue is an update on the next sync, not a
    // silent difference between the laptop and the stick.
    let mut track = Track::placeholder(1);
    track.artist = "Batu".into();
    track.title = "Marius".into();
    track.bpm = 130.0;
    track.has_grid = true;

    let before = booth::sync::fingerprint(&track);
    track.cues.push(booth::library::CueMark {
        letter: 3,
        time_ms: 64_000,
        label: "drop".into(),
        color: [226, 160, 63],
    });
    let after = booth::sync::fingerprint(&track);
    assert_ne!(before, after, "a placed cue must show up as a change");

    // Moving it is a change too, and so is removing it again.
    track.cues[0].time_ms = 64_500;
    assert_ne!(booth::sync::fingerprint(&track), after);
    track.cues.clear();
    assert_eq!(booth::sync::fingerprint(&track), before);
}

#[test]
fn naming_a_cue_is_not_something_the_player_sees() {
    // The label is for the person prepping, and does not reach the drive, so
    // renaming one must not mark a whole track for rewriting.
    let mut track = Track::placeholder(1);
    track.cues.push(booth::library::CueMark {
        letter: 1,
        time_ms: 1_000,
        label: String::new(),
        color: [0, 0, 0],
    });
    let before = booth::sync::fingerprint(&track);
    track.cues[0].label = "first drop".into();
    assert_eq!(
        booth::sync::fingerprint(&track),
        before,
        "renaming a cue should not rewrite the drive"
    );
}

#[test]
fn editing_the_names_marks_the_track_for_rewriting() {
    // The artist and title are in the database on the drive, so they do count.
    let mut track = Track::placeholder(1);
    track.artist = "Batu".into();
    track.title = "Marius".into();
    let before = booth::sync::fingerprint(&track);

    track.title = "Marius (Extended)".into();
    assert_ne!(booth::sync::fingerprint(&track), before);
}

#[test]
fn a_tag_is_the_collections_business_and_not_the_drives() {
    let mut track = Track::placeholder(1);
    let before = booth::sync::fingerprint(&track);
    track.tags.push("peak".into());
    assert_eq!(
        booth::sync::fingerprint(&track),
        before,
        "tagging a track should not queue a 40 MB rewrite"
    );
}

/// Editing a name and asking for it to be written really does rewrite the file.
///
/// Through the job, not the tag library directly, because what is being checked
/// is that the window hands over what the user typed — and reads back with the
/// same reader the import uses, so a write the importer cannot see is a failure.
#[test]
fn writing_tags_puts_the_collections_names_into_the_file() {
    use musicai::audio::encode::{write_file, Codec, EncodeOptions};
    use musicai::audio::Audio;

    let scratch = Scratch::new("retag");
    let path = scratch.0.join("track.flac");
    let rate = 44_100;
    let samples: Vec<f32> = (0..rate)
        .map(|i| 0.1 * (std::f32::consts::TAU * 220.0 * i as f32 / rate as f32).sin())
        .collect();
    let audio = Audio::new(rate as u32, vec![samples.clone(), samples]).unwrap();
    std::fs::create_dir_all(&scratch.0).unwrap();
    write_file(&path, &audio, Codec::Flac, &EncodeOptions::default()).unwrap();

    let job = Job::Retag(vec![booth::job::Retag {
        id: 1,
        path: path.clone(),
        artist: "Peverelist".into(),
        title: "Roll With The Punches".into(),
        album: "Livity Sound".into(),
        date: Some("2019".into()),
    }]);
    let mut runner = Runner::start(job, Arc::new(|| {}));
    runner.join();
    assert!(matches!(runner.drain().last(), Some(Update::Done(Ok(())))));

    let read = musicai::tag::read_metadata(&path).unwrap();
    assert_eq!(read.artist.as_deref(), Some("Peverelist"));
    assert_eq!(read.title.as_deref(), Some("Roll With The Punches"));
    assert_eq!(read.album.as_deref(), Some("Livity Sound"));
    assert_eq!(read.date.as_deref(), Some("2019"));
}

/// A WAV has nowhere to put them, and says so rather than appearing to work.
#[test]
fn a_format_with_no_tag_block_reports_that_it_has_none() {
    use musicai::audio::encode::{write_file, Codec, EncodeOptions};
    use musicai::audio::Audio;

    let scratch = Scratch::new("retag-wav");
    let path = scratch.0.join("track.wav");
    let audio = Audio::new(44_100, vec![vec![0.0; 4_410], vec![0.0; 4_410]]).unwrap();
    std::fs::create_dir_all(&scratch.0).unwrap();
    write_file(&path, &audio, Codec::Wav, &EncodeOptions::default()).unwrap();

    let job = Job::Retag(vec![booth::job::Retag {
        id: 1,
        path: path.clone(),
        artist: "X".into(),
        title: "Y".into(),
        album: String::new(),
        date: None,
    }]);
    let mut runner = Runner::start(job, Arc::new(|| {}));
    runner.join();

    let updates = runner.drain();
    assert!(
        updates.iter().any(|u| matches!(u, Update::Failed { .. })),
        "a silent no-op would look like it worked: {}",
        updates.len()
    );
    // And the batch still ends cleanly rather than the thread dying.
    assert!(matches!(updates.last(), Some(Update::Done(Ok(())))));
}
