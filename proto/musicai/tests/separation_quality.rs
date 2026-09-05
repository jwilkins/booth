//! Regression tests for how well the default separator routes sources.
//!
//! The defaults in `stems::dsp::Config` were chosen by measuring exactly this,
//! and they are not obvious — an earlier, plausible-looking set sent the lead
//! vocal into the melody stem and chords into the drum stem. These tests pin
//! the behaviour so that cannot come back unnoticed.

use booth_core::audio::Audio;
use booth_core::stems::dsp;

const SAMPLE_RATE: u32 = 44_100;

/// A mix built from three known sources, kept separately so each stem can be
/// scored against what it should have captured.
struct Mix {
    name: &'static str,
    audio: Audio,
    /// The sung lead: harmonically rich, with vibrato and slow pitch drift.
    lead: Vec<f32>,
    /// Bass plus a sustained triad — steady pitch, no vibrato.
    accompaniment: Vec<f32>,
    /// Kick and hat: broadband transients.
    drums: Vec<f32>,
}

/// Build a mix, varying key, lead register, vibrato and tempo.
fn build(
    name: &'static str,
    root_hz: f32,
    lead_hz: f32,
    vibrato_depth: f32,
    vibrato_rate: f32,
    beats_per_second: usize,
    seed: u32,
) -> Mix {
    let frames = SAMPLE_RATE as usize * 10;
    let mut noise_state = seed;
    let mut noise = move || {
        noise_state = noise_state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (noise_state >> 8) as f32 / (1u32 << 23) as f32 - 1.0
    };

    let mut lead = vec![0.0f32; frames];
    let mut accompaniment = vec![0.0f32; frames];
    let mut drums = vec![0.0f32; frames];
    let mut phase = 0.0f32;
    let tau = 2.0 * std::f32::consts::PI;

    for i in 0..frames {
        let t = i as f32 / SAMPLE_RATE as f32;

        // Steady bass note plus a triad above it.
        accompaniment[i] = 0.35 * (tau * root_hz * t).sin()
            + 0.16
                * ((tau * root_hz * 4.0 * t).sin()
                    + (tau * root_hz * 5.0 * t).sin()
                    + (tau * root_hz * 6.0 * t).sin());

        // The lead's pitch never sits still, which is what marks it as a voice.
        let f = lead_hz
            * (1.0 + vibrato_depth * (tau * vibrato_rate * t).sin() + 0.01 * (tau * 0.7 * t).sin());
        phase += tau * f / SAMPLE_RATE as f32;
        lead[i] = 0.28 * ((phase).sin() + 0.35 * (2.0 * phase).sin() + 0.15 * (3.0 * phase).sin());

        let beat = SAMPLE_RATE as usize / beats_per_second;
        let since_kick = i % beat;
        if since_kick < 1_500 {
            drums[i] += 0.8
                * (tau * 55.0 * since_kick as f32 / SAMPLE_RATE as f32).sin()
                * (-(since_kick as f32) / 1_200.0).exp();
        }
        let since_hat = i % (beat / 2);
        if since_hat < 800 {
            drums[i] += 0.35 * noise() * (-(since_hat as f32) / 120.0).exp();
        }
    }

    // Lead and drums centred, accompaniment spread slightly, as in a real mix.
    let mut left = Vec::with_capacity(frames);
    let mut right = Vec::with_capacity(frames);
    for i in 0..frames {
        left.push(0.5 * (accompaniment[i] * 1.1 + lead[i] + drums[i]));
        right.push(0.5 * (accompaniment[i] * 0.9 + lead[i] + drums[i]));
    }

    Mix {
        name,
        audio: Audio::new(SAMPLE_RATE, vec![left, right]).unwrap(),
        lead,
        accompaniment,
        drums,
    }
}

fn correlation(a: &[f32], b: &[f32]) -> f64 {
    let dot: f64 = a.iter().zip(b).map(|(&x, &y)| x as f64 * y as f64).sum();
    let energy_a: f64 = a.iter().map(|&x| (x as f64).powi(2)).sum();
    let energy_b: f64 = b.iter().map(|&x| (x as f64).powi(2)).sum();
    if energy_a * energy_b <= 0.0 {
        0.0
    } else {
        dot / (energy_a * energy_b).sqrt()
    }
}

/// How cleanly one source was routed: how much better the stem that should
/// have caught it correlates with it than either of the other two. Positive
/// means the source went to the right place.
struct Routing {
    lead_to_vocals: f64,
    accompaniment_to_melody: f64,
    drums_to_drums: f64,
}

