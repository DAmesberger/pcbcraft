//! Net classes: named copper rules for groups of nets, assigned explicitly
//! by net or by an explicit name pattern. A class never follows from a
//! net's electrical behaviour or matching names elsewhere; only these
//! entries assign one.
use super::{
    ClearanceSide, CopperObject, MAX_PROFILE_TEXT, MAX_RULE_MM, PadConnection, ProjectRules,
    Resolved, RuleKey, RuleSource, RulesCommand,
};
use crate::diagnostics::{Error, Result};
use crate::ids::Id;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Most net classes in one project.
pub const MAX_NET_CLASSES: usize = 128;
/// Most explicit assignments and name patterns.
pub const MAX_ASSIGNMENTS: usize = 65_536;
pub const MAX_PATTERNS: usize = 256;

/// Project-local class identity; never reused.
#[derive(
    Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
#[serde(transparent)]
pub struct NetClassId(pub u32);

pub(super) fn first_id() -> u32 {
    1
}

/// A class's values; `None` falls back to the effective rules.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct NetClassSpec {
    /// Unique (case-insensitively) and non-blank.
    pub name: String,
    /// The Route tool's width for this class's nets.
    #[serde(default)]
    pub track_width: Option<f64>,
    /// The narrowest track routing accepts on this class's nets (at
    /// least the effective rule minimum, which also applies when unset).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track_width_min: Option<f64>,
    /// Least gap from this class's copper to any other net's copper.
    #[serde(default)]
    pub clearance: Option<f64>,
    #[serde(default)]
    pub via_drill: Option<f64>,
    #[serde(default)]
    pub via_diameter: Option<f64>,
    /// Reserved for differential pairs (stored, not yet used).
    #[serde(default)]
    pub diff_pair_width: Option<f64>,
    /// Reserved for differential pairs (stored, not yet used).
    #[serde(default)]
    pub diff_pair_gap: Option<f64>,
    /// How fills of this class's nets join its pads.
    #[serde(default)]
    pub fill_connection: Option<PadConnection>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct NetClass {
    pub id: NetClassId,
    pub name: String,
    #[serde(default)]
    pub track_width: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track_width_min: Option<f64>,
    #[serde(default)]
    pub clearance: Option<f64>,
    #[serde(default)]
    pub via_drill: Option<f64>,
    #[serde(default)]
    pub via_diameter: Option<f64>,
    #[serde(default)]
    pub diff_pair_width: Option<f64>,
    #[serde(default)]
    pub diff_pair_gap: Option<f64>,
    #[serde(default)]
    pub fill_connection: Option<PadConnection>,
}

