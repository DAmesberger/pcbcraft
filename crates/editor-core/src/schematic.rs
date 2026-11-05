//! Schematic capture over one or more sheets: symbol occurrences, wires,
//! junctions, net labels and power ports ([`labels`]), sheets, hierarchical
//! sheet symbols and sheet furniture ([`sheets`]), and designator
//! annotation ([`annotate`]). Wire ends reference terminals, junctions,
//! labels or sheet pins; drawn crossings and coincident pins never imply a
//! connection, and labels join by name (`nets`). A wire end routed onto
//! another wire's segment splits it at a junction ([`wiring`]).
//! [`placed_unit`] resolves what an occurrence draws, [`definition_unit`]
//! what a unit being authored draws and [`scene`] everything else a sheet
//! draws. See docs/schematic-capture.md.
use crate::diagnostics::{Error, Result};
use crate::geometry::{Bounds, Point, Shape, Transform2, point_valid, segment_distance};
use crate::ids::{DefinitionId, Endpoint, GraphicId, Id, SheetId, TerminalId, UnitId};
use crate::instances::{self, MAX_OCCURRENCES};
pub use crate::library::graphics::PlacedText;
use crate::library::graphics::{SYMBOL_TEXT_HEIGHT_MM, TEXT_LINE_SPACING};
use crate::library::{
    BindingDefinition, ComponentDefinition, Fill, HAlign, Item, Justify, Library,
    PackageDefinition, Rotation, SymbolDefinition, SymbolHit, TextContent, TextMetric, Unit,
    VAlign,
};
use crate::model::{
    EditPolicy, LabelKind, Net, Paper, PaperOrientation, PortShape, Project, ProjectProperties,
    SheetItem, SheetSide, SymbolOccurrence, Wire, WireEnd,
};
use crate::nets;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const MAX_WIRES: usize = 16384;

pub mod annotate;
pub mod labels;
pub mod scene;
pub mod sheets;
mod wiring;
pub use annotate::{AnnotateMode, AnnotateOrder, Numbering, Renumbered};
pub use wiring::{
    JUNCTION_TOLERANCE_MM, MAX_JUNCTIONS, WirePoint, WireTarget, end_position, end_sheet,
    junction_dots, nearest_on_wire, tidy, wire_ends, wire_net, wire_route, wire_sheet,
};

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "command", content = "params", deny_unknown_fields)]
pub enum SchematicCommand {
    /// Draw an unplaced unit of an instance. Rejected if it is placed or
    /// marked unused.
    #[serde(rename = "symbol.place_unit")]
    PlaceUnit {
        instance: Id,
        unit: UnitId,
        position: Point,
        /// The sheet to draw it on: the sheet of the instance's other
        /// units, else the first sheet, when omitted.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sheet: Option<SheetId>,
    },
    /// Presentation only; electrical identities are unchanged.
    #[serde(rename = "symbol.move_unit")]
    MoveUnit { occurrence: Id, position: Point },
    /// Set an occurrence's orientation about its origin: `mirror` reflects
    /// the unit across its local x-axis, then `rotation` turns it
    /// counter-clockwise. Presentation only; wires follow their pins.
    #[serde(rename = "symbol.orient_unit")]
    OrientUnit {
        occurrence: Id,
        rotation: Rotation,
        mirror: bool,
    },
    /// Remove a unit from the sheet. Direct only when no wire touches its
    /// terminals; otherwise preview it as `Change::Schematic`.
    #[serde(rename = "symbol.unplace_unit")]
    UnplaceUnit { occurrence: Id },
    /// Draw a wire between two targets: a terminal, a junction, or a point
    /// on an existing wire, which splits that wire at a new junction. One
    /// undo step.
    #[serde(rename = "schematic.connect")]
    Connect { from: WireTarget, to: WireTarget },
    #[serde(rename = "schematic.disconnect")]
    Disconnect { wire: Id },
    /// Relabel a net. Its identity, membership and routed copper are
    /// unchanged. A net named by labels or sheet pins is renamed by renaming
    /// every one of them that spells its current name.
    #[serde(rename = "schematic.net_rename")]
    RenameNet { net: Id, name: String },
    /// Move a unit to another sheet. Refused while it is wired.
    #[serde(rename = "symbol.move_to_sheet")]
    MoveUnitToSheet {
        occurrence: Id,
        sheet: SheetId,
        position: Point,
    },
    /// Place a net label or power port. `attach` wires it on placement: a
    /// point on a wire splits the wire at the label; a terminal, junction,
    /// label or sheet pin gets a wire to it.
    #[serde(rename = "label.place")]
    PlaceLabel {
        /// The first sheet when omitted.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sheet: Option<SheetId>,
        name: String,
        kind: LabelKind,
        position: Point,
        #[serde(default)]
        rotation: Rotation,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        attach: Option<WireTarget>,
    },
    /// Presentation only; wires follow.
    #[serde(rename = "label.move")]
    MoveLabel { label: Id, position: Point },
    /// Rename, change the kind of, or turn a label. A name or kind change
    /// may join or split nets.
    #[serde(rename = "label.edit")]
    EditLabel {
        label: Id,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        kind: Option<LabelKind>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rotation: Option<Rotation>,
    },
    /// Remove a label; wires through it stay joined.
    #[serde(rename = "label.remove")]
    RemoveLabel { label: Id },
    /// Append a sheet ("Sheet n", A4 landscape unless given).
    #[serde(rename = "sheet.add")]
    AddSheet {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        paper: Option<Paper>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        orientation: Option<PaperOrientation>,
    },
    #[serde(rename = "sheet.rename")]
    RenameSheet { sheet: SheetId, name: String },
    #[serde(rename = "sheet.set_page")]
    SetPage {
        sheet: SheetId,
        paper: Paper,
        orientation: PaperOrientation,
    },
    /// Move a sheet to page `index` (from 0).
    #[serde(rename = "sheet.reorder")]
    ReorderSheet { sheet: SheetId, index: usize },
    /// Delete a sheet and what is drawn on it (its units return to
    /// unplaced) and the sheet symbol that uses it. The last sheet stays.
    #[serde(rename = "sheet.delete")]
    DeleteSheet { sheet: SheetId },
    /// Draw a sheet symbol for an existing sheet, or for a new one.
    #[serde(rename = "sheet_symbol.place")]
    PlaceSheetSymbol {
        /// The sheet to draw it on; the first sheet when omitted.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sheet: Option<SheetId>,
        /// An existing sheet; a new sheet when omitted.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        child: Option<SheetId>,
        /// The new sheet's name.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        /// Top-left corner.
        position: Point,
        size: Point,
    },
    #[serde(rename = "sheet_symbol.move")]
    MoveSheetSymbol { symbol: Id, position: Point },
    #[serde(rename = "sheet_symbol.resize")]
    ResizeSheetSymbol { symbol: Id, size: Point },
    #[serde(rename = "sheet_symbol.set_child")]
    SetSheetChild { symbol: Id, child: SheetId },
    /// Remove a sheet symbol; its sheet stays as a top-level sheet.
    #[serde(rename = "sheet_symbol.remove")]
    RemoveSheetSymbol { symbol: Id },
    /// Add a pin for every hierarchical label name of the child sheet the
    /// symbol lacks.
    #[serde(rename = "sheet_symbol.sync_pins")]
    SyncSheetPins { symbol: Id },
    #[serde(rename = "sheet_pin.add")]
    AddSheetPin {
        symbol: Id,
        name: String,
        #[serde(default)]
        shape: PortShape,
        side: SheetSide,
        offset: f64,
    },
    #[serde(rename = "sheet_pin.edit")]
    EditSheetPin {
        pin: Id,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        shape: Option<PortShape>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        side: Option<SheetSide>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        offset: Option<f64>,
    },
    #[serde(rename = "sheet_pin.remove")]
    RemoveSheetPin { pin: Id },
    /// Add a note, line or rectangle.
    #[serde(rename = "drawing.add")]
    AddDrawing {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sheet: Option<SheetId>,
        item: SheetItem,
    },
    /// Replace a drawing's item.
    #[serde(rename = "drawing.edit")]
    EditDrawing { drawing: Id, item: SheetItem },
    #[serde(rename = "drawing.remove")]
    RemoveDrawing { drawing: Id },
    /// Set the title-block properties.
    #[serde(rename = "project.set_properties")]
    SetProperties { properties: ProjectProperties },
    /// Name the project: the name fabrication outputs, drawings and the
    /// title block's Project cell show. The same name changes nothing and
    /// adds no undo step.
    #[serde(rename = "project.rename")]
    RenameProject { name: String },
    /// Number designators across sheets; existing annotated designators
    /// stay unless `mode` is `all`.
    #[serde(rename = "schematic.annotate")]
    Annotate {
        #[serde(default)]
        order: AnnotateOrder,
        #[serde(default)]
        mode: AnnotateMode,
        #[serde(default)]
        numbering: Numbering,
        /// Visit only instances whose first unit is on these sheets; all
        /// when empty.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        sheets: Vec<SheetId>,
    },
}

