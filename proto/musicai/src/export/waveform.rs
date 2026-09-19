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
//! rekordbox exactly. What is here is a per-track gain and a shaping curve; see
//! [`reference_gain`] and [`SHAPE`] for what each is for. It is one of the
//! things to compare against a rekordbox-produced reference once there is
//! hardware to check against.

use crate::audio::Audio;

/// Scrolling waveform columns per second of audio: 75 frames, two columns each.
const DETAIL_PER_SECOND: usize = 150;
/// Columns in the original monochrome preview.
const PREVIEW_COLUMNS: usize = 400;
/// Columns in the CDJ-900's smaller preview.
const TINY_COLUMNS: usize = 100;
/// Columns in the colour and three-band previews.
const WIDE_COLUMNS: usize = 1_200;

/// Where the bands are split.
///
/// These are picture-making corners rather than mixing ones. The upper one sits
/// at 4 kHz, well above where a crossover would go in a filter you could hear,
/// because the point is a legible colour: everything from a bassline to a vocal
/// then lands in one band and reads as one colour, where a 2 kHz corner split
/// the middle of the music between two bands and drew it in a washed-out
/// yellow-white. Below 200 Hz is the red channel, 200 Hz to 4 kHz the green,
/// and what is left the blue.
const LOW_CORNER_HZ: f32 = 200.0;
const HIGH_CORNER_HZ: f32 = 4_000.0;

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
        // The scrolling pictures and the whole-track ones are scaled apart,
        // because they are two different measurements and normalising both
        // against one of them ruins the other. See [`Scaled`].
        let detail = Scaled::of(columns, REFERENCE_QUANTILE);

        let wide = summarise(columns, WIDE_COLUMNS);
        // One reference across all three overviews, taken from the finest of
        // them, so the small picture in a browse list and the strip under the
        // deck agree about how tall the same moment is.
        let over =
            |width: usize| Scaled::like(&summarise(columns, width), &wide, OVERVIEW_QUANTILE);
        let preview = over(PREVIEW_COLUMNS);
        let tiny = over(TINY_COLUMNS);
        let wide = Scaled::of(&wide, OVERVIEW_QUANTILE);

        Self {
            preview: preview.mono_bytes(),
            tiny: tiny.tiny_bytes(),
            detail: detail.mono_bytes(),
            color_preview: wide.color_preview_bytes(),
            color_detail: detail.color_detail_bytes(),
            band_preview: wide.band_bytes(),
            band_detail: detail.band_bytes(),
        }
    }
}

/// Measure a decoded track.
///
/// One pass over the samples, filtering as it goes and keeping the peak of each
/// band within each column. Memory does not grow with the length of the track
/// beyond the columns themselves — a ten-minute track costs about 360 kB of
/// measurements, not a second copy of the audio.
///
/// Then the whole track is scaled against its own loudest content, before any
/// of the pictures are encoded, so every picture derived from it agrees.
pub fn analyze(audio: &Audio) -> WaveformData {
    WaveformData::from_columns(&measure(audio))
}

/// Columns scaled and ready to encode.
///
/// Two scales, because a column carries two quantities that do not live on the
/// same one. How *tall* the column is drawn is [`loudness`], a weighted blend
/// of the bands that sits below any of them. What the column is *made of* is
/// the bands themselves. One gain cannot serve both: set from the blend it
/// drives the band bytes past the top of their range — a ten-kilohertz tone
/// came out with a full mid band as well as a full high one — and set from the
/// bands it leaves the height using a third of the display.
///
/// The colours are ratios between bands and so do not care which gain was
/// applied; they are taken from the band-scaled columns because that is where
/// they sit.
struct Scaled {
    bands: Vec<Column>,
    /// Normalised heights, nought to one, one per column.
    heights: Vec<f32>,
}

impl Scaled {
    fn of(columns: &[Column], quantile: f32) -> Self {
        Self::like(columns, columns, quantile)
    }

    /// Scale `columns` against the references taken from `against`, so that
    /// pictures of different widths can be put on one scale.
    fn like(columns: &[Column], against: &[Column], quantile: f32) -> Self {
        let band = gain_for(against, quantile, |c| c.low.max(c.mid).max(c.high));
        let tall = gain_for(against, quantile, Column::loudness);
        Self {
            bands: scaled(columns, band),
            heights: columns.iter().map(|c| c.loudness() * tall).collect(),
        }
    }

    fn mono_bytes(&self) -> Vec<u8> {
        self.pairs().map(|(c, h)| c.mono_byte(h)).collect()
    }

    fn tiny_bytes(&self) -> Vec<u8> {
        self.heights.iter().map(|h| height(*h, 15)).collect()
    }

    fn band_bytes(&self) -> Vec<u8> {
        self.bands.iter().flat_map(Column::band_bytes).collect()
    }

    fn color_detail_bytes(&self) -> Vec<u8> {
        self.pairs().flat_map(|(c, h)| c.color_detail_bytes(h)).collect()
    }

