use super::*;
use crate::ids::Id;

fn proto() -> Result<ProjectRules> {
    fixtures::rules()
}

fn j<E: std::fmt::Display>(e: E) -> Error {
    Error::Invalid(e.to_string())
}

#[test]
fn every_key_round_trips_through_get_set_and_its_name() -> Result<()> {
    let base = fixtures::prototype()?.rules().clone();
    for key in RuleKey::all() {
        let mut rules = base.clone();
        key.set(&mut rules, key.get(&base))?;
        assert_eq!(rules, base, "{key:?}");
        assert_eq!(RuleKey::from_name(&key.name()), Some(key), "{key:?}");
    }
    // 2 track widths, 36 clearance pairs, 25 contract scalars, the 5
    // appended silkscreen-edge and land keys, the 2 etch-hazard keys, the
    // board outline fit, the legend stroke share and the pair gap
    // tolerance's minimum and share, and the NPTH pad annular ring.
    assert_eq!(RuleKey::all().len(), 2 + 36 + 25 + 5 + 2 + 1 + 3 + 1);
    let names: std::collections::BTreeSet<_> = RuleKey::all().iter().map(|k| k.name()).collect();
    assert_eq!(names.len(), RuleKey::all().len(), "names are unique");
    assert_eq!(
        RuleKey::Clearance {
            a: CopperObject::BoardEdge,
            b: CopperObject::Track
        }
        .name(),
        "clearance_track_board_edge"
    );
    Ok(())
}

#[test]
fn a_profile_file_must_set_every_rule_with_a_source() -> Result<()> {
    let value: serde_json::Value = serde_json::from_str(fixtures::PROTOTYPE_JSON).map_err(j)?;
    let parse = |v: &serde_json::Value| serde_json::from_value::<ProfileFile>(v.clone());
    assert!(parse(&value).is_ok());
    let mut missing = value.clone();
    if let Some(rules) = missing["fab_profile"]["rules"].as_object_mut() {
        rules.remove("mask_web_min");
    }
    let err = parse(&missing).map_err(|e| e.to_string());
    assert!(
        err.as_ref()
            .is_err_and(|e| e.contains("does not set mask_web_min")),
        "{err:?}"
    );
    let mut unknown = value.clone();
    unknown["fab_profile"]["rules"]["future_rule"] = serde_json::json!({"value": 1.0});
    assert!(parse(&unknown).is_err(), "unknown rules are refused");
    let mut both = value.clone();
    both["fab_profile"]["rules"]["drill_min"] = serde_json::json!({"value": 0.1, "formula": "0.1"});
    assert!(parse(&both).is_err(), "a value or a formula, not both");
    let mut wrong_kind = value.clone();
    wrong_kind["fab_profile"]["rules"]["tent_vias"] = serde_json::json!({"value": 1.0});
    assert!(parse(&wrong_kind).is_err());
    let mut extra = value.clone();
    extra["fab_profile"]["basis"] = "estimated".into();
    assert!(parse(&extra).is_err(), "unknown fields are refused");
    let mut unsourced = value;
    unsourced["fab_profile"]["source"]["publisher"] = "".into();
    let err = parse(&unsourced).map_err(|e| e.to_string());
    assert!(
        err.as_ref()
            .is_err_and(|e| e.contains("must name who published its numbers")),
        "{err:?}"
    );
    Ok(())
}

#[test]
fn a_profile_round_trips_byte_for_byte() -> Result<()> {
    for json in [fixtures::PROTOTYPE_JSON, fixtures::FINE_JSON] {
        let file: ProfileFile = serde_json::from_str(json).map_err(j)?;
        let mut text = serde_json::to_string_pretty(&file).map_err(j)?;
        text.push('\n');
        assert_eq!(text, json);
    }
    Ok(())
}

