//! Slice C7: JSON-only scenario of a datasheet pinout import (C3, with the
//! C1 rule scripts it carries), as an agent drives it through the MCP
//! instructions: schema → preview → show the review items → apply → the
//! F8 publish gate → a corrected re-import → the person's confirmation →
//! publish and place. `firmware_tests` continues the same scenario on the
//! placed instance (assign → report → export).
//!
//! The fixture `mcp/exm48.pinout.json` is original: the EXM-48 controller
//! is fictitious, and its pins, peripherals, mux codes, pages and rules are
//! invented for this test.
//!
//! Every step is asserted strictly. The pins track (C1–C4) is merged, so
//! no step of this scenario may end on a contract stub
//! (`mcp::scenario::STUBS` now lists only the template and model stubs).
use super::*;
use std::collections::BTreeMap;

pub(super) use crate::mcp::scenario::{EXM48, Fallible, Stub, finish, finish_stubbed, stub_in};

/// One session driven only by JSON requests. The engine is read only to
/// prove that refusals changed nothing.
pub(super) struct Scenario {
    pub session: Session,
}

impl Scenario {
    pub fn demo() -> Fallible<Self> {
        Ok(Self {
            session: Session::new(Engine::demo()?),
        })
    }

    pub fn revision(&self) -> u64 {
        self.session.engine().revision()
    }

    /// Dispatch one request that must succeed; a stub refusal stops the
    /// scenario (after proving it changed nothing).
    pub fn send(&mut self, step: &str, request: Value) -> Fallible<Value> {
        let before = self.session.engine().document().clone();
        let revision = self.revision();
        let reply = self.session.dispatch(request);
        if reply["ok"] == true {
            return Ok(reply["result"].clone());
        }
        assert_eq!(
            self.revision(),
            revision,
            "{step}: a refusal changed the revision"
        );
        assert_eq!(
            self.session.engine().document(),
            &before,
            "{step}: a refusal changed the document"
        );
        match stub_in(&reply["error"]) {
            Some(rule) => Err(Box::new(Stub {
                step: step.into(),
                rule: rule.into(),
            })),
            None => Err(format!("{step}: {reply}").into()),
        }
    }

    /// `project.apply` of an operation that must parse as the contract's
    /// `Operation` before it is sent.
    pub fn apply(&mut self, step: &str, operation: Value) -> Fallible<Value> {
        serde_json::from_value::<Operation>(operation.clone())
            .map_err(|e| format!("{step}: not a contract operation: {e}"))?;
        let revision = self.revision();
        let result = self.send(
            step,
            json!({"command": "project.apply", "params": {
                "expected_revision": revision, "operation": operation}}),
        )?;
        assert_eq!(self.revision(), revision + 1, "{step}: one undo step");
        Ok(result)
    }

    /// `project.query` of a query that must parse as the contract's
    /// `Query`; returns the report body and proves nothing changed.
    pub fn query(&mut self, step: &str, name: &str, params: Value) -> Fallible<Value> {
        let request = json!({"query": name, "params": params});
        serde_json::from_value::<Query>(request.clone())
            .map_err(|e| format!("{step}: not a contract query: {e}"))?;
        let revision = self.revision();
        let result = self.send(step, json!({"command": "project.query", "params": request}))?;
        assert_eq!(self.revision(), revision, "{step}: queries change nothing");
        Ok(result["result"].clone())
    }

    /// A request that must be refused with `code` (not by a stub) and
    /// change nothing. Returns the error.
    pub fn refuse(&mut self, step: &str, request: Value, code: &str) -> Fallible<Value> {
        match self.send(step, request) {
            Ok(result) => Err(format!("{step}: accepted, expected {code}: {result}").into()),
            Err(error) if error.is::<Stub>() => Err(error),
            Err(error) => {
                let text = error.to_string();
                assert!(
                    text.contains(&format!("\"code\":\"{code}\"")),
                    "{step}: expected {code}, got {text}"
                );
                Ok(json!(text))
            }
        }
    }

