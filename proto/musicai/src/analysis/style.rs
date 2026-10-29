//! What kind of record a track is, heard by a network trained on Discogs.
//!
//! The one trained model in the analysis — see [`super`] for why the rest is
//! not. Tempo and key can be measured; "is this Goa or melodic trance" cannot,
//! it can only be recognised, and recognising it well enough to tell hard
//! dance from tech house takes a model trained on a great many records that
//! people had already filed. Discogs is that filing: 400 styles, each the name
//! a record shop would use, and the model here was trained to predict them from
//! the audio.
//!
//! # The model
//!
//! Discogs-EffNet, from the Music Technology Group at Universitat Pompeu Fabra:
//! an EfficientNet-B0 trained on four million Discogs-tagged tracks, described
//! in Alonso-Jiménez, Serra and Bogdanov, *Music Representation Learning Based
//! on Editorial Metadata from Discogs* (ISMIR 2022). The file used here is
//! their ONNX export, [`MODEL_URL`], which this program reads with a pure-Rust
//! runtime, so there is no Python and no TensorFlow to install.
//!
//! The weights are released under CC BY-NC-SA 4.0, which is why the model is
//! not in this repository and is not fetched by the program: it is yours to
//! download, and it goes under `~/.cache/booth/models/` or wherever the setting
//! says. Booth is free software, and the attribution is above and in the
//! README. Nothing is downloaded at run time — the rule for the whole program.
//!
//! # The front end, measured rather than reasoned
//!
//! The model does not take audio; it takes 128-frame patches of a 96-band
//! log-mel spectrogram at 16 kHz, cut exactly the way Essentia's
//! `TensorflowPredictEffnetDiscogs` cuts them, and a classifier handed a
//! spectrogram that is nearly the same is a classifier that is nearly right.
//! Every stage here was checked against Essentia itself, running on the same
//! signal, and the numbers it was checked against are the tests' fixtures:
//!
//! * 512-sample frames every 256, a symmetric Hann window with no
//!   normalisation, an unscaled FFT and its power spectrum;
//! * 96 triangular bands from 0 to 8 kHz on Slaney's mel scale, each scaled
//!   to unit area (`unit_tri`), on the power spectrum;
//! * `log10(1 + 10000 x)` of each band — not a decibel scale.
//!
//! Framing is the part worth saying twice. The first frame is centred on the
//! first sample, so it starts half a frame before the signal with zeros in the
//! gap, and frames carry on until their centre passes the end of the signal,
//! zero-padded likewise. Starting from sample zero instead — the obvious
//! thing — puts every frame half a hop out and moves the activations by a few
//! hundredths; centred, the Rust front end and the TensorFlow graph agree to
//! five decimal places on every patch. The frames go to the model 128 at a
//! time, 62 apart, and a last patch that would not be full is dropped, which is
//! the predictor's own `lastPatchMode='discard'`.
//!
//! What comes out is one sigmoid activation per style, averaged over the
//! patches. They are not a distribution — a track can score high on two
//! styles, and most score low on all — so what is reported is the surest
//! style and how sure, not a share.
//!
//! On a real record the whole path was then checked against Essentia's own
//! loader and predictor (a netlabel techno release, 4:52, 292 patches, 16
//! October 2026). Handed Essentia's 16 kHz mono, this agrees with it to 0.0001
//! on every one of the 400 styles. Handed the MP3, so that the decoder and the
//! resampler are this program's, the surest style is the same (Techno, 0.60
//! against 0.54) and no style differs by more than 0.061 — which is the size
//! of the difference between Essentia's own resampling qualities on the same
//! file (0.066 between its best and its default sinc, 0.067 between its
//! default and linear). The model is that sensitive to the resampler, and
//! this one sits inside the spread of Essentia's.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use realfft::{RealFftPlanner, RealToComplex};
use tract_onnx::prelude::*;

use super::style_classes::CLASSES;
use super::tempo::Octave;
use crate::audio::resample::resample;
use crate::audio::Audio;

/// The file the model ships as.
pub const MODEL_FILE: &str = "discogs-effnet-bsdynamic-1.onnx";
/// Where the Music Technology Group publishes it.
pub const MODEL_URL: &str = "https://essentia.upf.edu/models/music-style-classification/discogs-effnet/discogs-effnet-bsdynamic-1.onnx";
/// The published file, as fetched on 15 October 2026. So that a download can
/// be checked, not so that another file is refused: a retrained model with
/// the same outputs would run.
pub const MODEL_SHA256: &str = "a280825b334797cf677939db8cd5762c0392aedd0ca6415dbc1cd083f045e43c";
/// Names the model file when no setting does.
pub const MODEL_VARIABLE: &str = "BOOTH_GENRE_MODEL";