#[test]
fn formulas_follow_their_inputs_and_project_overrides() -> Result<()> {
    let fine = fixtures::fine()?;
    // via_diameter_default = via_drill_default + 2.0 * via_annular_ring_min
    assert!((fine.rules().holes.via_diameter_default - 0.45).abs() < 1e-12);
    let mut rules = ProjectRules {
        profile: Some(fine),
        ..ProjectRules::default()
    };
    apply(
        &mut rules,
        RulesCommand::SetOverride {
            key: RuleKey::ViaDrillDefault,
            value: RuleValue::Length(0.3),
        },
    )?;
    assert!((rules.effective().holes.via_diameter_default - 0.55).abs() < 1e-12);
    let row = report(&rules)
        .rows
        .into_iter()
        .find(|r| r.key == RuleKey::ViaDiameterDefault)
        .ok_or_else(|| Error::Invalid("no row".into()))?;
    assert_eq!(
        row.formula.as_deref(),
        Some("via_drill_default + 2.0 * via_annular_ring_min")
    );
    // An override of the formula's own rule replaces the formula.
    apply(
        &mut rules,
        RulesCommand::SetOverride {
            key: RuleKey::ViaDiameterDefault,
            value: RuleValue::Length(0.7),
        },
    )?;
    assert_eq!(rules.effective().holes.via_diameter_default, 0.7);
    Ok(())
}

#[test]
fn a_formula_cycle_is_refused_with_its_chain() -> Result<()> {
    let mut p = fixtures::prototype()?;
    p.set(
        RuleKey::ViaDrillDefault,
        RuleEntry::formula("via_diameter_default - 0.3", ""),
    )?;
    let before = p.clone();
    let (rule, text) = refusal(p.set(
        RuleKey::ViaDiameterDefault,
        RuleEntry::formula("via_drill_default + 0.3", ""),
    ));
    assert_eq!(rule, "rules.formula_cycle", "{text}");
    assert!(
        text.contains("via_diameter_default → via_drill_default → via_diameter_default")
            || text.contains("via_drill_default → via_diameter_default → via_drill_default"),
        "{text}"
    );
    assert_eq!(p, before, "a refused formula changes nothing");
    Ok(())
}

#[test]
fn a_bad_formula_is_refused_in_plain_words() -> Result<()> {
    let mut p = fixtures::prototype()?;
    let refused = |p: &mut RuleProfile, key: RuleKey, text: &str| {
        let (rule, text) = refusal(p.set(key, RuleEntry::formula(text, "")));
        format!("{rule}: {text}")
    };
    let text = refused(&mut p, RuleKey::DrillMin, "pin_diagonal + 0.1");
    assert!(text.contains("pin_diagonal is not a rule name"), "{text}");
    let text = refused(&mut p, RuleKey::TentVias, "drill_min * 2.0");
    assert!(text.contains("another kind"), "{text}");
    let text = refused(&mut p, RuleKey::DrillMin, "loop { }");
    assert!(text.starts_with("rules.formula:"), "{text}");
    let text = refused(&mut p, RuleKey::DrillMin, "drill_min * 1000.0");
    assert!(
        text.starts_with("rules.formula_cycle:"),
        "a formula naming itself: {text}"
    );
    let text = refused(&mut p, RuleKey::DrillMin, "0.0");
    assert!(text.contains("min drill must be positive"), "{text}");
    // A sound formula of other rules is taken.
    p.set(
        RuleKey::FillWidthMin,
        RuleEntry::formula("max(track_width_min * 2.0, 0.1)", "derived"),
    )?;
    assert_eq!(p.rules().fill.width_min, 0.1);
    Ok(())
}

#[test]
fn a_project_without_a_profile_has_no_rules_and_tools_refuse() -> Result<()> {
    let rules = ProjectRules::default();
    assert!(rules.profile.is_none());
    assert_eq!(rules.effective(), DesignRules::unset());
    assert_eq!(refusal(rules.require()).0, "rules.no_fab_profile");
    let mut rules = rules;
    assert!(
        apply(
            &mut rules,
            RulesCommand::SetOverride {
                key: RuleKey::MaskWebMin,
                value: RuleValue::Length(0.1),
            },
        )
        .is_err(),
        "an override needs a profile to depart from"
    );
    apply(&mut rules, set_profile_cmd(fixtures::prototype()?, false))?;
    assert_eq!(&rules.effective(), fixtures::prototype()?.rules());
    Ok(())
}

#[test]
fn overrides_win_and_name_their_source() -> Result<()> {
    let mut rules = proto()?;
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
    assert_eq!(rules, proto()?);
    Ok(())
}

