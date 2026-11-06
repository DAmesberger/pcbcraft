//! Schematic capture over one or more sheets: symbol occurrences, wires,
//! junctions, net labels and power ports ([`labels`]), sheets, hierarchical
//! sheet symbols and sheet furniture ([`sheets`]), and designator
//! annotation ([`annotate`]). A wire is a path with corners; its ends
//! reference terminals, junctions, labels or sheet pins, or are free. A
//! connection point that lies on a wire's path connects to it: every edit
//! is normalised so the stored topology says so ([`tidy`]). Crossings join
//! nothing without a junction point, `schematic.connect` routes around
//! points it does not end on, and labels also join by name (`nets`).
//! [`placed_unit`] resolves what an occurrence draws, [`definition_unit`]
//! what a unit being authored draws and [`scene`] everything else a sheet
//! draws. See docs/schematic-capture.md.
use crate::diagnostics::{Error, Result};
use crate::geometry::{Bounds, Point, Shape, Transform2, point_valid, segment_distance};
use crate::ids::{DefinitionId, Endpoint, GraphicId, Id, SheetId, TerminalId, UnitId};
use crate::instances::{self, MAX_OCCURRENCES};
pub use crate::library::graphics::PlacedText;
use crate::library::graphics::{TEXT_LINE_SPACING, VALUE_TEXT_HEIGHT_MM};
use crate::library::{
    BindingDefinition, ComponentDefinition, Fill, HAlign, Item, Justify, Library,
    PackageDefinition, Rotation, SymbolDefinition, SymbolHit, TextContent, TextMetric, Unit,
    VAlign,
};
use crate::model::{
    Corner, EditPolicy, LabelKind, Net, Paper, PaperOrientation, PortShape, Project,
    ProjectProperties, SheetItem, SheetSide, SymbolOccurrence, Wire, WireEnd,
};
use crate::nets;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const MAX_WIRES: usize = 16384;
/// Corners one wire's path may have.
pub const MAX_CORNERS: usize = 256;

