//! Purchasing completeness, the BOM and Resolve-parts groups (S2). All
//! derived from the instances, their Parts and `fields::satisfies`; nothing
//! here is stored.
use crate::diagnostics::{Finding, Result};
use crate::fields::{self, FieldValue, Requirement, ValueType};
use crate::ids::{ComponentRef, DefinitionId, FootprintRef, Id, PartRef};
use crate::instances;
use crate::library::Library;
use crate::model::{ComponentInstance, PhysicalChoice, Project};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Whether an instance names an exact orderable part that agrees with it.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PurchasingState {
    /// Generic: no Part is chosen yet.
    NoPart,
    /// A Part is chosen but a finding remains (component or binding
    /// mismatch, an unmet requirement, a stale Part revision).
    Unresolved,
    /// A Part is chosen and nothing disagrees with it.
    Resolved,
}

/// The purchasing part of `component.inspect`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct PurchasingReport {
    pub state: PurchasingState,
    pub part: Option<PartRef>,
    pub manufacturer: Option<String>,
    pub mpn: Option<String>,
    /// Why the state is not `resolved`; advisory, they block no stage.
    pub findings: Vec<Finding>,
}

/// Purchasing completeness of one instance.
pub fn purchasing(project: &Project, instance: &ComponentInstance) -> PurchasingReport {
    let Some(r) = instance.part else {
        return PurchasingReport {
            state: PurchasingState::NoPart,
            part: None,
            manufacturer: None,
            mpn: None,
            findings: Vec::new(),
        };
    };
    let part = project.library.part(r);
    let findings = instances::part_findings(&project.library, instance);
    PurchasingReport {
        state: if findings.is_empty() {
            PurchasingState::Resolved
        } else {
            PurchasingState::Unresolved
        },
        part: Some(r),
        manufacturer: part.map(|p| p.definition.manufacturer.clone()),
        mpn: part.map(|p| p.definition.mpn.clone()),
        findings,
    }
}

/// One effective value printed on a BOM line.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct BomValue {
    pub field: DefinitionId,
    pub label: String,
    pub text: String,
}

/// One BOM line: instances that buy the same thing. Resolved instances
/// group by Part; unresolved ones by component revision, footprint and
/// identical requirements, exactly as Resolve parts groups them.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct BomLine {
    pub component: ComponentRef,
    pub footprint: Option<FootprintRef>,
    pub part: Option<PartRef>,
    pub manufacturer: Option<String>,
    pub mpn: Option<String>,
    pub values: Vec<BomValue>,
    /// In designator order.
    pub designators: Vec<String>,
    pub instances: Vec<Id>,
    pub quantity: u32,
    /// The line has no Part, or its Part has a purchasing finding.
    pub unresolved: bool,
    pub state: PurchasingState,
}

/// `project.bom`.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct BomReport {
    pub lines: Vec<BomLine>,
    /// Lines with `unresolved`.
    pub unresolved: usize,
}

/// The project BOM, lines in designator order of their first instance.
pub fn bom(project: &Project) -> Result<BomReport> {
    let rows = rows(project)?;
    let mut lines: Vec<(String, Vec<&Row<'_>>)> = Vec::new();
    let mut index: BTreeMap<String, usize> = BTreeMap::new();
    for row in &rows {
        let key = match row.report.state {
            PurchasingState::Resolved => {
                format!("part {:?} {:?}", row.instance.part, row.footprint)
            }
            _ => format!(
                "generic {:?} {:?} {:?} {}",
                row.instance.component, row.footprint, row.instance.part, row.key
            ),
        };
        group(&mut lines, &mut index, key, row);
    }
    let lines: Vec<BomLine> = lines
        .into_iter()
        .filter_map(|(_, members)| {
            let first = members.first()?;
            let state = members
                .iter()
                .map(|m| m.report.state)
                .find(|s| *s != PurchasingState::Resolved)
                .unwrap_or(PurchasingState::Resolved);
            Some(BomLine {
                component: first.instance.component,
                footprint: first.footprint,
                part: first.instance.part,
                manufacturer: first.report.manufacturer.clone(),
                mpn: first.report.mpn.clone(),
                values: values(&project.library, &first.requirements),
                designators: members
                    .iter()
                    .map(|m| m.instance.designator.clone())
                    .collect(),
                instances: members.iter().map(|m| m.instance.id).collect(),
                quantity: u32::try_from(members.len()).unwrap_or(u32::MAX),
                unresolved: state != PurchasingState::Resolved,
                state,
            })
        })
        .collect();
    Ok(BomReport {
        unresolved: lines.iter().filter(|l| l.unresolved).count(),
        lines,
    })
}

/// One instance with what grouping needs.
struct Row<'a> {
    instance: &'a ComponentInstance,
    report: PurchasingReport,
    requirements: Vec<Requirement>,
    footprint: Option<FootprintRef>,
    /// The requirements with presentation and the Part-derived nominal
    /// removed, so equal values group together.
    key: String,
}

