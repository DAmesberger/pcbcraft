//! A list's own state between frames: the selection set, the cursor (focus
//! row) and anchor (range start), the type-ahead buffer, folded groups,
//! the sort and the bounded "show more". Pure data; the egui list keeps it
//! in the context's memory under the list's id ([`ListState::load`]).
use super::model::RowKey;
use std::collections::BTreeSet;

/// How long typed letters keep adding to the type-ahead buffer, in seconds.
pub const TYPEAHEAD_SECONDS: f64 = 0.7;
/// Bytes of the type-ahead buffer.
pub const TYPEAHEAD_MAX: usize = 64;

/// The column a table sorts by and its direction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sort {
    pub column: usize,
    pub ascending: bool,
}

impl Sort {
    /// A click on `column`'s header: a new column sorts ascending, the same
    /// one flips.
    pub fn clicked(current: Option<Self>, column: usize) -> Self {
        match current {
            Some(s) if s.column == column => Self {
                column,
                ascending: !s.ascending,
            },
            _ => Self {
                column,
                ascending: true,
            },
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ListState {
    pub selection: BTreeSet<RowKey>,
    /// The focus row the keyboard moves.
    pub cursor: Option<RowKey>,
    /// Where a ⇧ range starts.
    pub anchor: Option<RowKey>,
    /// Group or tree rows that are folded.
    pub folded: BTreeSet<RowKey>,
    /// Typed letters and when the last one came.
    pub typed: String,
    pub typed_at: f64,
    pub sort: Option<Sort>,
    /// A bounded list shows every row ("Show n more" was pressed).
    pub show_all: bool,
    /// The keyboard moved the cursor last (the focus ring shows only then).
    pub keyboard: bool,
}

impl ListState {
    /// The state of the list `id`, or a fresh one.
    pub fn load(ctx: &egui::Context, id: egui::Id) -> Self {
        ctx.data_mut(|d| d.get_temp::<Self>(id)).unwrap_or_default()
    }

    pub fn store(self, ctx: &egui::Context, id: egui::Id) {
        ctx.data_mut(|d| d.insert_temp(id, self));
    }

    /// The type-ahead buffer while it is fresh at `now`.
    pub fn typing(&self, now: f64) -> Option<&str> {
        (!self.typed.is_empty() && now - self.typed_at <= TYPEAHEAD_SECONDS)
            .then_some(self.typed.as_str())
    }

    /// Select only `key`, as a click does, and make it cursor and anchor.
    pub fn select_one(&mut self, key: RowKey) {
        self.selection.clear();
        self.selection.insert(key);
        self.cursor = Some(key);
        self.anchor = Some(key);
    }
}