pub mod annotate;
pub mod arrange;
pub mod clipboard;
pub mod labels;
pub mod scene;
pub mod sheets;
pub mod wire_edit;
mod wiring;
pub use annotate::{AnnotateMode, AnnotateOrder, Numbering, Renumbered};
pub use arrange::Arrangement;
pub use clipboard::{ClipObject, ClipSymbol, Pasted, PastedSymbol, SheetClip};
pub use wiring::{
    JUNCTION_TOLERANCE_MM, MAX_JUNCTIONS, WirePoint, WireTarget, end_position, end_sheet,
    junction_dots, nearest_on_wire, tidy, wire_ends, wire_net, wire_path,
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
    /// Move a unit; wires follow its pins, the segment on each pin staying
    /// horizontal or vertical. A pin moved onto a wire connects to it.
    #[serde(rename = "symbol.move_unit")]
    MoveUnit { occurrence: Id, position: Point },
    /// Move, turn or mirror several sheet objects together, as one undo step.
    /// A wire with both ends in the group keeps its shape and a wire with one
    /// rubber-bands. A symbol brings all its units on the sheet; a named
    /// wire brings its free ends and the junctions it ends on.
    #[serde(rename = "schematic.arrange")]
    Arrange {
        /// The first sheet when omitted.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sheet: Option<SheetId>,
        objects: Vec<ClipObject>,
        action: Arrangement,
    },
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
    /// Draw a wire between two targets: a terminal, a junction, a label, a
    /// sheet pin, or a point on an existing wire, which splits that wire at
    /// a new junction. The engine picks the path: the horizontal-first L,
    /// or a detour around points in its way, with its bends as corners.
    /// One undo step.
    #[serde(rename = "schematic.connect")]
    Connect { from: WireTarget, to: WireTarget },
    /// Draw a wire along a given path: from `from` through `corners` to
    /// `to`, each a target or a free end `{"free": [x, y]}`. No corners: one
    /// straight segment. Connection points it runs over join it, as do
    /// free ends it meets. One undo step.
    #[serde(rename = "wire.draw")]
    DrawWire {
        /// The sheet of its attached ends; the first sheet when both are
        /// free and it is omitted.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sheet: Option<SheetId>,
        from: WireTarget,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        corners: Vec<Corner>,
        to: WireTarget,
    },
    /// Replace a wire's geometry: its corners and, when given, either end
    /// (a target or a free end). Points it then runs over join it.
    #[serde(rename = "wire.set_path")]
    SetWirePath {
        wire: Id,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        corners: Vec<Corner>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        from: Option<WireTarget>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        to: Option<WireTarget>,
    },
    /// Move corner `index` (from 0, counted from the wire's `from` end) to
    /// `to`. Its two segments keep their directions: their far vertices come
    /// along, and an attached end gets a new corner so the segment steps.
    /// `free` moves the corner alone, so its segments may slant.
    #[serde(rename = "wire.move_corner")]
    MoveWireCorner {
        wire: Id,
        index: usize,
        to: Point,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        free: bool,
    },
    /// Move segment `index` (from vertex `index` to `index + 1`; 0 starts at
    /// the `from` end) along its normal by `offset` mm: +y for a horizontal
    /// segment, +x for a vertical one, and the side of +x for a slanted one.
    /// Its neighbours stretch; on an attached end a short perpendicular
    /// segment is added.
    #[serde(rename = "wire.move_segment")]
    MoveWireSegment { wire: Id, index: usize, offset: f64 },
    /// Add a corner at `at`, a point on the wire, without changing its shape.
    #[serde(rename = "wire.insert_corner")]
    InsertWireCorner { wire: Id, at: Point },
    /// Remove corner `index`, merging its two segments; the result may slant.
    #[serde(rename = "wire.remove_corner")]
    RemoveWireCorner { wire: Id, index: usize },
    /// Remove segment `index`: the wire splits in two, with free ends at the
    /// gap. A segment at the end of the wire shortens it instead.
    #[serde(rename = "wire.remove_segment")]
    RemoveWireSegment { wire: Id, index: usize },
    /// Move or re-attach the wire's `from` or `to` end, rubber-banding its
    /// path. `to` is a target or a free end; a free end on another free end
    /// joins the two wires into one.
    #[serde(rename = "wire.move_end")]
    MoveWireEnd {
        wire: Id,
        end: wire_edit::WireSide,
        to: WireTarget,
    },
    #[serde(rename = "schematic.disconnect")]
    Disconnect { wire: Id },
    /// Put a junction point on the wires at `position`: on a wire's path it
    /// joins everything meeting there, and where two wires cross it joins
    /// them. Refused off every wire and on an existing connection point.
    #[serde(rename = "junction.place")]
    PlaceJunction {
        /// The first sheet when omitted.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sheet: Option<SheetId>,
        position: Point,
    },
    /// Remove a junction point where straight wires pass through it: each
    /// pair of wires leaving it in opposite directions becomes one wire
    /// again, so a crossing stops joining. Refused at a bend or a T, where
    /// removing a wire is the way to change the drawing.
    #[serde(rename = "junction.remove")]
    RemoveJunction { junction: Id },
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
    /// Create what `clip` (from the `schematic.copy` query) holds on a
    /// sheet, shifted by `offset`, as one undo step. A symbol becomes a new
    /// instance of the same component with the next free designator and
    /// its field values; a label keeps its name, so it joins that net as
    /// any label would. Wires follow the pasted objects at the ends the
    /// clip keeps.
    #[serde(rename = "schematic.paste")]
    Paste {
        /// The sheet to paste on; the first sheet when omitted.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sheet: Option<SheetId>,
        clip: Box<SheetClip>,
        /// Added to every position, in mm.
        offset: Point,
    },
}

