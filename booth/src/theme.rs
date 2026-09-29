//! The look: one dark surface, four accent colours, and no decoration.
//!
//! The shape of the palette is the one the spec page draws the booth panel in.
//! It is dark because that is where the work happens — a laptop open in a badly
//! lit room next to a mixer — and the accents each mean exactly one thing
//! wherever they appear: amber is the low band and anything in progress, blue
//! is the low frequencies and the break, green is verified, red is wrong.
//!
//! Which colours fill those roles is a [`Scheme`], and there are eleven: this
//! program's own, and ten of the colour schemes people already stare at all day
//! in an editor. A DJ who has spent ten years in gruvbox should not have to
//! learn a second set of colours to read a waveform.
//!
//! **Two things here are deliberately not themed.** [`CUE_COLORS`] is written
//! into the cues and goes on the drive, so it is data, not decoration: theming
//! it would repaint every cue on a player and mark every track changed on the
//! next sync. And the waveform's own red/green/blue bands, over in
//! [`crate::wave`], are the convention every other program draws frequency in —
//! a scheme that recoloured them would be a picture that no longer means what
//! every other picture means.

use std::cell::Cell;

use eframe::egui::{self, Color32, FontFamily, FontId, TextStyle};

/// One colour scheme, in the roles this window draws with.
///
/// The fields are the roles rather than the scheme's own names, because what
/// this program needs is somewhere to put a warning, not a `nord13`. Each
/// scheme fills them from its published palette; nothing here is invented
/// except where a scheme genuinely has no colour for a role, which is noted
/// where it happens.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Scheme {
    /// What to call it in the settings, and what gets stored.
    pub name: &'static str,
    /// The window's ground.
    pub booth: Color32,
    /// Bars and panels that sit above it.
    pub booth_2: Color32,
    /// Every rule and border.
    pub rule: Color32,
    /// Body text.
    pub text: Color32,
    /// Labels, units, and anything secondary.
    pub dim: Color32,
    /// The accent: selection, the queue, the drive delta, the sync button.
    pub amber: Color32,
    /// The low band, and the breakdown.
    pub blue: Color32,
    /// Verified.
    pub go: Color32,
    /// Wrong.
    pub alert: Color32,
    /// The stems, when the waveform is coloured by them.
    pub stem_vocals: Color32,
    pub stem_drums: Color32,
    pub stem_melody: Color32,
}

const fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

/// A scheme written the way its own documentation writes it.
///
/// Every scheme below is built from this, so adding one is eleven published
/// numbers rather than a judgement call about what its "alert" should be.
struct Core {
    name: &'static str,
    bg: u32,
    bg2: u32,
    rule: u32,
    text: u32,
    dim: u32,
    red: u32,
    green: u32,
    yellow: u32,
    blue: u32,
    magenta: u32,
    cyan: u32,
}

impl Core {
    const fn scheme(&self) -> Scheme {
        Scheme {
            name: self.name,
            booth: rgb(self.bg),
            booth_2: rgb(self.bg2),
            rule: rgb(self.rule),
            text: rgb(self.text),
            dim: rgb(self.dim),
            amber: rgb(self.yellow),
            blue: rgb(self.blue),
            go: rgb(self.green),
            alert: rgb(self.red),
            // Not the band colours: a picture coloured by what is playing has
            // to be told apart at a glance from one coloured by frequency, or
            // the two modes look like the same picture disagreeing with itself.
            stem_vocals: rgb(self.magenta),
            stem_drums: rgb(self.yellow),
            stem_melody: rgb(self.cyan),
        }
    }
}

