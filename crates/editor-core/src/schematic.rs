//! Single-sheet capture: symbol occurrences and wires. Wire endpoints
//! reference electrical identities; drawn crossings and coincident pins
//! never imply a connection. [`placed_unit`] resolves what an occurrence
//! draws.
use crate::diagnostics::{Error, Result};
use crate::geometry::{Point, Shape, Transform2, point_valid};
use crate::ids::{Endpoint, GraphicId, Id, TerminalId, UnitId};
use crate::instances::{self, MAX_OCCURRENCES};
use crate::library::{Fill, Item, Justify, TextContent};
use crate::model::{EditPolicy, Net, Project, SymbolOccurrence, Wire};
use crate::nets;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const MAX_WIRES: usize = 16384;

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
    },
    /// Presentation only; electrical identities are unchanged.
    #[serde(rename = "symbol.move_unit")]
    MoveUnit { occurrence: Id, position: Point },
    /// Remove a unit from the sheet. Direct only when no wire touches its
    /// terminals; otherwise preview it as `Change::Schematic`.
    #[serde(rename = "symbol.unplace_unit")]
    UnplaceUnit { occurrence: Id },
    #[serde(rename = "schematic.connect")]
    Connect { from: Endpoint, to: Endpoint },
    #[serde(rename = "schematic.disconnect")]
    Disconnect { wire: Id },
    /// Relabel a net. Its identity, membership and routed copper are unchanged.
    #[serde(rename = "schematic.net_rename")]
    RenameNet { net: Id, name: String },
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
/// reconciles nets and validates the document. Returns the occurrence a
/// `PlaceUnit` created.
pub fn apply(
    project: &mut Project,
    command: SchematicCommand,
    policy: EditPolicy,
) -> Result<Option<Id>> {
    match command {
        SchematicCommand::PlaceUnit {
            instance,
            unit,
            position,
        } => place_unit(project, instance, unit, position).map(Some),
        SchematicCommand::MoveUnit {
            occurrence,
            position,
        } => move_unit(project, occurrence, position).map(|()| None),
        SchematicCommand::UnplaceUnit { occurrence } => {
            unplace_unit(project, occurrence, policy).map(|()| None)
        }
        SchematicCommand::Connect { from, to } => connect(project, from, to).map(|()| None),
        SchematicCommand::Disconnect { wire } => disconnect(project, wire).map(|()| None),
        SchematicCommand::RenameNet { net, name } => rename_net(project, net, name).map(|()| None),
    }
}

pub fn place_unit(
    project: &mut Project,
    instance: Id,
    unit: UnitId,
    position: Point,
) -> Result<Id> {
    place_units(project, instance, &[(unit, position)])?
        .first()
        .copied()
        .ok_or_else(|| invalid("No unit was placed."))
}