/// Every instance in designator order (prefix, then number, then text).
fn rows(project: &Project) -> Result<Vec<Row<'_>>> {
    let mut out = Vec::with_capacity(project.instances.len());
    for instance in &project.instances {
        let resolved = instances::resolve(&project.library, instance)?;
        let requirements = instances::requirements(&project.library, instance)?;
        let normal: Vec<Requirement> = requirements
            .iter()
            .cloned()
            .map(|mut r| {
                if let FieldValue::Quantity { display_unit, .. } = &mut r.value {
                    *display_unit = None;
                }
                r.label.clear();
                // Derived from the pinned Part's tolerance basis, not a
                // requirement of the instance.
                r.nominal = None;
                r
            })
            .collect();
        out.push(Row {
            instance,
            report: purchasing(project, instance),
            footprint: resolved
                .physical
                .map(|p| FootprintRef::new(p.footprint.id, p.footprint.revision)),
            key: format!("{normal:?}"),
            requirements,
        });
    }
    out.sort_by(|a, b| {
        designator_key(&a.instance.designator)
            .cmp(&designator_key(&b.instance.designator))
            .then_with(|| a.instance.id.cmp(&b.instance.id))
    });
    Ok(out)
}

/// `R10` after `R9`: the letter prefix, then the number, then the text.
fn designator_key(designator: &str) -> (String, u64, String) {
    let digits = designator
        .char_indices()
        .rev()
        .take_while(|(_, c)| c.is_ascii_digit())
        .last()
        .map_or(designator.len(), |(i, _)| i);
    let (prefix, number) = designator.split_at(digits);
    (
        prefix.to_owned(),
        number.parse().unwrap_or(u64::MAX),
        designator.to_owned(),
    )
}

fn group<'a, 'b>(
    groups: &mut Vec<(String, Vec<&'b Row<'a>>)>,
    index: &mut BTreeMap<String, usize>,
    key: String,
    row: &'b Row<'a>,
) {
    match index.get(&key) {
        Some(&i) => {
            if let Some((_, members)) = groups.get_mut(i) {
                members.push(row);
            }
        }
        None => {
            index.insert(key.clone(), groups.len());
            groups.push((key, vec![row]));
        }
    }
}

/// The value type and quantity kind of a requirement, for formatting.
fn display(library: &Library, requirement: &Requirement) -> String {
    let kind = requirement
        .value_type
        .kind()
        .and_then(|k| library.quantity_kind(k))
        .map(|k| &k.definition);
    fields::display(&requirement.value, &requirement.value_type, kind)
}

fn values(library: &Library, requirements: &[Requirement]) -> Vec<BomValue> {
    requirements
        .iter()
        .map(|r| BomValue {
            field: r.field.id,
            label: r.label.clone(),
            text: display(library, r),
        })
        .collect()
}

/// Unresolved instances with identical requirements and footprint, which one
/// `instance.assign_part` can resolve together.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ResolveGroup {
    pub component: ComponentRef,
    pub physical: Option<PhysicalChoice>,
    pub footprint: Option<FootprintRef>,
    /// Shared by every instance of the group.
    pub requirements: Vec<Requirement>,
    /// In designator order.
    pub instances: Vec<Id>,
    pub designators: Vec<String>,
    /// A `library.search` query that finds Parts for the group: `kind:part`
    /// plus the requirements as field predicates. Pre-fills the Place
    /// palette's Choose….
    pub query: String,
}

/// `library.resolve_groups`.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ResolveReport {
    pub groups: Vec<ResolveGroup>,
}

