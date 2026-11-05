//! Net labels and power ports: placing, moving, editing and removing them.
//! A label is a connection point wires end on; its name joins it to other
//! labels by its kind's scope (`nets`). Renaming a net that labels name
//! renames those labels ([`super::rename_net`]).
use super::wiring::{self, WirePoint, WireTarget};
use super::{Result, connect, invalid, not_found};
use crate::diagnostics::Error;
use crate::geometry::{Point, point_valid};
use crate::ids::{Id, SheetId};
use crate::library::Rotation;
use crate::model::{Junction, Label, LabelKind, Project, WireEnd};
use crate::nets;

/// Labels and power ports a project holds.
pub const MAX_LABELS: usize = 16384;

pub(super) fn check_name(name: &str) -> Result<()> {
    if nets::name_valid(name) {
        Ok(())
    } else {
        Err(invalid("Label names must be 1–64 bytes without spaces."))
    }
}

pub(super) fn check_sheet(project: &Project, sheet: SheetId) -> Result<()> {
    project
        .sheet(sheet)
        .map(|_| ())
        .ok_or_else(|| not_found("Unknown sheet."))
}

pub(super) fn check_point(p: Point) -> Result<()> {
    if point_valid(p) {
        Ok(())
    } else {
        Err(invalid(
            "Schematic position must be finite and within ±10000 mm.",
        ))
    }
}

/// What `label.place` puts down.
pub struct NewLabel {
    pub sheet: SheetId,
    pub name: String,
    pub kind: LabelKind,
    pub position: Point,
    pub rotation: Rotation,
    pub attach: Option<WireTarget>,
}

/// Place a label. Attached to a point on a wire, the label sits on that
/// point and splits the wire there (at a wire's end it is wired to that
/// end); attached to a terminal, junction, label or sheet pin, a wire joins
/// them. Unattached, it joins only by name.
pub fn place(project: &mut Project, new: NewLabel) -> Result<Id> {
    check_name(&new.name)?;
    check_sheet(project, new.sheet)?;
    check_point(new.position)?;
    if project.labels.len() >= MAX_LABELS {
        return Err(Error::Limit(
            "This edition supports at most 16384 labels.".into(),
        ));
    }
    let id = project.allocate()?;
    let mut label = Label {
        id,
        sheet: new.sheet,
        name: new.name,
        kind: new.kind,
        position: new.position,
        rotation: new.rotation,
    };
    match new.attach {
        Some(WireTarget::OnWire(point)) => {
            let (at, end) = wiring::on_route(project, point)?;
            label.position = at;
            project.labels.push(label);
            match end {
                Some(end) => connect(project, WireTarget::Label(label_ref(id)), end.into())?,
                None => {
                    wiring::split(
                        project,
                        WirePoint {
                            wire: point.wire,
                            at,
                        },
                        Some(WireEnd::label(id)),
                    )?;
                }
            }
        }
        Some(target) => {
            project.labels.push(label);
            connect(project, WireTarget::Label(label_ref(id)), target)?;
        }
        None => project.labels.push(label),
    }
    Ok(id)
}

fn label_ref(id: Id) -> crate::model::LabelRef {
    crate::model::LabelRef { label: id }
}

fn label_mut(project: &mut Project, id: Id) -> Result<&mut Label> {
    project
        .labels
        .iter_mut()
        .find(|l| l.id == id)
        .ok_or_else(|| not_found("Unknown label."))
}

/// Presentation: wires on the label follow it. Joins by name are unchanged.
pub fn move_to(project: &mut Project, id: Id, position: Point) -> Result<()> {
    check_point(position)?;
    label_mut(project, id)?.position = position;
    Ok(())
}

/// Rename, change the kind of, or turn a label. A new name or kind may join
/// or split nets; routed nets are protected by the engine.
pub fn edit(
    project: &mut Project,
    id: Id,
    name: Option<String>,
    kind: Option<LabelKind>,
    rotation: Option<Rotation>,
) -> Result<()> {
    if let Some(name) = &name {
        check_name(name)?;
    }
    let label = label_mut(project, id)?;
    if let Some(name) = name {
        label.name = name;
    }
    if let Some(kind) = kind {
        label.kind = kind;
    }
    if let Some(rotation) = rotation {
        label.rotation = rotation;
    }
    Ok(())
}

/// Remove a label. Wires on it stay joined to each other at a junction left
/// in its place; a single wire left dangling is removed with it.
pub fn remove(project: &mut Project, id: Id) -> Result<()> {
    let label = project
        .label(id)
        .cloned()
        .ok_or_else(|| not_found("Unknown label."))?;
    detach_point(project, WireEnd::label(id), label.sheet, label.position)?;
    project.labels.retain(|l| l.id != id);
    Ok(())
}

/// Hand the wires on `end` to a new junction at `position`, so removing a
/// label or sheet pin keeps the wires through it joined. The engine's tidy
/// then drops a junction left with one wire, and that wire.
pub(super) fn detach_point(
    project: &mut Project,
    end: WireEnd,
    sheet: SheetId,
    position: Point,
) -> Result<()> {
    if !project.wires.iter().any(|w| w.from == end || w.to == end) {
        return Ok(());
    }
    if project.junctions.len() >= wiring::MAX_JUNCTIONS {
        return Err(Error::Limit(
            "This edition supports at most 16384 junctions.".into(),
        ));
    }
    let junction = project.allocate()?;
    project.junctions.push(Junction {
        id: junction,
        sheet,
        position,
        placed: false,
    });
    let to = WireEnd::junction(junction);
    for wire in &mut project.wires {
        if wire.from == end {
            wire.from = to;
        }
        if wire.to == end {
            wire.to = to;
        }
    }
    Ok(())
}

/// The labels and sheet pins that give `net` its current name; renaming
/// the net renames them. Refused when the net's name comes from no label.
pub(super) fn rename_sources(project: &mut Project, net: Id, name: &str) -> Result<bool> {
    if project.net(net).is_none() {
        return Err(not_found("Unknown net."));
    }
    let sources = nets::name_sources(project);
    let Some(sources) = sources.get(&net) else {
        return Ok(false);
    };
    let Some(first) = sources.first() else {
        return Ok(false);
    };
    // The winning name, unqualified: every source spelling it is renamed,
    // so the net keeps one name across its labels.
    let winning = first.name.to_ascii_lowercase();
    let mut renamed = false;
    for source in sources {
        if source.name.to_ascii_lowercase() != winning {
            continue;
        }
        match source.end {
            WireEnd::Label(l) => label_mut(project, l.label)?.name = name.to_owned(),
            WireEnd::SheetPin(p) => {
                if let Some(pin) = project
                    .sheet_symbols
                    .iter_mut()
                    .flat_map(|s| s.pins.iter_mut())
                    .find(|pin| pin.id == p.sheet_pin)
                {
                    pin.name = name.to_owned();
                }
            }
            _ => continue,
        }
        renamed = true;
    }
    Ok(renamed)
}
