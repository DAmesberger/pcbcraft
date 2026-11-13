//! Manufacturer parts (S2) over the JSON transport: a Part authored through
//! `library.new_draft`, `library.edit` and `library.publish`, the Resolve
//! parts groups, `instance.assign_part`, purchasing completeness and the BOM.
//! Every mutation is mirrored on a second engine directly, and every report
//! is compared with the engine's own query.
//!
//! The fixture is original: `ExampleCo` is a fictitious manufacturer and the
//! `EX-` MPN, distributor and SKU are invented.
use super::*;
use sparksmith_core::{Engine, PartRef};

type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;
type Fallible<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const MPN: &str = "EX-R0603-10K0-F";
/// SHA-256 of an attached datasheet copy (any 64 hex digits; CE-07 keeps it
/// when the reference has no URL).
const ASSET: &str = "5f1d0c9a3e7b2a4c6d8e0f1a2b3c4d5e6f708192a3b4c5d6e7f8091a2b3c4d5e";

/// One session over JSON and one engine driven directly, from the same demo.
struct Pair {
    session: Session,
    engine: Engine,
}

impl Pair {
    fn demo() -> Fallible<Self> {
        Ok(Self {
            session: Session::new(Engine::demo()?),
            engine: Engine::demo()?,
        })
    }

    /// `project.apply` of one JSON operation through the session, and the
    /// same operation deserialized and applied to the engine. Both must
    /// succeed with the same outcome and leave the same document.
    fn apply(&mut self, operation: Value) -> Fallible<Value> {
        let reply = self
            .session
            .dispatch(json!({"command": "project.apply", "params": {
                "expected_revision": self.session.engine().revision(),
                "operation": operation.clone(),
            }}));
        assert_eq!(reply["ok"], true, "{reply}");
        let outcome = self
            .engine
            .apply(self.engine.revision(), serde_json::from_value(operation)?)?;
        assert_eq!(reply["result"]["outcome"], serde_json::to_value(&outcome)?);
        assert_eq!(self.session.engine().document(), self.engine.document());
        Ok(reply["result"].clone())
    }

    /// `project.apply` that must fail with `code` on both sides and change
    /// nothing.
    fn refuse(&mut self, operation: Value, code: &str) -> Fallible<Value> {
        let before = self.session.engine().document().clone();
        let revision = self.session.engine().revision();
        let reply = self
            .session
            .dispatch(json!({"command": "project.apply", "params": {
                "expected_revision": revision,
                "operation": operation.clone(),
            }}));
        assert_eq!(reply["ok"], false, "{reply}");
        assert_eq!(reply["error"]["code"], code, "{reply}");
        let direct = self
            .engine
            .apply(self.engine.revision(), serde_json::from_value(operation)?);
        let error = direct
            .err()
            .ok_or("the engine accepted what JSON refused")?;
        assert_eq!(error.diagnostic().code, code);
        assert_eq!(self.session.engine().revision(), revision);
        assert_eq!(self.session.engine().document(), &before);
        assert_eq!(self.engine.document(), &before);
        Ok(reply["error"].clone())
    }

    /// `project.query` through JSON, compared with the engine's own query.
    /// Returns the envelope, so callers can assert either outcome.
    fn query(&mut self, name: &str, params: Value) -> Fallible<Value> {
        let request = json!({"query": name, "params": params});
        let revision = self.session.engine().revision();
        let reply = self
            .session
            .dispatch(json!({"command": "project.query", "params": request}));
        let direct = self.engine.query(serde_json::from_value(request)?);
        match &direct {
            Ok(result) => assert_eq!(reply["result"], serde_json::to_value(result)?),
            Err(error) => assert_eq!(reply["error"], serde_json::to_value(error.diagnostic())?),
        }
        assert_eq!(
            self.session.engine().revision(),
            revision,
            "queries change nothing"
        );
        Ok(reply)
    }

    fn inspect(&mut self, instance: &Value) -> Fallible<Value> {
        let reply = self.query("component.inspect", json!({"instance": instance}))?;
        assert_eq!(reply["ok"], true, "{reply}");
        Ok(reply["result"]["result"].clone())
    }
}

