use super::*;
use crate::material::BodyKind;
use crate::models::imported::SourceCache;
use crate::project::plan::{self, Level};
use crate::project::tests::{shaped, showcase};
use crate::project::{Flat, build, build_flat};
use pcbcraft_core::Project;

fn meshed(project: &Project) -> Result<(BoardPlan, Vec<Meshed>, Stats)> {
    let plan = plan::of(project)?;
    let (bodies, stats) = bodies(&plan, &plan_sources(&plan), &mut Memo::default())?;
    Ok((plan, bodies, stats))
}

/// Every flat body of the shaped fixture (every kind of land, both sides,
/// turned) and of the showcase (tracks, vias, fills on outer and inner
/// layers, foil levels) is meshed as planned: one mesh per plan body,
/// of its kind, source and layer, its box and volume the plan body's
/// within what flattening explains, and the laminate whole.
#[test]
fn meshed_bodies_are_the_plans() -> Result<()> {
    for project in [shaped()?, showcase()?] {
        let (plan, bodies, stats) = meshed(&project)?;
        let differences = parity(&plan, &bodies)?;
        assert!(differences.is_empty(), "{differences:#?}");
        let slabs = plan.laminate.len();
        assert_eq!(stats.bodies, plan.bodies().count() - slabs + 1);
        assert_eq!(bodies.len(), stats.bodies);
    }
    Ok(())
}

/// The parity check sees a mesh dropped, one added and one reshaped.
#[test]
fn the_parity_check_finds_a_dropped_added_or_reshaped_mesh() -> Result<()> {
    let (plan, bodies, _) = meshed(&showcase()?)?;
    let track = bodies
        .iter()
        .position(|m| matches!(m.source, Some(Origin::Track(_))))
        .ok_or_else(|| Error::Verification("no track".into()))?;
    let mut dropped = bodies.clone();
    dropped.remove(track);
    let differences = parity(&plan, &dropped)?;
    assert_eq!(differences.len(), 1, "{differences:#?}");
    assert!(differences[0].starts_with("planned, not meshed"));
    let mut added = bodies.clone();
    added.push(bodies[track].clone());
    let differences = parity(&plan, &added)?;
    assert_eq!(differences.len(), 1, "{differences:#?}");
    assert!(differences[0].starts_with("meshed twice"));
    // A track meshed 0.1 mm wider than planned.
    let project = showcase()?;
    let track = &project.tracks[0];
    let length = (track.end[0] - track.start[0]).hypot(track.end[1] - track.start[1]);
    let mut wider = plan.clone();
    wider.tracks[0].body.loops = vec![
        crate::project::polygon(&crate::project::track_outline(
            [0.0, 0.0],
            [length, 0.0],
            track.width + 0.1,
        ))
        .ok_or_else(|| Error::Verification("no track".into()))?,
    ];
    let (reshaped, _) = super::bodies(&wider, &plan_sources(&plan), &mut Memo::default())?;
    let differences = parity(&plan, &reshaped)?;
    assert_eq!(differences.len(), 2, "box and volume: {differences:#?}");
    assert!(
        differences
            .iter()
            .all(|d| d.starts_with(&wider.tracks[0].body.name))
    );
    Ok(())
}

/// The laminate is one body from the bottom face to the top: the board
/// outline less every drill, as every slab but a foil level holds it.
#[test]
fn the_laminate_is_one_body_through_the_board() -> Result<()> {
    let project = showcase()?;
    let (plan, bodies, _) = meshed(&project)?;
    assert!(
        plan.laminate
            .iter()
            .any(|s| matches!(s.source, Origin::Laminate(Level::Foil { .. }))),
        "the showcase has foil levels"
    );
    let laminate: Vec<&Meshed> = bodies
        .iter()
        .filter(|m| m.geometry.kind == BodyKind::Substrate)
        .collect();
    assert_eq!(laminate.len(), 1);
    let g = &laminate[0].geometry;
    assert!(laminate[0].source.is_none() && g.item.is_none());
    assert!(g.min[2].abs() < 1e-9);
    assert!((g.max[2] - project.stackup.dielectric_mm()).abs() < 1e-9);
    // Drilled through: a via's drill is open from top to bottom.
    let via = &project.vias[0];
    let hit = g.raycast(
        Vec3::new(via.position[0], via.position[1], 10.0),
        Vec3::new(0.0, 0.0, -1.0),
    );
    assert!(hit.is_none(), "the laminate is drilled at a via");
    Ok(())
}