/// What a schematic command created or changed, for the engine's outcome.
#[derive(Clone, Debug, PartialEq)]
pub enum Applied {
    None,
    Unit(Id),
    Label(Id),
    Sheet(SheetId),
    SheetSymbol { symbol: Id, child: SheetId },
    SheetPins(Vec<Id>),
    Drawing(Id),
    Annotated(Vec<Renumbered>),
}

fn invalid(message: &str) -> Error {
    Error::Invalid(message.into())
}
fn not_found(message: &str) -> Error {
    Error::NotFound(message.into())
}
fn conflict(message: &str) -> Error {
    Error::Conflict(message.into())
}

/// Apply to a candidate the caller discards on failure. The engine then
/// reconciles nets and validates the document.
pub fn apply(
    project: &mut Project,
    command: SchematicCommand,
    policy: EditPolicy,
) -> Result<Applied> {
    use SchematicCommand as C;
    let none = |r: Result<()>| r.map(|()| Applied::None);
    let first = project.sheets.first();
    match command {
        C::PlaceUnit {
            instance,
            unit,
            position,
            sheet,
        } => place_units(project, instance, &[(unit, position)], sheet)?
            .first()
            .copied()
            .map(Applied::Unit)
            .ok_or_else(|| invalid("No unit was placed.")),
        C::MoveUnit {
            occurrence,
            position,
        } => none(move_unit(project, occurrence, position)),
        C::OrientUnit {
            occurrence,
            rotation,
            mirror,
        } => none(orient_unit(project, occurrence, rotation, mirror)),
        C::UnplaceUnit { occurrence } => none(unplace_unit(project, occurrence, policy)),
        C::Connect { from, to } => none(connect(project, from, to)),
        C::Disconnect { wire } => none(disconnect(project, wire)),
        C::RenameNet { net, name } => none(rename_net(project, net, name)),
        C::MoveUnitToSheet {
            occurrence,
            sheet,
            position,
        } => none(sheets::move_unit_to_sheet(
            project, occurrence, sheet, position,
        )),
        C::PlaceLabel {
            sheet,
            name,
            kind,
            position,
            rotation,
            attach,
        } => labels::place(
            project,
            labels::NewLabel {
                sheet: sheet.unwrap_or(first),
                name,
                kind,
                position,
                rotation,
                attach,
            },
        )
        .map(Applied::Label),
        C::MoveLabel { label, position } => none(labels::move_to(project, label, position)),
        C::EditLabel {
            label,
            name,
            kind,
            rotation,
        } => none(labels::edit(project, label, name, kind, rotation)),
        C::RemoveLabel { label } => none(labels::remove(project, label)),
        C::AddSheet {
            name,
            paper,
            orientation,
        } => sheets::add(project, name, paper, orientation).map(Applied::Sheet),
        C::RenameSheet { sheet, name } => none(sheets::rename(project, sheet, name)),
        C::SetPage {
            sheet,
            paper,
            orientation,
        } => none(sheets::set_page(project, sheet, paper, orientation)),
        C::ReorderSheet { sheet, index } => none(sheets::reorder(project, sheet, index)),
        C::DeleteSheet { sheet } => none(sheets::delete(project, sheet)),
        C::PlaceSheetSymbol {
            sheet,
            child,
            name,
            position,
            size,
        } => sheets::place_symbol(
            project,
            sheets::NewSheetSymbol {
                sheet: sheet.unwrap_or(first),
                child,
                name,
                position,
                size,
            },
        )
        .map(|(symbol, child)| Applied::SheetSymbol { symbol, child }),
        C::MoveSheetSymbol { symbol, position } => {
            none(sheets::move_symbol(project, symbol, position))
        }
        C::ResizeSheetSymbol { symbol, size } => none(sheets::resize_symbol(project, symbol, size)),
        C::SetSheetChild { symbol, child } => none(sheets::set_child(project, symbol, child)),
        C::RemoveSheetSymbol { symbol } => none(sheets::remove_symbol(project, symbol)),
        C::SyncSheetPins { symbol } => sheets::sync_pins(project, symbol).map(Applied::SheetPins),
        C::AddSheetPin {
            symbol,
            name,
            shape,
            side,
            offset,
        } => sheets::add_pin(
            project,
            sheets::NewSheetPin {
                symbol,
                name,
                shape,
                side,
                offset,
            },
        )
        .map(|pin| Applied::SheetPins(vec![pin])),
        C::EditSheetPin {
            pin,
            name,
            shape,
            side,
            offset,
        } => none(sheets::edit_pin(project, pin, name, shape, side, offset)),
        C::RemoveSheetPin { pin } => none(sheets::remove_pin(project, pin)),
        C::AddDrawing { sheet, item } => {
            sheets::add_drawing(project, sheet.unwrap_or(first), item).map(Applied::Drawing)
        }
        C::EditDrawing { drawing, item } => none(sheets::edit_drawing(project, drawing, item)),
        C::RemoveDrawing { drawing } => none(sheets::remove_drawing(project, drawing)),
        C::SetProperties { properties } => none(sheets::set_properties(project, properties)),
        C::RenameProject { name } => none(sheets::rename_project(project, &name)),
        C::Annotate {
            order,
            mode,
            numbering,
            sheets,
        } => annotate::annotate(project, order, mode, numbering, &sheets).map(Applied::Annotated),
    }
}

