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
    // 2 track widths, 21 clearance pairs, 25 contract scalars, the 5
    // appended silkscreen-edge and land keys, the 2 etch-hazard keys and
    // the board outline fit.
    assert_eq!(RuleKey::all().len(), 2 + 21 + 25 + 5 + 2 + 1);
    let unique: std::collections::BTreeSet<_> = RuleKey::all().into_iter().collect();
    assert_eq!(unique.len(), RuleKey::all().len());
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

#[test]
fn appended_defaults_equal_the_estimator_constants() {
    use crate::library::package_form::tht;
    let r = DesignRules::default();
    assert_eq!(r.land.tht_drill_clearance, tht::DRILL_CLEARANCE_MM);
    assert_eq!(r.land.tht_land_extra, tht::RING_MM);
    assert_eq!(r.land.tht_land_gap, tht::LAND_GAP_MM);
    // `library::samples::TOE_HEEL_SHIFT_MM` (private) and no silkscreen
    // edge check before rules.
    assert_eq!(r.land.gull_wing_toe_shift, 0.1);
    assert_eq!(r.silkscreen.edge_clearance, 0.0);
    assert_eq!(r.land.gull_wing_width_extra, 0.2);
    assert_eq!(r.land.chip_gap_min, 0.2);
}

/// Every built-in keeps copper off the edge, and a fill computed under one
/// stays that far inside (`the_fill_keeps_the_profile_edge`).
/// The values are this project's estimates, in the 0.25–0.3 mm range routed
/// boards generally need; a zero here would mean "not checked".
#[test]
fn every_builtin_profile_keeps_copper_off_the_board_edge() {
    let expected = [
        ("pcbcraft-default", 0.3),
        ("std-6-6", 0.3),
        ("fine-4-4", 0.25),
        ("hdi-3-3", 0.25),
    ];
    let all = builtin::all();
    assert_eq!(all.len(), expected.len());
    for (profile, (id, edge)) in all.iter().zip(expected) {
        assert_eq!(profile.id, id);
        assert_eq!(profile.provenance.basis, ProfileBasis::Estimated, "{id}");
        assert!(profile.provenance.notes.contains("stimated"), "{id}");
        for object in [
            CopperObject::Track,
            CopperObject::Pad,
            CopperObject::Via,
            CopperObject::Fill,
            CopperObject::Hole,
        ] {
            assert_eq!(
                profile
                    .rules
                    .copper
                    .clearance
                    .get(object, CopperObject::BoardEdge),
                edge,
                "{id} {object:?}"
            );
        }
    }
}

#[test]
fn builtin_classes_tighten_in_order_and_fit_their_aspect_ratio() {
    let [_, std, fine, hdi] = [
        builtin::default_profile(),
        builtin::standard(),
        builtin::fine(),
        builtin::hdi(),
    ];
    for (loose, tight) in [(&std, &fine), (&fine, &hdi)] {
        let (l, t) = (&loose.rules, &tight.rules);
        assert!(t.copper.track_width_min < l.copper.track_width_min);
        assert!(
            t.copper
                .clearance
                .get(CopperObject::Track, CopperObject::Track)
                < l.copper
                    .clearance
                    .get(CopperObject::Track, CopperObject::Track)
        );
        assert!(t.holes.drill_min < l.holes.drill_min);
        assert!(t.holes.via_diameter_default < l.holes.via_diameter_default);
    }
    for p in [&std, &fine, &hdi] {
        let h = &p.rules.holes;
        let ratio = h.aspect_ratio_max.unwrap_or(f64::INFINITY);
        // A 1.6 mm board's default via is buildable under the profile.
        assert!(1.6 / h.via_drill_default <= ratio, "{}", p.name);
        assert_eq!(
            h.via_diameter_default,
            h.via_drill_default + 2.0 * h.via_annular_ring_min
        );
        assert!(p.provenance.notes.contains("stimated"));
        assert_eq!(builtin::by_id(&p.id).as_ref(), Some(p));
    }
    // 6 mil is 0.1524 mm.
    assert!((std.rules.copper.track_width_min - 0.1524).abs() < 1e-12);
    assert!(builtin::by_id("no-such-profile").is_none());
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
    let mut rules = ProjectRules::default();
    apply(
        &mut rules,
        RulesCommand::SetOverride {
            key: RuleKey::MaskWebMin,
            value: RuleValue::Length(0.12),
        },
    )?;
    let mut kept = rules.clone();
    apply(&mut kept, set_profile_cmd(builtin::fine(), true))?;
    assert_eq!(kept.profile.id, "fine-4-4");
    assert_eq!(kept.effective().mask.web_min, 0.12);
    assert_eq!(kept.source_of(RuleKey::MaskWebMin), RuleSource::Override);
    assert_eq!(
        kept.source_of(RuleKey::DrillMin).describe(),
        "Fine 4/4",
        "a rule without override names the profile"
    );
    let mut dropped = rules.clone();
    apply(&mut dropped, set_profile_cmd(builtin::fine(), false))?;
    assert!(dropped.overrides.is_empty());
    assert_eq!(dropped.effective(), builtin::fine().rules);
    let impact = rule_changes(&rules, &dropped);
    assert_eq!(impact.dropped_overrides, vec![RuleKey::MaskWebMin]);
    assert!(rule_changes(&rules, &kept).dropped_overrides.is_empty());
    Ok(())
}

