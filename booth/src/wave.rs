//! The waveform under the browser.
//!
//! It paints the same three-band picture the player will draw, from the same
//! bytes the analysis file will carry — so what is on screen while prepping is
//! what will be on the CDJ's screen, rather than a second rendering that agrees
//! with it approximately.

use eframe::egui::{self, Color32, Rect, Sense, Stroke, Ui, Vec2};

use crate::library::{CueMark, Phrase};
use crate::theme;

/// How tall the waveform draws, in points.
pub const HEIGHT: f32 = 132.0;
/// The strip of phrase names under it.
pub const STRIP_HEIGHT: f32 = 16.0;

/// The bytes in one column of the three-band preview, as the format stores
/// them: mid, high, low.
const COLUMN: usize = 3;

/// How the picture is coloured.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Paint {
    /// Three bands stacked, each in its own colour: the most readable of the
    /// three for finding a kick, because the low band is drawn on its own.
    Bands,
    /// One shape, its hue mixed from the frequency content — bass blue,
    /// mid-range amber, treble washing towards white. This is the picture the
    /// player itself draws, so it is the one to prep against.
    #[default]
    Frequency,
    /// One shape, its hue mixed from which stem is loudest. Needs a rendered
    /// kit; falls back to frequency without one.
    Stems,
}

impl Paint {
    pub const ALL: [Paint; 3] = [Paint::Bands, Paint::Frequency, Paint::Stems];

    pub fn label(self) -> &'static str {
        match self {
            Paint::Bands => "bands",
            Paint::Frequency => "colour",
            Paint::Stems => "stems",
        }
    }

    pub fn blurb(self) -> &'static str {
        match self {
            Paint::Bands => "Low, mid and high stacked — easiest for finding the kick",
            Paint::Frequency => "Hue from the frequency mix — what the player draws",
            Paint::Stems => "Hue from which stem is loudest — needs a rendered kit",
        }
    }
}

/// How loud each stem is across the track, one byte per column.
///
/// Measured from the rendered stem files rather than guessed from the mix: what
/// makes this worth having over the frequency colouring is that it is telling
/// you where the *voice* is, and no band split can do that.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StemEnvelopes {
    pub vocals: Vec<u8>,
    pub melody: Vec<u8>,
    pub drums: Vec<u8>,
}

impl StemEnvelopes {
    pub fn is_empty(&self) -> bool {
        self.vocals.is_empty() || self.melody.is_empty() || self.drums.is_empty()
    }

    pub fn columns(&self) -> usize {
        self.vocals.len().min(self.melody.len()).min(self.drums.len())
    }

    /// The colour of one column: each stem's colour, weighted by how much of
    /// the sound it is. A column with nothing in it is left grey rather than
    /// being given a third of each.
    fn color_at(&self, index: usize) -> Color32 {
        let at = |plane: &[u8]| plane.get(index).copied().unwrap_or(0) as f32;
        let (vocals, melody, drums) = (at(&self.vocals), at(&self.melody), at(&self.drums));
        let total = vocals + melody + drums;
        if total < 1.0 {
            return theme::RULE;
        }
        blend(&[
            (theme::STEM_VOCALS, vocals / total),
            (theme::STEM_MELODY, melody / total),
            (theme::STEM_DRUMS, drums / total),
        ])
    }
}

/// Mix colours by weight.
fn blend(parts: &[(Color32, f32)]) -> Color32 {
    let mut rgb = [0.0f32; 3];
    for (color, weight) in parts {
        rgb[0] += color.r() as f32 * weight;
        rgb[1] += color.g() as f32 * weight;
        rgb[2] += color.b() as f32 * weight;
    }
    Color32::from_rgb(
        rgb[0].round().clamp(0.0, 255.0) as u8,
        rgb[1].round().clamp(0.0, 255.0) as u8,
        rgb[2].round().clamp(0.0, 255.0) as u8,
    )
}

/// The hue of a column from its three band levels.
///
/// The same mix the analysis files carry to the player — bass reads blue, the
/// mid-range amber, and treble washes everything towards white — so the picture
/// on screen and the picture on the CDJ are the same picture.
pub fn frequency_color(low: f32, mid: f32, high: f32) -> Color32 {
    let total = low + mid + high + f32::EPSILON;
    let (low, mid, high) = (low / total, mid / total, high / total);
    blend(&[(theme::BAND_LOW, low), (theme::BAND_MID, mid), (theme::BAND_HIGH, high)])
}