pub fn place_unit(
    project: &mut Project,
    instance: Id,
    unit: UnitId,
    position: Point,
) -> Result<Id> {
    place_units(project, instance, &[(unit, position)], None)?
        .first()
        .copied()
        .ok_or_else(|| invalid("No unit was placed."))
}

/// Draw several units of one instance on `sheet` (by default the sheet of
/// its other units, else the first sheet), then check the instance once, so
/// units whose wired terminals need each other can be placed together.
pub fn place_units(
    project: &mut Project,
    instance: Id,
    units: &[(UnitId, Point)],
    sheet: Option<SheetId>,
) -> Result<Vec<Id>> {
    if project.instance(instance).is_none() {
        return Err(not_found("Unknown component instance."));
    }
    let sheet = sheet
        .or_else(|| project.occurrences_of(instance).next().map(|o| o.sheet))
        .unwrap_or_else(|| project.sheets.first());
    let mut placed = Vec::with_capacity(units.len());
    for (unit, position) in units {
        placed.push(instances::add_occurrence(
            project, instance, *unit, *position, sheet,
        )?);
    }
    instances::check_instance(project, instance)?;
    Ok(placed)
}

pub fn move_unit(project: &mut Project, occurrence: Id, position: Point) -> Result<()> {
    if !point_valid(position) {
        return Err(invalid(
            "Schematic position must be finite and within ±10000 mm.",
        ));
    }
    project
        .occurrences
        .iter_mut()
        .find(|o| o.id == occurrence)
        .ok_or_else(|| not_found("Unknown symbol occurrence."))?
        .position = position;
    Ok(())
}

/// Presentation only: wires reference terminals, so their ends follow the
/// turned or mirrored pins.
pub fn orient_unit(
    project: &mut Project,
    occurrence: Id,
    rotation: Rotation,
    mirror: bool,
) -> Result<()> {
    let item = project
        .occurrences
        .iter_mut()
        .find(|o| o.id == occurrence)
        .ok_or_else(|| not_found("Unknown symbol occurrence."))?;
    item.rotation = rotation;
    item.mirror = mirror;
    Ok(())
}

