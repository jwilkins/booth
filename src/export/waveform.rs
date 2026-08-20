//! Turning decoded audio into the waveform pictures a player draws.
//!
//! There are seven of them, in three resolutions and three eras of hardware,
//! but they are all the same measurement underneath: how loud the track is in
//! each of a few frequency bands, column by column. So the work is done once,
//! at the finest resolution anyone asks for, and everything else is derived
//! from it.
//!
//! The scrolling waveforms are quoted in *half-frames*: audio CDs run at 75
//! frames a second, so a player expects 150 columns per second of audio, and
//! that fixes the detail resolution. The previews are a fixed number of columns
//! for the whole track however long it is, which is why a nine-minute track
//! gets a coarser preview than a three-minute one.
//!
//! What the exact heights should be is not documented — the format notes say
//! only that "there is some scaling involved", and that nobody has yet matched
//! rekordbox exactly. The curve below is a square root, which keeps quiet
//! passages visible without flattening loud ones, and it is one of the things
//! to compare against a rekordbox-produced reference once there is hardware to
//! check against.

use crate::audio::Audio;

/// Scrolling waveform columns per second of audio: 75 frames, two columns each.
const DETAIL_PER_SECOND: usize = 150;
/// Columns in the original monochrome preview.
const PREVIEW_COLUMNS: usize = 400;
/// Columns in the CDJ-900's smaller preview.
const TINY_COLUMNS: usize = 100;
/// Columns in the colour and three-band previews.
const WIDE_COLUMNS: usize = 1_200;

/// Where the bands are split. A single-pole filter either side of each corner
/// is a gentle slope, but these pictures are 31 pixels tall and three bytes
/// wide — precision in the crossover is not what makes them readable.
const LOW_CORNER_HZ: f32 = 200.0;
const HIGH_CORNER_HZ: f32 = 2_000.0;

/// Every waveform a player might ask for, in the exact byte layout its section
/// expects. See [`crate::export::anlz`] for which file each one belongs in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WaveformData {
    /// `PWAV`: 400 bytes, five bits of height and three of whiteness.
    pub preview: Vec<u8>,
    /// `PWV2`: 100 bytes, four bits of height.
    pub tiny: Vec<u8>,
    /// `PWV3`: one byte per half-frame, encoded like `preview`.
    pub detail: Vec<u8>,
    /// `PWV4`: 1,200 columns of six bytes.
    pub color_preview: Vec<u8>,
    /// `PWV5`: two bytes per half-frame — three bits each of red, green and
    /// blue, then five of height.
    pub color_detail: Vec<u8>,
    /// `PWV6`: 1,200 columns of three bytes — mid, high, low.
    pub band_preview: Vec<u8>,
    /// `PWV7`: three bytes per half-frame, same order.
    pub band_detail: Vec<u8>,
}

impl WaveformData {
    /// The waveforms of a silent track of the given length. Useful for tests,
    /// and for the case where a file decodes to nothing at all.
    pub fn silent(duration_secs: f64) -> Self {
        Self::from_columns(&vec![Column::default(); detail_columns(duration_secs)])
    }

    /// How many half-frame columns the scrolling waveforms hold.
    pub fn detail_len(&self) -> usize {
        self.detail.len()
    }

    fn from_columns(columns: &[Column]) -> Self {
        let preview_columns = summarise(columns, PREVIEW_COLUMNS);
        let tiny_columns = summarise(columns, TINY_COLUMNS);
        let wide_columns = summarise(columns, WIDE_COLUMNS);

        Self {
            preview: preview_columns.iter().map(Column::mono_byte).collect(),
            tiny: tiny_columns.iter().map(Column::tiny_byte).collect(),
            detail: columns.iter().map(Column::mono_byte).collect(),
            color_preview: wide_columns.iter().flat_map(Column::color_preview_bytes).collect(),
            color_detail: columns.iter().flat_map(Column::color_detail_bytes).collect(),
            band_preview: wide_columns.iter().flat_map(Column::band_bytes).collect(),
            band_detail: columns.iter().flat_map(Column::band_bytes).collect(),
        }
    }
}

/// Measure a decoded track.
///
/// One pass over the samples, filtering as it goes and keeping the peak of each
/// band within each column. Memory does not grow with the length of the track
/// beyond the columns themselves — a ten-minute track costs about 360 kB of
/// measurements, not a second copy of the audio.
pub fn analyze(audio: &Audio) -> WaveformData {
    let columns = measure(audio);
    WaveformData::from_columns(&columns)
}

fn detail_columns(duration_secs: f64) -> usize {
    ((duration_secs * DETAIL_PER_SECOND as f64).round() as usize).max(1)
}

/// The peak amplitude of each band within one column of the picture.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
struct Column {
    full: f32,
    low: f32,
    mid: f32,
    high: f32,
}

impl Column {
    fn merge(&mut self, other: &Column) {
        self.full = self.full.max(other.full);
        self.low = self.low.max(other.low);
        self.mid = self.mid.max(other.mid);
        self.high = self.high.max(other.high);
    }

