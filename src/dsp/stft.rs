//! Short-time Fourier transform with exact overlap-add reconstruction.

use std::sync::Arc;

use realfft::num_complex::Complex32;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};

/// A magnitude/phase-preserving spectrogram in frame-major order.
///
/// `data[frame * bins + bin]`, with `bins == n_fft / 2 + 1`.
#[derive(Clone, Debug)]
pub struct Spectrogram {
    pub frames: usize,
    pub bins: usize,
    pub data: Vec<Complex32>,
}

impl Spectrogram {
    pub fn frame(&self, index: usize) -> &[Complex32] {
        &self.data[index * self.bins..(index + 1) * self.bins]
    }

    pub fn frame_mut(&mut self, index: usize) -> &mut [Complex32] {
        &mut self.data[index * self.bins..(index + 1) * self.bins]
    }

    /// Per-bin magnitudes, same layout as `data`.
    pub fn magnitudes(&self) -> Vec<f32> {
        self.data.iter().map(|c| c.norm()).collect()
    }

    /// Multiply each bin by the corresponding entry of `mask`.
    pub fn apply_mask(&self, mask: &[f32]) -> Spectrogram {
        debug_assert_eq!(mask.len(), self.data.len());
        Spectrogram {
            frames: self.frames,
            bins: self.bins,
            data: self.data.iter().zip(mask).map(|(c, &m)| c * m).collect(),
        }
    }

    /// Subtract `other` from `self` in place.
    ///
    /// Lets a caller get the complement of a masked spectrogram without
    /// building a second mask and a third full-size buffer.
    pub fn subtract(&mut self, other: &Spectrogram) {
        debug_assert_eq!(self.data.len(), other.data.len());
        for (a, b) in self.data.iter_mut().zip(&other.data) {
            *a -= b;
        }
    }
}

/// STFT/ISTFT pair sharing one FFT plan.
///
/// Analysis and synthesis both use a square-root Hann window. With the
/// overlap-add normalization applied on the way back out, `inverse(forward(x))`
/// reproduces `x` to within floating point error, including at the edges —
/// which matters here because we reconstruct three stems that must sum back to
/// the original mix.
pub struct Stft {
    n_fft: usize,
    hop: usize,
    window: Vec<f32>,
    forward: Arc<dyn RealToComplex<f32>>,
    inverse: Arc<dyn ComplexToReal<f32>>,
}

impl Stft {
    /// `n_fft` must be even and `hop` must divide it evenly for the window to
    /// satisfy the constant-overlap-add condition.
    pub fn new(n_fft: usize, hop: usize) -> Self {
        assert!(n_fft >= 2 && n_fft % 2 == 0, "n_fft must be even");
        assert!(hop > 0 && hop <= n_fft, "hop must be in 1..=n_fft");

        let mut planner = RealFftPlanner::<f32>::new();
        // Square root of a periodic Hann window, so analysis * synthesis is a
        // plain Hann window and overlap-add sums to a constant.
        let window = (0..n_fft)
            .map(|i| {
                let hann = 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / n_fft as f32).cos();
                hann.sqrt()
            })
            .collect();

