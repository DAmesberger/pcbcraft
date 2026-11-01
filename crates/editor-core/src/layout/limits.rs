//! Where the board checks take their limits from: the project's effective
//! design rules (profile snapshot plus overrides) and its net classes
//! (`rules::ProjectRules`). A [`BoardRules`] is built once per check run;
//! a clearance between two objects is a table lookup unless net classes
//! exist, and only a reported violation resolves its source text.
//!
//! Messages cite a limit as `needs 0.15 mm (Fine 4/4: min mask web)`, or
//! `(net class Power: clearance)` when a class sets it, after a `; ` so an
//! issue's title (its first clause) stays the same across profile
//! switches while its cause names the rule, value and source.
use crate::ids::Id;
use crate::model::Project;
use crate::rules::{
    ClearanceSide, CopperObject, DesignRules, NetClassId, ProjectRules, RuleKey, RuleSource,
    RuleValue,
};
use std::collections::BTreeMap;

/// One limit a check compares against, with the text naming where it
/// comes from (`Fine 4/4: min mask web`).
#[derive(Clone, Debug, PartialEq)]
pub struct Need {
    pub mm: f64,
    /// Where the value comes from: `Fine 4/4`, `net class Power`.
    pub source: String,
    /// The rule's name: `min mask web`, `clearance`.
    pub rule: String,
}

impl Need {
    /// `Fine 4/4: min mask web`.
    pub fn cite(&self) -> String {
        format!("{}: {}", self.source, self.rule)
    }

    /// `needs 0.15 mm (Fine 4/4: min mask web)`.
    pub fn text(&self) -> String {
        format!("needs {} mm ({})", mm(self.mm), self.cite())
    }

    /// `needs 0.15 mm (Fine 4/4)`: a refusal's shorter form, where the
    /// rule is what the refusal says.
    pub fn short(&self) -> String {
        format!("needs {} mm ({})", mm(self.mm), self.source)
    }
}

/// A length for a message: at most three decimals, no trailing zeros
/// (`0.2`, `0.102`, `0`).
pub fn mm(value: f64) -> String {
    let text = format!("{value:.3}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text.is_empty() || text == "-" || text == "-0" {
        "0".into()
    } else {
        text.into()
    }
}

/// The cite of a scalar rule resolved from `rules`.
pub fn cite(rules: &ProjectRules, key: RuleKey) -> String {
    format!("{}: {}", rules.source_of(key).describe(), key.label())
}

/// The effective length of `key` with its cite (0 for a non-length key).
pub fn need(rules: &ProjectRules, effective: &DesignRules, key: RuleKey) -> Need {
    let mm = match key.get(effective) {
        RuleValue::Length(mm) | RuleValue::Area(mm) => mm,
        _ => 0.0,
    };
    Need {
        mm,
        source: rules.source_of(key).describe(),
        rule: key.label(),
    }
}

fn index(object: CopperObject) -> usize {
    CopperObject::ALL
        .iter()
        .position(|o| *o == object)
        .unwrap_or(0)
}

/// The project's effective rules prepared for the board checks.
pub struct BoardRules<'a> {
    project: &'a ProjectRules,
    effective: DesignRules,
    /// The object-pair matrix by [`CopperObject::ALL`] index.
    pairs: [[f64; 6]; 6],
    /// Each net's class; empty when the project has no classes.
    classes: BTreeMap<Id, NetClassId>,
}

impl<'a> BoardRules<'a> {
    pub fn of(project: &'a Project) -> Self {
        let rules = &project.rules;
        let effective = rules.effective();
        let mut pairs = [[0.0; 6]; 6];
        for (i, a) in CopperObject::ALL.iter().enumerate() {
            for (j, b) in CopperObject::ALL.iter().enumerate() {
                pairs[i][j] = effective.copper.clearance.get(*a, *b);
            }
        }
        let classes = if rules.net_classes.is_empty() {
            BTreeMap::new()
        } else {
            project
                .nets
                .iter()
                .filter_map(|n| Some((n.id, rules.class_of(n.id, &n.name)?)))
                .collect()
        };
        Self {
            project: rules,
            effective,
            pairs,
            classes,
        }
    }

    /// The effective design rules (profile with overrides).
    pub fn rules(&self) -> &DesignRules {
        &self.effective
    }

    /// The project rules the limits resolve from.
    pub fn project(&self) -> &ProjectRules {
        self.project
    }

    /// A scalar rule's effective length with its cite.
    pub fn need(&self, key: RuleKey) -> Need {
        need(self.project, &self.effective, key)
    }

    /// The class of `net` (none for an object without a net).
    pub fn class_of(&self, net: Option<Id>) -> Option<NetClassId> {
        net.and_then(|n| self.classes.get(&n).copied())
    }

    fn side(&self, object: CopperObject, net: Option<Id>) -> ClearanceSide {
        ClearanceSide {
            object,
            class: self.class_of(net),
        }
    }

