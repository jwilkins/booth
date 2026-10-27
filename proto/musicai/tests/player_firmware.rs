//! A drive written here, read by a CDJ-2000NXS2 — the real firmware, under
//! QEMU.
//!
//! [cdj-nxs2-qemu](https://github.com/Stijn-Jacobs/cdj-nxs2-qemu) boots the
//! player's own firmware with board models of its three processors, mounts a
//! disk image as its USB stick, and can be driven headless: one deck boots,
//! presses USB, walks to the first track, loads it, and a scorer says whether
//! the overview strip then moved. That is the nearest thing to a nexus 2 booth
//! that is not one, and the only way short of hardware to find out what the
//! older players make of a drive this writes — the `.EXT` colour waveform they
//! draw and the CDJ-3000 ignores, the 48 kHz ceiling, the FAT32 volume.
//!
//! These are `#[ignore]`d: the emulator is built from source and needs the
//! player's firmware update file, which is yours to supply and nobody's to
//! ship, so none of this can run in CI. Run them deliberately, one at a time —
//! a run boots a whole player, which takes minutes and the machine:
//!
//! ```sh
//! BOOTH_CDJ_EMU=~/cdj-nxs2 cargo test --release --test player_firmware -- --ignored --test-threads=1
//! ```
//!
//! `BOOTH_CDJ_EMU` names a checkout where `./setup.sh` has been through its
//! build and firmware steps for the CDJ-2000NXS2 (the USB step may be skipped:
//! the stick comes from here). Without it the tests pass by doing nothing and
//! say so. `BOOTH_CDJ_WALK` overrides the key walk from the USB root to the
//! first track, in the rig's own words (`push,rot+1,push,...`), should a
//! firmware menu land the cursor somewhere the default does not expect.
//!
//! What a run leaves behind is the rig's: `/tmp/run-<tag>1.txt` is the driver's
//! account (its `VERDICT LOADED` line is the load), `/tmp/bridge-main-<tag>1.log`
//! is the firmware's own internal log, and `/tmp/<tag>1/` holds the frames the
//! playhead was scored from.

use std::path::{Path, PathBuf};
use std::process::Command;

use booth_cli::audio::encode::{write_file, Codec, EncodeOptions};
use booth_cli::audio::Audio;
use booth_cli::cli::{ExportArgs, InputArgs};
use booth_cli::commands;
use booth_cli::report::Collected;

/// A checkout of the emulator with its builds and firmware in place, or
/// nothing — in which case every test here says so and passes.
fn emulator() -> Option<PathBuf> {
    let root = PathBuf::from(std::env::var_os("BOOTH_CDJ_EMU")?);
    if !root.join("scripts/run/rig.sh").is_file() {
        panic!("BOOTH_CDJ_EMU={} is not a cdj-nxs2-qemu checkout", root.display());
    }
    Some(root)
}

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("musicai-firmware-{name}"));
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

const BPM: f64 = 128.0;

/// A track with a beat in it, at the given rate: four to the bar with a kick, a
/// hat and a pad, the drums dropping out for the middle third so that there is
/// an arrangement for the analysis to find. The same shape as the drive tests'
/// song, at whatever sample rate a player's ceiling is being tested with.
fn song(seconds: f32, sample_rate: u32) -> Audio {
    let beat = 60.0 / BPM as f32;
    let beats = (seconds / beat) as usize;
    let rate = sample_rate as f32;
    let len = sample_rate as usize / 2 + (rate * seconds) as usize;
    let mut plane = vec![0.0f32; len];
    let two_pi = 2.0 * std::f32::consts::PI;

    for index in 0..beats {
        let quiet = index > beats / 3 && index < beats * 2 / 3;
        let (kick, hat) = if quiet { (0.3f32, 0.0f32) } else { (1.0, 0.25) };
        let start = sample_rate as usize / 2 + (rate * beat * index as f32) as usize;
        for i in 0..(sample_rate as usize / 4) {
            let Some(sample) = plane.get_mut(start + i) else { break };
            let t = i as f32 / rate;
            let drop = (-t * 18.0).exp();
            *sample += kick * drop * (two_pi * (55.0 + 80.0 * drop) * t).sin();
            if i < sample_rate as usize / 40 {
                *sample += hat * (-t * 90.0).exp() * ((i * 7919 % 1000) as f32 / 500.0 - 1.0);
            }
        }
    }
    for (i, sample) in plane.iter_mut().enumerate() {
        let t = i as f32 / rate;
        *sample += 0.12 * (two_pi * 110.0 * t).sin() + 0.08 * (two_pi * 164.8 * t).sin();
    }
    Audio::new(sample_rate, vec![plane.clone(), plane]).unwrap()
}