/// The orientation after turning an occurrence a further 90°
/// counter-clockwise on the sheet about its origin.
pub fn rotated_on_sheet(rotation: Rotation, mirror: bool) -> (Rotation, bool) {
    (rotation.turned(1), mirror)
}

/// The orientation after flipping an occurrence left to right on the sheet
/// about the vertical line through its origin: a reflection across the
/// local x-axis followed by a half turn, composed after the current
/// orientation.
pub fn mirrored_on_sheet(rotation: Rotation, mirror: bool) -> (Rotation, bool) {
    (
        Rotation::from_quarter_turns(2 - i64::from(rotation.quarter_turns())),
        !mirror,
    )
}

/// Wires on terminals the occurrence's unit draws.
pub(crate) fn unit_wires(project: &Project, occurrence: &SymbolOccurrence) -> Result<Vec<Id>> {
    let instance = project
        .instance(occurrence.instance)
        .ok_or_else(|| not_found("Unknown component instance."))?;
    let symbol = &instances::resolve(&project.library, instance)?
        .symbol
        .definition;
    Ok(project
        .wires
        .iter()
        .filter(|w| {
            [w.from, w.to].iter().filter_map(|e| e.terminal()).any(|e| {
                e.instance == occurrence.instance
                    && instances::owning_unit(symbol, e.terminal) == Some(occurrence.unit)
            })
        })
        .map(|w| w.id)
        .collect())
}

/// Direct only when no wire touches the unit's terminals; a consequential
/// unplace removes those wires.
pub fn unplace_unit(project: &mut Project, occurrence: Id, policy: EditPolicy) -> Result<()> {
    let item = project
        .occurrence(occurrence)
        .cloned()
        .ok_or_else(|| not_found("Unknown symbol occurrence."))?;
    let wires = unit_wires(project, &item)?;
    if !wires.is_empty() {
        if policy == EditPolicy::Direct {
            return Err(conflict(
                "This unit is wired. Preview unplacing it with project.preview_update (Change::Schematic) to review the wires it removes.",
            ));
        }
        project.wires.retain(|w| !wires.contains(&w.id));
    }
    project.occurrences.retain(|o| o.id != occurrence);
    Ok(())
}

/// Remove the wires on some terminals of one instance. Direct only when none
/// is wired.
pub fn disconnect_terminals(
    project: &mut Project,
    instance: Id,
    terminals: &BTreeSet<TerminalId>,
    policy: EditPolicy,
) -> Result<()> {
    let touches = |w: &Wire| {
        [w.from, w.to]
            .iter()
            .filter_map(|e| e.terminal())
            .any(|e| e.instance == instance && terminals.contains(&e.terminal))
    };
    if project.wires.iter().any(touches) {
        if policy == EditPolicy::Direct {
            return Err(conflict(
                "These terminals are wired. Preview the change with project.preview_update to review the wires it removes.",
            ));
        }
        project.wires.retain(|w| !touches(w));
    }
    Ok(())
}

/// Remove every occurrence of an instance. Direct only without wires.
pub fn detach_instance(project: &mut Project, instance: Id, policy: EditPolicy) -> Result<()> {
    let on_instance = |w: &Wire| {
        [w.from, w.to]
            .iter()
            .filter_map(|e| e.terminal())
            .any(|e| e.instance == instance)
    };
    let wired = project.wires.iter().any(on_instance);
    if wired {
        if policy == EditPolicy::Direct {
            return Err(conflict(
                "This instance is wired. Preview its removal with project.preview_update (Change::Instance) to review the wires it removes.",
            ));
        }
        project.wires.retain(|w| !on_instance(w));
    }
    project.occurrences.retain(|o| o.instance != instance);
    Ok(())
}

/// Draw a wire between two targets. A target on a wire's segment splits
/// that wire at a new junction first; both targets on one wire are refused.
/// The caller discards the candidate on failure, so a refused connect leaves
/// no split behind.
pub fn connect(project: &mut Project, from: WireTarget, to: WireTarget) -> Result<()> {
    if let (WireTarget::OnWire(a), WireTarget::OnWire(b)) = (&from, &to)
        && a.wire == b.wire
    {
        return Err(invalid("A wire cannot join two points of the same wire."));
    }
    let from = wiring::resolve(project, from)?;
    let to = wiring::resolve(project, to)?;
    if from == to {
        return Err(invalid("A wire must connect two different points."));
    }
    if project
        .wires
        .iter()
        .any(|w| (w.from == from && w.to == to) || (w.from == to && w.to == from))
    {
        return Err(conflict("These points are already connected by a wire."));
    }
    if project.wires.len() >= MAX_WIRES {
        return Err(Error::Limit(
            "This edition supports at most 16384 wires.".into(),
        ));
    }
    if let (Some(a), Some(b)) = (end_sheet(project, from), end_sheet(project, to))
        && a != b
    {
        return Err(invalid(
            "A wire joins two points on one sheet; connect sheets with labels.",
        ));
    }
    let id = project.allocate()?;
    project.wires.push(Wire { id, from, to });
    // `check_instance` owns terminal validity: instance, terminal, unit
    // placement and NC/no-connect exclusion.
    for end in [from, to] {
        if let Some(endpoint) = end.terminal() {
            instances::check_instance(project, endpoint.instance)?;
        }
    }
    Ok(())
}

