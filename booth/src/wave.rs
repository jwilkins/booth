//! The waveform under the browser.
//!
//! It paints the same three-band picture the player will draw, from the same
//! bytes the analysis file will carry — so what is on screen while prepping is
//! what will be on the CDJ's screen, rather than a second rendering that agrees
//! with it approximately.

use eframe::egui::{self, Color32, Rect, Sense, Stroke, Ui, Vec2};

use booth_cli::export::waveform::loudness;

use crate::library::{CueMark, Phrase, PhraseEdit, BEATS_PER_BAR};
use crate::theme;

/// How tall the waveform draws, in points.
pub const HEIGHT: f32 = 132.0;
/// The strip of phrase names under it.
///
/// Taller than the sixteen points it was when it held nothing but a label,
/// because it now carries the shape of the music as well: at sixteen the
/// silhouette had about eight points of swing left after the text, which is
/// not enough to tell a build from a break.
pub const STRIP_HEIGHT: f32 = 26.0;

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

    /// The colour of one column: whichever stem is doing the most, tinted by
    /// the others. A column with nothing in it is left grey rather than being
    /// given a third of each.
    fn color_at(&self, index: usize) -> Color32 {
        let at = |plane: &[u8]| plane.get(index).copied().unwrap_or(0) as f32;
        let (vocals, melody, drums) = (at(&self.vocals), at(&self.melody), at(&self.drums));
        if vocals + melody + drums < 1.0 {
            return theme::rule();
        }
        mix(
            [theme::stem_vocals(), theme::stem_melody(), theme::stem_drums()],
            [vocals, melody, drums],
        )
    }
}

/// How hard the loudest of the three is made to win.
///
/// Both pictures are drawn from levels that have already been square-rooted,
/// which pulls them together: a column that is plainly a kick drum still
/// measures as about half low, a third mid and a sixth high. Mixed in those
/// proportions every column of every record lands on the same pale grey, which
/// is a true average and a useless picture. Raising the shares to a power
/// first does not invent a band that is not there — it stops the two that are
/// quieter from speaking as loudly as the one that is not.
///
/// Two rather than three. At three the leading band takes so much of the mix
/// that the others cannot tint it, and every column comes out the colour of
/// whichever band happened to lead — which for most music is the mid, all the
/// way through. A column with a strong bass under it and one with none came
/// out the same hue to within a third of a degree.
const CONTRAST: f32 = 2.0;

/// How far a fully mixed colour is then pushed back away from grey.
///
/// Three colours averaged sit nearer the middle of the palette than any of them
/// does, however they are weighted. This puts back what the averaging took out,
/// at the same brightness — the height of the column is already the loudness,
/// and the colour must not start saying it again.
const SATURATION: f32 = 1.4;

/// Mix three colours by weight, so that the largest weight is legible as a
/// colour rather than as a shade of the average.
///
/// The push is proportional to how much mixing there was to undo, so a column
/// that really is all one band comes out as exactly that band's colour. A
/// palette is a promise about what a colour means; saturating past it would
/// draw a blue no legend accounts for.
fn mix(colors: [Color32; 3], weights: [f32; 3]) -> Color32 {
    let total = weights.iter().sum::<f32>() + f32::EPSILON;
    let sharpened = weights.map(|w| (w / total).max(0.0).powf(CONTRAST));
    let sum = sharpened.iter().sum::<f32>() + f32::EPSILON;
    let shares = sharpened.map(|w| w / sum);
    let parts: Vec<(Color32, f32)> = colors.iter().zip(shares).map(|(&c, w)| (c, w)).collect();

    // One share of everything is the most averaged a column can be, and it is
    // two thirds rather than one, so the scale is stretched to reach it.
    let winner = shares.iter().copied().fold(0.0f32, f32::max);
    let mixedness = ((1.0 - winner) * 1.5).clamp(0.0, 1.0);
    saturate(blend(&parts), 1.0 + (SATURATION - 1.0) * mixedness)
}

/// Pull a colour away from grey without changing how bright it is.
fn saturate(color: Color32, amount: f32) -> Color32 {
    let rgb = [color.r() as f32, color.g() as f32, color.b() as f32];
    let grey = 0.299 * rgb[0] + 0.587 * rgb[1] + 0.114 * rgb[2];
    let pushed = rgb.map(|c| (grey + (c - grey) * amount).round().clamp(0.0, 255.0) as u8);
    Color32::from_rgb(pushed[0], pushed[1], pushed[2])
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

/// The palette the colour mode mixes in: one band per channel, low to high.
///
/// The convention every other DJ program uses, and the one the EQ colour charts
/// a DJ will have seen are drawn from — red is the bass, green the mid-range,
/// blue the treble. It is not an arbitrary choice and never was: with the three
/// bands on the three channels, a column made of two of them lands on a
/// secondary that names the pair. Bass and mid read yellow, mid and treble
/// cyan, bass and treble magenta, and a column with all three reads white.
/// Nothing cancels, because no two primaries are opposite each other.
///
/// This is also what the analysis files carry, so the picture on screen and the
/// picture on the deck are the same picture. See
/// [`booth_cli::export::waveform`], which packs the same three numbers into
/// three bits each.
///
/// Not the band colours the stacked mode uses. Those are three labels on three
/// bars that only have to be told apart, and they are read one at a time rather
/// than mixed.
const FREQ_LOW: Color32 = Color32::from_rgb(0xFF, 0x00, 0x00);
const FREQ_MID: Color32 = Color32::from_rgb(0x00, 0xFF, 0x00);
const FREQ_HIGH: Color32 = Color32::from_rgb(0x00, 0x00, 0xFF);

/// The colour of a column, from what it is made of.
///
/// The levels are undone first. They are stored bent by a display curve that
/// exists to make heights readable, and it flattens the bands against each
/// other on the way — a column that is plainly a kick reads 0.95 low against
/// 0.71 mid through it. A colour is a set of proportions, so it has to be
/// worked out on the amplitudes the curve was applied to rather than on what
/// came out.
pub fn frequency_color(low: f32, mid: f32, high: f32) -> Color32 {
    use booth_cli::export::waveform::unshape;
    additive(unshape(low), unshape(mid), unshape(high))
}

/// One band per channel, each measured against the loudest of the three.
///
/// Against their sum the three shares always total one, so the strongest band
/// can never reach the top of its channel and every column comes out a shade of
/// grey — which is what a CDJ-3000X drew from the exported bytes before they
/// were measured this way. Against the peak, whatever dominates the column
/// saturates and the rest fall away from it.
///
/// Squared, for the same reason and to the same degree as the exported colour:
/// a band a third as loud as the leader should tint the colour rather than
/// dilute it. The two are the same arithmetic on purpose — a screen that
/// disagreed with the deck about what a column is made of would be worse than
/// no colour at all.
///
/// Brightness is left out of it. The height of the column is already the
/// loudness, and a colour that said it again would make a quiet break
/// unreadable to save repeating something the shape has already shown.
fn additive(low: f32, mid: f32, high: f32) -> Color32 {
    let peak = low.max(mid).max(high);
    if peak <= f32::EPSILON {
        return theme::rule();
    }
    let share = |band: f32| {
        let ratio = (band / peak).clamp(0.0, 1.0);
        ratio * ratio
    };

    // Added rather than averaged. Averaging bass and mid gives the dull olive
    // halfway between red and green; adding them gives yellow, which is the
    // whole reason the convention is three primaries on three channels.
    let mut rgb = [0.0f32; 3];
    for (band, weight) in [(FREQ_LOW, share(low)), (FREQ_MID, share(mid)), (FREQ_HIGH, share(high))]
    {
        rgb[0] += band.r() as f32 * weight;
        rgb[1] += band.g() as f32 * weight;
        rgb[2] += band.b() as f32 * weight;
    }
    Color32::from_rgb(
        rgb[0].min(255.0).round() as u8,
        rgb[1].min(255.0).round() as u8,
        rgb[2].min(255.0).round() as u8,
    )
}

/// The bands in the order they have to be drawn: tallest first.
///
/// The bars are all centred on the same line, so a band drawn after a taller
/// one is hidden behind it completely. Drawn in a fixed low, mid, high order
/// that is only right when the low band is the loudest — which on most music
/// it is not. The mid band covers the whole range from a bassline to a vocal
/// and is usually the tallest, so it painted over the low band every time and
/// the kick, the one thing this mode exists to find, was never visible at all.
///
/// It matters most on a modern master, where the total height carries almost
/// nothing: a limiter flattens the peak of the whole signal to a straight
/// line, and the low band is the only measurement left that still moves with
/// the music.
pub fn stacked(peaks: [f32; 3], colors: [Color32; 3]) -> [(f32, Color32); 3] {
    let mut stack = [(peaks[0], colors[0]), (peaks[1], colors[1]), (peaks[2], colors[2])];
    stack.sort_by(|a, b| b.0.total_cmp(&a.0));
    stack
}

/// How many bars a phrase runs for.
///
/// Counted off the same grid the marks are drawn from, rather than worked out
/// from the tempo. The tempo was a second answer to the same question, and the
/// two came apart wherever a section did not hold a whole number of bars — a
/// section the tempo rounded up to sixteen with fifteen marks under it — and on
/// any grid that is not perfectly even, which is every grid a player has bent
/// by hand.
///
/// Every fourth beat, because [`crate::app`]'s grid starts on a downbeat.
///
/// `None` when there is no grid to count against, or when the phrase does not
/// hold a whole bar: a section labelled "0" says less than one with no number
/// at all.
fn bars_of(phrase: &Phrase, beat_ms: &[u32]) -> Option<usize> {
    let bars = beat_ms
        .iter()
        .step_by(BEATS_PER_BAR)
        .filter(|time| (phrase.start_ms..phrase.end_ms).contains(time))
        .count();
    (bars > 0).then_some(bars)
}

/// Which part of the track the picture is showing.
///
/// Both numbers are fractions of the whole track, so the view survives the
/// window being resized and does not have to know how long the track is.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Zoom {
    /// Where the left edge of the panel falls in the track.
    pub start: f32,
    /// How much of the track fits across the panel. One is the whole thing.
    pub span: f32,
}

impl Default for Zoom {
    fn default() -> Self {
        Self { start: 0.0, span: 1.0 }
    }
}

