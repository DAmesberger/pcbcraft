//! SolidCraft's token contract: every colour and size a UI component draws with ([`Tokens`]), SolidCraft's light and
//! dark values written in code (a dark theme, the default, a blue-grey in the spirit of desktop CAD tools, viewport
//! #2e3440, and a light one), colour vision modes, tab accents, and the per-thread and per-context theme state a host
//! sets. With nothing set, components draw with the built-in dark tokens. It holds no data files and knows no theme
//! file format: a host's theme system resolves its own palettes and hands them over ([`set_custom`],
//! [`scoped_panel_tokens`], [`set_published`]).

use std::cell::Cell;

mod palettes;

use egui::{Color32, Stroke};
use serde::{Deserialize, Serialize};
use solidcraft_color::AnalysisColors;

thread_local! {
    /// Theme state belongs to the UI thread drawing the frame. Concurrent app/test renderers
    /// must not change one another's palette while a frame or icon texture is being built.
    static DARK: Cell<bool> = const { Cell::new(true) };
    /// The colour vision mode, per thread. Each app
    /// sets it from its own preferences at the start of every frame (in its frame logic), on the
    /// thread that draws it. Tests run many apps side by side on their own threads in one process:
    /// with one global, a test in a mode changed the colours under another one
    /// (`hover_stays_off_during_spin_and_animated_views` failed that way). No worker thread reads
    /// it: the preview thread is handed its colours (`preview::Colors`) and thumbnails are drawn by
    /// the engine in the model's colours, so a new thread starts Off and stays Off.
    static VISION: Cell<ColorVision> = const { Cell::new(ColorVision::Off) };
    /// The palette of a theme file (`theme_files`) in place of the built-in one of its base
    /// (dark or light), per thread like the rest. `None`: the built-in palettes.
    static CUSTOM: Cell<Option<Custom>> = const { Cell::new(None) };
    /// Paint-only panel palette. It never changes authored theme state or cache generations.
    static PANEL_TOKENS: Cell<Option<Tokens>> = const { Cell::new(None) };
    static CUSTOM_GENERATION: Cell<u64> = const { Cell::new(0) };
    /// The application bar's logo in the theme's accent instead of its own logo colours
    /// (Preferences › Interface › Logo), per thread like the rest.
    static LOGO_ACCENT: Cell<bool> = const { Cell::new(false) };
    /// The 3D view's own dark or light when Preferences give it one ([`ViewportTheme`], #4146),
    /// per thread like the rest. `None`: the view is as dark as the window.
    static VIEWPORT_DARK: Cell<Option<bool>> = const { Cell::new(None) };
    /// The 3D view's background colour when Preferences give it one (#7688), per thread like the
    /// rest. `None`: the theme's gradient.
    static VIEWPORT_BACKGROUND: Cell<Option<[u8; 3]>> = const { Cell::new(None) };
    /// The last published frame, guarded by every palette input so another app/context cannot
    /// reuse it after changing the thread's theme. Resolution/fade state lives in that context.
    static PUBLISHED: Cell<Option<PublishedTab>> = const { Cell::new(None) };
}

/// Every input of the thread's theme state: a palette resolved under one is stale under another.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Environment {
    pub dark: bool,
    pub vision: ColorVision,
    pub custom_generation: u64,
    pub logo: bool,
    pub viewport: Option<bool>,
    pub background: Option<[u8; 3]>,
}

impl Environment {
    /// The thread's theme state now.
    pub fn current() -> Self {
        Self {
            dark: is_dark(),
            vision: vision(),
            custom_generation: CUSTOM_GENERATION.get(),
            logo: LOGO_ACCENT.get(),
            viewport: VIEWPORT_DARK.get(),
            background: viewport_background(),
        }
    }
}

/// A frame's published tab palette ([`set_published`]): what [`Tokens::get`] and [`Tokens::view`] return while the theme
/// state it was made in ([`Environment`]) holds.
#[derive(Clone, Copy)]
pub struct PublishedTab {
    /// The theme state the palettes were resolved in.
    pub environment: Environment,
    /// The window's tokens, with the tab's surfaces.
    pub tokens: Tokens,
    /// The tokens drawn straight onto the 3D view.
    pub view: Tokens,
    /// Whether the theme gives its tabs surfaces of their own ([`tab_surfaces_enabled`]).
    pub tab_surfaces: bool,
}

/// The surfaces a tab may give a colour of its own: the panel and the 3D view's background.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Surfaces {
    pub panel: Color32,
    pub top: Color32,
    pub bottom: Color32,
}

impl Surfaces {
    pub fn of(t: Tokens) -> Self {
        Self { panel: t.panel, top: t.viewport_top, bottom: t.viewport_bottom }
    }

    pub fn apply(self, t: &mut Tokens) {
        t.panel = self.panel;
        t.viewport_top = self.top;
        t.viewport_bottom = self.bottom;
    }
}

/// A fade of the tab surfaces from one tab's to another's, `seconds` long from `start` (egui's input time).
#[derive(Clone, Copy)]
pub struct Fade {
    pub from: Surfaces,
    pub to: Surfaces,
    pub start: f64,
    pub seconds: f64,
}

impl Fade {}

/// Three structural border levels plus independent content and control outlines.
/// Focus, selection, status, artwork and CAD geometry do not use these roles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BorderRole {
    Primary,
    Secondary,
    Tertiary,
    Container,
    Control,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Borders {
    pub primary: Option<Stroke>,
    pub secondary: Option<Stroke>,
    /// Pattern for secondary section rules; solid preserves existing themes.
    pub secondary_dotted: bool,
    pub tertiary: Option<Stroke>,
    /// Solid outlines for nested previews, information wells and shortcut keycaps.
    pub container: Option<Stroke>,
    pub control: Option<Stroke>,
    pub primary_inset: Option<InsetBorder>,
    /// Optional directional chrome. Absent themes keep their flat strokes.
    pub bevel: Option<Bevel>,
}

/// Two directional edge bands, shared by raised chrome and recessed editors.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bevel {
    pub width: f32,
    pub highlight: Color32,
    pub light: Color32,
    pub shadow: Color32,
    pub dark_shadow: Color32,
}

