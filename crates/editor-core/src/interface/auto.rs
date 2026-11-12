//! The automatic export rules and how the slot table follows the board
//! (docs/architecture/board-interface.md, "What is exported").
//!
//! Each rule that is on names the datum keys the board needs now
//! ([`needed`]). [`reconcile`] makes the table match: a needed key without
//! a live datum gets a new slot and id; an automatic datum whose key is no
//! longer needed (its owner is gone, or its rule is off) is retired. User
//! datums are never touched here: one whose owner is gone stays, lost,
//! until it is unexported.
//!
//! The table is stored from the first `interface.*` command on; until then
//! it stays empty and `interface.list` shows what that command would
//! store. The engine runs [`follow`] after every document change, and
//! [`carry`] after every undo and redo, so the slot and id counters only
//! grow: an undone export's slot is retired, never handed to the next
//! export.
use super::{allocate_slot, check_table, package};
use crate::diagnostics::{Error, Result};
use crate::model::Project;
use crate::model::interface::{
    AutoRule, DatumId, DatumKey, DatumOwner, DatumRole, DatumSource, InterfaceDatum,
    InterfaceTable, Slot,
};
use std::collections::{BTreeMap, BTreeSet};

/// The keys the automatic rules that are on in `table` need on `project`,
/// in allocation order: rule by rule (`AutoRule::ALL`), keys ascending
/// within a rule.
pub fn needed(project: &Project, table: &InterfaceTable) -> Vec<(AutoRule, DatumKey)> {
    let board = |role| DatumKey {
        owner: DatumOwner::Board,
        role,
        name: None,
    };
    let mut out = Vec::new();
    for rule in AutoRule::ALL {
        if !table.rule_on(rule) {
            continue;
        }
        let mut keys: Vec<DatumKey> = match rule {
            AutoRule::Board => vec![
                board(DatumRole::BoardFrame),
                board(DatumRole::BoardTop),
                board(DatumRole::BoardBottom),
                board(DatumRole::BoardThickness),
            ],
            AutoRule::Outline => vec![board(DatumRole::BoardOutline)],
            AutoRule::Cutouts => project
                .board()
                .cutouts
                .iter()
                .filter_map(|c| super::cutout_owner(&c.curves))
                .map(|curve| DatumKey {
                    owner: DatumOwner::Curve { id: curve.clone() },
                    role: DatumRole::BoardCutout,
                    name: None,
                })
                .collect(),
            AutoRule::Holes => project
                .board()
                .holes
                .iter()
                .map(|h| DatumKey {
                    owner: DatumOwner::Hole { id: h.id },
                    role: DatumRole::Hole,
                    name: None,
                })
                .collect(),
            AutoRule::ExternalDatums => project
                .placements
                .iter()
                .flat_map(|p| {
                    package::placed(project, p.instance)
                        .into_iter()
                        .filter(|d| d.external)
                        .map(move |d| DatumKey {
                            owner: DatumOwner::Instance { id: p.instance },
                            role: DatumRole::PartDatum,
                            name: Some(d.name.0),
                        })
                })
                .collect(),
            AutoRule::SideHeights => ["top", "bottom"]
                .into_iter()
                .map(|side| DatumKey {
                    owner: DatumOwner::Board,
                    role: DatumRole::SideHeight,
                    name: Some(side.into()),
                })
                .collect(),
        };
        keys.sort();
        keys.dedup();
        out.extend(keys.into_iter().map(|k| (rule, k)));
    }
    out
}

/// Allocate the next datum id from the table's own counter. Refused with
/// `interface.ids` when it is exhausted.
pub fn allocate_id(table: &mut InterfaceTable) -> Result<DatumId> {
    let id = table.next_datum.max(1);
    table.next_datum = id
        .checked_add(1)
        .ok_or_else(|| Error::Limit("interface.ids: datum identity space exhausted".into()))?;
    Ok(DatumId(id))
}

