//! Copy and paste on a sheet. `schematic.copy` reads sheet objects into a
//! self-contained [`SheetClip`], so the objects may be gone (cut) by the
//! time `schematic.paste` recreates them as one undo step. Nets stay
//! separate: wire ends in the clip refer to clip objects or are free, and a
//! label joins by its name as any label does.
use super::{Result, invalid, labels, not_found, sheets, wiring};
use crate::diagnostics::Error;
use crate::geometry::Point;
use crate::ids::{Endpoint, Id, SheetId};
use crate::instances::{self, NewInstance};
use crate::model::{
    ComponentInstance, Corner, Label, LabelRef, Project, SheetItem, SymbolOccurrence, Wire, WireEnd,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Objects one `schematic.copy` may name.
pub const MAX_CLIP_OBJECTS: usize = 4096;

/// A sheet object to copy.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ClipObject {
    /// A symbol: the instance with its units on the sheet.
    Instance {
        id: Id,
    },
    Label {
        id: Id,
    },
    Drawing {
        id: Id,
    },
    Wire {
        id: Id,
    },
}

/// One copied symbol.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ClipSymbol {
    pub instance: ComponentInstance,
    /// Its units on the copied sheet; at least one.
    pub occurrences: Vec<SymbolOccurrence>,
}

/// What `schematic.paste` recreates. A wire's terminal and label ends name
/// objects of this clip; every other end is free.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SheetClip {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub symbols: Vec<ClipSymbol>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<Label>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub drawings: Vec<SheetItem>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub wires: Vec<Wire>,
}

impl SheetClip {
    /// The point of its first object, the one a paste at a point puts there.
    pub fn anchor(&self) -> Option<Point> {
        let symbol = || {
            self.symbols
                .first()
                .and_then(|s| s.occurrences.first())
                .map(|o| o.position)
        };
        let label = || self.labels.first().map(|l| l.position);
        let drawing = || {
            self.drawings.first().map(|d| match d {
                SheetItem::Text { at, .. } => *at,
                SheetItem::Line { from, .. } => *from,
                SheetItem::Rect { min, .. } => *min,
            })
        };
        let wire = || {
            let wire = self.wires.first()?;
            wire.from
                .free_point()
                .or_else(|| wire.corners.first().map(|c| c.at))
                .or_else(|| wire.to.free_point())
        };
        symbol().or_else(label).or_else(drawing).or_else(wire)
    }
}

/// A symbol `schematic.paste` created.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PastedSymbol {
    pub instance: Id,
    /// Its first occurrence.
    pub occurrence: Id,
    pub designator: String,
}

/// What `schematic.paste` created. A wire listed here may be gone once the
/// engine's tidy joined it to another.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Pasted {
    pub symbols: Vec<PastedSymbol>,
    pub labels: Vec<Id>,
    pub drawings: Vec<Id>,
    pub wires: Vec<Id>,
}