    /// The least gap between `a` (on net `an`) and `b` (on `bn`), with the
    /// resolution of `ProjectRules::clearance`: the larger of the
    /// object-pair value, each copper side's class clearance and the
    /// class-pair entry.
    pub fn clearance(
        &self,
        a: CopperObject,
        an: Option<Id>,
        b: CopperObject,
        bn: Option<Id>,
    ) -> f64 {
        let base = self.pairs[index(a)][index(b)];
        if self.classes.is_empty() {
            return base;
        }
        let copper = |o: CopperObject| !matches!(o, CopperObject::BoardEdge | CopperObject::Hole);
        let (ca, cb) = (self.class_of(an), self.class_of(bn));
        let mut best = base;
        for (object, class) in [(a, ca), (b, cb)] {
            if copper(object)
                && let Some(mm) = class
                    .and_then(|c| self.project.class(c))
                    .and_then(|c| c.clearance)
            {
                best = best.max(mm);
            }
        }
        if copper(a)
            && copper(b)
            && let (Some(ca), Some(cb)) = (ca, cb)
            && let Some(pair) = self.project.class_clearances.iter().find(|p| p.is(ca, cb))
        {
            best = best.max(pair.mm);
        }
        best
    }

    /// [`Self::clearance`] with its cite, for a message.
    pub fn clearance_need(
        &self,
        a: CopperObject,
        an: Option<Id>,
        b: CopperObject,
        bn: Option<Id>,
    ) -> Need {
        let resolved = self.project.clearance(self.side(a, an), self.side(b, bn));
        let rule = match &resolved.source {
            RuleSource::Profile { .. } | RuleSource::Override => {
                RuleKey::Clearance { a, b }.normalized().label()
            }
            _ => "clearance".into(),
        };
        Need {
            mm: resolved.value,
            source: resolved.source.describe(),
            rule,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::{ClassClearance, NetClass, NetClassAssignment, RuleOverride};

    #[test]
    fn lengths_print_without_trailing_zeros() {
        assert_eq!(mm(0.2), "0.2");
        assert_eq!(mm(0.25), "0.25");
        assert_eq!(mm(4.0 * 0.0254), "0.102");
        assert_eq!(mm(0.0), "0");
        assert_eq!(mm(1.0), "1");
    }

    fn class(id: u32, name: &str, clearance: Option<f64>) -> NetClass {
        NetClass {
            id: NetClassId(id),
            name: name.into(),
            track_width: None,
            track_width_min: None,
            clearance,
            via_drill: None,
            via_diameter: None,
            diff_pair_width: None,
            diff_pair_gap: None,
            fill_connection: None,
        }
    }

    /// The fast lookup resolves exactly as `ProjectRules::clearance` for
    /// every object pair and class combination.
    #[test]
    fn clearance_lookup_matches_the_rules_resolution() {
        let mut project = Project::default();
        for (id, name) in [(1, "GND"), (2, "VBUS"), (3, "SDA")] {
            project.nets.push(crate::model::Net {
                id,
                name: name.into(),
                terminals: Vec::new(),
            });
        }
        project.rules.overrides.push(RuleOverride {
            key: RuleKey::Clearance {
                a: CopperObject::Hole,
                b: CopperObject::Pad,
            },
            value: RuleValue::Length(0.4),
        });
        project.rules.net_classes = vec![class(1, "Power", Some(0.3)), class(2, "Signal", None)];
        project.rules.assignments = vec![
            NetClassAssignment {
                net: 2,
                class: NetClassId(1),
            },
            NetClassAssignment {
                net: 3,
                class: NetClassId(2),
            },
        ];
        project.rules.class_clearances = vec![ClassClearance {
            a: NetClassId(1),
            b: NetClassId(2),
            mm: 0.5,
        }];
        let limits = BoardRules::of(&project);
        for a in CopperObject::ALL {
            for b in CopperObject::ALL {
                for an in [None, Some(1), Some(2), Some(3)] {
                    for bn in [None, Some(1), Some(2), Some(3)] {
                        let want = project.rules.clearance(
                            ClearanceSide {
                                object: a,
                                class: limits.class_of(an),
                            },
                            ClearanceSide {
                                object: b,
                                class: limits.class_of(bn),
                            },
                        );
                        let got = limits.clearance(a, an, b, bn);
                        assert_eq!(got, want.value, "{a:?}/{b:?} {an:?}/{bn:?}");
                        assert_eq!(limits.clearance_need(a, an, b, bn).mm, got);
                    }
                }
            }
        }
        let need = limits.clearance_need(CopperObject::Track, Some(2), CopperObject::Pad, Some(1));
        assert_eq!(need.text(), "needs 0.3 mm (net class Power: clearance)");
        let need = limits.clearance_need(CopperObject::Track, Some(2), CopperObject::Pad, Some(3));
        assert_eq!(
            need.text(),
            "needs 0.5 mm (net classes Power / Signal: clearance)"
        );
        let need = limits.clearance_need(CopperObject::Hole, None, CopperObject::Pad, Some(1));
        assert_eq!(
            need.text(),
            "needs 0.4 mm (project override: pad to hole clearance)"
        );
        let need = limits.clearance_need(CopperObject::Track, Some(1), CopperObject::Pad, Some(3));
        assert_eq!(
            need.text(),
            "needs 0.2 mm (Pcbcraft default: track to pad clearance)"
        );
    }
}
