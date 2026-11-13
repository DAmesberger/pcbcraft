//! Slice B5's gate: every row of the conflict table (docs/architecture/
//! board-interface.md, "The two-way check") through the JSON transport,
//! over the ESP32-S31 dev board example with an original headless
//! SolidCraft assembly, as in B4's gate (`assembly_link_tests`). Rows B4
//! already proves are named there: "a datum the linked design uses is gone"
//! is `b4_deleting_j1_breaks_the_cutout_and_blocks_publishing`, "the
//! linked design is missing or moved" `b4_a_missing_linked_design_keeps_the_snapshot`.
use super::Session;
use super::assembly_link_tests::{
    FAR_WALL_Y, changed, far_face, files, link, preflight, save_linked_design, status, with,
};
use super::interface_publish_tests::{
    TestResult, apply, faces, instance, linked_design, move_j1, ok, open, publish,
};
use serde_json::{Value, json};
use sparksmith_core::Operation;
use sparksmith_core::layout::outline::{BoardCommand, BoardEdit};
use sparksmith_core::library::footprint_source::{
    DimKind, DimValue, ParamName, SketchConstraintId, SketchCurveId, SketchEdit, SketchPointId,
    SketchRef,
};
use sparksmith_solidcraft::linked_design::{LinkedDesign, Operation as Extrude};
use std::path::Path;

/// `assembly.status`'s findings with `code`.
fn findings(status: &Value, code: &str) -> Vec<Value> {
    status["findings"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|f| f["code"] == code)
        .cloned()
        .collect()
}

/// SolidCraft's Update Links on the saved linked design at `path`, saved
/// again in place (as the user does in SolidCraft).
fn update_links(path: &Path) -> TestResult {
    let (mut assembly_link, newer) = LinkedDesign::open(&path.display().to_string())?;
    assert!(!newer);
    let update = assembly_link.update_links()?;
    assert!(update.failed.is_empty(), "{update:?}");
    let saved = path.with_extension("saving.solidcraft");
    let _ = std::fs::remove_file(&saved);
    assembly_link.save(&saved.display().to_string())?;
    std::fs::rename(&saved, path)?;
    Ok(())
}

/// The board's top edge held 0.5 mm from the far wall's inner face, which
/// the board follows as a followed face (B4's acceptance 7). Returns
/// the reply of `assembly.follow` and the fixed line the reference owns.
fn follow_top_edge(
    session: &mut Session,
    assembly_link: &Path,
    design: &Path,
) -> TestResult<(Value, String)> {
    let (face, pick) = far_face(assembly_link)?;
    let revision = session.engine().revision();
    let added = ok(
        session,
        json!({"command": "assembly.follow", "params": with(files(assembly_link, design), json!({
            "body": "Far wall", "face": face, "pick": pick, "name": "Far wall · inner face",
            "expected_revision": revision}))}),
    )?;
    let curve = added["assembly"]["references"][0]["sketch"]["curve"]
        .as_str()
        .ok_or("the reference owns a fixed line")?
        .to_owned();
    apply(
        session,
        Operation::Board(BoardCommand::Edit {
            edits: vec![
                BoardEdit::Sketch {
                    edit: SketchEdit::Delete {
                        ids: vec![SketchRef::Constraint(SketchConstraintId("k6".into()))],
                    },
                },
                BoardEdit::RemoveParameter {
                    name: ParamName("height".into()),
                },
                BoardEdit::Sketch {
                    edit: SketchEdit::AddDimension {
                        kind: DimKind::Distance,
                        refs: vec![
                            SketchRef::Point(SketchPointId("p2".into())),
                            SketchRef::Curve(SketchCurveId(curve.clone())),
                        ],
                        value: DimValue::Fixed(FAR_WALL_Y - 64.0),
                    },
                },
            ],
        }),
    )?;
    Ok((added, curve))
}

