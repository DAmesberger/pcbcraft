//! Net classes: named copper rules for groups of nets, assigned explicitly
//! by net or by an explicit name pattern. A class never follows from a
//! net's electrical behaviour or matching names elsewhere; only these
//! entries assign one.
use super::{MAX_PROFILE_TEXT, MAX_RULE_MM, PadConnection, ProjectRules, RulesCommand};
use crate::diagnostics::{Error, Result};
use crate::ids::Id;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

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
    #[serde(default)]
    pub track_width: Option<f64>,
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

fn class_ok(c: &NetClass) -> bool {
    !c.name.trim().is_empty()
        && c.name.len() <= MAX_PROFILE_TEXT
        && !c.name.chars().any(char::is_control)
        && mm_ok(c.track_width, true)
        && mm_ok(c.clearance, false)
        && mm_ok(c.via_drill, true)
        && mm_ok(c.via_diameter, true)
        && mm_ok(c.diff_pair_width, true)
        && mm_ok(c.diff_pair_gap, false)
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
        if !class_ok(c) {
            return Err(Error::Invalid(format!(
                "net_class.bounds: net class “{}” needs a name and values of 0–{MAX_RULE_MM} mm",
                c.name
            )));
        }
        if c.id.0 >= rules.next_class || !ids.insert(c.id) || !names.insert(c.name.to_lowercase()) {
            return Err(Error::Invalid(format!(
                "net_class.duplicate: net class “{}” is not unique",
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
        rules
            .class(id)
            .map(|_| ())
            .ok_or_else(|| Error::NotFound(format!("net_class.missing: no net class {}", id.0)))
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