// A new command chooses its permission class here.
declare_classes!(SchematicCommand {
    PlaceUnit "symbol.place_unit" Edit,
    MoveUnit "symbol.move_unit" Edit,
    Arrange "schematic.arrange" Edit,
    OrientUnit "symbol.orient_unit" Edit,
    UnplaceUnit "symbol.unplace_unit" Edit,
    Connect "schematic.connect" Edit,
    DrawWire "wire.draw" Edit,
    SetWirePath "wire.set_path" Edit,
    MoveWireCorner "wire.move_corner" Edit,
    MoveWireSegment "wire.move_segment" Edit,
    InsertWireCorner "wire.insert_corner" Edit,
    RemoveWireCorner "wire.remove_corner" Edit,
    RemoveWireSegment "wire.remove_segment" Edit,
    MoveWireEnd "wire.move_end" Edit,
    Disconnect "schematic.disconnect" Edit,
    PlaceJunction "junction.place" Edit,
    RemoveJunction "junction.remove" Edit,
    RenameNet "schematic.net_rename" Edit,
    MoveUnitToSheet "symbol.move_to_sheet" Edit,
    PlaceLabel "label.place" Edit,
    MoveLabel "label.move" Edit,
    EditLabel "label.edit" Edit,
    RemoveLabel "label.remove" Edit,
    AddSheet "sheet.add" Edit,
    RenameSheet "sheet.rename" Edit,
    SetPage "sheet.set_page" Edit,
    ReorderSheet "sheet.reorder" Edit,
    DeleteSheet "sheet.delete" Edit,
    PlaceSheetSymbol "sheet_symbol.place" Edit,
    MoveSheetSymbol "sheet_symbol.move" Edit,
    ResizeSheetSymbol "sheet_symbol.resize" Edit,
    SetSheetChild "sheet_symbol.set_child" Edit,
    RemoveSheetSymbol "sheet_symbol.remove" Edit,
    SyncSheetPins "sheet_symbol.sync_pins" Edit,
    AddSheetPin "sheet_pin.add" Edit,
    EditSheetPin "sheet_pin.edit" Edit,
    RemoveSheetPin "sheet_pin.remove" Edit,
    AddDrawing "drawing.add" Edit,
    EditDrawing "drawing.edit" Edit,
    RemoveDrawing "drawing.remove" Edit,
    SetProperties "project.set_properties" Edit,
    RenameProject "project.rename" Edit,
    Annotate "schematic.annotate" Edit,
    Paste "schematic.paste" Edit,
});

