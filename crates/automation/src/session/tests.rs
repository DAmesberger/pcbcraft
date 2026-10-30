use super::*;
use pcbcraft_core::{
    DefinitionKind, Engine, InstanceCommand, Operation, QueryResult, UnitId, instances::NewInstance,
};

type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;

fn r1(engine: &Engine) -> u64 {
    engine
        .project()
        .instance_by_designator("R1")
        .or_else(|| engine.project().instances.first())
        .map_or(0, |i| i.id)
}

fn apply(operation: &Operation, revision: u64) -> Value {
    json!({"command": "project.apply", "params": {
        "expected_revision": revision,
        "operation": serde_json::to_value(operation).unwrap_or_default()
    }})
}

fn edit(session: &Session, designator: &str, revision: u64) -> Value {
    apply(
        &Operation::Instance(InstanceCommand::SetDesignator {
            instance: r1(session.engine()),
            designator: designator.into(),
        }),
        revision,
    )
}

#[test]
fn json_requests_reach_the_engine_and_report_typed_errors() -> TestResult {
    let mut session = Session::new(Engine::demo()?);
    let revision = session.engine().revision();
    let reply = session.dispatch(edit(&session, "R10", revision));
    assert_eq!(reply["ok"], true, "{reply}");
    assert_eq!(
        reply["result"]["project"]["instances"][0]["designator"],
        "R10"
    );
    assert_eq!(reply["result"]["outcome"], json!({"outcome": "none"}));
    assert!(reply["result"]["erc"].is_array());

    let stale = session.dispatch(edit(&session, "R11", revision));
    assert_eq!(stale["ok"], false);
    assert_eq!(stale["error"]["code"], "stale_revision");
    assert_eq!(stale["error"]["actual_revision"], revision + 1);

    let unknown = session.dispatch(json!({"command": "project.unknown", "params": {}}));
    assert_eq!(unknown["error"]["code"], "malformed_request");
    let extra = session.dispatch(json!({"command": "project.undo", "params": {"x": 1}}));
    assert_eq!(extra["error"]["code"], "malformed_request");

    assert_eq!(
        session.dispatch(json!({"command": "project.undo", "params": {}}))["ok"],
        true
    );
    let empty = session.dispatch(json!({"command": "project.undo", "params": {}}));
    assert_eq!(empty["error"]["code"], "history_empty");
    Ok(())
}

#[test]
fn pad_generator_is_available_through_the_shared_transport_contract() -> TestResult {
    let mut session = Session::new(Engine::demo()?);
    let original = session.engine().document().clone();
    let revision = session.engine().revision();
    let (index, footprint) = session
        .engine()
        .drafts()
        .items
        .iter()
        .enumerate()
        .find(|(_, d)| d.definition.kind() == DefinitionKind::Footprint)
        .map(|(i, d)| (i, d.id))
        .ok_or("demo footprint draft")?;
    let operation = json!({
        "editor": "Library", "operation": {
            "command": "library.generate_pads", "params": {"draft": footprint, "script": {
                "api_version": 1,
                "source": "for i in 0..2 { footprint.pad(i + 1, i * pitch, q(0.0, \"mm\"), pitch, pitch); }",
                "units": [{"dimension": [1,0,0,0,0,0,0], "units": [
                    {"symbol": "m", "scale": 1.0, "si_prefixes": true}
                ]}],
                "inputs": {"pitch": {"type": "quantity", "value": 2.0, "unit": "mm"}}
            }}
        }
    });
    let reply = session.dispatch(json!({"command": "project.apply", "params": {
        "expected_revision": revision, "operation": operation,
    }}));
    assert_eq!(reply["ok"], true, "{reply}");
    let draft = &reply["result"]["drafts"][index];
    assert_eq!(
        draft["draft"]["definition"]["body"]["pads"][1]["position"],
        json!([2.0, 0.0])
    );
    assert_eq!(draft["check"]["result"]["publishable"], true, "{draft}");
    assert_eq!(session.engine().project(), original.project.as_ref());
    assert_eq!(
        session.dispatch(json!({"command": "project.undo", "params": {}}))["ok"],
        true
    );
    assert_eq!(session.engine().document(), &original);
    let catalog = serde_json::to_string(&Request::catalog())?;
    assert!(catalog.contains("library.generate_pads"));
    assert!(!catalog.contains("part."));
    assert!(!catalog.contains("migrate"));
    Ok(())
}