pub fn disconnect(project: &mut Project, wire: Id) -> Result<()> {
    if !project.wires.iter().any(|w| w.id == wire) {
        return Err(not_found("Unknown schematic wire."));
    }
    project.wires.retain(|w| w.id != wire);
    Ok(())
}

/// Rename a net. Refused when another net uses the name. A net named by
/// labels or sheet pins renames every one that spells its current name,
/// so the new name survives reconciliation (and hierarchical labels follow
/// their sheet pins); an unlabelled net stores the name.
pub fn rename_net(project: &mut Project, net: Id, name: String) -> Result<()> {
    if !nets::name_valid(&name) {
        return Err(invalid("Net names must be 1–64 bytes without spaces."));
    }
    if project
        .nets
        .iter()
        .any(|n| n.id != net && n.name.eq_ignore_ascii_case(&name))
    {
        return Err(conflict("Another net already uses this name."));
    }
    if labels::rename_sources(project, net, &name)? {
        return Ok(());
    }
    project
        .nets
        .iter_mut()
        .find(|n| n.id == net)
        .ok_or_else(|| not_found("Unknown net."))?
        .name = name;
    Ok(())
}

/// Direct edits may not change the membership of a routed net; the update
/// planner previews and applies such changes with explicit track removal.
pub fn protect_routed_nets(before: &Project, after: &Project) -> Result<()> {
    let routed: BTreeSet<Id> = before
        .tracks
        .iter()
        .map(|t| t.net)
        .chain(before.vias.iter().map(|v| v.net))
        .collect();
    for id in routed {
        let a = before.net(id).map(|net| &net.terminals);
        let b = after.net(id).map(|net| &net.terminals);
        if a != b {
            return Err(conflict(
                "This connectivity change affects routed copper. Preview it with project.preview_update and apply the reviewed track removals through project.apply_update.",
            ));
        }
    }
    Ok(())
}

/// Wire, occurrence, label and sheet records. Per-instance wiring rules
/// belong to `instances::check_instance`.
pub fn validate(project: &Project) -> Result<()> {
    sheets::validate(project)?;
    if project.wires.len() > MAX_WIRES || project.occurrences.len() > MAX_OCCURRENCES {
        return Err(invalid("Schematic exceeds supported item limits."));
    }
    for occurrence in &project.occurrences {
        if project.instance(occurrence.instance).is_none() {
            return Err(invalid(
                "A symbol occurrence references a missing instance.",
            ));
        }
    }
    wiring::validate(project)
}

/// The frame an occurrence draws its unit in: reflect across the local
/// x-axis if mirrored, turn by its rotation, then move to its position.
/// Every placed coordinate (drawing, pins, wire ends, picking and fit) goes
/// through it.
pub fn occurrence_transform(occurrence: &SymbolOccurrence) -> Transform2 {
    Transform2 {
        translate: occurrence.position,
        rotation_deg: occurrence.rotation.degrees(),
        mirror: occurrence.mirror,
    }
}

/// Global schematic pin position in millimetres: the pin's connection end in
/// the frame of the occurrence of its owning unit. `None` while that unit is
/// not placed.
pub fn pin_position(project: &Project, endpoint: Endpoint) -> Option<Point> {
    let instance = project.instance(endpoint.instance)?;
    let symbol = &instances::resolve(&project.library, instance)
        .ok()?
        .symbol
        .definition;
    let (unit, pin) = symbol.pin(endpoint.terminal)?;
    let occurrence = project
        .occurrences_of(instance.id)
        .find(|o| o.unit == unit.id)?;
    Some(occurrence_transform(occurrence).apply(pin.position))
}

/// One drawn shape of a placed unit.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct PlacedShape {
    pub graphic: GraphicId,
    pub shape: Shape,
    pub width: f64,
    pub fill: Fill,
}

/// One pin of a placed unit, in sheet coordinates.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct PlacedPin {
    pub terminal: TerminalId,
    /// The connection end wires attach to.
    pub end: Point,
    /// Where the pin meets the body.
    pub body_end: Point,
    /// Where the drawn stub stops (short of an inversion bubble).
    pub line_end: Point,
    pub decorations: Vec<Shape>,
    /// The terminal's name; `None` when hidden, or when it reads exactly
    /// as the shown number, which is then drawn alone.
    pub name: Option<PlacedText>,
    /// The designations the instance's binding maps to the terminal; `None`
    /// without a physical choice, never guessed.
    pub number: Option<PlacedText>,
}

/// Everything one unit draws, resolved against exact revisions. Views draw
/// only from this.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct PlacedUnit {
    /// The occurrence drawn; `None` for a definition drawn in its own frame
    /// by the Component editor ([`definition_unit`]).
    pub occurrence: Option<Id>,
    pub shapes: Vec<PlacedShape>,
    pub texts: Vec<PlacedText>,
    pub pins: Vec<PlacedPin>,
    /// Values of fields the component shows on the symbol that no `Field`
    /// text of this unit draws, centred and stacked below the unit so a
    /// shown value is never lost. Upright and not separately selectable.
    pub fields: Vec<PlacedText>,
}

impl PlacedUnit {
    /// The extent of everything drawn: stroke-inflated shapes, pin stubs and
    /// decorations, and text under the approximate core metric. `None` when
    /// nothing is drawn.
    pub fn bounds(&self, m: TextMetric) -> Option<Bounds> {
        let shapes = self.shapes.iter().map(|s| s.shape.bounds(s.width));
        let pins = self.pins.iter().flat_map(|p| {
            let stub = Bounds::point(p.end).including(p.body_end);
            let decorations = p.decorations.iter().map(|d| d.bounds(0.0));
            let texts = p.name.iter().chain(&p.number).map(|t| t.bounds(m));
            std::iter::once(stub).chain(decorations).chain(texts)
        });
        let texts = self.texts.iter().chain(&self.fields).map(|t| t.bounds(m));
        shapes.chain(pins).chain(texts).reduce(Bounds::union)
    }