    /// The browse-list picture is made of band levels only — it carries no
    /// height of its own — so it comes from the band-scaled columns alone.
    fn color_preview_bytes(&self) -> Vec<u8> {
        self.bands.iter().flat_map(Column::color_preview_bytes).collect()
    }

    fn pairs(&self) -> impl Iterator<Item = (&Column, f32)> {
        self.bands.iter().zip(self.heights.iter().copied())
    }
}

/// A copy of the columns with one gain applied to all of them.
fn scaled(columns: &[Column], gain: f32) -> Vec<Column> {
    let mut out = columns.to_vec();
    for column in &mut out {
        column.scale(gain);
    }
    out
}

/// The fraction of the track that is allowed to reach full height.
///
/// The maximum is not quite the right reference: a single freak column — one
/// clap that clips on an otherwise quiet record — would set it, and the whole
/// picture would then be drawn against a moment nobody is reading the waveform
/// to find. A quantile just short of the top throws that column away and keeps
/// everything else.
///
/// Just short of it, though, and not the loudest twentieth. This was 0.95,
/// from when the height was the peak of the whole signal — but on a limited
/// master half the track sits within a hair of that peak, so the 95th
/// percentile falls *inside* the loudest passage and draws its median at 29 of
/// 31 with the kicks clipped off above it. Measured on one, moving the
/// reference to 0.99 doubles the movement visible inside a loud section
/// (spread 0.048 to 0.094) and drops that median to 23, which is where a kick
/// has somewhere to go. One per cent of a six-minute track is still several
/// hundred columns discarded, which is a great deal more than a clap.
const REFERENCE_QUANTILE: f32 = 0.99;

/// The most a quiet track's picture is lifted, as a gain.
///
/// A quiet transfer should be drawn as though it were not, which is the whole
/// point of normalising. But a track that is mostly silence has a reference
/// level made of its own noise floor, and without a limit that noise is drawn
/// as a full-height block.
///
/// About forty decibels. The number has to be read against what it is capping:
/// [`loudness`] is a weighted blend of the bands and sits well below the peak
/// of the signal — for a tone in one band it is a third of it — so a cap set
/// as though it were a peak fires on legitimately quiet records rather than on
/// silence. What it works out to is a floor on the reference level: a track
/// whose loudest passage measures below about −42 dBFS is not lifted, and
/// there is nothing at that level anybody is trying to look at.
const MAX_GAIN: f32 = 128.0;

/// How tall a column is drawn, from the three bands it is made of.
///
/// Not the peak of the whole signal, which is what this used to be. A modern
/// master is limited, and limiting is precisely the business of making the peak
/// the same from moment to moment: measured column by column across one loud
/// section of one, the peak of the whole signal has a spread of 0.00. It is a
/// straight line, and no amount of scaling or curve-bending recovers a shape
/// from a straight line — which is why a loud track drew as a solid block
/// however its gain was worked out.
///
/// The bands are not all flat. Through the same section the mid band, the high
/// band and the overall RMS sit at 0.04 to 0.07, but the low band is at 0.36:
/// on a limited record the kick is the one thing still moving, and it is what
/// a waveform is read for.
///
/// So the low band leads and the other two carry the level. Weighted this way
/// a loud section keeps a visible pulse — 0.15 against 0.00 — while a
/// breakdown that takes the drums out but keeps a loud pad still draws at
/// about a third of full height rather than the tenth that the low band alone
/// would give it. Leading with the low band is not a distortion: a passage
/// with the drums out *should* look thinner than one with them in.
pub fn loudness(low: f32, mid: f32, high: f32) -> f32 {
    LOW_WEIGHT * low + MID_WEIGHT * mid + HIGH_WEIGHT * high
}

const LOW_WEIGHT: f32 = 0.6;
const MID_WEIGHT: f32 = 0.3;
const HIGH_WEIGHT: f32 = 0.1;

/// How the normalised amplitude is bent before it becomes a height.
///
/// Straight through, the quiet parts of a dynamic record are drawn so low as to
/// be a flat line; a square root — what this used to do, with no normalising in
/// front of it — lifts them so far that a loud record has no room left and its
/// drop, its build and its breakdown all draw at full height, which is a
/// picture with the arrangement taken out of it. Between the two, and nearer
/// the straight line: a −16 dB passage still draws at about a quarter height,
/// and the parts of a track that differ still look different.
const SHAPE: f32 = 0.7;

/// The quantile the whole-track pictures are referenced at.
///
/// The maximum, rather than the shade below it the scrolling pictures use. The
/// quantile is there to survive one freak column, and an overview column is a
/// second of audio averaged — which no single transient can carry, so the
/// outlier it guards against cannot occur here. Taking the maximum means the
/// loudest passage draws exactly at the top with nothing above it thrown away.
const OVERVIEW_QUANTILE: f32 = 1.0;