#[test]
fn a_kept_override_that_breaks_the_new_profile_is_refused() -> Result<()> {
    let mut rules = ProjectRules::default();
    // A 0.1 mm via drill is fine for the default profile (0.1 mm minimum
    // drill) but below Standard 6/6's 0.3 mm minimum.
    apply(
        &mut rules,
        RulesCommand::SetOverride {
            key: RuleKey::ViaDrillDefault,
            value: RuleValue::Length(0.1),
        },
    )?;
    let before = rules.clone();
    let (rule, err) = refusal(apply(
        &mut rules,
        set_profile_cmd(builtin::standard(), true),
    ));
    assert_eq!(rule, "rules.override_conflict");
    assert!(err.contains("default via drill"), "{err}");
    assert_eq!(rules, before, "a refused switch changes nothing");
    let (candidate, refusal) = preview_candidate(&rules, &builtin::standard(), true);
    assert!(refusal.is_some_and(|r| r.contains("Standard 6/6")));
    assert_eq!(candidate.profile.id, "std-6-6");
    // Dropping the overrides makes the same switch valid.
    let (_, refusal) = preview_candidate(&rules, &builtin::standard(), false);
    assert_eq!(refusal, None);
    Ok(())
}

#[test]
fn a_modified_builtin_is_refused_and_a_user_profile_accepted() -> Result<()> {
    let mut rules = ProjectRules::default();
    let mut tampered = builtin::fine();
    tampered.rules.holes.drill_min = 0.15;
    let (rule, _) = refusal(apply(&mut rules, set_profile_cmd(tampered.clone(), false)));
    assert_eq!(rule, "rules.builtin_modified");
    assert!(rules.is_default());
    tampered.id = "my-fab".into();
    tampered.name = "My fab 4/4".into();
    tampered.provenance.origin = ProfileOrigin::User;
    tampered.provenance.basis = ProfileBasis::FabricatorEntered;
    tampered.provenance.source = "Entered by hand".into();
    apply(&mut rules, set_profile_cmd(tampered, false))?;
    assert_eq!(rules.effective().holes.drill_min, 0.15);
    assert_eq!(
        rules.resolve(RuleKey::DrillMin).source.describe(),
        "My fab 4/4"
    );
    // An invalid user profile is refused whole.
    let mut broken = rules.profile.clone();
    broken.rules.copper.track_width_default = 0.01;
    assert!(apply(&mut rules.clone(), set_profile_cmd(broken, false)).is_err());
    Ok(())
}

#[test]
fn bounds_name_the_rule() {
    let cases: [(RuleKey, RuleValue, &str); 6] = [
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
    ];
    for (key, value, label) in cases {
        let mut rules = ProjectRules::default();
        let (rule, err) = refusal(apply(&mut rules, RulesCommand::SetOverride { key, value }));
        assert_eq!(rule, "rules.bounds", "{err}");
        assert!(err.contains(label), "{err}");
        assert!(rules.is_default());
    }
    // Negative mask expansion shrinks the opening and is allowed.
    let mut rules = ProjectRules::default();
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
    let mut rules = ProjectRules::default();
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
        object: CopperObject::Hole,
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
fn issue_diff_matches_by_identity_and_counts_repeats() {
    let mut impact = rule_changes(&ProjectRules::default(), &ProjectRules::default());
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
}

#[test]
fn documented_json_examples_parse_and_rules_round_trip()
-> std::result::Result<(), serde_json::Error> {
    let set: RulesCommand = serde_json::from_str(
        r#"{"command":"rules.set_override","params":{"key":{"rule":"clearance","a":"track","b":"via"},"value":{"kind":"length","value":0.15}}}"#,
    )?;
    assert!(
        matches!(set, RulesCommand::SetOverride { key: RuleKey::Clearance { .. }, value: RuleValue::Length(v) } if v == 0.15)
    );
    let clear: RulesCommand = serde_json::from_str(
        r#"{"command":"rules.clear_override","params":{"key":{"rule":"mask_web_min"}}}"#,
    )?;
    assert!(matches!(
        clear,
        RulesCommand::ClearOverride {
            key: RuleKey::MaskWebMin
        }
    ));
    let mut rules = ProjectRules::default();
    let _ = apply(&mut rules, set);
    let _ = apply(&mut rules, set_profile_cmd(builtin::hdi(), true));
    assert_eq!(rules.profile.id, "hdi-3-3");
    let text = serde_json::to_string(&rules)?;
    let back: ProjectRules = serde_json::from_str(&text)?;
    assert_eq!(back, rules);
    let file = ProfileFile {
        format: PROFILE_FILE_FORMAT,
        profile: builtin::standard(),
    };
    let back: ProfileFile = serde_json::from_str(&serde_json::to_string(&file)?)?;
    assert_eq!(back, file);
    // Unknown rule fields are refused, not dropped.
    let mut value = serde_json::to_value(builtin::standard())?;
    if let Some(copper) = value
        .pointer_mut("/rules/copper")
        .and_then(|v| v.as_object_mut())
    {
        copper.insert("future_rule".into(), serde_json::json!(1.0));
    }
    assert!(serde_json::from_value::<RuleProfile>(value).is_err());
    Ok(())
}

#[test]
fn engine_skips_no_op_rules_commands_and_previews_refusals() -> Result<()> {
    use crate::{Engine, Operation, Query, QueryResult};
    let mut engine = Engine::demo()?;
    let rev = engine.revision();
    // The project already uses the default profile.
    engine.apply(
        rev,
        Operation::Rules(set_profile_cmd(builtin::default_profile(), true)),
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
        profile: Box::new(builtin::standard()),
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
                Operation::Rules(set_profile_cmd(builtin::standard(), true)),
            )
            .is_err()
    );
    assert_eq!(engine.revision(), rev);
    let QueryResult::ProfileImpact(impact) = engine.query(Query::PreviewProfile {
        profile: Box::new(builtin::standard()),
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
    assert_eq!(
        names,
        ["Pcbcraft default", "Standard 6/6", "Fine 4/4", "HDI 3/3"]
    );
    Ok(())
}
