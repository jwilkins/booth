//! Median filtering of spectrograms, the primitive behind harmonic/percussive
//! separation.
//!
//! Filtering a magnitude spectrogram along time smears out brief broadband
//! events and leaves sustained tones — an estimate of the harmonic content.
//! Filtering along frequency does the reverse and estimates the percussive
//! content. (Fitzgerald, "Harmonic/Percussive Separation using Median
//! Filtering", DAFx-10.)

use rayon::prelude::*;

/// Round `n` up to the nearest odd number, with a floor of 1.
pub fn odd_kernel(n: usize) -> usize {
    if n <= 1 {
        1
    } else if n % 2 == 0 {
        n + 1
    } else {
        n
    }
}

/// Median filter each bin's trajectory across time.
pub fn median_along_time(mag: &[f32], frames: usize, bins: usize, kernel: usize) -> Vec<f32> {
    debug_assert_eq!(mag.len(), frames * bins);
    if frames == 0 || bins == 0 {
        return Vec::new();
    }

    let columns: Vec<Vec<f32>> = (0..bins)
        .into_par_iter()
        .map(|bin| {
            let column: Vec<f32> = (0..frames).map(|frame| mag[frame * bins + bin]).collect();
            median_1d(&column, kernel)
        })
        .collect();

    let mut out = vec![0.0f32; frames * bins];
    for (bin, column) in columns.iter().enumerate() {
        for (frame, &value) in column.iter().enumerate() {
            out[frame * bins + bin] = value;
        }
    }
    out
}

/// Median filter each frame's spectrum across frequency.
pub fn median_along_frequency(mag: &[f32], frames: usize, bins: usize, kernel: usize) -> Vec<f32> {
    debug_assert_eq!(mag.len(), frames * bins);
    if frames == 0 || bins == 0 {
        return Vec::new();
    }

    let mut out = vec![0.0f32; frames * bins];
    out.par_chunks_mut(bins)
        .zip(mag.par_chunks(bins))
        .for_each(|(dst, src)| dst.copy_from_slice(&median_1d(src, kernel)));
    out
}

/// One-dimensional running median with edge samples replicated.
fn median_1d(x: &[f32], kernel: usize) -> Vec<f32> {
    let kernel = odd_kernel(kernel).min(if x.is_empty() { 1 } else { x.len() * 2 + 1 });
    if x.is_empty() {
        return Vec::new();
    }
    if kernel == 1 {
        return x.to_vec();
    }

    let half = kernel / 2;
    let last = x.len() - 1;
    let mut window = vec![0.0f32; kernel];
    let mut out = Vec::with_capacity(x.len());

    for i in 0..x.len() {
        for (k, slot) in window.iter_mut().enumerate() {
            *slot = x[(i + k).saturating_sub(half).min(last)];
        }
        let (_, median, _) = window.select_nth_unstable_by(half, f32::total_cmp);
        out.push(*median);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounds_kernels_up_to_odd() {
        assert_eq!(odd_kernel(0), 1);
        assert_eq!(odd_kernel(1), 1);
        assert_eq!(odd_kernel(4), 5);
        assert_eq!(odd_kernel(7), 7);
    }

    #[test]
    fn removes_isolated_spikes() {
        let x = vec![1.0, 1.0, 9.0, 1.0, 1.0];
        assert_eq!(median_1d(&x, 3), vec![1.0, 1.0, 1.0, 1.0, 1.0]);
    }

    #[test]
    fn preserves_edges_by_replication() {
        let x = vec![5.0, 1.0, 1.0, 1.0];
        // The leading 5 is a lone spike even after replication, so it goes.
        assert_eq!(median_1d(&x, 3), vec![5.0, 1.0, 1.0, 1.0]);
    }

    #[test]
    fn kernel_of_one_is_identity() {
        let x = vec![3.0, 1.0, 4.0];
        assert_eq!(median_1d(&x, 1), x);
    }

    #[test]
    fn time_axis_keeps_sustained_rows() {
        // Two frames, three bins. Bin 1 is sustained; frame 1 is a transient.
        let frames = 5;
        let bins = 3;
        let mut mag = vec![0.0f32; frames * bins];
        for f in 0..frames {
            mag[f * bins + 1] = 1.0; // horizontal ridge
        }
        for b in 0..bins {
            mag[2 * bins + b] = 1.0; // vertical ridge
        }

        let at = |frame: usize, bin: usize| frame * bins + bin;

        let harmonic = median_along_time(&mag, frames, bins, 3);
        // The sustained ridge survives a median along time...
        assert_eq!(harmonic[at(0, 1)], 1.0);
        // ...while the one-frame transient in bin 0 does not.
        assert_eq!(harmonic[at(2, 0)], 0.0);

        let percussive = median_along_frequency(&mag, frames, bins, 3);
        // The broadband frame survives a median along frequency...
        assert_eq!(percussive[at(2, 1)], 1.0);
        // ...while the single-bin ridge in a quiet frame does not.
        assert_eq!(percussive[at(0, 1)], 0.0);
    }

    #[test]
    fn handles_kernels_wider_than_the_signal() {
        let x = vec![1.0, 2.0];
        assert_eq!(median_1d(&x, 99).len(), 2);
    }
}