/// What the panel needs to draw one track.
pub struct Waveform<'a> {
    /// 1,200 columns of three bytes. Empty when the track has not been analysed.
    pub bands: &'a [u8],
    pub duration_secs: f64,
    pub beat_ms: &'a [u32],
    pub cues: &'a [CueMark],
    /// Where the playhead sits, as a fraction of the track, if anywhere.
    pub position: Option<f32>,
    pub paint: Paint,
    /// Per-stem loudness, when a kit has been measured.
    pub stems: Option<&'a StemEnvelopes>,
}

impl Waveform<'_> {
    /// The mode actually used, which is not always the one asked for: stem
    /// colouring without a kit would be a blank picture, so it falls back.
    pub fn effective_paint(&self) -> Paint {
        match self.paint {
            Paint::Stems if self.stems.is_none_or(|s| s.is_empty()) => Paint::Frequency,
            other => other,
        }
    }
}

impl Waveform<'_> {
    /// How many columns of picture there are.
    pub fn columns(&self) -> usize {
        self.bands.len() / COLUMN
    }

    /// The three band heights of one column, as fractions of full scale.
    ///
    /// Returns them in the order they are painted rather than the order they
    /// are stored: lows first, so the mids and highs sit over the top of them.
    fn column(&self, index: usize) -> [f32; 3] {
        let at = index * COLUMN;
        let byte = |offset: usize| self.bands.get(at + offset).copied().unwrap_or(0) as f32 / 255.0;
        [byte(2), byte(0), byte(1)]
    }
}

/// What the pointer did to the waveform.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum Touched {
    /// The playhead was moved here, in milliseconds.
    Scrubbed(u32),
    /// A cue was dragged to here.
    Moved { letter: u8, time_ms: u32 },
}

/// How close to a cue's line the pointer has to be to take hold of it, in
/// points. Wide enough to grab without aiming, narrow enough that two cues a
/// bar apart are still two things.
const GRAB: f32 = 5.0;

/// Draw the waveform, the beat ticks, the cue flags and the phrase strip.
///
/// Returns what the pointer did, if anything. Dragging a cue moves it; clicking
/// anywhere else moves the playhead.
pub fn show(ui: &mut Ui, wave: &Waveform<'_>) -> Option<Touched> {
    let width = ui.available_width();
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(width, HEIGHT), Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, theme::BOOTH);

    if wave.columns() == 0 {
        let message = "not analysed yet";
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            message,
            theme::mono(theme::SMALL),
            theme::DIM,
        );
        return None;
    }

    // The centre line sits slightly above the middle, leaving room under the
    // wave for the beat ticks without them overlapping it.
    let middle = rect.top() + rect.height() * 0.52;
    let reach = rect.height() * 0.40;

    // One screen column per pixel, sampled from however many picture columns
    // that works out to — so a wide window shows more detail rather than a
    // stretched copy of the same 1,200.
    let pixels = (rect.width().round() as usize).max(1);
    let paint = wave.effective_paint();
    let bands = [theme::BAND_LOW, theme::BAND_MID, theme::BAND_HIGH];

    for pixel in 0..pixels {
        let from = pixel * wave.columns() / pixels;
        let to = ((pixel + 1) * wave.columns() / pixels).max(from + 1);

        // The peak across the columns this pixel covers, not the mean: a
        // waveform that averages its way through a kick drum stops showing
        // where the kick is, which is the only thing it is being read for.
        let mut peaks = [0.0f32; 3];
        let mut loudest = from;
        let mut loudest_at = -1.0f32;
        for index in from..to.min(wave.columns()) {
            let column = wave.column(index);
            for (peak, value) in peaks.iter_mut().zip(column) {
                *peak = peak.max(value);
            }
            let total: f32 = column.iter().sum();
            if total > loudest_at {
                loudest_at = total;
                loudest = index;
            }
        }

        let x = rect.left() + pixel as f32;
        let bar = |half: f32, color: Color32| {
            if half > 0.0 {
                painter.rect_filled(
                    Rect::from_min_max(
                        egui::pos2(x, middle - half),
                        egui::pos2(x + 1.0, middle + half),
                    ),
                    0.0,
                    color,
                );
            }
        };

        match paint {
            // Three bars, one per band, drawn low first so the quieter bands
            // land on top of the louder one.
            Paint::Bands => {
                for (band, color) in peaks.iter().zip(bands) {
                    bar(band * reach, color);
                }
            }
            // One bar as tall as the loudest band, coloured by the mix.
            Paint::Frequency => {
                let [low, mid, high] = peaks;
                bar(low.max(mid).max(high) * reach, frequency_color(low, mid, high));
            }
            Paint::Stems => {
                let [low, mid, high] = peaks;
                let color = wave
                    .stems
                    .map(|stems| stems.color_at(loudest * stems.columns() / wave.columns().max(1)))
                    .unwrap_or(theme::RULE);
                bar(low.max(mid).max(high) * reach, color);
            }
        }
    }

    beat_ticks(&painter, rect, wave);
    let held = dragged_cue(ui, &response, rect, wave);
    cue_flags(&painter, rect, wave, held);

    if let Some(position) = wave.position {
        let x = rect.left() + rect.width() * position.clamp(0.0, 1.0);
        painter.line_segment(
            [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
            Stroke::new(1.0, theme::TEXT),
        );
    }

    // The pointer changes over a cue, which is the only signal that it can be
    // taken hold of at all.
    if held.is_some() || cue_under(&response, rect, wave).is_some() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
    }

    let at = response.interact_pointer_pos()?;
    let time_ms = time_at(rect, wave, at.x);
    match held {
        Some(letter) => Some(Touched::Moved { letter, time_ms }),
        // A drag that started on empty space is a scrub, not a cue move.
        None if response.dragged() || response.clicked() => Some(Touched::Scrubbed(time_ms)),
        None => None,
    }
}

