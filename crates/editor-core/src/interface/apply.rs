//! The `interface.*` document commands on the slot table
//! (docs/architecture/board-interface.md, "Commands"). Each works on the
//! table as the automatic rules make it ([`super::list::table`]) and
//! returns the new table, or `None` when the command changes nothing; a
//! refusal returns an error and the caller keeps the document as it was.
use super::command::{ExportTarget, InterfaceCommand};
use super::{allocate_slot, auto, check_table, list, package};
use crate::diagnostics::{Error, Result};
use crate::ids::InterfaceSourceId;
use crate::model::Project;
use crate::model::board::{BoardShape, ConstraintTarget};
use crate::model::dimension::BoardRef;
use crate::model::interface::{
    DatumId, DatumKey, DatumOwner, DatumRole, DatumSource, InterfaceDatum, InterfaceTable,
    MAX_DATUMS, MAX_PUBLISHED_FILES, PublishedFile, PublishedRecord,
};
use std::collections::BTreeSet;

fn refused(code: &str, why: &str) -> Error {
    Error::Invalid(format!("{code}: {why}"))
}

/// The live datum with this id, or `interface.unknown`.
fn live(table: &InterfaceTable, datum: DatumId) -> Result<&InterfaceDatum> {
    table.datum(datum).ok_or_else(|| {
        Error::NotFound(format!(
            "interface.unknown: no exported datum has the id {datum}"
        ))
    })
}

/// A display name the user gave: printable and bounded, or refused.
fn checked_name(name: Option<String>) -> Result<Option<String>> {
    match name {
        Some(n) if !crate::ids::printable(&n) => Err(refused(
            "interface.name",
            "a datum's name must be printable text of at most 256 bytes",
        )),
        Some(n) => Ok(Some(n.trim().to_owned())),
        None => Ok(None),
    }
}

/// A placed part, or `interface.owner`.
fn placed(project: &Project, instance: crate::ids::Id) -> Result<()> {
    if project.placement_of(instance).is_some() {
        Ok(())
    } else {
        Err(Error::NotFound(
            "interface.owner: the part is not placed on the board".into(),
        ))
    }
}

/// The key and feature `target` exports, after checking what it names
/// exists.
fn key_of(project: &Project, target: &ExportTarget) -> Result<(DatumKey, Option<BoardRef>)> {
    let key = |owner, role, name: Option<String>| DatumKey { owner, role, name };
    match target {
        ExportTarget::Feature { feature } => {
            let owner = match feature {
                BoardRef::Housing(_) => {
                    return Err(refused(
                        "interface.housing",
                        "a housing reference belongs to the enclosure; the board cannot export it",
                    ));
                }
                BoardRef::Board(_) => DatumOwner::Board,
                BoardRef::Part(f) => match f.target() {
                    ConstraintTarget::Placement(id) => DatumOwner::Instance { id },
                    ConstraintTarget::Fill(_) | ConstraintTarget::Keepout(_) => DatumOwner::Board,
                },
            };
            let ids = crate::layout::dimensions::board_ids(project);
            let status =
                crate::layout::dimensions::reference_status(project, ids.as_ref(), feature);
            if status == crate::model::dimension::ReferenceStatus::Lost {
                return Err(Error::NotFound(
                    "interface.owner: the feature is not on the board".into(),
                ));
            }
            Ok((key(owner, DatumRole::Feature, None), Some(feature.clone())))
        }
        ExportTarget::PackageDatum { instance, name } => {
            placed(project, *instance)?;
            let declared = package::placed(project, *instance)
                .into_iter()
                .find(|d| d.name == *name)
                .ok_or_else(|| {
                    Error::NotFound(format!(
                        "interface.unknown_datum: the part's package declares no datum `{}`",
                        name.0
                    ))
                })?;
            if declared.external {
                return Err(Error::Conflict(
                    "interface.automatic: the package marks this datum for the enclosure, so it \
                     is exported for every placement; switch the external datums rule instead"
                        .into(),
                ));
            }
            Ok((
                key(
                    DatumOwner::Instance { id: *instance },
                    DatumRole::PartDatum,
                    Some(name.0.clone()),
                ),
                None,
            ))
        }
        ExportTarget::PartFrame { instance } => {
            placed(project, *instance)?;
            Ok((
                key(
                    DatumOwner::Instance { id: *instance },
                    DatumRole::PartFrame,
                    None,
                ),
                None,
            ))
        }
        ExportTarget::PartEnvelope { instance } => {
            placed(project, *instance)?;
            Ok((
                key(
                    DatumOwner::Instance { id: *instance },
                    DatumRole::PartEnvelope,
                    None,
                ),
                None,
            ))
        }
        ExportTarget::BoardParameter { name } => {
            let known = matches!(&project.board_shape, BoardShape::Sketch(s)
                if s.parameters.iter().any(|p| p.name == *name));
            if !known {
                return Err(Error::NotFound(format!(
                    "interface.owner: the board has no parameter `{}`",
                    name.0
                )));
            }
            Ok((
                key(
                    DatumOwner::Board,
                    DatumRole::BoardParameter,
                    Some(name.0.clone()),
                ),
                None,
            ))
        }
    }
}