/// Conflict row "the linked design holds an older board": the board publishes
/// a new revision the linked design has not read. The SHA-256 SolidCraft keeps
/// on the linked design's link is not the last published file's, so the link's
/// status reads "linked design holds an older board" and says, in the user's
/// words, which revision it holds and what to do; it is advisory. After
/// Update Links in SolidCraft and an update from the linked design, it is gone.
/// The consumers the board read carry the slot that carries their datum.
#[test]
fn b5_the_linked_design_holds_an_older_board_until_it_updates_links() -> TestResult {
    let dir = tempfile::tempdir()?;
    let design = dir.path().join("board.solidcraft");
    let assembly_link = dir.path().join("assembly.solidcraft");
    let mut session = open(dir.path())?;
    let opening = format!("i{}.opening", instance(&session, "J1")?);
    let first = publish(&mut session, &design, "interface")?;
    assert_eq!(first["published"]["revision"], 1);
    save_linked_design(&design, &opening, FAR_WALL_Y, &assembly_link)?;
    let linked = link(&mut session, &assembly_link, &design)?;
    let sidecar = sparksmith_io::interface::read_sidecar(&design)?.ok_or("a sidecar")?;
    let datum = sidecar
        .datums
        .iter()
        .find(|d| d.token == opening)
        .ok_or("J1's opening")?;
    let consumers = linked["assembly"]["consumers"]
        .as_array()
        .ok_or("consumers")?;
    let cutout = consumers
        .iter()
        .find(|c| c["feature"] == "USB opening")
        .ok_or("the cutout uses J1's opening")?;
    assert_eq!(cutout["slot"], datum.slot.0, "by slot: {cutout}");
    assert!(
        linked["assembly"]["board_sha256"].is_string(),
        "the linked design's link keeps the board file's hash: {linked}"
    );
    let agreed = status(&mut session)?;
    assert!(
        findings(&agreed, "assembly.older_board").is_empty(),
        "{agreed}"
    );

    move_j1(&mut session, 2.0)?;
    let second = publish(&mut session, &design, "interface")?;
    assert_eq!(second["published"]["revision"], 2);
    let older = status(&mut session)?;
    assert_eq!(older["state"], "older_board", "{older}");
    assert_eq!(older["words"], "linked design holds an older board");
    let found = findings(&older, "assembly.older_board");
    let [finding] = found.as_slice() else {
        return Err(format!("{older}").into());
    };
    assert_eq!(
        finding["message"],
        "The linked design still uses board revision 1. Run Update Links in SolidCraft, then check \
         again"
    );
    let items = preflight(&mut session, "assembly.older_board")?;
    let [item] = items.as_slice() else {
        return Err(format!("{items:?}").into());
    };
    assert_eq!(item["severity"], "warning", "advisory: {item}");

    // Update Links in SolidCraft, then the board reads the linked design again.
    update_links(&assembly_link)?;
    assert!(changed(&mut session, &assembly_link, &design)?);
    let preview = ok(
        &mut session,
        json!({"command": "assembly.preview", "params": files(&assembly_link, &design)}),
    )?;
    assert!(preview["refused"].is_null(), "{preview}");
    assert!(
        preview["findings"].as_array().is_none_or(Vec::is_empty),
        "{preview}"
    );
    let revision = session.engine().revision();
    ok(
        &mut session,
        json!({"command": "assembly.update", "params": with(files(&assembly_link, &design), json!({
            "expect_hash": preview["expect_hash"], "expected_revision": revision}))}),
    )?;
    let current = status(&mut session)?;
    assert_eq!(current["state"], "linked", "{current}");
    assert!(findings(&current, "assembly.older_board").is_empty());
    Ok(())
}

