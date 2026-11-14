//! [`update`]: the live view's meshed model is made again from what the
//! last one kept; the exact model is built for export and as the test
//! reference, never updated in place.
use super::tests::via_board;
use super::*;

/// A small 4-layer board: the showcase with vias and inner tracks.
fn board() -> Result<Project> {
    via_board(40)
}

/// A body's kind, pick and box.
type BodyBox = (BodyKind, Option<u64>, [f64; 3], [f64; 3]);

/// Each body's kind, pick and box, in model order.
fn bodies(model: &NativeModel) -> Vec<BodyBox> {
    model
        .geometry
        .iter()
        .map(|g| (g.kind, g.item, g.min, g.max))
        .collect()
}

/// The exact model has no in-place update: an edit's exact model is a
/// fresh build of the edited board, whatever changed.
#[test]
fn an_exact_model_is_built_anew_never_updated() -> Result<()> {
    let _context = crate::looks::test_context().map(|c| c.enter());
    let mut project = board()?;
    let model = build(&project)?;
    if let Some(track) = project.tracks.first_mut() {
        track.width += 0.05;
    }
    let (model, how) = update(model, &project, &SourceCache::default(), None)?;
    assert!(matches!(how, Rebuilt::Anew(Some(_))), "{how:?}");
    assert_eq!(model.flat(), Flat::Native);
    let fresh = build(&project)?;
    let (made, expected) = (bodies(&model), bodies(&fresh));
    assert_eq!(made.len(), expected.len());
    for (a, b) in made.iter().zip(&expected) {
        assert_eq!((a.0, a.1), (b.0, b.1));
        let off = (0..3)
            .map(|i| (a.2[i] - b.2[i]).abs().max((a.3[i] - b.3[i]).abs()))
            .fold(0.0, f64::max);
        assert!(off < 1e-9, "{a:?} against {b:?}");
    }
    Ok(())
}

/// A meshed model is made again from what it kept, with no native design
/// of the board, and keeps its way of holding the flat bodies.
#[test]
fn a_meshed_model_is_updated_from_what_it_kept() -> Result<()> {
    let _context = crate::looks::test_context().map(|c| c.enter());
    let mut project = board()?;
    let sources = SourceCache::default();
    let live = build_flat(&project, &sources, None, Flat::Meshed)?;
    if let Some(via) = project.vias.get_mut(3) {
        via.position[0] += 0.3;
    }
    let (model, how) = update(live, &project, &sources, None)?;
    assert_eq!(how, Rebuilt::InPlace);
    assert_eq!(model.flat(), Flat::Meshed);
    assert!(model.native().is_none() && model.world().is_none());
    Ok(())
}

/// The saved laminate keeps its outline's relationship to the board size:
/// changing `board_width` and `board_depth` in the saved file resizes it.
#[test]
fn the_saved_laminate_follows_the_board_size_parameters() -> Result<()> {
    let _context = crate::looks::test_context().map(|c| c.enter());
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