        Self {
            n_fft,
            hop,
            window,
            forward: planner.plan_fft_forward(n_fft),
            inverse: planner.plan_fft_inverse(n_fft),
        }
    }

    pub fn n_fft(&self) -> usize {
        self.n_fft
    }

    pub fn hop(&self) -> usize {
        self.hop
    }

    pub fn bins(&self) -> usize {
        self.n_fft / 2 + 1
    }

    /// Number of frames produced for a signal of `len` samples.
    pub fn frame_count(&self, len: usize) -> usize {
        // Centred framing: the signal is zero-padded by n_fft/2 on both sides
        // so that frame `t` is centred on sample `t * hop`.
        len.div_ceil(self.hop) + self.n_fft / self.hop
    }

    /// Analyse `signal` into a spectrogram.
    pub fn forward(&self, signal: &[f32]) -> Spectrogram {
        let frames = self.frame_count(signal.len());
        let bins = self.bins();
        let pad = self.n_fft / 2;

        let mut data = vec![Complex32::default(); frames * bins];
        let mut scratch = self.forward.make_scratch_vec();
        let mut time = vec![0.0f32; self.n_fft];

        for frame in 0..frames {
            let start = frame as isize * self.hop as isize - pad as isize;
            for (i, slot) in time.iter_mut().enumerate() {
                let idx = start + i as isize;
                let sample = if idx < 0 || idx as usize >= signal.len() {
                    0.0
                } else {
                    signal[idx as usize]
                };
                *slot = sample * self.window[i];
            }

            let out = &mut data[frame * bins..(frame + 1) * bins];
            self.forward
                .process_with_scratch(&mut time, out, &mut scratch)
                .expect("FFT input and output lengths are fixed by construction");
        }

        Spectrogram { frames, bins, data }
    }

    /// Synthesise a signal of exactly `len` samples from `spec`.
    pub fn inverse(&self, spec: &Spectrogram, len: usize) -> Vec<f32> {
        debug_assert_eq!(spec.bins, self.bins());
        let pad = self.n_fft / 2;
        let padded_len = len + 2 * pad;

        let mut acc = vec![0.0f32; padded_len + self.n_fft];
        let mut envelope = vec![0.0f32; padded_len + self.n_fft];
        let mut scratch = self.inverse.make_scratch_vec();
        let mut time = vec![0.0f32; self.n_fft];
        let mut bins = vec![Complex32::default(); self.bins()];

        for frame in 0..spec.frames {
            bins.copy_from_slice(spec.frame(frame));
            // realfft requires the DC and Nyquist bins to be purely real;
            // masking can leave a tiny imaginary residue, so clear it.
            bins[0].im = 0.0;
            if let Some(last) = bins.last_mut() {
                last.im = 0.0;
            }
            self.inverse
                .process_with_scratch(&mut bins, &mut time, &mut scratch)
                .expect("FFT input and output lengths are fixed by construction");

            let start = frame * self.hop;
            let scale = 1.0 / self.n_fft as f32;
            for i in 0..self.n_fft {
                let w = self.window[i];
                acc[start + i] += time[i] * scale * w;
                envelope[start + i] += w * w;
            }
        }

        // Divide out the accumulated window energy. Away from the edges this is
        // a constant; near them it tapers, and dividing by the true envelope is
        // what makes reconstruction exact all the way to the first and last
        // sample.
        acc.iter()
            .zip(&envelope)
            .skip(pad)
            .take(len)
            .map(|(&v, &e)| if e > 1e-8 { v / e } else { 0.0 })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_signal(len: usize) -> Vec<f32> {
        (0..len)
            .map(|i| {
                let t = i as f32 / 44_100.0;
                0.4 * (2.0 * std::f32::consts::PI * 440.0 * t).sin()
                    + 0.2 * (2.0 * std::f32::consts::PI * 1_310.0 * t).sin()
            })
            .collect()
    }

    #[test]
    fn round_trip_is_exact() {
        let signal = test_signal(10_000);
        let stft = Stft::new(1024, 256);
        let out = stft.inverse(&stft.forward(&signal), signal.len());

        assert_eq!(out.len(), signal.len());
        for (i, (&a, &b)) in signal.iter().zip(&out).enumerate() {
            assert!((a - b).abs() < 1e-4, "sample {i}: {a} vs {b}");
        }
    }

    #[test]
    fn round_trip_is_exact_at_the_edges() {
        // A ramp makes edge errors obvious: the first and last samples are the
        // extremes, so any window taper that is not divided out shows up here.
        let signal: Vec<f32> = (0..5_000).map(|i| i as f32 / 5_000.0 - 0.5).collect();
        let stft = Stft::new(2048, 512);
        let out = stft.inverse(&stft.forward(&signal), signal.len());

        for i in [0, 1, 2, 2_500, 4_997, 4_998, 4_999] {
            assert!((signal[i] - out[i]).abs() < 1e-4, "sample {i}: {} vs {}", signal[i], out[i]);
        }
    }

    #[test]
    fn complementary_masks_sum_to_the_original() {
        // The separator relies on this: mask and (1 - mask) must reconstruct
        // to the input when added back together.
        let signal = test_signal(8_000);
        let stft = Stft::new(1024, 256);
        let spec = stft.forward(&signal);

        let mask: Vec<f32> = (0..spec.data.len()).map(|i| (i % 7) as f32 / 7.0).collect();
        let complement: Vec<f32> = mask.iter().map(|m| 1.0 - m).collect();

        let a = stft.inverse(&spec.apply_mask(&mask), signal.len());
        let b = stft.inverse(&spec.apply_mask(&complement), signal.len());

        for (i, &orig) in signal.iter().enumerate() {
            assert!((a[i] + b[i] - orig).abs() < 1e-4, "sample {i}");
        }
    }

    #[test]
    fn handles_signals_shorter_than_one_window() {
        let signal = test_signal(100);
        let stft = Stft::new(1024, 256);
        let out = stft.inverse(&stft.forward(&signal), signal.len());
        assert_eq!(out.len(), 100);
        for (&a, &b) in signal.iter().zip(&out) {
            assert!((a - b).abs() < 1e-4);
        }
    }
}