impl Bevel {
    /// Draw inside existing geometry; no extra input targets or layout allocation.
    pub fn paint(self, painter: &egui::Painter, rect: egui::Rect, radius: impl Into<egui::CornerRadius>, recessed: bool, enabled: bool) {
        if !rect.is_finite() || self.width <= 0.0 || !self.width.is_finite() || rect.width().min(rect.height()) <= self.width * 4.0 {
            return;
        }
        let radius = radius.into();
        let bands = if recessed {
            [(self.shadow, self.highlight), (self.dark_shadow, self.light)]
        } else {
            [(self.highlight, self.dark_shadow), (self.light, self.shadow)]
        };
        for (index, (upper, lower)) in bands.into_iter().enumerate() {
            let inset = (index as f32 + 0.5) * self.width;
            let edge = rect.shrink(inset);
            let mut rounding: egui::epaint::CornerRadiusF32 = radius.into();
            rounding -= inset;
            let mut path = Vec::new();
            egui::epaint::tessellator::path::rounded_rectangle(&mut path, egui::epaint::RoundedRect::new(edge, rounding));
            if let Some(first) = path.first().copied() {
                path.push(first);
            }
            for pair in path.windows(2) {
                if let Some((a, b)) = pair.first().zip(pair.get(1)) {
                    let middle = *a + (*b - *a) * 0.5;
                    let relative = (middle - edge.center()) / edge.size();
                    let color = if relative.x + relative.y < 0.0 { upper } else { lower };
                    let color = if enabled { color } else { color.gamma_multiply(0.45) };
                    painter.line_segment([*a, *b], Stroke::new(self.width, color));
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InsetBorder {
    pub stroke: Stroke,
    /// Clear distance from the inner edge of the primary stroke, in logical points.
    pub gap: f32,
}

impl Borders {
    pub const LEGACY: Self = Self {
        primary: None,
        secondary: None,
        secondary_dotted: false,
        tertiary: None,
        container: None,
        control: None,
        primary_inset: None,
        bevel: None,
    };

    pub fn stroke(self, role: BorderRole, fallback: Stroke) -> Stroke {
        match role {
            BorderRole::Primary => self.primary,
            BorderRole::Secondary => self.secondary,
            BorderRole::Tertiary => self.tertiary,
            BorderRole::Container => self.container,
            BorderRole::Control => self.control,
        }
        .unwrap_or(fallback)
    }
}

impl Tokens {
    /// Bevel themes use the editable surface colour independently of button faces.
    pub fn input_fill(&self) -> Color32 {
        if self.borders.bevel.is_some() { self.field } else { self.control_fill }
    }

    pub fn input_bevel(&self, ui: &egui::Ui, response: &egui::Response) {
        if let Some(bevel) = self.borders.bevel {
            bevel.paint(ui.painter(), response.rect.shrink(1.0), self.control_radius, true, response.enabled());
        }
    }

    pub fn button_bevel(&self, ui: &egui::Ui, response: &egui::Response, selected: bool) {
        self.button_bevel_with_radius(ui, response, selected, self.control_radius);
    }

    /// Compound controls supply the same corner geometry as their selected fill.
    pub fn button_bevel_with_radius(&self, ui: &egui::Ui, response: &egui::Response, selected: bool, radius: impl Into<egui::CornerRadius>) {
        if let Some(bevel) = self.borders.bevel {
            let pressed = response.enabled()
                && (response.is_pointer_button_down_on()
                    || (response.has_focus() && ui.input(|i| i.key_down(egui::Key::Space) || i.key_down(egui::Key::Enter))));
            bevel.paint(ui.painter(), response.rect, radius, selected || pressed, response.enabled());
        }
    }

    pub fn primary_border(&self, color: Color32) -> Stroke {
        self.borders.stroke(BorderRole::Primary, Stroke::new(1.0, color))
    }
    pub fn secondary_border(&self, color: Color32) -> Stroke {
        self.borders.stroke(BorderRole::Secondary, Stroke::new(1.0, color))
    }
    /// Secondary rules share a pattern as well as a stroke. Dot count is bounded for tiny widths.
    fn secondary_dots(&self, painter: &egui::Painter, path: &[egui::Pos2], stroke: Stroke) {
        if stroke.width <= 0.0 || path.iter().any(|p| !p.is_finite()) {
            return;
        }
        let length: f32 = path.windows(2).map(|pair| pair.first().zip(pair.get(1)).map_or(0.0, |(a, b)| a.distance(*b))).sum();
        if !length.is_finite() {
            return;
        }
        let spacing = (stroke.width * 3.0).max(3.0).max(length / 4096.0);
        painter.extend(egui::Shape::dotted_line(path, stroke.color, spacing, stroke.width * 0.5));
    }

    pub fn secondary_line(&self, painter: &egui::Painter, points: [egui::Pos2; 2], color: Color32) {
        let stroke = self.secondary_border(color);
        if self.borders.secondary_dotted {
            self.secondary_dots(painter, &points, stroke);
        } else {
            painter.line_segment(points, stroke);
        }
    }

    pub fn secondary_hline(&self, painter: &egui::Painter, range: impl Into<egui::emath::Rangef>, y: f32, color: Color32) {
        let range = range.into();
        self.secondary_line(painter, [egui::pos2(range.min, y), egui::pos2(range.max, y)], color);
    }

    pub fn secondary_vline(&self, painter: &egui::Painter, x: f32, range: impl Into<egui::emath::Rangef>, color: Color32) {
        let range = range.into();
        self.secondary_line(painter, [egui::pos2(x, range.min), egui::pos2(x, range.max)], color);
    }

    /// Containers stay solid independently of the section-divider pattern.
    pub fn container_rect(&self, painter: &egui::Painter, rect: egui::Rect, radius: impl Into<egui::CornerRadius>, fill: Color32, color: Color32) {
        let radius = radius.into();
        painter.rect(rect, radius, fill, self.container_border(color), egui::StrokeKind::Inside);
        if let Some(bevel) = self.borders.bevel {
            bevel.paint(painter, rect, radius, false, true);
        }
    }

    pub fn container_border(&self, color: Color32) -> Stroke {
        self.borders.stroke(BorderRole::Container, Stroke::new(1.0, color))
    }

    pub fn tertiary_border(&self, color: Color32) -> Stroke {
        self.borders.stroke(BorderRole::Tertiary, Stroke::new(1.0, color))
    }
    pub fn control_border_stroke(&self, color: Color32) -> Stroke {
        self.borders.stroke(BorderRole::Control, Stroke::new(1.0, color))
    }
}

/// A theme file's palette and whether it is dark.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Custom {
    pub dark: bool,
    pub tokens: Tokens,
}

/// A tab's selection colours. This is deliberately separate from the global widget style:
/// application chrome, checkboxes, status colours and dialog actions retain their own tokens.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TabAccent {
    pub color: Color32,
    pub ink: Color32,
    pub soft: Color32,
}

/// One of the theme's accent ramps: a fill, a small-text ink and a selected-row tint in [`Tokens`],
/// named after its token slots. The host decides which ramp each of its tabs uses ([`TabTable`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum AccentRamp {
    /// The main accent: `accent`, `selected_ink`, `accent_soft`.
    #[default]
    Main,
    /// `sketch_accent`, `sketch_ink`, `sketch_soft`.
    Sketch,
    /// `surface_fill`, `surface_ink`, `surface_soft`.
    Surface,
    /// `mesh_fill`, `mesh_ink`, `mesh_soft`.
    Mesh,
    /// `form_fill`, `form_tab_ink`, `form_soft`.
    Form,
    /// `sheet_fill`, `sheet_ink`, `sheet_soft`.
    Sheet,
    /// `render_fill`, `render_ink`, `render_soft`.
    Render,
}

impl AccentRamp {
    /// The ramp's fill, ink and tint in `t`, before [`TabAccent::for_ramp`] protects the ink's contrast.
    fn slots(self, t: &Tokens) -> (Color32, Color32, Color32) {
        match self {
            Self::Main => (t.accent, t.selected_ink, t.accent_soft),
            Self::Sketch => (t.sketch_accent, t.sketch_ink, t.sketch_soft),
            Self::Surface => (t.surface_fill, t.surface_ink, t.surface_soft),
            Self::Mesh => (t.mesh_fill, t.mesh_ink, t.mesh_soft),
            Self::Form => (t.form_fill, t.form_tab_ink, t.form_soft),
            Self::Sheet => (t.sheet_fill, t.sheet_ink, t.sheet_soft),
            Self::Render => (t.render_fill, t.render_ink, t.render_soft),
        }
    }
}

/// How the host accents one of its tabs: the tab's name, its ramp, and the icon palette accent
/// (a ramp name of the host's icon set) of icons on a surface the tab accents; `None` keeps each
/// icon's own colours.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TabStyle {
    pub name: &'static str,
    pub ramp: AccentRamp,
    pub icon_accent: Option<&'static str>,
}

/// A host's tabs and how each is accented. The host owns the table (usually a `const`) and
/// resolves its tabs' accents through it; a name it does not list takes [`AccentRamp::Main`]
/// and no icon accent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TabTable(&'static [TabStyle]);

impl TabTable {
    pub const fn new(tabs: &'static [TabStyle]) -> Self {
        Self(tabs)
    }
}

/// WCAG contrast of two opaque sRGB colours.
pub fn contrast_ratio(a: Color32, b: Color32) -> f64 {
    let luminance = |c: Color32| {
        let linear = solidcraft_color::srgb::decode8;
        0.2126 * linear(c.r()) + 0.7152 * linear(c.g()) + 0.0722 * linear(c.b())
    };
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

/// Composite a translucent foreground over an opaque background in sRGB for contrast checks.
pub fn composite(fill: Color32, background: Color32) -> Color32 {
    let rgb = fill.to_srgba_unmultiplied();
    let a = f64::from(rgb[3]) / 255.0;
    let channel = |index: usize| (f64::from(rgb[index]) * a + f64::from(background[index]) * (1.0 - a)).round() as u8;
    Color32::from_rgb(channel(0), channel(1), channel(2))
}

fn readable_fill(fill: Color32, inks: &[Color32], scale: f32) -> Color32 {
    let readable = |candidate| {
        [Color32::BLACK, Color32::WHITE].iter().all(|background| {
            let ground = composite(candidate, *background);
            inks.iter().all(|ink| contrast_ratio(composite(*ink, ground), ground) >= 4.5)
        })
    };
    // A sampled/retargeted colour that already reads must stay bit-exact, including alpha.
    if scale >= 1.0 && readable(fill) {
        return fill;
    }
    let [r, g, b, alpha] = fill.to_srgba_unmultiplied();
    let requested = (f32::from(alpha) * scale).round() as u8;
    // At most 256 cheap colour comparisons per changed role; no allocations, GPU or I/O.
    for a in requested..=255 {
        let candidate = Color32::from_rgba_unmultiplied(r, g, b, a);
        if readable(candidate) {
            return candidate;
        }
    }
    // An authored foreground may already fail at full opacity. Never alter that foreground;
    // full alpha gives it the strongest protection this option can supply.
    Color32::from_rgb(r, g, b)
}

impl TabAccent {
    /// The selection colours of `ramp` in `t`, with an ink that holds small-text contrast on the tint.
    pub fn for_ramp(t: &Tokens, ramp: AccentRamp) -> Self {
        let (color, ink, soft) = ramp.slots(t);
        let ink = if contrast_ratio(ink, soft) >= 4.5 {
            ink
        } else if contrast_ratio(color, soft) >= 4.5 {
            color
        } else if contrast_ratio(t.text, soft) >= 4.5 {
            t.text
        } else if contrast_ratio(Color32::BLACK, soft) >= contrast_ratio(Color32::WHITE, soft) {
            Color32::BLACK
        } else {
            Color32::WHITE
        };
        Self { color, ink, soft }
    }

    pub fn current(ctx: &egui::Context) -> Self {
        ctx.data(|d| d.get_temp::<Self>(egui::Id::unique("sc_tab_accent"))).unwrap_or_else(|| Self::for_ramp(&Tokens::get(), AccentRamp::Main))
    }

    /// Paint only the focused numeric editor, without changing sibling buttons or checkboxes.
    pub fn number_focus(ui: &egui::Ui, response: &egui::Response) {
        let active = ui.ctx().data(|d| d.get_temp::<bool>(egui::Id::unique("sc_tool_accent"))).unwrap_or(false);
        let t = Tokens::get();
        let focus = ui.ctx().data(|d| d.get_temp::<Option<AccentRamp>>(egui::Id::unique("sc_focus_ramp"))).flatten();
        let color = match focus {
            Some(ramp) => Self::for_ramp(&t, ramp).color,
            None => Self::current(ui.ctx()).color,
        };
        if active && response.has_focus() && color != t.accent {
            ui.painter().rect_stroke(response.rect, t.radius_sm, Stroke::new(1.0, color), egui::StrokeKind::Inside);
        }
    }
}

/// Is the dark theme on?
pub fn is_dark() -> bool {
    DARK.get()
}

/// Is the 3D view dark? As the window, unless Preferences give the view its own (#4146).
pub fn viewport_dark() -> bool {
    VIEWPORT_DARK.get().unwrap_or_else(is_dark)
}

/// The 3D view's own background colour, or `None` for the theme's gradient (#7688).
pub fn viewport_background() -> Option<[u8; 3]> {
    VIEWPORT_BACKGROUND.get()
}

/// Colour vision modes (#778): a second set of values for the colours that carry a meaning, for
/// a weak or missing red (protan), green (deutan) or blue (tritan) sense. Not a filter: Off gives
/// back the theme's colours unchanged. Protan and Deutan share one red-green table, measured
/// clear for both (`colour_vision_tests`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ColorVision {
    #[default]
    Off,
    Protan,
    Deutan,
    Tritan,
}

impl ColorVision {
    pub const ALL: [ColorVision; 4] = [ColorVision::Off, ColorVision::Protan, ColorVision::Deutan, ColorVision::Tritan];