impl NetClass {
    pub fn from_spec(id: NetClassId, s: NetClassSpec) -> Self {
        Self {
            id,
            name: s.name,
            track_width: s.track_width,
            track_width_min: s.track_width_min,
            clearance: s.clearance,
            via_drill: s.via_drill,
            via_diameter: s.via_diameter,
            diff_pair_width: s.diff_pair_width,
            diff_pair_gap: s.diff_pair_gap,
            fill_connection: s.fill_connection,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct NetClassAssignment {
    pub net: Id,
    pub class: NetClassId,
}

/// Nets whose name matches `pattern` (`*` any run, `?` one character,
/// case-sensitive) join `class`, unless explicitly assigned.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct NetClassPattern {
    pub pattern: String,
    pub class: NetClassId,
}

/// A class-to-class clearance, unordered (`a <= b`).
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ClassClearance {
    pub a: NetClassId,
    pub b: NetClassId,
    pub mm: f64,
}

impl ClassClearance {
    pub fn is(&self, a: NetClassId, b: NetClassId) -> bool {
        (self.a == a && self.b == b) || (self.a == b && self.b == a)
    }
}

/// `*`/`?` glob match over characters.
pub fn glob_matches(pattern: &str, name: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let n: Vec<char> = name.chars().collect();
    let (mut pi, mut ni) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while ni < n.len() {
        match p.get(pi) {
            Some('*') => {
                star = Some((pi, ni));
                pi += 1;
            }
            Some(c) if *c == '?' || *c == n[ni] => {
                pi += 1;
                ni += 1;
            }
            _ => match star {
                Some((sp, sn)) => {
                    pi = sp + 1;
                    ni = sn + 1;
                    star = Some((sp, sn + 1));
                }
                None => return false,
            },
        }
    }
    p[pi..].iter().all(|c| *c == '*')
}

fn mm_ok(v: Option<f64>, positive: bool) -> bool {
    v.is_none_or(|mm| {
        mm.is_finite() && mm <= MAX_RULE_MM && if positive { mm > 0.0 } else { mm >= 0.0 }
    })
}

/// Why a class's own values are invalid, or `None`.
fn class_problem(c: &NetClass) -> Option<String> {
    let name = &c.name;
    if c.name.trim().is_empty()
        || c.name.len() > MAX_PROFILE_TEXT
        || c.name.chars().any(char::is_control)
    {
        return Some("net_class.name: a net class needs a short, non-blank name".into());
    }
    if !(mm_ok(c.track_width, true)
        && mm_ok(c.track_width_min, true)
        && mm_ok(c.clearance, false)
        && mm_ok(c.via_drill, true)
        && mm_ok(c.via_diameter, true)
        && mm_ok(c.diff_pair_width, true)
        && mm_ok(c.diff_pair_gap, false))
    {
        return Some(format!(
            "net_class.bounds: net class {name} needs values of 0–{MAX_RULE_MM} mm"
        ));
    }
    if let (Some(width), Some(min)) = (c.track_width, c.track_width_min)
        && width < min
    {
        return Some(format!(
            "net_class.width: net class {name} width is below its minimum width"
        ));
    }
    if let (Some(drill), Some(pad)) = (c.via_drill, c.via_diameter)
        && pad <= drill
    {
        return Some(format!(
            "net_class.via: net class {name} via pad must be wider than its drill"
        ));
    }
    None
}

pub(super) fn validate(rules: &ProjectRules) -> Result<()> {
    if rules.net_classes.len() > MAX_NET_CLASSES
        || rules.assignments.len() > MAX_ASSIGNMENTS
        || rules.patterns.len() > MAX_PATTERNS
        || rules.class_clearances.len() > MAX_NET_CLASSES * MAX_NET_CLASSES
    {
        return Err(Error::Limit(
            "net_class.limit: too many net class entries".into(),
        ));
    }
    let mut ids = BTreeSet::new();
    let mut names = BTreeSet::new();
    for c in &rules.net_classes {
        if let Some(problem) = class_problem(c) {
            return Err(Error::Invalid(problem));
        }
        if c.id.0 >= rules.next_class || !ids.insert(c.id) || !names.insert(c.name.to_lowercase()) {
            return Err(Error::Conflict(format!(
                "net_class.duplicate: a net class named {} already exists",
                c.name
            )));
        }
    }
    let known = |c: NetClassId| ids.contains(&c);
    let mut nets = BTreeSet::new();
    for a in &rules.assignments {
        if !known(a.class) || !nets.insert(a.net) {
            return Err(Error::Invalid(
                "net_class.assignment: each net has at most one explicit class, which exists"
                    .into(),
            ));
        }
    }
    for p in &rules.patterns {
        if !known(p.class)
            || p.pattern.trim().is_empty()
            || p.pattern.len() > MAX_PROFILE_TEXT
            || p.pattern.chars().any(char::is_control)
        {
            return Err(Error::Invalid(
                "net_class.pattern: a pattern is non-blank, bounded and names an existing class"
                    .into(),
            ));
        }
    }
    let mut pairs = BTreeSet::new();
    for p in &rules.class_clearances {
        if !known(p.a)
            || !known(p.b)
            || p.a > p.b
            || !mm_ok(Some(p.mm), false)
            || !pairs.insert((p.a, p.b))
        {
            return Err(Error::Invalid(
                "net_class.pair: class pairs are unique, ordered and within bounds".into(),
            ));
        }
    }
    Ok(())
}

pub(super) fn apply(rules: &mut ProjectRules, command: RulesCommand) -> Result<Option<NetClassId>> {
    let exists = |rules: &ProjectRules, id: NetClassId| -> Result<()> {
        rules.class(id).map(|_| ()).ok_or_else(|| {
            Error::NotFound("net_class.missing: that net class no longer exists".into())
        })
    };
    match command {
        RulesCommand::AddNetClass { class } => {
            let id = NetClassId(rules.next_class);
            rules.next_class = rules
                .next_class
                .checked_add(1)
                .ok_or_else(|| Error::Limit("net class identity space exhausted".into()))?;
            rules.net_classes.push(NetClass::from_spec(id, class));
            return Ok(Some(id));
        }
        RulesCommand::EditNetClass { class } => {
            exists(rules, class.id)?;
            if let Some(slot) = rules.net_classes.iter_mut().find(|c| c.id == class.id) {
                *slot = class;
            }
        }
        RulesCommand::DeleteNetClass { class } => {
            exists(rules, class)?;
            rules.net_classes.retain(|c| c.id != class);
            rules.assignments.retain(|a| a.class != class);
            rules.patterns.retain(|p| p.class != class);
            rules
                .class_clearances
                .retain(|p| p.a != class && p.b != class);
        }
        RulesCommand::AssignNets { nets, class } => {
            if nets.is_empty() {
                return Err(Error::Invalid(
                    "net_class.assign_empty: choose at least one net to assign".into(),
                ));
            }
            if let Some(c) = class {
                exists(rules, c)?;
            }
            rules.assignments.retain(|a| !nets.contains(&a.net));
            if let Some(class) = class {
                let mut nets = nets;
                nets.sort_unstable();
                nets.dedup();
                rules.assignments.extend(
                    nets.into_iter()
                        .map(|net| NetClassAssignment { net, class }),
                );
                rules.assignments.sort_by_key(|a| a.net);
            }
        }
        RulesCommand::SetPatterns { patterns } => rules.patterns = patterns,
        RulesCommand::SetClassClearance { a, b, mm } => {
            exists(rules, a)?;
            exists(rules, b)?;
            let (a, b) = if a <= b { (a, b) } else { (b, a) };
            rules.class_clearances.retain(|p| !p.is(a, b));
            if let Some(mm) = mm {
                rules.class_clearances.push(ClassClearance { a, b, mm });
                rules.class_clearances.sort_by_key(|p| (p.a, p.b));
            }
        }
        RulesCommand::SetProfile { .. }
        | RulesCommand::SetOverride { .. }
        | RulesCommand::ClearOverride { .. } => {
            return Err(Error::Invalid("not a net class command".into()));
        }
    }
    Ok(None)
}

impl ProjectRules {
    /// The narrowest track routing accepts on a net of `class`: the larger
    /// of the effective rule minimum and the class's own minimum.
    pub fn track_width_min(&self, class: Option<NetClassId>) -> Resolved<f64> {
        let rule = Resolved {
            value: self.effective().copper.track_width_min,
            source: self.source_of(RuleKey::TrackWidthMin),
        };
        match class.and_then(|c| self.class(c)) {
            Some(c) if c.track_width_min.is_some_and(|min| min > rule.value) => Resolved {
                value: c.track_width_min.unwrap_or(rule.value),
                source: RuleSource::NetClass {
                    class: c.name.clone(),
                },
            },
            _ => rule,
        }
    }

    /// How `net` (named `net_name`) gets its class, if it has one.
    pub fn class_basis(&self, net: Id, net_name: &str) -> Option<(NetClassId, ClassBasis)> {
        if let Some(a) = self.assignments.iter().find(|a| a.net == net) {
            return Some((a.class, ClassBasis::Explicit));
        }
        self.patterns
            .iter()
            .find(|p| glob_matches(&p.pattern, net_name))
            .map(|p| {
                (
                    p.class,
                    ClassBasis::Pattern {
                        pattern: p.pattern.clone(),
                    },
                )
            })
    }
}

/// How a net got its class: an explicit assignment or the first matching
/// name pattern. Nothing else assigns a class.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case", tag = "basis")]
pub enum ClassBasis {
    Explicit,
    Pattern { pattern: String },
}

/// One net's class in `net_class.list`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct NetClassMember {
    pub net: Id,
    pub name: String,
    /// `None`: the net follows the effective rules only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub class: Option<NetClassId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub basis: Option<ClassBasis>,
}

/// One class in `net_class.list` with the values routing and checks use
/// for its nets and where each comes from.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct NetClassRow {
    pub class: NetClass,
    /// Nets of this class (explicit and by pattern).
    pub members: usize,
    pub track_width: Resolved<f64>,
    pub track_width_min: Resolved<f64>,
    /// Via (drill, diameter).
    pub via: Resolved<(f64, f64)>,
    /// Track-to-track clearance to an unclassed net.
    pub clearance: Resolved<f64>,
}