#[test]
fn save_and_open_are_file_effects_outside_undo_history() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("session.pcbcraft");
    let path_text = path.display().to_string();
    let mut session = Session::new(Engine::demo()?);
    let revision = session.engine().revision();
    assert_eq!(
        session.dispatch(edit(&session, "R10", revision))["ok"],
        true
    );
    let before = session.engine().to_file();
    let revision = session.engine().revision();

    let saved = session.dispatch(json!({"command": "project.save", "params": {"path": path_text}}));
    assert_eq!(saved["result"]["saved_path"], path_text.as_str());
    assert_eq!(
        session.engine().revision(),
        revision,
        "saving is not an edit"
    );
    assert!(session.engine().can_undo());
    // Save without a path reuses the session's file.
    assert_eq!(
        session.dispatch(json!({"command": "project.save", "params": {}}))["ok"],
        true
    );

    let mut other = Session::default();
    let refused = other.dispatch(json!({"command": "project.save", "params": {}}));
    assert_eq!(refused["error"]["code"], "invalid_input");
    let opened = other.dispatch(json!({"command": "project.open", "params": {"path": path_text}}));
    assert_eq!(opened["ok"], true);
    assert_eq!(other.engine().to_file(), before);
    assert!(!other.engine().can_undo());

    std::fs::write(&path, b"broken")?;
    let failed = other.dispatch(json!({"command": "project.open", "params": {"path": path_text}}));
    assert_eq!(failed["ok"], false);
    assert_eq!(
        other.engine().to_file(),
        before,
        "a failed open changes nothing"
    );
    Ok(())
}

#[test]
fn native_export_writes_a_new_solidcraft_file_only() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("board.solidcraft").display().to_string();
    let mut session = Session::new(Engine::demo()?);
    let request = json!({"command": "project.export_native", "params": {"path": path}});
    let first = session.dispatch(request.clone());
    assert_eq!(first["ok"], true, "{first}");
    assert!(std::path::Path::new(&path).exists());
    let second = session.dispatch(request);
    assert_eq!(second["ok"], false, "existing files are never overwritten");
    Ok(())
}

#[test]
fn catalog_matches_the_request_type() -> TestResult {
    let catalog = Request::catalog();
    assert_eq!(catalog.len(), 11);
    let schema = serde_json::to_string(&schemas())?;
    for command in &catalog {
        assert!(
            schema.contains(command.id),
            "{} is in the schema",
            command.id
        );
        assert!(command.params.is_object());
    }
    Ok(())
}