fn export(
    project: &Project,
    table: &mut InterfaceTable,
    target: &ExportTarget,
    name: Option<String>,
) -> Result<()> {
    let (key, feature) = key_of(project, target)?;
    let name = checked_name(name)?;
    let taken = table.datums.iter().any(|d| match &feature {
        Some(f) => d.feature.as_ref() == Some(f),
        None => d.key == key,
    });
    if taken {
        return Err(Error::Conflict(
            "interface.exported: this is exported already".into(),
        ));
    }
    if table.datums.len() >= MAX_DATUMS {
        return Err(Error::Limit(format!(
            "interface.datums: at most {MAX_DATUMS} exported datums"
        )));
    }
    let slot = allocate_slot(table)?;
    let id = auto::allocate_id(table)?;
    table.datums.push(InterfaceDatum {
        id,
        slot,
        key,
        source: DatumSource::User,
        feature,
        name,
    });
    Ok(())
}

/// Check a publish the host recorded against the table, and store it.
fn record_publish(
    table: &mut InterfaceTable,
    source_id: InterfaceSourceId,
    record: PublishedRecord,
) -> Result<()> {
    if table.source_id.is_some_and(|s| s != source_id) {
        return Err(Error::Conflict(
            "interface.source: this board's interface was published under another identity".into(),
        ));
    }
    let expected = match &table.published {
        None => 1,
        Some(last) if last.hash == record.hash => last.revision,
        Some(last) => last
            .revision
            .checked_add(1)
            .ok_or_else(|| Error::Limit("interface.revision: revisions exhausted".into()))?,
    };
    if record.revision != expected {
        return Err(Error::Conflict(format!(
            "interface.revision: the next published revision is {expected}, not {}",
            record.revision
        )));
    }
    let mut seen = BTreeSet::new();
    let ordered = record.datums.windows(2).all(|w| w[0].slot < w[1].slot);
    let known = record
        .datums
        .iter()
        .all(|p| seen.insert(p.id) && table.datum(p.id).is_some_and(|d| d.slot == p.slot));
    if record.datums.len() > MAX_DATUMS || !ordered || !known {
        return Err(refused(
            "interface.record",
            "a published record lists live datums once each, in slot order",
        ));
    }
    if !record.earlier.is_empty() {
        return Err(refused(
            "interface.record",
            "the earlier published files are the board's own record",
        ));
    }
    let mut record = record;
    if let Some(last) = table.published.take() {
        // The files earlier publishes wrote name the revision an enclosure
        // still holds (slice B5); the oldest go first past the bound.
        let mut earlier = last.earlier;
        if let Some(sha256) = last.design_sha256
            && record.design_sha256 != Some(sha256)
        {
            earlier.retain(|f| f.sha256 != sha256);
            earlier.push(PublishedFile {
                revision: last.revision,
                sha256,
            });
        }
        if let Some(now) = record.design_sha256 {
            earlier.retain(|f| f.sha256 != now);
        }
        let over = earlier.len().saturating_sub(MAX_PUBLISHED_FILES);
        earlier.drain(..over);
        record.earlier = earlier;
    }
    table.source_id = Some(source_id);
    table.published = Some(record);
    Ok(())
}

/// Apply one interface command to `project`'s table. `Ok(None)`: it
/// changes nothing (no revision, no undo step).
pub fn apply(project: &Project, command: InterfaceCommand) -> Result<Option<InterfaceTable>> {
    let base = list::table(project)?;
    let mut table = base.as_ref().clone();
    match command {
        InterfaceCommand::Export { target, name } => export(project, &mut table, &target, name)?,
        InterfaceCommand::Unexport { datum } => {
            let found = live(&table, datum)?;
            if matches!(found.source, DatumSource::Auto { .. }) {
                return Err(Error::Conflict(
                    "interface.automatic: an automatic datum is withdrawn by switching its rule \
                     off"
                    .into(),
                ));
            }
            let slot = found.slot;
            super::retire(&mut table, slot)?;
        }
        InterfaceCommand::Rename { datum, name } => {
            let name = checked_name(name)?;
            live(&table, datum)?;
            if let Some(found) = table.datums.iter_mut().find(|d| d.id == datum) {
                found.name = name;
            }
        }
        InterfaceCommand::Auto { rule, on } => {
            let at = table.rules_off.binary_search(&rule);
            match (on, at) {
                (true, Ok(i)) => {
                    table.rules_off.remove(i);
                }
                (false, Err(i)) => table.rules_off.insert(i, rule),
                _ => {}
            }
            if let Some(next) = auto::reconcile(&table, project)? {
                table = next;
            }
        }
        InterfaceCommand::RecordPublish { source_id, record } => {
            record_publish(&mut table, source_id, record)?;
        }
    }
    if table == *base {
        return Ok(None);
    }
    check_table(&table)?;
    Ok(Some(table))
}