    /// `project.apply` that must be refused with `code`.
    pub fn refuse_apply(&mut self, step: &str, operation: Value, code: &str) -> Fallible<Value> {
        serde_json::from_value::<Operation>(operation.clone())
            .map_err(|e| format!("{step}: not a contract operation: {e}"))?;
        let revision = self.revision();
        self.refuse(
            step,
            json!({"command": "project.apply", "params": {
                "expected_revision": revision, "operation": operation}}),
            code,
        )
    }

    /// The current snapshot's draft of `id`: `{draft, check}`.
    pub fn draft(&mut self, id: u64) -> Fallible<Value> {
        let snapshot = self.send(
            "inspect",
            json!({"command": "project.inspect", "params": {}}),
        )?;
        snapshot["drafts"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|d| d["draft"]["id"] == id)
            .cloned()
            .ok_or_else(|| format!("no draft {id}").into())
    }
}

/// The finding codes of one draft check.
pub(super) fn codes(check: &Value) -> Vec<String> {
    check["result"]["findings"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|f| f["code"].as_str().map(str::to_owned))
        .collect()
}

pub(super) fn exm48() -> Fallible<Value> {
    Ok(serde_json::from_str(EXM48)?)
}

/// The document with row `r20` corrected the way the person confirmed it
/// against a clean copy of the datasheet: AF1 stands, the flag goes.
pub(super) fn exm48_corrected() -> Fallible<Value> {
    let mut document = exm48()?;
    let row = document["pins"]
        .as_array_mut()
        .and_then(|pins| pins.iter_mut().find(|p| p["row"] == "r20"))
        .ok_or("row r20")?;
    row["uncertain"] = json!(false);
    row["note"] = Value::Null;
    Ok(document)
}

pub(super) fn library_op(command: &str, params: Value) -> Value {
    json!({"editor": "Library", "operation": {"command": command, "params": params}})
}

pub(super) fn instance_op(command: &str, params: Value) -> Value {
    json!({"editor": "Instance", "operation": {"command": command, "params": params}})
}

pub(super) fn schematic_op(command: &str, params: Value) -> Value {
    json!({"editor": "Schematic", "operation": {"command": command, "params": params}})
}

/// The component draft an import produced, by label.
pub(super) struct Imported {
    pub component: u64,
    pub meta: Value,
    pub body: Value,
    /// Terminal ID by name.
    pub terminals: BTreeMap<String, u64>,
    /// Peripheral ID and its role IDs by label.
    pub peripherals: BTreeMap<String, (u64, BTreeMap<String, u64>)>,
    /// The block symbol the import drew for the component.
    pub symbol: u64,
}

impl Imported {
    fn read(draft: &Value) -> Fallible<Self> {
        let definition = &draft["draft"]["definition"];
        assert_eq!(definition["kind"], "component", "{definition}");
        let body = definition["body"].clone();
        let id = |v: &Value| v.as_u64().ok_or("id");
        let mut terminals = BTreeMap::new();
        for t in body["terminals"].as_array().ok_or("terminals")? {
            terminals.insert(t["name"].as_str().ok_or("name")?.to_owned(), id(&t["id"])?);
        }
        let mut peripherals = BTreeMap::new();
        for r in body["resources"].as_array().ok_or("resources")? {
            let mut roles = BTreeMap::new();
            for role in r["roles"].as_array().ok_or("roles")? {
                roles.insert(
                    role["label"].as_str().ok_or("label")?.to_owned(),
                    id(&role["id"])?,
                );
            }
            peripherals.insert(
                r["label"].as_str().ok_or("label")?.to_owned(),
                (id(&r["id"])?, roles),
            );
        }
        let symbol = body["representations"][0]["id"]
            .as_u64()
            .ok_or("the imported symbol")?;
        Ok(Self {
            component: id(&draft["draft"]["id"])?,
            meta: draft["draft"]["meta"].clone(),
            body,
            terminals,
            peripherals,
            symbol,
        })
    }

