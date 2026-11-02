//! The second live run's binding fault, reproduced without network: the
//! model imported the ESP32-S31-WROOM-3 pinout with the power, ground and
//! NC rows first, so the import's package terminals were in that order;
//! `library_package_from_template` then reused them by list order and
//! renumbered them from the module form (counter-clockwise rows 1–94,
//! EPAD 95, corner pads 96–99), so pad 6 landed a GND row and the model
//! had to re-key all 99 mappings with `library_edit`. The terminals now
//! carry over by number, so every pad lands its datasheet pin with no
//! correction step; position is only an explicit request option.
use super::*;
use pcbcraft_core::library::Definition;
use std::collections::BTreeMap;
use std::path::PathBuf;

/// A tool input of the reference replay (the hand-made WROOM-3 run whose
/// pinout rows are in pin-number order).
fn reference_input(tool: &str) -> Fallible<Value> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/esp32-s31-wroom-3/wroom3.replay.jsonl");
    for line in std::fs::read_to_string(path)?.lines() {
        let response: Value = serde_json::from_str(line)?;
        for block in response["content"].as_array().into_iter().flatten() {
            if block["type"] == "tool_use" && block["name"] == tool {
                return Ok(block["input"].clone());
            }
        }
    }
    Err(format!("no {tool} call in the reference replay").into())
}

/// The reference pinout with its rows in the live run's order: the 3V3,
/// GND and EN rows first, then the NC rows, then the GPIO rows. Pad order
/// (the module's counter-clockwise numbering) and terminal order now
/// differ.
fn reordered_pinout() -> Fallible<Value> {
    let mut document = reference_input("library_pinout_apply")?["document"].clone();
    let pins = document["pins"].as_array().cloned().ok_or("pins")?;
    let rank = |p: &Value| match (p["name"].as_str(), p["class"].as_str()) {
        (Some("GND" | "3V3" | "EN"), _) => 0,
        (_, Some("device_nc")) => 1,
        _ => 2,
    };
    let mut ordered = pins.clone();
    ordered.sort_by_key(rank);
    assert_ne!(ordered, pins, "the rows really are reordered");
    document["pins"] = Value::Array(ordered);
    Ok(document)
}

/// Each package terminal's number → the pinout row index it was imported
/// from (component terminals follow the rows in document order).
fn rows_by_number(document: &Value) -> Fallible<BTreeMap<String, usize>> {
    let mut out = BTreeMap::new();
    for (k, pin) in document["pins"]
        .as_array()
        .ok_or("pins")?
        .iter()
        .enumerate()
    {
        let number = pin["number"]["WROOM-3"].as_str().ok_or("number")?;
        out.insert(number.to_owned(), k);
    }
    Ok(out)
}

struct Built {
    session: Session,
    plan: Value,
}

/// Imports `document` and builds the module package with `terminals`
/// (`None`: the request's default), through the assistant's tools.
fn build(document: &Value, terminals: Option<&str>) -> Fallible<Built> {
    let mut session = Session::default();
    let mut workspace = tools::Workspace::default();
    let preview =
        json!({"target": {"target": "new", "name": "ESP32-S31-WROOM-3"}, "document": document});
    let plan = reply_json(tools::execute(
        &mut session,
        &mut workspace,
        "library_pinout_preview",
        &preview,
    ))?;
    let applied = reply_json(tools::execute(
        &mut session,
        &mut workspace,
        "library_pinout_apply",
        &json!({"base": plan["base"]}),
    ))?;
    let package = applied["drafts"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|d| d["kind"] == "package")
        .map(|d| d["id"].clone())
        .ok_or("a package draft")?;
    let mut request = reference_input("library_package_from_template")?;
    request["request"]["draft"] = package;
    if let Some(strategy) = terminals {
        request["request"]["terminals"] = json!(strategy);
    }
    let outcome = reply_json(tools::execute(
        &mut session,
        &mut workspace,
        "library_package_from_template",
        &request,
    ))?;
    Ok(Built {
        session,
        plan: outcome["outcome"]["plan"].clone(),
    })
}