/// The gain that puts a track's reference level at full height.
///
/// One gain for every column and every band in it, rather than one per band or
/// per column: the height of a column is what says how loud that moment is,
/// and the ratios between the bands are what the colour is made of, so scaling
/// them apart would wreck both and leave a picture in which every moment looks
/// equally loud.
///
/// `measure` has to be the same number the picture is drawn from. Reference
/// one thing and draw another and the picture comes out hot by whatever the
/// ratio between them happens to be — which is how the scrolling waveform came
/// to be drawn at twice its height, from a gain worked out on an overview
/// whose columns are the *average* of a second of the ones it was applied to.
fn gain_for(columns: &[Column], quantile: f32, measure: fn(&Column) -> f32) -> f32 {
    let mut peaks: Vec<f32> = columns.iter().map(measure).filter(|a| *a > 0.0).collect();
    if peaks.is_empty() {
        return 1.0;
    }
    peaks.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let at = ((peaks.len() as f32 - 1.0) * quantile).round() as usize;
    let reference = peaks[at.min(peaks.len() - 1)];
    if reference <= f32::EPSILON {
        return 1.0;
    }
    (1.0 / reference).min(MAX_GAIN)
}

fn detail_columns(duration_secs: f64) -> usize {
    ((duration_secs * DETAIL_PER_SECOND as f64).round() as usize).max(1)
}

/// One column of the picture: how tall it is, and what it is made of.
///
/// Two measurements of each band, because they answer different questions. The
/// *peak* is how tall to draw the band, and a waveform drawn from anything else
/// stops looking like the track. The *average* is what the column is made of,
/// and it is what decides the colour: a kick drum's transient puts energy in
/// every band at once, so a column coloured by peaks is a column coloured
/// white, which is how this used to look on a player.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
struct Column {
    full: f32,
    low: f32,
    mid: f32,
    high: f32,
    /// Sums of squares while measuring, root-mean-square afterwards.
    low_rms: f32,
    mid_rms: f32,
    high_rms: f32,
    samples: f32,
}

impl Column {
    /// Scale every measurement by one gain, so nothing about the column's
    /// proportions changes — only how tall it is drawn.
    fn scale(&mut self, gain: f32) {
        for value in [
            &mut self.full,
            &mut self.low,
            &mut self.mid,
            &mut self.high,
            &mut self.low_rms,
            &mut self.mid_rms,
            &mut self.high_rms,
        ] {
            *value *= gain;
        }
    }

    /// Add another column's measurements into this one, on the way to an
    /// average of them.
    fn add(&mut self, other: &Column) {
        self.full += other.full;
        self.low += other.low;
        self.mid += other.mid;
        self.high += other.high;
        self.low_rms += other.low_rms;
        self.mid_rms += other.mid_rms;
        self.high_rms += other.high_rms;
    }

    /// Five bits of height, three of whiteness — the encoding shared by the
    /// original preview and the scrolling monochrome waveform.
    fn mono_byte(&self, tall: f32) -> u8 {
        (self.whiteness() << 5) | height(tall, 31)
    }

