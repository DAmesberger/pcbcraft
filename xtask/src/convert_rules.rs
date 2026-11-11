//! `convert-v1`'s rule-profile step: rule profiles became files with a
//! stated source, every rule named (`pcbcraft_core::rules::profile`), and
//! Pcbcraft stopped shipping estimated built-in profiles.
//!
//! A project's old snapshot (`{id, name, provenance, rules: {copper: …}}`)
//! is replaced by the `--profile FILE.json` given, keeping the project's
//! overrides. Without one, a snapshot the user entered from a fabricator
//! (`basis: fabricator_entered` with a source and a date) is rewritten
//! into the new form; an estimated built-in one is refused, naming it. A
//! project with no saved rules (the old implicit default) gets the
//! `--profile` given, or stays without a profile. Old profile library
//! files (`rule-profiles/*.json`) convert the same way, and become fab
//! profile files: the file's `profile` is now `fab_profile` (move them to
//! `fab-profiles/`).
//!
//! The hole split: the one `hole` clearance object became `via_hole` (via
//! drills and unplated holes) and `pth_hole` (plated pad and mounting
//! holes). Every `clearance_*_hole` rule of a profile, and every project
//! override of a `hole` pair, becomes one per hole kind with the same
//! value and source, so a converted board is checked as before; enter the
//! fabricator's plated-hole values afterwards.
use crate::Result;
use serde_json::{Map, Value, json};

/// Rules the old profiles did not hold, with the value the code used.
const ADDED: &[(&str, &str, &str)] = &[
    (
        "silk_text_stroke_share",
        "formula",
        "Pcbcraft's former fixed legend stroke share, carried over by convert-v1.",
    ),
    (
        "pair_gap_tolerance",
        "0.02",
        "Pcbcraft's former coupled-gap tolerance floor, carried over by convert-v1.",
    ),
    (
        "pair_gap_tolerance_share",
        "0.1",
        "Pcbcraft's former coupled-gap tolerance share, carried over by convert-v1.",
    ),
];

/// Rewrite the project's rules; 1 if they changed.
pub fn convert_project_rules(
    project: &mut Map<String, Value>,
    pinned: Option<&Value>,
) -> Result<usize> {
    let rules = project
        .entry("rules")
        .or_insert_with(|| Value::Object(Map::new()));
    let rules = rules
        .as_object_mut()
        .ok_or("the project's rules are not an object")?;
    match rules.get("profile") {
        Some(old) if old.get("provenance").is_some() => {
            let new = match pinned {
                Some(profile) => profile.clone(),
                None => convert_profile(old)?,
            };
            rules.insert("profile".into(), new);
            Ok(1 + split_project_holes(rules))
        }
        Some(_) => Ok(split_project_holes(rules)),
        None => match pinned {
            Some(profile) => {
                rules.insert("profile".into(), profile.clone());
                Ok(1)
            }
            None => {
                if rules.is_empty() {
                    project.remove("rules");
                }
                Ok(0)
            }
        },
    }
}

/// An old profile library file (`{format, profile}`) as a fab profile
/// file (`{format, fab_profile}`); the number of changes.
pub fn convert_profile_file(file: &mut Value) -> Result<usize> {
    let object = file
        .as_object_mut()
        .ok_or("not a fab profile file: it is not an object")?;
    let mut n = 0;
    if let Some(old) = object.remove("profile") {
        object.insert("fab_profile".into(), old);
        n += 1;
    }
    let profile = object
        .get_mut("fab_profile")
        .ok_or("not a fab profile file: it has no fab_profile")?;
    if profile.get("provenance").is_some() {
        *profile = convert_profile(profile)?;
        return Ok(n + 1);
    }
    Ok(n + split_holes(profile))
}

/// The hole split in a project's rules: its profile snapshot's rules and
/// its overrides of `hole` pairs; the number of changes.
fn split_project_holes(rules: &mut Map<String, Value>) -> usize {
    let mut n = rules.get_mut("profile").map_or(0, split_holes);
    let Some(overrides) = rules.get_mut("overrides").and_then(Value::as_array_mut) else {
        return n;
    };
    let mut out = Vec::new();
    for o in overrides.drain(..) {
        let key = &o["key"];
        let (a, b) = (key["a"].as_str(), key["b"].as_str());
        if key["rule"] != "clearance" || (a != Some("hole") && b != Some("hole")) {
            out.push(o);
            continue;
        }
        n += 1;
        fn kinds(s: Option<&str>) -> Vec<String> {
            match s {
                Some("hole") => vec!["via_hole".into(), "pth_hole".into()],
                Some(other) => vec![other.into()],
                None => vec![],
            }
        }
        let (a, b) = (kinds(a), kinds(b));
        let mut seen = Vec::new();
        for x in &a {
            for y in &b {
                let pair = if x <= y { (x, y) } else { (y, x) };
                if seen.contains(&pair) {
                    continue;
                }
                seen.push(pair);
                let mut split = o.clone();
                split["key"]["a"] = x.as_str().into();
                split["key"]["b"] = y.as_str().into();
                out.push(split);
            }
        }
    }
    *overrides = out;
    n
}