    /// The saved value (`ui.prefs` `color_vision`).
    pub fn id(self) -> &'static str {
        match self {
            ColorVision::Off => "off",
            ColorVision::Protan => "protan",
            ColorVision::Deutan => "deutan",
            ColorVision::Tritan => "tritan",
        }
    }

    pub fn from_id(id: &str) -> Option<ColorVision> {
        ColorVision::ALL.into_iter().find(|m| m.id() == id)
    }

    /// Its name in the Preferences (a message id).
    pub fn label_id(self) -> &'static str {
        match self {
            ColorVision::Off => "ui.colour_vision_off",
            ColorVision::Protan => "ui.colour_vision_protan",
            ColorVision::Deutan => "ui.colour_vision_deutan",
            ColorVision::Tritan => "ui.colour_vision_tritan",
        }
    }

    /// Who it is for, in one line (its tooltip; a message id).
    pub fn help_id(self) -> &'static str {
        match self {
            ColorVision::Off => "ui.colour_vision_off_tip",
            ColorVision::Protan => "ui.colour_vision_protan_tip",
            ColorVision::Deutan => "ui.colour_vision_deutan_tip",
            ColorVision::Tritan => "ui.colour_vision_tritan_tip",
        }
    }
}

/// From saved preferences: a value this version doesn't know (a later version's mode, a hand
/// edit) is Off rather than an error, which would lose every other saved preference with it.
/// `ui.prefs` refuses such a value before it gets here (`prefs::set`).
impl<'de> Deserialize<'de> for ColorVision {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = serde_json::Value::deserialize(d)?;
        Ok(v.as_str().and_then(ColorVision::from_id).unwrap_or_default())
    }
}

