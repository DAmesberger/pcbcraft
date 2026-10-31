//! Same-cause merging. One problem stated from two sides, or repeated per
//! pad or per instance, becomes one row whose primary finding comes first;
//! every merged finding is kept unchanged ([`super::IssueRow::also`]), so
//! no count of findings, object or raw text is lost. A side without a
//! counterpart keeps its own row. Merging decides by objects and stored
//! data, never by matching message text.
use super::names::Names;
use crate::diagnostics::{Finding, ObjectRef};
use crate::ids::{DefinitionId, Id, PackageTerminalId};
use crate::library::Definition;
use std::collections::BTreeMap;

/// Group findings by `key`, in first-seen order; findings without a key
/// stay alone. Keys are internal and never shown.
fn by_key<K: Ord>(
    findings: Vec<Finding>,
    key: impl Fn(&Finding) -> Option<K>,
) -> Vec<Vec<Finding>> {
    let mut out: Vec<Vec<Finding>> = Vec::new();
    let mut at: BTreeMap<K, usize> = BTreeMap::new();
    for finding in findings {
        match key(&finding) {
            Some(k) => match at.get(&k) {
                Some(&i) => {
                    if let Some(group) = out.get_mut(i) {
                        group.push(finding);
                    }
                }
                None => {
                    at.insert(k, out.len());
                    out.push(vec![finding]);
                }
            },
            None => out.push(vec![finding]),
        }
    }
    out
}

/// Identical findings (code, message and objects) are one.
fn identical(findings: Vec<Finding>) -> Vec<Vec<Finding>> {
    by_key(findings, |f| Some((f.code.clone(), same(f))))
}

fn local(finding: &Finding) -> impl Iterator<Item = (DefinitionId, u32)> + '_ {
    finding.objects.iter().filter_map(|o| match *o {
        ObjectRef::LibraryObject { definition, local } => Some((definition, local)),
        _ => None,
    })
}

fn definitions(finding: &Finding) -> impl Iterator<Item = DefinitionId> + '_ {
    finding.objects.iter().filter_map(|o| match *o {
        ObjectRef::Definition { id } => Some(id),
        _ => None,
    })
}

/// Component findings: a pad without a package terminal and the package
/// terminal its number designates, when that terminal has no pad; an
/// electrical terminal a binding leaves unmapped and the one package
/// terminal it leaves unmapped, when each is the only one.
pub(super) fn component(names: &Names<'_>, findings: Vec<Finding>) -> Vec<Vec<Finding>> {
    let mut groups = identical(findings)
        .into_iter()
        .filter_map(|mut g| {
            let first = g.drain(..1).next()?;
            Some((first, g))
        })
        .collect::<Vec<_>>();
    pair_pads(names, &mut groups);
    pair_mappings(names, &mut groups);
    groups
        .into_iter()
        .map(|(first, rest)| std::iter::once(first).chain(rest).collect())
        .collect()
}

type Entry = (Finding, Vec<Finding>);

/// Move entry `from` into entry `into` (both indices valid, distinct).
fn absorb(groups: &mut Vec<Entry>, into: usize, from: usize) {
    if into == from || from >= groups.len() || into >= groups.len() {
        return;
    }
    let (first, rest) = groups.remove(from);
    let into = if from < into { into - 1 } else { into };
    if let Some((_, merged)) = groups.get_mut(into) {
        merged.push(first);
        merged.extend(rest);
    }
}

fn pair_pads(names: &Names<'_>, groups: &mut Vec<Entry>) {
    loop {
        let mut pair = None;
        'search: for (i, (pad, extra)) in groups.iter().enumerate() {
            if pad.code != "footprint.terminal_required" || !extra.is_empty() {
                continue;
            }
            let Some((footprint, pad_id)) = local(pad).next() else {
                continue;
            };
            let Some(Definition::Footprint(body)) = names.body(footprint) else {
                continue;
            };
            let Some(number) = body
                .pads
                .iter()
                .find(|p| p.id.0 == pad_id)
                .map(|p| p.number.trim().to_owned())
                .filter(|n| !n.is_empty())
            else {
                continue;
            };
            let package = body.package.id;
            let Some(Definition::Package(package_body)) = names.body(package) else {
                continue;
            };
            for (j, (terminal, more)) in groups.iter().enumerate() {
                if terminal.code != "footprint.terminal_without_pad"
                    || !more.is_empty()
                    || !definitions(terminal).any(|d| d == footprint)
                {
                    continue;
                }
                let designated = local(terminal).any(|(definition, id)| {
                    definition == package
                        && package_body
                            .terminal(PackageTerminalId(id))
                            .is_some_and(|t| t.designation.trim() == number)
                });
                if designated {
                    pair = Some((i, j));
                    break 'search;
                }
            }
        }
        match pair {
            Some((i, j)) => absorb(groups, i, j),
            None => return,
        }
    }
}

