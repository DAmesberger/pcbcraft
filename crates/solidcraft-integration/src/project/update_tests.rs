//! Updating a built board in place ([`update`]) gives the model a fresh
//! build gives, and only when the board changed in values.
use super::tests::via_board;
use super::*;
use pcbcraft_core::{Engine, Track};

/// Every native body of `a` and `b` alike, in order: kind, source, layer,
/// box and volume; and the same sources. The pieces of a laminate slab
/// (laminate left in a fill's clearances) may be numbered in another order
/// after an update, so the laminate's bodies are compared by where they
/// lie.
fn same_model(a: &NativeModel, b: &NativeModel) -> Result<()> {
    let native = |m: &NativeModel| -> Vec<Geometry> {
        let mut bodies: Vec<Geometry> = m
            .geometry
            .iter()
            .filter(|g| !g.kind.is_appearance())
            .cloned()
            .collect();
        let laminate = |g: &Geometry| g.kind == BodyKind::Substrate;
        let at = bodies.iter().position(laminate).unwrap_or(bodies.len());
        let end = at + bodies[at..].iter().take_while(|g| laminate(g)).count();
        bodies[at..end].sort_by(|g, h| {
            let key = |g: &Geometry| [g.min[2], g.min[0], g.min[1], g.max[0], g.max[1]];
            key(g)
                .iter()
                .zip(key(h))
                .map(|(p, q)| p.total_cmp(&q))
                .find(|o| o.is_ne())
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        bodies
    };
    let (x, y) = (native(a), native(b));
    assert_eq!(x.len(), y.len(), "native bodies");
    for (k, (g, h)) in x.iter().zip(&y).enumerate() {
        assert_eq!(
            (g.kind, g.item, g.layer),
            (h.kind, h.item, h.layer),
            "body {k}"
        );
        for axis in 0..3 {
            for (p, q) in [(g.min[axis], h.min[axis]), (g.max[axis], h.max[axis])] {
                assert!((p - q).abs() < 1e-6, "body {k} {:?}: {p} != {q}", g.kind);
            }
        }
        let (v, w) = (g.volume(), h.volume());
        assert!(
            (v - w).abs() <= 1e-6 * (1.0 + w.abs()),
            "body {k} {:?} volume {v} != {w}",
            g.kind
        );
    }
    // The display layers are drawn from the same board.
    let shown = |m: &NativeModel| {
        m.geometry
            .iter()
            .filter(|g| g.kind.is_appearance())
            .map(|g| (g.kind, g.item, g.mesh.triangles.len()))
            .collect::<Vec<_>>()
    };
    assert_eq!(shown(a), shown(b), "display layers");
    assert_eq!(
        serde_json::to_value(&a.sources)?,
        serde_json::to_value(&b.sources)?
    );
    // The saved metadata (sources, kinds, vias) is the fresh build's.
    assert_eq!(
        a.exact()?.session().doc.metadata,
        b.exact()?.session().doc.metadata
    );
    Ok(())
}

/// [`same_model`] for a model updated with objects added or removed: an
/// added object's component and occurrence get the design's next ids and
/// its features come at the end of the history, so the native bodies are
/// compared as sets (each by kind, source, layer, box and volume) and the
/// sources and saved identity without the native component and occurrence
/// ids. Everything else is compared as [`same_model`] compares it.
fn same_board(a: &NativeModel, b: &NativeModel) -> Result<()> {
    let native = |m: &NativeModel| -> Vec<Geometry> {
        let mut bodies: Vec<Geometry> = m
            .geometry
            .iter()
            .filter(|g| !g.kind.is_appearance())
            .cloned()
            .collect();
        bodies.sort_by(|g, h| {
            let key = |g: &Geometry| (format!("{:?}", g.kind), g.item, format!("{:?}", g.layer));
            let at = |g: &Geometry| [g.min[2], g.min[0], g.min[1], g.max[0], g.max[1], g.max[2]];
            key(g).cmp(&key(h)).then_with(|| {
                at(g)
                    .iter()
                    .zip(at(h))
                    .map(|(p, q)| p.total_cmp(&q))
                    .find(|o| o.is_ne())
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
        });
        bodies
    };
    let (x, y) = (native(a), native(b));
    assert_eq!(x.len(), y.len(), "native bodies");
    for (k, (g, h)) in x.iter().zip(&y).enumerate() {
        assert_eq!(
            (g.kind, g.item, g.layer),
            (h.kind, h.item, h.layer),
            "body {k}"
        );
        for axis in 0..3 {
            for (p, q) in [(g.min[axis], h.min[axis]), (g.max[axis], h.max[axis])] {
                assert!((p - q).abs() < 1e-6, "body {k} {:?}: {p} != {q}", g.kind);
            }
        }
        let (v, w) = (g.volume(), h.volume());
        assert!(
            (v - w).abs() <= 1e-6 * (1.0 + w.abs()),
            "body {k} {:?} volume {v} != {w}",
            g.kind
        );
    }
    let shown = |m: &NativeModel| {
        m.geometry
            .iter()
            .filter(|g| g.kind.is_appearance())
            .map(|g| (g.kind, g.item, g.mesh.triangles.len()))
            .collect::<Vec<_>>()
    };
    assert_eq!(shown(a), shown(b), "display layers");
    let anonymous = |sources: &[Source]| -> Result<Vec<serde_json::Value>> {
        sources
            .iter()
            .map(|s| {
                let mut v = serde_json::to_value(s)?;
                if let Some(map) = v.as_object_mut() {
                    map.remove("component");
                    map.remove("occurrence");
                }
                Ok(v)
            })
            .collect()
    };
    assert_eq!(anonymous(&a.sources)?, anonymous(&b.sources)?, "sources");
    let (saved_a, kinds_a) = identity::read(a.exact()?)?;
    let (saved_b, kinds_b) = identity::read(b.exact()?)?;
    assert_eq!(anonymous(&saved_a)?, anonymous(&saved_b)?, "saved sources");
    assert_eq!(kinds_a, kinds_b, "saved body kinds");
    let other = |m: &Native| {
        m.session()
            .doc
            .metadata
            .0
            .iter()
            .filter(|(k, _)| !k.starts_with("org.pcbcraft.electronics.identity."))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect::<Vec<_>>()
    };
    assert_eq!(other(a.exact()?), other(b.exact()?), "other saved metadata");
    // Every body keeps the look a fresh build assigns, and no body that is
    // gone keeps one.
    let looks = |m: &Native| m.session().doc.appearances.bodies.clone();
    assert_eq!(looks(a.exact()?), looks(b.exact()?), "body looks");
    Ok(())
}

/// Update `model` to `project` and check it matches a fresh build as
/// [`same_board`] compares them.
fn restructured(model: NativeModel, project: &Project) -> Result<(NativeModel, Rebuilt)> {
    let (model, how) = update(model, project, &SourceCache::default(), None)?;
    same_board(&model, &build(project)?)?;
    Ok((model, how))
}

/// A small 4-layer board: the showcase with vias and inner tracks.
fn board() -> Result<Project> {
    via_board(40)
}

/// Update `model` to `project`, check it matches a fresh build, and return
/// it with how it was made.
fn updated(model: NativeModel, project: &Project) -> Result<(NativeModel, Rebuilt)> {
    let (model, how) = update(model, project, &SourceCache::default(), None)?;
    same_model(&model, &build(project)?)?;
    Ok((model, how))
}

/// Value edits update the model in place, each one as a fresh build of
/// the edited board would be: a part moved, an outer track's end and
/// width, vias moved (the first of their size too), an inner track moved
/// (its foil slab redrawn), and the board resized.
#[test]
fn value_edits_update_the_board_in_place_as_a_fresh_build_would() -> Result<()> {
    let mut project = board()?;
    let mut model = build(&project)?;
    let in_place = |how: Rebuilt| assert!(matches!(how, Rebuilt::InPlace(n) if n > 0), "{how:?}");
    type Edit = Box<dyn Fn(&mut Project)>;
    let edits: Vec<(&str, Edit)> = vec![
        (
            "a part moved",
            Box::new(|p: &mut Project| {
                if let Some(placement) = p.placements.first_mut() {
                    placement.position[0] += 1.5;
                    placement.position[1] -= 0.25;
                }
            }),
        ),
        (
            "an outer track's end and width",
            Box::new(|p: &mut Project| {
                if let Some(track) = p.tracks.iter_mut().find(|t| t.layer.side().is_some()) {
                    track.end[0] += 0.5;
                    track.width = 0.3;
                }
            }),
        ),
        (
            "two vias moved, one the first of its size",
            Box::new(|p: &mut Project| {
                for k in [0, 7] {
                    if let Some(via) = p.vias.get_mut(k) {
                        via.position[1] += 0.4;
                    }
                }
            }),
        ),
        (
            "an inner track moved",
            Box::new(|p: &mut Project| {
                if let Some(track) = p.tracks.iter_mut().find(|t| t.layer.side().is_none()) {
                    track.start[1] += 0.3;
                    track.end[1] += 0.3;
                }
            }),
        ),
        (
            "the board resized",
            Box::new(|p: &mut Project| {
                p.board_size[0] += 2.0;
                p.board_size[1] += 1.0;
            }),
        ),
    ];
    for (what, edit) in edits {
        edit(&mut project);
        let (next, how) = updated(model, &project)?;
        println!("{what}: {how:?}");
        in_place(how);
        model = next;
    }
    // An update of an unchanged board runs nothing.
    let (_, how) = updated(model, &project)?;
    assert_eq!(how, Rebuilt::InPlace(0));
    Ok(())
}

/// Tracks added (as a carry or the router adds them) and removed update the
/// model in place, as a fresh build of the edited board would be: an outer
/// and an inner track added, the added one moved, an original removed, the
/// added ones removed again. A new via size is still built anew.
#[test]
fn tracks_added_and_removed_update_the_board_in_place() -> Result<()> {
    let mut project = board()?;
    let mut model = build(&project)?;
    let in_place = |what: &str, how: &Rebuilt| {
        assert!(
            matches!(how, Rebuilt::InPlace(n) if *n > 0),
            "{what}: {how:?}"
        );
    };
    let net = project.nets[0].id;
    let inner = project
        .tracks
        .iter()
        .find(|t| t.layer.side().is_none())
        .map(|t| t.layer)
        .ok_or_else(|| Error::Verification("an inner track".into()))?;
    let mut added = Vec::new();
    for (start, end, layer) in [
        ([2.0, 2.0], [6.0, 2.0], CopperLayer::Top),
        ([2.0, 3.0], [5.0, 4.0], inner),
    ] {
        let id = project
            .allocate()
            .map_err(|e| Error::Verification(e.to_string()))?;
        project.tracks.push(Track {
            id,
            net,
            start,
            end,
            width: 0.2,
            layer,
            locked: false,
        });
        added.push(id);
        let (next, how) = restructured(model, &project)?;
        in_place("a track added", &how);
        model = next;
    }
    // The added track moved: a value of the object the update made.
    if let Some(track) = project.tracks.iter_mut().find(|t| t.id == added[0]) {
        track.end[1] += 0.5;
        track.width = 0.25;
    }
    let (next, how) = restructured(model, &project)?;
    in_place("an added track edited", &how);
    model = next;
    // Unchanged: nothing runs.
    let (next, how) = restructured(model, &project)?;
    assert_eq!(how, Rebuilt::InPlace(0));
    model = next;
    // An original track removed, then the added ones.
    let first = project.tracks.remove(0).id;
    assert!(!added.contains(&first));
    let (next, how) = restructured(model, &project)?;
    in_place("a track removed", &how);
    model = next;
    project.tracks.retain(|t| !added.contains(&t.id));
    let (_, how) = restructured(model, &project)?;
    in_place("the added tracks removed", &how);
    Ok(())
}

/// Vias added and removed (as the router adds them with a layer change)
/// update the model in place, as a fresh build of the edited board would
/// be: a routed track with a via, more vias of that size, the first via of
/// a size removed (the next one becomes the definition's first), the added
/// ones removed. A via of a new size is still built anew.
#[test]
fn vias_added_and_removed_update_the_board_in_place() -> Result<()> {
    let mut project = board()?;
    let mut model = build(&project)?;
    let in_place = |what: &str, how: &Rebuilt| {
        assert!(
            matches!(how, Rebuilt::InPlace(n) if *n > 0),
            "{what}: {how:?}"
        );
    };
    let template = project
        .vias
        .first()
        .cloned()
        .ok_or_else(|| Error::Verification("a via".into()))?;
    let mut added = Vec::new();
    // A routed step: a track and a via at its end.
    let track = project
        .allocate()
        .map_err(|e| Error::Verification(e.to_string()))?;
    project.tracks.push(Track {
        id: track,
        net: template.net,
        start: [3.0, 3.0],
        end: [7.0, 3.0],
        width: 0.2,
        layer: CopperLayer::Top,
        locked: false,
    });
    for at in [[7.0, 3.0], [8.5, 3.5], [9.5, 4.5]] {
        let mut via = template.clone();
        via.id = project
            .allocate()
            .map_err(|e| Error::Verification(e.to_string()))?;
        via.position = at;
        added.push(via.id);
        project.vias.push(via);
        let (next, how) = restructured(model, &project)?;
        in_place("a via added", &how);
        model = next;
    }
    // The first via of its size removed, and one in the middle moved.
    project.vias.remove(0);
    if let Some(via) = project.vias.get_mut(2) {
        via.position[1] += 0.3;
    }
    let (next, how) = restructured(model, &project)?;
    in_place("the first via removed", &how);
    model = next;
    project.vias.retain(|v| !added.contains(&v.id));
    project.tracks.retain(|t| t.id != track);
    let (next, how) = restructured(model, &project)?;
    in_place("the routed step removed", &how);
    model = next;
    // A via of a new size needs a definition the board did not have.
    let mut via = project
        .vias
        .first()
        .cloned()
        .ok_or_else(|| Error::Verification("a via".into()))?;
    via.id = project
        .allocate()
        .map_err(|e| Error::Verification(e.to_string()))?;
    via.diameter += 0.1;
    via.position[0] += 1.0;
    project.vias.push(via);
    let (_, how) = restructured(model, &project)?;
    assert!(
        matches!(&how, Rebuilt::Anew(Some(why)) if why.contains("more than values")),
        "{how:?}"
    );
    Ok(())
}

/// On the shipped showcase (a sketched board with a cutout and a slot,
/// a bottom and a top pour and an inner plane), a pour refilled after a
/// stitching via moved, and a pour's copper alone changed, update the
/// model in place as a fresh build would: the tiles whose copper changed
/// are built again (at the end of the history, so the bodies compare as
/// [`same_board`] compares them), and the slabs with the inner plane's
/// foil level and the drills are redrawn.
#[test]
fn fill_edits_update_the_filled_board_in_place() -> Result<()> {
    use pcbcraft_core::layout::fill::{RefillTarget, refill};
    let mut project = Engine::showcase_hosted_fixture(pcbcraft_core::Evaluators {
        footprint: Some(&crate::footprint::SketchEvaluator),
        board: Some(&crate::board::SketchBoardEvaluator),
    })
    .map_err(|e| Error::Verification(e.to_string()))?
    .project()
    .clone();
    assert!(!project.board_shape.is_rectangle());
    assert!(project.fills.iter().any(|f| f.computed.is_some()));
    let refilled = |p: &mut Project| -> Result<()> {
        for (id, copper) in
            refill(p, RefillTarget::All).map_err(|e| Error::Verification(e.to_string()))?
        {
            if let Some(fill) = p.fills.iter_mut().find(|f| f.id == id) {
                fill.computed = Some(copper);
            }
        }
        Ok(())
    };
    let mut model = build(&project)?;
    let plane = project
        .fills
        .iter()
        .find(|f| f.layer.side().is_none())
        .and_then(|f| f.net)
        .ok_or_else(|| Error::Verification("inner plane".into()))?;
    // A stitching via of the plane's net, moved within the plane.
    let via = project
        .vias
        .iter()
        .position(|v| v.net == plane)
        .ok_or_else(|| Error::Verification("stitching via".into()))?;
    project.vias[via].position[0] += 0.2;
    refilled(&mut project)?;
    let (next, how) = restructured(model, &project)?;
    assert!(
        matches!(how, Rebuilt::InPlace(n) if n > 0),
        "via moved, refilled: {how:?}"
    );
    model = next;
    // The inner plane's copper alone: a notch off a corner of its first
    // region.
    let fill = project
        .fills
        .iter_mut()
        .find(|f| f.layer.side().is_none())
        .and_then(|f| f.computed.as_mut())
        .and_then(|c| c.regions.first_mut())
        .ok_or_else(|| Error::Verification("plane copper".into()))?;
    let corner = fill.outer[0];
    let next = fill.outer[1 % fill.outer.len()];
    fill.outer[0] = [
        corner[0] + 0.1 * (next[0] - corner[0]),
        corner[1] + 0.1 * (next[1] - corner[1]),
    ];
    let (_, how) = restructured(model, &project)?;
    assert!(
        matches!(how, Rebuilt::InPlace(n) if n > 0),
        "plane copper: {how:?}"
    );
    Ok(())
}

/// A refill that changes how many regions a pour has (a part moved, a
/// thermal split off an island) updates the model in place, as a fresh
/// build would: a region added to the bottom pour, then the first region
/// removed. Only the tiles whose copper changed are built again: the
/// update runs a few commands per changed tile, not the pour's.
#[test]
fn a_refill_of_another_number_of_regions_updates_in_place() -> Result<()> {
    let mut project = Engine::showcase_hosted_fixture(pcbcraft_core::Evaluators {
        footprint: Some(&crate::footprint::SketchEvaluator),
        board: Some(&crate::board::SketchBoardEvaluator),
    })
    .map_err(|e| Error::Verification(e.to_string()))?
    .project()
    .clone();
    let mut model = build(&project)?;
    let bottom = project
        .fills
        .iter()
        .position(|f| f.layer == CopperLayer::Bottom && f.computed.is_some())
        .ok_or_else(|| Error::Verification("a bottom pour".into()))?;
    let tiles_of = |p: &Project| -> usize {
        p.fills[bottom].computed.as_ref().map_or(0, |c| {
            fill::tiles(&c.regions, fill::TILE_MM, fill::MODEL_CHORD_MM).len()
        })
    };
    let before = tiles_of(&project);
    // A small island in a corner of the board, in one tile of its own.
    if let Some(copper) = project.fills[bottom].computed.as_mut() {
        copper
            .regions
            .push(pcbcraft_core::model::fill::FilledRegion {
                outer: vec![[0.6, 0.6], [1.6, 0.6], [1.6, 1.2], [0.6, 1.2]],
                holes: vec![],
            });
    }
    let (next, how) = restructured(model, &project)?;
    assert!(
        matches!(how, Rebuilt::InPlace(n) if n > 0),
        "a region added: {how:?}"
    );
    model = next;
    let added = tiles_of(&project);
    assert!(added > before, "{added} tiles against {before}");
    // The pour's first region gone (refilled without it).
    if let Some(copper) = project.fills[bottom].computed.as_mut() {
        copper.regions.remove(0);
    }
    let (_, how) = restructured(model, &project)?;
    assert!(
        matches!(how, Rebuilt::InPlace(n) if n > 0 && n < 4 * added),
        "a region removed: {how:?}"
    );
    Ok(())
}

/// Every part turned by 90° and back (an edit and its undo) updates the
/// model in place, as a fresh build would: a pad offset that rounds to a
/// negative zero after the turn writes the same joint expression as before
/// (`0 mm`, not `-0 mm`), so the board is not built anew.
#[test]
fn a_part_turn_and_its_undo_update_in_place() -> Result<()> {
    let mut project = board()?;
    let mut model = build(&project)?;
    for (what, turn) in [("turned", 90.0), ("turned back", -90.0)] {
        for placement in &mut project.placements {
            placement.rotation_deg = (placement.rotation_deg + turn).rem_euclid(360.0);
        }
        let (next, how) = updated(model, &project)?;
        assert!(
            matches!(how, Rebuilt::InPlace(n) if n > 0),
            "{what}: {how:?}"
        );
        model = next;
    }
    Ok(())
}

/// A track edit leaves every pour's body as it was: the fills come before
/// the tracks in the design's history and read none of them, so their
/// extrusions are not evaluated again (the same bodies, not equal ones).
#[test]
fn a_track_edit_does_not_extrude_the_fills_again() -> Result<()> {
    let mut project = Engine::showcase_hosted_fixture(pcbcraft_core::Evaluators {
        footprint: Some(&crate::footprint::SketchEvaluator),
        board: Some(&crate::board::SketchBoardEvaluator),
    })
    .map_err(|e| Error::Verification(e.to_string()))?
    .project()
    .clone();
    let model = build(&project)?;
    let fills = |m: &NativeModel| -> Result<Vec<(String, solidcraft_engine::kernel::Body)>> {
        Ok(m.exact()?
            .session()
            .model
            .state()
            .bodies
            .iter()
            .filter(|b| b.name.starts_with("fill_"))
            .map(|b| (b.name.clone(), b.body.clone()))
            .collect())
    };
    let before = fills(&model)?;
    assert!(!before.is_empty(), "the showcase has poured copper");
    let track = project
        .tracks
        .iter_mut()
        .find(|t| t.layer.side().is_some())
        .ok_or_else(|| Error::Verification("an outer track".into()))?;
    track.width += 0.01;
    let (model, how) = updated(model, &project)?;
    assert!(matches!(how, Rebuilt::InPlace(n) if n > 0), "{how:?}");
    let after = fills(&model)?;
    assert_eq!(after.len(), before.len());
    for ((name, was), (again, now)) in before.iter().zip(&after) {
        assert_eq!(name, again);
        assert!(now.same(was), "{name} was extruded again");
    }
    Ok(())
}

/// A model whose build a newer request cancelled after it finished still
/// updates: the flag stopped that build, not the model it made.
#[test]
fn a_finished_builds_cancel_flag_does_not_stop_its_update() -> Result<()> {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    let mut project = board()?;
    let cancel = Arc::new(AtomicBool::new(false));
    let model = build_cancellable(&project, Arc::clone(&cancel))?;
    cancel.store(true, Ordering::Relaxed);
    if let Some(track) = project.tracks.first_mut() {
        track.width += 0.05;
    }
    let (model, how) = update(model, &project, &SourceCache::default(), None)?;
    assert!(matches!(how, Rebuilt::InPlace(n) if n > 0), "{how:?}");
    same_model(&model, &build(&project)?)
}

/// An update's model saves and reopens like a fresh build's.
#[test]
fn an_updated_model_saves_the_edited_board() -> Result<()> {
    let mut project = board()?;
    let model = build(&project)?;
    if let Some(via) = project.vias.get_mut(3) {
        via.position[0] += 0.3;
    }
    let (model, how) = update(model, &project, &SourceCache::default(), None)?;
    assert!(matches!(how, Rebuilt::InPlace(_)));
    let temporary = tempfile::tempdir()?;
    let path = temporary
        .path()
        .join("updated.solidcraft")
        .to_string_lossy()
        .into_owned();
    model.save(&path)?;
    let reopened = Native::open(&path)?;
    let sources = saved_sources(&reopened)?;
    let again = finish(reopened, sources)?;
    let via = project
        .vias
        .get(3)
        .ok_or_else(|| Error::Verification("no via 3".into()))?;
    let picks: BTreeSet<u64> = again
        .sources
        .iter()
        .filter(|s| s.via == Some(via.id))
        .map(|s| s.pick_id)
        .collect();
    let copper: Vec<&Geometry> = again
        .geometry
        .iter()
        .filter(|g| g.item.is_some_and(|i| picks.contains(&i)))
        .collect();
    assert_eq!(copper.len(), 5);
    let r = via.diameter / 2.0;
    let low = copper.iter().map(|g| g.min[0]).fold(f64::MAX, f64::min);
    assert!((low - (via.position[0] - r)).abs() < 1e-6, "{low}");
    Ok(())
}

/// The saved laminate keeps its outline's relationship to the board size:
/// changing `board_width` and `board_depth` in the saved file resizes it.
#[test]
fn the_saved_laminate_follows_the_board_size_parameters() -> Result<()> {
    let project = board()?;
    let model = build(&project)?;
    let temporary = tempfile::tempdir()?;
    let path = temporary
        .path()
        .join("size.solidcraft")
        .to_string_lossy()
        .into_owned();
    model.save(&path)?;
    let mut reopened = Native::open(&path)?;
    let [w, d] = project.board_size;
    reopened.change_parameters(
        json!({"board_width": format!("{} mm", w + 5.0), "board_depth": format!("{} mm", d + 3.0)}),
    )?;
    let sources = saved_sources(&reopened)?;
    let changed = finish(reopened, sources)?;
    let slabs: Vec<&Geometry> = changed
        .geometry
        .iter()
        .filter(|g| g.kind == BodyKind::Substrate)
        .collect();
    assert_eq!(slabs.len(), 5);
    for slab in slabs {
        assert!((slab.max[0] - (w + 5.0)).abs() < 1e-6, "{:?}", slab.max);
        assert!((slab.max[1] - (d + 3.0)).abs() < 1e-6, "{:?}", slab.max);
        assert!(slab.min[0].abs() < 1e-6 && slab.min[1].abs() < 1e-6);
    }
    Ok(())
}

/// How long an edit takes to reach the model: a fresh build, then updates
/// for a few edits, each the best of a few runs (on a shared machine).
/// `cargo test --profile ci -p pcbcraft-solidcraft update_timings --
/// --ignored --nocapture`; `PCBCRAFT_BENCH_VIAS=0,1000` picks the boards
/// (0 is the showcase).
#[test]
#[ignore = "benchmark"]
fn update_timings() -> Result<()> {
    let boards: Vec<usize> = std::env::var("PCBCRAFT_BENCH_VIAS")
        .ok()
        .map(|v| v.split(',').filter_map(|n| n.trim().parse().ok()).collect())
        .unwrap_or_else(|| vec![0, 1000, 4096, 10_000]);
    type Edit = Box<dyn Fn(&mut Project, f64)>;
    let edits: Vec<(&str, Edit)> = vec![
        (
            "outer track width",
            Box::new(|p: &mut Project, k: f64| {
                if let Some(t) = p.tracks.iter_mut().find(|t| t.layer.side().is_some()) {
                    t.width = 0.2 + 0.01 * k;
                }
            }),
        ),
        (
            "part moved",
            Box::new(|p: &mut Project, k: f64| {
                if let Some(placement) = p.placements.last_mut() {
                    placement.position[0] += 0.1 * k;
                }
            }),
        ),
        (
            "via moved",
            Box::new(|p: &mut Project, k: f64| {
                let n = p.vias.len() / 2;
                if let Some(via) = p.vias.get_mut(n) {
                    via.position[1] += 0.01 * k;
                }
            }),
        ),
        (
            "inner track moved",
            Box::new(|p: &mut Project, k: f64| {
                if let Some(t) = p.tracks.iter_mut().find(|t| t.layer.side().is_none()) {
                    t.end[1] += 0.01 * k;
                }
            }),
        ),
    ];
    for count in boards {
        let mut project = if count == 0 {
            Engine::showcase()
                .map_err(|e| Error::Verification(e.to_string()))?
                .project()
                .clone()
        } else {
            via_board(count)?
        };
        let start = std::time::Instant::now();
        let mut model = build(&project)?;
        let fresh = start.elapsed();
        let name = if count == 0 {
            "showcase".to_owned()
        } else {
            format!("showcase, 4 layers, {count} vias")
        };
        println!(
            "{name}: build {:.0} ms, {} bodies",
            fresh.as_secs_f64() * 1e3,
            model.geometry.len()
        );
        let mut k = 0.0;
        for (what, edit) in &edits {
            let mut best = f64::MAX;
            let mut how = Rebuilt::Anew(None);
            for _ in 0..3 {
                k += 1.0;
                edit(&mut project, k);
                let start = std::time::Instant::now();
                let (next, made) = update(model, &project, &SourceCache::default(), None)?;
                best = best.min(start.elapsed().as_secs_f64() * 1e3);
                how = made;
                model = next;
            }
            println!("  {what}: update {best:.0} ms ({how:?})");
        }
    }
    Ok(())
}