/// The colour vision mode on now (on this thread).
pub fn vision() -> ColorVision {
    VISION.with(Cell::get)
}

/// Colours for native viewport markers and the agent pointer. Command artwork uses the SVG palette.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct IconColors {
    pub highlight: Color32,
    pub agent: Color32,
    pub marker_halo: Color32,
}

/// Every colour and size the UI draws with. The colours drawn in the 3D view are listed again in
/// [`Tokens::with_viewport`], which takes them from the other side when the 3D view is set to the
/// other one's colours (#4146): a new colour drawn in the view goes in both.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tokens {
    /// Navigation chrome from the approved Navbar boards.
    pub nav_hover: Color32,
    pub nav_press: Color32,
    pub nav_dim: Color32,
    pub nav_viewport: Color32,
    pub nav_tip: Color32,
    pub nav_tip_ink: Color32,
    pub nav_tip_key_border: Color32,
    pub nav_field: Color32,

    /// Optional structural/control strokes. None retains each surface’s legacy styling.
    pub borders: Borders,
    /// File/start presentation, with explicit foreground/background pairs.
    pub file_dim: Color32,
    pub file_recovery_well: Color32,
    pub file_action_height: f32,
    pub file_back_height: f32,
    pub file_recovery_icon: f32,
    pub file_grid_gap: f32,
    pub file_background: Color32,
    pub file_surface: Color32,
    pub file_border: Color32,
    /// Neutral multiplication tint for File-view model thumbnails.
    pub file_image_tint: Color32,
    pub file_rail: Color32,
    pub file_rail_ink: Color32,
    pub file_rail_hover: Color32,
    pub file_primary: Color32,
    pub file_primary_ink: Color32,
    pub file_primary_border: Color32,
    pub file_recovery: Color32,
    pub file_recovery_ink: Color32,
    pub file_recovery_border: Color32,
    pub file_nav_width: f32,
    pub file_nav_height: f32,
    pub file_tile_height: f32,
    pub file_tile_icon: f32,
    pub file_recent_height: f32,
    pub file_sample_image_height: f32,
    pub file_content_inset_x: f32,
    pub file_content_inset_y: f32,
    pub file_section_gap: f32,
    pub font_start_title: f32,
    // Shared dimensions in logical pixels; identical in both themes.
    pub space_2: f32,
    pub space_3: f32,
    pub space_4: f32,
    pub space_6: f32,
    pub space_8: f32,
    pub space_10: f32,
    pub space_12: f32,
    pub space_14: f32,
    pub space_16: f32,
    pub space_24: f32,
    pub radius_sm: u8,
    pub radius_md: u8,
    pub radius_lg: u8,
    pub radius_xl: u8,
    pub font_badge: f32,
    pub font_micro: f32,
    pub font_panel: f32,
    pub font_small: f32,
    pub font_caption: f32,
    pub font_body: f32,
    pub font_label: f32,
    pub font_button: f32,
    pub font_heading: f32,
    /// Tiny world-axis triad letters.
    pub font_triad: f32,
    /// Bold letters on the navigation compass discs.
    pub font_compass: f32,
    /// Home section titles, home actions and update titles.
    pub font_subtitle: f32,
    /// Introductory text on the home page.
    pub font_intro: f32,
    /// Preferences page title, independent of dialog and article headings.
    pub font_page_heading: f32,
    /// Top-level headings in assistant Markdown.
    pub font_article_heading: f32,
    /// Application title in the About dialog.
    pub font_title: f32,
    /// Application name on the home page.
    pub font_display: f32,
    /// Height of dense form controls and rows.
    pub control_height: f32,
    /// Card text, hint, primary and validation colours.
    pub form_ink: Color32,
    pub form_dim: Color32,
    pub form_primary: Color32,
    /// Primary action fill; theme files without this token inherit `form_primary`.
    pub form_primary_fill: Color32,
    pub form_error: Color32,
    pub form_error_fill: Color32,
    /// Successful validation text and its badge surface.
    pub form_success: Color32,
    pub form_success_fill: Color32,
    /// Corner radius of dense form controls and actions.
    pub control_radius: u8,
    /// Width of the dense form label column.
    pub form_label_width: f32,
    /// Vertical gap between dense form rows.
    pub form_gap_y: f32,
    /// Height of the full-width dense form footer.
    pub form_footer_height: f32,
    /// Height of the dense form header.
    pub form_header_height: f32,
    /// Outer width of a dense feature card.
    pub form_card_width: f32,
    /// Size of the icon in a dense form header.
    pub form_icon_size: f32,
    /// Size of the dense form close button.
    pub form_close_size: f32,
    /// Text inset in controls and gap after the header icon.
    pub form_inset: f32,
    /// Inset of artwork inside a dense form icon button.
    pub form_icon_inset: f32,
    /// Size of the section disclosure chevron.
    pub form_section_icon_size: f32,
    /// Height of a dense form checkbox row.
    pub form_checkbox_height: f32,
    /// Minimum height of a dense form footer action.
    pub form_button_height: f32,
    /// Outer width of the sketch options card.
    pub sketch_palette_width: f32,
    /// Square creation-mode button size and radius.
    pub sketch_mode_size: f32,
    pub sketch_mode_radius: u8,
    pub sketch_mode_border: f32,
    /// Opacity of the sketch card and its selected accent tint.
    pub sketch_palette_alpha: f32,
    pub sketch_mode_tint: f32,
    /// Footer navigation icon size.
    pub sketch_footer_icon_size: f32,
    /// Extra section heading letter spacing as a fraction of font size.
    pub font_section_tracking: f32,
    pub app_bar: Color32,
    /// Dividers between the application bar groups.
    pub app_bar_separator: Color32,
    pub toolbar: Color32,
    pub panel: Color32,
    pub panel_header: Color32,
    pub border: Color32,
    pub text: Color32,
    pub text_dim: Color32,
    pub accent: Color32,
    pub accent_soft: Color32,
    pub sketch_accent: Color32,
    pub surface_accent: Color32,
    pub mesh_accent: Color32,
    pub form_accent: Color32,
    pub render_accent: Color32,
    /// Per-tab fills, small-text inks and selected-row tints. Sketch reuses sketch_accent.
    /// The other fill keys preserve the legacy *_accent values used by other surfaces.
    pub sketch_ink: Color32,
    pub sketch_soft: Color32,
    pub surface_fill: Color32,
    pub surface_ink: Color32,
    pub surface_soft: Color32,
    pub mesh_fill: Color32,
    pub mesh_ink: Color32,
    pub mesh_soft: Color32,
    pub form_fill: Color32,
    pub form_tab_ink: Color32,
    pub form_soft: Color32,
    pub sheet_fill: Color32,
    pub sheet_ink: Color32,
    pub sheet_soft: Color32,
    pub render_fill: Color32,
    pub render_ink: Color32,
    pub render_soft: Color32,
    /// Dark text for tab pills whose fill has insufficient contrast with on_accent.
    pub tab_on_fill_dark: Color32,
    /// Ink on accent-filled buttons and selection chips.
    pub on_accent: Color32,
    /// Agent status on the dark application bar, in either theme.
    pub agent_ok: Color32,
    pub agent_busy: Color32,
    pub error: Color32,
    pub warning: Color32,
    pub icon: Color32,
    pub timeline: Color32,
    pub hover: Color32,
    // ---- viewport selection feedback ----
    /// Selected face (shaded fill).
    pub sel_face: Color32,
    /// Selected edge core and its lighter rim.
    pub sel_edge: Color32,
    pub sel_edge_rim: Color32,
    /// Hovered edge: dark core with a light halo.
    pub hover_edge: Color32,
    pub hover_edge_halo: Color32,
    /// How much lighter a hovered face is (grey levels).
    pub hover_face_lift: u8,
    /// Selected profile fill and outline.
    pub sel_profile: Color32,
    pub sel_profile_edge: Color32,
    /// Hovered profile: faint tint and a bright outline.
    pub hover_profile: Color32,
    pub hover_profile_edge: Color32,
    /// Selected vertex dot.
    pub sel_vertex: Color32,
    // ---- origin ----
    pub origin_plane: Color32,
    pub origin_plane_edge: Color32,
    pub origin_plane_hover: Color32,
    pub origin_point: Color32,
    /// The X, Y and Z colours, one set for every axis drawn (#976): the origin axes, the view
    /// triad, the navigation cube's triad, the Move gizmo, a 3D sketch point's triad, joint
    /// origins, measured distances along the axes and the headless picture's origin axes.
    pub axis_x: Color32,
    pub axis_y: Color32,
    pub axis_z: Color32,
    /// Construction planes.
    pub construction_plane: Color32,
    /// Construction axes and points.
    pub construction_geom: Color32,
    // ---- bodies and live previews ----
    pub body: Color32,
    pub body_edge: Color32,
    /// Command manipulators (direction arrows, handles).
    pub manipulator: Color32,
    /// Allowed assembly motion, separate from the selected face's blue.
    pub joint_manipulator: Color32,
    /// Assembly Move axis and centre, distinct from its draggable amber arrows.
    pub joint_axis: Color32,
    /// New surface in a feature preview.
    pub preview_add: Color32,
    /// The surface of a body a feature preview creates (New Body), apart from a join (#1805).
    pub preview_new: Color32,
    /// Proposed sketch geometry, distinct from blue selections.
    pub preview_sketch: Color32,
    /// Removed material in a feature preview (translucent).
    pub preview_cut: Color32,
    /// Navigation cube faces, outline and labels.
    pub cube_face: Color32,
    pub cube_border: Color32,
    pub cube_text: Color32,
    /// Light blue hover fill, readable on the pale cube in either theme.
    pub cube_hover: Color32,
    // ---- viewport ----
    /// Viewport background (top and bottom of the gradient; equal for a flat fill).
    pub viewport_top: Color32,
    pub viewport_bottom: Color32,
    pub grid: Color32,
    pub grid_major: Color32,
    /// A whole selected body.
    pub body_selected: Color32,
    /// Sketch curves: under-constrained, fully constrained (and finished sketches).
    pub sketch: Color32,
    pub sketch_fixed: Color32,
    /// Wider rim beneath sketch curves to separate them from shaded faces.
    pub sketch_halo: Color32,
    /// Fixed (locked) sketch geometry.
    pub sketch_locked: Color32,
    /// The open sketch's curves and points that a failing constraint holds, and that
    /// constraint's glyph or dimension, while the constraints conflict (#7244). The error red by
    /// default; a theme file that sets `error` but not this key takes its `error`.
    pub sketch_conflict: Color32,
    /// Sketch dimensions: driving, driven (reference). Each theme gives them a colour of their
    /// own, apart from the sketch curves they measure (#7861).
    pub dimension: Color32,
    pub dimension_driven: Color32,
    /// Sketch points: fully constrained, free.
    pub sketch_point: Color32,
    pub sketch_point_free: Color32,
    /// Command dialogs: panel, border, and the filled selection chip.
    pub dialog_bg: Color32,
    pub dialog_border: Color32,
    pub chip: Color32,
    /// Inside of bodies cut by a section.
    pub section_cap: Color32,
    /// The section hatch's other colours by part, after `section_cap` (Preferences > Graphics >
    /// Section hatch): of its family and lightness, so a cut stays calm.
    pub section_hatch: [Color32; 2],
    /// Closed sketch profiles (translucent fill).
    pub profile_fill: Color32,
    /// Constraint glyph boxes.
    pub glyph_bg: Color32,
    pub glyph_edge: Color32,
    pub glyph_text: Color32,
    /// Chips drawn over the viewport (status, navigation bar, dimension labels).
    pub overlay: Color32,
    /// Rubber band of the shape being drawn.
    pub rubber_band: Color32,
    /// Light text on the application bar.
    pub app_bar_text: Color32,
    /// A hovered tab on the application bar (keeps `app_bar_text` readable).
    pub app_bar_hover: Color32,
    pub app_bar_hover_subtle: Color32,
    pub app_bar_hover_button: Color32,
    pub app_bar_hover_strong: Color32,
    /// The active document tab: a solid surface lifted from the bar.
    pub app_bar_tab_active: Color32,
    /// Dense feature dialogs: the body surface, the footer band, the dividers, input controls
    /// (fill and border), field labels, and the selected-element foreground and background.
    pub dialog_body: Color32,
    pub dialog_footer: Color32,
    pub dialog_rule: Color32,
    pub control_fill: Color32,
    pub control_border: Color32,
    pub dialog_label: Color32,
    pub selected_ink: Color32,
    pub selected_bg: Color32,
    /// Ground under every Preferences figure.
    pub figure_ground: Color32,
    /// Outlines and reference lines of Preferences figures.
    pub figure_line: Color32,
    /// Neutral light reflected by a model in a Preferences figure, independent of theme polarity.
    pub figure_highlight: Color32,
    /// Neutral shadow cast by a model in a Preferences figure, independent of theme polarity.
    pub figure_shadow: Color32,
    /// Top face of the Preferences figure cube.
    pub figure_face_top: Color32,
    /// Left face of the Preferences figure cube.
    pub figure_face_left: Color32,
    /// Right face of the Preferences figure cube.
    pub figure_face_right: Color32,
    /// Paper, panels and pads drawn inside Preferences figures.
    pub figure_card: Color32,
    /// Fixed white label and highlight ink inside Preferences illustrations.
    pub figure_white: Color32,
    /// Fixed black hatch and shadow ink inside Preferences illustrations.
    pub figure_black: Color32,
    /// What a Preferences figure or selected card acts on.
    pub figure_accent: Color32,
    /// Reference point in a Preferences figure: pivot, origin, pointer.
    pub figure_reference: Color32,
    /// X axis in a Preferences figure.
    pub figure_axis_x: Color32,
    /// Y axis in a Preferences figure.
    pub figure_axis_y: Color32,
    /// Z axis in a Preferences figure.
    pub figure_axis_z: Color32,
    /// Done or fully constrained mark in a Preferences figure.
    pub figure_ok: Color32,
    /// Under-constrained sketch mark in a Preferences figure.
    pub figure_warn: Color32,
    /// Conflicting sketch geometry or state glyph in a Preferences figure.
    pub figure_conflict: Color32,
    /// Segmented control well and tint behind figures in Preferences.
    pub pref_well: Color32,
    /// Warning badge text and stroke in Preferences.
    pub pref_note: Color32,
    /// Warning badge fill in Preferences.
    pub pref_note_fill: Color32,
    /// Ink on a Preferences accent fill.
    pub pref_on_accent: Color32,
    /// Success badge text in Preferences.
    pub pref_ok: Color32,
    /// Success badge fill in Preferences.
    pub pref_ok_fill: Color32,
    /// Ink of a dimmed Preferences control.
    pub pref_disabled: Color32,
    /// Timeline items: normal and failed.
    pub timeline_item: Color32,
    pub timeline_item_error: Color32,
    pub timeline_marker: Color32,
    /// Button and field backgrounds on panels.
    pub field: Color32,
    // ---- box selection ----
    pub box_window: Color32,
    pub box_crossing: Color32,
    // ---- colours that came from the engine or were drawn in place ----
    /// Sketch curves the engine tags (`view::colors`): projected, lost link, construction,
    /// centerline.
    pub sketch_projected: Color32,
    pub sketch_lost: Color32,
    pub sketch_construction: Color32,
    pub sketch_centerline: Color32,
    /// Sheet metal flat pattern: bend up, bend down.
    pub bend_up: Color32,
    pub bend_down: Color32,
    /// Interference: the clash fill, before its shading.
    pub interference: Color32,
    /// Curvature Comb: the envelope through the teeth tips (purple) and the teeth, the scaled
    /// normals (orange).
    pub comb_envelope: Color32,
    pub comb_teeth: Color32,
    /// Continuity Analysis seams, best to worst as `inspect.continuity` judges them: smooth (G2),
    /// tangent with a curvature jump (G1), a crease (G0) and a gap. Bluish green, blue, yellow
    /// and reddish purple stay apart in every colour vision mode, so no mode changes them.
    pub continuity: [Color32; 4],
    pub analysis: AnalysisColors,
    pub icons: IconColors,
    /// The SolidCraft mark on the application bar: light, mid and dark faces (the gaps between
    /// them show the bar).
    pub logo: [Color32; 3],
}