/// Where a horizontal position falls in the track.
fn time_at(rect: Rect, wave: &Waveform<'_>, x: f32) -> u32 {
    let fraction = ((x - rect.left()) / rect.width()).clamp(0.0, 1.0) as f64;
    (fraction * wave.duration_secs * 1000.0).round() as u32
}

/// Which cue the pointer is over, if any.
fn cue_under(response: &egui::Response, rect: Rect, wave: &Waveform<'_>) -> Option<u8> {
    let at = response.hover_pos().or_else(|| response.interact_pointer_pos())?;
    if wave.duration_secs <= 0.0 {
        return None;
    }
    wave.cues
        .iter()
        .map(|cue| (cue.letter, cue_x(rect, wave, cue.time_ms)))
        .filter(|(_, x)| (x - at.x).abs() <= GRAB)
        .min_by(|a, b| (a.1 - at.x).abs().total_cmp(&(b.1 - at.x).abs()))
        .map(|(letter, _)| letter)
}

fn cue_x(rect: Rect, wave: &Waveform<'_>, time_ms: u32) -> f32 {
    let total_ms = (wave.duration_secs * 1000.0).max(1.0);
    rect.left() + rect.width() * (time_ms as f64 / total_ms).clamp(0.0, 1.0) as f32
}

/// The cue being dragged, remembered for the length of the drag.
///
/// Which cue is under the pointer is decided once, when the drag starts:
/// re-deciding every frame would let a fast drag hand over to a cue it passed.
fn dragged_cue(ui: &Ui, response: &egui::Response, rect: Rect, wave: &Waveform<'_>) -> Option<u8> {
    let id = response.id.with("dragging-cue");
    if response.drag_started() {
        let under = cue_under(response, rect, wave);
        ui.ctx().memory_mut(|memory| memory.data.insert_temp(id, under));
    }
    if response.drag_stopped() {
        ui.ctx().memory_mut(|memory| memory.data.remove::<Option<u8>>(id));
        return None;
    }
    if !response.dragged() {
        return None;
    }
    ui.ctx().memory(|memory| memory.data.get_temp::<Option<u8>>(id)).flatten()
}

/// Ticks along the bottom, tall on the downbeat.
///
/// They are drawn from the grid rather than from the tempo, so a grid that
/// drifts — which is the case this whole format handles well and most software
/// handles badly — is visible as ticks that drift.
fn beat_ticks(painter: &egui::Painter, rect: Rect, wave: &Waveform<'_>) {
    if wave.beat_ms.is_empty() || wave.duration_secs <= 0.0 {
        return;
    }
    let total_ms = wave.duration_secs * 1000.0;
    let faint = theme::TEXT.gamma_multiply(0.28);

    // At a normal window width there are more beats than pixels, so draw every
    // bar line and only as many beats as will read as separate marks.
    let spacing = rect.width() / wave.beat_ms.len() as f32;
    let every_beat = spacing >= 3.0;

    for (index, time) in wave.beat_ms.iter().enumerate() {
        let bar = index % 4 == 0;
        if !bar && !every_beat {
            continue;
        }
        let x = rect.left() + rect.width() * (*time as f64 / total_ms).clamp(0.0, 1.0) as f32;
        let height = if bar { 9.0 } else { 4.0 };
        painter.line_segment(
            [egui::pos2(x, rect.bottom() - height), egui::pos2(x, rect.bottom())],
            Stroke::new(1.0, faint),
        );
    }
}