fn write_song(scratch: &Scratch, name: &str, sample_rate: u32) -> PathBuf {
    let path = scratch.path(name);
    write_file(&path, &song(20.0, sample_rate), Codec::Flac, &EncodeOptions::default()).unwrap();
    path
}

/// A drive image, written the way the window and `booth-cli export --image`
/// write one: a partitioned FAT32 volume with both databases on it. For the
/// newest player, so that nothing is re-encoded on the way — the point is to
/// hand the older firmware exactly what a CDJ-3000 drive carries.
fn export_image(inputs: Vec<PathBuf>, image: &Path) -> Vec<String> {
    let args = ExportArgs {
        input: InputArgs { inputs, recursive: false },
        drive: None,
        image: Some(image.to_path_buf()),
        label: "REKORDBOX".to_string(),
        // Exhaustive on purpose, as in the drive tests: a new field is a
        // compile error here, and somebody decides what a firmware run wants.
        player: booth_cli::compat::Player::Cdj3000,
        bpm: None,
        playlist: "Firmware".to_string(),
        playlists: Vec::new(),
        dry_run: false,
        no_space_check: false,
        onelibrary_key: None,
        analysed_bits: None,
        no_onelibrary: false,
        companions: Vec::new(),
        already: Vec::new(),
        prepared: Vec::new(),
    };
    let reporter = Collected::new();
    commands::export(&args, &reporter).expect("export to an image failed");
    reporter.lines()
}

/// What one headless run of the rig came to.
struct Run {
    /// The rig's own stdout and stderr, for the failure message.
    output: String,
    /// `/tmp/run-<tag>1.txt`: the driver's account of the load.
    driver: String,
    /// `/tmp/bridge-main-<tag>1.log`: the firmware's internal log.
    firmware: String,
    /// `score_playhead.py`'s verdict on the frames: `MOTION`, `STATIC`,
    /// `NOWAVE`, `CORRUPT` or `WAVELOST`, with its reasons.
    score: String,
}

impl Run {
    fn loaded(&self) -> bool {
        self.driver.contains("VERDICT LOADED")
    }

    fn playhead_moved(&self) -> bool {
        self.score.contains("MOTION")
    }

    /// The firmware's own line for a detail waveform read off the `.EXT`
    /// file — the anchor `report_errors.sh` uses, since the rest of the line
    /// is Shift-JIS.
    fn drew_the_waveform(&self) -> bool {
        self.firmware.lines().any(|line| line.contains("size=") && line.contains(",*P="))
    }

    fn firmware_errors(&self) -> usize {
        self.firmware.matches("ErrEnd").count()
    }

    /// Whether the player would not have the track: its "E-8302 cannot play
    /// track" banner in the firmware's log, or no load at all.
    fn refused_the_track(&self) -> bool {
        self.firmware.contains("E-8302") || !self.loaded()
    }

    fn describe(&self) -> String {
        format!(
            "loaded: {}, waveform read: {}, ErrEnd lines: {}\nscore: {}\nrig:\n{}",
            self.loaded(),
            self.drew_the_waveform(),
            self.firmware_errors(),
            self.score.trim(),
            self.output
        )
    }
}