impl Tokens {
    /// An expanded outline follows its fill, including a theme's deliberately square corners.
    pub fn outset_radius(radius: f32, outset: f32) -> f32 {
        if radius > 0.0 { radius + outset } else { 0.0 }
    }

    /// Subtle shell elevation, derived from the semantic application-bar colour.
    pub fn shell_shadow(self) -> egui::Shadow {
        egui::Shadow { offset: [0, self.space_2 as i8], blur: self.space_8 as u8, spread: 0, color: self.app_bar.gamma_multiply(0.18) }
    }

    /// The X, Y and Z axis colours, in that order.
    pub const fn axes(&self) -> [Color32; 3] {
        [self.axis_x, self.axis_y, self.axis_z]
    }

    /// The tokens of the current theme and colour vision mode, the colours drawn in the 3D view
    /// from the view's own dark or light ([`viewport_dark`]).
    pub fn get() -> Tokens {
        Tokens::get_in(vision())
    }

    /// [`Tokens::get`] in another colour vision mode: the Colour vision preference previews the
    /// mode being chosen before it is applied (#7152).
    pub fn get_in(mode: ColorVision) -> Tokens {
        if let Some(tokens) = PANEL_TOKENS.get() {
            return tokens;
        }
        if let Some(published) = PUBLISHED.get().filter(|p| p.environment == Environment::current()) {
            if mode == published.environment.vision {
                return published.tokens;
            }
            let mut tokens = Self::base_in(mode);
            Surfaces::of(published.tokens).apply(&mut tokens);
            return tokens;
        }
        Self::base_in(mode)
    }