/// For each footprint pad: its number and the pinout row index of the
/// component terminal the binding maps its package terminal to.
fn landed_rows(session: &Session) -> Fallible<Vec<(String, usize)>> {
    let drafts = &session.engine().drafts().items;
    let find = |pick: fn(&Definition) -> bool| {
        drafts
            .iter()
            .map(|d| &d.definition)
            .find(|d| pick(d))
            .ok_or("draft")
    };
    let Definition::Footprint(footprint) = find(|d| matches!(d, Definition::Footprint(_)))? else {
        return Err("footprint".into());
    };
    let Definition::Binding(binding) = find(|d| matches!(d, Definition::Binding(_)))? else {
        return Err("binding".into());
    };
    let Definition::Component(component) = find(|d| matches!(d, Definition::Component(_)))? else {
        return Err("component".into());
    };
    let mut out = Vec::new();
    for pad in &footprint.pads {
        let terminal = pad.terminal.ok_or("every pad lands a terminal")?;
        let mapped = binding
            .mappings
            .iter()
            .find(|m| m.package_terminal == terminal)
            .ok_or_else(|| format!("pad {} is unmapped", pad.number))?;
        let row = component
            .terminals
            .iter()
            .position(|t| t.id == mapped.terminal)
            .ok_or("a component terminal")?;
        out.push((pad.number.clone(), row));
    }
    Ok(out)
}

#[test]
fn the_wroom3_module_binds_every_pad_by_number_whatever_the_row_order() -> Fallible {
    let document = reordered_pinout()?;
    let rows = rows_by_number(&document)?;
    let Built { session, plan } = build(&document, None)?;

    assert_eq!(plan["terminals"]["strategy"], "by_number", "{plan}");
    assert_eq!(
        (
            &plan["terminals"]["kept"],
            &plan["terminals"]["added"],
            &plan["terminals"]["removed"]
        ),
        (&json!(99), &json!(0), &json!(0))
    );
    assert_eq!(
        plan["terminals"]["summary"],
        "Terminals matched by number: 99 kept, 0 added, 0 removed."
    );
    let codes: Vec<&str> = plan["findings"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|f| f["code"].as_str())
        .collect();
    for code in [
        "binding.mapping_retained",
        "binding.mapping_missing",
        "binding.mapping_unknown",
        "package.terminals_unmatched",
    ] {
        assert!(!codes.contains(&code), "{code} in {codes:?}");
    }

    // 94 edge pads, nine EPAD pads (pin 95) and four corner pads: each
    // lands the row the datasheet gives its number, with no library_edit.
    let landed = landed_rows(&session)?;
    assert_eq!(landed.len(), 107);
    for (number, row) in &landed {
        assert_eq!(rows.get(number), Some(row), "pad {number}");
    }
    let names: Vec<&str> = document["pins"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|p| p["name"].as_str().unwrap_or_default())
        .collect();
    let name_of = |pad: &str| {
        landed
            .iter()
            .find(|(n, _)| n == pad)
            .and_then(|(_, row)| names.get(*row).copied())
    };
    // Pad 6 landed a GND row in the live run; IO2 is its datasheet pin.
    assert_eq!(name_of("6"), Some("IO2"));
    assert_eq!(name_of("1"), Some("GND"));
    assert_eq!(name_of("95"), Some("GND"));

    // The drafts' own checks agree: no binding finding remains.
    let drafts: Vec<Value> = tools::drafts(&session.snapshot());
    for draft in &drafts {
        for finding in draft["findings"].as_array().into_iter().flatten() {
            let code = finding["code"].as_str().unwrap_or_default();
            assert!(!code.starts_with("binding."), "{draft}");
        }
    }
    Ok(())
}

#[test]
fn position_is_an_explicit_choice_and_says_so() -> Fallible {
    // The old behaviour, asked for: the same reordered import reuses the
    // terminals in list order, so pad 6 lands the third row (a GND), and
    // the plan says so instead of passing it off as a match.
    let document = reordered_pinout()?;
    let rows = rows_by_number(&document)?;
    let Built { session, plan } = build(&document, Some("by_position"))?;
    assert_eq!(plan["terminals"]["strategy"], "by_position");
    assert!(
        plan["terminals"]["summary"]
            .as_str()
            .is_some_and(|s| s.starts_with("Terminals matched by position: 99 kept"))
    );
    assert!(
        plan["findings"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|f| f["code"] == "binding.mapping_retained"),
        "{plan}"
    );
    let landed = landed_rows(&session)?;
    let wrong = landed
        .iter()
        .filter(|(number, row)| rows.get(number) != Some(row))
        .count();
    assert!(wrong > 0, "position mis-binds the reordered rows");
    Ok(())
}