    pub fn terminal(&self, name: &str) -> Fallible<u64> {
        self.terminals
            .get(name)
            .copied()
            .ok_or_else(|| format!("terminal {name}").into())
    }

    pub fn peripheral(&self, label: &str) -> Fallible<u64> {
        Ok(self
            .peripherals
            .get(label)
            .ok_or_else(|| format!("peripheral {label}"))?
            .0)
    }

    pub fn role(&self, peripheral: &str, role: &str) -> Fallible<u64> {
        self.peripherals
            .get(peripheral)
            .and_then(|(_, roles)| roles.get(role))
            .copied()
            .ok_or_else(|| format!("role {peripheral}.{role}").into())
    }
}

/// The `library.pinout_preview` query parameters.
pub(super) fn preview_params(target: Value, document: &Value) -> Value {
    json!({"target": target, "document": document})
}

/// The `library.pinout_apply` operation.
pub(super) fn apply_op(target: Value, document: &Value, base: &Value, accept: Value) -> Value {
    library_op(
        "library.pinout_apply",
        json!({"target": target, "document": document, "base": base, "accept": accept}),
    )
}

fn rows_of(changes: &Value, object: &str, kind: &str) -> Vec<String> {
    changes
        .as_array()
        .into_iter()
        .flatten()
        .filter(|c| c["object"] == object && c["kind"] == kind)
        .filter_map(|c| c["row"].as_str().map(str::to_owned))
        .collect()
}

fn count(changes: &Value, object: &str, kind: &str) -> usize {
    changes
        .as_array()
        .into_iter()
        .flatten()
        .filter(|c| c["object"] == object && c["kind"] == kind)
        .count()
}

