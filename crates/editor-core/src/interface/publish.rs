//! What publishing the board's interface writes and what it would change
//! (docs/architecture/board-interface.md, "The export format" and "Link
//! updates"; slice B3).
//!
//! [`candidate`] is the sidecar a publish of the document as it stands
//! writes: every live datum with its derived geometry and content hash, in
//! slot order, the retired slots, the table's hash and the revision it
//! would record. The host builds the exported design from it (its slots
//! carry exactly these datums), fills in the face names it made, writes
//! both files and records the publish with [`record`].
//!
//! [`preview`] is `interface.publish_preview`'s diff: per datum, against
//! the recorded publish, and against the published sidecar when the host
//! could read it, so a datum that moved says by how much.
use super::report::{ChangeKind, DatumChange, PublishPreview};
use super::sidecar::{FORMAT, UNITS};
use super::{DatumGeometry, DatumStatus, Frame, Sidecar, SidecarDatum, Vec3, list, package};
use crate::diagnostics::{Error, Result};
use crate::ids::{ContentHash, InterfaceSourceId};
use crate::library::{Outline, Segment};
use crate::model::Project;
use crate::model::interface::{
    DatumOwner, DatumRole, InterfaceDatum, PublishedDatum, PublishedRecord, Slot, Tier,
};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

/// Positions, lengths and turns closer than this are the same (mm, and
/// unit vectors' components). Derived geometry is computed in `f64` from
/// whole nanometres, so a datum that did not change compares exactly; the
/// tolerance only absorbs rounding in a move's arithmetic.
const SAME: f64 = 1e-9;

/// The canonical hash of a published datum table: its live datums' slots,
/// tokens and content hashes in slot order, its retired slots and its
/// unresolved slots. Display names, the tier and face names are not part
/// of it, so renaming J1 or publishing another tier keeps the revision.
pub fn table_hash(
    datums: &[SidecarDatum],
    retired: &[Slot],
    unresolved: &[Slot],
) -> Result<ContentHash> {
    let rows: Vec<serde_json::Value> = datums
        .iter()
        .map(|d| serde_json::json!([d.slot, d.token, d.hash]))
        .collect();
    let canonical = serde_json::to_vec(&serde_json::json!({
        "datums": rows,
        "retired": retired,
        "unresolved": unresolved,
    }))?;
    Ok(ContentHash(Sha256::digest(&canonical).into()))
}

/// The clearance a part's package datum asks for, mm.
fn clearance(project: &Project, datum: &InterfaceDatum) -> Option<f64> {
    let (DatumOwner::Instance { id }, DatumRole::PartDatum) = (&datum.key.owner, datum.key.role)
    else {
        return None;
    };
    let name = datum.key.name.as_deref()?;
    package::placed(project, *id)
        .into_iter()
        .find(|d| d.name.0 == name)
        .and_then(|d| d.clearance_mm)
}

/// The sidecar publishing `project` at `tier` writes, under `source_id`,
/// without face names (the host fills those in as it builds the design).
/// A live datum without geometry (lost, or a parameter given as an
/// expression) is left out and its slot listed as unresolved: the design
/// carries nothing in it, and the linked design's references to it fail.
pub fn candidate(project: &Project, tier: Tier, source_id: InterfaceSourceId) -> Result<Sidecar> {
    let table = list::table(project)?;
    if table.source_id.is_some_and(|s| s != source_id) {
        return Err(Error::Conflict(
            "interface.source: this board's interface was published under another identity".into(),
        ));
    }
    let rows = list::rows(project, &table)?;
    let mut datums = Vec::with_capacity(rows.len());
    let mut unresolved = Vec::new();
    for (row, datum) in rows.into_iter().zip(table.datums.iter()) {
        let hash = list::datum_hash(&row)?;
        let Some(geometry) = row.geometry else {
            unresolved.push(row.slot);
            continue;
        };
        datums.push(SidecarDatum {
            id: row.id,
            slot: row.slot,
            token: row.token,
            name: row.name,
            role: row.role,
            kind: row.kind,
            owner: datum.key.owner.clone(),
            geometry,
            clearance_mm: clearance(project, datum),
            status: row.status,
            hash,
            faces: BTreeMap::new(),
        });
    }
    let hash = table_hash(&datums, &table.retired, &unresolved)?;
    let revision = match &table.published {
        None => 1,
        Some(last) if last.hash == hash => last.revision,
        Some(last) => last
            .revision
            .checked_add(1)
            .ok_or_else(|| Error::Limit("interface.revision: revisions exhausted".into()))?,
    };
    let sidecar = Sidecar {
        format: FORMAT.into(),
        source_id,
        revision,
        hash,
        units: UNITS.into(),
        tier,
        datums,
        retired_slots: table.retired.clone(),
        unresolved_slots: unresolved,
    };
    sidecar.check()?;
    Ok(sidecar)
}