    /// Unanimated colours for geometry cache keys: fading backgrounds must not rebuild bodies.
    pub fn base() -> Tokens {
        Self::base_in(vision())
    }

    fn base_in(mode: ColorVision) -> Tokens {
        let window = Tokens::themed(is_dark(), mode);
        let tokens = if viewport_dark() == is_dark() { window } else { window.with_viewport(Tokens::themed(viewport_dark(), mode)) };
        tokens.with_background(viewport_background())
    }

    /// The tokens of the 3D view's side (#4146), for a window colour drawn straight onto the view
    /// with nothing behind it (a status line, a label or a warning beside the geometry), so it
    /// reads on the view's background. As [`Tokens::get`] unless the view has its own side.
    pub fn view() -> Tokens {
        if let Some(published) = PUBLISHED.get().filter(|p| p.environment == Environment::current()) {
            return published.view;
        }
        Tokens::themed(viewport_dark(), vision()).with_background(viewport_background())
    }

    pub fn protect_tab_panel(&mut self) {
        self.panel = readable_fill(self.panel, &[self.text, self.text_dim, self.form_ink, self.dialog_label], 1.0);
    }

    /// Scale only floating surface fill alpha, preserving full-opacity tokens exactly.
    /// Raise each fill's floor to protect text over both black and white viewport extremes.
    pub fn apply_panel_opacity(&mut self, percent: f32) {
        if !percent.is_finite() || percent >= 100.0 {
            return;
        }
        let scale = percent.clamp(40.0, 100.0) / 100.0;
        let body_inks = [self.text, self.form_ink, self.dialog_label];
        let all_inks = [self.text, self.form_ink, self.dialog_label, self.text_dim, self.form_dim];
        for fill in [
            &mut self.panel,
            &mut self.panel_header,
            &mut self.toolbar,
            &mut self.timeline,
            &mut self.dialog_bg,
            &mut self.dialog_body,
            &mut self.dialog_footer,
            &mut self.overlay,
        ] {
            let protected = readable_fill(*fill, &all_inks, scale);
            // Some authored dim inks cannot meet the target without an opaque fill.
            // Keep the existing primary-text floor in that case so opacity still works.
            *fill = if protected.a() < 255 { protected } else { readable_fill(*fill, &body_inks, scale) };
        }
        self.nav_tip = readable_fill(self.nav_tip, &[self.nav_tip_ink], scale);
    }