/// Steps 1–7: the agent reads the schema, previews the EXM-48 document,
/// shows the review, applies it, meets the F8 gate, re-imports the
/// corrected row and the person confirms the review. Returns the draft,
/// ready to publish.
pub(super) fn import_reviewed(s: &mut Scenario) -> Fallible<Imported> {
    // 1. The schema the agent fills.
    let schema = s.send(
        "schema",
        json!({"command": "library.pinout_schema", "params": {}}),
    )?;
    assert_eq!(schema["format"], 1);
    let document = exm48()?;
    assert_eq!(document["format"], schema["format"]);

    // 2. The preview: every row planned, the one uncertain row up for review.
    let target = json!({"target": "new", "name": "EXM-48"});
    let plan = s.query(
        "pinout_preview",
        "library.pinout_preview",
        preview_params(target.clone(), &document),
    )?;
    let changes = &plan["changes"];
    let mut added = rows_of(changes, "terminal", "added");
    added.sort_by_key(|r| r[1..].parse::<u32>().unwrap_or(u32::MAX));
    let expected: Vec<String> = (1..=49).map(|n| format!("r{n}")).collect();
    assert_eq!(added, expected, "one terminal per row: {plan}");
    assert_eq!(count(changes, "peripheral", "added"), 8, "{plan}");
    assert_eq!(count(changes, "rule", "added"), 3, "{plan}");
    assert_eq!(count(changes, "terminal", "removed"), 0, "{plan}");
    let review = plan["review"].as_array().ok_or("review")?;
    assert_eq!(review.len(), 1, "only the flagged row: {plan}");
    assert_eq!(review[0]["row"], "r20");
    assert_eq!(review[0]["reason"], "uncertain");
    if let Some(rule) = stub_in(&plan["findings"]) {
        return Err(Box::new(Stub {
            step: "pinout_preview findings".into(),
            rule: rule.into(),
        }));
    }
    let finding_codes: Vec<&str> = plan["findings"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|f| f["code"].as_str())
        .collect();
    assert!(finding_codes.contains(&"pinout.uncertain"), "{plan}");
    assert!(
        !finding_codes.iter().any(|c| c.starts_with("rule.")),
        "the three rule scripts compile and resolve: {plan}"
    );

    // 3. A plan whose base no longer matches is refused as a conflict that
    // names the stale-plan rule (the base is a content token, not the
    // document revision); nothing changes.
    let stale = json!(plan["base"].as_u64().ok_or("base")? + 1);
    let refusal = s.refuse_apply(
        "pinout_apply stale",
        apply_op(target.clone(), &document, &stale, json!({"accept": "all"})),
        "conflict",
    )?;
    assert!(
        refusal.to_string().contains("pinout.stale_base"),
        "{refusal}"
    );

    // 4. Apply after the person saw the review: one undo step, a draft whose
    // every imported row is unreviewed.
    let applied = s.apply(
        "pinout_apply",
        apply_op(target, &document, &plan["base"], json!({"accept": "all"})),
    )?;
    let outcome = &applied["outcome"];
    assert_eq!(outcome["outcome"], "pinout_applied", "{outcome}");
    let report = &outcome["report"];
    let drafts: Vec<u64> = report["drafts"]
        .as_array()
        .ok_or("drafts")?
        .iter()
        .filter_map(Value::as_u64)
        .collect();
    let component = drafts.first().copied().ok_or("component first")?;
    // The component, its block symbol, and the package and binding of its
    // one variant (their pads come in the footprint step, `templates_tests`).
    assert_eq!(drafts.len(), 4, "{report}");
    let mut unreviewed: Vec<String> = report["unreviewed"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| r.as_str().map(str::to_owned))
        .collect();
    unreviewed.sort_by_key(|r| r[1..].parse::<u32>().unwrap_or(u32::MAX));
    assert_eq!(unreviewed, expected, "every imported row is unreviewed");
    let after_apply = s.session.engine().document().clone();
    s.send("undo", json!({"command": "project.undo", "params": {}}))?;
    assert!(
        s.draft(component).is_err(),
        "undo removes the import in one step"
    );
    s.send("redo", json!({"command": "project.redo", "params": {}}))?;
    assert_eq!(s.session.engine().document(), &after_apply);

    let draft = s.draft(component)?;
    let imported = Imported::read(&draft)?;
    assert_eq!(imported.terminals.len(), 49);
    assert_eq!(imported.peripherals.len(), 8);
    assert_eq!(imported.body["pin_rules"].as_array().map(Vec::len), Some(3));
    // Explicit data, never parsed from names: PA9's GPIO and USART1 TX's mux.
    let pa9 = imported.body["terminals"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|t| t["name"] == "PA9")
        .ok_or("PA9")?;
    let functions = &pa9["class"]["electrical"];
    assert_eq!(functions["gpio"], json!({"port": "A", "index": 9}));
    let tx = functions["functions"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|f| f["label"] == "usart1_tx")
        .ok_or("usart1_tx")?;
    assert_eq!(tx["mux"], "AF7");
    assert_eq!(
        tx["resource_role"],
        json!({"resource": imported.peripheral("USART1")?, "role": imported.role("USART1", "tx")?})
    );
    assert_eq!(tx["source"]["page"], 12);

    // 5. F8: the uncertain row blocks publishing, the rest warn.
    let check = &draft["check"];
    if let Some(rule) = stub_in(check) {
        return Err(Box::new(Stub {
            step: "imported draft check".into(),
            rule: rule.into(),
        }));
    }
    let found = codes(check);
    assert!(found.iter().any(|c| c == "pinout.uncertain"), "{check}");
    assert!(found.iter().any(|c| c == "pinout.unreviewed"), "{check}");
    assert_eq!(check["result"]["publishable"], false, "{check}");
    s.refuse_apply(
        "publish while uncertain",
        library_op("library.publish", json!({"drafts": [component]})),
        "conflict",
    )?;

    // 6. The person checked a clean copy: re-import the corrected row onto
    // the draft. Terminals are kept by number; only r20 changes.
    let corrected = exm48_corrected()?;
    let target = json!({"target": "draft", "id": component});
    let plan = s.query(
        "pinout_preview corrected",
        "library.pinout_preview",
        preview_params(target.clone(), &corrected),
    )?;
    assert_eq!(count(&plan["changes"], "terminal", "added"), 0, "{plan}");
    assert_eq!(count(&plan["changes"], "terminal", "removed"), 0, "{plan}");
    assert!(
        plan["review"].as_array().is_some_and(Vec::is_empty),
        "{plan}"
    );
    s.apply(
        "pinout_apply corrected",
        apply_op(target, &corrected, &plan["base"], json!({"accept": "all"})),
    )?;
    let draft = s.draft(component)?;
    assert_eq!(
        Imported::read(&draft)?.terminals,
        imported.terminals,
        "terminal identity survives the re-import"
    );
    let found = codes(&draft["check"]);
    assert!(!found.iter().any(|c| c == "pinout.uncertain"), "{draft}");
    assert!(found.iter().any(|c| c == "pinout.unreviewed"), "{draft}");

    // 7. The person confirms the reviewed rows: the review state is cleared
    // through an ordinary draft edit, one undo step.
    let mut body = draft["draft"]["definition"]["body"].clone();
    body["review"] = Value::Null;
    s.apply(
        "confirm review",
        library_op(
            "library.edit",
            json!({"edits": [{"id": component, "meta": draft["draft"]["meta"],
                "definition": {"kind": "component", "body": body}}]}),
        ),
    )?;
    let draft = s.draft(component)?;
    let found = codes(&draft["check"]);
    assert!(
        !found.iter().any(|c| c.starts_with("pinout.")),
        "{}",
        draft["check"]
    );
    Imported::read(&draft)
}