/// Conflict row "both changed": the board published revision 2 (J1
/// moved) while the linked design, still holding revision 1, was edited and
/// saved (its far wall moved). Slice P1: the preview is not refused; it
/// lists what the board changed beside the linked design's changes and says
/// both changed. Nothing the board follows was changed on both sides, so
/// there is no side to choose: a choice for an item that is no conflict is
/// refused, changing nothing, and the update at the preview's hash is
/// applied. (Conflicting items and the choice: `sparksmith-solidcraft`'s
/// `board::fixed_tests`.)
#[test]
fn b5_both_changed_shows_both_sides() -> TestResult {
    let dir = tempfile::tempdir()?;
    let design = dir.path().join("board.solidcraft");
    let assembly_link = dir.path().join("assembly.solidcraft");
    let edited = dir.path().join("edited.solidcraft");
    let mut session = open(dir.path())?;
    let opening = format!("i{}.opening", instance(&session, "J1")?);
    publish(&mut session, &design, "interface")?;
    save_linked_design(&design, &opening, FAR_WALL_Y, &assembly_link)?;
    // The same linked design with its far wall moved, saved on revision 1.
    save_linked_design(&design, &opening, FAR_WALL_Y + 1.0, &edited)?;
    link(&mut session, &assembly_link, &design)?;
    move_j1(&mut session, 2.0)?;
    publish(&mut session, &design, "interface")?;
    std::fs::rename(&edited, &assembly_link)?;

    let before = session.engine().project().clone();
    let revision = session.engine().revision();
    let preview = ok(
        &mut session,
        json!({"command": "assembly.preview", "params": files(&assembly_link, &design)}),
    )?;
    assert!(preview["refused"].is_null(), "{preview}");
    assert!(
        preview["conflicts"].is_null(),
        "nothing to choose: {preview}"
    );
    let board = preview["board_changes"]
        .as_array()
        .ok_or("the board's changes")?;
    assert_eq!(
        board,
        &[json!("J1 opening, changed on the board")],
        "{preview}"
    );
    let found: Vec<&Value> = preview["findings"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|f| f["code"] == "assembly.both_changed")
        .collect();
    let [both] = found.as_slice() else {
        return Err(format!("{preview}").into());
    };
    assert_eq!(
        both["message"],
        "The board and the linked design both changed since they last agreed. Preview the update \
         and choose which side wins where they conflict"
    );
    let stray = session.dispatch(json!({"command": "assembly.update", "params": with(
        files(&assembly_link, &design), json!({
        "expect_hash": preview["expect_hash"], "expected_revision": revision,
        "resolve": [{"item": {"reference": 1}, "wins": "board"}]}))}));
    assert_eq!(stray["ok"], false, "{stray}");
    assert_eq!(stray["error"]["rule"], "assembly.both_changed", "{stray}");
    assert_eq!(session.engine().revision(), revision);
    assert_eq!(*session.engine().project(), before);
    ok(
        &mut session,
        json!({"command": "assembly.update", "params": with(
            files(&assembly_link, &design), json!({
            "expect_hash": preview["expect_hash"], "expected_revision": revision}))}),
    )?;
    assert_eq!(status(&mut session)?["state"], "older_board");
    Ok(())
}

/// Conflict row "a reference is on an unexported face": published with
/// the mechanical tier, the linked design's ‘Lid rib’ is a sketch projecting
/// a part's pad, which no slot carries. Reading the linked design names it
/// and the part in the user's words, as an advisory; the publish
/// preview's dry run names it too.
#[test]
fn b5_a_feature_on_an_unexported_face_is_named() -> TestResult {
    let dir = tempfile::tempdir()?;
    let design = dir.path().join("board.solidcraft");
    let assembly_link = dir.path().join("assembly.solidcraft");
    let mut session = open(dir.path())?;
    let opening = format!("i{}.opening", instance(&session, "J1")?);
    publish(&mut session, &design, "mechanical")?;
    let (mut built, _) = linked_design(&design, &opening)?;
    // A pad of a placed part, by its body's name in the published design,
    // which no slot carries, and a face of it that looks up.
    let sidecar = sparksmith_io::interface::read_sidecar(&design)?.ok_or("a sidecar")?;
    let tokens: Vec<&str> = sidecar.datums.iter().map(|d| d.token.as_str()).collect();
    let mut linked = LinkedDesign::new("Probe")?;
    let bodies = linked.link(&design.display().to_string(), "Board")?;
    let mut target = None;
    for body in &bodies {
        // A pad's body names its part's instance: `pad_<instance>_<pad>`.
        let Some(instance) = body
            .strip_prefix("pad_")
            .and_then(|rest| rest.split('_').next())
            .and_then(|n| n.parse::<u64>().ok())
        else {
            continue;
        };
        if tokens.contains(&body.as_str()) {
            continue;
        }
        let Some(designator) = session
            .engine()
            .project()
            .instances
            .iter()
            .find(|i| i.id == instance)
            .map(|i| i.designator.clone())
        else {
            continue;
        };
        if let Some(up) = faces(&mut built, body)?
            .into_iter()
            .find(|f| f.normal.is_some_and(|n| n[2] > 0.99))
        {
            target = Some((designator, body.clone(), up));
            break;
        }
    }
    let (designator, body, up) =
        target.ok_or_else(|| format!("a part's pad with a face that looks up: {bodies:?}"))?;
    built.projected(
        "Lid rib",
        [
            [0.0, 0.0, up.point[2] + 3.0],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
        ],
        &body,
        up.point,
        1.0,
        Extrude::New,
    )?;
    let errors = built.feature_errors();
    assert!(errors.is_empty(), "{errors:?}");
    built.save(&assembly_link.display().to_string())?;

    let linked = link(&mut session, &assembly_link, &design)?;
    let unexported = linked["assembly"]["unexported"]
        .as_array()
        .ok_or("unexported")?;
    assert!(
        unexported
            .iter()
            .any(|u| u["feature"] == "Lid rib" && u["body"] == body.as_str()),
        "{unexported:?}"
    );
    let consumers = linked["assembly"]["consumers"]
        .as_array()
        .ok_or("consumers")?;
    assert!(
        !consumers.iter().any(|c| c["feature"] == "Lid rib"),
        "a face no slot carries is no consumer: {consumers:?}"
    );
    let now = status(&mut session)?;
    let found = findings(&now, "assembly.unexported_face");
    let [finding] = found.as_slice() else {
        return Err(format!("{now}").into());
    };
    assert_eq!(
        finding["message"],
        format!(
            "The linked design's ‘Lid rib’ is attached to a face of {designator} that is not \
             exported. It may move to another part when the board changes. Export the face to \
             keep it stable"
        )
    );
    let items = preflight(&mut session, "assembly.unexported_face")?;
    assert!(
        items.iter().all(|i| i["severity"] == "warning"),
        "advisory: {items:?}"
    );

    let preview = ok(
        &mut session,
        json!({"command": "interface.publish_preview", "params": {
            "path": design.display().to_string(), "tier": "mechanical",
            "assembly_path": assembly_link.display().to_string()}}),
    )?;
    let effects = preview["dry_run"]["effects"]
        .as_array()
        .ok_or("the dry run")?;
    assert!(
        effects
            .iter()
            .any(|e| e["effect"] == "unexported_reference" && e["feature"] == "Lid rib"),
        "{effects:?}"
    );
    Ok(())
}