impl Zoom {
    /// Whether the whole track is showing, which is the state the fit button
    /// and the readout are hidden in.
    pub fn is_fit(self) -> bool {
        self.span >= 1.0
    }

    /// Where a point in the track falls across the panel, as a fraction of its
    /// width. Outside 0..1 when the point is off-screen, which is what lets a
    /// cue flag be culled rather than drawn on the edge.
    fn across(self, fraction: f64) -> f32 {
        ((fraction - self.start as f64) / self.span.max(f32::EPSILON) as f64) as f32
    }

    /// The reverse: where a fraction of the panel's width falls in the track.
    fn into_track(self, across: f32) -> f64 {
        (self.start + across * self.span).clamp(0.0, 1.0) as f64
    }

    /// Pull the view back inside the track, keeping its width where it can.
    fn settled(mut self) -> Self {
        self.span = self.span.clamp(f32::EPSILON, 1.0);
        self.start = self.start.clamp(0.0, 1.0 - self.span);
        self
    }

    /// Zoom by a factor, holding the point under the pointer still.
    ///
    /// Holding that point is the whole trick: zooming about the centre means
    /// the thing you were looking at is the thing that moves away, and you
    /// chase it with the scrollbar.
    fn scaled(self, factor: f32, about: f32, floor: f32) -> Self {
        let held = self.into_track(about) as f32;
        let span = (self.span * factor).clamp(floor.min(1.0), 1.0);
        Self { start: held - about * span, span }.settled()
    }

    /// Slide the view sideways by a fraction of its own width.
    fn panned(self, by: f32) -> Self {
        Self { start: self.start + by * self.span, ..self }.settled()
    }

    /// Put a moment in the middle of the view, for following the playhead.
    fn centred(self, on: f32) -> Self {
        Self { start: on - self.span / 2.0, ..self }.settled()
    }
}

/// How much one notch of the wheel changes the span.
const ZOOM_STEP: f32 = 0.0015;
/// How much of the panel one notch of a sideways wheel moves it.
const PAN_STEP: f32 = 0.0012;

/// The finest view worth offering, given how much picture there is.
///
/// Past about one stored column per two pixels the picture is being stretched
/// rather than revealed, and a staircase drawn confidently is worse than a
/// coarse picture honestly drawn: it invites placing a cue against an edge that
/// is an artefact of the drawing.
fn zoom_floor(columns: usize, pixels: f32) -> f32 {
    if columns == 0 {
        return 1.0;
    }
    (pixels / 2.0 / columns as f32).clamp(0.0005, 1.0)
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
    /// Which part of the track is showing.
    pub zoom: Zoom,
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
    ///
    /// `at` indexes the cues the panel was drawn from, not the cue's letter.
    /// Every memory cue has letter zero, so a letter stopped being a name for
    /// one the moment a track could have more than one of them — dragging
    /// "Drop 2" moved whichever memory cue came first, which is the one the
    /// grid is anchored to.
    Moved { at: usize, time_ms: u32 },
}

/// How close to a line the pointer has to be to take hold of it, in points.
/// Wide enough to grab without aiming, narrow enough that two cues a bar apart
/// are still two things. Shared by the cues and by the phrase boundaries,
/// which are the same gesture on two different strips.
const GRAB: f32 = 5.0;

/// What one frame of the waveform panel came to.
pub struct Shown {
    /// What the pointer did, if anything.
    pub touched: Option<Touched>,
    /// The view after any scrolling. The caller keeps this: the panel is drawn
    /// from scratch every frame and has nowhere of its own to remember it.
    pub zoom: Zoom,
}

/// Draw the waveform, the beat ticks and the cue flags.
///
/// Dragging a cue moves it; clicking anywhere else moves the playhead. The
/// wheel zooms about the pointer, and shift or a sideways wheel pans.
pub fn show(ui: &mut Ui, wave: &Waveform<'_>) -> Shown {
    let width = ui.available_width();
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(width, HEIGHT), Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, theme::booth());

    if wave.columns() == 0 {
        let message = "not analysed yet";
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            message,
            theme::mono(theme::SMALL),
            theme::dim(),
        );
        // The view it was handed back, not a fitted one. A panel with nothing
        // to draw has not been zoomed out — it has not been asked about the
        // zoom at all — and the caller stores whatever comes back here, so
        // saying `default` throws somebody's view away for every frame the
        // picture happens to be missing. That is what reset the zoom on every
        // press of a grid button: the fix dropped the picture, this frame
        // reported a fitted view, and the next one drew it.
        return Shown { touched: None, zoom: wave.zoom };
    }

    // The wheel is read before anything is drawn, so a scroll and the frame it
    // scrolled in show the same view — reading it afterwards leaves the picture
    // one frame behind the pointer, which feels like lag rather than like zoom.
    let zoom = wheeled(ui, &response, rect, wave);

    // The centre line sits slightly above the middle, leaving room under the
    // wave for the beat ticks without them overlapping it.
    let middle = rect.top() + rect.height() * 0.52;
    let reach = rect.height() * 0.40;

    // One screen column per pixel, sampled from however many picture columns
    // that works out to — so a wide window shows more detail rather than a
    // stretched copy of the same 1,200.
    let pixels = (rect.width().round() as usize).max(1);
    let paint = wave.effective_paint();
    let bands = [theme::band_low(), theme::band_mid(), theme::band_high()];
    let columns = wave.columns();
    // Which stored columns each pixel covers, once the view has decided how
    // much of the track is across the panel.
    let column_at = |across: f32| {
        ((zoom.into_track(across) * columns as f64) as usize).min(columns.saturating_sub(1))
    };

    for pixel in 0..pixels {
        let from = column_at(pixel as f32 / pixels as f32);
        let to = column_at((pixel + 1) as f32 / pixels as f32).max(from + 1);

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
            // Three bars, one per band, tallest first so the quieter ones land
            // on top of the louder one rather than behind it.
            Paint::Bands => {
                for (band, color) in stacked(peaks, bands) {
                    bar(band * reach, color);
                }
            }
            // One bar, coloured by the mix. As tall as the same weighted
            // blend of the bands the drive is written from, so the picture on
            // screen and the picture on the player are the same picture — and
            // so that this mode is not the flat one. Drawn from the loudest
            // band it followed the mid, which on a limited master is a
            // straight line.
            Paint::Frequency => {
                let [low, mid, high] = peaks;
                bar(loudness(low, mid, high) * reach, frequency_color(low, mid, high));
            }
            Paint::Stems => {
                let [low, mid, high] = peaks;
                let color = wave
                    .stems
                    .map(|stems| stems.color_at(loudest * stems.columns() / wave.columns().max(1)))
                    .unwrap_or(theme::rule());
                bar(loudness(low, mid, high) * reach, color);
            }
        }
    }

    beat_ticks(&painter, rect, wave);
    let held = dragged_cue(ui, &response, rect, wave);
    cue_flags(&painter, rect, wave, held);

    if let Some(position) = wave.position {
        let across = zoom.across(position.clamp(0.0, 1.0) as f64);
        if (0.0..=1.0).contains(&across) {
            let x = rect.left() + rect.width() * across;
            painter.line_segment(
                [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
                Stroke::new(1.0_f32, theme::text()),
            );
        }
    }

    if !zoom.is_fit() {
        painter.text(
            egui::pos2(rect.right() - 6.0, rect.bottom() - 4.0),
            egui::Align2::RIGHT_BOTTOM,
            format!("{:.0}\u{d7}", 1.0 / zoom.span),
            theme::mono(9.5),
            theme::dim(),
        );
    }

    // The pointer changes over a cue, which is the only signal that it can be
    // taken hold of at all.
    let hovering_a_cue = response
        .hover_pos()
        .or_else(|| response.interact_pointer_pos())
        .and_then(|at| cue_under(at, rect, wave));
    if held.is_some() || hovering_a_cue.is_some() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
    }
    // What the marker has no room to say. Not while one is being dragged: the
    // help would sit over the thing being placed.
    let response = match hovering_a_cue.filter(|_| held.is_none()).and_then(|at| wave.cues.get(at))
    {
        Some(cue) => response.on_hover_text(cue_help(cue)),
        None => response,
    };

    let touched = response.interact_pointer_pos().and_then(|at| {
        let time_ms = time_at(rect, wave, at.x);
        match held {
            Some(at) => Some(Touched::Moved { at, time_ms }),
            // A drag that started on empty space is a scrub, not a cue move.
            None if response.dragged() || response.clicked() => Some(Touched::Scrubbed(time_ms)),
            None => None,
        }
    });
    Shown { touched, zoom }
}

/// Read the wheel over the panel, and keep a playing track in view.
///
/// Following the playhead only kicks in once it has actually left the view,
/// and only while it is running — see [`follows`]. Recentring on every frame
/// would make a zoomed picture scroll continuously, which is a different
/// instrument; what is wanted here is that the thing you zoomed in on to check
/// does not vanish while you listen to it.
fn wheeled(ui: &Ui, response: &egui::Response, rect: Rect, wave: &Waveform<'_>) -> Zoom {
    let floor = zoom_floor(wave.columns(), rect.width());
    let mut zoom = wave.zoom.settled();
    if zoom.span < floor {
        zoom.span = floor;
        zoom = zoom.settled();
    }

    if response.hovered() {
        let (scroll, modifiers) = ui.input(|i| (i.smooth_scroll_delta, i.modifiers));
        let about = response
            .hover_pos()
            .map(|at| ((at.x - rect.left()) / rect.width()).clamp(0.0, 1.0))
            .unwrap_or(0.5);
        // A sideways wheel, or shift with a vertical one: both are how a
        // trackpad and a mouse each say "along" rather than "in".
        let sideways = if modifiers.shift { -scroll.y } else { scroll.x };
        if sideways != 0.0 {
            zoom = zoom.panned(-sideways * PAN_STEP);
        } else if scroll.y != 0.0 {
            zoom = zoom.scaled((-scroll.y * ZOOM_STEP).exp(), about, floor);
        }
    }

    if let Some(position) = wave.position {
        // What the playhead was last frame, which is the only way to tell a
        // running one from a parked one: the panel is drawn from scratch every
        // frame and the position alone says nothing about whether it moved.
        let id = response.id.with("playhead-was");
        let was = ui.ctx().memory(|memory| memory.data.get_temp::<f32>(id));
        ui.ctx().memory_mut(|memory| memory.data.insert_temp(id, position));
        if follows(zoom, position, was, ui.input(|input| input.pointer.any_down())) {
            zoom = zoom.centred(position);
        }
    }
    zoom
}

