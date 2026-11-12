//! Dense editable tables. Geometry and typography share the form label-column tokens.
use egui::Ui;

/// A column's minimum width is a multiple of the measured form label column.
pub struct Column<'a> {
    label: &'a str,
    width: f32,
    min_content_width: f32,
    icon_only: bool,
    flexible: bool,
    /// A width the user dragged the column to; it replaces the automatic width and stops sharing.
    manual: Option<f32>,
}

impl<'a> Column<'a> {
    pub fn new(label: &'a str, width: f32) -> Self {
        Self { label, width, min_content_width: 0.0, icon_only: false, flexible: false, manual: None }
    }
}

pub struct Table<'a> {
    columns: &'a [Column<'a>],
    widths: Vec<f32>,
    id: String,
    rows: usize,
    empty: String,
    empty_shown: bool,
    reveal: Option<egui::Rect>,
}

impl Table<'_> {}

pub struct TableRow<'a> {
    ui: &'a mut Ui,
    widths: &'a [f32],
    columns: &'a [Column<'a>],
    rect: egui::Rect,
    column: usize,
    left: f32,
    id: String,
}

impl TableRow<'_> {}

/// How a table orders its rows by one column. `Listed` keeps the order the owner gave them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Sort {
    #[default]
    Listed,
    Ascending,
    Descending,
}

impl Sort {}