/// `table` matched to the automatic rules on `project`, or `None` when it
/// already matches. Datums stay in slot order: retired ones leave, new ones
/// take the next slots.
pub fn reconcile(table: &InterfaceTable, project: &Project) -> Result<Option<InterfaceTable>> {
    let needed = needed(project, table);
    let wanted: BTreeMap<&DatumKey, AutoRule> = needed.iter().map(|(r, k)| (k, *r)).collect();
    let stale: Vec<Slot> = table
        .datums
        .iter()
        .filter(|d| match d.source {
            DatumSource::Auto { rule } => wanted.get(&d.key) != Some(&rule),
            DatumSource::User => false,
        })
        .map(|d| d.slot)
        .collect();
    let live: BTreeSet<&DatumKey> = table
        .datums
        .iter()
        .filter(|d| !stale.contains(&d.slot))
        .map(|d| &d.key)
        .collect();
    let missing: Vec<&(AutoRule, DatumKey)> =
        needed.iter().filter(|(_, k)| !live.contains(k)).collect();
    if stale.is_empty() && missing.is_empty() {
        return Ok(None);
    }
    let mut next = table.clone();
    for slot in stale {
        super::retire(&mut next, slot)?;
    }
    for (rule, key) in missing {
        let slot = allocate_slot(&mut next)?;
        let id = allocate_id(&mut next)?;
        next.datums.push(InterfaceDatum {
            id,
            slot,
            key: key.clone(),
            source: DatumSource::Auto { rule: *rule },
            feature: None,
            name: None,
        });
    }
    check_table(&next)?;
    Ok(Some(next))
}

/// The table after a change from `before` to `after`, as if `before`'s
/// table had first been matched to `before` and then to `after`. `None`
/// when `after`'s table already matches, or when it is empty: a document
/// is stored with no table until its first `interface.*` command, so a
/// project that never uses the interface is saved, and undone, exactly as
/// before. Until then `interface.list` shows the slots that command would
/// store; nothing outside the document can have seen them, since only a
/// publish (itself an interface command) writes them out.
pub fn follow(before: &Project, after: &Project) -> Result<Option<InterfaceTable>> {
    if after.interface.is_empty() {
        return Ok(None);
    }
    let first = reconcile(&after.interface, before)?;
    let base: &InterfaceTable = first.as_ref().unwrap_or(&after.interface);
    match reconcile(base, after)? {
        Some(table) => Ok(Some(table)),
        None => Ok(first),
    }
}

/// `restored`'s table after an undo or redo from a document whose table
/// is `current`: the counters keep their highest values, so no slot or id
/// is ever handed out twice. An automatic datum `current` holds that the
/// restored board still needs keeps its slot and id (undoing a document's
/// first edit does not renumber the board's own datums); every other slot
/// the restored table does not hold is retired. `None` when nothing
/// changes.
pub fn carry(current: &InterfaceTable, restored: &Project) -> Result<Option<InterfaceTable>> {
    let table: &InterfaceTable = &restored.interface;
    let mut next = table.clone();
    next.next_slot = table.next_slot.max(current.next_slot);
    next.next_datum = table.next_datum.max(current.next_datum);
    let wanted: BTreeMap<DatumKey, AutoRule> = needed(restored, table)
        .into_iter()
        .map(|(r, k)| (k, r))
        .collect();
    for datum in &current.datums {
        let adopt = matches!(datum.source, DatumSource::Auto { rule }
            if wanted.get(&datum.key) == Some(&rule))
            && !next
                .datums
                .iter()
                .any(|d| d.slot == datum.slot || d.key == datum.key || d.id == datum.id);
        if adopt {
            let at = next.datums.partition_point(|d| d.slot < datum.slot);
            next.datums.insert(at, datum.clone());
        }
    }
    let live: BTreeSet<Slot> = next.datums.iter().map(|d| d.slot).collect();
    next.retired = (0..next.next_slot)
        .map(Slot)
        .filter(|s| !live.contains(s))
        .collect();
    if next == *table {
        return Ok(None);
    }
    check_table(&next)?;
    Ok(Some(next))
}
