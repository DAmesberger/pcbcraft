//! Sheets, the hierarchy and sheet furniture: adding, renaming, ordering
//! and deleting sheets; sheet symbols and their pins; notes, lines and
//! rectangles; title-block properties; moving a unit to another sheet.
use super::labels::{check_name, check_point, check_sheet, detach_point};
use super::{Result, conflict, invalid, not_found, wiring};
use crate::diagnostics::Error;
use crate::geometry::{Point, point_valid};
use crate::ids::{Id, SheetId, printable};
use crate::model::{
    LabelKind, Paper, PaperOrientation, PortShape, Project, ProjectProperties, Sheet, SheetDrawing,
    SheetItem, SheetPin, SheetSide, SheetSymbol, WireEnd,
};
use std::collections::BTreeSet;

/// Sheets a project holds.
pub const MAX_SHEETS: usize = 256;
/// Sheet symbols, and pins on one symbol.
pub const MAX_SHEET_SYMBOLS: usize = 1024;
pub const MAX_SHEET_PINS: usize = 512;
/// Notes, lines and rectangles a project holds.
pub const MAX_DRAWINGS: usize = 16384;
/// Longest sheet name, in bytes.
pub const MAX_SHEET_NAME: usize = 64;
/// Longest note, in bytes.
pub const MAX_NOTE: usize = 4096;
/// Note cap heights, in mm.
pub const NOTE_HEIGHT_MM: (f64, f64) = (0.5, 50.0);
/// Sheet symbol sides, in mm.
pub const SYMBOL_SIZE_MM: (f64, f64) = (5.0, 2000.0);
/// Spacing of pins [`sync_pins`] adds, in mm.
pub const PIN_PITCH_MM: f64 = 5.0;

fn check_sheet_name(project: &Project, name: &str, except: Option<SheetId>) -> Result<()> {
    if !printable(name) || name.len() > MAX_SHEET_NAME {
        return Err(invalid(
            "Sheet names must be 1–64 bytes without control characters.",
        ));
    }
    if project
        .sheets
        .list
        .iter()
        .any(|s| Some(s.id) != except && s.name.trim().eq_ignore_ascii_case(name.trim()))
    {
        return Err(conflict("Another sheet already uses this name."));
    }
    Ok(())
}

/// The first free "Sheet n" name.
fn default_sheet_name(project: &Project) -> String {
    (1..=MAX_SHEETS + 1)
        .map(|n| format!("Sheet {n}"))
        .find(|name| {
            !project
                .sheets
                .list
                .iter()
                .any(|s| s.name.eq_ignore_ascii_case(name))
        })
        .unwrap_or_else(|| "Sheet".into())
}

/// Append a sheet. Its name defaults to the first free "Sheet n".
pub fn add(
    project: &mut Project,
    name: Option<String>,
    paper: Option<Paper>,
    orientation: Option<PaperOrientation>,
) -> Result<SheetId> {
    if project.sheets.list.len() >= MAX_SHEETS {
        return Err(Error::Limit(
            "This edition supports at most 256 sheets.".into(),
        ));
    }
    let name = name.unwrap_or_else(|| default_sheet_name(project));
    check_sheet_name(project, &name, None)?;
    let id = SheetId(project.sheets.next);
    project.sheets.next = project
        .sheets
        .next
        .checked_add(1)
        .ok_or_else(|| Error::Limit("sheet identity space exhausted".into()))?;
    project.sheets.list.push(Sheet {
        id,
        name,
        paper: paper.unwrap_or_default(),
        orientation: orientation.unwrap_or_default(),
    });
    Ok(id)
}

fn sheet_mut(project: &mut Project, id: SheetId) -> Result<&mut Sheet> {
    project
        .sheets
        .list
        .iter_mut()
        .find(|s| s.id == id)
        .ok_or_else(|| not_found("Unknown sheet."))
}

pub fn rename(project: &mut Project, id: SheetId, name: String) -> Result<()> {
    check_sheet_name(project, &name, Some(id))?;
    sheet_mut(project, id)?.name = name;
    Ok(())
}

