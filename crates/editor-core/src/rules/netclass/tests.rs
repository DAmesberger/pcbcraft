use super::*;
use crate::rules::{self, RuleValue};

fn spec(name: &str) -> NetClassSpec {
    NetClassSpec {
        name: name.into(),
        ..Default::default()
    }
}

fn add(rules: &mut ProjectRules, class: NetClassSpec) -> Result<NetClassId> {
    rules::apply(rules, RulesCommand::AddNetClass { class })?
        .ok_or_else(|| Error::Invalid("no class".into()))
}

#[test]
fn class_values_are_checked_against_each_other() -> Result<()> {
    let mut rules = ProjectRules::default();
    let narrow = NetClassSpec {
        track_width: Some(0.2),
        track_width_min: Some(0.3),
        ..spec("Power")
    };
    assert!(
        rules::apply(
            &mut rules.clone(),
            RulesCommand::AddNetClass { class: narrow }
        )
        .is_err()
    );
    let via = NetClassSpec {
        via_drill: Some(0.4),
        via_diameter: Some(0.4),
        ..spec("Power")
    };
    assert!(rules::apply(&mut rules.clone(), RulesCommand::AddNetClass { class: via }).is_err());
    add(&mut rules, spec("Power"))?;
    let duplicate = rules::apply(
        &mut rules.clone(),
        RulesCommand::AddNetClass {
            class: spec("power"),
        },
    );
    assert!(matches!(duplicate, Err(Error::Conflict(_))));
    let blank = rules::apply(
        &mut rules.clone(),
        RulesCommand::AddNetClass { class: spec("  ") },
    );
    assert!(blank.is_err());
    for message in [narrow_message(), duplicate_message()] {
        assert!(
            crate::issues::style::check(&message).is_empty(),
            "{message}"
        );
    }
    Ok(())
}

fn narrow_message() -> String {
    let mut rules = ProjectRules::default();
    let class = NetClassSpec {
        track_width: Some(0.2),
        track_width_min: Some(0.3),
        ..spec("Power")
    };
    rules::apply(&mut rules, RulesCommand::AddNetClass { class })
        .err()
        .map(|e| e.to_string())
        .unwrap_or_default()
}

fn duplicate_message() -> String {
    let mut rules = ProjectRules::default();
    let _ = add(&mut rules, spec("Power"));
    rules::apply(
        &mut rules,
        RulesCommand::AddNetClass {
            class: spec("POWER"),
        },
    )
    .err()
    .map(|e| e.to_string())
    .unwrap_or_default()
}

#[test]
fn minimum_width_is_the_larger_of_rule_and_class() -> Result<()> {
    let mut rules = crate::rules::fixtures::rules()?;
    let power = add(
        &mut rules,
        NetClassSpec {
            track_width: Some(0.5),
            track_width_min: Some(0.2),
            ..spec("Power")
        },
    )?;
    let thin = add(
        &mut rules,
        NetClassSpec {
            track_width_min: Some(0.01),
            ..spec("Thin")
        },
    )?;
    assert_eq!(rules.track_width_min(None).value, 0.05);
    let class_min = rules.track_width_min(Some(power));
    assert_eq!(class_min.value, 0.2);
    assert_eq!(class_min.source.describe(), "net class Power");
    // A class minimum below the rule minimum never loosens it.
    assert_eq!(rules.track_width_min(Some(thin)).value, 0.05);
    rules::apply(
        &mut rules,
        RulesCommand::SetOverride {
            key: RuleKey::TrackWidthMin,
            value: RuleValue::Length(0.25),
        },
    )?;
    let raised = rules.track_width_min(Some(power));
    assert_eq!((raised.value, raised.source), (0.25, RuleSource::Override));
    assert_eq!(rules.track_width(Some(power)).value, 0.5);
    Ok(())
}

