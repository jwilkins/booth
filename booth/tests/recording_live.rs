//! Recording from a real input, which no machine that runs the suite has.
//!
//! Ignored, and run on purpose with the mixer plugged in:
//!
//! ```sh
//! BOOTH_RECORD_INPUT="DJM-900NXS2" BOOTH_RECORD_PAIR=3 \
//!     cargo test -p booth --test recording_live -- --ignored --nocapture
//! ```
//!
//! `BOOTH_RECORD_INPUT` is the input's name as Settings lists it, or the
//! identifier this prints beside it, and the default input when it is not
//! set; `BOOTH_RECORD_PAIR` is the first channel of the pair, counted from 1
//! as a mixer's own menu counts them. What it prints is the measurement worth
//! keeping: what the device offered, and how far the file's length is from the
//! time it ran.
//!
//! ALSA's `null` device fails it, and is worth running for that: it hands
//! over silence as fast as it is read rather than at its rate. In a debug
//! build on Linux, five seconds of it put 718 seconds of audio in the file and
//! counted 408 more as dropped. So the disk side keeps up with over a hundred
//! times real time, and a queue that does fill is counted rather than stalling
//! the input. What it cannot show is a clocked input's timing, which is what a
//! mixer is for.

use std::time::{Duration, Instant};

use booth::config::Recording;
use booth::recorder::{inputs, Capture};
use booth_cli::audio::decode::decode_file;

#[test]
#[ignore = "needs an audio input: run with --ignored beside a mixer or an interface"]
fn five_seconds_from_a_real_input_are_five_seconds_in_the_file() {
    let listed = inputs();
    for input in &listed {
        println!(
            "input: {} [{}] ({} channels at {} Hz)",
            input.name, input.id, input.channels, input.rate
        );
    }
    let wanted = std::env::var("BOOTH_RECORD_INPUT").ok().map(|wanted| {
        listed
            .iter()
            .find(|input| input.id == wanted || input.name == wanted)
            .unwrap_or_else(|| panic!("no input is called {wanted:?}"))
            .clone()
    });
    let folder = std::env::temp_dir().join(format!("booth-record-live-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&folder);
    let pair: u16 =
        std::env::var("BOOTH_RECORD_PAIR").ok().and_then(|first| first.parse().ok()).unwrap_or(1);
    let settings = Recording {
        device: wanted.as_ref().map(|input| input.id.clone()),
        device_name: wanted.map(|input| input.name),
        first_channel: pair.saturating_sub(1),
        folder: folder.clone(),
    };

    let capture = Capture::start(&settings, "live").expect("the input would not start");
    println!(
        "recording {} at {} Hz, {} channels, kept awake: {}",
        capture.device, capture.rate, capture.channels, capture.awake
    );
    let began = Instant::now();
    let mut loudest = [0.0f32; 2];
    while began.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(100));
        for (loudest, level) in loudest.iter_mut().zip(capture.levels()) {
            *loudest = loudest.max(level);
        }
    }
    let dropped = capture.dropped_seconds();
    let ran = began.elapsed().as_secs_f64();
    let rate = capture.rate;
    let done = capture.stop().expect("the recording did not close");

    let audio = decode_file(&done.parts[0]).expect("the file does not open");
    let seconds = audio.frames() as f64 / audio.sample_rate as f64;
    println!(
        "ran {ran:.3} s, the file holds {seconds:.3} s, {dropped:.3} s dropped, {} clipped, \
         peaks {:.1} / {:.1} dBFS",
        done.clipped,
        20.0 * loudest[0].max(1e-9).log10(),
        20.0 * loudest[1].max(1e-9).log10(),
    );
    let _ = std::fs::remove_dir_all(&folder);

    assert_eq!(audio.sample_rate, rate);
    assert_eq!(dropped, 0.0, "the disk could not keep up with five seconds");
    // The stream takes a moment to start, and what was in flight at the stop
    // is written: a quarter of a second either way, not more.
    assert!((seconds - ran).abs() < 0.25, "{seconds:.3} s in the file after {ran:.3} s");
}