/// Conflict rows "a cycle" and "the board's driven outline was edited
/// locally", with the linked design half of the cycle graph read from the
/// file: the linked design's far wall is a sketch on a fixed plane
/// projecting the board slab's top face, so it is built on the board
/// outline; the board's top edge follows the far wall's inner face. That
/// face is the sketch's plane: the outline sets its extent, never its
/// position, so the loop settles (slice P1). It is named in both files'
/// words as a warning, reads no loop in the status and blocks nothing. The
/// wall's end face, placed by the projected outline edge, is built on the
/// outline by position (such a loop blocks fabrication output:
/// `sparksmith-solidcraft`'s `board::fixed_tests`). Deleting the line the
/// linked design drives is refused in the user's words.
#[test]
fn b5_a_loop_between_board_and_linked_design_is_found() -> TestResult {
    let dir = tempfile::tempdir()?;
    let design = dir.path().join("board.solidcraft");
    let assembly_link = dir.path().join("assembly.solidcraft");
    let mut session = open(dir.path())?;
    let opening = format!("i{}.opening", instance(&session, "J1")?);
    publish(&mut session, &design, "interface")?;
    let (mut built, _) = linked_design(&design, &opening)?;
    let top = faces(&mut built, "board")?
        .into_iter()
        .find(|f| f.normal.is_some_and(|n| n[1] > 0.99))
        .ok_or("the slab's top side face")?;
    built.projected(
        "Far wall",
        [[0.0, FAR_WALL_Y, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, -1.0]],
        "board",
        top.point,
        2.0,
        Extrude::New,
    )?;
    let errors = built.feature_errors();
    assert!(errors.is_empty(), "{errors:?}");
    built.save(&assembly_link.display().to_string())?;
    link(&mut session, &assembly_link, &design)?;

    let (added, curve) = follow_top_edge(&mut session, &assembly_link, &design)?;
    let built_on = added["assembly"]["references"][0]["built_on"]
        .as_array()
        .ok_or("the far wall is built on board datums")?;
    let outline = sidecar_datum(&design, "board.outline")?;
    assert!(
        built_on
            .iter()
            .any(|c| c["feature"] == "Far wall" && c["datum"] == outline.0),
        "{built_on:?}"
    );
    let by_outline: Vec<&Value> = built_on
        .iter()
        .filter(|c| c["datum"] == outline.0)
        .collect();
    assert!(
        by_outline.iter().all(|c| c["via"] == "extent"),
        "the inner face's extent only: {by_outline:?}"
    );
    let looped = status(&mut session)?;
    assert_eq!(looped["state"], "linked", "{looped}");
    assert!(findings(&looped, "assembly.cycle").is_empty(), "{looped}");
    let found = findings(&looped, "assembly.extent_cycle");
    let [finding] = found.as_slice() else {
        return Err(format!("{looped}").into());
    };
    let name = &outline.1;
    let capital = format!(
        "{}{}",
        name.chars()
            .next()
            .map(|c| c.to_uppercase().to_string())
            .unwrap_or_default(),
        name.chars().skip(1).collect::<String>()
    );
    assert_eq!(
        finding["message"],
        format!(
            "{capital} follows the linked design's Far wall · inner face, and the linked design \
             takes the extent of ‘Far wall’ from {name}, not its position. The loop settles; \
             stop following Far wall · inner face to break it"
        )
    );
    let items = preflight(&mut session, "assembly.extent_cycle")?;
    let [item] = items.as_slice() else {
        return Err(format!("{items:?}").into());
    };
    assert_eq!(item["severity"], "warning");
    assert!(preflight(&mut session, "assembly.cycle")?.is_empty());

    // The wall's end face is placed by the projected outline edge.
    let (mut read_back, newer) = LinkedDesign::open(&assembly_link.display().to_string())?;
    assert!(!newer);
    let end = faces(&mut read_back, "Far wall")?
        .into_iter()
        .find(|f| f.normal.is_some_and(|n| n[0] > 0.99))
        .ok_or("the far wall's end face")?;
    let revision = session.engine().revision();
    let added = ok(
        &mut session,
        json!({"command": "assembly.follow", "params": with(files(&assembly_link, &design), json!({
            "body": "Far wall", "face": end.name, "pick": end.point, "name": "Far wall · end",
            "expected_revision": revision}))}),
    )?;
    let end_on = added["assembly"]["references"][1]["built_on"]
        .as_array()
        .ok_or("the end face is built on board datums")?;
    assert!(
        end_on
            .iter()
            // A sketch use, the default, is not written out.
            .any(|c| c["datum"] == outline.0 && c.get("via").is_none_or(|v| v == "sketch")),
        "by position: {end_on:?}"
    );

    // The line the linked design drives is not the board's to delete.
    let revision = session.engine().revision();
    let delete = session.dispatch(json!({"command": "project.apply", "params": {
        "expected_revision": revision,
        "operation": serde_json::to_value(Operation::Board(BoardCommand::Edit {
            edits: vec![BoardEdit::Sketch { edit: SketchEdit::Delete {
                ids: vec![SketchRef::Curve(SketchCurveId(curve))] } }],
        }))?}}));
    assert_eq!(delete["ok"], false, "{delete}");
    assert!(
        delete
            .to_string()
            .contains("This edge follows the linked design's wall. Break the link to edit it here"),
        "{delete}"
    );
    assert_eq!(session.engine().revision(), revision);
    Ok(())
}

