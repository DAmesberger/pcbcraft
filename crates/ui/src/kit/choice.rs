//! Choice elements for dense cards; see `docs/ui-kit.md` for tokens and usage.

/// Text segments retain mixed states and optional compact clipping instead of dropdown fallback.
#[derive(Default)]
pub struct Segments<'a> {
    /// Explicit focus requested by the owning form when a setting is opened for editing.
    pub focus: Option<usize>,
    /// Per-option availability; omitted entries are enabled. Unavailable options stay faded.
    pub enabled: &'a [bool],
    /// Optional per-option refusal reasons. Disabled options remain visible and explain why.
    pub disabled: &'a [Option<&'a str>],
    /// Optional stable scenario targets, independent of translated option labels.
    pub targets: &'a [&'a str],
    /// Optional leading icons retain the same text-segment geometry and accessibility.
    pub icons: &'a [&'a str],
    pub workspace_accent: bool,
    pub dense: bool,
    pub small: bool,
    pub states: &'a [bool],
    /// Per-segment disabled explanation; unavailable choices remain visible.
    pub disabled_reasons: &'a [Option<&'a str>],
    pub compact: bool,
    pub primary: bool,
    pub intrinsic: bool,
    pub proportional: bool,
}

/// One segment of [`segmented_icons`]: a stable key (also the accessible fallback text), the icon
/// master to draw and the hover text. The icon's colours come from the palette (its roles and the
/// accent assigned to its id in `palette.toml`), not from the caller.
pub struct IconSegment {
    pub key: &'static str,
    pub icon: &'static str,
    pub tip: String,
}

/// The host's text for [`text_emphasis`]: the two style names, and why Bold is off.
pub struct EmphasisLabels<'a> {
    pub bold: &'a str,
    pub italic: &'a str,
    pub no_bold: &'a str,
}

/// A command variant supplied by the panel; the kit only draws and returns its key.
pub struct ToolChoice<'a> {
    pub key: &'a str,
    pub icon: &'a str,
    pub label: &'a str,
    pub enabled: bool,
}

/// Thumbnail variant of the shared selection chip. No document or render-job ownership.
#[derive(Default)]
pub struct ThumbnailChoice {
    pub texture: Option<egui::TextureId>,
    pub selected: bool,
    pub modified: bool,
    pub progress: Option<f32>,
}

/// A completion/suggestion entry; text and detail are application-supplied data.
pub struct PopupOption<'a> {
    pub text: &'a str,
    pub detail: &'a str,
    pub kind: &'a str,
    pub color: egui::Color32,
}
