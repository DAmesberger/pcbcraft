//! Persistent net identity. Wires define connectivity; nets record which
//! connected terminal group carries which stable ID and name.
//!
//! After any schematic change, [`reconcile`] keeps every existing net ID with
//! the connected group holding most of its former terminals. Merged groups keep
//! the routed (or else oldest) ID; split-off groups and new terminals receive new
//! IDs. Tracks are never retagged here: callers decide whether a change to
//! routed nets is acceptable.
use crate::diagnostics::{Error, Result};
use crate::ids::{Endpoint, Id};
use crate::model::{Net, Project};
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_NETS: usize = 262_144;

/// What a reconciliation did to net identities.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NetChanges {
    /// (merged-away ID, surviving ID)
    pub merged: Vec<(Id, Id)>,
    /// (original ID, new ID of a split-off group)
    pub split: Vec<(Id, Id)>,
    pub created: Vec<Id>,
    /// IDs whose terminals all disappeared.
    pub removed: Vec<Id>,
}

/// Every terminal of every instance, whatever its unit's state, in sorted
/// order.
pub fn terminals(project: &Project) -> BTreeSet<Endpoint> {
    let mut result = BTreeSet::new();
    for instance in &project.instances {
        if let Some(component) = project.library.component(instance.component) {
            for terminal in &component.definition.terminals {
                result.insert(Endpoint {
                    instance: instance.id,
                    terminal: terminal.id,
                });
            }
        }
    }
    result
}

/// A terminal is accounted for when it is connected (its net has at least two
/// terminals), marked project no-connect, or not electrical (device NC or
/// reserved), which needs no decision.
pub fn accounted_for(project: &Project, terminal: Endpoint) -> bool {
    let Some(instance) = project.instance(terminal.instance) else {
        return false;
    };
    let no_decision = project
        .library
        .component(instance.component)
        .and_then(|c| c.definition.terminal(terminal.terminal))
        .is_some_and(|t| !t.is_electrical());
    no_decision
        || instance.no_connect.contains(&terminal.terminal)
        || project
            .net_of(terminal)
            .is_some_and(|net| net.terminals.len() >= 2)
}

/// Wire-connected terminal groups, ordered by their smallest terminal.
pub fn components(project: &Project, terminals: &BTreeSet<Endpoint>) -> Vec<Vec<Endpoint>> {
    let index: BTreeMap<Endpoint, usize> =
        terminals.iter().enumerate().map(|(i, t)| (*t, i)).collect();
    let mut parent: Vec<usize> = (0..terminals.len()).collect();
    fn find(parent: &mut [usize], mut at: usize) -> usize {
        while parent[at] != at {
            parent[at] = parent[parent[at]];
            at = parent[at];
        }
        at
    }
    for wire in &project.wires {
        if let (Some(&a), Some(&b)) = (index.get(&wire.from), index.get(&wire.to)) {
            let (a, b) = (find(&mut parent, a), find(&mut parent, b));
            if a != b {
                parent[a.max(b)] = a.min(b);
            }
        }
    }
    let mut groups: BTreeMap<usize, Vec<Endpoint>> = BTreeMap::new();
    for (i, terminal) in terminals.iter().enumerate() {
        let root = find(&mut parent, i);
        groups.entry(root).or_default().push(*terminal);
    }
    // Roots are the smallest index of each group, so this is min-terminal order.
    groups.into_values().collect()
}