/// Boot one headless CDJ-2000NXS2 on the image, let the rig load the first
/// track and film the playhead, then score the film.
///
/// The same steps as the emulator's own `scripts/test/nxs2_play.sh`, done
/// here so that the files it leaves can be read back and so that the scorer
/// is told where the frames are without going through `cygpath`.
fn play_on_nxs2(emulator: &Path, image: &Path, tag: &str) -> Run {
    // Absolute, because the rig opens it from its own working directory.
    let image = std::fs::canonicalize(image).unwrap();
    let frames = 4;
    let mut rig = Command::new("bash");
    rig.arg(emulator.join("scripts/run/rig.sh"))
        .arg(tag)
        .arg(frames.to_string())
        .current_dir(emulator)
        .env("MEDIA_MODE", "img")
        .env("MEDIA_IMG_SRC", &image)
        .env("GUI_DISPLAY", "none")
        .env("NDECKS", "1")
        .env("NOSOUND", "1")
        .env("DJLINK", "0")
        .env("REPORTS", "0")
        .env("FILMN", frames.to_string())
        .env("MOTION_MS", "2000");
    if let Some(walk) = std::env::var_os("BOOTH_CDJ_WALK") {
        rig.env("WALK", walk);
    }
    let out = rig.output().expect("could not start the rig");
    let output = format!(
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let read = |path: String| std::fs::read_to_string(&path).unwrap_or_default();
    let driver = read(format!("/tmp/run-{tag}1.txt"));
    let firmware = read(format!("/tmp/bridge-main-{tag}1.log"));

    let scored = Command::new("python3")
        .arg(emulator.join("scripts/run/score_playhead.py"))
        .arg(format!("{tag}1"))
        .current_dir(emulator)
        .env("FRAMES_ROOT", "/tmp")
        .output()
        .expect("could not run the scorer");
    let score = String::from_utf8_lossy(&scored.stdout)
        .lines()
        .chain(String::from_utf8_lossy(&scored.stderr).lines())
        .filter(|line| line.contains("VERDICT"))
        .map(|line| line.trim_matches('*').trim().to_string())
        .collect::<Vec<_>>()
        .join(" ");

    Run { output, driver, firmware, score }
}

#[test]
#[ignore = "boots real CDJ-2000NXS2 firmware under QEMU; needs BOOTH_CDJ_EMU"]
fn a_nexus_2_loads_and_plays_the_first_track_off_a_drive_written_here() {
    let Some(emulator) = emulator() else {
        eprintln!("BOOTH_CDJ_EMU is not set: nothing to boot");
        return;
    };
    let scratch = Scratch::new("nxs2-plays");
    let image = scratch.path("REKORDBOX.img");
    export_image(
        vec![
            write_song(&scratch, "01 - Opener.flac", 44_100),
            write_song(&scratch, "02 - After Hours.flac", 48_000),
        ],
        &image,
    );

    let run = play_on_nxs2(&emulator, &image, "booth");
    assert!(run.loaded(), "the first track did not load:\n{}", run.describe());
    assert!(run.playhead_moved(), "loaded, but did not play:\n{}", run.describe());
    assert!(
        run.drew_the_waveform(),
        "played without reading the .EXT waveform:\n{}",
        run.describe()
    );
    assert_eq!(run.firmware_errors(), 0, "the firmware logged errors:\n{}", run.describe());
}

#[test]
#[ignore = "boots real CDJ-2000NXS2 firmware under QEMU; needs BOOTH_CDJ_EMU"]
fn a_nexus_2_refuses_a_ninety_six_kilohertz_flac() {
    // The single most useful thing the compatibility table claims, and so far
    // only read off a specification sheet: a 96 kHz FLAC copies, browses, and
    // will not load on a nexus 2. Exported for a CDJ-3000, so it is copied as
    // it is rather than re-encoded down.
    let Some(emulator) = emulator() else {
        eprintln!("BOOTH_CDJ_EMU is not set: nothing to boot");
        return;
    };
    let scratch = Scratch::new("nxs2-96k");
    let image = scratch.path("REKORDBOX.img");
    export_image(vec![write_song(&scratch, "01 - Too High.flac", 96_000)], &image);

    let run = play_on_nxs2(&emulator, &image, "booth96");
    assert!(
        run.refused_the_track(),
        "a 96 kHz FLAC was not refused — the table says it would be:\n{}",
        run.describe()
    );
    assert!(
        !run.playhead_moved(),
        "a 96 kHz FLAC played on a nexus 2 — the ceiling in compat.rs is wrong:\n{}",
        run.describe()
    );
}