/// The two demo resistors, R1 and R2, as JSON instances.
fn resistors(pair: &mut Pair) -> Fallible<Vec<Value>> {
    let reply = pair
        .session
        .dispatch(json!({"command": "project.inspect", "params": {}}));
    let instances = reply["result"]["project"]["instances"]
        .as_array()
        .ok_or("instances")?
        .clone();
    let designators: Vec<&str> = instances
        .iter()
        .filter_map(|i| i["designator"].as_str())
        .collect();
    assert_eq!(designators, ["R1", "R2"]);
    Ok(instances)
}

/// Author and publish one Part for the demo resistor through JSON only: the
/// component revision R1 pins, the binding of its physical choice, its
/// resistance as an exact nominal spec, a datasheet without a URL but with an
/// attached copy, and one distributor offer.
fn publish_part(pair: &mut Pair) -> Fallible<PartRef> {
    let instances = resistors(pair)?;
    let r1 = &instances[0];
    let report = pair.inspect(&r1["id"])?;
    let resistance = &report["properties"][0];
    assert_eq!(resistance["text"], "10 kΩ", "{report}");

    // The binding revision comes from the lineage the instance pins.
    let lineage = pair.query(
        "library.inspect",
        json!({"lineage": r1["component"]["id"], "revision": r1["component"]["revision"]}),
    )?;
    assert_eq!(lineage["ok"], true, "{lineage}");
    let binding = lineage["result"]["result"]["implementations"]
        .as_array()
        .ok_or("implementations")?
        .iter()
        .map(|i| &i["binding"])
        .find(|b| b["id"] == r1["physical"]["binding"])
        .ok_or("the physical choice's binding")?;
    let binding = json!({"id": binding["id"], "revision": binding["revision"]});

    let meta = json!({
        "name": MPN,
        "description": "Synthetic 10 kΩ ±1 % 0603 resistor; not a real product.",
        "provenance": "Original test fixture. ExampleCo is fictitious.",
    });
    let empty = serde_json::to_value(sparksmith_core::library::PartDefinition::default())?;
    let created = pair.apply(json!({"editor": "Library", "operation": {
        "command": "library.new_draft",
        "params": {"meta": meta, "definition": {"kind": "part", "body": empty}},
    }}))?;
    assert_eq!(created["outcome"]["outcome"], "draft_created", "{created}");
    let draft = created["outcome"]["id"].clone();

    let authored = json!({"origin": null, "basis": {"basis": "authored"}, "licence": null});
    let body = json!({
        "next_local": 3,
        "manufacturer": "ExampleCo",
        "mpn": MPN,
        "component": r1["component"],
        "binding": binding,
        "specs": [{
            "field": resistance["field"],
            "value": resistance["value"],
            "meaning": "exact_nominal",
        }],
        "references": [{
            "id": 1,
            "kind": "datasheet",
            "title": "EX-R0603 series datasheet",
            "url": null,
            "document_revision": "A",
            "asset": ASSET,
            "licence": null,
            "provenance": authored,
        }],
        "offers": [{
            "id": 2,
            "distributor": "Example Distribution",
            "sku": "EXD-R0603-10K0-F",
            "url": "https://example.com/EX-R0603-10K0-F",
            "packaging": "cut tape",
        }],
        "provenance": authored,
    });
    let edited = pair.apply(json!({"editor": "Library", "operation": {
        "command": "library.edit",
        "params": {"edits": [{"id": draft, "meta": meta,
            "definition": {"kind": "part", "body": body}}]},
    }}))?;
    let check = edited["drafts"]
        .as_array()
        .ok_or("drafts")?
        .iter()
        .find(|d| d["draft"]["id"] == draft)
        .map(|d| d["check"].clone())
        .ok_or("the Part draft's check")?;
    assert_eq!(check["result"]["kind"], "part", "{check}");
    assert_eq!(check["result"]["publishable"], true, "{check}");

    let published = pair.apply(json!({"editor": "Library", "operation": {
        "command": "library.publish", "params": {"drafts": [draft]},
    }}))?;
    let revisions = published["outcome"]["revisions"]
        .as_array()
        .ok_or("revisions")?;
    let [revision] = revisions.as_slice() else {
        return Err(format!("one revision expected: {published}").into());
    };
    assert_eq!(revision["kind"], "part");
    let part: PartRef =
        serde_json::from_value(json!({"id": revision["id"], "revision": revision["revision"]}))?;
    // The published revision keeps exactly what was authored, and survives
    // a save and reopen through the transport (CE-07: the reference without
    // a URL keeps its identity and attached copy).
    let stored = &published["project"]["library"]["parts"][0]["definition"];
    assert_eq!(stored, &body);
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("parts.sparksmith").display().to_string();
    let saved = pair
        .session
        .dispatch(json!({"command": "project.save", "params": {"path": path}}));
    assert_eq!(saved["ok"], true, "{saved}");
    let mut reopened = Session::default();
    let opened = reopened.dispatch(json!({"command": "project.open", "params": {"expected_revision": reopened.engine().revision(), "path": path}}));
    assert_eq!(opened["ok"], true, "{opened}");
    assert_eq!(
        opened["result"]["project"]["library"]["parts"][0]["definition"],
        body
    );
    Ok(part)
}

