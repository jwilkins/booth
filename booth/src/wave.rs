//! The waveform under the browser.
//!
//! It paints the same three-band picture the player will draw, from the same
//! bytes the analysis file will carry — so what is on screen while prepping is
//! what will be on the CDJ's screen, rather than a second rendering that agrees
//! with it approximately.

use eframe::egui::{self, Color32, Rect, Sense, Stroke, Ui, Vec2};

use booth_cli::export::waveform::loudness;

use crate::library::{CueMark, Phrase, PhraseEdit};
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

    /// The colour of one column: whichever stem is doing the most, tinted by
    /// the others. A column with nothing in it is left grey rather than being
    /// given a third of each.
    fn color_at(&self, index: usize) -> Color32 {
        let at = |plane: &[u8]| plane.get(index).copied().unwrap_or(0) as f32;
        let (vocals, melody, drums) = (at(&self.vocals), at(&self.melody), at(&self.drums));
        if vocals + melody + drums < 1.0 {
            return theme::RULE;
        }
        mix([theme::STEM_VOCALS, theme::STEM_MELODY, theme::STEM_DRUMS], [vocals, melody, drums])
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
        return theme::RULE;
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

/// How many bars a phrase runs for, at this tempo.
///
/// `None` when there is no grid to count against, or when the phrase is shorter
/// than a bar: a section labelled "0" says less than one with no number at all.
fn bars_of(phrase: &Phrase, bpm: f64) -> Option<usize> {
    if bpm <= 0.0 {
        return None;
    }
    let beats = phrase.len_ms() as f64 / (60_000.0 / bpm);
    let bars = (beats / 4.0).round() as usize;
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
    Moved { letter: u8, time_ms: u32 },
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
        return Shown { touched: None, zoom: Zoom::default() };
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
    let bands = [theme::BAND_LOW, theme::BAND_MID, theme::BAND_HIGH];
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
                    .unwrap_or(theme::RULE);
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
                Stroke::new(1.0_f32, theme::TEXT),
            );
        }
    }

    if !zoom.is_fit() {
        painter.text(
            egui::pos2(rect.right() - 6.0, rect.bottom() - 4.0),
            egui::Align2::RIGHT_BOTTOM,
            format!("{:.0}\u{d7}", 1.0 / zoom.span),
            theme::mono(9.5),
            theme::DIM,
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

    let touched = response.interact_pointer_pos().and_then(|at| {
        let time_ms = time_at(rect, wave, at.x);
        match held {
            Some(letter) => Some(Touched::Moved { letter, time_ms }),
            // A drag that started on empty space is a scrub, not a cue move.
            None if response.dragged() || response.clicked() => Some(Touched::Scrubbed(time_ms)),
            None => None,
        }
    });
    Shown { touched, zoom }
}

/// Read the wheel over the panel, and keep a playing track in view.
///
/// Following the playhead only kicks in once it has actually left the view.
/// Recentring on every frame would make a zoomed picture scroll continuously,
/// which is a different instrument — what is wanted here is that the thing you
/// zoomed in on to check does not vanish while you listen to it.
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
        if !zoom.is_fit() && !(0.0..=1.0).contains(&zoom.across(position as f64)) {
            zoom = zoom.centred(position);
        }
    }
    zoom
}

/// Where a horizontal position falls in the track.
fn time_at(rect: Rect, wave: &Waveform<'_>, x: f32) -> u32 {
    let fraction = ((x - rect.left()) / rect.width()).clamp(0.0, 1.0) as f64;
    (fraction * wave.duration_secs * 1000.0).round() as u32
}