/// The same requests through the JSON transport and directly through the
/// engine give the same document and the same query reports.
#[test]
fn placing_and_inspecting_through_json_matches_the_engine() -> TestResult {
    let mut session = Session::new(Engine::demo()?);
    let mut engine = Engine::demo()?;
    let component = engine.project().instances[0].component;
    let first = engine.project().instances[0].clone();
    let place = Operation::Instance(InstanceCommand::Place(NewInstance {
        component,
        designator: Some("R3".into()),
        properties: vec![],
        representation: first.representation,
        unit: UnitId(1),
        position: [30.0, 0.0],
        physical: first.physical,
    }));
    let reply = session.dispatch(apply(&place, session.engine().revision()));
    assert_eq!(reply["ok"], true, "{reply}");
    let outcome = engine.apply(engine.revision(), place)?;
    assert_eq!(reply["result"]["outcome"], serde_json::to_value(&outcome)?);
    let instance = reply["result"]["outcome"]["instance"]
        .as_u64()
        .ok_or("instance outcome")?;
    assert_eq!(session.engine().project(), engine.project());

    let inspected =
        session.dispatch(json!({"command": "project.query", "params": {"query": "component.inspect", "params": {"instance": instance}}}));
    assert_eq!(inspected["ok"], true, "{inspected}");
    let direct = engine.query(Query::InspectInstance { instance })?;
    assert_eq!(inspected["result"], serde_json::to_value(&direct)?);
    let QueryResult::Instance(report) = direct else {
        return Err("instance report expected".into());
    };
    assert_eq!(report.designator, "R3");
    assert_eq!(report.occurrences, 1);
    assert_eq!(
        inspected["result"]["result"]["units"][0]["state"]["state"],
        "placed"
    );

    let target = json!({"target": "revision", "id": {"kind": "component", "id": component.id, "revision": component.revision}});
    let checked =
        session.dispatch(json!({"command": "project.query", "params": {"query": "component.check", "params": {"target": target}}}));
    assert_eq!(checked["ok"], true, "{checked}");
    let direct = engine.query(Query::CheckDefinition {
        target: serde_json::from_value(target)?,
    })?;
    assert_eq!(checked["result"], serde_json::to_value(&direct)?);
    let revision = session.engine().revision();
    let missing =
        session.dispatch(json!({"command": "project.query", "params": {"query": "component.inspect", "params": {"instance": 999}}}));
    assert_eq!(missing["error"]["code"], "not_found");
    assert_eq!(
        session.engine().revision(),
        revision,
        "queries change nothing"
    );
    assert!(!session.engine().can_redo());
    Ok(())
}

/// `instance.set_properties` sent as JSON text returns the same effective
/// values as the engine; invalid text is a typed error that changes nothing.
#[test]
fn set_properties_through_json_returns_the_engine_effective_values() -> TestResult {
    let mut session = Session::new(Engine::demo()?);
    let mut engine = Engine::demo()?;
    let instance = r1(&engine);
    let field = engine
        .project()
        .library
        .fields
        .first()
        .map(|f| f.id)
        .ok_or("demo field")?;
    let request = |text: &str, revision: u64| {
        json!({"command": "project.apply", "params": {
            "expected_revision": revision,
            "operation": {"editor": "Instance", "operation": {
                "command": "instance.set_properties",
                "params": {"instance": instance, "edits": [
                    {"field": field, "input": {"input": "text", "value": text}}
                ]}
            }}
        }})
    };
    let reply = session.dispatch(request("4.7k", session.engine().revision()));
    assert_eq!(reply["ok"], true, "{reply}");
    let operation: Operation =
        serde_json::from_value(request("4.7k", engine.revision())["params"]["operation"].clone())?;
    let outcome = engine.apply(engine.revision(), operation)?;
    assert_eq!(reply["result"]["outcome"], serde_json::to_value(&outcome)?);
    let effective = &reply["result"]["outcome"]["effective"][0];
    assert_eq!(effective["value"]["value"]["si"], 4700.0);
    assert_eq!(effective["value"]["value"]["display_unit"], "kΩ");
    assert_eq!(effective["origin"], "assigned");
    assert_eq!(effective["text"], "4.7 kΩ");
    assert_eq!(session.engine().project(), engine.project());

    let revision = session.engine().revision();
    let rejected = session.dispatch(request("10 µF", revision));
    assert_eq!(rejected["error"]["code"], "invalid_input", "{rejected}");
    assert_eq!(session.engine().revision(), revision);
    assert_eq!(session.engine().project(), engine.project());
    Ok(())
}