    /// How tall this column is drawn, before scaling. See [`loudness`].
    fn loudness(&self) -> f32 {
        loudness(self.low, self.mid, self.high)
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
    fn color_detail_bytes(&self, tall: f32) -> [u8; 2] {
        let (r, g, b) = self.color();
        let packed = ((r as u16) << 13)
            | ((g as u16) << 10)
            | ((b as u16) << 7)
            | ((height(tall, 31) as u16) << 2);
        packed.to_be_bytes()
    }

    /// Six bytes: two of whiteness, then the energy in the bottom half of the
    /// range, then the low, mid and high bands.
    ///
    /// The three band bytes are what the browse list draws its little pictures
    /// from, and a rekordbox-written drive draws them in strong oranges,
    /// greens, magentas and blues. Writing each band's own loudness there gives
    /// three large similar numbers for most music, which is a pale wash — so
    /// each band is scaled by how much of the column it accounts for. The
    /// loudest band keeps its full value, and the ones underneath it fall away,
    /// which is what leaves a colour rather than a grey.
    fn color_preview_bytes(&self) -> [u8; 6] {
        let white = self.whiteness() * 36; // 0-7 spread across a byte
        let (low, mid, high) = self.shares();
        [
            white,
            white,
            level(self.low.max(self.mid)),
            level(self.low * low),
            level(self.mid * mid),
            level(self.high * high),
        ]
    }

    /// How much of this column each band accounts for, against the loudest of
    /// them rather than against their sum.
    ///
    /// Against the sum the three always add to one, so the loudest band can
    /// never reach the top of its range and every column comes out a shade of
    /// grey. Against the peak, whatever dominates the column saturates —
    /// squared, so that a band a third as loud as the leader tints the colour
    /// instead of diluting it.
    ///
    /// From the averages rather than the peaks: a kick drum's transient is loud
    /// in every band at once, so peaks would call almost every column white.
    fn shares(&self) -> (f32, f32, f32) {
        let peak = self.low_rms.max(self.mid_rms).max(self.high_rms);
        if peak <= f32::EPSILON {
            return (0.0, 0.0, 0.0);
        }
        let share = |band: f32| (band / peak) * (band / peak);
        (share(self.low_rms), share(self.mid_rms), share(self.high_rms))
    }

    /// Three bytes, in the order the player wants them: mid, high, low. Drawn
    /// as amber, white and dark blue.
    fn band_bytes(&self) -> [u8; 3] {
        [level(self.mid), level(self.high), level(self.low)]
    }

    /// The hue of a column, as three-bit components: one band per channel.
    ///
    /// Red is the bass, green the mid-range, blue the treble — the convention
    /// every DJ program draws waveforms in, and the reason it is a convention
    /// is that it makes a column of two bands land on a secondary that names
    /// the pair. Bass and mid read yellow, mid and treble cyan, bass and treble
    /// magenta, and a column with all three reads white. Nothing cancels,
    /// because no two primaries sit opposite each other.
    ///
    /// This used to map the bands onto a palette of its own — bass blue,
    /// mid-range amber — which put a picture on the deck that no chart a DJ has
    /// ever read accounts for, and which disagreed with what Booth drew on
    /// screen besides.
    ///
    /// Measured against the **loudest** band rather than against the sum of
    /// them. Against the sum, the three shares add to one and the strongest
    /// band can never reach the top of its three bits: a bass-heavy column and
    /// a bright one come out different shades of the same grey, which is
    /// exactly what a CDJ-3000X drew from this before. Against the peak, the
    /// band that dominates the column saturates and the others fall away from
    /// it — and squaring the ratios sharpens that, because a band 30% as loud
    /// as the leader should tint the colour rather than dilute it.
    fn color(&self) -> (u8, u8, u8) {
        let (low, mid, high) = self.shares();
        let bit = |v: f32| (v * 7.0).round().clamp(0.0, 7.0) as u8;
        (bit(low), bit(mid), bit(high))
    }
}

/// Amplitude to an `n`-step height, once the track has been scaled against its
/// own reference level. See [`SHAPE`] for the curve and why it is not a square
/// root any more.
fn height(amplitude: f32, max: u8) -> u8 {
    let shaped = amplitude.clamp(0.0, 1.0).powf(SHAPE);
    (shaped * max as f32).round().clamp(0.0, max as f32) as u8
}

fn level(amplitude: f32) -> u8 {
    height(amplitude, 255)
}

/// Undo [`SHAPE`], turning a stored level back into the amplitude it was made
/// from.
///
/// The curve is there to make a picture readable, and it flattens the
/// differences between the bands on the way: a column that is plainly a kick
/// comes out of it reading 0.95 low against 0.71 mid. Anything working out
/// what a column is *made of* — a colour, a ratio — has to undo it first, or
/// it is reading proportions the display invented.
pub fn unshape(level: f32) -> f32 {
    level.clamp(0.0, 1.0).powf(1.0 / SHAPE)
}

/// A low-pass of two single poles in series, kept as its own state so the whole
/// track can be filtered in one streaming pass.
///
/// Two poles rather than one because one is a 6 dB per octave slope, and a
/// slope that gentle leaves a bass note plainly audible in the treble band an
/// octave and a half up. The bands are still exactly complementary — they are
/// differences of the same two filtered signals, whatever shape those have —
/// so nothing is double-counted or lost between them.
struct Slope {
    coefficient: f32,
    first: f32,
    second: f32,
}

impl Slope {
    fn new(corner_hz: f32, sample_rate: u32) -> Self {
        let coefficient = (-2.0 * std::f32::consts::PI * corner_hz / sample_rate as f32).exp();
        Self { coefficient, first: 0.0, second: 0.0 }
    }

    fn next(&mut self, input: f32) -> f32 {
        let open = 1.0 - self.coefficient;
        self.first = input * open + self.first * self.coefficient;
        self.second = self.first * open + self.second * self.coefficient;
        self.second
    }
}

fn measure(audio: &Audio) -> Vec<Column> {
    let mut columns = vec![Column::default(); detail_columns(audio.duration_secs())];
    if audio.is_empty() {
        return columns;
    }

    let channels = audio.channels() as f32;
    let frames = audio.frames();
    let mut low_pass = Slope::new(LOW_CORNER_HZ, audio.sample_rate);
    let mut low_mid_pass = Slope::new(HIGH_CORNER_HZ, audio.sample_rate);
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
        c.low_rms += low * low;
        c.mid_rms += mid * mid;
        c.high_rms += high * high;
        c.samples += 1.0;
    }

    // Sums of squares become the root-mean-square they were being accumulated
    // for, once it is known how many samples each column got.
    for column in &mut columns {
        let n = column.samples.max(1.0);
        column.low_rms = (column.low_rms / n).sqrt();
        column.mid_rms = (column.mid_rms / n).sqrt();
        column.high_rms = (column.high_rms / n).sqrt();
    }

    columns
}

