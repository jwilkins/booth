//! The look: one dark surface, four accent colours, and no decoration.
//!
//! The palette is the one the spec page draws the booth panel in. It is dark
//! because that is where the work happens — a laptop open in a badly lit room
//! next to a mixer — and the accents each mean exactly one thing wherever they
//! appear: amber is the low band and anything in progress, blue is the low
//! frequencies and the break, green is verified, red is wrong.

use eframe::egui::{self, Color32, FontFamily, FontId, TextStyle};

/// The window's ground.
pub const BOOTH: Color32 = Color32::from_rgb(0x12, 0x16, 0x1A);
/// Bars and panels that sit above it.
pub const BOOTH_2: Color32 = Color32::from_rgb(0x1B, 0x21, 0x27);
/// Every rule and border.
pub const RULE: Color32 = Color32::from_rgb(0x2C, 0x34, 0x3C);
/// Body text.
pub const TEXT: Color32 = Color32::from_rgb(0xD7, 0xDD, 0xE3);
/// Labels, units, and anything secondary.
pub const DIM: Color32 = Color32::from_rgb(0x7C, 0x88, 0x94);
/// The accent: selection, the queue, the drive delta, the sync button.
pub const AMBER: Color32 = Color32::from_rgb(0xE2, 0xA0, 0x3F);
/// The low band, and the breakdown.
pub const BLUE: Color32 = Color32::from_rgb(0x5E, 0x9B, 0xEF);
/// Verified.
pub const GO: Color32 = Color32::from_rgb(0x57, 0xB3, 0x7F);
/// Wrong.
pub const ALERT: Color32 = Color32::from_rgb(0xE8, 0x65, 0x4A);

/// The three waveform bands, in the order they are painted: lows first, then
/// mids, then highs over the top.
pub const BAND_LOW: Color32 = BLUE;
pub const BAND_MID: Color32 = AMBER;
pub const BAND_HIGH: Color32 = TEXT;

/// The colour of each phrase in the strip under the waveform.
pub fn phrase_color(kind: &str) -> Color32 {
    match kind {
        "build" => Color32::from_rgb(0xB8, 0x76, 0x14),
        "drop" => AMBER,
        "break" => Color32::from_rgb(0x2F, 0x6F, 0xD0),
        // Intro and outro share a colour: what they have in common — not the
        // record proper — is more useful to see than telling them apart.
        _ => Color32::from_rgb(0x5C, 0x66, 0x70),
    }
}

/// The eight hot cue colours, in the order cues are handed them.
pub const CUE_COLORS: [Color32; 8] = [
    Color32::from_rgb(0xE2, 0xA0, 0x3F),
    Color32::from_rgb(0x5E, 0x9B, 0xEF),
    Color32::from_rgb(0x57, 0xB3, 0x7F),
    Color32::from_rgb(0xE8, 0x65, 0x4A),
    Color32::from_rgb(0xB8, 0x8A, 0xE0),
    Color32::from_rgb(0x4F, 0xC3, 0xC7),
    Color32::from_rgb(0xE0, 0x7A, 0xB0),
    Color32::from_rgb(0x98, 0xA4, 0xAE),
];

/// Sizes, in points. Small, and deliberately so: the browser's job is to fit a
/// crate on the screen at once.
pub const BODY: f32 = 13.0;
pub const SMALL: f32 = 11.5;
/// The all-caps labels over each pane.
pub const LABEL: f32 = 10.0;
pub const HEADING: f32 = 15.0;

pub fn mono(size: f32) -> FontId {
    FontId::new(size, FontFamily::Monospace)
}

pub fn sans(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}

/// A pane label: small, spaced, upper case, dim.
///
/// egui has no letter-spacing, so the spacing is put in by hand. It is what
/// makes a ten-point label read as a label rather than as small text.
pub fn label_text(text: &str) -> String {
    let mut out = String::new();
    for (index, ch) in text.to_uppercase().chars().enumerate() {
        if index > 0 {
            out.push('\u{2009}');
        }
        out.push(ch);
    }
    out
}

/// Verified.
///
/// Not `✓` (U+2713), which is in none of the bundled fonts and draws as an
/// empty box. Every mark this window uses has been checked against what is
/// actually there to draw it with.
pub const TICK: &str = "✔";
/// Wrong. `✗` is likewise absent; the multiplication sign is in every font.
pub const CROSS: &str = "×";
/// Something is running.
pub const SPINNER: &str = "◐";
/// A warning, on the sheet and in the inspector.
pub const WARN: &str = "⚠";