    /// The topmost drawn object within `tol_mm` of `p`, in the same
    /// coordinates and with the same resolved text as [`Self::bounds`], so
    /// what is drawn is what is picked: pins (stub, decorations, name and
    /// number) first, then text graphics and stacked field values (drawn
    /// over shapes), then shapes, later ones before earlier ones.
    pub fn hit(&self, p: Point, tol_mm: f64, m: TextMetric) -> Option<SymbolHit> {
        let near_text =
            |t: &PlacedText| !t.text.is_empty() && t.bounds(m).inflated(tol_mm).contains(p);
        let pin = self
            .pins
            .iter()
            .filter_map(|pin| {
                let stub = segment_distance(p, pin.end, pin.body_end);
                let decoration = pin
                    .decorations
                    .iter()
                    .filter(|d| d.coarse_bounds(0.0).inflated(tol_mm).contains(p))
                    .map(|d| d.distance(p, false))
                    .fold(f64::INFINITY, f64::min);
                let text = pin.name.iter().chain(&pin.number).any(near_text);
                let d = if text { 0.0 } else { stub.min(decoration) };
                (d <= tol_mm).then_some((d, pin.terminal))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0));
        if let Some((_, terminal)) = pin {
            return Some(SymbolHit::Pin(terminal));
        }
        if self.fields.iter().any(near_text) {
            return Some(SymbolHit::Field);
        }
        if let Some(graphic) = self
            .texts
            .iter()
            .rev()
            .find(|t| near_text(t))
            .and_then(|t| t.graphic)
        {
            return Some(SymbolHit::Graphic(graphic));
        }
        self.shapes
            .iter()
            .rev()
            .find(|s| {
                // A cheap box first: most shapes are far from the pointer,
                // and measuring rounded ones builds their fillets.
                s.shape.coarse_bounds(s.width).inflated(tol_mm).contains(p)
                    && s.shape.distance(p, s.fill != Fill::None) <= tol_mm + s.width / 2.0
            })
            .map(|s| SymbolHit::Graphic(s.graphic))
    }
}

/// Gap between a unit and the shown field values stacked below it, mm.
const FIELD_GAP_MM: f64 = 0.5;

/// A field the drawn unit may show: its lineage, resolved text and whether
/// the component shows it on the symbol.
struct ShownField {
    lineage: DefinitionId,
    text: String,
    shown: bool,
}

/// How a unit's text placeholders resolve.
struct Resolution<'a> {
    designator: String,
    component_name: &'a str,
    fields: Vec<ShownField>,
    /// Binding and package supplying pin numbers; none hides them.
    physical: Option<(&'a BindingDefinition, &'a PackageDefinition)>,
}

/// Designations of the package terminals `binding` maps `terminal` to,
/// joined (stacked terminals read "2,7"); `None` when it maps none.
fn pin_numbers(
    binding: &BindingDefinition,
    package: &PackageDefinition,
    terminal: TerminalId,
) -> Option<String> {
    let designations: Vec<&str> = package
        .terminals
        .iter()
        .filter(|pt| {
            binding
                .mappings
                .iter()
                .any(|m| m.terminal == terminal && m.package_terminal == pt.id)
        })
        .map(|pt| pt.designation.as_str())
        .collect();
    (!designations.is_empty()).then(|| designations.join(","))
}

fn flip(v: VAlign) -> VAlign {
    match v {
        VAlign::Top => VAlign::Bottom,
        VAlign::Middle => VAlign::Middle,
        VAlign::Bottom => VAlign::Top,
    }
}

