//! Slice C7: the footprint and model steps of the EXM-48 scenario over
//! JSON. The pinout import leaves the package and binding drafts without
//! pads; the agent reads the package drawing, finds a package row with
//! `library.templates` (B1), builds package, footprint draft and lands
//! from the row and lists the footprint in the binding with one
//! `library.package_from_template` (previewed by `library.package_plan`),
//! and checks the generated model with
//! `library.models` (D1). `pinout_tests::publish_and_place` then publishes
//! the whole working set and places U1 with its physical side.
use super::pinout_tests::{Fallible, Imported, Scenario, codes, library_op};
use super::*;

/// The diagnostic code of a refused reply.
fn refused(reply: &Value) -> Option<String> {
    (reply["ok"] == false).then(|| {
        reply["error"]["code"]
            .as_str()
            .unwrap_or_default()
            .to_owned()
    })
}

#[test]
fn the_template_queries_answer_on_the_wire() -> Fallible<()> {
    let mut session = Session::new(Engine::demo()?);
    let reply = session.dispatch(json!({"command": "project.query",
        "params": {"query": "library.templates", "params": {"query": "soic pins:8"}}}));
    assert_eq!(refused(&reply), None, "{reply}");
    // The SOIC-8 custom entry of the gull-wing family.
    let hits = reply["result"]["result"]["hits"].as_array().ok_or("hits")?;
    assert!(hits.iter().any(|h| h["family"] == "gull_wing"), "{reply}");
    let reply = session.dispatch(json!({"command": "project.query",
        "params": {"query": "library.templates", "params": {"query": "kind:part"}}}));
    assert_eq!(refused(&reply).as_deref(), Some("invalid_input"), "{reply}");
    Ok(())
}

#[test]
fn the_exm48_package_is_found_among_the_no_lead_templates() -> Fallible<()> {
    let mut s = Scenario::demo()?;
    let report = s.query(
        "templates",
        "library.templates",
        json!({"query": "qfn pins:48", "limit": 8}),
    )?;
    let hits = report["hits"].as_array().ok_or("hits")?;
    assert!(!hits.is_empty(), "{report}");
    assert!(hits.len() <= 8, "the limit holds: {report}");
    assert!(report["more"].is_u64(), "{report}");
    assert!(
        hits.iter().all(|h| h["family"] == "no_lead"),
        "a QFN is a no-lead package: {report}"
    );
    Ok(())
}

/// The physical side the footprint step built: package, footprint and
/// binding drafts of the imported component.
pub(super) struct Physical {
    pub package: u64,
    pub footprint: u64,
    pub binding: u64,
}

/// The pads of a footprint draft as (pad number, package terminal).
fn pads(draft: &Value) -> Vec<(String, u64)> {
    draft["draft"]["definition"]["body"]["pads"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| Some((p["number"].as_str()?.to_owned(), p["terminal"].as_u64()?)))
        .collect()
}

