//! Granular graphic edits for scripts, the CLI and MCP (`library.edit_graphics`).
//! Edits apply in order to a copy of one symbol draft; the copy replaces the
//! draft only when every edit succeeded and the result is within the
//! persistence bounds, so a failure leaves the draft unchanged. Footprint
//! targets, layers and polarity arrive with footprint graphics.
use super::{Item, Pin};
use crate::diagnostics::{Error, Result};
use crate::geometry::{Point, point_valid};
use crate::ids::{GraphicId, TerminalId, UnitId};
use crate::library::{Definition, SymbolDefinition};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Edits one `library.edit_graphics` applies together.
pub const MAX_GRAPHIC_EDITS: usize = 256;

/// Where an added graphic goes.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum GraphicTarget {
    /// Appended (drawn on top) to a symbol unit.
    Unit(UnitId),
}

/// One change to a draft's drawing. Graphics are named by their stable
/// local ID; pins by their terminal.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum GraphicEdit {
    /// Add a graphic with a new ID; the outcome lists new IDs in order.
    Add { target: GraphicTarget, item: Item },
    /// Replace what a graphic draws; its ID and place in the order stay.
    Replace { id: GraphicId, item: Item },
    /// Move graphics by `delta` millimetres.
    Translate { ids: Vec<GraphicId>, delta: Point },
    /// Remove a graphic; its ID is retired.
    Remove { id: GraphicId },
    /// Add or replace the pin of `pin.terminal`, moving it to `unit`. The
    /// terminal must exist on the owning component; none is inferred.
    SetPin { unit: UnitId, pin: Pin },
    /// Remove a terminal's pin; the terminal stays.
    RemovePin { terminal: TerminalId },
    /// Height of pin names and numbers, 0.1–50 mm.
    SetPinTextHeight { height: f64 },
}

fn missing(id: GraphicId) -> Error {
    Error::NotFound(format!("graphic {id} is not in this symbol"))
}

fn graphic_mut(symbol: &mut SymbolDefinition, id: GraphicId) -> Result<&mut super::Graphic> {
    symbol
        .units
        .iter_mut()
        .flat_map(|u| u.graphics.iter_mut())
        .find(|g| g.id == id)
        .ok_or_else(|| missing(id))
}

/// Apply `edits` in order to a copy of `definition`. `terminals` are the
/// owning component's terminals. Returns the edited body and the IDs `Add`
/// allocated, in order. Bounds are checked by the caller's draft edit.
pub fn apply(
    definition: &Definition,
    terminals: &BTreeSet<TerminalId>,
    edits: &[GraphicEdit],
) -> Result<(Definition, Vec<GraphicId>)> {
    if edits.is_empty() || edits.len() > MAX_GRAPHIC_EDITS {
        return Err(Error::Invalid(format!(
            "edit graphics with 1–{MAX_GRAPHIC_EDITS} edits at a time"
        )));
    }
    let Definition::Symbol(symbol) = definition else {
        return Err(Error::Invalid(
            "graphic edits apply to symbol drafts; footprint graphics are not available yet".into(),
        ));
    };
    let mut symbol = symbol.clone();
    let mut added = Vec::new();
    for edit in edits {
        match edit {
            GraphicEdit::Add { target, item } => {
                let GraphicTarget::Unit(unit) = target;
                let id = GraphicId(symbol.next_local);
                symbol.next_local = symbol
                    .next_local
                    .checked_add(1)
                    .ok_or_else(|| Error::Limit("symbol local identities exhausted".into()))?;
                symbol
                    .units
                    .iter_mut()
                    .find(|u| u.id == *unit)
                    .ok_or_else(|| Error::NotFound(format!("unit {unit} is not in this symbol")))?
                    .graphics
                    .push(super::Graphic {
                        id,
                        item: item.clone(),
                    });
                added.push(id);
            }
            GraphicEdit::Replace { id, item } => {
                graphic_mut(&mut symbol, *id)?.item = item.clone();
            }
            GraphicEdit::Translate { ids, delta } => {
                if !point_valid(*delta) {
                    return Err(Error::Invalid("the move must be finite".into()));
                }
                let unique: BTreeSet<_> = ids.iter().collect();
                if ids.is_empty() || unique.len() != ids.len() {
                    return Err(Error::Invalid(
                        "translate names each graphic once, at least one".into(),
                    ));
                }
                for id in ids {
                    let graphic = graphic_mut(&mut symbol, *id)?;
                    graphic.item = graphic.item.translated(*delta);
                }
            }
            GraphicEdit::Remove { id } => {
                let unit = symbol
                    .units
                    .iter_mut()
                    .find(|u| u.graphics.iter().any(|g| g.id == *id))
                    .ok_or_else(|| missing(*id))?;
                unit.graphics.retain(|g| g.id != *id);
            }
            GraphicEdit::SetPin { unit, pin } => {
                if !terminals.contains(&pin.terminal) {
                    return Err(Error::NotFound(format!(
                        "terminal {} is not on the owning component",
                        pin.terminal
                    )));
                }
                if symbol.unit(*unit).is_none() {
                    return Err(Error::NotFound(format!(
                        "unit {unit} is not in this symbol"
                    )));
                }
                for u in &mut symbol.units {
                    if u.id == *unit {
                        match u.pins.iter_mut().find(|p| p.terminal == pin.terminal) {
                            Some(existing) => *existing = pin.clone(),
                            None => u.pins.push(pin.clone()),
                        }
                    } else {
                        u.pins.retain(|p| p.terminal != pin.terminal);
                    }
                }
            }
            GraphicEdit::RemovePin { terminal } => {
                let before: usize = symbol.units.iter().map(|u| u.pins.len()).sum();
                for u in &mut symbol.units {
                    u.pins.retain(|p| p.terminal != *terminal);
                }
                let after: usize = symbol.units.iter().map(|u| u.pins.len()).sum();
                if before == after {
                    return Err(Error::NotFound(format!("terminal {terminal} has no pin")));
                }
            }
            GraphicEdit::SetPinTextHeight { height } => {
                symbol.pin_text_height = *height;
            }
        }
    }
    Ok((Definition::Symbol(symbol), added))
}