fn route(mix: &Mix, config: &dsp::Config) -> Routing {
    let stems = dsp::separate(&mix.audio, config).unwrap();
    let vocals = &stems.vocals.planes[0];
    let melody = &stems.melody.planes[0];
    let drums = &stems.drums.planes[0];

    Routing {
        lead_to_vocals: correlation(vocals, &mix.lead)
            - correlation(melody, &mix.lead).max(correlation(drums, &mix.lead)),
        accompaniment_to_melody: correlation(melody, &mix.accompaniment)
            - correlation(vocals, &mix.accompaniment).max(correlation(drums, &mix.accompaniment)),
        drums_to_drums: correlation(drums, &mix.drums)
            - correlation(vocals, &mix.drums).max(correlation(melody, &mix.drums)),
    }
}

fn all_mixes() -> Vec<Mix> {
    vec![
        build("low key, strong vibrato", 82.4, 660.0, 0.030, 5.5, 2, 7),
        build("high key, light vibrato", 146.8, 880.0, 0.015, 6.5, 3, 99),
        build("low lead, slow vibrato", 65.4, 330.0, 0.025, 4.5, 2, 1_234),
        build("fast tempo, deep vibrato", 110.0, 550.0, 0.045, 7.0, 4, 555),
    ]
}

#[test]
fn defaults_route_every_source_to_the_right_stem() {
    for mix in all_mixes() {
        let r = route(&mix, &dsp::Config::default());
        assert!(
            r.lead_to_vocals > 0.25,
            "{}: the lead did not clearly land in the vocal stem ({:.3})",
            mix.name,
            r.lead_to_vocals
        );
        assert!(
            r.accompaniment_to_melody > 0.5,
            "{}: the accompaniment did not clearly land in the melody stem ({:.3})",
            mix.name,
            r.accompaniment_to_melody
        );
        assert!(
            r.drums_to_drums > 0.4,
            "{}: the drums did not clearly land in the drum stem ({:.3})",
            mix.name,
            r.drums_to_drums
        );
    }
}

#[test]
fn a_drum_window_too_short_to_resolve_chords_is_worse() {
    // This is the trap the defaults were tuned away from: at 1024 points the
    // bins are 43 Hz wide, chord tones in the low-mid range fall inside a
    // single bin, and the frequency median reads the whole chord as a
    // transient. Kept as a test so the reasoning behind `drum_fft` is checked
    // rather than just asserted in a comment.
    let mix = build("low key, strong vibrato", 82.4, 660.0, 0.030, 5.5, 2, 7);

    let good = route(&mix, &dsp::Config::default());
    let too_short = route(&mix, &dsp::Config { drum_fft: 1024, ..Default::default() });

    assert!(
        good.accompaniment_to_melody > too_short.accompaniment_to_melody + 0.2,
        "a 1024-point drum window was not measurably worse: {:.3} vs {:.3}",
        too_short.accompaniment_to_melody,
        good.accompaniment_to_melody
    );
}

#[test]
fn a_wide_vocal_frequency_kernel_loses_the_lead() {
    // The other half of the tuning: a frequency median much wider than a
    // vibrato-smeared partial swallows the lead along with the steady notes,
    // and the vocal stem stops tracking the voice.
    let mix = build("low key, strong vibrato", 82.4, 660.0, 0.030, 5.5, 2, 7);

    let good = route(&mix, &dsp::Config::default());
    let too_wide = route(&mix, &dsp::Config { voice_freq_hz: 200.0, ..Default::default() });

    assert!(
        good.lead_to_vocals > too_wide.lead_to_vocals + 0.2,
        "a 200 Hz vocal kernel was not measurably worse: {:.3} vs {:.3}",
        too_wide.lead_to_vocals,
        good.lead_to_vocals
    );
}

#[test]
fn centre_weighting_helps_on_a_mix_with_panned_accompaniment() {
    // The lead is centred and the accompaniment is not, so leaning on the
    // stereo image should improve the vocal stem rather than harm it.
    let mix = build("low key, strong vibrato", 82.4, 660.0, 0.030, 5.5, 2, 7);

    let with_centre = route(&mix, &dsp::Config::default());
    let without = route(&mix, &dsp::Config { center_weight: 0.0, ..Default::default() });

    assert!(
        with_centre.lead_to_vocals >= without.lead_to_vocals,
        "centre weighting hurt the vocal stem: {:.3} with, {:.3} without",
        with_centre.lead_to_vocals,
        without.lead_to_vocals
    );
}