/// Draw several units of one instance, then check the instance once, so units
/// whose wired terminals need each other can be placed together.
pub fn place_units(
    project: &mut Project,
    instance: Id,
    units: &[(UnitId, Point)],
) -> Result<Vec<Id>> {
    if project.instance(instance).is_none() {
        return Err(not_found("Unknown component instance."));
    }
    let mut placed = Vec::with_capacity(units.len());
    for (unit, position) in units {
        placed.push(instances::add_occurrence(
            project, instance, *unit, *position,
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

/// Wires on terminals the occurrence's unit draws.
fn unit_wires(project: &Project, occurrence: &SymbolOccurrence) -> Result<Vec<Id>> {
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
            [w.from, w.to].iter().any(|e| {
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
    let wired = project
        .wires
        .iter()
        .any(|w| w.from.instance == instance || w.to.instance == instance);
    if wired {
        if policy == EditPolicy::Direct {
            return Err(conflict(
                "This instance is wired. Preview its removal with project.preview_update (Change::Instance) to review the wires it removes.",
            ));
        }
        project
            .wires
            .retain(|w| w.from.instance != instance && w.to.instance != instance);
    }
    project.occurrences.retain(|o| o.instance != instance);
    Ok(())
}

pub fn connect(project: &mut Project, from: Endpoint, to: Endpoint) -> Result<()> {
    if from == to {
        return Err(invalid("A wire must connect two different terminals."));
    }
    if project
        .wires
        .iter()
        .any(|w| (w.from == from && w.to == to) || (w.from == to && w.to == from))
    {
        return Err(conflict("These terminals are already connected by a wire."));
    }
    if project.wires.len() >= MAX_WIRES {
        return Err(Error::Limit(
            "This edition supports at most 16384 wires.".into(),
        ));
    }
    let id = project.allocate()?;
    project.wires.push(Wire { id, from, to });
    // `check_instance` owns endpoint validity: instance, terminal, unit
    // placement and NC/no-connect exclusion.
    instances::check_instance(project, from.instance)?;
    instances::check_instance(project, to.instance)
}

pub fn disconnect(project: &mut Project, wire: Id) -> Result<()> {
    if !project.wires.iter().any(|w| w.id == wire) {
        return Err(not_found("Unknown schematic wire."));
    }
    project.wires.retain(|w| w.id != wire);
    Ok(())
}

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
    let routed: BTreeSet<Id> = before.tracks.iter().map(|t| t.net).collect();
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

/// Wire and occurrence records. Per-instance wiring rules belong to
/// `instances::check_instance`.
pub fn validate(project: &Project) -> Result<()> {
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
    let mut pairs = BTreeSet::new();
    for wire in &project.wires {
        if wire.from == wire.to {
            return Err(invalid("A wire must connect two different terminals."));
        }
        for endpoint in [wire.from, wire.to] {
            if project.instance(endpoint.instance).is_none() {
                return Err(invalid("A wire references a missing terminal."));
            }
        }
        let pair = if wire.from < wire.to {
            (wire.from, wire.to)
        } else {
            (wire.to, wire.from)
        };
        if !pairs.insert(pair) {
            return Err(invalid("Duplicate schematic connection."));
        }
    }
    Ok(())
}

/// The frame an occurrence draws its unit in. Translation only today; the
/// single seam for rotating and mirroring occurrences later.
pub fn occurrence_transform(occurrence: &SymbolOccurrence) -> Transform2 {
    Transform2::translation(occurrence.position)
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

/// Resolved text placed on the sheet.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct PlacedText {
    pub text: String,
    pub at: Point,
    pub height: f64,
    pub rotation_deg: f64,
    pub justify: Justify,
    /// Drawn reflected (an occurrence or board side mirror).
    pub mirrored: bool,
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
    pub name: Option<PlacedText>,
    /// The designations the instance's binding maps to the terminal; `None`
    /// without a physical choice, never guessed.
    pub number: Option<PlacedText>,
}

/// Everything one occurrence draws, resolved against its instance's exact
/// revisions. Views draw only from this.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct PlacedUnit {
    pub occurrence: Id,
    pub shapes: Vec<PlacedShape>,
    pub texts: Vec<PlacedText>,
    pub pins: Vec<PlacedPin>,
}

/// Resolve one occurrence's drawing: graphics in order, visible text with
/// designator, name, unit label and field values substituted, and pins with
/// their names and binding numbers.
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
    let fields = instances::effective_fields(&project.library, instance)?;
    let t = occurrence_transform(item);
    let place =
        |text: String, at: Point, height: f64, rotation_deg: f64, justify: Justify| PlacedText {
            text,
            at: t.apply(at),
            height,
            rotation_deg: t.apply_angle(rotation_deg).rem_euclid(360.0),
            justify,
            mirrored: t.mirror,
        };
    let mut shapes = Vec::new();
    let mut texts = Vec::new();
    for graphic in &unit.graphics {
        match &graphic.item {
            Item::Shape { shape, width, fill } => shapes.push(PlacedShape {
                graphic: graphic.id,
                shape: shape.transformed(&t),
                width: *width,
                fill: *fill,
            }),
            Item::Text(text) if text.visible => {
                let content = match &text.content {
                    TextContent::Literal(s) => s.clone(),
                    TextContent::Designator => instances::display_label(project, instance, unit.id),
                    TextContent::ComponentName => resolved.component.meta.name.clone(),
                    TextContent::UnitLabel => unit.label.clone(),
                    TextContent::Field(lineage) => fields
                        .iter()
                        .find(|f| f.field.id == *lineage)
                        .and_then(|f| f.text.clone())
                        .unwrap_or_default(),
                };
                texts.push(place(
                    content,
                    text.at,
                    text.height,
                    text.rotation.degrees(),
                    text.justify,
                ));
            }
            Item::Text(_) => {}
        }
    }
    let component = &resolved.component.definition;
    let numbers = |terminal: TerminalId| -> Option<String> {
        let physical = resolved.physical?;
        let designations: Vec<&str> = physical
            .package
            .definition
            .terminals
            .iter()
            .filter(|pt| {
                physical
                    .binding
                    .definition
                    .mappings
                    .iter()
                    .any(|m| m.terminal == terminal && m.package_terminal == pt.id)
            })
            .map(|pt| pt.designation.as_str())
            .collect();
        (!designations.is_empty()).then(|| designations.join(","))
    };
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
                    place(terminal.name.clone(), at, height, rotation, justify)
                });
            let number = numbers(pin.terminal)
                .filter(|_| pin.number.visible)
                .map(|text| {
                    let (at, rotation, justify) = pin.number_anchor();
                    place(text, at, height, rotation, justify)
                });
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
    Ok(PlacedUnit {
        occurrence,
        shapes,
        texts,
        pins,
    })
}

/// The project's nets, sorted by ID.
pub fn nets(project: &Project) -> &[Net] {
    &project.nets
}