/// Placement of the reviewed import.
pub(super) struct Placed {
    pub imported: Imported,
    pub instance: Value,
    pub designator: String,
}

/// Step 8: publish the component with the block symbol the import drew
/// and place U1.
pub(super) fn publish_and_place(s: &mut Scenario, imported: Imported) -> Fallible<Placed> {
    let symbol = imported.symbol;
    let mut component = imported.body.clone();
    if component["designator_prefix"]
        .as_str()
        .is_none_or(str::is_empty)
    {
        component["designator_prefix"] = json!("U");
    }
    // The pins workflow publishes the logical component; the imported
    // package and binding drafts wait for their footprint, so this
    // revision lists no binding and is placed without a physical side.
    component["implementations"] = json!([]);
    s.apply(
        "logical component",
        library_op(
            "library.edit",
            json!({"edits": [
                {"id": imported.component, "meta": imported.meta,
                 "definition": {"kind": "component", "body": component}},
            ]}),
        ),
    )?;
    let published = s.apply(
        "publish",
        library_op(
            "library.publish",
            json!({"drafts": [imported.component, symbol]}),
        ),
    )?;
    let revision = published["outcome"]["revisions"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|r| r["kind"] == "component" && r["id"] == imported.component)
        .ok_or("the component revision")?
        .clone();
    let placed = s.apply(
        "place",
        instance_op(
            "component.place",
            json!({"component": {"id": revision["id"], "revision": revision["revision"]},
                "designator": "U1", "properties": [], "representation": symbol,
                "unit": 1, "position": [60.0, 0.0], "physical": null}),
        ),
    )?;
    assert_eq!(placed["outcome"]["designator"], "U1");
    Ok(Placed {
        imported,
        instance: placed["outcome"]["instance"].clone(),
        designator: "U1".into(),
    })
}