/// The published datum with `role`: its id and display name.
fn sidecar_datum(design: &Path, role: &str) -> TestResult<(u64, String)> {
    let sidecar = sparksmith_io::interface::read_sidecar(design)?.ok_or("a sidecar")?;
    let datum = sidecar
        .datums
        .iter()
        .find(|d| d.role == role)
        .ok_or(role.to_owned())?;
    Ok((datum.id.0, datum.name.clone()))
}

/// Rewrite the design container at `path` as a newer SolidCraft would
/// save it: its design's format number past what this build reads.
fn saved_by_a_newer_solidcraft(path: &Path) -> TestResult {
    use std::io::{Read, Write};
    let bytes = std::fs::read(path)?;
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))?;
    let mut out = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let mut rewritten = 0;
    for k in 0..archive.len() {
        let mut entry = archive.by_index(k)?;
        let name = entry.name()?.into_owned();
        let mut data = Vec::new();
        entry.read_to_end(&mut data)?;
        if let Ok(mut design) = serde_json::from_slice::<Value>(&data)
            && design["format"]
                .as_str()
                .is_some_and(|f| f.starts_with("solidcraft/"))
        {
            design["format"] = json!("solidcraft/9999");
            data = serde_json::to_vec(&design)?;
            rewritten += 1;
        }
        out.start_file(name, zip::write::SimpleFileOptions::default())?;
        out.write_all(&data)?;
    }
    assert_eq!(rewritten, 1, "one design in the container");
    std::fs::write(path, out.finish()?.into_inner())?;
    Ok(())
}