/// Whether the view should be pulled back to the playhead.
///
/// Only while the playhead is running. A parked one is not going anywhere, and
/// pulling the view back to it anyway undid every scroll on the frame after it
/// was made: dragging the phrase strip stuttered and never got more than a few
/// points from wherever the playhead was sitting, which on a track nobody had
/// scrubbed was the top of it.
///
/// And never while the pointer is down, because then the view is being moved
/// on purpose and the thing worth following is the hand doing it.
fn follows(zoom: Zoom, at: f32, was: Option<f32>, held: bool) -> bool {
    if zoom.is_fit() || held {
        return false;
    }
    let running = was.is_some_and(|before| before != at);
    running && !(0.0..=1.0).contains(&zoom.across(at as f64))
}

/// Where a horizontal position falls in the track.
///
/// Through the zoom, like everything else that crosses between a time and a
/// place on the panel. Zoomed in, a pixel is a fraction of the *window* and the
/// window is a fraction of the track; reading it as a fraction of the track put
/// a click at the wrong moment by however far the view had been scrolled.
fn time_at(rect: Rect, wave: &Waveform<'_>, x: f32) -> u32 {
    let across = ((x - rect.left()) / rect.width()).clamp(0.0, 1.0);
    (wave.zoom.into_track(across) * wave.duration_secs * 1000.0).round() as u32
}

/// Which cue the pointer is over, if any, by its place in the list drawn.
fn cue_under(at: egui::Pos2, rect: Rect, wave: &Waveform<'_>) -> Option<usize> {
    if wave.duration_secs <= 0.0 {
        return None;
    }
    wave.cues
        .iter()
        .enumerate()
        .map(|(index, cue)| (index, cue_x(rect, wave, cue.time_ms)))
        .filter(|(_, x)| (x - at.x).abs() <= GRAB)
        .min_by(|a, b| (a.1 - at.x).abs().total_cmp(&(b.1 - at.x).abs()))
        .map(|(index, _)| index)
}

/// Where a moment in the track falls on the panel.
///
/// The same mapping [`cue_flags`] draws with, which is the point of it being
/// one function: they disagreed, so a cue was drawn in the right place and
/// grabbed in another, and zoomed in the gap between the two was the whole
/// width of the panel.
fn cue_x(rect: Rect, wave: &Waveform<'_>, time_ms: u32) -> f32 {
    let total_ms = (wave.duration_secs * 1000.0).max(1.0);
    let fraction = (time_ms as f64 / total_ms).clamp(0.0, 1.0);
    rect.left() + rect.width() * wave.zoom.across(fraction)
}

/// The cue being dragged, remembered for the length of the drag.
///
/// Which cue is under the pointer is decided once, when the drag starts:
/// re-deciding every frame would let a fast drag hand over to a cue it passed.
fn dragged_cue(
    ui: &Ui,
    response: &egui::Response,
    rect: Rect,
    wave: &Waveform<'_>,
) -> Option<usize> {
    let id = response.id.with("dragging-cue");
    if response.drag_started() {
        // From where the button went down, not from where the pointer is by
        // the time egui calls it a drag — by then it has already travelled off
        // the line it took hold of. See [`dragged_boundary`].
        let from = ui
            .ctx()
            .input(|input| input.pointer.press_origin())
            .or_else(|| response.interact_pointer_pos());
        let under = from.and_then(|at| cue_under(at, rect, wave));
        ui.ctx().memory_mut(|memory| memory.data.insert_temp(id, under));
    }
    if response.drag_stopped() {
        ui.ctx().memory_mut(|memory| memory.data.remove::<Option<usize>>(id));
        return None;
    }
    if !response.dragged() {
        return None;
    }
    ui.ctx().memory(|memory| memory.data.get_temp::<Option<usize>>(id)).flatten()
}

/// The downbeat, in the colour every player draws it in.
///
/// Red on the one and white on the other three is not a choice so much as a
/// convention: it is what a CDJ shows, so it is what a DJ reads without having
/// to think about it, and a grid that used one colour for all four made the
/// one indistinguishable from the rest at a glance.
const DOWNBEAT: Color32 = Color32::from_rgb(0xE5, 0x3E, 0x3E);
const OFFBEAT: Color32 = Color32::from_rgb(0xD8, 0xDE, 0xE4);

/// Ticks along the bottom, tall and red on the downbeat.
///
/// They are drawn from the grid rather than from the tempo, so a grid that
/// drifts — which is the case this whole format handles well and most software
/// handles badly — is visible as ticks that drift.
///
/// Through the zoom, like the waveform above them. They were not, so zooming in
/// left the grid where it was while the music moved out from under it, which is
/// the one thing a grid must never do.
fn beat_ticks(painter: &egui::Painter, rect: Rect, wave: &Waveform<'_>) {
    if wave.beat_ms.is_empty() || wave.duration_secs <= 0.0 {
        return;
    }
    let total_ms = wave.duration_secs * 1000.0;

    // At a normal window width there are more beats than pixels, so draw every
    // bar line and only as many beats as will read as separate marks. Counted
    // against the beats that are *showing*: zoomed in, a handful of bars have
    // the whole panel to themselves and every beat has room.
    let showing = wave.beat_ms.len() as f32 * wave.zoom.span.max(f32::EPSILON);
    let every_beat = rect.width() / showing.max(1.0) >= 3.0;

    for (index, time) in wave.beat_ms.iter().enumerate() {
        let downbeat = index % BEATS_PER_BAR == 0;
        if !downbeat && !every_beat {
            continue;
        }
        let across = wave.zoom.across((*time as f64 / total_ms).clamp(0.0, 1.0));
        if !(0.0..=1.0).contains(&across) {
            continue;
        }
        let x = rect.left() + rect.width() * across;
        let (height, color) = match downbeat {
            true => (9.0, DOWNBEAT.gamma_multiply(0.9)),
            false => (4.0, OFFBEAT.gamma_multiply(0.45)),
        };
        painter.line_segment(
            [egui::pos2(x, rect.bottom() - height), egui::pos2(x, rect.bottom())],
            Stroke::new(1.0_f32, color),
        );
    }
}

/// A line and a flag per cue, with its letter. The one being dragged is drawn
/// brighter, so it is clear which one moved.
///
/// Hot cues hang from the top and memory cues from the bottom. They are two
/// different things — eight buttons against as many markers as a track needs —
/// and a track with an arrangement's worth of memory cues on it was a row of
/// identical flags along the top with the buttons lost among them. Opposite
/// edges separate them at a glance and give each the full height of the panel
/// to be read against.
fn cue_flags(painter: &egui::Painter, rect: Rect, wave: &Waveform<'_>, held: Option<usize>) {
    if wave.duration_secs <= 0.0 {
        return;
    }
    let total_ms = wave.duration_secs * 1000.0;

    for (index, cue) in wave.cues.iter().enumerate() {
        let fraction = (cue.time_ms as f64 / total_ms).clamp(0.0, 1.0);
        let across = wave.zoom.across(fraction);
        if !(0.0..=1.0).contains(&across) {
            continue;
        }
        let x = rect.left() + rect.width() * across;
        let mut color = Color32::from_rgb(cue.color[0], cue.color[1], cue.color[2]);
        if held == Some(index) {
            color = theme::text();
        }
        // A memory cue reads upwards from the bottom edge; a hot cue downwards
        // from the top. `edge` is the edge it hangs off and `into` the
        // direction the track is from there, so one set of arithmetic draws
        // both the right way up.
        let memory = cue.letter == 0;
        let (edge, into) = match memory {
            true => (rect.bottom(), -1.0),
            false => (rect.top(), 1.0),
        };

        painter.line_segment(
            [egui::pos2(x, edge), egui::pos2(x, edge + into * (rect.height() - 10.0))],
            Stroke::new(1.5_f32, color),
        );
        // A flag, pointing the way the cue reads: from the marker into the
        // track.
        painter.add(egui::Shape::convex_polygon(
            vec![
                egui::pos2(x - 4.5, edge),
                egui::pos2(x + 6.0, edge),
                egui::pos2(x + 0.75, edge + into * 9.0),
            ],
            color,
            Stroke::NONE,
        ));
        // The name only. What a memory cue is called is a sentence now — "V1
        // Get Down" — and painting that along the panel would write it over
        // the next three cues, so the whole of it is on the hover instead.
        painter.text(
            egui::pos2(x + 8.0, edge + into * 1.0),
            match memory {
                true => egui::Align2::LEFT_BOTTOM,
                false => egui::Align2::LEFT_TOP,
            },
            cue.name(),
            theme::mono(9.5),
            color,
        );
    }
}

/// What resting on a cue says: what it is called, and where it is.
///
/// The marker itself has room for a letter or a dot. A memory cue's name is
/// the thing worth reading — "Drop 2", "V1 Get Down" — and the only place it
/// fits is here.
///
/// It does not add that a memory cue is a memory cue. The marker already says
/// so: it hangs off the bottom edge where a hot cue hangs off the top, and it
/// carries a dot where a hot cue carries its letter. A hot cue's letter is
/// repeated because that is the button that gets pressed.
fn cue_help(cue: &CueMark) -> String {
    let at = crate::app::time_text(cue.time_ms);
    let label = cue.label.trim();
    match (label.is_empty(), cue.letter) {
        // Nothing else to go on, so what it is is all there is to say.
        (true, 0) => format!("memory cue \u{2014} {at}"),
        (true, _) => format!("hot cue {} \u{2014} {at}", cue.name()),
        (false, 0) => format!("{label} \u{2014} {at}"),
        (false, _) => format!("{label} \u{2014} hot cue {}, {at}", cue.name()),
    }
}

/// What the phrase strip was used for this frame.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Strip {
    /// A new view, when the strip was dragged sideways.
    pub zoom: Option<Zoom>,
    /// A change to the sections, when one was dragged, split, merged or named.
    pub edit: Option<PhraseEdit>,
}