/// The record `interface.record_publish` stores after `sidecar` was
/// written: its revision, hash and tier, and every live datum's slot and
/// content hash (unresolved ones included, so a preview tells them apart
/// from new ones). The host sets `design_sha256` to the file it wrote.
pub fn record(project: &Project, sidecar: &Sidecar) -> Result<PublishedRecord> {
    let table = list::table(project)?;
    let rows = list::rows(project, &table)?;
    let mut datums = Vec::with_capacity(rows.len());
    for row in &rows {
        datums.push(PublishedDatum {
            id: row.id,
            slot: row.slot,
            hash: list::datum_hash(row)?,
        });
    }
    Ok(PublishedRecord {
        revision: sidecar.revision,
        hash: sidecar.hash,
        tier: sidecar.tier,
        datums,
        design_sha256: None,
        earlier: Vec::new(),
    })
}

/// A number for people: at most four decimals, without trailing zeros.
fn number(value: f64) -> String {
    let text = format!("{:.4}", value + 0.0);
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text == "-0" {
        "0".into()
    } else {
        text.into()
    }
}

/// The sentence a preview row reads: "J1 opening, 2 mm along X".
pub fn summary(name: &str, change: &ChangeKind) -> String {
    let what = match change {
        ChangeKind::Unchanged => "unchanged".into(),
        ChangeKind::Added => "added".into(),
        ChangeKind::Retired => "removed: a linked design feature that uses it will fail".into(),
        ChangeKind::Reshaped => "reshaped".into(),
        ChangeKind::Changed => "changed".into(),
        ChangeKind::StatusChanged { to, .. } => match to {
            DatumStatus::Resolved => "found again".into(),
            DatumStatus::Inexact => "found again only by geometry".into(),
            DatumStatus::Lost => {
                "lost: what it belongs to is gone, and a linked design feature that uses it will fail"
                    .into()
            }
        },
        ChangeKind::Moved { offset, turn_deg } => {
            let [x, y, z] = offset.0;
            let axes: Vec<(&str, f64)> = [("X", x), ("Y", y), ("Z", z)]
                .into_iter()
                .filter(|(_, v)| v.abs() > SAME)
                .collect();
            let mut text = match axes.as_slice() {
                [] => String::new(),
                [(axis, v)] => format!("{} mm along {axis}", number(*v)),
                _ => format!(
                    "{} mm ({})",
                    number((x * x + y * y + z * z).sqrt()),
                    axes.iter()
                        .map(|(axis, v)| format!("{} along {axis}", number(*v)))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            };
            if turn_deg.abs() > SAME {
                if !text.is_empty() {
                    text.push_str(", ");
                }
                text.push_str(&format!("turned {}°", number(*turn_deg)));
            }
            text
        }
    };
    format!("{name}, {what}")
}

fn sub(a: Vec3, b: Vec3) -> Vec3 {
    Vec3([a.0[0] - b.0[0], a.0[1] - b.0[1], a.0[2] - b.0[2]])
}

fn same_vec(a: Vec3, b: Vec3) -> bool {
    a.0.iter()
        .zip(b.0.iter())
        .all(|(p, q)| (p - q).abs() <= SAME)
}

fn same(a: f64, b: f64) -> bool {
    (a - b).abs() <= SAME
}

fn cross(a: Vec3, b: Vec3) -> Vec3 {
    let [ax, ay, az] = a.0;
    let [bx, by, bz] = b.0;
    Vec3([ay * bz - az * by, az * bx - ax * bz, ax * by - ay * bx])
}

fn dot(a: Vec3, b: Vec3) -> f64 {
    a.0.iter().zip(b.0.iter()).map(|(p, q)| p * q).sum()
}

/// The turn about `axis` from `from` to `to`, degrees.
fn turn(from: Vec3, to: Vec3, axis: Vec3) -> f64 {
    let angle = dot(cross(from, to), axis).atan2(dot(from, to)).to_degrees();
    if angle.abs() <= SAME { 0.0 } else { angle }
}

/// `b` as `a` shifted by `d` in the loop's plane: `Some(d)` when every
/// point moved alike and every arc kept its bulge.
fn loop_shift(a: &Outline, b: &Outline) -> Option<[f64; 2]> {
    if a.segments.len() != b.segments.len() {
        return None;
    }
    let d = [b.start[0] - a.start[0], b.start[1] - a.start[1]];
    let moved = |p: [f64; 2], q: [f64; 2]| same(q[0] - p[0], d[0]) && same(q[1] - p[1], d[1]);
    a.segments
        .iter()
        .zip(b.segments.iter())
        .all(|(s, t)| {
            moved(s.to(), t.to())
                && match (s, t) {
                    (Segment::Line { .. }, Segment::Line { .. }) => true,
                    (Segment::Arc { bulge: p, .. }, Segment::Arc { bulge: q, .. }) => same(*p, *q),
                    _ => false,
                }
        })
        .then_some(d)
}

/// Every loop shifted alike.
fn loops_shift(a: &[Outline], b: &[Outline]) -> Option<[f64; 2]> {
    if a.len() != b.len() {
        return None;
    }
    let mut shift = None;
    for (p, q) in a.iter().zip(b.iter()) {
        let d = loop_shift(p, q)?;
        match shift {
            None => shift = Some(d),
            Some(e) if same(d[0], e[0]) && same(d[1], e[1]) => {}
            Some(_) => return None,
        }
    }
    Some(shift.unwrap_or([0.0, 0.0]))
}

/// A frame's move to another frame with the same z (a turn about it and a
/// shift), with the loops drawn in each the same apart from a shift in the
/// plane: the offset in the board frame and the turn.
fn framed_move(a: &Frame, la: &[Outline], b: &Frame, lb: &[Outline]) -> Option<(Vec3, f64)> {
    let z = cross(a.x, a.y);
    if !same_vec(z, cross(b.x, b.y)) {
        return None;
    }
    let along = turn(a.x, b.x, z);
    if along == 0.0 {
        let d = loops_shift(la, lb)?;
        let shift = Vec3([
            b.x.0[0] * d[0] + b.y.0[0] * d[1],
            b.x.0[1] * d[0] + b.y.0[1] * d[1],
            b.x.0[2] * d[0] + b.y.0[2] * d[1],
        ]);
        let o = sub(b.origin, a.origin);
        return Some((
            Vec3([
                o.0[0] + shift.0[0],
                o.0[1] + shift.0[1],
                o.0[2] + shift.0[2],
            ]),
            0.0,
        ));
    }
    // Turned: the same loops in turned frames.
    (la == lb).then(|| (sub(b.origin, a.origin), along))
}

/// How `before` became `after`, both of one datum's kind; `None` when it
/// is not a rigid move (a reshape).
fn moved(before: &DatumGeometry, after: &DatumGeometry) -> Option<(Vec3, f64)> {
    use DatumGeometry as G;
    match (before, after) {
        (G::Frame { frame: a }, G::Frame { frame: b }) => framed_move(a, &[], b, &[]),
        (
            G::Plane {
                origin: a,
                normal: n,
            },
            G::Plane {
                origin: b,
                normal: m,
            },
        ) => same_vec(*n, *m).then(|| (sub(*b, *a), 0.0)),
        (
            G::Axis {
                point: a,
                direction: u,
                diameter: p,
            },
            G::Axis {
                point: b,
                direction: v,
                diameter: q,
            },
        ) => {
            let same_diameter = match (p, q) {
                (Some(p), Some(q)) => same(*p, *q),
                (None, None) => true,
                _ => false,
            };
            (same_vec(*u, *v) && same_diameter).then(|| (sub(*b, *a), 0.0))
        }
        (
            G::Profile {
                frame: a,
                loops: la,
            },
            G::Profile {
                frame: b,
                loops: lb,
            },
        ) => framed_move(a, la, b, lb),
        (
            G::Volume {
                frame: a,
                loops: la,
                height: h,
            },
            G::Volume {
                frame: b,
                loops: lb,
                height: k,
            },
        ) if same(*h, *k) => framed_move(a, la, b, lb),
        _ => None,
    }
}

/// One live datum's change against what was published.
fn change_of(
    datum: &SidecarDatum,
    published: Option<&PublishedDatum>,
    before: Option<&SidecarDatum>,
) -> ChangeKind {
    let Some(published) = published else {
        return ChangeKind::Added;
    };
    if published.hash == datum.hash {
        return ChangeKind::Unchanged;
    }
    let Some(before) = before.filter(|b| b.hash == published.hash) else {
        return ChangeKind::Changed;
    };
    if before.status != datum.status {
        return ChangeKind::StatusChanged {
            from: before.status,
            to: datum.status,
        };
    }
    match moved(&before.geometry, &datum.geometry) {
        Some((offset, turn_deg)) if same_vec(offset, Vec3::default()) && turn_deg == 0.0 => {
            ChangeKind::Changed
        }
        Some((offset, turn_deg)) => ChangeKind::Moved { offset, turn_deg },
        None => ChangeKind::Reshaped,
    }
}

/// `interface.publish_preview`'s diff: publishing `project` at `tier` (the
/// last published tier, or the mechanical one, when omitted) against the
/// recorded publish and, when given, the published sidecar (`previous`; it
/// is not trusted for the comparison unless its datums' hashes are the
/// recorded ones). Every live datum and every datum published and gone
/// since, in slot order. Reads only.
pub fn preview(
    project: &Project,
    tier: Option<Tier>,
    previous: Option<&Sidecar>,
    document_revision: u64,
) -> Result<PublishPreview> {
    let table = list::table(project)?;
    let tier = tier
        .or(table.published.as_ref().map(|r| r.tier))
        .unwrap_or_default();
    let source = table
        .source_id
        .or(previous.map(|p| p.source_id))
        .unwrap_or_default();
    let previous = previous.filter(|p| table.source_id.is_none_or(|s| s == p.source_id));
    let candidate = candidate(project, tier, source)?;
    let record = table.published.as_ref();
    let published = |slot: Slot| record.and_then(|r| r.datums.iter().find(|d| d.slot == slot));
    let before = |slot: Slot| previous.and_then(|p| p.datums.iter().find(|d| d.slot == slot));
    let rows = list::rows(project, &table)?;
    let mut changes: Vec<DatumChange> = Vec::new();
    for row in &rows {
        let change = match candidate.datums.iter().find(|d| d.slot == row.slot) {
            Some(datum) => change_of(datum, published(row.slot), before(row.slot)),
            // Unresolved now.
            None => match (published(row.slot), before(row.slot)) {
                (None, _) => ChangeKind::Added,
                (Some(p), _) if p.hash == list::datum_hash(row)? => ChangeKind::Unchanged,
                (Some(_), Some(b)) if b.status != row.status => ChangeKind::StatusChanged {
                    from: b.status,
                    to: row.status,
                },
                _ => ChangeKind::Changed,
            },
        };
        // A datum whose owner is gone reads under the name it was
        // published with ("R4 envelope", not "A deleted part's envelope").
        let name = match (&change, before(row.slot)) {
            (ChangeKind::StatusChanged { .. }, Some(b)) if row.geometry.is_none() => b.name.clone(),
            _ => row.name.clone(),
        };
        changes.push(DatumChange {
            datum: row.id,
            slot: row.slot,
            summary: summary(&name, &change),
            name,
            change,
        });
    }
    for gone in record.into_iter().flat_map(|r| r.datums.iter()) {
        if rows.iter().any(|r| r.id == gone.id) {
            continue;
        }
        let name = before(gone.slot).map_or_else(
            || format!("The datum in slot {}", gone.slot.0),
            |b| b.name.clone(),
        );
        let change = ChangeKind::Retired;
        changes.push(DatumChange {
            datum: gone.id,
            slot: gone.slot,
            summary: summary(&name, &change),
            name,
            change,
        });
    }
    changes.sort_by_key(|c| c.slot);
    Ok(PublishPreview {
        document_revision,
        from: record.map(|r| r.revision),
        to: candidate.revision,
        hash: candidate.hash,
        tier,
        changes,
        dry_run: None,
    })
}