/// Every via of a size, and every placement of a pad, shares one mesh,
/// placed where it lies.
#[test]
fn bodies_alike_share_one_mesh() -> Result<()> {
    let project = showcase()?;
    let (_, bodies, stats) = meshed(&project)?;
    let barrels: Vec<&Meshed> = bodies
        .iter()
        .filter(|m| {
            matches!(
                m.source,
                Some(Origin::Via {
                    part: plan::ViaPart::Barrel,
                    ..
                })
            )
        })
        .collect();
    assert!(barrels.len() > 1);
    let sizes: std::collections::HashSet<(u64, u64)> = project
        .vias
        .iter()
        .map(|v| (v.drill.to_bits(), v.diameter.to_bits()))
        .collect();
    let meshes: std::collections::HashSet<usize> = barrels
        .iter()
        .map(|m| Arc::as_ptr(&m.geometry.mesh) as usize)
        .collect();
    assert_eq!(meshes.len(), sizes.len());
    assert!(stats.meshed < stats.bodies);
    Ok(())
}

/// An edit meshes only what it changed: a track made wider is meshed
/// again, every other body is taken from the memo, and meshing the same
/// board again makes nothing.
#[test]
fn an_edit_meshes_only_what_it_changed() -> Result<()> {
    let mut project = showcase()?;
    let plan = plan::of(&project)?;
    let mut memo = Memo::default();
    let (_, cold) = bodies(&plan, &plan_sources(&plan), &mut memo)?;
    assert_eq!(cold.reused, 0);
    let (_, again) = bodies(&plan, &plan_sources(&plan), &mut memo)?;
    assert_eq!(again.meshed, 0);
    assert_eq!(again.reused, cold.meshed);
    project.tracks[0].width += 0.05;
    let wider = plan::of(&project)?;
    let (bodies, edited) = bodies(&wider, &plan_sources(&wider), &mut memo)?;
    assert_eq!(edited.meshed, 1, "{edited:?}");
    assert!(edited.reused >= cold.meshed - 1, "{edited:?}");
    assert!(parity(&wider, &bodies)?.is_empty());
    // The memo keeps only the meshes the last board used.
    assert_eq!(memo.len(), cold.meshed);
    Ok(())
}