/// The rate the model was trained at.
pub const MODEL_RATE: u32 = 16_000;
const FRAME: usize = 512;
const HOP: usize = 256;
const BINS: usize = FRAME / 2 + 1;
const BANDS: usize = 96;
const TOP_HZ: f64 = 8_000.0;
/// Frames per patch, and the hop between patches, in frames.
const PATCH: usize = 128;
const PATCH_HOP: usize = 62;
/// One activation per style.
const STYLES: usize = CLASSES.len();

/// How many patches go to the model in one run.
///
/// The model takes any number at once — its batch dimension is left symbolic
/// when it is loaded — and one at a time turned out fastest: on a minute of the
/// reference signal, release build, one thread, 44 ms a patch alone against 48
/// in eights and 61 in thirty-twos. A five-minute track is some 290 patches,
/// so about thirteen seconds; the measurement is kept as an ignored test below.
/// (onnxruntime does the same patch in 1.4 ms. The pure-Rust runtime is the
/// price of nothing to install.)
const BATCH: usize = 1;

/// Where the weights go when nothing says otherwise.
///
/// `~/.cache/booth/models/` — honouring `XDG_CACHE_HOME` — which is where the
/// repository's session-start hook puts them and what the README says to do
/// by hand. The same place on every platform, because the one thing a person
/// following the README should not need is a different path per machine.
pub fn default_location() -> Option<PathBuf> {
    let cache = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))?;
    Some(cache.join("booth").join("models").join(MODEL_FILE))
}

/// The model file: the one named, else the one `BOOTH_GENRE_MODEL` names,
/// else the one in the cache if it is there.
///
/// A named file is returned whether or not it exists, so that a setting
/// pointing at nothing fails loudly when it is loaded rather than being
/// skipped in silence; only the cache is checked, because nobody wrote that
/// path down.
pub fn locate(named: Option<&Path>) -> Option<PathBuf> {
    located(named, std::env::var(MODEL_VARIABLE).ok(), default_location())
}

/// The rule behind [`locate`], apart from the environment so that it can be
/// tested on any machine — see `booth::config::Whisper::chosen` for the week
/// of red tests that taught this repository to keep the two apart.
fn located(
    named: Option<&Path>,
    from_env: Option<String>,
    cached: Option<PathBuf>,
) -> Option<PathBuf> {
    if let Some(named) = named.filter(|path| !path.as_os_str().is_empty()) {
        return Some(named.to_path_buf());
    }
    if let Some(variable) = from_env.map(|v| v.trim().to_string()).filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(variable));
    }
    cached.filter(|path| path.is_file())
}

/// One of the 400 styles, with how sure the model was of it.
#[derive(Clone, Debug, PartialEq)]
pub struct Style {
    /// Discogs's genre, the broad family: `Electronic`.
    pub genre: &'static str,
    /// Discogs's style within it: `Goa Trance`. What a DJ calls the genre, and
    /// what goes in a player's genre field.
    pub style: &'static str,
    /// The model's activation for this style, 0 to 1, averaged over the track.
    /// Not a share of anything: the 400 do not sum to one.
    pub confidence: f32,
}

impl Style {
    fn at(index: usize, confidence: f32) -> Self {
        let (genre, style) = split(CLASSES[index]);
        Self { genre, style, confidence }
    }
}

/// `Genre---Style` into its two halves.
pub fn split(class: &str) -> (&str, &str) {
    class.split_once("---").unwrap_or((class, class))
}

/// A genre whose tempo is counted in one octave.
struct Counted {
    because: &'static str,
    from_bpm: f64,
    /// Its Discogs styles, as genre and style.
    styles: &'static [(&'static str, &'static str)],
}

/// The genres whose tempo is counted in one octave: what each is called, where
/// its octave starts, and the Discogs styles that make it up.
///
/// Only styles whose convention is clear. Trap and Miami Bass are filed under
/// hip hop but counted from about 130, Cloud Rap and Crunk either way;
/// Downtempo runs on past 120 and Breakcore past 240. A record of theirs keeps
/// the octave the correlation chose.
const OCTAVES: [Counted; 3] = [
    Counted {
        because: "drum and bass",
        from_bpm: 120.0,
        styles: &[
            ("Electronic", "Drum n Bass"),
            ("Electronic", "Jungle"),
            ("Electronic", "Halftime"),
        ],
    },
    Counted {
        because: "hip hop",
        from_bpm: 60.0,
        styles: &[
            ("Hip Hop", "Boom Bap"),
            ("Hip Hop", "Conscious"),
            ("Hip Hop", "Cut-up/DJ"),
            ("Hip Hop", "G-Funk"),
            ("Hip Hop", "Gangsta"),
            ("Hip Hop", "Hardcore Hip-Hop"),
            ("Hip Hop", "Horrorcore"),
            ("Hip Hop", "Instrumental"),
            ("Hip Hop", "Jazzy Hip-Hop"),
            ("Hip Hop", "Pop Rap"),
            ("Hip Hop", "Thug Rap"),
            ("Hip Hop", "Trip Hop"),
            ("Hip Hop", "Turntablism"),
            ("Electronic", "Trip Hop"),
        ],
    },
    Counted {
        because: "dubstep",
        from_bpm: 100.0,
        styles: &[("Electronic", "Dubstep"), ("Electronic", "Grime"), ("Hip Hop", "Grime")],
    },
];

