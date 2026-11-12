//! The list system: one component family for every list and table
//! (list-system spec, direction A "Ledger").
//!
//! - Pure, without egui: [`model`] (row keys, shown lines, natural order),
//!   [`nav`] (the keyboard and pointer reducer), [`virt`] (virtualisation,
//!   sticky headers, reveal), [`columns`] (column priorities and dropping)
//!   and [`query`] (the filter, read by the one catalog query grammar).
//! - egui: [`tokens`] (every size, colour role and duration, with the
//!   density and reduced-motion settings), [`row`] (the one row painter),
//!   [`view`] (focus, keys, bounded or virtualised layout, reorder, FLIP),
//!   [`table`] (column captions with sort over [`columns`]), [`filter`]
//!   (the filter field, chips and saved filters), [`measure`] (decimal
//!   slots) and [`motion`] (the value flash).
//!
//! Feature code builds [`model::Line`]s from its data each frame, paints
//! each with [`row::paint`] and acts on [`view::ListOutput::effects`]
//! through typed commands; a list never changes the document itself.
// Wave 1 migrates the first lists onto the kit; the parts of its API the
// remaining lists will use (spec §7) are not all called yet.
#![allow(dead_code, reason = "the list kit's API, used as lists migrate (spec §7)")]
pub mod columns;
pub mod edit;
pub mod filter;
pub mod keys;
pub mod measure;
pub mod model;
pub mod motion;
pub mod nav;
pub mod query;
pub mod row;
pub mod state;
pub mod table;
pub mod tokens;
pub mod view;
pub mod virt;

pub use keys::LIST_KEYS;
#[allow(unused_imports, reason = "the list kit's API; lists use it as they migrate")]
pub use {
    model::{Fold, Line, RowKey, natural_cmp},
    nav::Effect,
    row::{Action, Columns, Row, RowResponse, Swatch, Value},
    state::{ListState, Sort},
    tokens::{Density, ListTokens, Prefs},
    view::{Layout, LineCtx, ListOutput, ListView},
};

#[cfg(test)]
mod tests;