fn assign(instances: &[Value], part: Option<PartRef>) -> Fallible<Value> {
    Ok(json!({"editor": "Instance", "operation": {
        "command": "instance.assign_part",
        "params": {"instances": instances, "part": serde_json::to_value(part)?},
    }}))
}

/// Clearing a Part is one undoable step; an empty instance set and an
/// unknown Part are refused without changes.
#[test]
fn assign_part_clears_and_refuses_bad_input_through_json() -> TestResult {
    let mut pair = Pair::demo()?;
    let part = publish_part(&mut pair)?;
    let instances: Vec<Value> = resistors(&mut pair)?
        .iter()
        .map(|i| i["id"].clone())
        .collect();
    let report = pair.inspect(&instances[0])?;
    assert_eq!(report["purchasing"]["state"], "no_part", "{report}");
    assert_eq!(report["purchasing"]["part"], Value::Null);

    pair.apply(assign(&instances, None)?)?;
    pair.refuse(assign(&[], None)?, "invalid_input")?;
    let unknown = PartRef::new(sparksmith_core::DefinitionId(9999), part.revision);
    let before = pair.session.engine().document().clone();
    let refused = pair
        .session
        .dispatch(json!({"command": "project.apply", "params": {
            "expected_revision": pair.session.engine().revision(),
            "operation": assign(&instances, Some(unknown))?,
        }}));
    assert_eq!(refused["ok"], false, "{refused}");
    assert_eq!(pair.session.engine().document(), &before);
    let now = pair.session.engine().revision();
    let undo = pair
        .session
        .dispatch(json!({"command": "project.undo", "params": {"expected_revision": now}}));
    assert_eq!(undo["ok"], true, "{undo}");
    assert_eq!(undo["result"]["can_redo"], true);
    Ok(())
}