/// Which cue the pointer is over, if any.
fn cue_under(at: egui::Pos2, rect: Rect, wave: &Waveform<'_>) -> Option<u8> {
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
            Stroke::new(1.0_f32, faint),
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
        let fraction = (cue.time_ms as f64 / total_ms).clamp(0.0, 1.0);
        let across = wave.zoom.across(fraction);
        if !(0.0..=1.0).contains(&across) {
            continue;
        }
        let x = rect.left() + rect.width() * across;
        let mut color = Color32::from_rgb(cue.color[0], cue.color[1], cue.color[2]);
        if held == Some(cue.letter) {
            color = theme::TEXT;
        }

        painter.line_segment(
            [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom() - 10.0)],
            Stroke::new(1.5_f32, color),
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

/// What the phrase strip was used for this frame.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Strip {
    /// A new view, when the map was used to move the window.
    pub zoom: Option<Zoom>,
    /// A change to the sections, when one was dragged, split, merged or named.
    pub edit: Option<PhraseEdit>,
}

/// The phrase strip: one block per section, as wide as the section is long.
///
/// The strip always shows the whole track, whatever the waveform above it is
/// showing, which makes it the map: `zoom` is drawn over it as the window, and
/// dragging inside a block moves that window.
///
/// It is also where the sections are corrected. A detector working from onset
/// strength gets a good many boundaries right and some plainly wrong, and a
/// wrong one is worth more than a missing one — it is a lie about where the
/// drop is, on the strip a player draws. So drag a boundary to move it,
/// right-click a block to rename it, and split or merge to put a boundary
/// where the detector did not find one or take away one it invented.
pub fn phrase_strip(
    ui: &mut Ui,
    phrases: &[Phrase],
    duration_secs: f64,
    bpm: f64,
    zoom: Zoom,
) -> Strip {
    let width = ui.available_width();
    // Draggable whatever the zoom is: the map only means something zoomed in,
    // but the sections are edited at any zoom and mostly at none.
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(width, STRIP_HEIGHT), Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    let mut strip = Strip::default();
    if phrases.is_empty() || duration_secs <= 0.0 {
        return strip;
    }
    let total_ms = duration_secs * 1000.0;
    let across = |ms: u32| rect.left() + rect.width() * (ms as f64 / total_ms) as f32;
    let time_at =
        |x: f32| (((x - rect.left()) / rect.width()).clamp(0.0, 1.0) as f64 * total_ms) as u32;

    for phrase in phrases {
        let (from, to) = (across(phrase.start_ms), across(phrase.end_ms));
        // A one-pixel gap between blocks, which is what makes them read as
        // separate phrases rather than as a colour bar.
        let block =
            Rect::from_min_max(egui::pos2(from, rect.top()), egui::pos2(to - 1.0, rect.bottom()));
        if block.width() <= 0.0 {
            continue;
        }
        let color = theme::phrase_color(&phrase.kind);
        painter.rect_filled(block, 0.0, color);

        // The name and how long it runs for. A DJ builds in eights and
        // sixteens, and "BREAK 16" is the difference between seeing that a
        // breakdown is the usual length and counting the bars to find out.
        //
        // Rounded, because a phrase is a musical length and a boundary dragged
        // by hand lands on a bar rather than on an exact multiple of one.
        let label = match bars_of(phrase, bpm) {
            Some(bars) => format!("{} {bars}", theme::label_text(&phrase.kind)),
            None => theme::label_text(&phrase.kind),
        };
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

    // Painted rather than built out of widgets, so this is the only thing that
    // says the strip is there and that it can be worked — and it is what lets a
    // test find it and drag one of its boundaries.
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Other, true, "phrases"));

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
            Stroke::new(1.0_f32, theme::TEXT),
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
            ui.label(egui::RichText::new("no section here").color(theme::DIM));
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

    if zoom.is_fit() || held.is_some() {
        return strip;
    }

    // The window, drawn by dimming everything outside it rather than by
    // outlining it: the phrase colours are the thing being pointed at, and a
    // box around them competes with them for the same edge.
    let shade = theme::BOOTH.gamma_multiply(0.72);
    let left = rect.left() + rect.width() * zoom.start;
    let right = rect.left() + rect.width() * (zoom.start + zoom.span).min(1.0);
    painter.rect_filled(
        Rect::from_min_max(rect.left_top(), egui::pos2(left, rect.bottom())),
        0.0,
        shade,
    );
    painter.rect_filled(
        Rect::from_min_max(egui::pos2(right, rect.top()), rect.right_bottom()),
        0.0,
        shade,
    );
    painter.rect_stroke(
        Rect::from_min_max(egui::pos2(left, rect.top()), egui::pos2(right, rect.bottom())),
        0.0,
        Stroke::new(1.0_f32, theme::TEXT.gamma_multiply(0.75)),
        egui::StrokeKind::Inside,
    );

    if response.hovered() && lit.is_none() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
    }
    // Clicking the map puts the window where you clicked, which is the whole
    // point of having one: getting from the intro to the last drop should not
    // be a scroll.
    let Some(at) = response.interact_pointer_pos() else { return strip };
    if response.dragged() || response.clicked() {
        let across = ((at.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
        strip.zoom = Some(zoom.centred(across));
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

    use egui_kittest::kittest::Queryable;
    use egui_kittest::Harness;

    fn sections(runs: &[(u32, u32, &str)]) -> Vec<Phrase> {
        runs.iter()
            .map(|(start, end, kind)| Phrase {
                start_ms: *start,
                end_ms: *end,
                kind: (*kind).to_string(),
            })
            .collect()
    }

    /// Drive the strip, and hand back what it asked for.
    fn worked(
        phrases: Vec<Phrase>,
        act: impl Fn(&mut egui_kittest::Harness<'_>, Rect),
    ) -> Option<PhraseEdit> {
        let asked = std::cell::RefCell::new(None);
        let mut harness = Harness::new_ui(|ui| {
            let strip = phrase_strip(ui, &phrases, 90.0, 128.0, Zoom::default());
            if strip.edit.is_some() {
                *asked.borrow_mut() = strip.edit.clone();
            }
        });
        harness.run();
        let rect = harness.get_by_label("phrases").rect();
        act(&mut harness, rect);
        let out = asked.borrow().clone();
        out
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
    fn dragging_the_middle_of_a_section_moves_the_view_and_not_the_boundary() {
        // The strip is a map as well as an editor, and the two gestures share
        // it. Away from a boundary the drag belongs to the map.
        let phrases = sections(&[(0, 30_000, "intro"), (30_000, 90_000, "drop")]);
        let edit = worked(phrases, |harness, rect| {
            let middle = egui::pos2(rect.left() + rect.width() * 0.7, rect.center().y);
            drag(harness, middle, middle + egui::vec2(20.0, 0.0));
        });
        assert_eq!(edit, None, "a drag in open ground moved a boundary");
    }

    #[test]
    fn the_menu_on_a_section_offers_every_name_and_a_way_to_cut_it() {
        let phrases = sections(&[(0, 30_000, "intro"), (30_000, 90_000, "drop")]);
        let asked = std::cell::RefCell::new(None);
        let mut harness = Harness::new_ui(|ui| {
            let strip = phrase_strip(ui, &phrases, 90.0, 128.0, Zoom::default());
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
        let colors = [theme::BAND_LOW, theme::BAND_MID, theme::BAND_HIGH];

        // A typical column of a modern master: mid loudest, the kick under it.
        let order = stacked([0.37, 1.0, 0.46], colors);
        assert_eq!(order[0].1, theme::BAND_MID, "the tallest band must go down first");
        assert_eq!(order[2].1, theme::BAND_LOW, "the kick is still buried");
        assert!(order[0].0 >= order[1].0 && order[1].0 >= order[2].0);

        // And a bass-heavy one, where the old fixed order happened to be right.
        let order = stacked([1.0, 0.4, 0.2], colors);
        assert_eq!(order[0].1, theme::BAND_LOW);
        assert_eq!(order[2].1, theme::BAND_HIGH);
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
        assert_eq!(frequency_color(0.0, 0.0, 0.0), theme::RULE);
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
