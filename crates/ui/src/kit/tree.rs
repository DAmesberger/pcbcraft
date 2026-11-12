//! Shared Browser tree rows; feature modules supply content and dispatch commands.
use egui::{Color32, Pos2, Rect};

/// Trailing tree action; disabled actions retain their reason and a scenario rectangle.
pub struct TreeAction<'a> {
    pub icon: &'a str,
    pub hover_only: bool,
    pub label: &'a str,
    pub reason: Option<&'a str>,
    pub target: &'a str,
}

#[derive(Default)]
pub struct Row<'a> {
    pub depth: usize,
    /// Compact single-line scene rows, with inset content and a trailing eye.
    pub compact: bool,
    /// Some(open) for rows that fold.
    pub fold: Option<bool>,
    /// Some(visible) for rows with an eye.
    pub eye: Option<bool>,
    /// The eye between the fold arrow and the radio, as before #8718, instead of at the row's end.
    pub eye_left: bool,
    pub icon: &'a str,
    pub label: &'a str,
    /// The label is a name someone gave (a body, sketch, component, …): it is shown as typed,
    /// not translated when it happens to match a UI label.
    pub name: bool,
    pub selected: bool,
    pub dim: bool,
    /// What Cut has marked, until Paste moves it: the row has a dashed outline.
    pub cut: bool,
    pub color: Option<Color32>,
    /// Draw the icon in this one colour instead of its artwork (a sketch's solve state, #7245).
    pub icon_ink: Option<Color32>,
    /// The row's tooltip (a sketch's solve state), after its full name when that is cut short.
    pub tip: Option<&'a str>,
    /// Component rows: is it the active one?
    pub radio: Option<bool>,
    /// Trailing badges: "lock", "pin".
    pub badges: &'a [&'a str],
    /// A button right after the name: (icon, tooltip label), e.g. a feature's profile sketch.
    pub action: Option<(&'a str, &'a str)>,
    /// A message id the row's tooltip names it by (a body row: mesh, surface or solid body).
    pub kind_tip: Option<&'a str>,
    /// Lit by the selection or the 3D view's pointer: the item's own row, or a row leading to it.
    pub mark: Mark,
    /// A feature that failed or built with a warning, or a folded row holding one: its mark at
    /// the row's end, the message in [`Row::tip`].
    pub status: Option<Status>,
    /// Optional document appearance and shared trailing text slots.
    pub swatch: Option<Color32>,
    pub secondary: &'a str,
    pub count: &'a str,
    pub modified: bool,
    pub subtitle: &'a str,
    pub actions: &'a [TreeAction<'a>],
    pub radio_hint: &'a str,
}

/// How a row relates to what the selection and the 3D view's pointer light.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mark {
    #[default]
    None,
    /// A row a lit item is inside (its component, placement, folder or group): a faint bar.
    Branch,
    /// A lit item's own row: a solid bar, and the hover colour unless it is selected.
    Here,
}

/// A feature's build state as its row marks it; the worse one sorts last.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Status {
    Warning,
    Error,
}

impl Status {}

/// What happened on a row.
#[derive(Default)]
pub struct RowResp {
    pub rect: Option<Rect>,
    pub clicked: bool,
    pub double: bool,
    pub secondary: Option<Pos2>,
    pub fold: bool,
    /// The eye was clicked, or a drag over the eyes sets it (#9680, see [`eye_drag_id`]).
    pub eye: bool,
    pub eye_rect: Option<Rect>,
    pub radio: bool,
    /// The row's [`Row::action`] button was clicked.
    pub action: bool,
    pub hovered: bool,
    /// The row's response (drag and drop).
    pub resp: Option<egui::Response>,
    /// The press began on the fold arrow, eye or radio (not a drag of the row).
    pub on_control: bool,
    /// Centres of the fold arrow, eye and radio, for rows that publish them under other handles.
    pub fold_at: Option<Pos2>,
    pub eye_at: Option<Pos2>,
    pub radio_at: Option<Pos2>,
    /// Where the label starts.
    pub label_left: Option<f32>,
    pub actions: Vec<egui::Response>,
}