/// A line and a flag per cue, with its letter. The one being dragged is drawn
/// brighter, so it is clear which one moved.
fn cue_flags(painter: &egui::Painter, rect: Rect, wave: &Waveform<'_>, held: Option<u8>) {
    if wave.duration_secs <= 0.0 {
        return;
    }
    let total_ms = wave.duration_secs * 1000.0;

    for cue in wave.cues {
        let fraction = (cue.time_ms as f64 / total_ms).clamp(0.0, 1.0) as f32;
        let x = rect.left() + rect.width() * fraction;
        let mut color = Color32::from_rgb(cue.color[0], cue.color[1], cue.color[2]);
        if held == Some(cue.letter) {
            color = theme::TEXT;
        }

        painter.line_segment(
            [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom() - 10.0)],
            Stroke::new(1.5, color),
        );
        // A flag, pointing the way the cue reads: from the marker into the
        // track.
        painter.add(egui::Shape::convex_polygon(
            vec![
                egui::pos2(x - 4.5, rect.top()),
                egui::pos2(x + 6.0, rect.top()),
                egui::pos2(x + 0.75, rect.top() + 9.0),
            ],
            color,
            Stroke::NONE,
        ));
        painter.text(
            egui::pos2(x + 8.0, rect.top() + 1.0),
            egui::Align2::LEFT_TOP,
            cue.name(),
            theme::mono(9.5),
            color,
        );
    }
}