/// Resolve-parts groups, in designator order of their first instance:
/// instances without a Part, or whose Part has a purchasing finding,
/// grouped by component revision, physical choice and identical
/// requirements.
pub fn resolve_groups(project: &Project) -> Result<ResolveReport> {
    let rows = rows(project)?;
    let mut groups: Vec<(String, Vec<&Row<'_>>)> = Vec::new();
    let mut index: BTreeMap<String, usize> = BTreeMap::new();
    for row in rows
        .iter()
        .filter(|r| r.report.state != PurchasingState::Resolved)
    {
        let key = format!(
            "{:?} {:?} {}",
            row.instance.component, row.instance.physical, row.key
        );
        group(&mut groups, &mut index, key, row);
    }
    let groups = groups
        .into_iter()
        .filter_map(|(_, members)| {
            let first = members.first()?;
            Some(ResolveGroup {
                component: first.instance.component,
                physical: first.instance.physical,
                footprint: first.footprint,
                query: requirement_query(&project.library, &first.requirements),
                requirements: first.requirements.clone(),
                instances: members.iter().map(|m| m.instance.id).collect(),
                designators: members
                    .iter()
                    .map(|m| m.instance.designator.clone())
                    .collect(),
            })
        })
        .collect();
    Ok(ResolveReport { groups })
}

/// Reserved catalog filter keys. They mirror `catalog::RESERVED`, which this
/// module may not name; a catalog test keeps the two equal.
const RESERVED_KEYS: &[&str] = &[
    "pkg", "has", "missing", "src", "kind", "tag", "shelf", "mpn", "life", "pins", "pitch", "body",
];
/// Terms and bytes of one catalog query (`catalog::MAX_TERMS`, `MAX_QUERY`).
const QUERY_TERMS: usize = 16;
const QUERY_BYTES: usize = 512;

/// Case folding as the catalog query applies it.
fn fold(text: &str) -> String {
    text.to_lowercase().replace('\u{b5}', "\u{3bc}")
}

/// A `library.search` query for Parts meeting `requirements`: `kind:part`
/// and one `key:value` predicate per requirement whose field the query can
/// name unambiguously and whose value it can write as one token. On a Part,
/// `key:value` means the rated value meets the value (catalog search reads
/// it through the spec's meaning). A requirement that cannot be written is
/// left out, so the query may find more Parts than satisfy; assignment
/// still checks every requirement.
pub fn requirement_query(library: &Library, requirements: &[Requirement]) -> String {
    let mut query = String::from("kind:part");
    let mut terms = 1;
    for requirement in requirements {
        let Some(predicate) = predicate(library, requirement) else {
            continue;
        };
        if terms >= QUERY_TERMS || query.len() + 1 + predicate.len() > QUERY_BYTES {
            break;
        }
        query.push(' ');
        query.push_str(&predicate);
        terms += 1;
    }
    query
}

/// A field name the query resolves to exactly this field lineage: not
/// reserved, not a unit of any field's quantity kind, then an exact name or
/// a unique name prefix, as the catalog's key rules read it.
fn key(library: &Library, field: DefinitionId) -> Option<String> {
    let latest: BTreeMap<DefinitionId, &str> = library
        .fields
        .iter()
        .filter(|f| {
            library.latest(f.id).is_some_and(|(_, r)| r == f.revision)
                && f.definition
                    .value_type
                    .kind()
                    .is_none_or(|k| library.quantity_kind(k).is_some())
        })
        .map(|f| (f.id, f.meta.name.as_str()))
        .collect();
    let name = latest.get(&field)?.trim();
    let mut candidates = vec![name];
    candidates.extend(name.split_whitespace().next().filter(|w| *w != name));
    candidates.into_iter().find_map(|key| {
        let safe = !key.is_empty()
            && !key
                .chars()
                .any(|c| c.is_whitespace() || c.is_control() || "<>=:\"".contains(c));
        let folded = fold(key);
        let unit = library.fields.iter().any(|f| {
            latest.contains_key(&f.id)
                && f.definition
                    .value_type
                    .kind()
                    .and_then(|k| library.quantity_kind(k))
                    .is_some_and(|k| k.definition.units.resolve(key).is_some())
        });
        if !safe || unit || RESERVED_KEYS.contains(&folded.as_str()) {
            return None;
        }
        let named = |exact: bool| -> Vec<DefinitionId> {
            latest
                .iter()
                .filter(|(_, n)| {
                    let n = fold(n.trim());
                    if exact {
                        n == folded
                    } else {
                        n.starts_with(&folded)
                    }
                })
                .map(|(id, _)| *id)
                .collect()
        };
        let exact = named(true);
        let found = if exact.is_empty() {
            named(false)
        } else {
            exact
        };
        (found == [field]).then(|| key.to_owned())
    })
}

fn predicate(library: &Library, requirement: &Requirement) -> Option<String> {
    let key = key(library, requirement.field.id)?;
    let value = match (&requirement.value_type, &requirement.value) {
        (ValueType::Enum { .. } | ValueType::Text { .. }, _) => display(library, requirement),
        (_, FieldValue::Integer(v)) => v.to_string(),
        (_, FieldValue::Boolean(v)) => v.to_string(),
        _ => display(library, requirement)
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect(),
    };
    let writable = !value.is_empty()
        && !value
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || c == '"');
    writable.then(|| format!("{key}:{value}"))
}

#[cfg(test)]
mod tests {
    #[test]
    fn reserved_keys_mirror_the_catalog() {
        assert_eq!(super::RESERVED_KEYS, crate::catalog::RESERVED);
        assert_eq!(super::QUERY_TERMS, crate::catalog::MAX_TERMS);
        assert_eq!(super::QUERY_BYTES, crate::catalog::MAX_QUERY);
    }
}