/// A meshed model holds the parts natively and the flat bodies as meshes:
/// each flat body picked as its object's source (the laminate as the
/// board), the same objects, kinds and layers as the exact model's
/// flat bodies, and the same display layers. A ray onto a track meets
/// that track's mesh.
#[test]
fn a_meshed_model_picks_every_flat_body_as_its_object() -> Result<()> {
    let project = shaped()?;
    let exact = build(&project)?;
    let live = build_flat(&project, &SourceCache::default(), None, Flat::Meshed)?;
    assert_eq!(live.flat(), Flat::Meshed);
    let flat = |kind: BodyKind| {
        matches!(
            kind,
            BodyKind::Substrate
                | BodyKind::PadCopper
                | BodyKind::CopperTrack
                | BodyKind::ViaCopper
                | BodyKind::FillCopper
        )
    };
    let label = |model: &crate::project::NativeModel, g: &Geometry| -> String {
        g.item
            .and_then(|pick| model.sources.iter().find(|s| s.pick_id == pick))
            .map_or_else(|| "board".to_owned(), |s| s.label.clone())
    };
    let objects = |model: &crate::project::NativeModel| {
        let mut out: Vec<(String, String)> = model
            .geometry
            .iter()
            .filter(|g| flat(g.kind))
            .map(|g| (format!("{:?}", g.kind), label(model, g)))
            .collect();
        out.sort();
        out.dedup();
        out
    };
    assert_eq!(objects(&live), objects(&exact));
    // No native flat body in the live model's design.
    let world = live.native().world_state()?;
    assert!(
        live.geometry[..world.bodies.len()]
            .iter()
            .all(|g| !flat(g.kind)),
        "the meshed model's design holds parts only"
    );
    // The same display layers.
    let display = |model: &crate::project::NativeModel| {
        let mut kinds: Vec<String> = model
            .geometry
            .iter()
            .filter(|g| !flat(g.kind) && !g.kind.is_envelope() && !g.kind.is_generated())
            .map(|g| format!("{:?} {}", g.kind, label(model, g)))
            .collect();
        kinds.sort();
        kinds
    };
    assert_eq!(display(&live), display(&exact));
    // A ray down onto the middle of each track meets that track first among
    // the flat bodies.
    for track in &project.tracks {
        let mid = [
            (track.start[0] + track.end[0]) / 2.0,
            (track.start[1] + track.end[1]) / 2.0,
        ];
        let (z, down) = if track.layer == pcbcraft_core::CopperLayer::Bottom {
            (-10.0, 1.0)
        } else {
            (10.0, -1.0)
        };
        let first = live
            .geometry
            .iter()
            .filter(|g| flat(g.kind))
            .filter_map(|g| {
                g.raycast(Vec3::new(mid[0], mid[1], z), Vec3::new(0.0, 0.0, down))
                    .map(|(t, _)| (t, g))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .ok_or_else(|| Error::Verification("the ray met nothing".into()))?;
        let source = first
            .1
            .item
            .and_then(|pick| live.sources.iter().find(|s| s.pick_id == pick));
        assert_eq!(source.and_then(|s| s.track), Some(track.id));
    }
    Ok(())
}

/// A meshed model's design lacks the board's flat bodies: it is drawn,
/// never saved.
#[test]
fn a_meshed_model_is_never_saved() -> Result<()> {
    let live = build_flat(&shaped()?, &SourceCache::default(), None, Flat::Meshed)?;
    let dir = tempfile::tempdir().map_err(|e| Error::Verification(e.to_string()))?;
    let path = dir.path().join("live.solidcraft");
    let refused = live.save(&path.to_string_lossy());
    assert!(matches!(refused, Err(Error::Invalid(_))), "{refused:?}");
    assert!(!path.exists());
    Ok(())
}

/// A meshed model updated for a track edit keeps its way and meshes only
/// that track again; the native design runs no command.
#[test]
fn an_update_of_a_meshed_model_stays_meshed() -> Result<()> {
    let mut project = showcase()?;
    let sources = SourceCache::default();
    let live = build_flat(&project, &sources, None, Flat::Meshed)?;
    let held = live.meshes.len();
    project.tracks[0].width += 0.05;
    let (updated, how) = crate::project::update(live, &project, &sources, None)?;
    assert_eq!(updated.flat(), Flat::Meshed);
    assert_eq!(how, crate::project::Rebuilt::InPlace(0));
    assert_eq!(updated.meshes.len(), held);
    let plan = plan::of(&project)?;
    let track = updated
        .geometry
        .iter()
        .find(|g| {
            g.kind == BodyKind::CopperTrack
                && g.item
                    .and_then(|p| updated.sources.iter().find(|s| s.pick_id == p))
                    .and_then(|s| s.track)
                    == Some(project.tracks[0].id)
        })
        .ok_or_else(|| Error::Verification("the track is not drawn".into()))?;
    let (min, max) = plan.tracks[0].body.bounds()?;
    for k in 0..3 {
        assert!((track.min[k] - min[k]).abs() <= PARITY_MM);
        assert!((track.max[k] - max[k]).abs() <= PARITY_MM);
    }
    Ok(())
}

/// A plan made with the fills' memo is the plan made anew: after a track
/// edit and its refill, the refilled fills are planned again and the
/// others taken as they were.
#[test]
fn a_plan_from_the_memo_is_the_plan_made_anew() -> Result<()> {
    use pcbcraft_core::layout::fill::{RefillTarget, refill};
    let mut project = showcase()?;
    let mut memo = plan::Memo::default();
    let pads = crate::project::placed_pads(&project)?;
    let first = plan::of_pads(&project, &pads, &mut memo)?;
    assert_eq!(format!("{first:?}"), format!("{:?}", plan::of(&project)?));
    for track in &mut project.tracks {
        track.width += 0.05;
    }
    let refilled =
        refill(&project, RefillTarget::All).map_err(|e| Error::Verification(e.to_string()))?;
    assert!(!refilled.is_empty());
    for (id, copper) in refilled {
        if let Some(fill) = project.fills.iter_mut().find(|f| f.id == id) {
            fill.computed = Some(copper);
        }
    }
    let pads = crate::project::placed_pads(&project)?;
    let again = plan::of_pads(&project, &pads, &mut memo)?;
    assert_eq!(format!("{again:?}"), format!("{:?}", plan::of(&project)?));
    assert_ne!(format!("{again:?}"), format!("{first:?}"));
    Ok(())
}