/// The phrase strip: one block per section, as wide as the section is long.
///
/// Shows what the waveform above it shows, through the same zoom, so a block
/// sits under the music it names. It used to draw the whole track at every
/// zoom and carry the window as a box over itself, which made it a map but put
/// the drop's block nowhere near the drop as soon as you zoomed in. A strip
/// that lines up with the picture is worth more than a map of a track you can
/// already see the whole of at full width.
///
/// Grab it and drag to move the view, which is what a strip showing a window
/// rather than a whole track can be grabbed for.
///
/// It is also where the sections are corrected. A detector working from onset
/// strength gets a good many boundaries right and some plainly wrong, and a
/// wrong one is worth more than a missing one — it is a lie about where the
/// drop is, on the strip a player draws. So drag a boundary to move it,
/// right-click a block to rename it, and split or merge to put a boundary
/// where the detector did not find one or take away one it invented.
/// What the pointer resting on one section says.
///
/// The name, how long it runs, the rule that gave it that name, and how the
/// number that rule is about is measured. The last two come from the engine that
/// decides, not from here, so an explanation cannot outlive the rule.
fn phrase_help(phrase: &Phrase, beat_ms: &[u32]) -> String {
    use booth_cli::analysis::structure::Kind;

    let length = match bars_of(phrase, beat_ms) {
        Some(bars) => format!(" \u{2014} {}", crate::library::plural(bars, "bar")),
        None => String::new(),
    };
    // Plain capitals, not the letter-spaced form the block is painted with:
    // spacing is what makes a four-letter label read as a heading at nine
    // points, and what makes a sentence of help unreadable.
    let name = phrase.kind.to_uppercase();
    match Kind::from_label(&phrase.kind) {
        Some(kind) => format!("{name}{length}\n\n{}\n\n{}", kind.rules(), Kind::MEASURE),
        // A section renamed by hand, or read off a drive under a name the
        // format has no phrase for. There is no rule behind it, and inventing
        // one would be worse than saying so.
        None => format!("{name}{length}\n\nNamed by hand rather than measured."),
    }
}

/// How much of the strip's height the loudest part of the record fills.
///
/// Not all of it: a silhouette that touches the top edge reads as clipped, and
/// the label has to stay legible over the loud parts.
const SILHOUETTE: f32 = 0.78;

/// Below this much swing between the quiet parts and the loud ones, the
/// picture is stretched to fill the strip.
///
/// A modern club master is limited to within a few decibels of itself from end
/// to end. Drawn honestly against full scale that is a rectangle — true, and
/// it says nothing about where the break is, which is the one thing this strip
/// is read for. Measured on the records this was built against, a dynamic
/// record swings by about 0.6 of full scale across its sections and a limited
/// one by under 0.2.
const FLAT_ENOUGH: f32 = 0.35;

/// How tall the quietest part is drawn after a stretch.
///
/// Not zero. Stretching a flat master onto the full height makes its quietest
/// bar nothing at all, which reads as silence — and silence is a thing the
/// record could have had and does not. A short bar says "quieter"; no bar says
/// something untrue.
const STRETCHED_FLOOR: f32 = 0.18;

/// The level of each pixel of the strip, and whether it had to be stretched to
/// be worth looking at.
///
/// One number per pixel: the loudest column it covers, by the same weighting
/// the drive is written with, so the silhouette here and the picture above it
/// are made of the same measurement.
fn silhouette(bands: &[u8], width: usize, zoom: Zoom) -> (Vec<f32>, bool) {
    let columns = bands.len() / COLUMN;
    if columns == 0 || width == 0 {
        return (Vec::new(), false);
    }
    let column_at = |fraction: f32| {
        ((zoom.into_track(fraction) * columns as f64) as usize).min(columns.saturating_sub(1))
    };
    let mut levels = Vec::with_capacity(width);
    for pixel in 0..width {
        let from = column_at(pixel as f32 / width as f32);
        let to = column_at((pixel + 1) as f32 / width as f32).max(from + 1);
        let mut peak = 0.0f32;
        for index in from..to.min(columns) {
            let at = index * COLUMN;
            let byte = |offset: usize| bands.get(at + offset).copied().unwrap_or(0) as f32 / 255.0;
            peak = peak.max(loudness(byte(2), byte(0), byte(1)));
        }
        levels.push(peak);
    }

    // The ends of the range taken off the fifth and ninety-fifth of the sorted
    // levels rather than off the smallest and largest. One silent gap between
    // tracks, or one column that happens to clip, would otherwise set the whole
    // scale and leave everything else in the middle of it.
    let mut sorted = levels.clone();
    sorted.sort_by(f32::total_cmp);
    let quiet = sorted[sorted.len() * 5 / 100];
    let loud = sorted[(sorted.len() * 95 / 100).min(sorted.len() - 1)];
    let swing = loud - quiet;
    if swing >= FLAT_ENOUGH || swing <= f32::EPSILON {
        return (levels, false);
    }

    for level in &mut levels {
        let within = ((*level - quiet) / swing).clamp(0.0, 1.0);
        *level = STRETCHED_FLOOR + (1.0 - STRETCHED_FLOOR) * within;
    }
    (levels, true)
}

pub fn phrase_strip(
    ui: &mut Ui,
    phrases: &[Phrase],
    duration_secs: f64,
    beat_ms: &[u32],
    zoom: Zoom,
    bands: &[u8],
) -> Strip {
    let width = ui.available_width();
    // Draggable whatever the zoom is: there is nowhere to scroll to at full
    // width, but the sections are edited at any zoom and mostly at none.
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(width, STRIP_HEIGHT), Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    let mut strip = Strip::default();
    // The help for whichever block the pointer is over, filled in while they
    // are drawn and shown once they all have been.
    let mut about: Option<String> = None;
    if phrases.is_empty() || duration_secs <= 0.0 {
        return strip;
    }
    // Measured once for the whole strip rather than per block, because the
    // stretch has to be decided across the record: worked out per section, a
    // break and a drop would each fill their own block and the strip would say
    // they were the same loudness.
    let (levels, stretched) = silhouette(bands, rect.width().max(0.0) as usize, zoom);

    let total_ms = duration_secs * 1000.0;
    // The same two mappings the waveform uses, so a boundary is drawn, grabbed
    // and dropped at the moment it belongs to whatever the view is.
    let across = |ms: u32| {
        let fraction = (ms as f64 / total_ms).clamp(0.0, 1.0);
        rect.left() + rect.width() * zoom.across(fraction)
    };
    let time_at = |x: f32| {
        let across = ((x - rect.left()) / rect.width()).clamp(0.0, 1.0);
        (zoom.into_track(across) * total_ms) as u32
    };

    for phrase in phrases {
        let (from, to) = (across(phrase.start_ms), across(phrase.end_ms));
        // Outside the view: nothing to draw and no name to place.
        if to < rect.left() || from > rect.right() {
            continue;
        }
        // A one-pixel gap between blocks, which is what makes them read as
        // separate phrases rather than as a colour bar.
        let block =
            Rect::from_min_max(egui::pos2(from, rect.top()), egui::pos2(to - 1.0, rect.bottom()));
        if block.width() <= 0.0 {
            continue;
        }
        let color = theme::phrase_color(&phrase.kind);
        // The block, then the shape of the music inside it. The block is dimmed
        // so the silhouette has somewhere to stand out from and the label stays
        // readable over both; it is still the section's own colour, because
        // what the strip is first read for is which part this is.
        painter.rect_filled(block, 0.0, color.gamma_multiply(0.45));

        // Half a waveform rather than a mirrored one. Mirrored, the strip would
        // have thirteen points either side of a centre line and the shape would
        // be mush; standing on the floor it gets the whole height, and the eye
        // reads the top edge as the tune.
        if !levels.is_empty() {
            let floor = block.bottom();
            let reach = block.height() * SILHOUETTE;
            let seen = block.intersect(rect);
            let first = (seen.left() - rect.left()).floor().max(0.0) as usize;
            let last = (seen.right() - rect.left()).ceil().max(0.0) as usize;
            for (pixel, level) in levels.iter().enumerate().take(last.min(levels.len())).skip(first)
            {
                let x = rect.left() + pixel as f32;
                if x < block.left() || x >= block.right() {
                    continue;
                }
                let tall = level * reach;
                if tall <= 0.0 {
                    continue;
                }
                painter.rect_filled(
                    Rect::from_min_max(egui::pos2(x, floor - tall), egui::pos2(x + 1.0, floor)),
                    0.0,
                    color,
                );
            }
        }

        // What this section is and why it came out that way. Only the one the
        // pointer is over: a strip that explained all five at once would be a
        // paragraph nobody reads, and the question somebody actually has is
        // "why is that bit a break".
        //
        // Hung off the strip's own response rather than a widget per block,
        // because the blocks are painted — the strip is one control that can be
        // dragged, and adding a widget per section would take the drag away.
        if response.hover_pos().is_some_and(|at| block.contains(at)) {
            about = Some(phrase_help(phrase, beat_ms));
        }

        // Zoomed far enough in, the section you are inside starts off the left
        // of the panel. Its name goes against that edge rather than off it, so
        // the strip still says where you are.
        let seen = block.intersect(rect);

        // The name and how long it runs for. A DJ builds in eights and
        // sixteens, and "BREAK 16" is the difference between seeing that a
        // breakdown is the usual length and counting the bars to find out.
        //
        // It is the bar marks under the block that are being counted, so the
        // number can be checked against the picture and always comes out the
        // same. A boundary dragged to the middle of a bar loses that bar rather
        // than rounding up to it, which is what the marks show too.
        let label = match bars_of(phrase, beat_ms) {
            Some(bars) => format!("{} {bars}", theme::label_text(&phrase.kind)),
            None => theme::label_text(&phrase.kind),
        };
        // Ink chosen against the block it sits on: an accent is near-white in
        // some schemes and near-black in others, and a fixed dark would be a
        // guess at what the block says in half of them.
        let ink = theme::ink_on(color);
        let galley = painter.layout_no_wrap(label, theme::sans(9.0), ink);
        if galley.size().x + 8.0 < seen.width() {
            painter.galley(
                egui::pos2(seen.left() + 4.0, seen.center().y - galley.size().y / 2.0),
                galley,
                ink,
            );
        }
    }

    // Painted rather than built out of widgets, so this is the only thing that
    // says the strip is there and that it can be worked — and it is what lets a
    // test find it and drag one of its boundaries.
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Other, true, "phrases"));
    // Not while a boundary is being dragged: the help would sit over the very
    // thing being lined up.
    let response = match about.filter(|_| !response.dragged()) {
        Some(about) => response.on_hover_text(about),
        // Still says what the strip is for when the pointer is between blocks
        // or past the end of the record.
        None => response.on_hover_text(match stretched {
            false => "The arrangement, section by section, over the shape of the music. Drag a \
                      boundary to move it, or the strip itself to pan. Rest on a section to \
                      read why it is called what it is."
                .to_string(),
            // Said rather than left to be noticed. A stretched picture is not
            // the same picture as the waveform above it, and somebody
            // comparing the two deserves to know which one they are reading.
            true => "The arrangement, section by section, over the shape of the music — \
                     stretched, because this master runs at nearly one level from end to end \
                     and drawn honestly it would be a rectangle. Heights here are relative to \
                     each other, not to full scale."
                .to_string(),
        }),
    };

    // Which section the pointer is over, and which boundary — if any — it is
    // near enough to take hold of. The first section has no boundary before it:
    // that edge is the start of the track.
    let under = |x: f32| {
        let ms = time_at(x);
        phrases.iter().position(|phrase| ms >= phrase.start_ms && ms < phrase.end_ms)
    };
    let boundary_near = |x: f32| {
        (1..phrases.len())
            .map(|at| (at, (across(phrases[at].start_ms) - x).abs()))
            .filter(|(_, gap)| *gap <= GRAB)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(at, _)| at)
    };

    let held = dragged_boundary(ui, &response, boundary_near);
    if let Some(at) = held {
        strip.edit = Some(PhraseEdit::Move { at, time_ms: time_at(pointer_x(&response, rect)) });
    }

    // Lit while the pointer is on it or dragging it, because a line one pixel
    // wide that does something is a line that has to say so first.
    let lit = held.or_else(|| response.hover_pos().and_then(|at| boundary_near(at.x)));
    if let Some(at) = lit {
        let x = match held == Some(at) {
            true => pointer_x(&response, rect),
            false => across(phrases[at].start_ms),
        };
        painter.line_segment(
            [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
            Stroke::new(1.0_f32, theme::text()),
        );
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
    }

    // The block a right-click was on, remembered: once the menu is open the
    // pointer is over the menu and no longer over anything on the strip.
    let menu_id = response.id.with("phrase-menu");
    if response.secondary_clicked() {
        let on = response.interact_pointer_pos().and_then(|at| under(at.x));
        let ms = response.interact_pointer_pos().map(|at| time_at(at.x));
        ui.ctx().memory_mut(|memory| memory.data.insert_temp(menu_id, (on, ms)));
    }
    let (on, at_ms) = ui
        .ctx()
        .memory(|memory| memory.data.get_temp::<(Option<usize>, Option<u32>)>(menu_id))
        .unwrap_or((None, None));
    response.context_menu(|ui| {
        let (Some(at), Some(time_ms)) = (on, at_ms) else {
            ui.label(egui::RichText::new("no section here").color(theme::dim()));
            return;
        };
        ui.set_min_width(160.0);
        for kind in Phrase::KINDS {
            let picked = phrases[at].kind == kind;
            let label = egui::RichText::new(theme::label_text(kind))
                .color(theme::phrase_color(kind))
                .size(theme::LABEL);
            if ui.radio(picked, label).clicked() {
                strip.edit = Some(PhraseEdit::Name { at, kind: kind.to_string() });
                ui.close();
            }
        }
        ui.separator();
        if ui.button("Split here").on_hover_text("Put a boundary where the pointer is").clicked() {
            strip.edit = Some(PhraseEdit::Split { at, time_ms });
            ui.close();
        }
        // Nothing before the first section to fold it into, and the button
        // saying so is better than one that quietly does nothing.
        if ui
            .add_enabled(at > 0, egui::Button::new("Join to the one before"))
            .on_disabled_hover_text("This is the first section")
            .on_hover_text("Take away the boundary at its left edge")
            .clicked()
        {
            strip.edit = Some(PhraseEdit::Merge { at });
            ui.close();
        }
    });

    // Nothing left to do at full width — there is nowhere to move to — and
    // nothing while a boundary is being dragged, or the section would be moved
    // and the view moved out from under it at once.
    if zoom.is_fit() || held.is_some() {
        return strip;
    }

    if response.hovered() && lit.is_none() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
    }
    // Away from a boundary, a drag moves the view with the pointer: what is
    // under your finger stays under it, which is the only behaviour a strip
    // that draws a window can have without arguing with the picture above it.
    let moved = response.drag_delta().x;
    if response.dragged() && moved != 0.0 {
        strip.zoom = Some(zoom.panned(-moved / rect.width().max(1.0)));
    }
    strip
}

