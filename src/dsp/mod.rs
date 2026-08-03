//! Signal-processing primitives shared by the normalizer and the separator.

pub mod median;
pub mod stft;

/// Convert a linear amplitude ratio to decibels.
///
/// Returns `-inf` for zero, which is what the loudness reporting expects for
/// digital silence.
pub fn linear_to_db(linear: f64) -> f64 {
    if linear <= 0.0 {
        f64::NEG_INFINITY
    } else {
        20.0 * linear.log10()
    }
}

/// Convert decibels to a linear amplitude ratio.
pub fn db_to_linear(db: f64) -> f64 {
    10f64.powf(db / 20.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn db_conversions_round_trip() {
        for db in [-60.0, -6.0, 0.0, 3.0] {
            assert!((linear_to_db(db_to_linear(db)) - db).abs() < 1e-9);
        }
    }

    #[test]
    fn known_db_values() {
        assert!((db_to_linear(-6.0206) - 0.5).abs() < 1e-4);
        assert!((linear_to_db(1.0)).abs() < 1e-12);
        assert_eq!(linear_to_db(0.0), f64::NEG_INFINITY);
    }
}