/// Squeeze the detail columns down to a fixed-width overview, by averaging each
/// span rather than taking its peak.
///
/// The peak is right for the scrolling waveform, where a column is a
/// hundred-and-fiftieth of a second and a kick drum is several columns wide. It
/// is wrong here. A preview column of a six-minute track covers about a third
/// of a second, which is most of a beat, so whether it reaches the top comes
/// down to whether a transient happened to land inside it — and the answer
/// alternates. That draws a comb: a picture whose loudest feature is the
/// sampling, at a spacing that has nothing to do with the music.
///
/// It also hides the thing the overview is for. With every column catching some
/// transient, a breakdown and a drop both pin near the top and the arrangement
/// flattens out. Averaging measures how much is going on across the span
/// instead, which is what separates them: on a test track it cut the
/// column-to-column jitter sixfold and *widened* the gap between the quietest
/// section and the loudest.
fn summarise(columns: &[Column], width: usize) -> Vec<Column> {
    let mut out = vec![Column::default(); width];
    if columns.is_empty() {
        return out;
    }
    // Each column averages a window centred on it, and the window is wider
    // than the spacing between columns — so consecutive ones overlap and the
    // picture is an envelope rather than a series of separate samples of a
    // pulse. See `OVERVIEW_WINDOW`.
    let pitch = columns.len().div_ceil(width);
    let window = pitch.max(OVERVIEW_WINDOW).min(columns.len());
    for (i, target) in out.iter_mut().enumerate() {
        let centre = (i * columns.len() / width) + pitch / 2;
        let from = centre.saturating_sub(window / 2).min(columns.len() - 1);
        let to = (from + window).min(columns.len());
        for column in &columns[from..to] {
            target.add(column);
        }
        target.scale(1.0 / (to - from) as f32);
    }
    out
}