/// Rebuild `project.nets` from wires while preserving existing identities.
pub fn reconcile(project: &mut Project) -> Result<NetChanges> {
    let terminals = terminals(project);
    if terminals.len() > MAX_NETS {
        return Err(Error::Limit(format!(
            "projects support at most {MAX_NETS} terminals"
        )));
    }
    let groups = components(project, &terminals);
    let old_net_of: BTreeMap<Endpoint, Id> = project
        .nets
        .iter()
        .flat_map(|net| net.terminals.iter().map(move |t| (*t, net.id)))
        .collect();
    let routed: BTreeSet<Id> = project.tracks.iter().map(|t| t.net).collect();
    // Overlap of each old net with each group; the largest overlap is its home.
    let mut overlap: BTreeMap<Id, BTreeMap<usize, usize>> = BTreeMap::new();
    for (group, members) in groups.iter().enumerate() {
        for terminal in members {
            if let Some(id) = old_net_of.get(terminal) {
                *overlap.entry(*id).or_default().entry(group).or_default() += 1;
            }
        }
    }
    let mut homed: BTreeMap<usize, Vec<Id>> = BTreeMap::new();
    for (id, counts) in &overlap {
        // max_by_key keeps the last maximum; iterate reversed to prefer the
        // earliest group on ties.
        if let Some((group, _)) = counts.iter().rev().max_by_key(|(_, count)| **count) {
            homed.entry(*group).or_default().push(*id);
        }
    }
    let names: BTreeMap<Id, String> = project
        .nets
        .iter()
        .map(|n| (n.id, n.name.clone()))
        .collect();
    let mut changes = NetChanges::default();
    let mut nets = Vec::with_capacity(groups.len());
    let mut group_net = Vec::with_capacity(groups.len());
    for (group, members) in groups.into_iter().enumerate() {
        let candidates = homed.remove(&group).unwrap_or_default();
        let survivor = match candidates
            .iter()
            .find(|id| routed.contains(id))
            .or(candidates.first())
        {
            Some(id) => *id,
            None => {
                let id = project.allocate_net()?;
                changes.created.push(id);
                id
            }
        };
        for id in &candidates {
            if *id != survivor {
                changes.merged.push((*id, survivor));
            }
        }
        let name = names
            .get(&survivor)
            .cloned()
            .unwrap_or_else(|| default_name(survivor));
        group_net.push(survivor);
        nets.push(Net {
            id: survivor,
            name,
            terminals: members,
        });
    }
    for (id, counts) in &overlap {
        for group in counts.keys() {
            let assigned = group_net.get(*group).copied();
            if let Some(assigned) = assigned
                && assigned != *id
                && !changes.merged.contains(&(*id, assigned))
            {
                changes.split.push((*id, assigned));
            }
        }
    }
    let kept: BTreeSet<Id> = nets.iter().map(|n| n.id).collect();
    let merged: BTreeSet<Id> = changes.merged.iter().map(|(id, _)| *id).collect();
    changes.removed = project
        .nets
        .iter()
        .map(|n| n.id)
        .filter(|id| !kept.contains(id) && !merged.contains(id))
        .collect();
    nets.sort_by_key(|n| n.id);
    project.nets = nets;
    Ok(changes)
}

pub fn default_name(id: Id) -> String {
    format!("N{id}")
}

pub fn name_valid(name: &str) -> bool {
    !name.trim().is_empty()
        && name.len() <= 64
        && !name.chars().any(|c| c.is_control() || c.is_whitespace())
}

/// Net records must match wire connectivity exactly.
pub fn validate(project: &Project) -> Result<()> {
    let invalid = |message: &str| Err(Error::Invalid(message.into()));
    if project.nets.len() > MAX_NETS {
        return invalid("too many nets");
    }
    let terminals = terminals(project);
    let mut seen_ids = BTreeSet::new();
    let mut names = BTreeSet::new();
    let mut members: BTreeMap<Endpoint, Id> = BTreeMap::new();
    for net in &project.nets {
        if net.id == 0 || net.id >= project.next_net || !seen_ids.insert(net.id) {
            return invalid("net identities must be nonzero, allocated and unique");
        }
        if !name_valid(&net.name) || !names.insert(net.name.to_lowercase()) {
            return invalid("net names must be unique, 1–64 bytes, without spaces");
        }
        if net.terminals.is_empty() || !net.terminals.windows(2).all(|w| w[0] < w[1]) {
            return invalid("net terminals must be nonempty and strictly sorted");
        }
        for terminal in &net.terminals {
            if !terminals.contains(terminal) {
                return invalid("a net references a missing terminal");
            }
            if members.insert(*terminal, net.id).is_some() {
                return invalid("a terminal belongs to more than one net");
            }
        }
    }
    if members.len() != terminals.len() {
        return invalid("every terminal must belong to a net");
    }
    for group in components(project, &terminals) {
        let id = group.first().and_then(|t| members.get(t));
        let Some(net) = id.and_then(|id| project.net(*id)) else {
            return invalid("a connected terminal group has no net");
        };
        if net.terminals != group {
            return invalid("net membership does not match wire connectivity");
        }
    }
    Ok(())
}