#[test]
fn explicit_assignment_beats_patterns_and_the_report_says_how() -> Result<()> {
    let mut rules = ProjectRules::default();
    let power = add(&mut rules, spec("Power"))?;
    let signal = add(&mut rules, spec("Signal"))?;
    rules::apply(
        &mut rules,
        RulesCommand::SetPatterns {
            patterns: vec![
                NetClassPattern {
                    pattern: "+*V".into(),
                    class: power,
                },
                NetClassPattern {
                    pattern: "*".into(),
                    class: signal,
                },
            ],
        },
    )?;
    rules::apply(
        &mut rules,
        RulesCommand::AssignNets {
            nets: vec![3],
            class: Some(power),
        },
    )?;
    let nets = [(1, "+3V3"), (2, "SDA"), (3, "VBUS"), (4, "+5V")];
    let report = report(&rules, nets);
    let class = |net: Id| {
        report
            .nets
            .iter()
            .find(|m| m.net == net)
            .and_then(|m| m.class)
    };
    // "+3V3" ends in 3, not V: the second pattern matches.
    assert_eq!(class(1), Some(signal));
    assert_eq!(class(2), Some(signal));
    assert_eq!(class(3), Some(power));
    assert_eq!(class(4), Some(power));
    assert_eq!(
        report
            .nets
            .iter()
            .find(|m| m.net == 3)
            .and_then(|m| m.basis.clone()),
        Some(ClassBasis::Explicit)
    );
    assert_eq!(
        report
            .nets
            .iter()
            .find(|m| m.net == 4)
            .and_then(|m| m.basis.clone()),
        Some(ClassBasis::Pattern {
            pattern: "+*V".into()
        })
    );
    let row = report.classes.iter().find(|r| r.class.id == power);
    assert_eq!(row.map(|r| r.members), Some(2));
    Ok(())
}

#[test]
fn pattern_preview_lists_only_changed_nets_and_applies_nothing() -> Result<()> {
    let mut rules = ProjectRules::default();
    let power = add(&mut rules, spec("Power"))?;
    let before = rules.clone();
    let nets = [(1, "VCC"), (2, "GND"), (3, "VCC_IO")];
    let changes = preview_patterns(
        &rules,
        vec![NetClassPattern {
            pattern: "VCC*".into(),
            class: power,
        }],
        nets,
    )?;
    assert_eq!(
        changes.iter().map(|c| c.net).collect::<Vec<_>>(),
        vec![1, 3]
    );
    assert!(
        changes
            .iter()
            .all(|c| c.from.is_none() && c.to == Some(power))
    );
    assert_eq!(rules, before);
    let bad = preview_patterns(
        &rules,
        vec![NetClassPattern {
            pattern: "VCC".into(),
            class: NetClassId(99),
        }],
        nets,
    );
    assert!(bad.is_err());
    Ok(())
}

#[test]
fn assignments_follow_merges_and_splits() -> Result<()> {
    let mut rules = ProjectRules::default();
    let power = add(&mut rules, spec("Power"))?;
    let signal = add(&mut rules, spec("Signal"))?;
    rules::apply(
        &mut rules,
        RulesCommand::AssignNets {
            nets: vec![1, 2],
            class: Some(power),
        },
    )?;
    rules::apply(
        &mut rules,
        RulesCommand::AssignNets {
            nets: vec![5],
            class: Some(signal),
        },
    )?;
    let live: BTreeSet<Id> = [3, 4, 6, 7, 8].into_iter().collect();
    // 1 merges into unclassed 3: 3 takes Power. 2 and 5 both merge into
    // 4 with different classes: 4 takes none. 1 also split off new 7.
    // 6 is an existing net that absorbed part of 5: it takes nothing.
    follow_nets(
        &mut rules,
        &[(1, 3), (2, 4), (5, 4)],
        &[(1, 7), (5, 6)],
        &[7],
        &live,
    );
    let explicit: Vec<(Id, NetClassId)> =
        rules.assignments.iter().map(|a| (a.net, a.class)).collect();
    assert_eq!(explicit, vec![(3, power), (7, power)]);
    rules.validate()
}

#[test]
fn a_survivors_own_class_wins_over_merged_ones() -> Result<()> {
    let mut rules = ProjectRules::default();
    let power = add(&mut rules, spec("Power"))?;
    let signal = add(&mut rules, spec("Signal"))?;
    rules::apply(
        &mut rules,
        RulesCommand::AssignNets {
            nets: vec![1],
            class: Some(power),
        },
    )?;
    rules::apply(
        &mut rules,
        RulesCommand::AssignNets {
            nets: vec![2],
            class: Some(signal),
        },
    )?;
    follow_nets(&mut rules, &[(2, 1)], &[], &[], &[1].into_iter().collect());
    assert_eq!(
        rules.assignments,
        vec![NetClassAssignment {
            net: 1,
            class: power
        }]
    );
    Ok(())
}

#[test]
fn assigning_no_nets_is_refused() {
    let mut rules = ProjectRules::default();
    let result = rules::apply(
        &mut rules,
        RulesCommand::AssignNets {
            nets: vec![],
            class: None,
        },
    );
    assert!(result.is_err());
}

#[test]
fn spec_without_minimum_serializes_as_before() -> Result<()> {
    let mut rules = ProjectRules::default();
    add(&mut rules, spec("Power"))?;
    let json = serde_json::to_string(&rules.net_classes)?;
    assert!(!json.contains("track_width_min"), "{json}");
    Ok(())
}