/// Draw `unit` through `t` with its placeholders resolved by `r`.
fn draw(
    symbol: &SymbolDefinition,
    unit: &Unit,
    component: &ComponentDefinition,
    t: Transform2,
    r: &Resolution<'_>,
    occurrence: Option<Id>,
) -> PlacedUnit {
    let place = |graphic: Option<GraphicId>,
                 text: String,
                 at: Point,
                 height: f64,
                 rotation_deg: f64,
                 justify: Justify| PlacedText {
        graphic,
        text,
        at: t.apply(at),
        height,
        rotation_deg: t.apply_angle(rotation_deg).rem_euclid(360.0),
        // A reflection across the baseline puts the box on the other side
        // of it while the glyphs stay readable.
        justify: if t.mirror {
            Justify {
                h: justify.h,
                v: flip(justify.v),
            }
        } else {
            justify
        },
        mirrored: false,
        line_spacing: TEXT_LINE_SPACING,
    };
    let mut shapes = Vec::new();
    let mut texts = Vec::new();
    let mut drawn_fields = BTreeSet::new();
    for graphic in &unit.graphics {
        match &graphic.item {
            Item::Shape { shape, width, fill } => shapes.push(PlacedShape {
                graphic: graphic.id,
                shape: shape.transformed(&t),
                width: *width,
                fill: *fill,
            }),
            Item::Text(text) => {
                if let TextContent::Field(lineage) = text.content {
                    drawn_fields.insert(lineage);
                }
                if !text.visible {
                    continue;
                }
                let content = match &text.content {
                    TextContent::Literal(s) => s.clone(),
                    TextContent::Designator => r.designator.clone(),
                    TextContent::ComponentName => r.component_name.to_owned(),
                    TextContent::UnitLabel => unit.label.clone(),
                    TextContent::Field(lineage) => r
                        .fields
                        .iter()
                        .find(|f| f.lineage == *lineage)
                        .map(|f| f.text.clone())
                        .unwrap_or_default(),
                };
                texts.push(PlacedText {
                    line_spacing: text.line_spacing,
                    ..place(
                        Some(graphic.id),
                        content,
                        text.at,
                        text.height,
                        text.rotation.degrees(),
                        text.justify,
                    )
                });
            }
        }
    }
    let height = symbol.pin_text_height;
    let pins = unit
        .pins
        .iter()
        .map(|pin| {
            let [_, line_end] = pin.line();
            let name = component
                .terminal(pin.terminal)
                .filter(|_| pin.name.visible)
                .map(|terminal| {
                    let (at, rotation, justify) = pin.name_anchor();
                    place(None, terminal.name.clone(), at, height, rotation, justify)
                });
            let number = r
                .physical
                .and_then(|(binding, package)| pin_numbers(binding, package, pin.terminal))
                .filter(|_| pin.number.visible)
                .map(|text| {
                    let (at, rotation, justify) = pin.number_anchor();
                    place(None, text, at, height, rotation, justify)
                });
            // A name that only repeats the number ("1" on pin 1) says
            // nothing twice: the number alone is drawn.
            let name = name.filter(|n| number.as_ref().is_none_or(|m| m.text != n.text));
            PlacedPin {
                terminal: pin.terminal,
                end: t.apply(pin.position),
                body_end: t.apply(pin.body_end()),
                line_end: t.apply(line_end),
                decorations: pin
                    .decoration_shapes()
                    .iter()
                    .map(|s| s.transformed(&t))
                    .collect(),
                name,
                number,
            }
        })
        .collect();
    let mut placed = PlacedUnit {
        occurrence,
        shapes,
        texts,
        pins,
        fields: Vec::new(),
    };
    let m = TextMetric::default();
    let below = placed
        .bounds(m)
        .unwrap_or_else(|| Bounds::point(t.translate));
    let mut y = below.min[1] - FIELD_GAP_MM;
    for field in &r.fields {
        if !field.shown || field.text.is_empty() || drawn_fields.contains(&field.lineage) {
            continue;
        }
        placed.fields.push(PlacedText {
            graphic: None,
            text: field.text.clone(),
            at: [below.center()[0], y],
            height: SYMBOL_TEXT_HEIGHT_MM,
            rotation_deg: 0.0,
            justify: Justify {
                h: HAlign::Center,
                v: VAlign::Top,
            },
            mirrored: false,
            line_spacing: TEXT_LINE_SPACING,
        });
        y -= SYMBOL_TEXT_HEIGHT_MM + FIELD_GAP_MM;
    }
    placed
}

/// Resolve one occurrence's drawing: graphics in order, visible text with
/// designator, name, unit label and field values substituted, pins with
/// their names and binding numbers, and shown field values no text draws.
pub fn placed_unit(project: &Project, occurrence: Id) -> Result<PlacedUnit> {
    let item = project
        .occurrence(occurrence)
        .ok_or_else(|| not_found("Unknown symbol occurrence."))?;
    let instance = project
        .instance(item.instance)
        .ok_or_else(|| not_found("Unknown component instance."))?;
    let resolved = instances::resolve(&project.library, instance)?;
    let symbol = &resolved.symbol.definition;
    let unit = symbol
        .unit(item.unit)
        .ok_or_else(|| invalid("The occurrence's unit is not in its symbol."))?;
    let fields = instances::effective_fields(&project.library, instance)?
        .into_iter()
        .map(|f| ShownField {
            lineage: f.field.id,
            text: f.text.unwrap_or_default(),
            shown: f.show_on_symbol,
        })
        .collect();
    let resolution = Resolution {
        designator: instances::display_label(project, instance, unit.id),
        component_name: &resolved.component.meta.name,
        fields,
        physical: resolved
            .physical
            .map(|p| (&p.binding.definition, &p.package.definition)),
    };
    Ok(draw(
        symbol,
        unit,
        &resolved.component.definition,
        occurrence_transform(item),
        &resolution,
        Some(occurrence),
    ))
}

/// A unit of a definition being authored, as the Component editor shows it.
#[derive(Clone, Copy)]
pub struct DefinitionUnit<'a> {
    pub symbol: &'a SymbolDefinition,
    pub unit: UnitId,
    pub component: &'a ComponentDefinition,
    pub component_name: &'a str,
    /// The working binding and its package, for pin numbers; none hides
    /// them.
    pub physical: Option<(&'a BindingDefinition, &'a PackageDefinition)>,
}

/// Draw a definition's unit in its own frame, as [`placed_unit`] draws an
/// occurrence. The designator reads `prefix + "?"` (plus the unit label for
/// several units); a field shows the component or field default, or its
/// name when it has none, so its place is visible while authoring.
pub fn definition_unit(library: &Library, d: DefinitionUnit<'_>) -> Result<PlacedUnit> {
    let unit = d
        .symbol
        .unit(d.unit)
        .ok_or_else(|| not_found("Unknown symbol unit."))?;
    let mut designator = format!("{}?", d.component.designator_prefix);
    if d.symbol.units.len() > 1 {
        designator.push_str(&unit.label);
    }
    // A field shows its default, or its name when it has none.
    let fields = instances::default_fields(library, d.component)?
        .into_iter()
        .map(|f| ShownField {
            lineage: f.field.id,
            text: f.text.unwrap_or(f.label),
            shown: f.show_on_symbol,
        })
        .collect();
    let resolution = Resolution {
        designator,
        component_name: d.component_name,
        fields,
        physical: d.physical,
    };
    Ok(draw(
        d.symbol,
        unit,
        d.component,
        Transform2::IDENTITY,
        &resolution,
        None,
    ))
}

