//! High-speed signal rules as data: the controlled-impedance targets and
//! pair limits a net class carries ([`SignalRules`]), differential pairs
//! (explicit positive and negative nets, [`DiffPair`]) and length-matching
//! groups ([`LengthGroup`]). Bounds, identities and the commands that edit
//! them live here; the impedance model, pair geometry, length measurement
//! and checks read them from `layout` (docs/high-speed-routing.md).
//!
//! A pair is never inferred: [`suggest`] proposes pairs from net names
//! (`USB_D+`/`USB_D-`, `_P`/`_N`, `DP`/`DM`), and only `diff_pair.add`
//! makes one.
use super::{MAX_PROFILE_TEXT, ProjectRules, RulesCommand};
use crate::diagnostics::{Error, Result};
use crate::ids::{Endpoint, Id};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Most differential pairs in one project.
pub const MAX_PAIRS: usize = 1024;
/// Most length groups in one project.
pub const MAX_GROUPS: usize = 256;
/// Most nets and pairs one length group lists.
pub const MAX_GROUP_MEMBERS: usize = 256;
/// Single-ended impedance targets accepted, Ω.
pub const IMPEDANCE_RANGE: std::ops::RangeInclusive<f64> = 10.0..=200.0;
/// Differential impedance targets accepted, Ω.
pub const DIFF_IMPEDANCE_RANGE: std::ops::RangeInclusive<f64> = 20.0..=300.0;
/// Impedance tolerance accepted, percent of the target.
pub const TOLERANCE_RANGE: std::ops::RangeInclusive<f64> = 0.5..=50.0;
/// Longest target length and largest length tolerance, mm.
pub const MAX_LENGTH_MM: f64 = 10_000.0;

/// What the checks take from the host rather than the document: values
/// the user sets in Settings (`sparksmith_io::settings`) for limits a
/// project leaves open. The core holds no default; an unset value means
/// the host has none, and the check says so.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CheckSettings {
    /// Impedance tolerance, percent, for a net class that states none.
    pub impedance_tolerance: Option<f64>,
}

impl CheckSettings {
    /// Refuse a value outside the accepted bounds.
    pub fn check(&self) -> Result<()> {
        match self.impedance_tolerance {
            Some(v) if !(v.is_finite() && TOLERANCE_RANGE.contains(&v)) => Err(Error::Invalid(
                "The impedance tolerance setting must be 0.5–50 %.".into(),
            )),
            _ => Ok(()),
        }
    }
}

/// A net class's high-speed rules. Every value is optional; an empty set
/// is not saved.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SignalRules {
    /// Single-ended characteristic impedance target, Ω. Routing derives
    /// the track width on each layer from the stackup.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub impedance: Option<f64>,
    /// Differential (edge-coupled) impedance target of the class's pairs,
    /// Ω. Routing derives the pair width and gap on each layer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff_impedance: Option<f64>,
    /// How far a routed track's impedance may stray from the target,
    /// percent. Unset, the host's Settings value applies
    /// ([`CheckSettings::impedance_tolerance`]); with neither, routed
    /// impedance is reported unchecked, never checked against a guess.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub impedance_tolerance: Option<f64>,
    /// Longest stretch, in mm, of either half of a pair that does not run
    /// coupled to the other at the pair gap (pad fan-out included).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pair_max_uncoupled: Option<f64>,
    /// Largest length difference between a pair's two halves, mm. Unset:
    /// no limit, and skew is not checked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pair_max_skew: Option<f64>,
    /// How far, mm, a coupled stretch's edge gap may stray from the pair
    /// gap; unset: the rules' `pair gap tolerance`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pair_gap_tolerance: Option<f64>,
}

impl SignalRules {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// The impedance tolerance in percent the checks use: the class's
    /// own, else the host's Settings value, else none.
    pub fn tolerance_pct(&self, settings: &CheckSettings) -> Option<f64> {
        self.impedance_tolerance.or(settings.impedance_tolerance)
    }