pub fn set_page(
    project: &mut Project,
    id: SheetId,
    paper: Paper,
    orientation: PaperOrientation,
) -> Result<()> {
    let sheet = sheet_mut(project, id)?;
    sheet.paper = paper;
    sheet.orientation = orientation;
    Ok(())
}

/// Move a sheet to page `index` (from 0, clamped to the last page).
pub fn reorder(project: &mut Project, id: SheetId, index: usize) -> Result<()> {
    let from = project
        .sheets
        .list
        .iter()
        .position(|s| s.id == id)
        .ok_or_else(|| not_found("Unknown sheet."))?;
    let sheet = project.sheets.list.remove(from);
    let index = index.min(project.sheets.list.len());
    project.sheets.list.insert(index, sheet);
    Ok(())
}

/// Delete a sheet and everything drawn on it: its units return to
/// unplaced, its wires, junctions, labels, sheet symbols and furniture go,
/// and the sheet symbol that used it goes with its pins. Sheets its own
/// sheet symbols used become top-level sheets. The last sheet stays.
pub fn delete(project: &mut Project, id: SheetId) -> Result<()> {
    check_sheet(project, id)?;
    if project.sheets.list.len() <= 1 {
        return Err(conflict("A schematic keeps at least one sheet."));
    }
    // Wires on the sheet, and those on the pins of the symbol using it.
    let parent_pins: BTreeSet<WireEnd> = project
        .parent_symbol(id)
        .map(|s| s.pins.iter().map(|p| WireEnd::sheet_pin(p.id)).collect())
        .unwrap_or_default();
    let doomed: BTreeSet<Id> = project
        .wires
        .iter()
        .filter(|w| {
            wiring::wire_sheet(project, w) == Some(id)
                || parent_pins.contains(&w.from)
                || parent_pins.contains(&w.to)
        })
        .map(|w| w.id)
        .collect();
    project.wires.retain(|w| !doomed.contains(&w.id));
    project.occurrences.retain(|o| o.sheet != id);
    project.junctions.retain(|j| j.sheet != id);
    project.labels.retain(|l| l.sheet != id);
    project.drawings.retain(|d| d.sheet != id);
    project
        .sheet_symbols
        .retain(|s| s.sheet != id && s.child != id);
    project.sheets.list.retain(|s| s.id != id);
    Ok(())
}

/// The sheets above `sheet` through the sheet symbols that use them,
/// nearest first.
fn ancestors(project: &Project, sheet: SheetId) -> Vec<SheetId> {
    let mut out = Vec::new();
    let mut at = sheet;
    while let Some(symbol) = project.parent_symbol(at) {
        if out.contains(&symbol.sheet) || out.len() > project.sheets.list.len() {
            break;
        }
        out.push(symbol.sheet);
        at = symbol.sheet;
    }
    out
}

fn check_size(size: Point) -> Result<()> {
    let ok = |v: f64| v.is_finite() && (SYMBOL_SIZE_MM.0..=SYMBOL_SIZE_MM.1).contains(&v);
    if ok(size[0]) && ok(size[1]) {
        Ok(())
    } else {
        Err(invalid("Sheet symbols are 5–2000 mm on each side."))
    }
}

/// Whether `child` may be used by a symbol drawn on `sheet`: not the sheet
/// itself, not one of its ancestors, and used by no other symbol.
fn check_child(
    project: &Project,
    sheet: SheetId,
    child: SheetId,
    except: Option<Id>,
) -> Result<()> {
    check_sheet(project, child)?;
    if child == sheet || ancestors(project, sheet).contains(&child) {
        return Err(conflict(
            "A sheet cannot contain itself or a sheet above it.",
        ));
    }
    if project
        .sheet_symbols
        .iter()
        .any(|s| s.child == child && Some(s.id) != except)
    {
        return Err(conflict(
            "That sheet is already used by another sheet symbol; reusing a sheet twice is not supported yet.",
        ));
    }
    Ok(())
}

/// What `sheet_symbol.place` puts down.
pub struct NewSheetSymbol {
    pub sheet: SheetId,
    /// An existing sheet to use; a new sheet when omitted.
    pub child: Option<SheetId>,
    /// The new child sheet's name.
    pub name: Option<String>,
    pub position: Point,
    pub size: Point,
}