/// `instance.select_functions` sent as JSON returns the engine's resolution;
/// a function of another terminal is a typed error that changes nothing.
#[test]
fn select_functions_through_json_returns_the_engine_resolution() -> TestResult {
    let mut session = Session::new(Engine::demo()?);
    let mut engine = Engine::demo()?;
    let instance = r1(&engine);
    let functions: Vec<u32> = engine
        .project()
        .library
        .components
        .first()
        .ok_or("demo component")?
        .definition
        .terminals
        .iter()
        .filter_map(|t| t.functions())
        .filter_map(|f| f.functions.first().map(|f| f.id.0))
        .collect();
    let [first, second] = functions[..] else {
        return Err("two fixed functions".into());
    };
    let request = |function: u32, revision: u64| {
        json!({"command": "project.apply", "params": {
            "expected_revision": revision,
            "operation": {"editor": "Instance", "operation": {
                "command": "instance.select_functions",
                "params": {"instance": instance, "edits": [
                    {"terminal": 1, "function": function}
                ]}
            }}
        }})
    };
    let reply = session.dispatch(request(first, session.engine().revision()));
    assert_eq!(reply["ok"], true, "{reply}");
    let operation: Operation =
        serde_json::from_value(request(first, engine.revision())["params"]["operation"].clone())?;
    let outcome = engine.apply(engine.revision(), operation)?;
    assert_eq!(reply["result"]["outcome"], serde_json::to_value(&outcome)?);
    let active = &reply["result"]["outcome"]["resolution"]["active"][0];
    assert_eq!(active["function"], first);
    assert_eq!(active["origin"], "selected");
    assert_eq!(active["direction"], "passive");
    assert_eq!(session.engine().project(), engine.project());

    let revision = session.engine().revision();
    let rejected = session.dispatch(request(second, revision));
    assert_eq!(rejected["error"]["code"], "invalid_input", "{rejected}");
    assert_eq!(session.engine().revision(), revision);
    assert_eq!(session.engine().project(), engine.project());
    Ok(())
}

/// A synthetic parts project as a session, built through the public format.
fn parts_session() -> std::result::Result<Session, Box<dyn std::error::Error>> {
    let file = EditorFile {
        version: pcbcraft_core::format::FILE_VERSION,
        project: pcbcraft_core::catalog::synthetic::project(24)?,
        drafts: Drafts::default(),
    };
    Ok(Session::new(Engine::from_file(file)?))
}

fn query(name: &str, params: Value) -> Value {
    json!({"command": "project.query", "params": {"query": name, "params": params}})
}