    /// Why these values are invalid for class `name`, or `None`.
    pub(super) fn problem(&self, name: &str) -> Option<String> {
        let within = |v: Option<f64>, range: &std::ops::RangeInclusive<f64>| {
            v.is_none_or(|v| v.is_finite() && range.contains(&v))
        };
        if !within(self.impedance, &IMPEDANCE_RANGE) {
            return Some(format!(
                "net_class.impedance: net class {name} needs an impedance of 10–200 Ω"
            ));
        }
        if !within(self.diff_impedance, &DIFF_IMPEDANCE_RANGE) {
            return Some(format!(
                "net_class.impedance: net class {name} needs a differential impedance of 20–300 Ω"
            ));
        }
        if !within(self.impedance_tolerance, &TOLERANCE_RANGE) {
            return Some(format!(
                "net_class.impedance: net class {name} needs a tolerance of 0.5–50 %"
            ));
        }
        if !within(self.pair_max_uncoupled, &(0.0..=MAX_LENGTH_MM))
            || !within(self.pair_max_skew, &(0.0..=MAX_LENGTH_MM))
            || !within(self.pair_gap_tolerance, &(0.0..=super::MAX_RULE_MM))
        {
            return Some(format!(
                "net_class.pair: net class {name} needs pair limits of 0–10000 mm and a gap tolerance of 0–25 mm"
            ));
        }
        None
    }
}

/// Project-local identity of a differential pair; never reused.
#[derive(
    Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
#[serde(transparent)]
pub struct DiffPairId(pub u32);

/// Project-local identity of a length group; never reused.
#[derive(
    Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
#[serde(transparent)]
pub struct LengthGroupId(pub u32);

/// A differential pair: two distinct nets routed together, with an
/// explicit polarity. Its width, gap, impedance and limits come from the
/// positive net's class.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DiffPair {
    pub id: DiffPairId,
    /// Unique (case-insensitively) and non-blank, such as `USB_D`.
    pub name: String,
    pub positive: Id,
    pub negative: Id,
    /// Where the pair's signal path runs, for skew and uncoupled length;
    /// derived from the copper when unset (`layout::length::path`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoints: Option<PairEndpoints>,
}

/// The pads a pair's signal path joins: `[start, end]` terminals per half.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PairEndpoints {
    pub positive: [Endpoint; 2],
    pub negative: [Endpoint; 2],
}

impl DiffPair {
    /// Whether `net` is one of the pair's halves.
    pub fn has(&self, net: Id) -> bool {
        self.positive == net || self.negative == net
    }

    /// The other half of the pair from `net`.
    pub fn partner(&self, net: Id) -> Option<Id> {
        if net == self.positive {
            Some(self.negative)
        } else if net == self.negative {
            Some(self.positive)
        } else {
            None
        }
    }
}

/// A new pair's values (`diff_pair.add`).
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DiffPairSpec {
    pub name: String,
    pub positive: Id,
    pub negative: Id,
}

/// Nets whose routed lengths must match. With a `target`, every member
/// must lie within `tolerance` of it; without one, within `tolerance` of
/// the longest member. A pair member contributes both halves, so a group
/// of one pair matches the pair's halves to each other.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LengthGroup {
    pub id: LengthGroupId,
    pub name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub nets: Vec<Id>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pairs: Vec<DiffPairId>,
    /// Target routed length, mm.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<f64>,
    /// Allowed deviation, mm.
    pub tolerance: f64,
}

/// A new group's values (`length_group.add`).
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LengthGroupSpec {
    pub name: String,
    #[serde(default)]
    pub nets: Vec<Id>,
    #[serde(default)]
    pub pairs: Vec<DiffPairId>,
    #[serde(default)]
    pub target: Option<f64>,
    pub tolerance: f64,
}

impl LengthGroup {
    /// The member nets: the listed nets, then each pair's halves, in
    /// order without repeats. Pairs the rules lack are skipped.
    pub fn member_nets(&self, rules: &ProjectRules) -> Vec<Id> {
        let mut seen = BTreeSet::new();
        let mut out = Vec::new();
        let halves = self
            .pairs
            .iter()
            .filter_map(|p| rules.diff_pair(*p))
            .flat_map(|p| [p.positive, p.negative]);
        for net in self.nets.iter().copied().chain(halves) {
            if seen.insert(net) {
                out.push(net);
            }
        }
        out
    }
}

/// What a rules command created, for its outcome.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Added {
    Class(super::NetClassId),
    Pair(DiffPairId),
    Group(LengthGroupId),
}

fn name_ok(name: &str) -> bool {
    !name.trim().is_empty() && name.len() <= MAX_PROFILE_TEXT && !name.chars().any(char::is_control)
}