/// How many detail columns an overview column averages, at least.
///
/// One second's worth. At 1,200 columns a preview column of a six-minute track
/// covers a third of a second, which is less than a beat at any tempo anybody
/// plays — so windows that merely touch each other resolve individual kick
/// drums, and the picture is a comb whose spacing is the sampling rather than
/// the music. That is what an overview drawn this way looks like on a player,
/// and it buries the thing it is for.
///
/// A second spans a beat at every tempo, so what survives the averaging is how
/// much is going on, which is the arrangement. A drop's edge blurs across
/// three columns of twelve hundred, which is nothing to look at and the price
/// of the rest.
const OVERVIEW_WINDOW: usize = DETAIL_PER_SECOND;

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

    /// Music-shaped: a sustained bed with sharp percussive hits over it, so
    /// that the peak within any second sits well above the average of that
    /// second.
    ///
    /// The tones the other tests are built from do not do this — a steady sine
    /// averaged over a second is very nearly its own peak — which is exactly
    /// why they went on passing while the scrolling waveform was being drawn
    /// at more than twice the height it should have been.
    fn percussive(levels: &[f32], secs_each: f32) -> Audio {
        let rate = 44_100;
        let mut plane = Vec::new();
        for (n, level) in levels.iter().enumerate() {
            for i in 0..(rate as f32 * secs_each) as usize {
                let t = (n as f32 * secs_each) + i as f32 / rate as f32;
                let bed = (2.0 * std::f32::consts::PI * 220.0 * t).sin() * 0.12;
                // Two hits a second, each decaying over about 40 ms.
                let since = (t * 2.0).fract() / 2.0;
                let hit = (-since * 60.0).exp() * (2.0 * std::f32::consts::PI * 60.0 * t).sin();
                plane.push((bed + hit) * level);
            }
        }
        Audio::new(rate, vec![plane.clone(), plane]).unwrap()
    }

    fn heights(bytes: &[u8]) -> Vec<u8> {
        let mut out: Vec<u8> = bytes.iter().map(|b| b & 0x1f).collect();
        out.sort_unstable();
        out
    }

    #[test]
    fn the_scrolling_waveform_is_not_flattened_against_the_top() {
        // The fault this is here for. The gain was worked out on an overview,
        // where a column is a second of audio averaged, and then applied to
        // these columns, where a column is a peak — about twice too much on
        // anything with transients in it. Every hit pinned against the top and
        // the picture became a solid block.
        let w = analyze(&percussive(&[0.8], 20.0));
        let h = heights(&w.detail);
        let full = h.iter().filter(|&&x| x == 31).count();

        // The reference is a quantile, so a little clipping is the point —
        // but only a little.
        assert!(full * 10 <= h.len(), "{full} of {} columns are at full height", h.len());
        assert!(h[h.len() / 2] < 21, "the middle of the track draws at {} of 31", h[h.len() / 2]);
    }

    #[test]
    fn the_scrolling_waveform_tells_one_passage_from_another() {
        // What the picture is read for. A quarter of the level is about 12 dB
        // down and has to look plainly different, not a shade shorter.
        let levels = [1.0f32, 0.25];
        let w = analyze(&percussive(&levels, 10.0));
        let half = w.detail.len() / 2;
        let loud = heights(&w.detail[..half]);
        let quiet = heights(&w.detail[half..]);
        let (loud, quiet) = (loud[loud.len() / 2], quiet[quiet.len() / 2]);
        // As a ratio rather than a number of steps. On sparse material most
        // columns sit between the hits, so the middle of even a loud section
        // is low on the scale and the two medians are small numbers — what
        // has to hold is that one is plainly taller than the other, which is
        // the same claim wherever on the scale they land.
        assert!(loud >= quiet * 2 && loud - quiet >= 4, "loud {loud} quiet {quiet} of 31");
    }

    #[test]
    fn the_two_pictures_are_scaled_for_what_each_of_them_measures() {
        // A peak and a one-second average are different measurements of the
        // same track, and one gain cannot serve both: normalising against
        // either ruins the other. On material with transients the two
        // references genuinely differ, which is the whole reason for the
        // split — if they ever stop differing here, this test is no longer
        // watching anything.
        let columns = measure(&percussive(&[0.8], 20.0));
        let detail = gain_for(&columns, REFERENCE_QUANTILE, Column::loudness);
        let overview =
            gain_for(&summarise(&columns, WIDE_COLUMNS), OVERVIEW_QUANTILE, Column::loudness);
        assert!(
            overview > detail * 1.3,
            "peak gain {detail:.2} and average gain {overview:.2} are close enough that \
             one would have done"
        );
    }

    #[test]
    fn the_height_is_referenced_against_what_the_height_is_drawn_from() {
        // A column's height comes from the peak of the whole signal; the bands
        // are complementary, so the loudest of them is always the smaller
        // number. Referencing that made every picture about a quarter hotter
        // than it was meant to be.
        let columns = measure(&percussive(&[0.8], 10.0));
        let loudest_band =
            columns.iter().map(|c| c.low.max(c.mid).max(c.high)).fold(0.0f32, f32::max);
        let whole = columns.iter().map(|c| c.full).fold(0.0f32, f32::max);
        assert!(whole > loudest_band, "full {whole} band {loudest_band}");
    }

    /// Sections of a limited master, some with a kick and some without: a
    /// breakdown that drops the drums but keeps a loud pad is the case a
    /// low-weighted height can get badly wrong.
    fn mastered(sections: &[(f32, bool)], secs_each: f32) -> Audio {
        let rate = 44_100;
        let mut seed = 0x2545_F491_4F6C_DD1Du64;
        let mut noise = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed >> 40) as f32 / 8_388_608.0 - 1.0
        };
        let mut plane = Vec::new();
        for (n, (level, kicking)) in sections.iter().enumerate() {
            for i in 0..(rate as f32 * secs_each) as usize {
                let t = (n as f32 * secs_each) + i as f32 / rate as f32;
                let since = (t * 2.0).fract() / 2.0;
                let kick = match kicking {
                    true => (-since * 24.0).exp() * (2.0 * std::f32::consts::PI * 55.0 * t).sin(),
                    false => 0.0,
                };
                // A kickless section is a breakdown: pads and a vocal, with
                // essentially nothing below 200 Hz. That is the case a height
                // weighted towards the low band can draw as silence.
                let bed = match kicking {
                    true => noise() * 0.5 + (2.0 * std::f32::consts::PI * 220.0 * t).sin() * 0.35,
                    false => {
                        (2.0 * std::f32::consts::PI * 900.0 * t).sin() * 0.5
                            + (2.0 * std::f32::consts::PI * 3_000.0 * t).sin() * 0.35
                    }
                };
                plane.push((3.0 * (bed + kick * 0.8)).tanh() * level);
            }
        }
        Audio::new(rate, vec![plane.clone(), plane]).unwrap()
    }

    #[test]
    fn a_limited_master_still_shows_its_kick() {
        // The fault this is here for. A limiter's whole business is making the
        // peak the same from moment to moment, so on a modern master the peak
        // of the signal is a straight line — spread 0.00 measured across a
        // loud section — and a picture drawn from it is a solid block however
        // it is scaled. Leading the height with the low band puts the kick
        // back, because on such a record the low band is the only measurement
        // still moving.
        let w = analyze(&mastered(&[(1.0, true)], 12.0));
        let h: Vec<f32> = w.detail.iter().map(|b| (b & 0x1f) as f32).collect();
        let mean = h.iter().sum::<f32>() / h.len() as f32;
        let spread =
            (h.iter().map(|x| (x - mean).powi(2)).sum::<f32>() / h.len() as f32).sqrt() / mean;
        assert!(spread > 0.07, "the loud section is a flat block: spread {spread:.2}");

        // And it is not a block against the ceiling either.
        let pinned = h.iter().filter(|x| **x == 31.0).count();
        assert!(pinned * 5 < h.len(), "{pinned} of {} columns are at full height", h.len());
    }

    #[test]
    fn a_breakdown_without_drums_is_thinner_but_not_empty() {
        // The other side of the same bargain, and the way weighting the height
        // towards the low band can go badly wrong. A breakdown that takes the
        // drums out but keeps a loud pad has almost nothing below 200 Hz: the
        // low band alone would draw it at a tenth of full height, which reads
        // as silence in the middle of a record. It should look thinner than a
        // section with the drums in — that is the useful part — and still
        // plainly be there.
        let w = analyze(&mastered(&[(1.0, true), (0.6, false), (1.0, true)], 8.0));
        let third = w.detail.len() / 3;
        let median = |part: &[u8]| {
            let mut h: Vec<u8> = part.iter().map(|b| b & 0x1f).collect();
            h.sort_unstable();
            h[h.len() / 2]
        };
        let full = median(&w.detail[..third]);
        let broken = median(&w.detail[third..third * 2]);
        assert!(broken < full, "the breakdown is as tall as the drop: {broken} and {full}");
        assert!(broken >= 6, "the breakdown all but vanished: {broken} of 31");
    }

    #[test]
    fn the_bands_are_not_driven_past_the_top_of_their_range() {
        // The height is a weighted blend of the bands and sits below any one of
        // them, so a gain worked out from it and then applied to the bands
        // overdrives them: a ten-kilohertz tone came out with a full mid band
        // as well as a full high one, and every colour went with it. The two
        // are scaled apart for exactly this reason.
        for hz in [50.0f32, 800.0, 10_000.0] {
            let w = analyze(&tone(hz, 1.0, 0.8));
            let at = w.band_detail.len() / 2 / 3 * 3;
            let full = w.band_detail[at..at + 3].iter().filter(|b| **b == 255).count();
            assert!(full <= 1, "{hz} Hz saturated {full} bands: {:?}", &w.band_detail[at..at + 3]);
        }
    }

    #[test]
    fn silence_draws_nothing() {
        let silent = Audio::new(44_100, vec![vec![0.0; 44_100]]).unwrap();
        let w = analyze(&silent);
        assert!(w.preview.iter().all(|&b| b == 0));
        assert!(w.band_detail.iter().all(|&b| b == 0));
        assert_eq!(w, WaveformData::silent(1.0));
    }

    /// Two stretches of one track, so the comparison is the one the picture is
    /// read for: not how loud the record is, but which part of it is louder.
    fn loud_then_quiet() -> Audio {
        let loud = tone(1_000.0, 1.0, 0.9);
        let quiet = tone(1_000.0, 1.0, 0.1);
        let planes: Vec<Vec<f32>> = loud
            .planes
            .iter()
            .zip(&quiet.planes)
            .map(|(a, b)| a.iter().chain(b.iter()).copied().collect())
            .collect();
        Audio::new(loud.sample_rate, planes).unwrap()
    }

    #[test]
    fn a_loud_passage_is_taller_than_a_quiet_one() {
        let w = analyze(&loud_then_quiet());
        let half = w.preview.len() / 2;
        let tallest = |part: &[u8]| part.iter().map(|b| b & 0x1f).max().unwrap();
        assert!(
            tallest(&w.preview[..half]) > tallest(&w.preview[half..]),
            "loud {} quiet {}",
            tallest(&w.preview[..half]),
            tallest(&w.preview[half..])
        );
    }

    #[test]
    fn a_quiet_record_is_drawn_at_the_same_size_as_a_loud_one() {
        // The picture is scaled against the track's own loudest content, so it
        // says how a record is put together rather than how hot it was
        // mastered. Two takes of the same thing at different levels are the
        // same arrangement and draw the same.
        let quiet: Vec<Vec<f32>> = loud_then_quiet()
            .planes
            .iter()
            .map(|plane| plane.iter().map(|s| s * 0.05).collect())
            .collect();
        let quiet = analyze(&Audio::new(44_100, quiet).unwrap());
        let loud = analyze(&loud_then_quiet());

        let tallest = |w: &WaveformData| w.preview.iter().map(|b| b & 0x1f).max().unwrap();
        assert_eq!(tallest(&quiet), tallest(&loud), "a quiet transfer was drawn as a quiet track");
    }

    #[test]
    fn a_loud_record_still_shows_its_arrangement() {
        // The failure this is here for: with no normalising and a square-root
        // curve, everything on a modern master drew at full height, and a drop,
        // a build and a breakdown became the same picture.
        let w = analyze(&loud_then_quiet());
        let half = w.preview.len() / 2;
        let median = |part: &[u8]| {
            let mut heights: Vec<u8> = part.iter().map(|b| b & 0x1f).collect();
            heights.sort_unstable();
            heights[heights.len() / 2]
        };
        let (loud, quiet) = (median(&w.preview[..half]), median(&w.preview[half..]));
        // A passage 19 dB down should look plainly different, not a shade
        // shorter: at least a third of the height between them.
        assert!(loud as i32 - quiet as i32 >= 10, "loud {loud} quiet {quiet} of 31");
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

    /// The three-bit red, green and blue of the middle column of the scrolling
    /// colour waveform.
    fn hue(hz: f32) -> (u16, u16, u16) {
        let w = analyze(&tone(hz, 1.0, 0.8));
        let middle = (w.color_detail.len() / 4) * 2;
        let packed = u16::from_be_bytes([w.color_detail[middle], w.color_detail[middle + 1]]);
        ((packed >> 13) & 7, (packed >> 10) & 7, (packed >> 7) & 7)
    }

    /// The three band bytes of the middle column of the browse-list preview.
    fn preview_hue(hz: f32) -> (u8, u8, u8) {
        let w = analyze(&tone(hz, 1.0, 0.8));
        let at = (w.color_preview.len() / 12) * 6;
        (w.color_preview[at + 3], w.color_preview[at + 4], w.color_preview[at + 5])
    }

    #[test]
    fn the_browse_preview_is_a_colour_rather_than_a_wash() {
        // The picture beside each row in the browse list. A rekordbox drive
        // draws these in strong oranges, greens and blues; writing each band's
        // own loudness gave three large similar numbers and a pale wash, which
        // is what a player showed.
        let saturation = |(a, b, c): (u8, u8, u8)| {
            let (top, bottom) = (a.max(b).max(c) as f32, a.min(b).min(c) as f32);
            bottom / top.max(1.0)
        };

        let (low, mid, high) = preview_hue(60.0);
        assert!(low > mid && mid > high, "bass column reads {low},{mid},{high}");
        let (low, mid, high) = preview_hue(700.0);
        assert!(mid > low && mid > high, "a mid column reads {low},{mid},{high}");
        let (low, mid, high) = preview_hue(9_000.0);
        assert!(high > low && high > mid, "a treble column reads {low},{mid},{high}");

        for hz in [60.0, 700.0, 9_000.0] {
            let washed = saturation(preview_hue(hz));
            assert!(
                washed < 0.35,
                "{hz} Hz is a wash: the quietest band is {washed} of the loudest"
            );
        }
    }

    #[test]
    fn a_column_takes_the_colour_of_whatever_is_loudest_in_it() {
        // The complaint this answers is "it isn't as colourful as I'm used
        // to": a player drew every column of a real track in much the same
        // pale grey, because the shares were measured against their own sum
        // and so could never reach the top of three bits.
        // One band per channel, the way every other DJ program draws it: red
        // is the bass, green the mid-range, blue the treble.
        let (r, g, b) = hue(60.0);
        assert!(r > g && r > b, "bass should read red, not {r},{g},{b}");
        assert_eq!(r, 7, "and the loudest band in a column should saturate");

        let (r, g, b) = hue(1_000.0);
        assert!(g > r && g > b, "the mid-range should read green, not {r},{g},{b}");
        assert_eq!(g, 7);

        let (r, g, b) = hue(9_000.0);
        assert!(b > r && b > g, "treble should read blue, not {r},{g},{b}");
        assert_eq!(b, 7);
    }

    #[test]
    fn a_kick_does_not_paint_the_whole_track_white() {
        // A transient has energy in every band at once, so a colour taken from
        // peaks is white whatever the track is made of. The colour comes from
        // the average instead, and this is the case that tells the two apart.
        let rate = 44_100;
        let mut plane = vec![0.0f32; rate];
        for (i, sample) in plane.iter_mut().enumerate() {
            let t = i as f32 / rate as f32;
            let bass = (2.0 * std::f32::consts::PI * 55.0 * t).sin() * 0.7;
            // A click every half second: broadband, brief, and loud.
            let click = match i % (rate / 2) < 32 {
                true => 0.9,
                false => 0.0,
            };
            *sample = bass + click;
        }
        let w = analyze(&Audio::new(rate as u32, vec![plane.clone(), plane]).unwrap());

        // Most of this second is a bass note, and most of it should look like
        // one: the red channel — the low band — above the blue.
        let bassy = w
            .color_detail
            .chunks_exact(2)
            .map(|c| u16::from_be_bytes([c[0], c[1]]))
            .filter(|packed| (packed >> 13) & 7 > (packed >> 7) & 7)
            .count();
        let columns = w.color_detail.len() / 2;
        assert!(bassy * 2 > columns, "only {bassy} of {columns} columns read as bass");
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

    /// Four minutes of four-to-the-floor: a kick on every beat, which is what
    /// combs an overview drawn from peaks.
    fn four_to_the_floor(seconds: f32) -> Audio {
        use std::f32::consts::PI;
        let rate = 44_100u32;
        let beat = 60.0 / 128.0;
        let n = (rate as f32 * seconds) as usize;
        let mut plane = vec![0.0f32; n];
        let mut at = 0usize;
        while at < n {
            for k in 0..(rate as f32 * beat) as usize {
                let i = at + k;
                if i >= n {
                    break;
                }
                let t = k as f32 / rate as f32;
                plane[i] = 0.9 * (-28.0 * t).exp() * (2.0 * PI * 52.0 * t).sin()
                    + 0.2 * (2.0 * PI * 330.0 * (i as f32 / rate as f32)).sin();
            }
            at += (rate as f32 * beat) as usize;
        }
        Audio::new(rate, vec![plane.clone(), plane]).unwrap()
    }

    #[test]
    fn the_overview_of_a_steady_track_is_steady() {
        // The fault this is here for: a preview column of a long track is most
        // of a beat, so a column drawn from the peak of its span reaches the
        // top only when a transient happened to land inside it, and the answer
        // alternates. The picture that draws is a comb whose spacing is the
        // sampling rather than the music.
        let w = analyze(&four_to_the_floor(240.0));
        let heights: Vec<f32> =
            w.color_preview.chunks(6).map(|c| c[3].max(c[4]).max(c[5]) as f32).collect();
        let jitter: f32 = heights.windows(2).map(|pair| (pair[0] - pair[1]).abs()).sum::<f32>()
            / (heights.len() - 1) as f32;
        assert!(
            jitter < 12.0,
            "the overview jumps {jitter:.1} levels a column on a track that does the same thing \
             throughout"
        );
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