/// `library.search`, `library.facets` and `library.inspect` reach the engine
/// through JSON and agree with a direct query; the found revision places
/// through `project.apply`; a malformed query reports the term.
#[test]
fn library_search_through_json_matches_the_engine_and_places() -> TestResult {
    let mut session = parts_session()?;
    let reply = session.dispatch(query(
        "library.search",
        json!({"query": "res:4k7..22k pkg:0603"}),
    ));
    assert_eq!(reply["ok"], true, "{reply}");
    let direct = session.engine().query(Query::Search {
        query: "res:4k7..22k pkg:0603".into(),
        limit: None,
        include_drafts: false,
    })?;
    assert_eq!(reply["result"], serde_json::to_value(&direct)?);
    let results = reply["result"]["result"]["results"]
        .as_array()
        .ok_or("no results")?
        .clone();
    let found: Vec<&str> = results
        .iter()
        .filter_map(|h| h["entry"]["name"].as_str())
        .collect();
    assert_eq!(found, ["EX-R0603-10K0-F", "EX-R0603-22K0-D"]);
    assert_eq!(
        reply["result"]["result"]["chips"][0],
        "Resistance 4.7 kΩ .. 22 kΩ"
    );

    let facets = session.dispatch(query("library.facets", json!({"query": "ex-c"})));
    assert_eq!(facets["ok"], true, "{facets}");
    let labels: Vec<&str> = facets["result"]["result"]["fields"]
        .as_array()
        .ok_or("no facets")?
        .iter()
        .filter_map(|f| f["label"].as_str())
        .collect();
    assert_eq!(labels, ["Capacitance", "Max voltage"]);

    let lineage = results[0]["entry"]["lineage"].as_u64().ok_or("lineage")?;
    let inspected = session.dispatch(query("library.inspect", json!({"lineage": lineage})));
    assert_eq!(inspected["ok"], true, "{inspected}");
    let report = inspected["result"]["result"].clone();
    assert_eq!(report["latest"], true);

    // Place the found revision through the one engine.
    let id = |value: &Value| value.as_u64().map(pcbcraft_core::DefinitionId);
    let revision = results[0]["entry"]["revision"].as_u64().ok_or("revision")?;
    let place = Operation::Instance(InstanceCommand::Place(NewInstance {
        component: pcbcraft_core::ComponentRef::new(
            pcbcraft_core::DefinitionId(lineage),
            pcbcraft_core::Revision(u32::try_from(revision)?),
        ),
        designator: None,
        properties: vec![],
        representation: id(&report["symbols"][0]["symbol"]["id"]).ok_or("symbol")?,
        unit: UnitId(1),
        position: [0.0, 0.0],
        physical: Some(pcbcraft_core::PhysicalChoice {
            binding: id(&report["implementations"][0]["binding"]["id"]).ok_or("binding")?,
            footprint: id(&report["implementations"][0]["footprints"][0]["footprint"]["id"])
                .ok_or("footprint")?,
        }),
    }));
    // Without a designator the request carries none and the core numbers
    // the resistor under its component's prefix.
    let request = apply(&place, session.engine().revision());
    assert!(
        request["params"]["operation"]["params"]
            .get("designator")
            .is_none()
    );
    let placed = session.dispatch(request);
    assert_eq!(placed["ok"], true, "{placed}");
    assert_eq!(placed["result"]["outcome"]["designator"], "R1");
    assert_eq!(
        placed["result"]["project"]["instances"][0]["designator"],
        "R1"
    );
    // Now used in the project, the part ranks first for a shared prefix.
    let again = session.dispatch(query(
        "library.search",
        json!({"query": "ex-r0603", "limit": 3}),
    ));
    let first = &again["result"]["result"]["results"][0]["entry"];
    assert_eq!(first["name"], "EX-R0603-10K0-F");
    assert_eq!(first["used"], 1);

    let revision = session.engine().revision();
    let bad = session.dispatch(query("library.search", json!({"query": "V>=10k"})));
    assert_eq!(bad["ok"], false);
    assert_eq!(bad["error"]["code"], "invalid_input");
    assert_eq!(
        bad["error"]["message"],
        "V>=10k: 10 k has no unit of voltage"
    );
    assert_eq!(
        session.engine().revision(),
        revision,
        "queries change nothing"
    );
    Ok(())
}

/// `layout.set_fabrication` sent as JSON changes the saved attributes, is
/// undoable, appears in the discovery schemas, and an unknown colour is a
/// malformed request that changes nothing.
#[test]
fn board_fabrication_through_json_is_undoable_and_discoverable() -> TestResult {
    let mut session = Session::new(Engine::demo()?);
    let request = |colour: &str, revision: u64| {
        json!({"command": "project.apply", "params": {
            "expected_revision": revision,
            "operation": {"editor": "Layout", "operation": {
                "command": "layout.set_fabrication",
                "params": {"mask_colour": colour, "mask_finish": "matte",
                    "pad_finish": "immersion_silver", "silkscreen_colour": "black"}
            }}
        }})
    };
    let revision = session.engine().revision();
    let reply = session.dispatch(request("black", revision));
    assert_eq!(reply["ok"], true, "{reply}");
    assert_eq!(
        reply["result"]["project"]["fabrication"],
        json!({"mask_colour": "black", "mask_finish": "matte",
            "pad_finish": "immersion_silver", "silkscreen_colour": "black"})
    );
    let rejected = session.dispatch(request("orange", revision + 1));
    assert_eq!(rejected["error"]["code"], "malformed_request", "{rejected}");
    assert_eq!(session.engine().revision(), revision + 1);
    let undo = session.dispatch(json!({"command": "project.undo", "params": {}}));
    assert_eq!(undo["ok"], true);
    assert_eq!(
        session.engine().project().fabrication,
        pcbcraft_core::Fabrication::default()
    );
    let schemas = schemas().to_string();
    assert!(schemas.contains("layout.set_fabrication"));
    assert!(schemas.contains("immersion_silver"));
    Ok(())
}
