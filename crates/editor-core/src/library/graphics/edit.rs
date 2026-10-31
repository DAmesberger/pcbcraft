//! Granular graphic edits for scripts, the CLI and MCP (`library.edit_graphics`).
//! Edits apply in order to a copy of one symbol or footprint draft; the copy
//! replaces the draft only when every edit succeeded and the result is
//! within the persistence bounds, so a failure leaves the draft unchanged.
//! Graphics a footprint source derives are edited through their role, not
//! here.
use super::{DrawingLayer, FootprintGraphic, Item, Pin, Polarity, Side};
use crate::diagnostics::{Error, Result};
use crate::geometry::{Point, point_valid};
use crate::ids::{GraphicId, TerminalId, UnitId};
use crate::library::{Definition, FootprintDefinition, SymbolDefinition};
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
    /// Appended to a footprint on `layer`, on the component or opposite
    /// side.
    Footprint { layer: DrawingLayer, side: Side },
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
    /// Move a footprint graphic to another layer or side.
    SetLayer {
        id: GraphicId,
        layer: DrawingLayer,
        side: Side,
    },
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
    /// Set or clear a footprint's authored pin-1 pad and its markers. The
    /// pad and every marker must exist; markers are silkscreen or assembly
    /// graphics.
    SetPolarity { polarity: Option<Polarity> },
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
/// owning component's terminals (symbols only). Returns the edited body and
/// the IDs `Add` allocated, in order. Bounds are checked by the caller's
/// draft edit.
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
    match definition {
        Definition::Symbol(symbol) => {
            let (symbol, added) = apply_symbol(symbol, terminals, edits)?;
            Ok((Definition::Symbol(symbol), added))
        }
        Definition::Footprint(footprint) => {
            let (footprint, added) = apply_footprint(footprint, edits)?;
            Ok((Definition::Footprint(footprint), added))
        }
        _ => Err(Error::Invalid(
            "graphic edits apply to symbol and footprint drafts".into(),
        )),
    }
}

fn translate_ids(ids: &[GraphicId], delta: Point) -> Result<()> {
    if !point_valid(delta) {
        return Err(Error::Invalid("the move must be finite".into()));
    }
    let unique: BTreeSet<_> = ids.iter().collect();
    if ids.is_empty() || unique.len() != ids.len() {
        return Err(Error::Invalid(
            "translate names each graphic once, at least one".into(),
        ));
    }
    Ok(())
}

fn next_id(next_local: &mut u32, what: &str) -> Result<GraphicId> {
    let id = GraphicId(*next_local);
    *next_local = next_local
        .checked_add(1)
        .ok_or_else(|| Error::Limit(format!("{what} local identities exhausted")))?;
    Ok(id)
}

fn apply_symbol(
    symbol: &SymbolDefinition,
    terminals: &BTreeSet<TerminalId>,
    edits: &[GraphicEdit],
) -> Result<(SymbolDefinition, Vec<GraphicId>)> {
    let mut symbol = symbol.clone();
    let mut added = Vec::new();
    for edit in edits {
        match edit {
            GraphicEdit::Add { target, item } => {
                let GraphicTarget::Unit(unit) = target else {
                    return Err(Error::Invalid(
                        "a symbol graphic is added to a unit, not a footprint layer".into(),
                    ));
                };
                let id = next_id(&mut symbol.next_local, "symbol")?;
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
                translate_ids(ids, *delta)?;
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
            GraphicEdit::SetLayer { .. } | GraphicEdit::SetPolarity { .. } => {
                return Err(Error::Invalid(
                    "layers and polarity belong to footprint graphics".into(),
                ));
            }
        }
    }
    Ok((symbol, added))
}

/// A footprint graphic that is authored, not derived from a source role.
fn footprint_graphic_mut(
    footprint: &mut FootprintDefinition,
    id: GraphicId,
) -> Result<&mut FootprintGraphic> {
    let derived = footprint
        .source
        .as_ref()
        .is_some_and(|s| s.graphics.iter().any(|r| r.id == id));
    if derived {
        return Err(Error::Conflict(format!(
            "graphic {id} is derived from the footprint source; edit its role instead"
        )));
    }
    footprint
        .graphics
        .iter_mut()
        .find(|g| g.id == id)
        .ok_or_else(|| Error::NotFound(format!("graphic {id} is not in this footprint")))
}

fn apply_footprint(
    footprint: &FootprintDefinition,
    edits: &[GraphicEdit],
) -> Result<(FootprintDefinition, Vec<GraphicId>)> {
    let mut footprint = footprint.clone();
    let mut added = Vec::new();
    for edit in edits {
        match edit {
            GraphicEdit::Add { target, item } => {
                let GraphicTarget::Footprint { layer, side } = *target else {
                    return Err(Error::Invalid(
                        "a footprint graphic is added to a layer, not a symbol unit".into(),
                    ));
                };
                let id = next_id(&mut footprint.next_local, "footprint")?;
                footprint.graphics.push(FootprintGraphic {
                    id,
                    layer,
                    side,
                    item: item.clone(),
                });
                added.push(id);
            }
            GraphicEdit::Replace { id, item } => {
                footprint_graphic_mut(&mut footprint, *id)?.item = item.clone();
            }
            GraphicEdit::SetLayer { id, layer, side } => {
                let graphic = footprint_graphic_mut(&mut footprint, *id)?;
                graphic.layer = *layer;
                graphic.side = *side;
            }
            GraphicEdit::Translate { ids, delta } => {
                translate_ids(ids, *delta)?;
                for id in ids {
                    let graphic = footprint_graphic_mut(&mut footprint, *id)?;
                    graphic.item = graphic.item.translated(*delta);
                }
            }
            GraphicEdit::Remove { id } => {
                footprint_graphic_mut(&mut footprint, *id)?;
                footprint.graphics.retain(|g| g.id != *id);
                if let Some(polarity) = &mut footprint.polarity {
                    polarity.markers.retain(|m| m != id);
                }
            }
            GraphicEdit::SetPolarity { polarity } => {
                if let Some(p) = polarity {
                    if footprint.pad(p.pad).is_none() {
                        return Err(Error::NotFound(format!(
                            "pad {} is not in this footprint",
                            p.pad
                        )));
                    }
                    if p.markers.len() > super::MAX_POLARITY_MARKERS {
                        return Err(Error::Limit(format!(
                            "a footprint lists at most {} polarity markers",
                            super::MAX_POLARITY_MARKERS
                        )));
                    }
                    let unique: BTreeSet<_> = p.markers.iter().collect();
                    if unique.len() != p.markers.len() {
                        return Err(Error::Invalid("each polarity marker is listed once".into()));
                    }
                    for marker in &p.markers {
                        let ok = footprint.graphics.iter().any(|g| {
                            g.id == *marker
                                && matches!(
                                    g.layer,
                                    DrawingLayer::Silkscreen | DrawingLayer::Assembly
                                )
                        });
                        if !ok {
                            return Err(Error::NotFound(format!(
                                "marker {marker} is not a silkscreen or assembly graphic of this footprint"
                            )));
                        }
                    }
                }
                footprint.polarity = polarity.clone();
            }
            GraphicEdit::SetPin { .. }
            | GraphicEdit::RemovePin { .. }
            | GraphicEdit::SetPinTextHeight { .. } => {
                return Err(Error::Invalid(
                    "footprints have pads, not symbol pins".into(),
                ));
            }
        }
    }
    Ok((footprint, added))
}