/// What a schematic command created or changed, for the engine's outcome.
#[derive(Clone, Debug, PartialEq)]
pub enum Applied {
    None,
    Unit(Id),
    Label(Id),
    Junction(Id),
    Sheet(SheetId),
    SheetSymbol { symbol: Id, child: SheetId },
    SheetPins(Vec<Id>),
    Drawing(Id),
    Annotated(Vec<Renumbered>),
    Pasted(Pasted),
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
/// reconciles nets and validates the document. Wires on a part, label,
/// sheet symbol or sheet pin that moves, turns or mirrors follow it in the
/// same step ([`wire_edit`]).
pub fn apply(
    project: &mut Project,
    command: SchematicCommand,
    policy: EditPolicy,
) -> Result<Applied> {
    use SchematicCommand as C;
    let moves = matches!(
        command,
        C::MoveUnit { .. }
            | C::OrientUnit { .. }
            | C::MoveLabel { .. }
            | C::EditLabel { .. }
            | C::MoveSheetSymbol { .. }
            | C::ResizeSheetSymbol { .. }
            | C::EditSheetPin { .. }
    );
    let before = moves.then(|| wire_edit::ends(project));
    let applied = apply_command(project, command, policy)?;
    if let Some(before) = before {
        wire_edit::follow_moved(project, &before);
    }
    Ok(applied)
}

fn apply_command(
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
        C::Arrange {
            sheet,
            objects,
            action,
        } => none(arrange::arrange(
            project,
            sheet.unwrap_or(first),
            &objects,
            action,
        )),
        C::OrientUnit {
            occurrence,
            rotation,
            mirror,
        } => none(orient_unit(project, occurrence, rotation, mirror)),
        C::UnplaceUnit { occurrence } => none(unplace_unit(project, occurrence, policy)),
        C::Connect { from, to } => none(connect(project, from, to)),
        C::DrawWire {
            sheet,
            from,
            corners,
            to,
        } => draw_wire(project, sheet, from, corners, to).map(|_| Applied::None),
        C::SetWirePath {
            wire,
            corners,
            from,
            to,
        } => none(wire_edit::set_path(project, wire, corners, from, to)),
        C::MoveWireCorner {
            wire,
            index,
            to,
            free,
        } => none(wire_edit::apply_move_corner(project, wire, index, to, free)),
        C::MoveWireSegment {
            wire,
            index,
            offset,
        } => none(wire_edit::apply_move_segment(project, wire, index, offset)),
        C::InsertWireCorner { wire, at } => none(wire_edit::apply_insert_corner(project, wire, at)),
        C::RemoveWireCorner { wire, index } => {
            none(wire_edit::apply_remove_corner(project, wire, index))
        }
        C::RemoveWireSegment { wire, index } => {
            none(wire_edit::remove_segment(project, wire, index))
        }
        C::MoveWireEnd { wire, end, to } => none(wire_edit::apply_move_end(project, wire, end, to)),
        C::Disconnect { wire } => none(disconnect(project, wire)),
        C::PlaceJunction { sheet, position } => {
            wiring::place_junction(project, sheet.unwrap_or(first), position).map(Applied::Junction)
        }
        C::RemoveJunction { junction } => none(wiring::remove_junction(project, junction)),
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
        C::Paste {
            sheet,
            clip,
            offset,
        } => clipboard::paste(project, sheet.unwrap_or(first), *clip, offset).map(Applied::Pasted),
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
                "This unit is wired. Review unplacing it first: the review lists the wires it removes.",
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
                "These terminals are wired. Review the change first: the review lists the wires it removes.",
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
                "This instance is wired. Review its removal first: the review lists the wires it removes.",
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
    if matches!(from, WireTarget::Free(_)) || matches!(to, WireTarget::Free(_)) {
        return Err(invalid(
            "schematic.connect joins two connection points; draw a free end with wire.draw.",
        ));
    }
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
    let sheet = match (end_sheet(project, from), end_sheet(project, to)) {
        (Some(a), Some(b)) if a != b => {
            return Err(invalid(
                "A wire joins two points on one sheet; connect sheets with labels.",
            ));
        }
        (a, b) => a.or(b).unwrap_or_else(|| project.sheets.first()),
    };
    // The wire must not run over a connection point it does not end on,
    // which would join that point too: route around it, with each bend a
    // corner. A label's first wire to a pin attaches it: the label then
    // sits on the pin's end and the wire has no length.
    let attaches = |label: WireEnd, pin: WireEnd| {
        label
            .label_id()
            .and_then(|id| project.label(id))
            .is_some_and(|l| !l.manual)
            && pin.terminal().is_some()
            && project.wires_at(label).next().is_none()
    };
    let path = match (
        wiring::end_position(project, from),
        wiring::end_position(project, to),
    ) {
        _ if attaches(from, to) || attaches(to, from) => Vec::new(),
        (Some(a), Some(b)) => wiring::clean_route(project, sheet, a, b, [from, to])?,
        _ => Vec::new(),
    };
    let corners = path
        .get(1..path.len().saturating_sub(1))
        .unwrap_or_default()
        .iter()
        .map(|at| Corner { at: *at })
        .collect();
    add_wire(project, sheet, from, corners, to).map(|_| ())
}

/// Store a new wire, without corners that add nothing, then check the
/// instances on its terminal ends.
fn add_wire(
    project: &mut Project,
    sheet: SheetId,
    from: WireEnd,
    corners: Vec<Corner>,
    to: WireEnd,
) -> Result<Id> {
    if project.wires.len() >= MAX_WIRES {
        return Err(Error::Limit(
            "This edition supports at most 16384 wires.".into(),
        ));
    }
    let id = project.allocate()?;
    let mut wire = Wire {
        id,
        sheet,
        from,
        to,
        corners,
    };
    wiring::simplify(project, &mut wire);
    if wire.from == wire.to && wire.corners.is_empty() {
        return Err(invalid("A wire must connect two different points."));
    }
    project.wires.push(wire);
    // `check_instance` owns terminal validity: instance, terminal, unit
    // placement and NC/no-connect exclusion.
    for end in [from, to] {
        if let Some(endpoint) = end.terminal() {
            instances::check_instance(project, endpoint.instance)?;
        }
    }
    Ok(id)
}

/// Draw a wire along a given path (`wire.draw`). Its sheet is that of its
/// attached ends, which must agree with `sheet` when given; the first
/// sheet when every end is free and none is given. A target on a wire's
/// path splits that wire first; both targets on one wire are refused.
pub fn draw_wire(
    project: &mut Project,
    sheet: Option<SheetId>,
    from: WireTarget,
    corners: Vec<Corner>,
    to: WireTarget,
) -> Result<Id> {
    if corners.len() > MAX_CORNERS {
        return Err(Error::Limit("A wire has at most 256 corners.".into()));
    }
    if !corners.iter().all(|c| point_valid(c.at)) {
        return Err(invalid(
            "Schematic position must be finite and within ±10000 mm.",
        ));
    }
    if let (WireTarget::OnWire(a), WireTarget::OnWire(b)) = (&from, &to)
        && a.wire == b.wire
    {
        return Err(invalid("A wire cannot join two points of the same wire."));
    }
    let from = wiring::resolve(project, wiring::on_its_pin(project, from))?;
    let to = wiring::resolve(project, wiring::on_its_pin(project, to))?;
    let mut sheets = [from, to]
        .into_iter()
        .filter_map(|end| end_sheet(project, end))
        .chain(sheet);
    let first = sheets.next();
    if sheets.any(|s| Some(s) != first) {
        return Err(invalid(
            "A wire joins points on one sheet; connect sheets with labels.",
        ));
    }
    let sheet = first.unwrap_or_else(|| project.sheets.first());
    if project.sheet(sheet).is_none() {
        return Err(not_found("Unknown sheet."));
    }
    add_wire(project, sheet, from, corners, to)
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
                "This connectivity change affects routed copper. Review it first: the review lists the tracks and vias it removes.",
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

/// Radius of the open ring that marks a pin end still needing a decision
/// (wire it or mark it no-connect): its diameter is 0.8 of the pin text
/// height, so it reads as a mark on the pin rather than a symbol of its own.
pub const PIN_RING_MM: f64 = 0.4 * crate::library::graphics::PIN_TEXT_HEIGHT_MM;
/// Half the width of the no-connect cross on a decided pin end.
pub const NO_CONNECT_MM: f64 = 0.5 * crate::library::graphics::PIN_TEXT_HEIGHT_MM;

impl PlacedPin {
    /// The unit vector from the pin's body end out through its connection
    /// end; zero for a pin of no length.
    pub fn outward(&self) -> Point {
        let d = [
            self.end[0] - self.body_end[0],
            self.end[1] - self.body_end[1],
        ];
        let len = d[0].hypot(d[1]);
        if len > 0.0 && len.is_finite() {
            [d[0] / len, d[1] / len]
        } else {
            [0.0, 0.0]
        }
    }
    /// Where a ring of `radius` touching the connection end from outside
    /// is centred, so the pin line stops at the ring's edge instead of
    /// running into it.
    pub fn ring_centre(&self, radius: f64) -> Point {
        let o = self.outward();
        [self.end[0] + o[0] * radius, self.end[1] + o[1] * radius]
    }
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
    /// The instance's value from its shown fields
    /// ([`instances::value_text`]).
    value: Option<String>,
    /// The chosen Part's manufacturer part number.
    mpn: Option<String>,
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
    // The value slot (a `ComponentName` text) shows the instance's value
    // (`instances::value_text`, the value the BOM shows too), else the
    // Part's MPN, else the component name, which identifies an IC
    // ("AP2112K-3.3") or a rail symbol ("GND"). A two-terminal part without
    // a value shows none: its component name ("Chip capacitor 0603") is a
    // description, never a value.
    let value = r
        .value
        .clone()
        .or_else(|| r.mpn.clone().filter(|m| !m.trim().is_empty()))
        .or_else(|| (component.terminals.len() != 2).then(|| r.component_name.to_owned()));
    // Turned a quarter, the symbol keeps its designator and value upright
    // and beside the body, reading outward from it, instead of along the
    // pins where they would run over neighbouring pins and labels.
    let sideways = (t.rotation_deg.rem_euclid(180.0) - 90.0).abs() < 1e-9;
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
                let mut height = text.height;
                let content = match &text.content {
                    TextContent::Literal(s) => s.clone(),
                    TextContent::Designator => r.designator.clone(),
                    TextContent::ComponentName => {
                        let Some(value) = &value else {
                            continue;
                        };
                        // The shown fields are the value: not stacked again.
                        if r.value.is_some() {
                            drawn_fields.extend(
                                r.fields
                                    .iter()
                                    .filter(|f| f.shown && !f.text.trim().is_empty())
                                    .map(|f| f.lineage),
                            );
                        }
                        height = height.min(VALUE_TEXT_HEIGHT_MM);
                        value.clone()
                    }
                    TextContent::UnitLabel => unit.label.clone(),
                    TextContent::Field(lineage) => r
                        .fields
                        .iter()
                        .find(|f| f.lineage == *lineage)
                        .map(|f| f.text.clone())
                        .unwrap_or_default(),
                };
                let mut placed = PlacedText {
                    line_spacing: text.line_spacing,
                    ..place(
                        Some(graphic.id),
                        content,
                        text.at,
                        height,
                        text.rotation.degrees(),
                        text.justify,
                    )
                };
                if sideways
                    && matches!(
                        text.content,
                        TextContent::Designator
                            | TextContent::ComponentName
                            | TextContent::Field(_)
                    )
                {
                    let dx = placed.at[0] - t.translate[0];
                    placed.rotation_deg = 0.0;
                    placed.justify = Justify {
                        h: if dx < -1e-9 {
                            HAlign::Right
                        } else if dx > 1e-9 {
                            HAlign::Left
                        } else {
                            HAlign::Center
                        },
                        v: VAlign::Middle,
                    };
                }
                texts.push(placed);
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
                    let width =
                        text.chars().count() as f64 * TextMetric::default().advance * height;
                    let (at, rotation, justify) = pin.number_anchor(width);
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
            height: VALUE_TEXT_HEIGHT_MM,
            rotation_deg: 0.0,
            justify: Justify {
                h: HAlign::Center,
                v: VAlign::Top,
            },
            mirrored: false,
            line_spacing: TEXT_LINE_SPACING,
        });
        y -= VALUE_TEXT_HEIGHT_MM + FIELD_GAP_MM;
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
        value: instances::value_text(&project.library, instance)?,
        mpn: instance
            .part
            .and_then(|p| project.library.part(p))
            .map(|p| p.definition.mpn.clone()),
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
    let fields: Vec<ShownField> = instances::default_fields(library, d.component)?
        .into_iter()
        .map(|f| ShownField {
            lineage: f.field.id,
            text: f.text.unwrap_or(f.label),
            shown: f.show_on_symbol,
        })
        .collect();
    // No instance yet: the value slot shows the shown fields' placeholders.
    let shown: Vec<&str> = fields
        .iter()
        .filter(|f| f.shown && !f.text.trim().is_empty())
        .map(|f| f.text.as_str())
        .collect();
    let resolution = Resolution {
        designator,
        component_name: d.component_name,
        value: (!shown.is_empty()).then(|| shown.join(" ")),
        fields,
        mpn: None,
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
        *wires.entry(wire.sheet).or_default() += 1;
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