/// `net_class.list`: every class with its resolved values, every net with
/// its class and basis, the patterns and the class-pair clearances.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct NetClassReport {
    pub classes: Vec<NetClassRow>,
    pub nets: Vec<NetClassMember>,
    pub patterns: Vec<NetClassPattern>,
    pub class_clearances: Vec<ClassClearance>,
}

/// The class of each of `nets` (id, name) under `rules`.
pub fn members<'a>(
    rules: &ProjectRules,
    nets: impl IntoIterator<Item = (Id, &'a str)>,
) -> Vec<NetClassMember> {
    nets.into_iter()
        .map(|(net, name)| {
            let basis = rules.class_basis(net, name);
            NetClassMember {
                net,
                name: name.to_owned(),
                class: basis.as_ref().map(|(c, _)| *c),
                basis: basis.map(|(_, b)| b),
            }
        })
        .collect()
}

pub fn report<'a>(
    rules: &ProjectRules,
    nets: impl IntoIterator<Item = (Id, &'a str)>,
) -> NetClassReport {
    let nets = members(rules, nets);
    let mut counts: BTreeMap<NetClassId, usize> = BTreeMap::new();
    for class in nets.iter().filter_map(|m| m.class) {
        *counts.entry(class).or_default() += 1;
    }
    let side = |class| ClearanceSide {
        object: CopperObject::Track,
        class,
    };
    let classes = rules
        .net_classes
        .iter()
        .map(|c| NetClassRow {
            class: c.clone(),
            members: counts.get(&c.id).copied().unwrap_or(0),
            track_width: rules.track_width(Some(c.id)),
            track_width_min: rules.track_width_min(Some(c.id)),
            via: rules.via_size(Some(c.id)),
            clearance: rules.clearance(side(Some(c.id)), side(None)),
        })
        .collect();
    NetClassReport {
        classes,
        nets,
        patterns: rules.patterns.clone(),
        class_clearances: rules.class_clearances.clone(),
    }
}