    /// Scale neutral backing colours before opacity protection; semantic ink stays authored.
    pub fn apply_surface_brightness(&mut self, percent: f32) {
        if !percent.is_finite() || percent == 100.0 {
            return;
        }
        let scale = percent.clamp(60.0, 120.0) / 100.0;
        let adjust = |color: &mut Color32| {
            let [r, g, b, a] = color.to_srgba_unmultiplied();
            if a > 0 {
                let channel = |v: u8| (f32::from(v) * scale).round().clamp(0.0, 255.0) as u8;
                *color = Color32::from_rgba_unmultiplied(channel(r), channel(g), channel(b), a);
            }
        };
        for color in [
            &mut self.panel,
            &mut self.panel_header,
            &mut self.toolbar,
            &mut self.timeline,
            &mut self.timeline_item,
            &mut self.dialog_bg,
            &mut self.dialog_body,
            &mut self.dialog_footer,
            &mut self.overlay,
            &mut self.field,
            &mut self.control_fill,
            &mut self.app_bar,
            &mut self.app_bar_tab_active,
            &mut self.nav_tip,
            &mut self.nav_field,
            &mut self.nav_viewport,
            &mut self.glyph_bg,
            &mut self.cube_face,
            &mut self.viewport_top,
            &mut self.viewport_bottom,
            &mut self.grid,
            &mut self.grid_major,
            &mut self.border,
            &mut self.dialog_border,
            &mut self.dialog_rule,
            &mut self.control_border,
            &mut self.app_bar_separator,
            &mut self.nav_tip_key_border,
            &mut self.cube_border,
            &mut self.file_background,
            &mut self.file_surface,
            &mut self.file_border,
            &mut self.file_recovery_well,
        ] {
            adjust(color);
        }
        for stroke in [
            &mut self.borders.primary,
            &mut self.borders.secondary,
            &mut self.borders.tertiary,
            &mut self.borders.container,
            &mut self.borders.control,
        ]
        .into_iter()
        .flatten()
        {
            adjust(&mut stroke.color);
        }
        if let Some(inset) = &mut self.borders.primary_inset {
            adjust(&mut inset.stroke.color);
        }
    }

    /// These tokens with the 3D view's background one flat colour (#7688), or as they are for `None`.
    pub fn with_background(mut self, colour: Option<[u8; 3]>) -> Tokens {
        if let Some([r, g, b]) = colour {
            self.viewport_top = Color32::from_rgb(r, g, b);
            self.viewport_bottom = self.viewport_top;
        }
        self
    }

    /// These tokens with the colours drawn in the 3D view taken from `view`: the background and
    /// grid, bodies, selection and hover, the origin and construction geometry, previews, sketches
    /// and their dimensions and glyphs, the navigation cube and triads, box selection and the
    /// analysis colours. The window keeps its own: panels, dialogs, menus, and the chips and cards
    /// over the view (`overlay` with its text, icons and borders).
    pub fn with_viewport(self, view: Tokens) -> Tokens {
        Tokens {
            sel_face: view.sel_face,
            sel_edge: view.sel_edge,
            sel_edge_rim: view.sel_edge_rim,
            hover_edge: view.hover_edge,
            hover_edge_halo: view.hover_edge_halo,
            hover_face_lift: view.hover_face_lift,
            sel_profile: view.sel_profile,
            sel_profile_edge: view.sel_profile_edge,
            hover_profile: view.hover_profile,
            hover_profile_edge: view.hover_profile_edge,
            sel_vertex: view.sel_vertex,
            origin_plane: view.origin_plane,
            origin_plane_edge: view.origin_plane_edge,
            origin_plane_hover: view.origin_plane_hover,
            origin_point: view.origin_point,
            axis_x: view.axis_x,
            axis_y: view.axis_y,
            axis_z: view.axis_z,
            construction_plane: view.construction_plane,
            construction_geom: view.construction_geom,
            body: view.body,
            body_edge: view.body_edge,
            manipulator: view.manipulator,
            joint_manipulator: view.joint_manipulator,
            joint_axis: view.joint_axis,
            preview_add: view.preview_add,
            preview_new: view.preview_new,
            preview_sketch: view.preview_sketch,
            preview_cut: view.preview_cut,
            cube_face: view.cube_face,
            cube_border: view.cube_border,
            cube_text: view.cube_text,
            cube_hover: view.cube_hover,
            viewport_top: view.viewport_top,
            viewport_bottom: view.viewport_bottom,
            grid: view.grid,
            grid_major: view.grid_major,
            body_selected: view.body_selected,
            sketch: view.sketch,
            sketch_fixed: view.sketch_fixed,
            sketch_locked: view.sketch_locked,
            sketch_conflict: view.sketch_conflict,
            dimension: view.dimension,
            dimension_driven: view.dimension_driven,
            sketch_point: view.sketch_point,
            sketch_point_free: view.sketch_point_free,
            section_cap: view.section_cap,
            section_hatch: view.section_hatch,
            profile_fill: view.profile_fill,
            glyph_bg: view.glyph_bg,
            glyph_edge: view.glyph_edge,
            glyph_text: view.glyph_text,
            rubber_band: view.rubber_band,
            box_window: view.box_window,
            box_crossing: view.box_crossing,
            sketch_projected: view.sketch_projected,
            sketch_lost: view.sketch_lost,
            sketch_construction: view.sketch_construction,
            sketch_centerline: view.sketch_centerline,
            bend_up: view.bend_up,
            bend_down: view.bend_down,
            interference: view.interference,
            comb_envelope: view.comb_envelope,
            comb_teeth: view.comb_teeth,
            continuity: view.continuity,
            analysis: view.analysis,
            ..self
        }
    }