/// Draw a sheet symbol for `child`, or for a new sheet appended to the
/// page order. Returns the symbol and its child sheet.
pub fn place_symbol(project: &mut Project, new: NewSheetSymbol) -> Result<(Id, SheetId)> {
    check_sheet(project, new.sheet)?;
    check_point(new.position)?;
    check_size(new.size)?;
    if project.sheet_symbols.len() >= MAX_SHEET_SYMBOLS {
        return Err(Error::Limit(
            "This edition supports at most 1024 sheet symbols.".into(),
        ));
    }
    let child = match new.child {
        Some(child) => {
            if new.name.is_some() {
                return Err(invalid(
                    "Name a new child sheet only; an existing sheet keeps its name.",
                ));
            }
            child
        }
        None => add(project, new.name, None, None)?,
    };
    check_child(project, new.sheet, child, None)?;
    let id = project.allocate()?;
    project.sheet_symbols.push(SheetSymbol {
        id,
        sheet: new.sheet,
        child,
        position: new.position,
        size: new.size,
        pins: Vec::new(),
    });
    Ok((id, child))
}

fn symbol_mut(project: &mut Project, id: Id) -> Result<&mut SheetSymbol> {
    project
        .sheet_symbols
        .iter_mut()
        .find(|s| s.id == id)
        .ok_or_else(|| not_found("Unknown sheet symbol."))
}

fn side_length(size: Point, side: SheetSide) -> f64 {
    match side {
        SheetSide::Left | SheetSide::Right => size[1],
        SheetSide::Top | SheetSide::Bottom => size[0],
    }
}

fn check_pins_fit(symbol: &SheetSymbol) -> Result<()> {
    for pin in &symbol.pins {
        if !pin.offset.is_finite()
            || pin.offset < 0.0
            || pin.offset > side_length(symbol.size, pin.side)
        {
            return Err(invalid(
                "A sheet pin must lie on its side of the sheet symbol.",
            ));
        }
    }
    Ok(())
}

/// Move a sheet symbol; wires on its pins follow.
pub fn move_symbol(project: &mut Project, id: Id, position: Point) -> Result<()> {
    check_point(position)?;
    symbol_mut(project, id)?.position = position;
    Ok(())
}

/// Resize a sheet symbol; refused when a pin would leave its side.
pub fn resize_symbol(project: &mut Project, id: Id, size: Point) -> Result<()> {
    check_size(size)?;
    let symbol = symbol_mut(project, id)?;
    symbol.size = size;
    check_pins_fit(symbol)
}

/// Point a sheet symbol at another sheet.
pub fn set_child(project: &mut Project, id: Id, child: SheetId) -> Result<()> {
    let sheet = project
        .sheet_symbol(id)
        .map(|s| s.sheet)
        .ok_or_else(|| not_found("Unknown sheet symbol."))?;
    check_child(project, sheet, child, Some(id))?;
    symbol_mut(project, id)?.child = child;
    Ok(())
}

/// Remove a sheet symbol. Its child sheet stays as a top-level sheet; wires
/// on its pins stay joined to each other at junctions.
pub fn remove_symbol(project: &mut Project, id: Id) -> Result<()> {
    let symbol = project
        .sheet_symbol(id)
        .cloned()
        .ok_or_else(|| not_found("Unknown sheet symbol."))?;
    for pin in &symbol.pins {
        detach_point(
            project,
            WireEnd::sheet_pin(pin.id),
            symbol.sheet,
            symbol.pin_position(pin),
        )?;
    }
    project.sheet_symbols.retain(|s| s.id != id);
    Ok(())
}

fn check_pin_name(symbol: &SheetSymbol, name: &str, except: Option<Id>) -> Result<()> {
    check_name(name)?;
    if symbol
        .pins
        .iter()
        .any(|p| Some(p.id) != except && p.name.eq_ignore_ascii_case(name))
    {
        return Err(conflict(
            "This sheet symbol already has a pin of this name.",
        ));
    }
    Ok(())
}