/// Every scheme on offer. The first is the default.
pub const SCHEMES: [Scheme; 11] = [
    // This program's own, and the one everything else here was drawn against.
    Core {
        name: "booth",
        bg: 0x12161A,
        bg2: 0x1B2127,
        rule: 0x2C343C,
        text: 0xD7DDE3,
        dim: 0x7C8894,
        red: 0xE8654A,
        green: 0x57B37F,
        yellow: 0xE2A03F,
        blue: 0x5E9BEF,
        magenta: 0xE07AB0,
        cyan: 0x4FC3C7,
    }
    .scheme(),
    Core {
        name: "gruvbox",
        bg: 0x282828,
        bg2: 0x3C3836,
        rule: 0x504945,
        text: 0xEBDBB2,
        dim: 0x928374,
        red: 0xFB4934,
        green: 0xB8BB26,
        yellow: 0xFABD2F,
        blue: 0x83A598,
        magenta: 0xD3869B,
        cyan: 0x8EC07C,
    }
    .scheme(),
    Core {
        name: "solarized",
        bg: 0x002B36,
        bg2: 0x073642,
        rule: 0x586E75,
        text: 0x93A1A1,
        dim: 0x657B83,
        red: 0xDC322F,
        green: 0x859900,
        yellow: 0xB58900,
        blue: 0x268BD2,
        magenta: 0xD33682,
        cyan: 0x2AA198,
    }
    .scheme(),
    Core {
        name: "nord",
        bg: 0x2E3440,
        bg2: 0x3B4252,
        rule: 0x4C566A,
        text: 0xECEFF4,
        dim: 0x7B88A1,
        red: 0xBF616A,
        green: 0xA3BE8C,
        yellow: 0xEBCB8B,
        blue: 0x81A1C1,
        magenta: 0xB48EAD,
        cyan: 0x88C0D0,
    }
    .scheme(),
    // Dracula has no blue of its own; purple is what its own UI uses where a
    // blue would go, so that is what stands in rather than a colour invented
    // for the gap.
    Core {
        name: "dracula",
        bg: 0x282A36,
        bg2: 0x343746,
        rule: 0x44475A,
        text: 0xF8F8F2,
        dim: 0x6272A4,
        red: 0xFF5555,
        green: 0x50FA7B,
        yellow: 0xF1FA8C,
        blue: 0xBD93F9,
        magenta: 0xFF79C6,
        cyan: 0x8BE9FD,
    }
    .scheme(),
    Core {
        name: "monokai",
        bg: 0x272822,
        bg2: 0x3E3D32,
        rule: 0x49483E,
        text: 0xF8F8F2,
        dim: 0x75715E,
        red: 0xF92672,
        green: 0xA6E22E,
        yellow: 0xE6DB74,
        blue: 0x66D9EF,
        magenta: 0xAE81FF,
        cyan: 0xA1EFE4,
    }
    .scheme(),
    Core {
        name: "tokyonight",
        bg: 0x1A1B26,
        bg2: 0x24283B,
        rule: 0x414868,
        text: 0xC0CAF5,
        dim: 0x565F89,
        red: 0xF7768E,
        green: 0x9ECE6A,
        yellow: 0xE0AF68,
        blue: 0x7AA2F7,
        magenta: 0xBB9AF7,
        cyan: 0x7DCFFF,
    }
    .scheme(),
    Core {
        name: "catppuccin",
        bg: 0x1E1E2E,
        bg2: 0x313244,
        rule: 0x45475A,
        text: 0xCDD6F4,
        dim: 0x6C7086,
        red: 0xF38BA8,
        green: 0xA6E3A1,
        yellow: 0xF9E2AF,
        blue: 0x89B4FA,
        magenta: 0xCBA6F7,
        cyan: 0x94E2D5,
    }
    .scheme(),
    Core {
        name: "everforest",
        bg: 0x2D353B,
        bg2: 0x343F44,
        rule: 0x475258,
        text: 0xD3C6AA,
        dim: 0x859289,
        red: 0xE67E80,
        green: 0xA7C080,
        yellow: 0xDBBC7F,
        blue: 0x7FBBB3,
        magenta: 0xD699B6,
        cyan: 0x83C092,
    }
    .scheme(),
    Core {
        name: "onedark",
        bg: 0x282C34,
        bg2: 0x31353F,
        rule: 0x3E4452,
        text: 0xABB2BF,
        dim: 0x5C6370,
        red: 0xE06C75,
        green: 0x98C379,
        yellow: 0xE5C07B,
        blue: 0x61AFEF,
        magenta: 0xC678DD,
        cyan: 0x56B6C2,
    }
    .scheme(),
    // The one light scheme, and the reason anything here asks how bright a
    // colour is rather than assuming.
    Core {
        name: "papercolor",
        bg: 0xEEEEEE,
        bg2: 0xE4E4E4,
        rule: 0xBCBCBC,
        text: 0x444444,
        dim: 0x878787,
        red: 0xAF0000,
        green: 0x008700,
        yellow: 0xD75F00,
        blue: 0x0087AF,
        magenta: 0x8700AF,
        cyan: 0x005F87,
    }
    .scheme(),
];