/// Steps 8a–8d: the package drawing says QFN-48, 7 × 7 mm, 0.5 mm pitch
/// with a 5.3 mm exposed pad. Find the row; preview and build package,
/// footprint draft, lands and the binding's footprint listing from the row
/// in one request; and check the model the package now generates.
pub(super) fn build_footprint(s: &mut Scenario, imported: &Imported) -> Fallible<Physical> {
    let binding = imported.binding;
    let binding_body = s.draft(binding)?["draft"]["definition"]["body"].clone();
    assert_eq!(binding_body["component"], imported.component);
    assert_eq!(
        binding_body["footprints"],
        json!([]),
        "the import made no footprint"
    );
    let package = binding_body["package"]["id"]
        .as_u64()
        .ok_or("the imported package")?;
    let before = s.draft(package)?;
    assert_eq!(before["draft"]["definition"]["kind"], "package");
    let terminals = before["draft"]["definition"]["body"]["terminals"]
        .as_array()
        .ok_or("package terminals")?
        .clone();
    assert_eq!(terminals.len(), 49, "48 leads and the exposed pad");

    // 8a. The row: a built-in starter row named by the drawing's values.
    let report = s.query(
        "templates",
        "library.templates",
        json!({"query": "qfn pins:48 pitch:0.5 body:7x7"}),
    )?;
    let hit = report["hits"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|h| h["starter"].as_str().is_some_and(|n| n.contains("EP 5.3")))
        .ok_or_else(|| format!("a QFN-48 7×7 0.5 EP 5.3 row: {report}"))?
        .clone();
    assert_eq!(hit["family"], "no_lead", "{hit}");
    let row = json!({"source": "starter", "name": hit["starter"]});

    // 8b. Preview, then build package, a footprint draft for the imported
    // package (the import made none) with its lands, and its listing in
    // the one binding draft from the row: one request, one undo step.
    // The datasheet numbers the exposed pad 49; the QFN form calls it EP.
    // Matched by number, that is reported, not guessed: 48 kept, 49
    // removed, EP new and unmapped. (This step used to rely on terminals
    // being reused by list order, which renamed 49 to EP silently.)
    let unstated = s.query(
        "package plan by number",
        "library.package_plan",
        json!({"request": {"draft": package, "row": row}}),
    )?;
    assert_eq!(unstated["terminals"]["strategy"], "by_number", "{unstated}");
    assert_eq!(
        (
            &unstated["terminals"]["kept"],
            &unstated["terminals"]["added"],
            &unstated["terminals"]["removed"]
        ),
        (&json!(48), &json!(1), &json!(1)),
        "{unstated}"
    );
    let unmatched = unstated["findings"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|f| f["code"] == "package.terminals_unmatched")
        .ok_or_else(|| format!("the mismatch is reported: {unstated}"))?;
    let message = unmatched["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("new terminal(s) EP") && message.contains("current terminal(s) 49"),
        "{message}"
    );
    // The person states that 49 is the exposed pad.
    let request = json!({"draft": package, "row": row, "renumber": [{"from": "49", "to": "EP"}]});
    let plan = s.query(
        "package plan",
        "library.package_plan",
        json!({"request": request}),
    )?;
    assert_eq!(plan["terminals_before"], 49, "{plan}");
    assert_eq!(plan["terminals_after"], 49, "{plan}");
    assert_eq!(plan["pads_after"], 49, "{plan}");
    assert!(plan["model_family_rev"].is_u64(), "{plan}");
    assert_eq!(plan["footprint_created"], true, "{plan}");
    assert_eq!(plan["footprint_bound"], binding, "{plan}");
    assert_eq!(
        plan["terminals"]["summary"],
        "Terminals matched by number: 49 kept, 0 added, 0 removed, 1 renumbered as requested (49 → EP).",
        "{plan}"
    );
    let applied = s.apply(
        "package from template",
        library_op("library.package_from_template", request),
    )?;
    let applied = &applied["outcome"]["plan"];
    assert_eq!(applied["footprint"], plan["footprint"], "{applied}");
    let footprint = applied["footprint"]
        .as_u64()
        .ok_or_else(|| format!("the created footprint: {applied}"))?;
    // One undo step takes back the footprint draft and its listing too.
    s.send("undo", json!({"command": "project.undo", "params": {}}))?;
    assert!(s.draft(footprint).is_err(), "undo removes the footprint");
    assert_eq!(
        s.draft(binding)?["draft"]["definition"]["body"]["footprints"],
        json!([])
    );
    s.send("redo", json!({"command": "project.redo", "params": {}}))?;
    // Terminal identities are kept, so the import's mappings still hold;
    // leads keep their numbers and the 49th is designated EP.
    let built = s.draft(package)?;
    let after = built["draft"]["definition"]["body"]["terminals"]
        .as_array()
        .ok_or("package terminals")?;
    let ids = |list: &[Value]| -> Vec<Value> { list.iter().map(|t| t["id"].clone()).collect() };
    assert_eq!(ids(after), ids(&terminals), "terminal identity survives");
    assert_eq!(after[25]["designation"], "26");
    assert_eq!(after[48]["designation"], "EP");
    assert_eq!(
        built["draft"]["definition"]["body"]["form"]["family"],
        "no_lead"
    );
    let lands = pads(&s.draft(footprint)?);
    assert_eq!(lands.len(), 49, "one land per terminal");
    let pa9 = after[25]["id"].as_u64().ok_or("terminal 26")?;
    assert!(
        lands.iter().any(|(number, t)| number == "26" && *t == pa9),
        "pad 26 lands PA9's package terminal: {lands:?}"
    );
    // A refusal: the footprint has pads now, so a second build needs
    // replace and changes nothing without it.
    s.refuse_apply(
        "package from template again",
        library_op(
            "library.package_from_template",
            json!({"draft": package, "row": row}),
        ),
        "conflict",
    )?;

    // 8c. The binding offers the footprint, pinned at its first revision.
    let listed = s.draft(binding)?["draft"]["definition"]["body"]["footprints"].clone();
    assert_eq!(
        listed,
        json!([{"id": footprint, "revision": 1}]),
        "the binding lists the created footprint"
    );
    let named = s.draft(footprint)?["draft"]["meta"]["name"].clone();
    assert!(
        named.as_str().is_some_and(|n| n.ends_with(" footprint")),
        "{named}"
    );
    for id in [package, footprint, binding] {
        let check = &s.draft(id)?["check"];
        assert_eq!(check["result"]["publishable"], true, "{id}: {check}");
        assert!(
            !codes(check).iter().any(|c| c.ends_with(".unsupported")),
            "{check}"
        );
    }

    // 8d. The model: the chooser for this package ranks the generated
    // no-lead model first, with its fit.
    let models = s.query(
        "models",
        "library.models",
        json!({"query": "", "fits": package}),
    )?;
    let first = models["hits"]
        .as_array()
        .and_then(|h| h.first())
        .ok_or_else(|| format!("model hits: {models}"))?;
    assert_eq!(
        first["kind"],
        json!({"hit": "generated", "family": "no_lead"}),
        "{models}"
    );
    assert_eq!(first["fit"]["fit"], "bounds", "{models}");
    Ok(Physical {
        package,
        footprint,
        binding,
    })
}