/// Bounds and identities of the pairs and groups. Whether their nets
/// exist is the layout validation's (`layout::length::validate`), which
/// sees nets.
pub(super) fn validate(rules: &ProjectRules) -> Result<()> {
    if rules.diff_pairs.len() > MAX_PAIRS || rules.length_groups.len() > MAX_GROUPS {
        return Err(Error::Limit(
            "diff_pair.limit: too many pairs or length groups".into(),
        ));
    }
    let mut ids = BTreeSet::new();
    let mut names = BTreeSet::new();
    let mut halves = BTreeSet::new();
    for pair in &rules.diff_pairs {
        if !name_ok(&pair.name) {
            return Err(Error::Invalid(
                "diff_pair.name: a pair needs a short, non-blank name".into(),
            ));
        }
        if pair.id.0 >= rules.next_pair
            || !ids.insert(pair.id)
            || !names.insert(pair.name.to_lowercase())
        {
            return Err(Error::Conflict(format!(
                "diff_pair.duplicate: a pair named {} already exists",
                pair.name
            )));
        }
        if pair.positive == pair.negative {
            return Err(Error::Invalid(format!(
                "diff_pair.nets: pair {} needs two different nets",
                pair.name
            )));
        }
        if !halves.insert(pair.positive) || !halves.insert(pair.negative) {
            return Err(Error::Conflict(format!(
                "diff_pair.nets: a net of pair {} is already in another pair",
                pair.name
            )));
        }
    }
    let mut group_ids = BTreeSet::new();
    let mut group_names = BTreeSet::new();
    for group in &rules.length_groups {
        if !name_ok(&group.name) {
            return Err(Error::Invalid(
                "length_group.name: a length group needs a short, non-blank name".into(),
            ));
        }
        if group.id.0 >= rules.next_group
            || !group_ids.insert(group.id)
            || !group_names.insert(group.name.to_lowercase())
        {
            return Err(Error::Conflict(format!(
                "length_group.duplicate: a length group named {} already exists",
                group.name
            )));
        }
        let length_ok = |v: f64| v.is_finite() && (0.0..=MAX_LENGTH_MM).contains(&v);
        if !length_ok(group.tolerance) || !group.target.is_none_or(length_ok) {
            return Err(Error::Invalid(format!(
                "length_group.bounds: length group {} needs lengths of 0–10000 mm",
                group.name
            )));
        }
        if group.nets.len() + group.pairs.len() > MAX_GROUP_MEMBERS {
            return Err(Error::Limit(format!(
                "length_group.limit: length group {} has too many members",
                group.name
            )));
        }
        if group.pairs.iter().any(|p| !ids.contains(p)) {
            return Err(Error::NotFound(format!(
                "length_group.pair_missing: length group {} lists a pair that no longer exists",
                group.name
            )));
        }
        let unique_nets: BTreeSet<_> = group.nets.iter().collect();
        let unique_pairs: BTreeSet<_> = group.pairs.iter().collect();
        if unique_nets.len() != group.nets.len() || unique_pairs.len() != group.pairs.len() {
            return Err(Error::Invalid(format!(
                "length_group.duplicate_member: length group {} lists a member twice",
                group.name
            )));
        }
        if group.nets.is_empty() && group.pairs.is_empty() {
            return Err(Error::Invalid(format!(
                "length_group.empty: length group {} needs at least one net or pair",
                group.name
            )));
        }
    }
    Ok(())
}

/// Apply one pair or group command to `rules` (the caller validates and
/// discards on failure). Returns what `*.add` created.
pub(super) fn apply(rules: &mut ProjectRules, command: RulesCommand) -> Result<Option<Added>> {
    match command {
        RulesCommand::AddDiffPair { pair } => {
            let id = DiffPairId(rules.next_pair);
            rules.next_pair = rules
                .next_pair
                .checked_add(1)
                .ok_or_else(|| Error::Limit("pair identity space exhausted".into()))?;
            rules.diff_pairs.push(DiffPair {
                id,
                name: pair.name,
                positive: pair.positive,
                negative: pair.negative,
                endpoints: None,
            });
            return Ok(Some(Added::Pair(id)));
        }
        RulesCommand::EditDiffPair { pair } => {
            let slot = rules
                .diff_pairs
                .iter_mut()
                .find(|p| p.id == pair.id)
                .ok_or_else(|| {
                    Error::NotFound("diff_pair.missing: that pair no longer exists".into())
                })?;
            *slot = pair;
        }
        RulesCommand::DeleteDiffPair { pair } => {
            let before = rules.diff_pairs.len();
            rules.diff_pairs.retain(|p| p.id != pair);
            if rules.diff_pairs.len() == before {
                return Err(Error::NotFound(
                    "diff_pair.missing: that pair no longer exists".into(),
                ));
            }
            for group in &mut rules.length_groups {
                group.pairs.retain(|p| *p != pair);
            }
            rules
                .length_groups
                .retain(|g| !g.nets.is_empty() || !g.pairs.is_empty());
        }
        RulesCommand::AddLengthGroup { group } => {
            let id = LengthGroupId(rules.next_group);
            rules.next_group = rules
                .next_group
                .checked_add(1)
                .ok_or_else(|| Error::Limit("group identity space exhausted".into()))?;
            rules.length_groups.push(LengthGroup {
                id,
                name: group.name,
                nets: group.nets,
                pairs: group.pairs,
                target: group.target,
                tolerance: group.tolerance,
            });
            return Ok(Some(Added::Group(id)));
        }
        RulesCommand::EditLengthGroup { group } => {
            let slot = rules
                .length_groups
                .iter_mut()
                .find(|g| g.id == group.id)
                .ok_or_else(|| {
                    Error::NotFound(
                        "length_group.missing: that length group no longer exists".into(),
                    )
                })?;
            *slot = group;
        }
        RulesCommand::DeleteLengthGroup { group } => {
            let before = rules.length_groups.len();
            rules.length_groups.retain(|g| g.id != group);
            if rules.length_groups.len() == before {
                return Err(Error::NotFound(
                    "length_group.missing: that length group no longer exists".into(),
                ));
            }
        }
        _ => return Err(Error::Invalid("not a pair or length group command".into())),
    }
    Ok(None)
}