#[test]
fn net_class_clearance_takes_the_largest_value() -> Result<()> {
    let mut rules = proto()?;
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
fn invalid_rules_are_refused() -> Result<()> {
    let mut rules = fixtures::prototype()?.rules().clone();
    rules.holes.via_diameter_default = 0.4;
    assert!(rules.validate().is_err());
    let mut rules = fixtures::prototype()?.rules().clone();
    rules.copper.clearance.pairs.pop();
    assert!(rules.validate().is_err());
    Ok(())
}

#[test]
fn the_legend_stroke_is_a_share_of_the_height_and_never_under_the_line_rule() -> Result<()> {
    let silk = fixtures::prototype()?.rules().silkscreen;
    // 1/7 of 1 mm, above the 0.1 mm line rule.
    assert!((silk.text_stroke(1.0) - 1.0 / 7.0).abs() < 1e-12);
    // 1/7 of 0.6 mm is under it: the rule's width.
    assert_eq!(silk.text_stroke(0.6), 0.1);
    Ok(())
}

/// The rule code and the lower-cased text of a refusal ("" when none).
fn refusal<T>(result: Result<T>) -> (String, String) {
    match result {
        Ok(_) => (String::new(), String::new()),
        Err(e) => (
            e.rule().unwrap_or_default().to_string(),
            e.to_string().to_lowercase(),
        ),
    }
}

fn set_profile_cmd(profile: RuleProfile, keep_overrides: bool) -> RulesCommand {
    RulesCommand::SetProfile {
        profile: Box::new(profile),
        keep_overrides,
    }
}

#[test]
fn profile_switch_keeps_or_drops_overrides() -> Result<()> {
    let mut rules = proto()?;
    apply(
        &mut rules,
        RulesCommand::SetOverride {
            key: RuleKey::MaskWebMin,
            value: RuleValue::Length(0.12),
        },
    )?;
    let mut kept = rules.clone();
    apply(&mut kept, set_profile_cmd(fixtures::fine()?, true))?;
    assert_eq!(
        kept.profile.as_ref().map(|p| p.id.as_str()),
        Some("test-fine")
    );
    assert_eq!(kept.effective().mask.web_min, 0.12);
    assert_eq!(kept.source_of(RuleKey::MaskWebMin), RuleSource::Override);
    assert_eq!(
        kept.source_of(RuleKey::DrillMin).describe(),
        "Test fine",
        "a rule without override names the profile"
    );
    let mut dropped = rules.clone();
    apply(&mut dropped, set_profile_cmd(fixtures::fine()?, false))?;
    assert!(dropped.overrides.is_empty());
    assert_eq!(&dropped.effective(), fixtures::fine()?.rules());
    let impact = rule_changes(&rules, &dropped);
    assert_eq!(impact.dropped_overrides, vec![RuleKey::MaskWebMin]);
    assert!(rule_changes(&rules, &kept).dropped_overrides.is_empty());
    Ok(())
}

#[test]
fn a_kept_override_that_breaks_the_new_profile_is_refused() -> Result<()> {
    let mut rules = proto()?;
    // A 0.1 mm via drill is fine for the prototype (0.1 mm minimum drill)
    // but below the fine fixture's 0.2 mm minimum.
    apply(
        &mut rules,
        RulesCommand::SetOverride {
            key: RuleKey::ViaDrillDefault,
            value: RuleValue::Length(0.1),
        },
    )?;
    let before = rules.clone();
    let (rule, err) = refusal(apply(&mut rules, set_profile_cmd(fixtures::fine()?, true)));
    assert_eq!(rule, "rules.override_conflict");
    assert!(err.contains("default via drill"), "{err}");
    assert_eq!(rules, before, "a refused switch changes nothing");
    let (candidate, refusal) = preview_candidate(&rules, &fixtures::fine()?, true);
    assert!(refusal.is_some_and(|r| r.contains("Test fine")));
    assert_eq!(
        candidate.profile.as_ref().map(|p| p.id.as_str()),
        Some("test-fine")
    );
    // Dropping the overrides makes the same switch valid.
    let (_, refusal) = preview_candidate(&rules, &fixtures::fine()?, false);
    assert_eq!(refusal, None);
    Ok(())
}

#[test]
fn any_sourced_profile_is_accepted_and_an_invalid_one_refused_whole() -> Result<()> {
    let mut rules = proto()?;
    let mut mine = fixtures::fine()?;
    mine.id = "my-fab".into();
    mine.name = "My fab 4/4".into();
    mine.set(
        RuleKey::DrillMin,
        RuleEntry::value(RuleValue::Length(0.15), "Entered by hand"),
    )?;
    apply(&mut rules, set_profile_cmd(mine, false))?;
    assert_eq!(rules.effective().holes.drill_min, 0.15);
    assert_eq!(
        rules.resolve(RuleKey::DrillMin).source.describe(),
        "My fab 4/4"
    );
    // An invalid profile cannot even be made.
    let mut broken = fixtures::fine()?;
    assert!(
        broken
            .set(
                RuleKey::TrackWidthDefault,
                RuleEntry::value(RuleValue::Length(0.01), "")
            )
            .is_err()
    );
    Ok(())
}

#[test]
fn bounds_name_the_rule() -> Result<()> {
    let cases: [(RuleKey, RuleValue, &str); 7] = [
        (
            RuleKey::SilkLineWidthMin,
            RuleValue::Length(0.0),
            "min silkscreen width",
        ),
        (RuleKey::DrillMin, RuleValue::Length(30.0), "min drill"),
        (
            RuleKey::MaskExpansion,
            RuleValue::Length(-2.0),
            "mask expansion",
        ),
        (
            RuleKey::ThermalSpokeCount,
            RuleValue::Count(0),
            "thermal spoke count",
        ),
        (
            RuleKey::AspectRatioMax,
            RuleValue::Ratio(Some(0.5)),
            "max aspect ratio",
        ),
        (
            RuleKey::IslandAreaMin,
            RuleValue::Area(f64::NAN),
            "min island area",
        ),
        (
            RuleKey::SilkTextStrokeShare,
            RuleValue::Fraction(1.5),
            "silkscreen text stroke share",
        ),
    ];
    for (key, value, label) in cases {
        let mut rules = proto()?;
        let (rule, err) = refusal(apply(&mut rules, RulesCommand::SetOverride { key, value }));
        assert_eq!(rule, "rules.bounds", "{err}");
        assert!(err.contains(label), "{err}");
        assert_eq!(rules, proto()?);
    }
    // Negative mask expansion shrinks the opening and is allowed.
    let mut rules = proto()?;
    assert!(
        apply(
            &mut rules,
            RulesCommand::SetOverride {
                key: RuleKey::MaskExpansion,
                value: RuleValue::Length(-0.02),
            },
        )
        .is_ok()
    );
    assert!(
        apply(
            &mut rules,
            RulesCommand::ClearOverride {
                key: RuleKey::TentVias
            }
        )
        .is_err_and(|e| e.rule() == Some("rules.no_override"))
    );
    Ok(())
}

#[test]
fn resolution_order_is_profile_override_class_pair_region() {
    let sources = [
        RuleSource::Profile { name: "P".into() },
        RuleSource::Override,
        RuleSource::NetClass { class: "C".into() },
        RuleSource::ClassPair {
            a: "A".into(),
            b: "B".into(),
        },
        RuleSource::Region { name: "R".into() },
    ];
    let ranks: Vec<u8> = sources.iter().map(RuleSource::precedence).collect();
    assert_eq!(ranks, vec![0, 1, 2, 3, 4]);
    assert_eq!(sources[4].describe(), "region R");
    assert_eq!(sources[1].describe(), "project override");
}

#[test]
fn net_class_values_win_over_an_override_and_hole_pairs_ignore_classes() -> Result<()> {
    let mut rules = proto()?;
    apply(
        &mut rules,
        RulesCommand::SetOverride {
            key: RuleKey::TrackWidthDefault,
            value: RuleValue::Length(0.4),
        },
    )?;
    let power = apply(
        &mut rules,
        RulesCommand::AddNetClass {
            class: NetClassSpec {
                name: "Power".into(),
                clearance: Some(0.5),
                track_width: Some(0.8),
                via_drill: Some(0.4),
                ..Default::default()
            },
        },
    )?
    .ok_or_else(|| Error::Invalid("no class".into()))?;
    assert_eq!(rules.track_width(None).value, 0.4);
    assert_eq!(rules.track_width(None).source, RuleSource::Override);
    assert_eq!(rules.track_width(Some(power)).value, 0.8);
    let via = rules.via_size(Some(power));
    assert_eq!(via.value, (0.4, 0.6));
    assert_eq!(via.source.precedence(), 2);
    let hole = ClearanceSide {
        object: CopperObject::ViaHole,
        class: Some(power),
    };
    let track = ClearanceSide {
        object: CopperObject::Track,
        class: None,
    };
    // The class clearance applies to its copper, not to a hole wall.
    let gap = rules.clearance(hole, track);
    assert_eq!(gap.value, 0.25);
    assert_eq!(gap.source.precedence(), 0);
    let copper = ClearanceSide {
        object: CopperObject::Via,
        class: Some(power),
    };
    assert_eq!(rules.clearance(copper, track).value, 0.5);
    Ok(())
}

#[test]
fn issue_diff_matches_by_identity_and_counts_repeats() -> Result<()> {
    let mut impact = rule_changes(&proto()?, &proto()?);
    assert!(impact.changes.is_empty());
    let pair = |id: &str, title: &str| (id.to_string(), title.to_string());
    diff_issues(
        &mut impact,
        [
            pair("web R1", "R1 web 0.1 mm"),
            pair("drill V1", "V1 drill"),
            pair("dup", "Twice"),
        ],
        [
            pair("web R1", "R1 web 0.15 mm"),
            pair("ring V2", "V2 ring"),
            pair("dup", "Twice"),
            pair("dup", "Twice"),
        ],
    );
    assert_eq!(impact.new_issues, vec!["V2 ring", "Twice"]);
    assert_eq!(impact.resolved_issues, vec!["V1 drill"]);
    Ok(())
}

#[test]
fn documented_json_examples_parse_and_rules_round_trip() -> Result<()> {
    let set: RulesCommand = serde_json::from_str(
        r#"{"command":"rules.set_override","params":{"key":{"rule":"clearance","a":"track","b":"via"},"value":{"kind":"length","value":0.15}}}"#,
    )
    .map_err(j)?;
    assert!(
        matches!(set, RulesCommand::SetOverride { key: RuleKey::Clearance { .. }, value: RuleValue::Length(v) } if v == 0.15)
    );
    let clear: RulesCommand = serde_json::from_str(
        r#"{"command":"rules.clear_override","params":{"key":{"rule":"mask_web_min"}}}"#,
    )
    .map_err(j)?;
    assert!(matches!(
        clear,
        RulesCommand::ClearOverride {
            key: RuleKey::MaskWebMin
        }
    ));
    let mut rules = proto()?;
    apply(&mut rules, set)?;
    apply(&mut rules, set_profile_cmd(fixtures::fine()?, true))?;
    let text = serde_json::to_string(&rules).map_err(j)?;
    let back: ProjectRules = serde_json::from_str(&text).map_err(j)?;
    assert_eq!(back, rules);
    Ok(())
}

#[test]
fn engine_skips_no_op_rules_commands_and_previews_refusals() -> Result<()> {
    use crate::{Engine, Operation, Query, QueryResult};
    let mut engine = Engine::demo()?;
    let rev = engine.revision();
    // The demo already uses the prototype fixture.
    engine.apply(
        rev,
        Operation::Rules(set_profile_cmd(fixtures::prototype()?, true)),
    )?;
    assert_eq!(engine.revision(), rev, "no change, no undo step");
    engine.apply(
        rev,
        Operation::Rules(RulesCommand::SetOverride {
            key: RuleKey::ViaDrillDefault,
            value: RuleValue::Length(0.1),
        }),
    )?;
    let rev = engine.revision();
    let QueryResult::ProfileImpact(impact) = engine.query(Query::PreviewProfile {
        profile: Box::new(fixtures::fine()?),
        keep_overrides: true,
    })?
    else {
        return Err(Error::Invalid("not an impact".into()));
    };
    assert!(impact.refusal.is_some());
    assert!(!impact.changes.is_empty());
    assert!(impact.new_issues.is_empty() && impact.resolved_issues.is_empty());
    assert!(
        engine
            .apply(
                rev,
                Operation::Rules(set_profile_cmd(fixtures::fine()?, true)),
            )
            .is_err()
    );
    assert_eq!(engine.revision(), rev);
    let QueryResult::ProfileImpact(impact) = engine.query(Query::PreviewProfile {
        profile: Box::new(fixtures::fine()?),
        keep_overrides: false,
    })?
    else {
        return Err(Error::Invalid("not an impact".into()));
    };
    assert_eq!(impact.refusal, None);
    assert_eq!(impact.dropped_overrides, vec![RuleKey::ViaDrillDefault]);
    assert!(
        impact
            .changes
            .iter()
            .any(|c| c.key == RuleKey::TrackWidthMin)
    );
    assert_eq!(engine.revision(), rev, "a preview changes nothing");
    let QueryResult::RuleProfiles(profiles) = engine.query(Query::RuleProfiles {})? else {
        return Err(Error::Invalid("not profiles".into()));
    };
    let names: Vec<_> = profiles.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["Test prototype"], "only the project's own");
    Ok(())
}