/// A profile's `clearance_<x>_hole` rules, one per hole kind (same value
/// and source); the number of rules split.
pub fn split_holes(profile: &mut Value) -> usize {
    let Some(rules) = profile.get_mut("rules").and_then(Value::as_object_mut) else {
        return 0;
    };
    let mut n = 0;
    for x in ["track", "pad", "via", "fill", "board_edge"] {
        if let Some(entry) = rules.remove(&format!("clearance_{x}_hole")) {
            rules.insert(format!("clearance_{x}_via_hole"), entry.clone());
            rules.insert(format!("clearance_{x}_pth_hole"), entry);
            n += 1;
        }
    }
    if let Some(entry) = rules.remove("clearance_hole_hole") {
        for pair in [
            "via_hole_via_hole",
            "via_hole_pth_hole",
            "pth_hole_pth_hole",
        ] {
            rules.insert(format!("clearance_{pair}"), entry.clone());
        }
        n += 1;
    }
    n
}

/// An old profile the user entered from a fabricator, in the new form.
pub fn convert_profile(old: &Value) -> Result<Value> {
    let name = old["name"].as_str().unwrap_or_default();
    let p = &old["provenance"];
    let text = |key: &str| p[key].as_str().unwrap_or_default().to_owned();
    if p["basis"] != "fabricator_entered" || text("source").is_empty() || text("date").is_empty() {
        return Err(format!(
            "the profile {name:?} is an estimate without a source and date; \
             pass --profile FILE.json naming a sourced profile (examples/profiles/)"
        )
        .into());
    }
    let mut source = Map::new();
    source.insert("publisher".into(), text("source").into());
    for key in ["url", "notes"] {
        if !text(key).is_empty() {
            source.insert(key.into(), text(key).into());
        }
    }
    source.insert("date".into(), text("date").into());
    let mut rules = Map::new();
    for (name, value) in old_values(&old["rules"])? {
        rules.insert(name, json!({"value": value}));
    }
    for (name, kind, why) in ADDED {
        let entry = if *kind == "formula" {
            json!({"formula": "1.0 / 7.0", "source": why})
        } else {
            json!({"value": kind.parse::<f64>().unwrap_or_default(), "source": why})
        };
        rules.entry(name.to_string()).or_insert(entry);
    }
    let mut new = json!({
        "id": old["id"],
        "name": old["name"],
        "source": source,
        "drill_files": {
            "kind": "not_published",
            "source": "Converted by convert-v1: the old profile did not say how the \
                       fabricator reads drill files; check and set it."
        },
        "rules": rules,
    });
    split_holes(&mut new);
    Ok(new)
}