/// What `sheet_pin.add` puts on a symbol's edge.
pub struct NewSheetPin {
    pub symbol: Id,
    pub name: String,
    pub shape: PortShape,
    pub side: SheetSide,
    pub offset: f64,
}

pub fn add_pin(project: &mut Project, new: NewSheetPin) -> Result<Id> {
    let symbol = project
        .sheet_symbol(new.symbol)
        .ok_or_else(|| not_found("Unknown sheet symbol."))?;
    check_pin_name(symbol, &new.name, None)?;
    if symbol.pins.len() >= MAX_SHEET_PINS {
        return Err(Error::Limit(
            "This edition supports at most 512 pins on a sheet symbol.".into(),
        ));
    }
    let id = project.allocate()?;
    let symbol = symbol_mut(project, new.symbol)?;
    symbol.pins.push(SheetPin {
        id,
        name: new.name,
        shape: new.shape,
        side: new.side,
        offset: new.offset,
    });
    check_pins_fit(symbol)?;
    Ok(id)
}

/// The symbol holding pin `pin`.
fn pin_symbol(project: &Project, pin: Id) -> Result<Id> {
    project
        .sheet_pin(pin)
        .map(|(s, _)| s.id)
        .ok_or_else(|| not_found("Unknown sheet pin."))
}

/// Rename, reshape or move a pin along the symbol's edge. A new name joins
/// the hierarchical labels of that name on the child sheet instead.
pub fn edit_pin(
    project: &mut Project,
    pin: Id,
    name: Option<String>,
    shape: Option<PortShape>,
    side: Option<SheetSide>,
    offset: Option<f64>,
) -> Result<()> {
    let symbol_id = pin_symbol(project, pin)?;
    if let Some(name) = &name
        && let Some(symbol) = project.sheet_symbol(symbol_id)
    {
        check_pin_name(symbol, name, Some(pin))?;
    }
    let symbol = symbol_mut(project, symbol_id)?;
    let item = symbol
        .pins
        .iter_mut()
        .find(|p| p.id == pin)
        .ok_or_else(|| not_found("Unknown sheet pin."))?;
    if let Some(name) = name {
        item.name = name;
    }
    if let Some(shape) = shape {
        item.shape = shape;
    }
    if let Some(side) = side {
        item.side = side;
    }
    if let Some(offset) = offset {
        item.offset = offset;
    }
    check_pins_fit(symbol)
}

/// Remove a pin; wires on it stay joined to each other at a junction.
pub fn remove_pin(project: &mut Project, pin: Id) -> Result<()> {
    let symbol_id = pin_symbol(project, pin)?;
    let (sheet, at) = project
        .sheet_pin(pin)
        .map(|(s, p)| (s.sheet, s.pin_position(p)))
        .ok_or_else(|| not_found("Unknown sheet pin."))?;
    detach_point(project, WireEnd::sheet_pin(pin), sheet, at)?;
    symbol_mut(project, symbol_id)?.pins.retain(|p| p.id != pin);
    Ok(())
}

/// Add a pin for every hierarchical label name on the child sheet that the
/// symbol has no pin for, down its left side 5 mm apart (growing the symbol
/// to fit), with the label's shape. Returns the pins added, in name order.
pub fn sync_pins(project: &mut Project, symbol: Id) -> Result<Vec<Id>> {
    let item = project
        .sheet_symbol(symbol)
        .cloned()
        .ok_or_else(|| not_found("Unknown sheet symbol."))?;
    let mut wanted: Vec<(String, PortShape)> = Vec::new();
    let mut seen: BTreeSet<String> = item
        .pins
        .iter()
        .map(|p| p.name.to_ascii_lowercase())
        .collect();
    let mut labels: Vec<_> = project
        .labels
        .iter()
        .filter(|l| l.sheet == item.child)
        .filter_map(|l| match l.kind {
            LabelKind::Hierarchical { shape } => Some((l.name.clone(), shape)),
            _ => None,
        })
        .collect();
    labels.sort_by(|a, b| a.0.cmp(&b.0));
    for (name, shape) in labels {
        if seen.insert(name.to_ascii_lowercase()) {
            wanted.push((name, shape));
        }
    }
    let mut offset = item
        .pins
        .iter()
        .filter(|p| p.side == SheetSide::Left)
        .map(|p| p.offset)
        .fold(0.0, f64::max);
    let mut added = Vec::with_capacity(wanted.len());
    for (name, shape) in wanted {
        offset += PIN_PITCH_MM;
        let needed = offset + PIN_PITCH_MM;
        if needed > SYMBOL_SIZE_MM.1 {
            return Err(Error::Limit(
                "The sheet symbol cannot grow to fit every pin.".into(),
            ));
        }
        let current = project.sheet_symbol(symbol).map_or(0.0, |s| s.size[1]);
        if needed > current {
            symbol_mut(project, symbol)?.size[1] = needed;
        }
        added.push(add_pin(
            project,
            NewSheetPin {
                symbol,
                name,
                shape,
                side: SheetSide::Left,
                offset,
            },
        )?);
    }
    Ok(added)
}

