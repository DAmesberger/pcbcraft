use super::*;

fn near(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() < 1e-5 + expected.abs() * 1e-6,
        "{actual} != {expected}"
    );
}

#[test]
fn copper_has_physical_thickness_empty_drill_and_expected_volumes() -> Result<()> {
    let b = Board::fixture()?;
    for id in [1, 2, 3, 5, 6] {
        let g = b
            .geometry
            .iter()
            .find(|g| g.item == Some(id))
            .ok_or_else(|| Error::Verification("missing item".into()))?;
        near(g.min[2], 1.6);
        near(g.max[2], 1.635);
    }
    let volume = |id| {
        b.geometry
            .iter()
            .filter(|g| g.item == Some(id))
            .map(|g| g.volume())
            .sum::<f64>()
    };
    near(volume(1), 10.0 * 0.5 * 0.035);
    // Upstream mass() integrates a tessellation: circular boundaries have chord
    // error, unlike box volumes. Bound lost area by perimeter × chord tolerance,
    // then multiply by extrusion height. Native radius/empty-drill checks below
    // are independent of this numerical measurement budget.
    let zone_expected = (100.0 - std::f64::consts::PI * 4.0) * 0.035;
    let zone_bound = 2.0 * std::f64::consts::PI * 2.0 * 0.0006 * 0.035;
    assert!((volume(6) - zone_expected).abs() < zone_bound);
    let via_expected = 2.0 * (2.56 - std::f64::consts::PI * 0.09) * 0.035
        + std::f64::consts::PI * (0.25 - 0.09) * 1.6;
    let via_bound = 2.0 * std::f64::consts::PI * 0.0001 * (2.0 * 0.3 * 0.035 + 0.8 * 1.6);
    assert!((volume(4) - via_expected).abs() < via_bound);
    assert_eq!(b.pick(Vec3::new(18.25, 20.0, 10.0), -Vec3::Z, false), None);
    assert_eq!(b.pick(Vec3::new(13.0, 8.0, 10.0), -Vec3::Z, false), Some(1));
    assert_eq!(b.pick(Vec3::new(25.0, 20.0, 10.0), -Vec3::Z, false), None);
    assert_eq!(b.pick(Vec3::new(25.0, 20.0, 10.0), -Vec3::Z, true), Some(7));
    Ok(())
}

#[test]
fn shared_selection_and_net_highlight_reuse_the_same_meshes() -> Result<()> {
    let mut b = Board::fixture()?;
    let meshes = b
        .geometry
        .iter()
        .map(|g| g.mesh.clone())
        .collect::<Vec<_>>();
    b.execute(Command::Select { item: Some(1) })?;
    let s = b.execute(Command::HighlightNet {
        net: Some("SIGNAL".into()),
    })?;
    assert!(s.highlighted_items.contains(&5));
    assert!(!s.highlighted_items.contains(&6));
    b.execute(Command::View {
        mode: ViewMode::Solid,
    })?;
    b.execute(Command::View {
        mode: ViewMode::Layout,
    })?;
    assert_eq!(b.selected, Some(1));
    assert_eq!(b.scene_builds, 1);
    assert_eq!(b.revision, 1);
    assert!(
        b.geometry
            .iter()
            .zip(meshes)
            .all(|(g, m)| Arc::ptr_eq(&g.mesh, &m))
    );
    Ok(())
}

#[test]
fn width_undo_and_rejection_are_atomic() -> Result<()> {
    let mut b = Board::fixture()?;
    let original = b.snapshot()?;
    assert!(
        b.execute(Command::Width {
            expected_revision: 0,
            width_mm: 0.8
        })
        .is_err()
    );
    assert!(
        b.execute(Command::Width {
            expected_revision: 1,
            width_mm: f64::NAN
        })
        .is_err()
    );
    assert_eq!(b.revision, original.revision);
    b.execute(Command::Width {
        expected_revision: 1,
        width_mm: 0.8,
    })?;
    near(
        b.geometry
            .iter()
            .filter(|g| g.item == Some(1))
            .map(|g| g.volume())
            .sum(),
        10.0 * 0.8 * 0.035,
    );
    b.execute(Command::Undo {})?;
    near(b.snapshot()?.trace_width_mm, 0.5);
    b.execute(Command::Redo {})?;
    near(b.snapshot()?.trace_width_mm, 0.8);
    Ok(())
}

#[test]
fn native_reopen_retains_identity_and_regenerates_copper() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir
        .path()
        .join("copper.solidcraft")
        .to_string_lossy()
        .into_owned();
    let mut b = Board::fixture()?;
    b.execute(Command::Save { path: path.clone() })?;
    let mut s = Session::default();
    call(&mut s, "doc.open", json!({"path":path,"read_only":true}))?;
    call(
        &mut s,
        "parameters.change",
        json!({"changes":{"trace_width":"0.8 mm","copper_thickness":"0.07 mm"}}),
    )?;
    let (items, geometry) = evaluate(&s)?;
    assert_eq!(items, b.items);
    let trace = geometry
        .iter()
        .find(|g| g.item == Some(1))
        .ok_or_else(|| Error::Verification("missing trace".into()))?;
    near(trace.volume(), 10.0 * 0.8 * 0.07);
    near(trace.max[2], 1.67);
    Ok(())
}