/// How sure the model has to be of a genre before its octave is taken: its
/// styles' activations added up, over the five styles it is surest of.
///
/// Summed, because the model spreads a genre over its styles: The Werewolf
/// scores 0.34 for instrumental hip hop and 0.28 and 0.27 for the two trip
/// hops. On 75 CC records, every tempo this moved came with 0.36 or more, and
/// the four scoring from 0.23 to 0.30 were in their genre's octave already, so
/// where in that gap the line goes is reasoned rather than measured. Under it
/// the model is guessing: it hears Hard Epic Dubstep as dubstep at 0.10, after
/// synth-pop and electro.
const OCTAVE_SURE: f32 = 0.3;
const OCTAVE_TOP: usize = 5;

/// The octave named `because` in [`OCTAVES`], for a track that was heard in it
/// once and is measured again without the model.
pub fn octave_called(because: &str) -> Option<Octave> {
    OCTAVES
        .iter()
        .find(|counted| counted.because == because)
        .map(|counted| Octave { from_bpm: counted.from_bpm, because: counted.because })
}

/// The octave a file's own genre tag puts its tempo in: a tag naming one of
/// the [`OCTAVES`] genres, and no other.
///
/// Somebody's filing rather than the model's ear, and on 75 CC records worth
/// nearly as much: of the 62 tempos madmom and Essentia agree on or a listener
/// has counted, the tags alone would have taken Booth from 51 to 56, the model
/// from 51 to 57. A tag that names two of them (IDM, Jungle, Dubstep) says
/// nothing, and nor does one with trap in it, which is filed with hip hop and
/// counted from about 130.
pub fn octave_tagged(tag: &str) -> Option<Octave> {
    const WORDS: [(&str, &[&str]); 3] = [
        (
            "drum and bass",
            &["drum & bass", "drum and bass", "drum n bass", "drum'n'bass", "dnb", "d&b", "jungle"],
        ),
        ("hip hop", &["hip-hop", "hip hop", "hiphop", "trip-hop", "trip hop", "boom bap"]),
        ("dubstep", &["dubstep", "grime"]),
    ];
    let tag = tag.to_lowercase();
    if tag.contains("trap") {
        return None;
    }
    let mut named = WORDS.iter().filter(|(_, words)| words.iter().any(|word| tag.contains(word)));
    match (named.next(), named.next()) {
        (Some(&(because, _)), None) => octave_called(because),
        _ => None,
    }
}

/// The octave a record's tempo is counted in, if the model is sure enough of
/// a genre that has one. `styles` surest first, as [`Classifier::classify`]
/// gives them.
pub fn octave(styles: &[Style]) -> Option<Octave> {
    let top = &styles[..styles.len().min(OCTAVE_TOP)];
    OCTAVES
        .iter()
        .map(|counted| {
            let sure: f32 = top
                .iter()
                .filter(|style| counted.styles.contains(&(style.genre, style.style)))
                .map(|style| style.confidence)
                .sum();
            (sure, Octave { from_bpm: counted.from_bpm, because: counted.because })
        })
        .filter(|&(sure, _)| sure >= OCTAVE_SURE)
        .max_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, octave)| octave)
}

/// The model, loaded and ready to be asked about tracks.
///
/// Loading is a second or two — the runtime reads the graph and specialises it
/// — so a job that has many tracks to classify loads once and shares it: a
/// classifier can be asked from several threads at a time.
pub struct Classifier {
    model: Arc<TypedRunnableModel>,
    front_end: FrontEnd,
}

impl Classifier {
    pub fn load(path: &Path) -> Result<Self> {
        let model = tract_onnx::onnx()
            .model_for_path(path)
            .with_context(|| format!("reading the genre model {}", path.display()))?;
        // The graph names its batch dimension rather than fixing it, and the
        // runtime wants to be told so rather than handed a number: a fixed
        // batch fails to unify with the name inside the graph.
        let batch = model.symbols.sym("batch_size");
        let model = model
            .with_input_fact(0, f32::fact([batch.to_dim(), PATCH.to_dim(), BANDS.to_dim()]).into())?
            // The graph also hands out its 1280-dimensional embedding, which
            // is for building other classifiers on and not for this.
            .with_outputs_by_name(["activations"])?
            .into_optimized()?
            .into_runnable()
            .with_context(|| format!("preparing the genre model {}", path.display()))?;
        Ok(Self { model, front_end: FrontEnd::new() })
    }

    /// The model's activation for every style, averaged over the track's
    /// patches, in [`CLASSES`] order. Empty for a track too short for one
    /// patch — just over two seconds.
    pub fn activations(&self, audio: &Audio) -> Result<Vec<f32>> {
        self.activations_in_batches(audio, BATCH)
    }