fn check_item(item: &SheetItem) -> Result<()> {
    match item {
        SheetItem::Text {
            at, text, height, ..
        } => {
            check_point(*at)?;
            if text.trim().is_empty()
                || text.len() > MAX_NOTE
                || text.chars().any(|c| c.is_control() && c != '\n')
            {
                return Err(invalid(
                    "Notes are 1–4096 bytes; only line breaks may be control characters.",
                ));
            }
            if !height.is_finite() || !(NOTE_HEIGHT_MM.0..=NOTE_HEIGHT_MM.1).contains(height) {
                return Err(invalid("Note text is 0.5–50 mm high."));
            }
        }
        SheetItem::Line { from, to } => {
            check_point(*from)?;
            check_point(*to)?;
            if from == to {
                return Err(invalid("A line needs two different points."));
            }
        }
        SheetItem::Rect { min, max } => {
            check_point(*min)?;
            check_point(*max)?;
            if min[0] == max[0] || min[1] == max[1] {
                return Err(invalid("A rectangle needs a width and a height."));
            }
        }
    }
    Ok(())
}

pub fn add_drawing(project: &mut Project, sheet: SheetId, item: SheetItem) -> Result<Id> {
    check_sheet(project, sheet)?;
    check_item(&item)?;
    if project.drawings.len() >= MAX_DRAWINGS {
        return Err(Error::Limit(
            "This edition supports at most 16384 notes and drawings.".into(),
        ));
    }
    let id = project.allocate()?;
    project.drawings.push(SheetDrawing { id, sheet, item });
    Ok(id)
}

/// Replace a drawing's item (move, resize or retext it).
pub fn edit_drawing(project: &mut Project, id: Id, item: SheetItem) -> Result<()> {
    check_item(&item)?;
    project
        .drawings
        .iter_mut()
        .find(|d| d.id == id)
        .ok_or_else(|| not_found("Unknown drawing."))?
        .item = item;
    Ok(())
}

pub fn remove_drawing(project: &mut Project, id: Id) -> Result<()> {
    if project.drawing(id).is_none() {
        return Err(not_found("Unknown drawing."));
    }
    project.drawings.retain(|d| d.id != id);
    Ok(())
}

/// Longest title-block field, in bytes.
pub const MAX_PROPERTY: usize = 256;

pub fn set_properties(project: &mut Project, properties: ProjectProperties) -> Result<()> {
    for field in [
        &properties.title,
        &properties.revision,
        &properties.date,
        &properties.author,
        &properties.company,
    ] {
        if field.len() > MAX_PROPERTY || field.chars().any(char::is_control) {
            return Err(invalid(
                "Title-block fields are at most 256 bytes without control characters.",
            ));
        }
    }
    project.properties = properties;
    Ok(())
}

/// Name the project: trimmed, not empty, at most [`MAX_PROPERTY`] bytes (the
/// bound a saved project is validated against), without control characters.
pub fn rename_project(project: &mut Project, name: &str) -> Result<()> {
    let name = name.trim();
    if name.is_empty() || name.len() > MAX_PROPERTY || name.chars().any(char::is_control) {
        return Err(invalid(
            "project.name: a project name is 1 to 256 bytes without control characters.",
        ));
    }
    name.clone_into(&mut project.name);
    Ok(())
}

