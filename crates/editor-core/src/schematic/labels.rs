//! Net labels and power ports: placing, moving, editing and removing them.
//! A label is a connection point wires end on; its name joins it to other
//! labels by its kind's scope (`nets`). Renaming a net that labels name
//! renames those labels ([`super::rename_net`]).
//!
//! A label attached to a pin places itself: until a person moves or turns
//! it, a label whose only wire goes to a pin on its sheet sits on that pin's
//! end ([`anchor`]) and faces away from the symbol ([`outward`],
//! [`auto_rotation`]); [`super::scene::sheet_labels`] draws it clear of
//! symbols and other labels. Nothing of that placement is stored.
use super::wiring::{self, WirePoint, WireTarget};
use super::{PlacedUnit, Result, connect, invalid, not_found, placed_unit};
use crate::diagnostics::Error;
use crate::geometry::{Bounds, Point, point_valid};
use crate::ids::{Endpoint, Id, SheetId, TerminalId};
use crate::library::Rotation;
use crate::model::{Junction, Label, LabelKind, PowerStyle, Project, WireEnd};
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
        // Attached to a pin, it places itself; anywhere else it stays
        // where it was put.
        manual: !matches!(new.attach, Some(WireTarget::Terminal(_))),
    };
    match new.attach {
        Some(WireTarget::OnWire(point)) => {
            let (at, _, end) = wiring::on_path(project, point)?;
            label.position = at;
            project.labels.push(label);
            match end {
                // The engine's tidy attaches a free end the label sits on.
                Some(WireEnd::Free(_)) => {}
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
/// A label that placed itself keeps the way it faced and stays where it is
/// put from now on.
pub fn move_to(project: &mut Project, id: Id, position: Point) -> Result<()> {
    check_point(position)?;
    let rotation = current_rotation(project, id)?;
    let label = label_mut(project, id)?;
    label.position = position;
    label.rotation = rotation;
    label.manual = true;
    Ok(())
}

/// Rename, change the kind of, or turn a label. A new name or kind may join
/// or split nets; routed nets are protected by the engine. A label that
/// placed itself and is turned stays on its pin's end, turned as asked,
/// from now on.
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
    let at = match rotation {
        Some(_) => project.label(id).map(|l| anchor(project, l)),
        None => None,
    };
    let label = label_mut(project, id)?;
    if let Some(name) = name {
        label.name = name;
    }
    if let Some(kind) = kind {
        label.kind = kind;
    }
    if let Some(rotation) = rotation {
        label.rotation = rotation;
        label.manual = true;
    }
    if let Some(at) = at {
        label.position = at;
    }
    Ok(())
}

/// The pin a label is attached to while it places itself: it is not
/// manual, and its only wire goes to a terminal drawn on its sheet.
pub fn attached_pin(project: &Project, label: &Label) -> Option<Endpoint> {
    if label.manual {
        return None;
    }
    let me = WireEnd::label(label.id);
    let mut wires = project.wires_at(me);
    let wire = wires.next()?;
    if wires.next().is_some() {
        return None;
    }
    let other = if wire.from == me { wire.to } else { wire.from };
    match other {
        WireEnd::Terminal(endpoint) if wiring::end_sheet(project, other) == Some(label.sheet) => {
            Some(endpoint)
        }
        _ => None,
    }
}

/// A label's connection point, where its wires end: its pin's end while it
/// places itself, else its stored position.
pub fn anchor(project: &Project, label: &Label) -> Point {
    attached_pin(project, label)
        .and_then(|e| super::pin_position(project, e))
        .unwrap_or(label.position)
}

/// The axis pointing away from a placed pin's symbol: from where the pin
/// meets the body to its end, so the occurrence's rotation and mirror are
/// already applied; from the body's centre for a pin of no length.
pub fn outward(unit: &PlacedUnit, terminal: TerminalId) -> Option<Point> {
    let pin = unit.pins.iter().find(|p| p.terminal == terminal)?;
    let mut d = [pin.end[0] - pin.body_end[0], pin.end[1] - pin.body_end[1]];
    if d[0].abs() < 1e-9 && d[1].abs() < 1e-9 {
        let body = unit
            .shapes
            .iter()
            .map(|s| s.shape.bounds(s.width))
            .reduce(Bounds::union)?;
        let c = body.center();
        d = [pin.end[0] - c[0], pin.end[1] - c[1]];
    }
    Some(if d[0].abs() >= d[1].abs() {
        [if d[0] < 0.0 { -1.0 } else { 1.0 }, 0.0]
    } else {
        [0.0, if d[1] < 0.0 { -1.0 } else { 1.0 }]
    })
}

/// How a label of `kind` turns to face along the axis `d`: a flag or text
/// runs along it, and a supply port's bar or arrow or a ground's bars lie
/// beyond the pin's end.
pub fn auto_rotation(kind: LabelKind, d: Point) -> Rotation {
    // Quarter turns from +x to d.
    let turns: u8 = if d[0] > 0.5 {
        0
    } else if d[1] > 0.5 {
        1
    } else if d[0] < -0.5 {
        2
    } else {
        3
    };
    // Which way the drawing faces unturned, in quarter turns from +x: a
    // supply port up, a ground down, anything else along +x.
    let facing: u8 = match kind {
        LabelKind::Power {
            style: PowerStyle::Ground,
        } => 3,
        LabelKind::Power { .. } => 1,
        _ => 0,
    };
    match (turns + 4 - facing) % 4 {
        0 => Rotation::R0,
        1 => Rotation::R90,
        2 => Rotation::R180,
        _ => Rotation::R270,
    }
}

/// The pin end and outward axis of a label that places itself.
pub fn pin_frame(project: &Project, label: &Label) -> Option<(Point, Point)> {
    let endpoint = attached_pin(project, label)?;
    project
        .occurrences_of(endpoint.instance)
        .filter(|o| o.sheet == label.sheet)
        .find_map(|o| {
            let unit = placed_unit(project, o.id).ok()?;
            let pin = unit.pins.iter().find(|p| p.terminal == endpoint.terminal)?;
            Some((pin.end, outward(&unit, endpoint.terminal)?))
        })
}

/// The rotation a label is drawn with now.
pub(super) fn current_rotation(project: &Project, id: Id) -> Result<Rotation> {
    let label = project
        .label(id)
        .ok_or_else(|| not_found("Unknown label."))?;
    Ok(match pin_frame(project, label) {
        Some((_, d)) => auto_rotation(label.kind, d),
        None => label.rotation,
    })
}

/// Remove a label. Wires on it stay joined to each other at a junction left
/// in its place; a single wire left dangling is removed with it.
pub fn remove(project: &mut Project, id: Id) -> Result<()> {
    let label = project
        .label(id)
        .cloned()
        .ok_or_else(|| not_found("Unknown label."))?;
    detach_point(
        project,
        WireEnd::label(id),
        label.sheet,
        anchor(project, &label),
    )?;
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