/// Where the pointer is, clamped to the strip.
fn pointer_x(response: &egui::Response, rect: Rect) -> f32 {
    response
        .interact_pointer_pos()
        .map(|at| at.x)
        .unwrap_or(rect.left())
        .clamp(rect.left(), rect.right())
}

/// The boundary a drag took hold of, for as long as the drag lasts.
///
/// Decided once, when the drag starts, and remembered: a boundary that was
/// re-chosen every frame from whatever is under the pointer would hand the
/// drag to its neighbour the moment the two crossed.
///
/// Decided from where the button went *down*, not from where the pointer is by
/// the time egui calls it a drag. A drag is only a drag once the pointer has
/// travelled, so by then it has already left the line it took hold of — and a
/// quick flick, which travels furthest before the first frame reports it, is
/// exactly the drag most likely to miss.
fn dragged_boundary(
    ui: &Ui,
    response: &egui::Response,
    near: impl Fn(f32) -> Option<usize>,
) -> Option<usize> {
    let id = response.id.with("dragging-boundary");
    if response.drag_started() {
        let from = ui
            .ctx()
            .input(|input| input.pointer.press_origin())
            .or_else(|| response.interact_pointer_pos());
        let under = from.and_then(|at| near(at.x));
        ui.ctx().memory_mut(|memory| memory.data.insert_temp(id, under));
    }
    if response.drag_stopped() {
        ui.ctx().memory_mut(|memory| memory.data.remove::<Option<usize>>(id));
        return None;
    }
    if !response.dragged() {
        return None;
    }
    ui.ctx().memory(|memory| memory.data.get_temp::<Option<usize>>(id)).flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bands(columns: &[[u8; 3]]) -> Vec<u8> {
        columns.iter().flatten().copied().collect()
    }

    /// A record whose columns all sit at `level`, except where `loud` says
    /// otherwise. One column a pixel at the widths these tests use.
    fn at_levels(levels: &[u8]) -> Vec<u8> {
        bands(&levels.iter().map(|level| [*level; 3]).collect::<Vec<_>>())
    }

    mod the_shape_under_the_sections {
        use super::*;

        #[test]
        fn a_record_that_already_swings_is_drawn_as_it_is() {
            // A quiet intro, a loud drop. Nothing to fix: the picture is
            // already saying where the record moves.
            let mut levels = vec![20u8; 50];
            levels.extend(std::iter::repeat_n(250u8, 50));
            let (shown, stretched) = silhouette(&at_levels(&levels), 100, Zoom::default());

            assert!(!stretched, "a dynamic record was stretched");
            assert!(shown[10] < 0.4, "the quiet half came out loud: {}", shown[10]);
            assert!(shown[90] > 0.8, "the loud half came out quiet: {}", shown[90]);
        }

        #[test]
        fn a_master_limited_flat_is_stretched_until_it_says_something() {
            // The case this exists for: a club record mastered to within a few
            // decibels of itself from end to end. Drawn honestly it is a
            // rectangle, which is true and tells nobody where the break is.
            let mut levels = vec![200u8; 50];
            levels.extend(std::iter::repeat_n(215u8, 50));
            let (shown, stretched) = silhouette(&at_levels(&levels), 100, Zoom::default());

            assert!(stretched, "a flat master was left as a rectangle");
            assert!(
                shown[90] - shown[10] > 0.5,
                "the stretch did not separate the two halves: {} against {}",
                shown[10],
                shown[90]
            );
        }

        #[test]
        fn a_stretched_picture_never_draws_a_quiet_part_as_silence() {
            // Mapping the quietest part to nothing would say the record has
            // silence in it, which is a thing it could have had and does not.
            let mut levels = vec![200u8; 50];
            levels.extend(std::iter::repeat_n(215u8, 50));
            let (shown, _) = silhouette(&at_levels(&levels), 100, Zoom::default());

            assert!(
                shown.iter().all(|level| *level >= STRETCHED_FLOOR - f32::EPSILON),
                "{shown:?}"
            );
        }

        #[test]
        fn one_silent_gap_does_not_set_the_scale_for_the_whole_record() {
            // The reason the ends of the range are percentiles rather than the
            // smallest and largest. A single column of nothing — a gap between
            // two cuts, a drop-out — would otherwise be the floor, and the
            // quiet half and the loud half would both be squashed into the top
            // of the strip and look alike.
            let mut levels = vec![200u8; 50];
            levels.extend(std::iter::repeat_n(215u8, 49));
            levels[40] = 0;
            let (shown, stretched) = silhouette(&at_levels(&levels), 99, Zoom::default());

            assert!(stretched, "{shown:?}");
            assert!(
                shown[90] - shown[10] > 0.5,
                "one silent column flattened the rest: {} against {}",
                shown[10],
                shown[90]
            );
        }

        #[test]
        fn a_record_at_one_level_from_end_to_end_is_left_alone() {
            // Nothing to reveal. Stretching a picture with no variation in it
            // at all would turn rounding into a shape, which is a picture of
            // nothing presented as a picture of something.
            let (shown, stretched) = silhouette(&at_levels(&[200u8; 99]), 99, Zoom::default());

            assert!(!stretched);
            assert!(shown.windows(2).all(|pair| pair[0] == pair[1]), "{shown:?}");
        }

        #[test]
        fn a_track_with_no_picture_yet_draws_no_shape() {
            let (shown, stretched) = silhouette(&[], 100, Zoom::default());
            assert!(shown.is_empty());
            assert!(!stretched);
        }
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
            zoom: Zoom::default(),
        }
    }

    #[test]
    fn the_help_on_a_section_says_what_it_is_and_why() {
        let break_ = Phrase { start_ms: 0, end_ms: 8_000, kind: "break".into() };
        let beats: Vec<u32> = (0..16).map(|i| i * 500).collect();
        let said = phrase_help(&break_, &beats);

        assert!(
            said.starts_with("BREAK \u{2014} 4 bars"),
            "the name and its length come first: {said}"
        );
        assert!(said.contains("quietest third"), "the rule is not in it: {said}");
        assert!(said.contains("onset strength"), "how it is measured is not in it: {said}");

        // Each kind gets its own rule and not a general description of the
        // strip, which is the whole point of hanging it off the block.
        let drop = Phrase { start_ms: 0, end_ms: 8_000, kind: "drop".into() };
        assert!(phrase_help(&drop, &beats).contains("busiest third"), "{said}");
        assert_ne!(phrase_help(&drop, &beats), said);
    }

    #[test]
    fn a_section_named_by_hand_says_so_rather_than_inventing_a_rule() {
        // A name the format has no phrase for — renamed in the window, or read
        // off a drive somebody else wrote. Nothing measured it, so there is no
        // rule to quote.
        let mine = Phrase { start_ms: 0, end_ms: 4_000, kind: "the bit I like".into() };
        let said = phrase_help(&mine, &[]);
        assert!(said.contains("Named by hand"), "{said}");
        assert!(!said.contains("onset strength"), "{said}");
    }

    use egui_kittest::kittest::Queryable;
    use egui_kittest::Harness;

    /// An even grid, the way the window reconstructs one from a tempo.
    fn grid(length_ms: u32, bpm: f64) -> Vec<u32> {
        let period = 60_000.0 / bpm;
        let count = (length_ms as f64 / period).floor().max(0.0) as usize;
        (0..count).map(|i| (i as f64 * period).round() as u32).collect()
    }

    fn sections(runs: &[(u32, u32, &str)]) -> Vec<Phrase> {
        runs.iter()
            .map(|(start, end, kind)| Phrase {
                start_ms: *start,
                end_ms: *end,
                kind: (*kind).to_string(),
            })
            .collect()
    }

    /// Drive the strip at a given view, and hand back what it asked for.
    ///
    /// The view is fed back in each frame, the way the window does it, so a
    /// drag that moves the view a little at a time adds up over the frames the
    /// gesture takes rather than being measured from a standing start.
    fn worked_at(
        zoom: Zoom,
        phrases: Vec<Phrase>,
        act: impl Fn(&mut egui_kittest::Harness<'_>, Rect),
    ) -> Strip {
        let asked = std::cell::RefCell::new(Strip::default());
        let view = std::cell::Cell::new(zoom);
        let mut harness = Harness::new_ui(|ui| {
            let strip = phrase_strip(ui, &phrases, 90.0, &grid(90_000, 128.0), view.get(), &[]);
            if let Some(moved) = strip.zoom {
                view.set(moved);
                asked.borrow_mut().zoom = Some(moved);
            }
            if strip.edit.is_some() {
                asked.borrow_mut().edit = strip.edit.clone();
            }
        });
        harness.run();
        let rect = harness.get_by_label("phrases").rect();
        act(&mut harness, rect);
        let out = asked.borrow().clone();
        out
    }

    /// Drive the strip at full width, and hand back the edit it asked for.
    fn worked(
        phrases: Vec<Phrase>,
        act: impl Fn(&mut egui_kittest::Harness<'_>, Rect),
    ) -> Option<PhraseEdit> {
        worked_at(Zoom::default(), phrases, act).edit
    }

    #[test]
    fn a_panel_with_nothing_to_draw_reports_the_view_it_was_given() {
        // The caller stores whatever comes back, so a panel that says "fitted"
        // on a frame it drew nothing has thrown somebody's view away. This is
        // why a grid fix used to reset the zoom: it dropped the picture, and
        // the frame in between answered for a panel it had not drawn.
        let close = Zoom { start: 0.25, span: 0.05 };
        let reported = std::cell::Cell::new(Zoom::default());
        let mut harness = Harness::new_ui(|ui| {
            let nothing = Waveform {
                bands: &[],
                duration_secs: 90.0,
                beat_ms: &[],
                cues: &[],
                position: None,
                paint: Default::default(),
                stems: None,
                zoom: close,
            };
            reported.set(show(ui, &nothing).zoom);
        });
        harness.run();

        assert_eq!(reported.get(), close);
    }

    #[test]
    fn dragging_a_phrase_boundary_asks_for_it_to_be_moved() {
        // A ninety-second track with the boundary a third of the way along, so
        // the line to grab is a third of the way across the strip.
        let phrases = sections(&[(0, 30_000, "intro"), (30_000, 90_000, "drop")]);
        let edit = worked(phrases, |harness, rect| {
            let boundary = egui::pos2(rect.left() + rect.width() / 3.0, rect.center().y);
            drag(harness, boundary, boundary + egui::vec2(rect.width() / 9.0, 0.0));
        });
        // Which boundary, not where it landed: where is the grid's business,
        // and the strip does not know the grid.
        assert!(
            matches!(edit, Some(PhraseEdit::Move { at: 1, .. })),
            "the drag did not take hold of the boundary: {edit:?}"
        );
    }

    #[test]
    fn the_number_on_a_section_is_the_number_of_bar_marks_under_it() {
        // The two used to be worked out separately — the marks off the grid,
        // the number off the tempo — and a DJ counting the red lines in a
        // breakdown could get a different answer from the one on the block.
        let beats = grid(180_000, 126.0);
        let phrases = sections(&[
            (0, beats[64], "intro"),
            (beats[64], beats[128], "drop"),
            // The case the two answers came apart on: a section that starts
            // off the bar and runs fifty-nine beats. Fourteen marks fall
            // inside it; the tempo divided by four and rounded to fifteen.
            (beats[129], beats[188], "break"),
        ]);
        let lengths: Vec<Option<usize>> =
            phrases.iter().map(|phrase| bars_of(phrase, &beats)).collect();
        assert_eq!(lengths, vec![Some(16), Some(16), Some(14)]);

        for phrase in &phrases {
            let marks = beats
                .iter()
                .step_by(BEATS_PER_BAR)
                .filter(|at| (phrase.start_ms..phrase.end_ms).contains(at))
                .count();
            assert_eq!(
                bars_of(phrase, &beats),
                Some(marks),
                "{} says one length and the picture draws another",
                phrase.kind
            );
        }
    }

    #[test]
    fn a_section_with_no_grid_under_it_gives_no_length_rather_than_zero() {
        // Nothing is drawn under it either, so a number would be a claim about
        // a picture that is not there.
        let phrases = sections(&[(0, 30_000, "intro")]);
        assert_eq!(bars_of(&phrases[0], &[]), None);
    }

    #[test]
    fn dragging_the_middle_of_a_section_moves_the_view_and_not_the_boundary() {
        // The strip is scrolled as well as edited, and the two gestures share
        // it. Away from a boundary the drag belongs to the view.
        let phrases = sections(&[(0, 30_000, "intro"), (30_000, 90_000, "drop")]);
        let half = Zoom { start: 0.25, span: 0.5 };
        let travel = 40.0_f32;
        let panel = std::cell::Cell::new(0.0_f32);
        let worked = worked_at(half, phrases, |harness, rect| {
            panel.set(rect.width());
            let middle = egui::pos2(rect.left() + rect.width() * 0.7, rect.center().y);
            // Leftwards, which pulls later music into the view.
            drag(harness, middle, middle - egui::vec2(travel, 0.0));
        });
        assert_eq!(worked.edit, None, "a drag in open ground moved a boundary");
        let moved = worked.zoom.expect("a drag in open ground did not move the view");
        assert_eq!(moved.span, half.span, "scrolling the strip changed how much it shows");

        // The view moves by the drag as a fraction of its own width, so what
        // was under the pointer is still under it. Jumping to put the pointer
        // in the middle — which is what the strip did while it was a map —
        // would have landed at 0.45 instead.
        let want = half.start + travel / panel.get() * half.span;
        assert!(
            (moved.start - want).abs() < 0.002,
            "the view moved to {} rather than {want}",
            moved.start
        );
    }

    fn marked(letter: u8, time_ms: u32, label: &str) -> CueMark {
        CueMark { letter, time_ms, label: label.into(), color: [200, 60, 60] }
    }

    #[test]
    fn a_cue_is_named_by_where_it_is_in_the_list_rather_than_by_its_letter() {
        // Every memory cue carries letter zero, so a letter stopped naming one
        // the moment a track could have an arrangement's worth of them. Under
        // the old rule, grabbing any of them named the first — which is the one
        // the grid is anchored to, so dragging "Drop 2" silently re-gridded the
        // record.
        let cues = [marked(0, 0, "Start"), marked(1, 30_000, ""), marked(0, 60_000, "Drop 1")];
        let wave = Waveform { cues: &cues, duration_secs: 120.0, ..wave(&[]) };
        let rect = Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(400.0, 100.0));

        let on_the_last = egui::pos2(cue_x(rect, &wave, 60_000), 50.0);
        assert_eq!(cue_under(on_the_last, rect, &wave), Some(2), "the third cue, not the first");

        let on_the_first = egui::pos2(cue_x(rect, &wave, 0), 50.0);
        assert_eq!(cue_under(on_the_first, rect, &wave), Some(0));
    }

    #[test]
    fn what_resting_on_a_cue_says_is_what_the_marker_has_no_room_for() {
        // The marker is a letter or a dot. "V1 Get Down" is the thing worth
        // reading and the one thing it cannot show.
        let hook = cue_help(&marked(0, 62_500, "V1 Get Down"));
        assert_eq!(hook, "V1 Get Down \u{2014} 1:02.50");

        // A hot cue says which button it is, since that is what gets pressed.
        let hot = cue_help(&marked(2, 1_000, "Drop 1"));
        assert!(hot.contains("hot cue B"), "{hot}");

        // And one nobody named still says what it is rather than nothing.
        let bare = cue_help(&marked(0, 0, "   "));
        assert!(bare.starts_with("memory cue"), "{bare}");
    }

    #[test]
    fn a_memory_cue_is_not_told_it_is_a_memory_cue() {
        // The marker already says so: it hangs off the bottom edge and carries
        // a dot, where a hot cue hangs off the top and carries its letter.
        let named = cue_help(&marked(0, 100_000, "V1 everybody in the room"));
        assert!(!named.contains("memory cue"), "{named}");
        assert!(named.starts_with("V1 everybody in the room"), "{named}");
        assert!(named.contains("1:40.00"), "{named}");
    }

    #[test]
    fn a_section_is_drawn_and_grabbed_where_the_waveform_puts_it() {
        // The strip shows the window, not the whole track, so the boundary at
        // a third of the way in is half way across a view that starts at a
        // sixth and holds a third. Drawn anywhere else it would be pointing at
        // music that is not under it.
        let phrases = sections(&[(0, 30_000, "intro"), (30_000, 90_000, "drop")]);
        let zoom = Zoom { start: 1.0 / 6.0, span: 1.0 / 3.0 };
        let worked = worked_at(zoom, phrases, |harness, rect| {
            let on_the_boundary = egui::pos2(rect.center().x, rect.center().y);
            drag(harness, on_the_boundary, on_the_boundary + egui::vec2(12.0, 0.0));
        });
        let Some(PhraseEdit::Move { at, time_ms }) = worked.edit else {
            panic!("the boundary was not where the zoom draws it: {:?}", worked.edit)
        };
        assert_eq!(at, 1);
        // Twelve points along a panel showing thirty seconds, so the boundary
        // should have landed a little after where it was and nowhere near the
        // place the unzoomed strip would have read.
        assert!(
            (30_000..34_000).contains(&time_ms),
            "the drop landed at {time_ms} ms, which is not where it was dropped"
        );
    }

    #[test]
    fn the_whole_track_showing_leaves_the_view_alone() {
        // There is nowhere to scroll to, and a strip that lurched on every
        // stray drag would be worse than one that does nothing.
        let phrases = sections(&[(0, 30_000, "intro"), (30_000, 90_000, "drop")]);
        let worked = worked_at(Zoom::default(), phrases, |harness, rect| {
            let middle = egui::pos2(rect.left() + rect.width() * 0.7, rect.center().y);
            drag(harness, middle, middle - egui::vec2(40.0, 0.0));
        });
        assert_eq!(worked.zoom, None);
    }

    #[test]
    fn the_menu_on_a_section_offers_every_name_and_a_way_to_cut_it() {
        let phrases = sections(&[(0, 30_000, "intro"), (30_000, 90_000, "drop")]);
        let asked = std::cell::RefCell::new(None);
        let mut harness = Harness::new_ui(|ui| {
            let strip =
                phrase_strip(ui, &phrases, 90.0, &grid(90_000, 128.0), Zoom::default(), &[]);
            if strip.edit.is_some() {
                *asked.borrow_mut() = strip.edit.clone();
            }
        });
        harness.run();
        let rect = harness.get_by_label("phrases").rect();
        let on_the_drop = egui::pos2(rect.left() + rect.width() * 0.7, rect.center().y);

        harness.event(egui::Event::PointerMoved(on_the_drop));
        harness.run();
        for pressed in [true, false] {
            harness.event(egui::Event::PointerButton {
                pos: on_the_drop,
                button: egui::PointerButton::Secondary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            });
            harness.run();
        }

        // Every name is offered, not just the ones this track happens to use:
        // a menu that only lists what is already there cannot correct anything.
        for kind in Phrase::KINDS {
            harness.get_by_label_contains(&theme::label_text(kind));
        }
        harness.get_by_label("Split here");

        harness.get_by_label_contains(&theme::label_text("break")).click();
        harness.run();
        harness.run();
        assert_eq!(
            *asked.borrow(),
            Some(PhraseEdit::Name { at: 1, kind: "break".to_string() }),
            "renaming the section under the pointer did not come back"
        );
    }

    /// Press, travel, release — egui only calls it a drag once the pointer has
    /// actually moved.
    fn drag(harness: &mut Harness<'_>, from: egui::Pos2, to: egui::Pos2) {
        harness.event(egui::Event::PointerMoved(from));
        harness.run();
        harness.event(egui::Event::PointerButton {
            pos: from,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        });
        harness.run();
        for step in 1..=4 {
            harness.event(egui::Event::PointerMoved(from + (to - from) * (step as f32 / 4.0)));
            harness.run();
        }
        harness.event(egui::Event::PointerButton {
            pos: to,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        });
        harness.run();
    }

    #[test]
    fn zooming_holds_the_point_under_the_pointer_still() {
        let zoom = Zoom::default();
        // A quarter of the way across, zoomed in four times: whatever was under
        // the pointer has to still be under the pointer, or you spend the zoom
        // chasing it back.
        let held = zoom.into_track(0.25);
        let closer = zoom.scaled(0.25, 0.25, 0.0);
        assert!((closer.span - 0.25).abs() < 1e-5, "{closer:?}");
        assert!((closer.into_track(0.25) - held).abs() < 1e-4, "{closer:?} moved {held}");

        // And again from somewhere that is not the start of the track.
        let deeper = closer.scaled(0.5, 0.8, 0.0);
        assert!((deeper.into_track(0.8) - closer.into_track(0.8)).abs() < 1e-4, "{deeper:?}");
    }

    #[test]
    fn a_playhead_parked_outside_the_view_does_not_pull_it_back() {
        // The reported bug, in one line: every scroll away from the playhead
        // was undone on the next frame, so dragging the phrase strip stuttered
        // and stayed by the top of the track.
        let looking = Zoom { start: 0.5, span: 0.25 };
        let parked = 0.0;
        assert!(!follows(looking, parked, Some(parked), false));
        // Whole track showing: there is nowhere to pull it back to.
        assert!(!follows(Zoom::default(), parked, Some(0.5), false));
    }

    #[test]
    fn a_playhead_running_out_of_the_view_pulls_it_back() {
        // The behaviour worth keeping: what you zoomed in on to check should
        // not silently be left behind while the record plays on.
        let looking = Zoom { start: 0.5, span: 0.25 };
        assert!(follows(looking, 0.80, Some(0.79), false));
        // Still inside the view, so there is nothing to catch up with.
        assert!(!follows(looking, 0.60, Some(0.59), false));
        // Nothing to compare against on the first frame a track is shown.
        assert!(!follows(looking, 0.80, None, false));
    }

    #[test]
    fn dragging_the_view_stops_the_playhead_pulling_it_around() {
        // Otherwise the same fight happens during playback, where the playhead
        // is moving on its own and would win every frame.
        let looking = Zoom { start: 0.5, span: 0.25 };
        assert!(!follows(looking, 0.80, Some(0.79), true));
    }

    #[test]
    fn the_view_never_leaves_the_track() {
        // Zooming about the far right, then panning past the end.
        let zoom = Zoom::default().scaled(0.1, 1.0, 0.0).panned(5.0);
        assert!(zoom.start >= 0.0 && zoom.start + zoom.span <= 1.0 + 1e-6, "{zoom:?}");
        // And past the start.
        let zoom = zoom.panned(-50.0);
        assert!(zoom.start >= 0.0, "{zoom:?}");
        // Centring on the very first moment cannot push the window negative.
        let zoom = zoom.centred(0.0);
        assert!(zoom.start >= 0.0, "{zoom:?}");
        assert!((zoom.centred(1.0).start + zoom.span - 1.0).abs() < 1e-6);
    }

    #[test]
    fn zooming_out_stops_at_the_whole_track() {
        let zoom = Zoom::default().scaled(4.0, 0.5, 0.0);
        assert!(zoom.is_fit(), "{zoom:?}");
        assert_eq!(zoom.start, 0.0);
    }

    #[test]
    fn the_view_stops_where_the_picture_runs_out() {
        // A five-minute track at the scrolling resolution, on a wide panel.
        let columns = (300.0 * 150.0) as usize;
        let floor = zoom_floor(columns, 900.0);
        // Deep enough to be worth having — a couple of seconds of a five-minute
        // track — and not so deep that the picture is being stretched.
        let visible_secs = floor as f64 * 300.0;
        assert!((0.5..12.0).contains(&visible_secs), "{visible_secs}s at the limit");

        let zoom = Zoom::default().scaled(0.0001, 0.5, floor);
        assert!(zoom.span >= floor - 1e-9, "{zoom:?} went past {floor}");

        // A track with no picture at all cannot be zoomed into at all.
        assert_eq!(zoom_floor(0, 900.0), 1.0);
    }

    #[test]
    fn a_point_outside_the_view_reads_as_outside_it() {
        // What culls the cue flags and the beat ticks. Clamping here instead
        // would stack every cue before the window onto the left edge, which
        // reads as a cluster of cues that is not there.
        let zoom = Zoom { start: 0.4, span: 0.2 };
        assert!(zoom.across(0.1) < 0.0);
        assert!(zoom.across(0.9) > 1.0);
        assert!((zoom.across(0.5) - 0.5).abs() < 1e-6);
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
    fn the_quieter_bands_are_drawn_on_top_of_the_louder_one() {
        // All three bars are centred on the same line, so a band drawn after a
        // taller one is hidden behind it. The bug this is here for: they were
        // drawn low, mid, high regardless of height, and the mid band — which
        // on most music is the tallest — painted over the low band every time.
        // The kick, which is the whole reason for this mode, was never visible.
        let colors = [theme::band_low(), theme::band_mid(), theme::band_high()];

        // A typical column of a modern master: mid loudest, the kick under it.
        let order = stacked([0.37, 1.0, 0.46], colors);
        assert_eq!(order[0].1, theme::band_mid(), "the tallest band must go down first");
        assert_eq!(order[2].1, theme::band_low(), "the kick is still buried");
        assert!(order[0].0 >= order[1].0 && order[1].0 >= order[2].0);

        // And a bass-heavy one, where the old fixed order happened to be right.
        let order = stacked([1.0, 0.4, 0.2], colors);
        assert_eq!(order[0].1, theme::band_low());
        assert_eq!(order[2].1, theme::band_high());
    }

    #[test]
    fn frequency_colour_follows_whichever_band_is_loudest() {
        // A column that is only one band comes out exactly that band's colour.
        assert_eq!(frequency_color(1.0, 0.0, 0.0), FREQ_LOW);
        assert_eq!(frequency_color(0.0, 1.0, 0.0), FREQ_MID);
        assert_eq!(frequency_color(0.0, 0.0, 1.0), FREQ_HIGH);
    }

    #[test]
    fn two_bands_together_land_on_the_colour_that_names_the_pair() {
        // The whole reason the convention is three primaries on three
        // channels, and what every EQ colour chart a DJ has read is drawn
        // from: a column of two bands is the secondary between them, and
        // there is no pair that cancels.
        assert_eq!(frequency_color(1.0, 1.0, 0.0), Color32::from_rgb(255, 255, 0), "yellow");
        assert_eq!(frequency_color(0.0, 1.0, 1.0), Color32::from_rgb(0, 255, 255), "cyan");
        assert_eq!(frequency_color(1.0, 0.0, 1.0), Color32::from_rgb(255, 0, 255), "magenta");
        assert_eq!(frequency_color(1.0, 1.0, 1.0), Color32::WHITE, "all three");
    }

    #[test]
    fn a_quiet_column_is_still_the_colour_of_what_is_in_it() {
        // The height already says how loud it is. A colour that said it again
        // would leave a breakdown too dark to read for the sake of repeating
        // something the shape has already shown.
        let loud = frequency_color(0.8, 0.08, 0.02);
        let quiet = frequency_color(0.2, 0.02, 0.005);
        assert_eq!(loud, quiet, "the same balance at two volumes is the same colour");

        // And silence is not black, which would be a column that looks like a
        // hole in the picture.
        assert_eq!(frequency_color(0.0, 0.0, 0.0), theme::rule());
    }

    /// Everything that crosses between a moment in the track and a place on
    /// the panel has to go through the zoom, or the picture and the things
    /// drawn over it stop agreeing.
    mod zoomed_in {
        use super::*;

        const TOTAL: f64 = 240.0;

        fn cue(letter: u8, time_ms: u32) -> CueMark {
            CueMark { letter, time_ms, label: String::new(), color: [1, 2, 3] }
        }

        fn panel() -> Rect {
            Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1_000.0, 100.0))
        }

        /// Showing the second quarter of the track: 60 s to 120 s.
        fn wave<'a>(cues: &'a [CueMark], beats: &'a [u32]) -> Waveform<'a> {
            Waveform {
                bands: &[],
                duration_secs: TOTAL,
                beat_ms: beats,
                cues,
                position: None,
                paint: Paint::Bands,
                stems: None,
                zoom: Zoom { start: 0.25, span: 0.25 },
            }
        }

        #[test]
        fn a_cue_is_grabbed_where_it_is_drawn() {
            // They were two different mappings: one through the zoom and one
            // not. A cue was drawn in the right place and grabbed in another,
            // and zoomed in the gap between them was the whole panel.
            let cues = [cue(1, 90_000)];
            let wave = wave(&cues, &[]);
            let rect = panel();

            // 90 s is halfway through a window running 60 s to 120 s.
            let x = cue_x(rect, &wave, 90_000);
            assert!((x - rect.center().x).abs() < 0.5, "drawn at {x}");
            // The only cue in the list, so its place is the first one.
            assert_eq!(cue_under(egui::pos2(x, 50.0), rect, &wave), Some(0));
        }

        #[test]
        fn a_cue_outside_the_window_cannot_be_grabbed_through_it() {
            // It is not drawn, so grabbing it would be grabbing something
            // invisible — and before this it was grabbable at whatever place
            // the unzoomed mapping happened to put it.
            let cues = [cue(1, 10_000)];
            let wave = wave(&cues, &[]);
            let rect = panel();
            for x in [0.0, 250.0, 500.0, 750.0, 999.0] {
                assert_eq!(cue_under(egui::pos2(x, 50.0), rect, &wave), None, "grabbed at {x}");
            }
        }

        #[test]
        fn a_click_lands_on_the_moment_under_the_pointer() {
            let wave = wave(&[], &[]);
            let rect = panel();
            // The window runs 60 s to 120 s across a thousand points.
            assert_eq!(time_at(rect, &wave, rect.left()), 60_000);
            assert_eq!(time_at(rect, &wave, rect.center().x), 90_000);
            assert_eq!(time_at(rect, &wave, rect.right()), 120_000);
        }

        #[test]
        fn a_moment_survives_the_round_trip_through_the_panel() {
            let wave = wave(&[], &[]);
            let rect = panel();
            for at in [60_000u32, 75_000, 90_000, 119_000] {
                let back = time_at(rect, &wave, cue_x(rect, &wave, at));
                assert!(back.abs_diff(at) <= 120, "{at} came back as {back}");
            }
        }

        #[test]
        fn the_whole_track_is_the_same_mapping_it_always_was() {
            // Zoomed out, nothing should have changed.
            let cues = [cue(1, 120_000)];
            let mut wave = wave(&cues, &[]);
            wave.zoom = Zoom::default();
            let rect = panel();

            assert!((cue_x(rect, &wave, 120_000) - rect.center().x).abs() < 0.5);
            assert_eq!(time_at(rect, &wave, rect.center().x), 120_000);
        }
    }

    #[test]
    fn the_grid_marks_the_downbeat_in_the_colour_a_player_uses() {
        // Red on the one and white on the other three is what a CDJ shows, so
        // it is what a DJ reads without having to think about it.
        assert_ne!(DOWNBEAT, OFFBEAT);
        assert!(DOWNBEAT.r() > DOWNBEAT.g() && DOWNBEAT.r() > DOWNBEAT.b(), "{DOWNBEAT:?}");
        let spread = |c: Color32| {
            let v = [c.r(), c.g(), c.b()];
            v.iter().max().unwrap() - v.iter().min().unwrap()
        };
        assert!(spread(OFFBEAT) < 20, "the off-beats should read as white: {OFFBEAT:?}");
    }

    /// Where a colour sits on the wheel, in degrees, and how far from grey.
    fn hue(colour: Color32) -> (f32, f32) {
        let (r, g, b) =
            (colour.r() as f32 / 255.0, colour.g() as f32 / 255.0, colour.b() as f32 / 255.0);
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let spread = max - min;
        if spread < 1e-6 {
            return (0.0, 0.0);
        }
        let degrees = if max == r {
            60.0 * (((g - b) / spread) % 6.0)
        } else if max == g {
            60.0 * ((b - r) / spread + 2.0)
        } else {
            60.0 * ((r - g) / spread + 4.0)
        };
        ((degrees + 360.0) % 360.0, spread / max)
    }

    #[test]
    fn a_column_with_bass_under_it_is_a_different_colour_from_one_without() {
        // The fault this is here for: it was not. The palette was blue bass
        // and amber mid, which sit opposite each other on the wheel, so a
        // column holding both cancelled to grey — a body with a strong bass
        // and a breakdown with none came out at hue 36.0 and hue 35.8, the
        // same colour for the two passages a DJ most needs to tell apart.
        // Red, green and blue have no opposite pair among them, which is why
        // this is the convention rather than a matter of taste.
        //
        // The levels here are as they are stored, bent by the display curve.
        let body = frequency_color(0.59, 0.86, 0.49);
        let breakdown = frequency_color(0.09, 0.95, 0.57);
        let bass = frequency_color(0.95, 0.71, 0.08);

        let apart = |a: Color32, b: Color32| {
            let gap = (hue(a).0 - hue(b).0).abs();
            gap.min(360.0 - gap)
        };
        assert!(
            apart(body, breakdown) > 15.0,
            "a body and a breakdown are {:.1} degrees apart",
            apart(body, breakdown)
        );
        // A sixth of the wheel apart is the bar: red-orange against green is
        // not a shade, it is a different colour. Measured at 83.
        assert!(
            apart(bass, body) > 60.0,
            "bass and a full-band body are {:.1} degrees apart",
            apart(bass, body)
        );

        // And they are colours, not shades of beige.
        for (name, colour) in [("body", body), ("breakdown", breakdown), ("bass", bass)] {
            assert!(hue(colour).1 > 0.3, "{name} came out {:.2} from grey", hue(colour).1);
        }
    }

    /// How far a colour is from the grey of the same brightness.
    fn colourfulness(colour: Color32) -> f32 {
        let rgb = [colour.r() as f32, colour.g() as f32, colour.b() as f32];
        let grey = 0.299 * rgb[0] + 0.587 * rgb[1] + 0.114 * rgb[2];
        rgb.iter().map(|c| (c - grey).abs()).fold(0.0f32, f32::max)
    }

    #[test]
    fn an_ordinary_column_is_a_colour_and_not_a_shade_of_grey() {
        // What a kick drum measures as, once the square root in the band bytes
        // has pulled the three levels together: plainly bass, and nowhere near
        // pure. Averaging these three in proportion is what made every column
        // of every record come out the same beige.
        let kick = frequency_color(0.95, 0.63, 0.32);
        assert!(
            colourfulness(kick) > 30.0,
            "a bass-heavy column should read as bass: {kick:?} is {:.0} off grey",
            colourfulness(kick)
        );
        let leads = |colour: Color32| {
            [colour.r(), colour.g(), colour.b()].iter().copied().enumerate().max_by_key(|(_, v)| *v)
        };
        assert_eq!(leads(kick).map(|(i, _)| i), Some(0), "the low band is the red one: {kick:?}");

        // The same shape with the mid-range winning has to be visibly a
        // different colour, not a different shade of the same one. Measured
        // round the wheel rather than down one channel, because "which channel
        // is biggest" is a weaker claim than "these are different colours".
        let lead = frequency_color(0.32, 0.95, 0.63);
        assert_eq!(leads(lead).map(|(i, _)| i), Some(1), "the mid band is the green one: {lead:?}");
        let gap = (hue(kick).0 - hue(lead).0).abs();
        let apart = gap.min(360.0 - gap);
        // Two primaries apart, which is what the convention is for. Measured
        // at exactly 120.
        assert!(apart > 90.0, "the two are {apart:.0} degrees apart: {kick:?} vs {lead:?}");
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
        assert_eq!(envelopes.color_at(0), theme::stem_vocals());
        assert_eq!(envelopes.color_at(1), theme::stem_melody());
        assert_eq!(envelopes.color_at(2), theme::stem_drums());
    }

    #[test]
    fn a_column_with_no_stem_playing_is_not_a_third_of_each() {
        let envelopes = StemEnvelopes { vocals: vec![0], melody: vec![0], drums: vec![0] };
        // A blend of three colours at zero weight would be black, which reads
        // as "nothing here" only by accident; this says it deliberately.
        assert_eq!(envelopes.color_at(0), theme::rule());
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