thread_local! {
    /// The scheme this thread draws in.
    ///
    /// Per thread rather than global because the tests run in parallel and one
    /// of them setting a scheme must not repaint another's assertions. The
    /// window is one thread, so it sees one scheme.
    static CURRENT: Cell<Scheme> = const { Cell::new(SCHEMES[0]) };
}

/// The scheme in use.
pub fn scheme() -> Scheme {
    CURRENT.with(Cell::get)
}

/// Draw in the scheme of this name from here on.
///
/// An unknown name leaves the scheme alone and says so, because a stored
/// setting outlives the build that wrote it and a misspelt one should not
/// leave somebody with no colours at all.
pub fn use_scheme(name: &str) -> bool {
    match SCHEMES.iter().find(|scheme| scheme.name == name) {
        Some(found) => {
            CURRENT.with(|current| current.set(*found));
            true
        }
        None => false,
    }
}

/// The window's ground.
pub fn booth() -> Color32 {
    scheme().booth
}
/// Bars and panels that sit above it.
pub fn booth_2() -> Color32 {
    scheme().booth_2
}
/// Every rule and border.
pub fn rule() -> Color32 {
    scheme().rule
}
/// Body text.
pub fn text() -> Color32 {
    scheme().text
}
/// Labels, units, and anything secondary.
pub fn dim() -> Color32 {
    scheme().dim
}
/// The accent: selection, the queue, the drive delta, the sync button.
pub fn amber() -> Color32 {
    scheme().amber
}
/// The low band, and the breakdown.
pub fn blue() -> Color32 {
    scheme().blue
}
/// Verified.
pub fn go() -> Color32 {
    scheme().go
}
/// Wrong.
pub fn alert() -> Color32 {
    scheme().alert
}

/// The three waveform bands, in the order they are painted: lows first, then
/// mids, then highs over the top.
pub fn band_low() -> Color32 {
    blue()
}
pub fn band_mid() -> Color32 {
    amber()
}
pub fn band_high() -> Color32 {
    text()
}

pub fn stem_vocals() -> Color32 {
    scheme().stem_vocals
}
pub fn stem_drums() -> Color32 {
    scheme().stem_drums
}
pub fn stem_melody() -> Color32 {
    scheme().stem_melody
}

/// How bright a colour reads, 0 to 1.
///
/// The usual weighted sum rather than a plain average, because the eye is far
/// more sensitive to green than to blue and an average calls a saturated blue
/// bright when nothing can be read on it.
pub fn brightness(color: Color32) -> f32 {
    let [r, g, b, _] = color.to_array();
    (0.2126 * r as f32 + 0.7152 * g as f32 + 0.0722 * b as f32) / 255.0
}