    fn activations_in_batches(&self, audio: &Audio, batch: usize) -> Result<Vec<f32>> {
        let mono = audio.to_mono();
        let signal = resample(&mono, audio.sample_rate, MODEL_RATE);
        let mel = self.front_end.mel(&signal);
        let patches = patch_count(mel.len() / BANDS);
        if patches == 0 {
            return Ok(Vec::new());
        }

        let mut sum = vec![0.0f64; STYLES];
        let mut input = Vec::with_capacity(batch * PATCH * BANDS);
        for first in (0..patches).step_by(batch) {
            let count = (patches - first).min(batch);
            input.clear();
            for patch in first..first + count {
                let from = patch * PATCH_HOP * BANDS;
                input.extend_from_slice(&mel[from..from + PATCH * BANDS]);
            }
            let tensor = Tensor::from_shape(&[count, PATCH, BANDS], &input)?;
            let out = self.model.run(tvec!(tensor.into_tvalue()))?;
            let activations = out[0].view().as_slice::<f32>()?;
            for row in activations.chunks_exact(STYLES) {
                for (total, &activation) in sum.iter_mut().zip(row) {
                    *total += activation as f64;
                }
            }
        }
        Ok(sum.iter().map(|total| (total / patches as f64) as f32).collect())
    }

    /// Every style, surest first. Empty for a track too short to hear.
    pub fn classify(&self, audio: &Audio) -> Result<Vec<Style>> {
        let activations = self.activations(audio)?;
        let mut styles: Vec<Style> =
            activations.iter().enumerate().map(|(index, &c)| Style::at(index, c)).collect();
        styles.sort_by(|a, b| b.confidence.total_cmp(&a.confidence));
        Ok(styles)
    }
}

/// How many frames a signal of `samples` samples cuts into: one per hop,
/// counting the one centred on the first sample, up to the last whose centre is
/// still inside the signal.
fn frame_count(samples: usize) -> usize {
    samples.div_ceil(HOP)
}

/// How many whole patches `frames` frames make. A last partial one is dropped.
fn patch_count(frames: usize) -> usize {
    match frames < PATCH {
        true => 0,
        false => (frames - PATCH) / PATCH_HOP + 1,
    }
}

/// Audio at 16 kHz to the log-mel frames the model reads.
struct FrontEnd {
    window: Vec<f32>,
    fft: Arc<dyn RealToComplex<f32>>,
    /// Per band: the first bin that counts, and the weights from there on.
    bands: Vec<(usize, Vec<f32>)>,
}

impl FrontEnd {
    fn new() -> Self {
        // Symmetric, not periodic: Essentia's `Windowing(type='hann')` divides
        // by N-1, and the two differ enough at this length to show in the
        // reference frames.
        let window = (0..FRAME)
            .map(|i| {
                (0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / (FRAME - 1) as f64).cos()) as f32
            })
            .collect();
        let fft = RealFftPlanner::<f32>::new().plan_fft_forward(FRAME);
        Self { window, fft, bands: filterbank() }
    }

    /// Frame-major: `frames * BANDS` values.
    fn mel(&self, signal: &[f32]) -> Vec<f32> {
        let frames = frame_count(signal.len());
        let mut out = Vec::with_capacity(frames * BANDS);
        let mut time = vec![0.0f32; FRAME];
        let mut spectrum = self.fft.make_output_vec();
        let mut scratch = self.fft.make_scratch_vec();
        let mut power = [0.0f32; BINS];

        for frame in 0..frames {
            let start = frame as isize * HOP as isize - (FRAME / 2) as isize;
            for (i, slot) in time.iter_mut().enumerate() {
                *slot = match usize::try_from(start + i as isize) {
                    Ok(at) if at < signal.len() => signal[at] * self.window[i],
                    _ => 0.0,
                };
            }
            self.fft
                .process_with_scratch(&mut time, &mut spectrum, &mut scratch)
                .expect("FFT input and output lengths are fixed by construction");
            for (p, bin) in power.iter_mut().zip(&spectrum) {
                *p = bin.norm_sqr();
            }
            for (first, weights) in &self.bands {
                let energy: f32 = weights.iter().zip(&power[*first..]).map(|(w, p)| w * p).sum();
                out.push((1.0 + 10_000.0 * energy).log10());
            }
        }
        out
    }
}

/// Slaney's mel scale, as Essentia's `warpingFormula='slaneyMel'` and
/// librosa's default have it: linear to 1 kHz, logarithmic above.
fn hz_to_mel(hz: f64) -> f64 {
    const F_SP: f64 = 200.0 / 3.0;
    const MIN_LOG_HZ: f64 = 1_000.0;
    let min_log_mel = MIN_LOG_HZ / F_SP;
    let logstep = 6.4f64.ln() / 27.0;
    match hz < MIN_LOG_HZ {
        true => hz / F_SP,
        false => min_log_mel + (hz / MIN_LOG_HZ).ln() / logstep,
    }
}