/// The project's nets, sorted by ID.
pub fn nets(project: &Project) -> &[Net] {
    &project.nets
}

/// One sheet in the hierarchy (`schematic.sheets`).
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct SheetRow {
    pub id: SheetId,
    pub name: String,
    /// Page number, from 1.
    pub page: usize,
    pub paper: Paper,
    pub orientation: PaperOrientation,
    /// The sheet symbol that uses this sheet and the sheet it is drawn on.
    pub parent: Option<SheetParent>,
    /// Depth below its top-level sheet (0 at the top).
    pub depth: usize,
    pub symbols: usize,
    pub labels: usize,
    pub wires: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct SheetParent {
    pub symbol: Id,
    pub sheet: SheetId,
}

/// Every sheet in page order with its place in the hierarchy.
pub fn sheet_rows(project: &Project) -> Vec<SheetRow> {
    let depths = nets::sheet_depths(project);
    let mut wires: std::collections::BTreeMap<SheetId, usize> = Default::default();
    for wire in &project.wires {
        if let Some(sheet) = wire_sheet(project, wire) {
            *wires.entry(sheet).or_default() += 1;
        }
    }
    project
        .sheets
        .list
        .iter()
        .enumerate()
        .map(|(i, sheet)| SheetRow {
            id: sheet.id,
            name: sheet.name.clone(),
            page: i + 1,
            paper: sheet.paper,
            orientation: sheet.orientation,
            parent: project.parent_symbol(sheet.id).map(|s| SheetParent {
                symbol: s.id,
                sheet: s.sheet,
            }),
            depth: depths.get(&sheet.id).copied().unwrap_or(0),
            symbols: project
                .occurrences
                .iter()
                .filter(|o| o.sheet == sheet.id)
                .count(),
            labels: project
                .labels
                .iter()
                .filter(|l| l.sheet == sheet.id)
                .count(),
            wires: wires.get(&sheet.id).copied().unwrap_or(0),
        })
        .collect()
}

/// What sits at one place of a net on some sheet.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NetPlaceKind {
    Label {
        label: Id,
        name: String,
        label_kind: LabelKind,
    },
    SheetPin {
        pin: Id,
        symbol: Id,
        name: String,
    },
    Terminal {
        endpoint: Endpoint,
        /// "R1.2"-style: designator and terminal name.
        name: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct NetPlace {
    pub sheet: SheetId,
    pub position: Point,
    #[serde(flatten)]
    pub kind: NetPlaceKind,
}

/// Where a net appears across sheets, in page order then position: its
/// labels, sheet pins and placed terminals (`schematic.net_places`), so a
/// view can jump between a label's other occurrences.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct NetPlaces {
    pub net: Id,
    pub name: String,
    pub places: Vec<NetPlace>,
}

pub fn net_places(project: &Project, net: Id) -> Result<NetPlaces> {
    let item = project.net(net).ok_or_else(|| not_found("Unknown net."))?;
    let mut places = Vec::new();
    if let Some(sources) = nets::name_sources(project).get(&net) {
        for source in sources {
            let Some(position) = end_position(project, source.end) else {
                continue;
            };
            let kind = match source.end {
                WireEnd::Label(l) => match project.label(l.label) {
                    Some(label) => NetPlaceKind::Label {
                        label: label.id,
                        name: label.name.clone(),
                        label_kind: label.kind,
                    },
                    None => continue,
                },
                WireEnd::SheetPin(p) => match project.sheet_pin(p.sheet_pin) {
                    Some((symbol, pin)) => NetPlaceKind::SheetPin {
                        pin: pin.id,
                        symbol: symbol.id,
                        name: pin.name.clone(),
                    },
                    None => continue,
                },
                _ => continue,
            };
            places.push(NetPlace {
                sheet: source.sheet,
                position,
                kind,
            });
        }
    }
    for endpoint in &item.terminals {
        let (Some(position), Some(sheet)) = (
            pin_position(project, *endpoint),
            end_sheet(project, WireEnd::Terminal(*endpoint)),
        ) else {
            continue;
        };
        let name = project
            .instance(endpoint.instance)
            .map_or_else(String::new, |i| {
                let terminal = project
                    .library
                    .component(i.component)
                    .and_then(|c| c.definition.terminal(endpoint.terminal))
                    .map_or_else(|| endpoint.terminal.to_string(), |t| t.name.clone());
                format!("{}.{terminal}", i.designator)
            });
        places.push(NetPlace {
            sheet,
            position,
            kind: NetPlaceKind::Terminal {
                endpoint: *endpoint,
                name,
            },
        });
    }
    let page = |s: SheetId| project.sheets.number(s).unwrap_or(usize::MAX);
    places.sort_by(|a, b| {
        (page(a.sheet), -a.position[1], a.position[0])
            .partial_cmp(&(page(b.sheet), -b.position[1], b.position[0]))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    Ok(NetPlaces {
        net,
        name: item.name.clone(),
        places,
    })
}