    /// The tokens of a theme and colour vision mode: a theme file's palette when one is on for
    /// that base ([`set_custom`]), else the built-in one; the colour vision mode on top.
    pub fn themed(dark: bool, mode: ColorVision) -> Tokens {
        let custom = CUSTOM.get().filter(|c| c.dark == dark).map(|c| c.tokens);
        let t = custom.unwrap_or(if dark { Tokens::dark() } else { Tokens::light() });
        let t = if LOGO_ACCENT.get() { t.with_accent_logo() } else { t };
        t.with_vision(dark, mode)
    }

    /// These tokens with the logo's three faces in shades of the accent: lighter, the accent
    /// itself, darker (as the built-in logo's blues are).
    pub fn with_accent_logo(mut self) -> Tokens {
        let accent = self.accent.to_opaque();
        self.logo = [accent.lerp_to_gamma(Color32::WHITE, 0.22), accent, accent.lerp_to_gamma(Color32::BLACK, 0.2)];
        self
    }

    /// These tokens with a colour vision mode's values on top (the tables in `palettes.rs`); Off
    /// gives them back unchanged.
    pub fn with_vision(self, dark: bool, mode: ColorVision) -> Tokens {
        let mut t = match mode {
            ColorVision::Off => return self,
            ColorVision::Protan | ColorVision::Deutan => self.red_green(dark),
            ColorVision::Tritan => self.blue_yellow(dark),
        };
        // Figure axes carry the same X/Y/Z meanings as the viewport and its navigation triad.
        t.figure_axis_x = t.axis_x;
        t.figure_axis_y = t.axis_y;
        t.figure_axis_z = t.axis_z;
        // Match SketchState::ink: fully constrained, under constrained, and conflicting.
        t.figure_ok = t.sketch_fixed;
        t.figure_warn = t.sketch;
        t.figure_conflict = t.error;
        t
    }

    /// Fade idle cube colours into the viewport while retaining opaque facets.
    pub fn navigation_cube(self, active: bool) -> Tokens {
        if active {
            return self;
        }
        let mut t = self;
        t.cube_face = t.cube_face.lerp_to_gamma(t.viewport_top, 0.55);
        t.cube_border = t.cube_border.lerp_to_gamma(t.viewport_top, 0.55);
        t.cube_text = t.cube_text.lerp_to_gamma(t.viewport_top, 0.55);
        t.axis_x = t.axis_x.lerp_to_gamma(t.viewport_top, 0.55);
        t.axis_y = t.axis_y.lerp_to_gamma(t.viewport_top, 0.55);
        t.axis_z = t.axis_z.lerp_to_gamma(t.viewport_top, 0.55);
        t
    }
}

/// Switch themes (the visuals follow on the next [`apply`]).
pub fn set_dark(dark: bool) {
    DARK.set(dark);
}

fn visuals(dark: bool) -> egui::Visuals {
    let t = Tokens::themed(dark, vision());
    let mut v = if dark { egui::Visuals::dark() } else { egui::Visuals::light() };
    v.panel_fill = t.panel;
    v.window_fill = t.panel;
    v.window_stroke = t.primary_border(t.border);
    v.extreme_bg_color = t.field;
    v.faint_bg_color = t.panel_header;
    v.override_text_color = Some(t.text);
    v.widgets.inactive.weak_bg_fill = t.panel_header;
    v.widgets.inactive.bg_fill = t.panel_header;
    v.widgets.active.weak_bg_fill = t.accent_soft;
    v.selection.bg_fill = t.accent_soft;
    v.selection.stroke = Stroke::new(1.0, t.accent);
    v.widgets.noninteractive.bg_stroke = t.tertiary_border(t.border);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, t.text);
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, t.text);
    v.widgets.hovered.weak_bg_fill = t.hover;
    v.widgets.hovered.bg_fill = t.hover;
    // egui uses the open-state fill for a movable window's active title bar.
    v.widgets.open.weak_bg_fill = t.panel_header;
    for state in [&mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
        state.bg_stroke = t.borders.stroke(BorderRole::Control, state.bg_stroke);
    }
    v.window_corner_radius = egui::CornerRadius::same(t.radius_md);
    v.menu_corner_radius = egui::CornerRadius::same(t.radius_md);
    // Keep the existing 2/3 point egui shapes under the default 4 point small token.
    // Set every interaction state so hovering or focusing cannot restore rounded corners.
    for widget in [&mut v.widgets.noninteractive, &mut v.widgets.inactive, &mut v.widgets.active, &mut v.widgets.open] {
        widget.corner_radius = egui::CornerRadius::same(t.radius_sm / 2);
    }
    v.widgets.hovered.corner_radius = egui::CornerRadius::same(t.radius_sm.saturating_sub(1));
    v
}

pub fn apply(ctx: &egui::Context) {
    let dark = is_dark();
    ctx.set_theme(if dark { egui::Theme::Dark } else { egui::Theme::Light });
    ctx.set_visuals(visuals(dark));
    let t = Tokens::get();
    ctx.global_style_mut(|s| {
        s.spacing.item_spacing = egui::vec2(t.space_6, t.space_4);
        s.spacing.button_padding = egui::vec2(t.space_6, t.space_3);
    });
    // The new visuals are painted on the next pass: without a repaint request the widgets
    // keep the previous theme's colours until some other event schedules a pass (#5802).
    ctx.request_repaint();
}