/// Let pairs and groups follow a net rebuild: a merged-away net is
/// replaced by its survivor; nets that no longer exist (`live`) leave
/// their groups; a pair loses its meaning, and is removed, when a half is
/// gone or both halves became one net. Groups left without members go.
pub fn follow_nets(rules: &mut ProjectRules, merged: &[(Id, Id)], live: &BTreeSet<Id>) {
    if rules.diff_pairs.is_empty() && rules.length_groups.is_empty() {
        return;
    }
    let survivor: BTreeMap<Id, Id> = merged.iter().copied().collect();
    let follow = |net: Id| survivor.get(&net).copied().unwrap_or(net);
    let mut dropped = BTreeSet::new();
    for pair in &mut rules.diff_pairs {
        pair.positive = follow(pair.positive);
        pair.negative = follow(pair.negative);
        if pair.positive == pair.negative
            || !live.contains(&pair.positive)
            || !live.contains(&pair.negative)
        {
            dropped.insert(pair.id);
        }
    }
    rules.diff_pairs.retain(|p| !dropped.contains(&p.id));
    // Two pairs merged onto one net: keep the first.
    let mut halves = BTreeSet::new();
    rules
        .diff_pairs
        .retain(|p| halves.insert(p.positive) && halves.insert(p.negative));
    let pairs: BTreeSet<DiffPairId> = rules.diff_pairs.iter().map(|p| p.id).collect();
    for group in &mut rules.length_groups {
        let mut seen = BTreeSet::new();
        group.nets = group
            .nets
            .iter()
            .map(|n| follow(*n))
            .filter(|n| live.contains(n) && seen.insert(*n))
            .collect();
        group.pairs.retain(|p| pairs.contains(p));
    }
    rules
        .length_groups
        .retain(|g| !g.nets.is_empty() || !g.pairs.is_empty());
}

/// A pair [`suggest`] proposes from two net names.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct PairSuggestion {
    /// The shared stem, such as `USB_D`.
    pub name: String,
    pub positive: Id,
    pub negative: Id,
    pub positive_name: String,
    pub negative_name: String,
}

/// Name endings that mark the positive and negative halves of a pair,
/// tried in order: `D+`/`D-`, `_P`/`_N`, `DP`/`DM` (USB), `P`/`N`.
const SUFFIXES: [(&str, &str); 6] = [
    ("+", "-"),
    ("_P", "_N"),
    ("_p", "_n"),
    ("DP", "DM"),
    ("P", "N"),
    ("p", "n"),
];

/// Pairs proposed from net names (`(id, name)`): two nets whose names
/// differ only in a positive and a negative ending ([`SUFFIXES`]) and that
/// are in no pair yet. A suggestion is never applied by itself.
pub fn suggest<'a>(
    rules: &ProjectRules,
    nets: impl IntoIterator<Item = (Id, &'a str)>,
) -> Vec<PairSuggestion> {
    let paired: BTreeSet<Id> = rules
        .diff_pairs
        .iter()
        .flat_map(|p| [p.positive, p.negative])
        .collect();
    let by_name: BTreeMap<&str, Id> = nets
        .into_iter()
        .filter(|(id, _)| !paired.contains(id))
        .map(|(id, name)| (name, id))
        .collect();
    let mut used = BTreeSet::new();
    let mut out = Vec::new();
    for (name, id) in &by_name {
        if used.contains(id) {
            continue;
        }
        for (plus, minus) in SUFFIXES {
            let Some(stem) = name.strip_suffix(plus) else {
                continue;
            };
            if stem.is_empty() {
                continue;
            }
            let other = format!("{stem}{minus}");
            let Some(negative) = by_name.get(other.as_str()).copied() else {
                continue;
            };
            if negative == *id || used.contains(&negative) {
                continue;
            }
            used.insert(*id);
            used.insert(negative);
            let stem = stem.trim_end_matches(['_', '-', '.']);
            out.push(PairSuggestion {
                name: if stem.is_empty() {
                    (*name).to_owned()
                } else {
                    stem.to_owned()
                },
                positive: *id,
                negative,
                positive_name: (*name).to_owned(),
                negative_name: other,
            });
            break;
        }
    }
    out
}

