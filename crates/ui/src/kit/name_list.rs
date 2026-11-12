//! A list of named rows, such as the web app's stored files: the name takes the width the other
//! cells leave. Built on the kit's table grid; see `docs/ui-kit.md` for tokens and usage.
use super::table::{Table, TableRow};
use egui::Ui;

/// The rows of a [`name_list`].
pub struct NameList<'a, 'b> {
    ui: &'a mut Ui,
    table: &'a mut Table<'b>,
    widest: &'a mut Vec<f32>,
}

impl NameList<'_, '_> {}

/// The cells of one [`NameList`] row.
pub struct NameRow<'a, 'b> {
    row: &'a mut TableRow<'b>,
    widest: &'a mut Vec<f32>,
    column: usize,
}

impl NameRow<'_, '_> {}