/// Ink that can be read on a fill of this colour.
///
/// The phrase strip writes on blocks of accent colour, and an accent is
/// near-white in some schemes and near-black in others — the one light scheme
/// here has a red of `#af0000`. Asking the fill rather than assuming is what
/// keeps the names on those blocks legible in all eleven.
pub fn ink_on(fill: Color32) -> Color32 {
    match brightness(fill) > 0.55 {
        true => Color32::from_rgb(0x0F, 0x13, 0x16),
        false => Color32::from_rgb(0xF2, 0xF4, 0xF6),
    }
}

/// Whether this scheme is a light one.
///
/// Asked of the ground rather than stored, so a scheme added later cannot
/// forget to say.
pub fn is_light() -> bool {
    brightness(scheme().booth) > 0.5
}

/// The colour of each phrase in the strip under the waveform.
///
/// Off the scheme's accents, because the strip has to read as part of the same
/// picture as the waveform above it. Build is the accent taken towards the
/// warning colour — the two are next to each other in a track and want telling
/// apart without a second hue.
pub fn phrase_color(kind: &str) -> Color32 {
    let scheme = scheme();
    match kind {
        "build" => mix(scheme.amber, scheme.alert, 0.45),
        "drop" => scheme.amber,
        "break" => scheme.blue,
        // Intro and outro share a colour: what they have in common — not the
        // record proper — is more useful to see than telling them apart.
        _ => scheme.dim,
    }
}

/// Part of the way from one colour to another.
fn mix(from: Color32, to: Color32, how_far: f32) -> Color32 {
    let how_far = how_far.clamp(0.0, 1.0);
    let [fr, fg, fb, _] = from.to_array();
    let [tr, tg, tb, _] = to.to_array();
    let blend = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * how_far).round() as u8;
    Color32::from_rgb(blend(fr, tr), blend(fg, tg), blend(fb, tb))
}