impl ProjectRules {
    pub fn diff_pair(&self, id: DiffPairId) -> Option<&DiffPair> {
        self.diff_pairs.iter().find(|p| p.id == id)
    }

    /// The pair `net` belongs to, if any.
    pub fn pair_of(&self, net: Id) -> Option<&DiffPair> {
        self.diff_pairs.iter().find(|p| p.has(net))
    }

    pub fn length_group(&self, id: LengthGroupId) -> Option<&LengthGroup> {
        self.length_groups.iter().find(|g| g.id == id)
    }

    /// The signal rules of `class` (empty for none).
    pub fn signal(&self, class: Option<super::NetClassId>) -> SignalRules {
        class
            .and_then(|c| self.class(c))
            .map(|c| c.signal.clone())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names() -> Vec<(Id, &'static str)> {
        vec![
            (1, "USB_D+"),
            (2, "USB_D-"),
            (3, "ETH_TX_P"),
            (4, "ETH_TX_N"),
            (5, "GND"),
            (6, "OTG_DP"),
            (7, "OTG_DM"),
            (8, "VP"),
        ]
    }

    #[test]
    fn suggestions_pair_names_with_polarity_endings() {
        let rules = ProjectRules::default();
        let got: Vec<(String, Id, Id)> = suggest(&rules, names())
            .into_iter()
            .map(|s| (s.name, s.positive, s.negative))
            .collect();
        assert_eq!(
            got,
            vec![
                ("ETH_TX".to_owned(), 3, 4),
                ("OTG".to_owned(), 6, 7),
                ("USB_D".to_owned(), 1, 2),
            ]
        );
    }

    #[test]
    fn paired_nets_are_not_suggested_again() -> Result<()> {
        let mut rules = ProjectRules::default();
        super::super::apply_added(
            &mut rules,
            RulesCommand::AddDiffPair {
                pair: DiffPairSpec {
                    name: "USB".into(),
                    positive: 1,
                    negative: 2,
                },
            },
        )?;
        assert!(suggest(&rules, names()).iter().all(|s| s.positive != 1));
        Ok(())
    }

    #[test]
    fn pairs_and_groups_are_checked_and_follow_merges() -> Result<()> {
        let mut rules = ProjectRules::default();
        let Some(Added::Pair(pair)) = super::super::apply_added(
            &mut rules,
            RulesCommand::AddDiffPair {
                pair: DiffPairSpec {
                    name: "USB".into(),
                    positive: 1,
                    negative: 2,
                },
            },
        )?
        else {
            return Err(Error::Invalid("no pair".into()));
        };
        // A net in two pairs, a pair of one net and a blank name refuse.
        for (name, positive, negative) in [("B", 2, 3), ("C", 4, 4), (" ", 5, 6)] {
            let mut copy = rules.clone();
            let refused = super::super::apply_added(
                &mut copy,
                RulesCommand::AddDiffPair {
                    pair: DiffPairSpec {
                        name: name.into(),
                        positive,
                        negative,
                    },
                },
            );
            assert!(refused.is_err(), "{name}");
        }
        super::super::apply_added(
            &mut rules,
            RulesCommand::AddLengthGroup {
                group: LengthGroupSpec {
                    name: "Bus".into(),
                    nets: vec![3, 4],
                    pairs: vec![pair],
                    target: Some(40.0),
                    tolerance: 0.5,
                },
            },
        )?;
        let group = rules.length_groups.first().cloned();
        assert_eq!(group.map(|g| g.member_nets(&rules)), Some(vec![3, 4, 1, 2]));
        // Net 2 merges into 1: the pair has one net and goes; net 4 merges
        // into 3 and the group lists 3 once.
        let live: BTreeSet<Id> = [1, 3].into_iter().collect();
        follow_nets(&mut rules, &[(2, 1), (4, 3)], &live);
        assert!(rules.diff_pairs.is_empty());
        assert_eq!(
            rules
                .length_groups
                .first()
                .map(|g| (g.nets.clone(), g.pairs.len())),
            Some((vec![3], 0))
        );
        Ok(())
    }
}