    /// Five bits of height, three of whiteness — the encoding shared by the
    /// original preview and the scrolling monochrome waveform.
    fn mono_byte(&self) -> u8 {
        (self.whiteness() << 5) | height(self.full, 31)
    }

    fn tiny_byte(&self) -> u8 {
        height(self.full, 15)
    }

    /// Higher values are drawn in a whiter, less saturated blue. Treble is what
    /// whitens a column, so the ratio of the high band to the whole is the
    /// closest honest measurement we have of it.
    fn whiteness(&self) -> u8 {
        let share = self.high / (self.full + f32::EPSILON);
        (share * 7.0).round().clamp(0.0, 7.0) as u8
    }

    /// Three bits each of red, green and blue, then five of height, packed
    /// big-endian into two bytes with the low two bits unused.
    fn color_detail_bytes(&self) -> [u8; 2] {
        let (r, g, b) = self.color();
        let packed = ((r as u16) << 13)
            | ((g as u16) << 10)
            | ((b as u16) << 7)
            | ((height(self.full, 31) as u16) << 2);
        packed.to_be_bytes()
    }

    /// Six bytes: two of whiteness, then the energy in the bottom half of the
    /// range, then the low, mid and high bands.
    fn color_preview_bytes(&self) -> [u8; 6] {
        let white = self.whiteness() * 36; // 0-7 spread across a byte
        [
            white,
            white,
            level(self.low.max(self.mid)),
            level(self.low),
            level(self.mid),
            level(self.high),
        ]
    }

    /// Three bytes, in the order the player wants them: mid, high, low. Drawn
    /// as amber, white and dark blue.
    fn band_bytes(&self) -> [u8; 3] {
        [level(self.mid), level(self.high), level(self.low)]
    }

    /// The hue of a column, as three-bit components. Bass reads blue, the
    /// mid-range reads amber, and treble washes everything towards white,
    /// which is the palette a player draws in.
    fn color(&self) -> (u8, u8, u8) {
        let total = self.low + self.mid + self.high + f32::EPSILON;
        let (low, mid, high) = (self.low / total, self.mid / total, self.high / total);
        let bit = |v: f32| (v * 7.0).round().clamp(0.0, 7.0) as u8;
        (bit(mid + high), bit(mid * 0.7 + high), bit(low + high))
    }
}

/// Amplitude to an `n`-step height. The square root keeps a −20 dB passage
/// visible rather than collapsing it onto the baseline.
fn height(amplitude: f32, max: u8) -> u8 {
    (amplitude.max(0.0).sqrt() * max as f32).round().clamp(0.0, max as f32) as u8
}

fn level(amplitude: f32) -> u8 {
    (amplitude.max(0.0).sqrt() * 255.0).round().clamp(0.0, 255.0) as u8
}

/// A single-pole low-pass, kept as its own state so the whole track can be
/// filtered in one streaming pass.
struct OnePole {
    coefficient: f32,
    state: f32,
}

impl OnePole {
    fn new(corner_hz: f32, sample_rate: u32) -> Self {
        let coefficient = (-2.0 * std::f32::consts::PI * corner_hz / sample_rate as f32).exp();
        Self { coefficient, state: 0.0 }
    }

    fn next(&mut self, input: f32) -> f32 {
        self.state = input * (1.0 - self.coefficient) + self.state * self.coefficient;
        self.state
    }
}

fn measure(audio: &Audio) -> Vec<Column> {
    let mut columns = vec![Column::default(); detail_columns(audio.duration_secs())];
    if audio.is_empty() {
        return columns;
    }

    let channels = audio.channels() as f32;
    let frames = audio.frames();
    let mut low_pass = OnePole::new(LOW_CORNER_HZ, audio.sample_rate);
    let mut low_mid_pass = OnePole::new(HIGH_CORNER_HZ, audio.sample_rate);
    let last = columns.len() - 1;

    for i in 0..frames {
        let mono: f32 = audio.planes.iter().map(|p| p[i]).sum::<f32>() / channels;
        // Three complementary bands that add back up to the input, so no energy
        // is counted twice or lost between them.
        let low = low_pass.next(mono);
        let low_mid = low_mid_pass.next(mono);
        let mid = low_mid - low;
        let high = mono - low_mid;

        let column = (i * DETAIL_PER_SECOND / audio.sample_rate as usize).min(last);
        let c = &mut columns[column];
        c.full = c.full.max(mono.abs());
        c.low = c.low.max(low.abs());
        c.mid = c.mid.max(mid.abs());
        c.high = c.high.max(high.abs());
    }

    columns
}