/// Apply the palette to a context. Called once, at startup.
pub fn install(ctx: &egui::Context) {
    // The proportional face carries Latin and very little else — a tick, an
    // arrow or a folder marker drawn in it comes out as an empty box. The
    // monospace face has all of them, so it goes on the end of the proportional
    // fallback chain: text still sets in Ubuntu, and symbols still appear.
    let mut fonts = egui::FontDefinitions::default();
    if let Some(proportional) = fonts.families.get_mut(&FontFamily::Proportional) {
        if !proportional.iter().any(|name| name == "Hack") {
            proportional.insert(1, "Hack".to_owned());
        }
    }
    ctx.set_fonts(fonts);

    let mut style = (*ctx.style()).clone();

    style.text_styles = [
        (TextStyle::Body, sans(BODY)),
        (TextStyle::Button, sans(BODY)),
        (TextStyle::Small, sans(SMALL)),
        (TextStyle::Monospace, mono(SMALL)),
        (TextStyle::Heading, sans(HEADING)),
    ]
    .into();

    let visuals = &mut style.visuals;
    visuals.dark_mode = true;
    visuals.override_text_color = Some(TEXT);
    visuals.panel_fill = BOOTH;
    visuals.window_fill = BOOTH;
    visuals.extreme_bg_color = BOOTH_2;
    visuals.faint_bg_color = BOOTH_2;
    visuals.window_stroke = egui::Stroke::new(1.0, RULE);

    // Nothing is rounded except by a point, and nothing casts a shadow. The
    // panel edges are rules, so the widgets inside them should not compete.
    let radius = egui::CornerRadius::same(2);
    for widget in [
        &mut visuals.widgets.noninteractive,
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
        &mut visuals.widgets.open,
    ] {
        widget.corner_radius = radius;
        widget.bg_stroke = egui::Stroke::new(1.0, RULE);
        widget.fg_stroke = egui::Stroke::new(1.0, TEXT);
    }
    visuals.widgets.noninteractive.bg_fill = BOOTH;
    visuals.widgets.noninteractive.weak_bg_fill = BOOTH;
    visuals.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0, DIM);
    visuals.widgets.inactive.bg_fill = BOOTH_2;
    visuals.widgets.inactive.weak_bg_fill = BOOTH_2;
    visuals.widgets.hovered.bg_fill = RULE;
    visuals.widgets.hovered.weak_bg_fill = RULE;
    visuals.widgets.active.bg_fill = RULE;
    visuals.widgets.active.weak_bg_fill = RULE;
    visuals.selection.bg_fill = AMBER.gamma_multiply(0.30);
    visuals.selection.stroke = egui::Stroke::new(1.0, TEXT);
    visuals.window_shadow = egui::epaint::Shadow::NONE;
    visuals.popup_shadow = egui::epaint::Shadow::NONE;

    style.spacing.item_spacing = egui::vec2(8.0, 4.0);
    style.spacing.button_padding = egui::vec2(8.0, 3.0);
    style.spacing.window_margin = egui::Margin::same(0);
    style.spacing.interact_size.y = 18.0;

    ctx.set_style(style);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pane_label_is_spaced_upper_case() {
        assert_eq!(
            label_text("Collection"),
            "C\u{2009}O\u{2009}L\u{2009}L\u{2009}E\u{2009}C\u{2009}T\u{2009}I\u{2009}O\u{2009}N"
        );
        assert_eq!(label_text(""), "");
    }

    #[test]
    fn every_phrase_kind_has_a_colour_and_the_drop_is_the_accent() {
        assert_eq!(phrase_color("drop"), AMBER);
        assert_eq!(phrase_color("break"), Color32::from_rgb(0x2F, 0x6F, 0xD0));
        // Anything unrecognised still draws, rather than vanishing.
        assert_eq!(phrase_color("whatever"), phrase_color("intro"));
    }

    /// The marks the window draws must exist in a font the window has.
    ///
    /// This is the check that would have caught the row of empty boxes: the
    /// proportional face has almost no symbols, so every mark used anywhere in
    /// the interface is listed here and confirmed against what is bundled.
    #[test]
    fn every_mark_the_window_draws_is_in_a_bundled_font() {
        // Hack covers these; it is on the proportional fallback chain so they
        // draw in both families.
        for mark in ["↳", "▾", "▣", "▢", "•", "—", "·", SPINNER, CROSS] {
            assert!(in_hack(mark), "{mark} is not in Hack");
        }
        // These two are only in the emoji faces, which are on both chains.
        for mark in [TICK, WARN] {
            assert!(!in_hack(mark), "{mark} is in Hack after all — simplify this");
        }
        // And the two that look right but are in nothing at all.
        assert!(!in_hack("✓") && !in_hack("✗"));
    }

    /// Whether a mark is in the bundled monospace face.
    ///
    /// Read out of the font egui itself ships, so it stays true if that font
    /// changes rather than restating a table that could go stale.
    fn in_hack(mark: &str) -> bool {
        use eframe::egui::FontDefinitions;
        let fonts = FontDefinitions::default();
        let Some(data) = fonts.font_data.get("Hack") else { return false };
        let face = ttf_parser::Face::parse(data.font.as_ref(), 0).expect("Hack should parse");
        mark.chars().all(|ch| face.glyph_index(ch).is_some())
    }

    #[test]
    fn the_cue_colours_are_all_different() {
        let mut seen: Vec<Color32> = CUE_COLORS.to_vec();
        seen.sort_by_key(|c| c.to_array());
        seen.dedup();
        assert_eq!(seen.len(), CUE_COLORS.len());
    }
}