/// Every value of an old `DesignRules` object by its rule name.
fn old_values(rules: &Value) -> Result<Vec<(String, Value)>> {
    const PATHS: &[(&str, &str)] = &[
        ("/copper/track_width_min", "track_width_min"),
        ("/copper/track_width_default", "track_width_default"),
        ("/holes/drill_min", "drill_min"),
        ("/holes/via_annular_ring_min", "via_annular_ring_min"),
        ("/holes/pth_annular_ring_min", "pth_annular_ring_min"),
        ("/holes/aspect_ratio_max", "aspect_ratio_max"),
        ("/holes/via_drill_default", "via_drill_default"),
        ("/holes/via_diameter_default", "via_diameter_default"),
        ("/mask/expansion", "mask_expansion"),
        ("/mask/web_min", "mask_web_min"),
        ("/mask/tent_vias", "tent_vias"),
        ("/silkscreen/line_width_min", "silk_line_width_min"),
        ("/silkscreen/text_height_min", "silk_text_height_min"),
        ("/silkscreen/pad_clearance", "silk_pad_clearance"),
        ("/silkscreen/edge_clearance", "silk_edge_clearance"),
        ("/paste/reduction", "paste_reduction"),
        ("/courtyard/clearance", "courtyard_clearance"),
        ("/fill/clearance", "fill_clearance"),
        ("/fill/width_min", "fill_width_min"),
        ("/fill/thermal/spoke_width", "thermal_spoke_width"),
        ("/fill/thermal/spoke_count", "thermal_spoke_count"),
        ("/fill/thermal/gap", "thermal_gap"),
        ("/fill/thermal/angle_deg", "thermal_angle"),
        ("/fill/connection", "fill_connection"),
        ("/fill/island_area_min", "island_area_min"),
        ("/land/gull_wing_width_extra", "gull_wing_width_extra"),
        ("/land/chip_gap_min", "chip_gap_min"),
        ("/land/exposed_pad_clearance", "exposed_pad_clearance"),
        ("/land/gull_wing_toe_shift", "gull_wing_toe_shift"),
        ("/land/tht_drill_clearance", "tht_drill_clearance"),
        ("/land/tht_land_extra", "tht_land_extra"),
        ("/land/tht_land_gap", "tht_land_gap"),
    ];
    // Left out of old files when unset: etch checks 0, outline fit 0.01.
    const OPTIONAL: &[(&str, &str, f64)] = &[
        ("/etch/copper_angle_min", "copper_angle_min", 0.0),
        ("/etch/sliver_width_min", "sliver_width_min", 0.0),
        ("/board/outline_fit", "board_outline_fit", 0.01),
    ];
    let mut out = Vec::new();
    for (path, name) in PATHS {
        let value = rules
            .pointer(path)
            .ok_or_else(|| format!("the old profile has no {path}"))?;
        out.push(((*name).to_owned(), value.clone()));
    }
    for (path, name, default) in OPTIONAL {
        let value = rules.pointer(path).cloned().unwrap_or(json!(default));
        out.push(((*name).to_owned(), value));
    }
    for pair in rules
        .pointer("/copper/clearance/pairs")
        .and_then(Value::as_array)
        .ok_or("the old profile has no clearance pairs")?
    {
        let (Some(a), Some(b)) = (pair["a"].as_str(), pair["b"].as_str()) else {
            return Err("a clearance pair has no objects".into());
        };
        out.push((format!("clearance_{a}_{b}"), pair["mm"].clone()));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn old(basis: &str) -> Value {
        let pairs: Vec<Value> = ["track", "pad", "via", "fill", "board_edge", "hole"]
            .iter()
            .enumerate()
            .flat_map(|(i, a)| {
                ["track", "pad", "via", "fill", "board_edge", "hole"][i..]
                    .iter()
                    .map(move |b| json!({"a": a, "b": b, "mm": 0.2}))
            })
            .collect();
        json!({
            "id": "my-fab", "name": "My fab",
            "provenance": {"origin": "user", "basis": basis, "source": "My Fab capabilities",
                           "url": "https://example.invalid/caps", "date": "2026-10-30", "notes": ""},
            "rules": {
                "copper": {"track_width_min": 0.1, "track_width_default": 0.25,
                           "clearance": {"pairs": pairs}},
                "holes": {"drill_min": 0.2, "via_annular_ring_min": 0.1, "pth_annular_ring_min": 0.15,
                          "aspect_ratio_max": null, "via_drill_default": 0.3, "via_diameter_default": 0.6},
                "mask": {"expansion": 0.05, "web_min": 0.1, "tent_vias": true},
                "silkscreen": {"line_width_min": 0.15, "text_height_min": 1.0,
                               "pad_clearance": 0.15, "edge_clearance": 0.0},
                "paste": {"reduction": 0.0}, "courtyard": {"clearance": 0.0},
                "fill": {"clearance": 0.2, "width_min": 0.2,
                         "thermal": {"spoke_width": 0.3, "spoke_count": 4, "gap": 0.25, "angle_deg": 45.0},
                         "connection": "thermal", "island_area_min": 0.0},
                "land": {"gull_wing_width_extra": 0.2, "chip_gap_min": 0.2, "exposed_pad_clearance": 0.2,
                         "gull_wing_toe_shift": 0.1, "tht_drill_clearance": 0.3, "tht_land_extra": 0.6,
                         "tht_land_gap": 0.25}
            }
        })
    }

    #[test]
    fn a_fabricator_profile_converts_with_every_rule_named() -> Result<()> {
        let new = convert_profile(&old("fabricator_entered"))?;
        assert_eq!(new["source"]["publisher"], "My Fab capabilities");
        assert_eq!(new["source"]["date"], "2026-10-30");
        let rules = new["rules"].as_object().ok_or("rules")?;
        // 28 clearance pairs (the old hole pairs split by hole kind) and
        // 38 other rules.
        assert_eq!(rules.len(), 66);
        assert_eq!(rules["drill_min"], json!({"value": 0.2}));
        assert_eq!(rules["clearance_track_board_edge"], json!({"value": 0.2}));
        assert_eq!(rules["board_outline_fit"], json!({"value": 0.01}));
        assert_eq!(rules["clearance_track_pth_hole"], json!({"value": 0.2}));
        assert_eq!(rules["clearance_via_hole_pth_hole"], json!({"value": 0.2}));
        assert!(rules.get("clearance_track_hole").is_none());
        assert_eq!(rules["aspect_ratio_max"], json!({"value": null}));
        assert!(rules["pair_gap_tolerance"]["source"].is_string());
        assert!(new.get("provenance").is_none());
        Ok(())
    }

    #[test]
    fn an_estimated_profile_needs_a_sourced_one() -> Result<()> {
        let mut project = json!({"rules": {"profile": old("estimated"), "overrides": [1]}});
        let map = project.as_object_mut().ok_or("object")?;
        assert!(convert_project_rules(map, None).is_err());
        let pinned = json!({"id": "fab", "name": "Fab"});
        assert_eq!(convert_project_rules(map, Some(&pinned))?, 1);
        assert_eq!(project["rules"]["profile"], pinned);
        assert_eq!(project["rules"]["overrides"], json!([1]), "overrides kept");
        let map = project.as_object_mut().ok_or("object")?;
        assert_eq!(convert_project_rules(map, Some(&pinned))?, 0, "once");
        Ok(())
    }

    #[test]
    fn the_hole_rules_split_by_hole_kind_once() -> Result<()> {
        let override_of = |a: &str, b: &str| {
            json!({"key": {"rule": "clearance", "a": a, "b": b},
                   "value": {"kind": "length", "value": 0.4}})
        };
        let mut project = json!({"rules": {
            "profile": {"id": "fab", "rules": {
                "clearance_track_hole": {"value": 0.2, "source": "Via hole to Track"},
                "clearance_hole_hole": {"value": 0.25},
                "clearance_track_track": {"value": 0.1}}},
            "overrides": [override_of("track", "hole"), override_of("hole", "hole"),
                          override_of("track", "pad")]}});
        let map = project.as_object_mut().ok_or("object")?;
        assert_eq!(convert_project_rules(map, None)?, 4);
        let rules = &project["rules"]["profile"]["rules"];
        assert_eq!(
            rules["clearance_track_pth_hole"],
            json!({"value": 0.2, "source": "Via hole to Track"})
        );
        assert_eq!(rules["clearance_track_via_hole"]["value"], 0.2);
        assert_eq!(rules["clearance_pth_hole_pth_hole"]["value"], 0.25);
        assert_eq!(rules["clearance_track_track"]["value"], 0.1);
        assert_eq!(rules.as_object().ok_or("rules")?.len(), 6);
        let overrides = project["rules"]["overrides"]
            .as_array()
            .ok_or("overrides")?
            .clone();
        assert_eq!(
            overrides,
            vec![
                override_of("track", "via_hole"),
                override_of("track", "pth_hole"),
                override_of("via_hole", "via_hole"),
                override_of("via_hole", "pth_hole"),
                override_of("pth_hole", "pth_hole"),
                override_of("track", "pad"),
            ]
        );
        let map = project.as_object_mut().ok_or("object")?;
        assert_eq!(convert_project_rules(map, None)?, 0, "once");
        Ok(())
    }

    #[test]
    fn a_profile_file_becomes_a_fab_profile_file_once() -> Result<()> {
        let mut file = json!({"format": 1, "profile": {"id": "fab", "rules": {
            "clearance_track_hole": {"value": 0.2}}}});
        assert_eq!(convert_profile_file(&mut file)?, 2);
        assert!(file.get("profile").is_none());
        assert_eq!(
            file["fab_profile"]["rules"]["clearance_track_pth_hole"],
            json!({"value": 0.2})
        );
        assert_eq!(convert_profile_file(&mut file)?, 0, "once");
        Ok(())
    }

    #[test]
    fn a_project_without_rules_takes_the_given_profile_or_none() -> Result<()> {
        let mut bare = json!({"name": "x"});
        let map = bare.as_object_mut().ok_or("object")?;
        assert_eq!(convert_project_rules(map, None)?, 0);
        assert_eq!(bare, json!({"name": "x"}));
        let pinned = json!({"id": "fab"});
        let map = bare.as_object_mut().ok_or("object")?;
        assert_eq!(convert_project_rules(map, Some(&pinned))?, 1);
        assert_eq!(bare["rules"]["profile"], pinned);
        Ok(())
    }
}