/// Squeeze the detail columns down to a fixed-width preview by taking the peak
/// of each span. Peak rather than average, because a preview whose job is to
/// show you where the drops are should not smooth them away.
fn summarise(columns: &[Column], width: usize) -> Vec<Column> {
    let mut out = vec![Column::default(); width];
    if columns.is_empty() {
        return out;
    }
    for (i, target) in out.iter_mut().enumerate() {
        let from = i * columns.len() / width;
        let to = ((i + 1) * columns.len() / width).max(from + 1).min(columns.len());
        for column in &columns[from..to] {
            target.merge(column);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(hz: f32, secs: f32, amplitude: f32) -> Audio {
        let rate = 44_100;
        let frames = (rate as f32 * secs) as usize;
        let plane: Vec<f32> = (0..frames)
            .map(|i| {
                let t = i as f32 / rate as f32;
                (2.0 * std::f32::consts::PI * hz * t).sin() * amplitude
            })
            .collect();
        Audio::new(rate, vec![plane.clone(), plane]).unwrap()
    }

    #[test]
    fn every_waveform_has_the_length_its_section_requires() {
        let w = analyze(&tone(1_000.0, 2.0, 0.5));
        assert_eq!(w.preview.len(), PREVIEW_COLUMNS);
        assert_eq!(w.tiny.len(), TINY_COLUMNS);
        assert_eq!(w.color_preview.len(), WIDE_COLUMNS * 6);
        assert_eq!(w.band_preview.len(), WIDE_COLUMNS * 3);
        // Two seconds at 150 columns a second.
        assert_eq!(w.detail.len(), 300);
        assert_eq!(w.color_detail.len(), 300 * 2);
        assert_eq!(w.band_detail.len(), 300 * 3);
    }

    #[test]
    fn silence_draws_nothing() {
        let silent = Audio::new(44_100, vec![vec![0.0; 44_100]]).unwrap();
        let w = analyze(&silent);
        assert!(w.preview.iter().all(|&b| b == 0));
        assert!(w.band_detail.iter().all(|&b| b == 0));
        assert_eq!(w, WaveformData::silent(1.0));
    }

    #[test]
    fn a_loud_tone_is_taller_than_a_quiet_one() {
        let loud = analyze(&tone(1_000.0, 1.0, 0.9));
        let quiet = analyze(&tone(1_000.0, 1.0, 0.1));
        let tallest = |w: &WaveformData| w.preview.iter().map(|b| b & 0x1f).max().unwrap();
        assert!(
            tallest(&loud) > tallest(&quiet),
            "loud {} quiet {}",
            tallest(&loud),
            tallest(&quiet)
        );
    }

    #[test]
    fn the_bands_follow_the_pitch() {
        // Byte order within a three-band entry is mid, high, low.
        let band = |hz: f32| {
            let w = analyze(&tone(hz, 1.0, 0.8));
            let at = w.band_detail.len() / 2 / 3 * 3; // a column from the middle
            (w.band_detail[at], w.band_detail[at + 1], w.band_detail[at + 2])
        };

        let (mid, high, low) = band(50.0);
        assert!(low > mid && low > high, "50 Hz should be mostly low: {low} {mid} {high}");

        let (mid, high, low) = band(800.0);
        assert!(mid > low && mid > high, "800 Hz should be mostly mid: {low} {mid} {high}");

        let (mid, high, low) = band(10_000.0);
        assert!(high > low && high > mid, "10 kHz should be mostly high: {low} {mid} {high}");
    }

    #[test]
    fn treble_whitens_a_column() {
        let whiteness = |hz: f32| {
            let w = analyze(&tone(hz, 1.0, 0.8));
            w.detail[w.detail.len() / 2] >> 5
        };
        assert!(whiteness(10_000.0) > whiteness(50.0));
    }

    #[test]
    fn a_full_scale_column_fills_the_height_without_spilling_into_the_colour() {
        // Bass at full scale: the tallest a column can be, and almost no
        // whiteness, so the whole byte should be the height and nothing else.
        // An unclamped scaling curve would carry into the top three bits here
        // and paint a loud track white.
        let w = analyze(&tone(50.0, 1.0, 1.0));
        let middle = w.detail[w.detail.len() / 2];
        assert_eq!(middle & 0x1f, 31, "a full-scale column should be full height");
        assert_eq!(middle >> 5, 0, "bass should not be white");
        assert_eq!(middle, 31);
    }

    #[test]
    fn the_preview_summarises_the_whole_track_however_long_it_is() {
        for secs in [0.5, 5.0, 60.0] {
            let w = analyze(&tone(1_000.0, secs, 0.7));
            assert_eq!(w.preview.len(), PREVIEW_COLUMNS);
            assert!(w.preview.iter().all(|&b| b & 0x1f > 0), "{secs}s left empty columns");
        }
    }

    #[test]
    fn a_track_shorter_than_one_column_still_produces_a_waveform() {
        let short = Audio::new(44_100, vec![vec![0.5; 100]]).unwrap();
        let w = analyze(&short);
        assert_eq!(w.detail.len(), 1);
        assert_eq!(w.preview.len(), PREVIEW_COLUMNS);
    }
}
