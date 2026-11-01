use super::*;
use crate::ids::Id;

#[test]
fn every_builtin_profile_is_valid_and_estimated() -> Result<()> {
    let all = builtin::all();
    assert_eq!(all[0].id, builtin::DEFAULT_PROFILE_ID);
    assert_eq!(all[0].rules, DesignRules::default());
    for p in &all {
        p.validate()?;
        assert_eq!(p.provenance.basis, ProfileBasis::Estimated);
        assert_eq!(p.provenance.origin, ProfileOrigin::BuiltIn);
    }
    let ids: std::collections::BTreeSet<_> = all.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(ids.len(), all.len());
    Ok(())
}

#[test]
fn every_key_round_trips_through_get_and_set() -> Result<()> {
    let base = DesignRules::default();
    for key in RuleKey::all() {
        let mut rules = base.clone();
        key.set(&mut rules, key.get(&base))?;
        assert_eq!(rules, base, "{key:?}");
    }
    assert_eq!(RuleKey::all().len(), 2 + 21 + 25);
    Ok(())
}

#[test]
fn overrides_win_and_name_their_source() -> Result<()> {
    let mut rules = ProjectRules::default();
    let key = RuleKey::Clearance {
        a: CopperObject::Via,
        b: CopperObject::Track,
    };
    apply(
        &mut rules,
        RulesCommand::SetOverride {
            key,
            value: RuleValue::Length(0.3),
        },
    )?;
    assert_eq!(rules.resolve(key).value, RuleValue::Length(0.3));
    assert_eq!(rules.source_of(key), RuleSource::Override);
    let wrong = apply(
        &mut rules.clone(),
        RulesCommand::SetOverride {
            key: RuleKey::TentVias,
            value: RuleValue::Length(1.0),
        },
    );
    assert!(wrong.is_err());
    apply(
        &mut rules,
        RulesCommand::ClearOverride {
            key: key.normalized(),
        },
    )?;
    assert_eq!(rules, ProjectRules::default());
    Ok(())
}

#[test]
fn net_class_clearance_takes_the_largest_value() -> Result<()> {
    let mut rules = ProjectRules::default();
    let power = apply(
        &mut rules,
        RulesCommand::AddNetClass {
            class: NetClassSpec {
                name: "Power".into(),
                clearance: Some(0.3),
                track_width: Some(0.5),
                ..Default::default()
            },
        },
    )?
    .ok_or_else(|| Error::Invalid("no class".into()))?;
    let hv = apply(
        &mut rules,
        RulesCommand::AddNetClass {
            class: NetClassSpec {
                name: "HV".into(),
                ..Default::default()
            },
        },
    )?
    .ok_or_else(|| Error::Invalid("no class".into()))?;
    apply(
        &mut rules,
        RulesCommand::SetClassClearance {
            a: hv,
            b: power,
            mm: Some(1.0),
        },
    )?;
    let side = |class| ClearanceSide {
        object: CopperObject::Track,
        class,
    };
    let plain = rules.clearance(side(None), side(None));
    assert_eq!(plain.value, 0.2);
    let one = rules.clearance(side(Some(power)), side(None));
    assert_eq!(one.value, 0.3);
    assert_eq!(one.source.describe(), "net class Power");
    assert_eq!(
        rules.clearance(side(Some(power)), side(Some(hv))).value,
        1.0
    );
    assert_eq!(rules.track_width(Some(power)).value, 0.5);
    assert_eq!(rules.track_width(None).value, 0.3);
    // Explicit assignment beats a pattern; nothing else assigns.
    apply(
        &mut rules,
        RulesCommand::SetPatterns {
            patterns: vec![NetClassPattern {
                pattern: "V*".into(),
                class: power,
            }],
        },
    )?;
    let net: Id = 7;
    assert_eq!(rules.class_of(net, "VBUS"), Some(power));
    assert_eq!(rules.class_of(net, "GND"), None);
    apply(
        &mut rules,
        RulesCommand::AssignNets {
            nets: vec![net],
            class: Some(hv),
        },
    )?;
    assert_eq!(rules.class_of(net, "VBUS"), Some(hv));
    apply(&mut rules, RulesCommand::DeleteNetClass { class: hv })?;
    assert!(rules.assignments.is_empty() && rules.class_clearances.is_empty());
    Ok(())
}

#[test]
fn glob_patterns() {
    assert!(glob_matches("*", ""));
    assert!(glob_matches("V?_*", "V3_3V"));
    assert!(glob_matches("*GND", "AGND"));
    assert!(!glob_matches("GND", "AGND"));
    assert!(!glob_matches("V?", "V"));
}

#[test]
fn invalid_rules_are_refused() {
    let mut rules = DesignRules::default();
    rules.holes.via_diameter_default = 0.4;
    assert!(rules.validate().is_err());
    let mut rules = DesignRules::default();
    rules.copper.clearance.pairs.pop();
    assert!(rules.validate().is_err());
}