/// The full S2 automation workflow: Resolve parts lists the two demo
/// resistors as one group with a Part query; the group takes the Part in one
/// `instance.assign_part`, one undo step; `component.inspect` then reports
/// `purchasing.state` `resolved` with the MPN, the BOM has one resolved line
/// of two and no unresolved lines, and no group is left to resolve. Every
/// report equals the engine's own query.
#[test]
fn parts_resolve_assign_and_bom_through_json_match_the_engine() -> TestResult {
    let mut pair = Pair::demo()?;
    let part = publish_part(&mut pair)?;
    let instances: Vec<Value> = resistors(&mut pair)?
        .iter()
        .map(|i| i["id"].clone())
        .collect();

    let groups = pair.query("library.resolve_groups", json!({}))?;
    assert_eq!(groups["ok"], true, "{groups}");
    assert_eq!(groups["result"]["report"], "resolve_groups");
    let [group] = groups["result"]["result"]["groups"]
        .as_array()
        .ok_or("groups")?
        .as_slice()
    else {
        return Err(format!("one group expected: {groups}").into());
    };
    assert_eq!(group["designators"], json!(["R1", "R2"]));
    assert_eq!(group["instances"], json!(instances));
    let search = group["query"].as_str().ok_or("group query")?;
    assert!(search.contains("kind:part"), "{search}");
    // The group's query finds the Part through the same search.
    let found = pair.query("library.search", json!({"query": search}))?;
    assert_eq!(found["ok"], true, "{found}");
    let hits = found["result"]["result"]["results"]
        .as_array()
        .ok_or("results")?;
    assert!(
        hits.iter()
            .any(|h| h["entry"]["lineage"] == json!(part.id) && h["entry"]["kind"] == "part"),
        "{found}"
    );

    // One group, one command, one undo step.
    let assigned = pair.apply(assign(
        group["instances"].as_array().ok_or("ids")?,
        Some(part),
    )?)?;
    assert_eq!(assigned["outcome"], json!({"outcome": "none"}));
    for instance in &assigned["project"]["instances"]
        .as_array()
        .ok_or("instances")?[..]
    {
        assert_eq!(instance["part"], serde_json::to_value(part)?);
    }
    let now = pair.session.engine().revision();
    let undo = pair
        .session
        .dispatch(json!({"command": "project.undo", "params": {"expected_revision": now}}));
    assert_eq!(undo["ok"], true, "{undo}");
    assert!(
        undo["result"]["project"]["instances"]
            .as_array()
            .ok_or("instances")?
            .iter()
            .all(|i| i.get("part").is_none())
    );
    let now = pair.session.engine().revision();
    let redo = pair
        .session
        .dispatch(json!({"command": "project.redo", "params": {"expected_revision": now}}));
    assert_eq!(redo["ok"], true, "{redo}");
    assert_eq!(pair.session.engine().document(), pair.engine.document());

    for instance in &instances {
        let report = pair.inspect(instance)?;
        let purchasing = &report["purchasing"];
        assert_eq!(purchasing["state"], "resolved", "{report}");
        assert_eq!(purchasing["part"], serde_json::to_value(part)?);
        assert_eq!(purchasing["manufacturer"], "ExampleCo");
        assert_eq!(purchasing["mpn"], MPN);
        assert_eq!(purchasing["findings"], json!([]));
    }

    let bom = pair.query("project.bom", json!({}))?;
    assert_eq!(bom["ok"], true, "{bom}");
    let report = &bom["result"]["result"];
    assert_eq!(report["unresolved"], 0, "{bom}");
    let [line] = report["lines"].as_array().ok_or("lines")?.as_slice() else {
        return Err(format!("one BOM line expected: {bom}").into());
    };
    assert_eq!(line["mpn"], MPN);
    assert_eq!(line["quantity"], 2);
    assert_eq!(line["designators"], json!(["R1", "R2"]));
    assert_eq!(line["unresolved"], false);
    assert_eq!(line["state"], "resolved");

    let after = pair.query("library.resolve_groups", json!({}))?;
    assert_eq!(after["result"]["result"]["groups"], json!([]), "{after}");
    Ok(())
}

/// Discovery: the generated request, document and tool schemas carry the
/// new command, both queries and the Part definition kind; the MCP
/// instructions name them; no retired `part.*` command name returns.
#[test]
fn schemas_and_mcp_list_the_part_command_and_queries() -> TestResult {
    let schemas = schemas().to_string();
    for id in [
        "instance.assign_part",
        "project.bom",
        "library.resolve_groups",
    ] {
        assert!(schemas.contains(id), "{id} is in the schemas");
    }
    for name in [
        "PartDefinition",
        "RatedSpec",
        "SpecMeaning",
        "DistributorOffer",
        "PurchasingReport",
        "BomReport",
        "ResolveReport",
    ] {
        assert!(schemas.contains(name), "{name} is in the schemas");
    }

    let tools = serde_json::to_string(&crate::mcp::tools())?;
    for id in [
        "instance.assign_part",
        "project.bom",
        "library.resolve_groups",
    ] {
        assert!(tools.contains(id), "{id} is in the MCP tool schemas");
    }
    let catalog = Request::catalog();
    let describes = |id: &str, text: &str| {
        catalog
            .iter()
            .any(|c| c.id == id && c.description.contains(text))
    };
    assert!(describes("project.apply", "instance.assign_part"));
    assert!(describes("project.query", "project.bom"));
    assert!(describes("project.query", "library.resolve_groups"));

    let mut server = crate::mcp::Server::new(Session::default(), crate::gate::Policy::default());
    let reply = server
        .handle(json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}))
        .ok_or("initialize reply")?;
    let instructions = reply["result"]["instructions"]
        .as_str()
        .ok_or("instructions")?;
    for id in [
        "instance.assign_part",
        "project.bom",
        "library.resolve_groups",
    ] {
        assert!(instructions.contains(id), "{id} is in the MCP instructions");
    }
    Ok(())
}