/// The phrase strip: one block per section, as wide as the section is long.
pub fn phrase_strip(ui: &mut Ui, phrases: &[Phrase], duration_secs: f64) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, STRIP_HEIGHT), Sense::hover());
    let painter = ui.painter_at(rect);
    if phrases.is_empty() || duration_secs <= 0.0 {
        return;
    }
    let total_ms = duration_secs * 1000.0;

    for phrase in phrases {
        let from = rect.left() + rect.width() * (phrase.start_ms as f64 / total_ms) as f32;
        let to = rect.left() + rect.width() * (phrase.end_ms as f64 / total_ms) as f32;
        // A one-pixel gap between blocks, which is what makes them read as
        // separate phrases rather than as a colour bar.
        let block =
            Rect::from_min_max(egui::pos2(from, rect.top()), egui::pos2(to - 1.0, rect.bottom()));
        if block.width() <= 0.0 {
            continue;
        }
        let color = theme::phrase_color(&phrase.kind);
        painter.rect_filled(block, 0.0, color);

        // The name only goes in when it fits; a clipped label is worse than
        // the colour on its own, which already says what the phrase is.
        let label = theme::label_text(&phrase.kind);
        let galley =
            painter.layout_no_wrap(label, theme::sans(9.0), Color32::from_rgb(0x0F, 0x13, 0x16));
        if galley.size().x + 8.0 < block.width() {
            painter.galley(
                egui::pos2(block.left() + 4.0, block.center().y - galley.size().y / 2.0),
                galley,
                Color32::BLACK,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bands(columns: &[[u8; 3]]) -> Vec<u8> {
        columns.iter().flatten().copied().collect()
    }

    fn wave(bytes: &[u8]) -> Waveform<'_> {
        Waveform {
            bands: bytes,
            duration_secs: 10.0,
            beat_ms: &[],
            cues: &[],
            position: None,
            paint: Paint::Bands,
            stems: None,
        }
    }

    #[test]
    fn a_column_is_read_lows_first_whatever_order_it_is_stored_in() {
        // The format stores mid, high, low; the picture is painted low, mid,
        // high, so that the quieter bands land on top of the louder one.
        let bytes = bands(&[[51, 102, 255]]);
        let wave = wave(&bytes);
        let [low, mid, high] = wave.column(0);
        assert!((low - 1.0).abs() < 0.01, "low should be the third byte: {low}");
        assert!((mid - 0.2).abs() < 0.01, "mid should be the first byte: {mid}");
        assert!((high - 0.4).abs() < 0.01, "high should be the second byte: {high}");
    }

    #[test]
    fn a_short_or_ragged_picture_does_not_panic() {
        // A truncated file should draw as much as it has rather than crash the
        // window that is showing it.
        let bytes = vec![10, 20];
        let wave = wave(&bytes);
        assert_eq!(wave.columns(), 0);
        assert_eq!(wave.column(5), [0.0, 0.0, 0.0]);
    }

    #[test]
    fn frequency_colour_follows_whichever_band_is_loudest() {
        // Pure bass reads blue, pure mid-range amber, pure treble near-white:
        // the palette the player itself draws in.
        assert_eq!(frequency_color(1.0, 0.0, 0.0), theme::BAND_LOW);
        assert_eq!(frequency_color(0.0, 1.0, 0.0), theme::BAND_MID);
        assert_eq!(frequency_color(0.0, 0.0, 1.0), theme::BAND_HIGH);

        // A mix lands between them rather than snapping to one.
        let mixed = frequency_color(1.0, 1.0, 0.0);
        assert!(mixed != theme::BAND_LOW && mixed != theme::BAND_MID);
        let between = |a: u8, b: u8, c: u8| c >= a.min(b) && c <= a.max(b);
        assert!(between(theme::BAND_LOW.r(), theme::BAND_MID.r(), mixed.r()));
    }

    #[test]
    fn a_silent_column_has_a_colour_rather_than_a_division_by_zero() {
        let colour = frequency_color(0.0, 0.0, 0.0);
        assert!(colour.a() > 0, "a silent column still has to draw as something");
    }

    #[test]
    fn stem_colour_follows_whichever_stem_is_loudest() {
        let envelopes = StemEnvelopes {
            vocals: vec![255, 0, 0],
            melody: vec![0, 255, 0],
            drums: vec![0, 0, 255],
        };
        assert_eq!(envelopes.color_at(0), theme::STEM_VOCALS);
        assert_eq!(envelopes.color_at(1), theme::STEM_MELODY);
        assert_eq!(envelopes.color_at(2), theme::STEM_DRUMS);
    }

    #[test]
    fn a_column_with_no_stem_playing_is_not_a_third_of_each() {
        let envelopes = StemEnvelopes { vocals: vec![0], melody: vec![0], drums: vec![0] };
        // A blend of three colours at zero weight would be black, which reads
        // as "nothing here" only by accident; this says it deliberately.
        assert_eq!(envelopes.color_at(0), theme::RULE);
    }

    #[test]
    fn stem_colouring_falls_back_when_there_is_no_kit() {
        let bytes = bands(&[[10, 20, 30]]);
        let mut wave = wave(&bytes);
        wave.paint = Paint::Stems;
        assert_eq!(wave.effective_paint(), Paint::Frequency, "a blank picture is not an answer");

        let envelopes = StemEnvelopes { vocals: vec![1], melody: vec![1], drums: vec![1] };
        wave.stems = Some(&envelopes);
        assert_eq!(wave.effective_paint(), Paint::Stems);

        // A half-measured kit is no kit.
        let partial = StemEnvelopes { vocals: vec![1], melody: Vec::new(), drums: vec![1] };
        wave.stems = Some(&partial);
        assert_eq!(wave.effective_paint(), Paint::Frequency);
    }

    #[test]
    fn asking_for_bands_or_frequency_never_falls_back() {
        let bytes = bands(&[[10, 20, 30]]);
        let mut wave = wave(&bytes);
        for mode in [Paint::Bands, Paint::Frequency] {
            wave.paint = mode;
            assert_eq!(wave.effective_paint(), mode);
        }
    }

    #[test]
    fn envelopes_of_different_lengths_report_the_shortest() {
        // The three stems are measured separately and a truncated one must not
        // be read past the end of.
        let envelopes =
            StemEnvelopes { vocals: vec![1, 2, 3], melody: vec![1, 2], drums: vec![1, 2, 3, 4] };
        assert_eq!(envelopes.columns(), 2);
        assert!(!envelopes.is_empty());
        // Reading past what the shortest has does not panic.
        let _ = envelopes.color_at(99);
    }

    #[test]
    fn the_column_count_is_the_number_of_whole_columns() {
        let bytes = bands(&[[1, 2, 3], [4, 5, 6]]);
        assert_eq!(wave(&bytes).columns(), 2);
        assert_eq!(wave(&[]).columns(), 0);
    }
}