fn mel_to_hz(mel: f64) -> f64 {
    const F_SP: f64 = 200.0 / 3.0;
    const MIN_LOG_HZ: f64 = 1_000.0;
    let min_log_mel = MIN_LOG_HZ / F_SP;
    let logstep = 6.4f64.ln() / 27.0;
    match mel < min_log_mel {
        true => mel * F_SP,
        false => MIN_LOG_HZ * (logstep * (mel - min_log_mel)).exp(),
    }
}

/// The 96 triangles, each as the bins it covers and their weights.
///
/// Essentia's `MelBands` with `weighting='linear'`, `normalize='unit_tri'`:
/// the triangle rises from one edge to the next and falls to the one after,
/// both ends exclusive on the way up and inclusive at the peak, and is scaled
/// by `2 / (upper - lower)` so that it has unit area in hertz.
fn filterbank() -> Vec<(usize, Vec<f32>)> {
    let (low, high) = (hz_to_mel(0.0), hz_to_mel(TOP_HZ));
    let edges: Vec<f64> = (0..BANDS + 2)
        .map(|i| mel_to_hz(low + (high - low) * i as f64 / (BANDS + 1) as f64))
        .collect();
    let bin_hz = MODEL_RATE as f64 / FRAME as f64;

    (0..BANDS)
        .map(|band| {
            let (lower, centre, upper) = (edges[band], edges[band + 1], edges[band + 2]);
            let scale = 2.0 / (upper - lower);
            let mut first = None;
            let mut weights = Vec::new();
            for bin in 0..BINS {
                let hz = bin as f64 * bin_hz;
                let weight = if hz >= lower && hz < centre {
                    (hz - lower) / (centre - lower)
                } else if hz >= centre && hz < upper {
                    (upper - hz) / (upper - centre)
                } else {
                    0.0
                };
                if weight > 0.0 {
                    first.get_or_insert(bin);
                    weights.push((weight * scale) as f32);
                } else if first.is_some() {
                    break;
                }
            }
            (first.unwrap_or(0), weights)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The reference signal the fixtures below were measured on: three tones,
    /// a click every half second, no noise — so that it can be rebuilt here
    /// to the sample. Six seconds, which is four patches.
    fn reference_signal() -> Vec<f32> {
        use std::f64::consts::TAU;
        (0..MODEL_RATE as usize * 6)
            .map(|i| {
                let t = i as f64 / MODEL_RATE as f64;
                let mut sample = 0.5 * (TAU * 55.0 * t).sin()
                    + 0.25 * (TAU * 1_000.0 * t).sin()
                    + 0.1 * (TAU * 6_000.0 * t).sin();
                if i % (MODEL_RATE as usize / 2) < 32 {
                    sample += 0.8;
                }
                sample as f32
            })
            .collect()
    }

    /// Essentia 2.1's `TensorflowInputMusiCNN` on the 512 samples from 0,
    /// measured on 15 October 2026. Frame 1 here, since frame 0 is centred on
    /// sample 0 and starts half a frame before it.
    const MEL_FROM_0: [f32; BANDS] = [
        5.78411, 6.08155, 5.17919, 3.43655, 1.63775, 0.82621, 1.0136, 1.16056, 1.22076, 1.23541,
        1.22365, 1.1948, 1.15402, 1.10462, 1.04912, 0.9897, 0.92857, 0.86795, 0.81009, 0.7571,
        0.71059, 0.67146, 0.63968, 0.61432, 0.59377, 0.57593, 0.55825, 0.53702, 0.50383, 0.42012,
        4.84829, 5.45348, 5.06637, 3.94446, 0.39411, 0.38443, 0.36965, 0.35119, 0.35249, 0.33539,
        0.29711, 0.31294, 0.2798, 0.2784, 0.25662, 0.25902, 0.22906, 0.22663, 0.21963, 0.20135,
        0.1938, 0.18599, 0.17773, 0.1689, 0.15964, 0.15038, 0.14805, 0.13464, 0.12933, 0.12565,
        0.1173, 0.11058, 0.10757, 0.10024, 0.09535, 0.09086, 0.08619, 0.08327, 0.07722, 0.07405,
        0.07078, 0.0675, 0.06369, 0.06092, 0.05816, 0.05531, 0.05301, 0.05063, 0.04837, 0.04618,
        0.04446, 0.04265, 0.04077, 0.03949, 0.03793, 0.03639, 2.4762, 4.07258, 2.79965, 0.03282,
        0.03159, 0.03118, 0.03049, 0.03002, 0.02978, 0.02954,
    ];

    /// The same on the 512 samples from 25,600: frame 101, between clicks,
    /// where the bands between the tones are as near empty as float gets.
    const MEL_FROM_25600: [f32; BANDS] = [
        5.78139,
        6.0823,
        5.18068,
        3.45392,
        1.90056,
        1.15319,
        0.654259,
        0.343777,
        0.173974,
        0.0890136,
        0.0473386,
        0.0263706,
        0.0153538,
        0.00929183,
        0.00580821,
        0.00372286,
        0.0024322,
        0.00160517,
        0.00106086,
        0.00069319,
        0.000437977,
        0.000258162,
        0.000130653,
        4.61781e-5,
        9.31885e-6,
        5.64795e-5,
        0.000321333,
        0.00134208,
        0.00608812,
        0.0469582,
        4.84436,
        5.45531,
        5.06496,
        3.94061,
        0.0090902,
        0.00159692,
        0.000512757,
        0.000194515,
        8.53118e-5,
        3.64977e-5,
        1.5376e-5,
        8.74937e-6,
        4.14174e-6,
        2.3815e-6,
        1.39784e-6,
        9.83666e-7,
        6.21263e-7,
        5.17719e-7,
        4.65947e-7,
        3.62403e-7,
        3.10632e-7,
        3.10632e-7,
        2.5886e-7,
        2.5886e-7,
        2.07088e-7,
        1.55316e-7,
        1.55316e-7,
        1.55316e-7,
        1.03544e-7,
        1.03544e-7,
        1.03544e-7,
        1.03544e-7,
        5.17719e-8,
        5.17719e-8,
        5.17719e-8,
        5.17719e-8,
        5.17719e-8,
        5.17719e-8,
        5.17719e-8,
        5.17719e-8,
        5.17719e-8,
        5.17719e-8,
        0.0,
        0.0,
        0.0,
        0.0,
        5.17719e-8,
        5.17719e-8,
        5.17719e-8,
        5.17719e-8,
        5.17719e-8,
        1.03544e-7,
        1.55316e-7,
        3.62403e-7,
        1.13898e-6,
        6.78207e-6,
        2.47604,
        4.07264,
        2.7988,
        6.0055e-6,
        8.2835e-7,
        2.07088e-7,
        5.17719e-8,
        5.17719e-8,
        0.0,
        0.0,
    ];

    #[test]
    fn the_front_end_reproduces_essentias_mel_frames() {
        let mel = FrontEnd::new().mel(&reference_signal());
        for (frame, wanted) in [(1usize, &MEL_FROM_0), (101, &MEL_FROM_25600)] {
            let got = &mel[frame * BANDS..(frame + 1) * BANDS];
            let (band, worst) = got
                .iter()
                .zip(wanted.iter())
                .map(|(g, w)| (g - w).abs())
                .enumerate()
                .fold((0, 0.0f32), |best, (i, d)| if d > best.1 { (i, d) } else { best });
            assert!(worst < 2e-3, "frame {frame}, band {band}: off by {worst}");
        }
    }

    #[test]
    fn the_first_frame_is_centred_on_the_first_sample() {
        // One click at sample 0 and nothing else. The frame centred there puts
        // it under the middle of the window and hears it in every band; the
        // frame starting there puts it under the window's first point, which
        // is zero, and hears nothing. A front end that started at zero would
        // only have the second.
        let mut click = vec![0.0f32; MODEL_RATE as usize];
        click[0] = 1.0;
        let mel = FrontEnd::new().mel(&click);
        assert_eq!(mel.len() / BANDS, frame_count(click.len()));
        assert!(mel[..BANDS].iter().all(|&band| band > 0.0), "{:?}", &mel[..4]);
        assert!(
            mel[BANDS..2 * BANDS].iter().all(|&band| band == 0.0),
            "{:?}",
            &mel[BANDS..BANDS + 4]
        );
    }

    #[test]
    fn frames_and_patches_are_counted_the_way_the_predictor_counts_them() {
        // Measured on 15 October 2026 against Essentia's
        // TensorflowPredictEffnetDiscogs, by handing it noise of each length
        // and counting the rows it returned. The two rules it tells apart:
        // keeping a frame while it fits inside the signal gives one patch at
        // 48,639 samples and four at 96,255; keeping it while its centre is
        // inside gives two and five, which is what the predictor gave.
        for (samples, patches) in [
            (32_767, 1),
            (32_768, 1),
            (48_383, 1),
            (48_384, 1),
            (48_639, 2),
            (48_640, 2),
            (96_000, 4),
            (96_255, 5),
            (96_256, 5),
        ] {
            assert_eq!(patch_count(frame_count(samples)), patches, "{samples} samples");
        }
        assert_eq!(frame_count(0), 0);
        assert_eq!(patch_count(0), 0);
        assert_eq!(patch_count(PATCH - 1), 0, "a track too short for one patch has none");
    }

    #[test]
    fn the_bands_are_essentias() {
        // Slaney's scale is linear below 1 kHz, where 97 steps from 0 to the
        // mel of 8 kHz come out 31.097 Hz apart, and the top edge is 8 kHz
        // itself.
        let (low, high) = (hz_to_mel(0.0), hz_to_mel(TOP_HZ));
        let edge = |i: usize| mel_to_hz(low + (high - low) * i as f64 / (BANDS + 1) as f64);
        assert!((edge(1) - 31.097).abs() < 0.001, "{}", edge(1));
        assert!((edge(2) - 62.194).abs() < 0.001, "{}", edge(2));
        assert!((edge(BANDS + 1) - 8_000.0).abs() < 1e-6, "{}", edge(BANDS + 1));
        assert!((mel_to_hz(hz_to_mel(3_456.0)) - 3_456.0).abs() < 1e-9);

        let bands = filterbank();
        assert_eq!(bands.len(), BANDS);
        // The lowest band is one bin wide: bin 1 at 31.25 Hz, just past its
        // 31.097 Hz peak. Nothing below the first bin counts, and nothing at
        // 8 kHz does either.
        assert_eq!(bands[0].0, 1);
        assert_eq!(bands[0].1.len(), 1);
        let (first, weights) = &bands[BANDS - 1];
        assert!(first + weights.len() < BINS, "the top band reaches the Nyquist bin");
    }

    fn heard(styles: &[(&'static str, &'static str, f32)]) -> Vec<Style> {
        styles
            .iter()
            .map(|&(genre, style, confidence)| Style { genre, style, confidence })
            .collect()
    }

    #[test]
    fn a_genre_whose_styles_add_up_to_sure_enough_says_its_octave() {
        let jungle = heard(&[
            ("Electronic", "Drum n Bass", 0.2),
            ("Electronic", "Jungle", 0.15),
            ("Electronic", "Techno", 0.1),
        ]);
        assert_eq!(
            octave(&jungle).map(|o| (o.because, o.from_bpm)),
            Some(("drum and bass", 120.0))
        );
        // Each alone is under what it takes; the two hip hops together are not.
        let hip_hop = heard(&[("Hip Hop", "Instrumental", 0.2), ("Electronic", "Trip Hop", 0.12)]);
        assert_eq!(octave(&hip_hop).map(|o| o.because), Some("hip hop"));
        assert_eq!(octave(&hip_hop[..1]), None);
    }

    #[test]
    fn a_tag_naming_one_of_the_genres_says_its_octave_and_one_naming_two_does_not() {
        let named = |tag: &str| octave_tagged(tag).map(|octave| octave.because);
        assert_eq!(named("Drum & Bass"), Some("drum and bass"));
        assert_eq!(named("Instrumental Hip-Hop"), Some("hip hop"));
        assert_eq!(named("Trip-Hop/Lo-Fi/Swing"), Some("hip hop"));
        assert_eq!(named("Dubstep"), Some("dubstep"));
        assert_eq!(named("IDM, Jungle, Dubstep, Experimental"), None);
        assert_eq!(named("Hip-Hop/Trap"), None);
        assert_eq!(named("Tech House"), None);
    }

    #[test]
    fn an_octave_kept_by_name_is_found_again_and_an_unknown_name_is_not() {
        let found = octave_called("drum and bass").unwrap();
        assert_eq!((found.because, found.from_bpm), ("drum and bass", 120.0));
        assert_eq!(octave_called("polka"), None);
    }

    #[test]
    fn a_style_counted_either_way_or_past_the_fifth_says_nothing() {
        let trap = heard(&[("Hip Hop", "Trap", 0.6), ("Hip Hop", "Cloud Rap", 0.3)]);
        assert_eq!(octave(&trap), None);
        let mut far = heard(&[("Electronic", "Techno", 0.5); 5]);
        far.extend(heard(&[("Electronic", "Drum n Bass", 0.4)]));
        assert_eq!(octave(&far), None);
    }

    #[test]
    fn a_class_splits_into_discogs_genre_and_style() {
        assert_eq!(split("Electronic---Goa Trance"), ("Electronic", "Goa Trance"));
        assert_eq!(split("Stage & Screen---Score"), ("Stage & Screen", "Score"));
        // Every one of the 400 has both halves — "Classical---Classical" has
        // the same word twice, which is Discogs's filing and not a fault.
        for class in CLASSES {
            let (genre, style) = split(class);
            assert!(class.contains("---") && !genre.is_empty() && !style.is_empty(), "{class}");
        }
        // The ones this was asked for in the first place.
        for wanted in ["Goa Trance", "Progressive Trance", "Tech House", "Hard House", "Hardstyle"]
        {
            assert!(CLASSES.iter().any(|c| split(c).1 == wanted), "no {wanted}");
        }
    }

    #[test]
    fn the_setting_wins_then_the_environment_then_the_cache() {
        let named = Some(Path::new("/set/model.onnx"));
        let env = Some("/env/model.onnx".to_string());
        let missing = Some(PathBuf::from("/nowhere/model.onnx"));
        assert_eq!(
            located(named, env.clone(), missing.clone()).unwrap(),
            Path::new("/set/model.onnx")
        );
        assert_eq!(
            located(Some(Path::new("")), env.clone(), missing.clone()).unwrap(),
            Path::new("/env/model.onnx"),
            "an empty setting is no setting"
        );
        assert_eq!(
            located(None, Some("  ".into()), missing).map(|p| p.display().to_string()),
            None,
            "a cached file that is not there is not an answer"
        );

        // The cache counts only when the file is really there, since nobody
        // wrote that path down and a missing one should read as not set up.
        let dir = std::env::temp_dir().join("booth-style-locate");
        std::fs::create_dir_all(&dir).unwrap();
        let cached = dir.join(MODEL_FILE);
        std::fs::write(&cached, b"not really a model").unwrap();
        assert_eq!(located(None, None, Some(cached.clone())), Some(cached));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_classifier_can_be_shared_between_threads() {
        fn shareable<T: Send + Sync>() {}
        shareable::<Classifier>();
    }

    /// What the TensorFlow graph, through Essentia, made of the reference
    /// signal on 15 October 2026: the five surest of the 400, by index into
    /// [`CLASSES`]. Clicks and sines read as glitch, which is fair.
    const HEARD: [(usize, &str, f32); 5] = [
        (77, "Electronic---Glitch", 0.4862),
        (105, "Electronic---Minimal", 0.2814),
        (70, "Electronic---Experimental", 0.2723),
        (35, "Electronic---Ambient", 0.1878),
        (129, "Electronic---Techno", 0.1691),
    ];

    /// The one test that needs the weights, so it does not run in CI.
    /// `BOOTH_GENRE_MODEL=... cargo test -p booth-cli style -- --ignored`.
    #[test]
    #[ignore = "needs the model file: BOOTH_GENRE_MODEL or ~/.cache/booth/models"]
    fn the_model_hears_the_reference_signal_the_way_essentia_does() {
        let path = locate(None).expect("no model file to test against");
        let started = std::time::Instant::now();
        let classifier = Classifier::load(&path).unwrap();
        eprintln!("loaded {} in {:.2}s", path.display(), started.elapsed().as_secs_f32());

        let audio = Audio::new(MODEL_RATE, vec![reference_signal()]).unwrap();
        let started = std::time::Instant::now();
        let activations = classifier.activations(&audio).unwrap();
        eprintln!("four patches in {:.3}s", started.elapsed().as_secs_f32());

        assert_eq!(activations.len(), STYLES);
        let mut worst = 0.0f32;
        for (index, class, confidence) in HEARD {
            assert_eq!(CLASSES[index], class);
            worst = worst.max((activations[index] - confidence).abs());
            assert!(
                (activations[index] - confidence).abs() < 5e-3,
                "{class}: {} here, {confidence} from Essentia",
                activations[index]
            );
        }
        eprintln!("furthest from Essentia on the five: {worst:.5}");
        let styles = classifier.classify(&audio).unwrap();
        assert_eq!((styles[0].genre, styles[0].style), ("Electronic", "Glitch"));
        assert!(styles.windows(2).all(|pair| pair[0].confidence >= pair[1].confidence));

        // A stereo track at a studio rate is the same track.
        let at_44100 = resample(&reference_signal(), MODEL_RATE, 44_100);
        let stereo = Audio::new(44_100, vec![at_44100.clone(), at_44100]).unwrap();
        let again = classifier.classify(&stereo).unwrap();
        assert_eq!(again[0].style, "Glitch");
        assert!((again[0].confidence - styles[0].confidence).abs() < 0.05, "{:?}", again[0]);

        // And nothing is heard in nothing.
        let short = Audio::new(MODEL_RATE, vec![vec![0.0; MODEL_RATE as usize]]).unwrap();
        assert!(classifier.classify(&short).unwrap().is_empty());
    }

    /// How long a patch takes at each batch size, for choosing [`BATCH`].
    #[test]
    #[ignore = "a measurement, not a check: needs the model file"]
    fn how_long_a_patch_takes_at_each_batch_size() {
        let path = locate(None).expect("no model file to time");
        let started = std::time::Instant::now();
        let classifier = Classifier::load(&path).unwrap();
        eprintln!("loaded in {:.2}s", started.elapsed().as_secs_f32());
        let minute = Audio::new(MODEL_RATE, vec![reference_signal().repeat(10)]).unwrap();
        let patches = patch_count(frame_count(minute.frames()));
        for batch in [1, 8, 32, 128] {
            let started = std::time::Instant::now();
            let activations = classifier.activations_in_batches(&minute, batch).unwrap();
            let took = started.elapsed().as_secs_f32();
            eprintln!(
                "batch {batch}: {patches} patches in {took:.2}s = {:.1} ms each (top {:.3})",
                took * 1000.0 / patches as f32,
                activations.iter().cloned().fold(0.0f32, f32::max)
            );
        }
    }
}