/// Read `objects` of `sheet` into a clip. A wire comes along when it is
/// named, or when both its ends are on named symbols or labels; an end on
/// anything else becomes a free end where it is.
pub fn copy(project: &Project, sheet: SheetId, objects: &[ClipObject]) -> Result<SheetClip> {
    if objects.is_empty() {
        return Err(invalid("Name at least one object to copy."));
    }
    if objects.len() > MAX_CLIP_OBJECTS {
        return Err(Error::Limit("Copy at most 4096 objects at a time.".into()));
    }
    labels::check_sheet(project, sheet)?;
    let mut clip = SheetClip::default();
    let mut symbols = BTreeSet::new();
    let mut names = BTreeSet::new();
    let mut drawings = BTreeSet::new();
    let mut named_wires = BTreeSet::new();
    for object in objects {
        match *object {
            ClipObject::Instance { id } => {
                let instance = project
                    .instance(id)
                    .ok_or_else(|| not_found("Unknown component instance."))?;
                if !symbols.insert(id) {
                    continue;
                }
                let occurrences: Vec<SymbolOccurrence> = project
                    .occurrences_of(id)
                    .filter(|o| o.sheet == sheet)
                    .cloned()
                    .collect();
                if occurrences.is_empty() {
                    return Err(invalid("Only a symbol on this sheet can be copied."));
                }
                clip.symbols.push(ClipSymbol {
                    instance: instance.clone(),
                    occurrences,
                });
            }
            ClipObject::Label { id } => {
                let label = project
                    .label(id)
                    .filter(|l| l.sheet == sheet)
                    .ok_or_else(|| not_found("No such label on this sheet."))?;
                if names.insert(id) {
                    let mut copied = label.clone();
                    // Where it is drawn, which a label placed by its pin
                    // does not store.
                    copied.position = labels::anchor(project, label);
                    copied.rotation = labels::current_rotation(project, id)?;
                    clip.labels.push(copied);
                }
            }
            ClipObject::Drawing { id } => {
                let drawing = project
                    .drawing(id)
                    .filter(|d| d.sheet == sheet)
                    .ok_or_else(|| not_found("No such drawing on this sheet."))?;
                if drawings.insert(id) {
                    clip.drawings.push(drawing.item.clone());
                }
            }
            ClipObject::Wire { id } => {
                project
                    .wire(id)
                    .filter(|w| w.sheet == sheet)
                    .ok_or_else(|| not_found("No such wire on this sheet."))?;
                named_wires.insert(id);
            }
        }
    }
    let kept = |end: WireEnd| match end {
        WireEnd::Terminal(endpoint) => symbols.contains(&endpoint.instance),
        WireEnd::Label(LabelRef { label }) => names.contains(&label),
        WireEnd::Junction(_) | WireEnd::SheetPin(_) | WireEnd::Free(_) => false,
    };
    let clipped = |end: WireEnd| {
        if kept(end) {
            return Ok(end);
        }
        wiring::end_position(project, end)
            .and_then(WireEnd::free)
            .ok_or_else(|| invalid("A wire end has no position on this sheet."))
    };
    for wire in project.wires.iter().filter(|w| w.sheet == sheet) {
        if named_wires.contains(&wire.id) || (kept(wire.from) && kept(wire.to)) {
            clip.wires.push(Wire {
                from: clipped(wire.from)?,
                to: clipped(wire.to)?,
                ..wire.clone()
            });
        }
    }
    // A label placed by its pin keeps doing so only with that pin's wire;
    // alone it stays where it is pasted.
    let wires = &clip.wires;
    for label in &mut clip.labels {
        let end = WireEnd::label(label.id);
        label.manual = label.manual || !wires.iter().any(|w| w.from == end || w.to == end);
    }
    Ok(clip)
}