fn pair_mappings(names: &Names<'_>, groups: &mut Vec<Entry>) {
    // Per binding lineage: the unmapped electrical terminals and the
    // package terminals no mapping carries.
    let mut electrical: BTreeMap<DefinitionId, Vec<usize>> = BTreeMap::new();
    let mut package: BTreeMap<DefinitionId, Vec<usize>> = BTreeMap::new();
    for (i, (finding, extra)) in groups.iter().enumerate() {
        if !extra.is_empty() {
            continue;
        }
        match finding.code.as_str() {
            "component.mapping_missing" => {
                for binding in definitions(finding) {
                    electrical.entry(binding).or_default().push(i);
                }
            }
            "binding.mapping_missing" => {
                let Some(binding) = definitions(finding).next() else {
                    continue;
                };
                let Some(Definition::Binding(body)) = names.body(binding) else {
                    continue;
                };
                let unmapped = local(finding).any(|(definition, id)| {
                    definition == body.package.id
                        && !body
                            .mappings
                            .iter()
                            .any(|m| m.package_terminal == PackageTerminalId(id))
                });
                if unmapped {
                    package.entry(binding).or_default().push(i);
                }
            }
            _ => {}
        }
    }
    let mut pairs: Vec<(usize, usize)> = electrical
        .iter()
        .filter_map(
            |(binding, e)| match (e.as_slice(), package.get(binding)?.as_slice()) {
                ([one], [other]) => Some((*one, *other)),
                _ => None,
            },
        )
        .collect();
    // Absorb from the back so earlier indices stay valid.
    pairs.sort_by_key(|(_, from)| std::cmp::Reverse(*from));
    for (into, from) in pairs {
        absorb(groups, into, from);
    }
}

/// Schematic findings: one Part shared by several instances that disagrees
/// with each of them the same way is one row; ERC and completeness
/// findings stay per object.
pub(super) fn schematic(findings: Vec<Finding>) -> Vec<Vec<Finding>> {
    by_key(findings, |f| {
        let shared = f.code.starts_with("part.").then(|| {
            let part = definitions(f).next()?;
            let (_, rest) = f.message.split_once(": ")?;
            Some((part.0, rest.to_owned()))
        });
        Some(match shared.flatten() {
            Some((part, rest)) => (f.code.clone(), part, rest),
            None => (f.code.clone(), 0, same(f)),
        })
    })
}

/// The identity of a finding for exact-duplicate merging.
fn same(f: &Finding) -> String {
    format!(
        "{}\u{0}{}",
        f.message,
        serde_json::to_string(&f.objects).unwrap_or_default()
    )
}

/// Board findings: every clearance violation between the same two parties
/// (placements or tracks) is one row, and every unrouted pad of one net is
/// one row.
pub(super) fn board(findings: Vec<Finding>) -> Vec<Vec<Finding>> {
    by_key(findings, |f| {
        // A party is (0, placement) or (1, track).
        let mut parties: Vec<(u8, Id)> = f
            .objects
            .iter()
            .filter_map(|o| match *o {
                ObjectRef::Pad { instance, .. } => Some((0, instance)),
                ObjectRef::Track { id } => Some((1, id)),
                _ => None,
            })
            .collect();
        parties.sort_unstable();
        let net = f.objects.iter().find_map(|o| match *o {
            ObjectRef::Net { id } => Some(id),
            _ => None,
        });
        Some(match (f.code.as_str(), parties.as_slice(), net) {
            ("board.clearance", [a, b], _) => {
                (f.code.clone(), format!("{} {} {} {}", a.0, a.1, b.0, b.1))
            }
            ("board.unrouted", _, Some(net)) => (f.code.clone(), net.to_string()),
            _ => (f.code.clone(), same(f)),
        })
    })
}