/// The fixture is a valid format-1 pinout document by the contract's own
/// types, and internally consistent: unique rows and numbers, every
/// function's peripheral and role declared, defaults among the functions,
/// one uncertain row, and rule scripts within the C1 source bound.
#[test]
fn the_exm48_fixture_is_a_consistent_pinout_document() -> Fallible<()> {
    use pcbcraft_core::library::pinout::{PINOUT_FORMAT, PinoutDocument};
    let document: PinoutDocument = serde_json::from_str(EXM48)?;
    assert_eq!(document.format, PINOUT_FORMAT);
    let [variant] = document.variants.as_slice() else {
        return Err("one variant".into());
    };
    assert_eq!(variant.terminal_count as usize, document.pins.len());
    let mut rows = std::collections::BTreeSet::new();
    let mut numbers = std::collections::BTreeSet::new();
    for pin in &document.pins {
        assert!(rows.insert(pin.row.clone()), "row {}", pin.row);
        let number = pin.number.get(&variant.name).ok_or("number per variant")?;
        assert!(numbers.insert(number.clone()), "number {number}");
        if let Some(default) = &pin.default {
            assert!(
                pin.functions.iter().any(|f| &f.label == default),
                "{}",
                pin.name
            );
        }
        for function in &pin.functions {
            if let Some(label) = &function.peripheral {
                let peripheral = document
                    .peripherals
                    .iter()
                    .find(|p| &p.label == label)
                    .ok_or_else(|| format!("{}: peripheral {label}", pin.name))?;
                let role = function.role.as_ref().ok_or("role with peripheral")?;
                assert!(
                    peripheral.roles.iter().any(|r| &r.label == role),
                    "{label}.{role}"
                );
            }
        }
    }
    let uncertain: Vec<&str> = document
        .pins
        .iter()
        .filter(|p| p.uncertain)
        .map(|p| p.row.as_str())
        .collect();
    assert_eq!(uncertain, ["r20"]);
    assert_eq!(document.rules.len(), 3);
    assert!(
        document
            .rules
            .iter()
            .all(|r| r.script.len() <= pcbcraft_core::functions::rules::MAX_RULE_SOURCE)
    );
    // The corrected copy differs only in r20's flag and note.
    let corrected: PinoutDocument = serde_json::from_value(exm48_corrected()?)?;
    let changed: Vec<&str> = document
        .pins
        .iter()
        .zip(&corrected.pins)
        .filter(|(a, b)| a != b)
        .map(|(a, _)| a.row.as_str())
        .collect();
    assert_eq!(changed, ["r20"]);
    Ok(())
}

/// The served schema and the fixture agree on the document's top level.
#[test]
fn the_pinout_schema_is_served_and_names_the_fixture_fields() -> Fallible<()> {
    let mut s = Scenario::demo()?;
    let schema = s.send(
        "schema",
        json!({"command": "library.pinout_schema", "params": {}}),
    )?;
    assert_eq!(schema["format"], 1);
    let properties = &schema["schema"]["properties"];
    for (key, _) in exm48()?.as_object().ok_or("object")? {
        assert!(properties.get(key).is_some(), "schema lacks {key}");
    }
    Ok(())
}

/// A malformed document (an unknown field, a misspelt direction) is
/// refused at the transport with nothing changed: agents get the error,
/// never a partly read pinout.
#[test]
fn a_malformed_pinout_document_is_refused_unchanged() -> Fallible<()> {
    let mut s = Scenario::demo()?;
    let mut document = exm48()?;
    document["pins"][0]["functions"][0]["direction"] = json!("sideways");
    let reply = s.refuse(
        "malformed",
        json!({"command": "project.query", "params": {"query": "library.pinout_preview",
            "params": preview_params(json!({"target": "new", "name": "EXM-48"}), &document)}}),
        "malformed_request",
    );
    reply?;
    let mut document = exm48()?;
    document["pins"][0]["peripheral"] = json!("USART1");
    s.refuse(
        "unknown field",
        json!({"command": "project.query", "params": {"query": "library.pinout_preview",
            "params": preview_params(json!({"target": "new", "name": "EXM-48"}), &document)}}),
        "malformed_request",
    )?;
    Ok(())
}

/// Import → review → apply → the F8 gate → corrected re-import →
/// confirmation → publish and place, through JSON only.
#[test]
fn exm48_imports_through_review_before_it_publishes() -> Fallible<()> {
    finish((|| -> Fallible<()> {
        let mut s = Scenario::demo()?;
        let imported = import_reviewed(&mut s)?;
        let placed = publish_and_place(&mut s, imported)?;
        assert_eq!(placed.designator, "U1");
        Ok(())
    })())
}