/// Create `clip` on `sheet`, shifted by `offset`. The caller discards the
/// candidate on failure.
pub fn paste(
    project: &mut Project,
    sheet: SheetId,
    clip: SheetClip,
    offset: Point,
) -> Result<Pasted> {
    if clip == SheetClip::default() {
        return Err(invalid("There is nothing to paste."));
    }
    labels::check_sheet(project, sheet)?;
    labels::check_point(offset)?;
    let shift = |p: Point| [p[0] + offset[0], p[1] + offset[1]];
    let mut pasted = Pasted::default();
    let mut new_instances = BTreeMap::new();
    let mut new_labels = BTreeMap::new();
    for symbol in clip.symbols {
        let source = symbol.instance.id;
        let (id, occurrence) = paste_symbol(project, sheet, symbol, offset)?;
        new_instances.insert(source, id);
        pasted.symbols.push(PastedSymbol {
            instance: id,
            occurrence,
            designator: project
                .instance(id)
                .map(|i| i.designator.clone())
                .unwrap_or_default(),
        });
    }
    for label in clip.labels {
        let id = labels::place(
            project,
            labels::NewLabel {
                sheet,
                name: label.name,
                kind: label.kind,
                position: shift(label.position),
                rotation: label.rotation,
                attach: None,
            },
        )?;
        if !label.manual
            && let Some(placed) = project.labels.iter_mut().find(|l| l.id == id)
        {
            placed.manual = false;
        }
        new_labels.insert(label.id, id);
        pasted.labels.push(id);
    }
    for item in clip.drawings {
        pasted
            .drawings
            .push(sheets::add_drawing(project, sheet, shifted(item, offset))?);
    }
    let end = |end: WireEnd| -> Result<WireEnd> {
        let unknown = || invalid("A pasted wire ends on an object the clip does not hold.");
        match end {
            WireEnd::Terminal(endpoint) => Ok(WireEnd::Terminal(Endpoint {
                instance: *new_instances.get(&endpoint.instance).ok_or_else(unknown)?,
                terminal: endpoint.terminal,
            })),
            WireEnd::Label(LabelRef { label }) => {
                Ok(WireEnd::label(*new_labels.get(&label).ok_or_else(unknown)?))
            }
            WireEnd::Free(free) => WireEnd::free(shift(free.point()))
                .ok_or_else(|| invalid("A pasted wire end is off the sheet's range.")),
            WireEnd::Junction(_) | WireEnd::SheetPin(_) => Err(unknown()),
        }
    };
    for wire in clip.wires {
        pasted.wires.push(super::draw_wire(
            project,
            Some(sheet),
            end(wire.from)?.into(),
            wire.corners
                .iter()
                .map(|c| Corner {
                    at: shift(c.at),
                    ..*c
                })
                .collect(),
            end(wire.to)?.into(),
        )?);
    }
    Ok(pasted)
}

/// A new instance of the symbol's component, with its next free designator,
/// everything else the instance holds, and its units in place.
fn paste_symbol(
    project: &mut Project,
    sheet: SheetId,
    symbol: ClipSymbol,
    offset: Point,
) -> Result<(Id, Id)> {
    let source = symbol.instance;
    let shift = |p: Point| [p[0] + offset[0], p[1] + offset[1]];
    let mut units = symbol.occurrences.into_iter();
    let first = units
        .next()
        .ok_or_else(|| invalid("A pasted symbol needs a placed unit."))?;
    labels::check_point(shift(first.position))?;
    let (id, occurrence) = instances::place(
        project,
        NewInstance {
            component: source.component,
            designator: None,
            properties: Vec::new(),
            representation: source.representation,
            unit: first.unit,
            position: shift(first.position),
            physical: source.physical,
            sheet: Some(sheet),
        },
    )?;
    orient(project, occurrence, &first);
    for unit in units {
        labels::check_point(shift(unit.position))?;
        let added = instances::add_occurrence(project, id, unit.unit, shift(unit.position), sheet)?;
        orient(project, added, &unit);
    }
    let instance = instances::instance_mut(project, id)?;
    instance.properties = source.properties;
    instance.unused_units = source.unused_units;
    instance.no_connect = source.no_connect;
    instance.functions = source.functions;
    instance.part = source.part;
    instance.pin_config = source.pin_config;
    instance.marking = source.marking;
    instance.text_placements = source.text_placements;
    instances::check_instance(project, id)?;
    Ok((id, occurrence))
}

fn orient(project: &mut Project, occurrence: Id, like: &SymbolOccurrence) {
    if let Some(o) = project.occurrences.iter_mut().find(|o| o.id == occurrence) {
        o.rotation = like.rotation;
        o.mirror = like.mirror;
    }
}

fn shifted(item: SheetItem, offset: Point) -> SheetItem {
    let shift = |p: Point| [p[0] + offset[0], p[1] + offset[1]];
    match item {
        SheetItem::Text {
            at,
            text,
            height,
            rotation,
        } => SheetItem::Text {
            at: shift(at),
            text,
            height,
            rotation,
        },
        SheetItem::Line { from, to } => SheetItem::Line {
            from: shift(from),
            to: shift(to),
        },
        SheetItem::Rect { min, max } => SheetItem::Rect {
            min: shift(min),
            max: shift(max),
        },
    }
}