/// Move a unit to another sheet. Refused while wires touch its terminals,
/// because a wire cannot cross sheets: connect across sheets with labels.
pub fn move_unit_to_sheet(
    project: &mut Project,
    occurrence: Id,
    sheet: SheetId,
    position: Point,
) -> Result<()> {
    check_sheet(project, sheet)?;
    check_point(position)?;
    let item = project
        .occurrence(occurrence)
        .cloned()
        .ok_or_else(|| not_found("Unknown symbol occurrence."))?;
    if !super::unit_wires(project, &item)?.is_empty() {
        return Err(conflict(
            "This unit is wired. Disconnect it first, then join the sheets with labels.",
        ));
    }
    let o = project
        .occurrences
        .iter_mut()
        .find(|o| o.id == occurrence)
        .ok_or_else(|| not_found("Unknown symbol occurrence."))?;
    o.sheet = sheet;
    o.position = position;
    Ok(())
}

/// Sheets, sheet symbols, pins, labels and drawings: limits, references,
/// unique names and an acyclic hierarchy in which each sheet has at most
/// one parent symbol.
pub(super) fn validate(project: &Project) -> Result<()> {
    let sheets = &project.sheets;
    if sheets.list.is_empty() || sheets.list.len() > MAX_SHEETS {
        return Err(invalid("A schematic has 1–256 sheets."));
    }
    let mut ids = BTreeSet::new();
    let mut names = BTreeSet::new();
    for sheet in &sheets.list {
        if sheet.id.0 == 0 || sheet.id.0 >= sheets.next || !ids.insert(sheet.id) {
            return Err(invalid("Sheet identities must be allocated and unique."));
        }
        if !printable(&sheet.name)
            || sheet.name.len() > MAX_SHEET_NAME
            || !names.insert(sheet.name.trim().to_lowercase())
        {
            return Err(invalid("Sheet names must be valid and unique."));
        }
    }
    let known = |s: SheetId| ids.contains(&s);
    if !project.occurrences.iter().all(|o| known(o.sheet))
        || !project.junctions.iter().all(|j| known(j.sheet))
        || !project.labels.iter().all(|l| known(l.sheet))
        || !project.drawings.iter().all(|d| known(d.sheet))
    {
        return Err(invalid("An object is on a missing sheet."));
    }
    if project.labels.len() > super::labels::MAX_LABELS
        || project.drawings.len() > MAX_DRAWINGS
        || project.sheet_symbols.len() > MAX_SHEET_SYMBOLS
    {
        return Err(invalid("Schematic exceeds supported item limits."));
    }
    for label in &project.labels {
        if !crate::nets::name_valid(&label.name) || !point_valid(label.position) {
            return Err(invalid("A label has an invalid name or position."));
        }
    }
    for drawing in &project.drawings {
        check_item(&drawing.item)?;
    }
    let mut children = BTreeSet::new();
    for symbol in &project.sheet_symbols {
        if !known(symbol.sheet) || !known(symbol.child) {
            return Err(invalid("A sheet symbol references a missing sheet."));
        }
        if !children.insert(symbol.child) {
            return Err(invalid("A sheet is used by more than one sheet symbol."));
        }
        if !point_valid(symbol.position) || symbol.pins.len() > MAX_SHEET_PINS {
            return Err(invalid("A sheet symbol is out of range."));
        }
        check_size(symbol.size)?;
        check_pins_fit(symbol)?;
        let mut pin_names = BTreeSet::new();
        for pin in &symbol.pins {
            if !crate::nets::name_valid(&pin.name)
                || !pin_names.insert(pin.name.to_ascii_lowercase())
            {
                return Err(invalid(
                    "Sheet pin names must be valid and unique per symbol.",
                ));
            }
        }
    }
    for symbol in &project.sheet_symbols {
        if symbol.child == symbol.sheet || ancestors(project, symbol.sheet).contains(&symbol.child)
        {
            return Err(invalid("The sheet hierarchy has a cycle."));
        }
    }
    Ok(())
}