/// Conflict row "the linked design is from a newer SolidCraft": the reader
/// refuses the version in the user's words; nothing is applied and the
/// board keeps its snapshot.
#[test]
fn b5_a_linked_design_from_a_newer_solidcraft_is_refused() -> TestResult {
    let dir = tempfile::tempdir()?;
    let design = dir.path().join("board.solidcraft");
    let assembly_link = dir.path().join("assembly.solidcraft");
    let mut session = open(dir.path())?;
    let opening = format!("i{}.opening", instance(&session, "J1")?);
    publish(&mut session, &design, "interface")?;
    save_linked_design(&design, &opening, FAR_WALL_Y, &assembly_link)?;
    link(&mut session, &assembly_link, &design)?;
    saved_by_a_newer_solidcraft(&assembly_link)?;
    let before = session.engine().project().clone();
    let preview = session.dispatch(json!({"command": "assembly.preview",
        "params": files(&assembly_link, &design)}));
    assert_eq!(preview["ok"], false, "{preview}");
    assert!(
        preview.to_string().contains(
            "This linked design was saved by a newer SolidCraft than this Sparksmith can read"
        ),
        "{preview}"
    );
    assert_eq!(*session.engine().project(), before);
    assert_eq!(status(&mut session)?["state"], "linked");
    Ok(())
}

/// Slice P1, item 6: SolidCraft names a package's bodies by the package
/// ("pkg15r1 body", then "pkg15r1 body (pkg15r1:2)" for the next part of
/// that package), so the published sidecar maps each part body to its
/// part. A linked design feature on a face of the second part's body is
/// named with the second part's designator, never the first's, in the
/// finding and in the publish dry run.
#[test]
fn a_feature_on_a_repeated_package_names_its_own_part() -> TestResult {
    let dir = tempfile::tempdir()?;
    let design = dir.path().join("board.solidcraft");
    let assembly_link = dir.path().join("assembly.solidcraft");
    let mut session = open(dir.path())?;
    let opening = format!("i{}.opening", instance(&session, "J1")?);
    publish(&mut session, &design, "mechanical")?;
    let sidecar = sparksmith_io::interface::read_sidecar(&design)?.ok_or("a sidecar")?;
    // A body of a package placed more than once, at its second placement,
    // and the first placement's same body.
    let (second, first) = sidecar
        .parts
        .iter()
        .find_map(|p| {
            let (source, _) = p.body.split_once(" (")?;
            let first = sidecar.part_of(source)?;
            Some((p.clone(), first.clone()))
        })
        .ok_or("a package placed twice")?;
    assert_ne!(second.instance, first.instance);
    assert_ne!(second.designator, first.designator);
    let (mut built, _) = linked_design(&design, &opening)?;
    let up = faces(&mut built, &second.body)?
        .into_iter()
        .find(|f| f.normal.is_some_and(|n| n[2] > 0.99))
        .ok_or("a face of the second part that looks up")?;
    built.projected(
        "Lid rib",
        [
            [0.0, 0.0, up.point[2] + 3.0],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
        ],
        &second.body,
        up.point,
        1.0,
        Extrude::New,
    )?;
    let errors = built.feature_errors();
    assert!(errors.is_empty(), "{errors:?}");
    built.save(&assembly_link.display().to_string())?;

    let linked = link(&mut session, &assembly_link, &design)?;
    let unexported = linked["assembly"]["unexported"]
        .as_array()
        .ok_or("unexported")?;
    assert!(
        unexported.iter().any(|u| u["feature"] == "Lid rib"
            && u["body"] == second.body.as_str()
            && u["part"] == second.instance),
        "{unexported:?}"
    );
    let now = status(&mut session)?;
    let found = findings(&now, "assembly.unexported_face");
    let [finding] = found.as_slice() else {
        return Err(format!("{now}").into());
    };
    let message = finding["message"].as_str().ok_or("a message")?;
    assert_eq!(
        message,
        format!(
            "The linked design's ‘Lid rib’ is attached to a face of {} that is not exported. It \
             may move to another part when the board changes. Export the face to keep it stable",
            second.designator
        )
    );
    assert_eq!(finding["objects"][0]["id"], second.instance, "{finding}");

    let preview = ok(
        &mut session,
        json!({"command": "interface.publish_preview", "params": {
            "path": design.display().to_string(), "tier": "mechanical",
            "assembly_path": assembly_link.display().to_string()}}),
    )?;
    let effects = preview["dry_run"]["effects"]
        .as_array()
        .ok_or("the dry run")?;
    let rib = effects
        .iter()
        .find(|e| e["effect"] == "unexported_reference" && e["feature"] == "Lid rib")
        .ok_or("the dry run names the rib")?;
    let words = rib["message"].as_str().ok_or("words")?;
    assert!(
        words.contains(&format!("a face of {} ", second.designator)),
        "{words}"
    );
    Ok(())
}