/// The eight hot cue colours, in the order cues are handed them.
///
/// Not themed, and that is the point: these are written into the cues and go
/// on the drive, so they are what a player lights its buttons with. A scheme
/// that repainted them would change somebody's drive and mark every track as
/// changed on the next sync.
pub const CUE_COLORS: [Color32; 8] = [
    rgb(0xE2A03F),
    rgb(0x5E9BEF),
    rgb(0x57B37F),
    rgb(0xE8654A),
    rgb(0xB88AE0),
    rgb(0x4FC3C7),
    rgb(0xE07AB0),
    rgb(0x98A4AE),
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
/// How long the pointer has to rest on a control before its help appears.
///
/// Set every frame rather than once: it is a setting somebody can move while
/// looking at the window, and a hover delay that only takes effect after a
/// restart is one nobody can tune.
pub fn help_delay(ctx: &egui::Context, milliseconds: u32) {
    let wanted = milliseconds as f32 / 1000.0;
    if ctx.style().interaction.tooltip_delay == wanted {
        return;
    }
    ctx.style_mut(|style| {
        style.interaction.tooltip_delay = wanted;
        // The grace period is how long the help stays reachable while the
        // pointer moves between two controls. Kept short and tied to the delay,
        // so that a window set to show help at once does not keep showing the
        // last control's help over the next one.
        style.interaction.tooltip_grace_time = (wanted * 0.75).min(0.3);
    });
}

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

    // Start from egui's own light or dark base so that everything this does
    // not name — a scrollbar, a text cursor — is at least the right way round.
    let visuals = &mut style.visuals;
    *visuals = match is_light() {
        true => egui::Visuals::light(),
        false => egui::Visuals::dark(),
    };
    visuals.dark_mode = !is_light();
    visuals.override_text_color = Some(text());
    visuals.panel_fill = booth();
    visuals.window_fill = booth();
    visuals.extreme_bg_color = booth_2();
    visuals.faint_bg_color = booth_2();
    visuals.window_stroke = egui::Stroke::new(1.0_f32, rule());

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
        widget.bg_stroke = egui::Stroke::new(1.0_f32, rule());
        widget.fg_stroke = egui::Stroke::new(1.0_f32, text());
    }
    visuals.widgets.noninteractive.bg_fill = booth();
    visuals.widgets.noninteractive.weak_bg_fill = booth();
    visuals.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0_f32, dim());
    visuals.widgets.inactive.bg_fill = booth_2();
    visuals.widgets.inactive.weak_bg_fill = booth_2();
    visuals.widgets.hovered.bg_fill = rule();
    visuals.widgets.hovered.weak_bg_fill = rule();
    visuals.widgets.active.bg_fill = rule();
    visuals.widgets.active.weak_bg_fill = rule();
    visuals.selection.bg_fill = amber().gamma_multiply(0.30);
    visuals.selection.stroke = egui::Stroke::new(1.0_f32, text());
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
        assert_eq!(phrase_color("drop"), amber());
        assert_eq!(phrase_color("break"), blue());
        // Anything unrecognised still draws, rather than vanishing.
        assert_eq!(phrase_color("whatever"), phrase_color("intro"));
        // And build is not the drop, which is the pair a strip has to tell
        // apart most often.
        assert_ne!(phrase_color("build"), phrase_color("drop"));
    }

    #[test]
    fn a_scheme_can_be_chosen_by_name_and_a_wrong_name_changes_nothing() {
        assert!(use_scheme("gruvbox"));
        assert_eq!(scheme().name, "gruvbox");
        assert_eq!(booth(), Color32::from_rgb(0x28, 0x28, 0x28));

        // A setting outlives the build that wrote it, so a name this build has
        // never heard of leaves the colours alone rather than blanking them.
        assert!(!use_scheme("a scheme from next year"));
        assert_eq!(scheme().name, "gruvbox");

        use_scheme("booth");
    }

    #[test]
    fn every_scheme_can_be_read() {
        // The check that would catch a palette typed in wrong: a background and
        // a text colour that are nearly the same brightness is a scheme nobody
        // can use, whichever way round it is.
        for scheme in SCHEMES {
            let gap = (brightness(scheme.text) - brightness(scheme.booth)).abs();
            assert!(gap > 0.35, "{} has text at {gap:.2} from its ground", scheme.name);

            // And the dim colour has to be dimmer than the text but still
            // visible, or every label is either shouting or gone.
            let dim = (brightness(scheme.dim) - brightness(scheme.booth)).abs();
            assert!(dim > 0.10, "{}'s dim is invisible on its ground", scheme.name);
            assert!(dim < gap, "{}'s dim is not dimmer than its text", scheme.name);
        }
    }

    #[test]
    fn every_scheme_is_named_once() {
        let mut names: Vec<&str> = SCHEMES.iter().map(|scheme| scheme.name).collect();
        names.sort_unstable();
        let all = names.len();
        names.dedup();
        assert_eq!(names.len(), all, "two schemes share a name, so one cannot be chosen");
        assert_eq!(SCHEMES[0].name, "booth", "the first scheme is the default");
    }

    #[test]
    fn a_phrase_name_can_be_read_on_every_phrase_colour() {
        // The strip writes on blocks of accent colour. One scheme here is
        // light, where the accents are dark enough that black on them is a
        // guess at what the block says.
        for scheme in SCHEMES {
            use_scheme(scheme.name);
            for kind in ["intro", "build", "break", "drop", "outro"] {
                let fill = phrase_color(kind);
                let ink = ink_on(fill);
                let gap = (brightness(fill) - brightness(ink)).abs();
                assert!(gap > 0.35, "{} on {}'s {kind} is {gap:.2} apart", "the name", scheme.name);
            }
        }
        use_scheme("booth");
    }

    #[test]
    fn only_the_light_scheme_says_it_is_light() {
        use_scheme("papercolor");
        assert!(is_light());
        for dark in ["booth", "gruvbox", "nord", "dracula", "tokyonight"] {
            use_scheme(dark);
            assert!(!is_light(), "{dark} read as a light scheme");
        }
        use_scheme("booth");
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