/// One net whose class a pattern change would change.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ClassChange {
    pub net: Id,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<NetClassId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<NetClassId>,
}

/// `net_class.preview_patterns`: the nets whose class `patterns` would
/// change, evaluated explicitly over today's nets. Refused like
/// `net_class.set_patterns` when a pattern is invalid.
pub fn preview_patterns<'a>(
    rules: &ProjectRules,
    patterns: Vec<NetClassPattern>,
    nets: impl IntoIterator<Item = (Id, &'a str)> + Clone,
) -> Result<Vec<ClassChange>> {
    let mut candidate = rules.clone();
    super::apply(&mut candidate, RulesCommand::SetPatterns { patterns })?;
    Ok(nets
        .into_iter()
        .filter_map(|(net, name)| {
            let from = rules.class_of(net, name);
            let to = candidate.class_of(net, name);
            (from != to).then(|| ClassChange {
                net,
                name: name.to_owned(),
                from,
                to,
            })
        })
        .collect())
}

/// Let explicit assignments follow a net rebuild. A merged-away net's
/// class passes to the surviving net when the survivor has no explicit
/// class of its own and every merged-away net with one names the same
/// class (conflicting classes pass none). A net newly created by a split
/// takes the explicit class of the net it split from. Assignments of nets
/// that no longer exist (`live`) go. Patterns need no follow-up: they
/// match the surviving names.
pub fn follow_nets(
    rules: &mut ProjectRules,
    merged: &[(Id, Id)],
    split: &[(Id, Id)],
    created: &[Id],
    live: &BTreeSet<Id>,
) {
    if rules.assignments.is_empty() {
        return;
    }
    let explicit: BTreeMap<Id, NetClassId> =
        rules.assignments.iter().map(|a| (a.net, a.class)).collect();
    // survivor -> candidate classes from merged-away nets
    let mut inherited: BTreeMap<Id, BTreeSet<NetClassId>> = BTreeMap::new();
    for (gone, survivor) in merged {
        if let Some(class) = explicit.get(gone) {
            inherited.entry(*survivor).or_default().insert(*class);
        }
    }
    let created: BTreeSet<Id> = created.iter().copied().collect();
    for (original, piece) in split {
        if created.contains(piece)
            && let Some(class) = explicit.get(original)
        {
            inherited.entry(*piece).or_default().insert(*class);
        }
    }
    let mut next: BTreeMap<Id, NetClassId> = explicit
        .into_iter()
        .filter(|(net, _)| live.contains(net))
        .collect();
    for (net, classes) in inherited {
        if live.contains(&net)
            && !next.contains_key(&net)
            && classes.len() == 1
            && let Some(class) = classes.first()
        {
            next.insert(net, *class);
        }
    }
    rules.assignments = next
        .into_iter()
        .map(|(net, class)| NetClassAssignment { net, class })
        .collect();
}

#[cfg(test)]
mod tests;
