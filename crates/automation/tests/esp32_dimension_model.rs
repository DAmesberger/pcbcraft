//! The ESP32-S31 dev board's 3D model follows J1 (the USB-C receptacle,
//! held by its shell's front face a part dimension from the bottom edge)
//! when that dimension changes, and when the board widens. Moved up, J1's
//! shell pad opening covers most of a small top pour island the stale
//! pour still has (pours are refilled after the edit); the island's mask
//! cap, the island less that opening, must still be a closed solid.
//!
//! A move of 0.5 or 1 mm changes values only, and the model is updated in
//! place as a fresh build of the edited board would be, though the inner
//! foil's laminate falls into another number of pieces (J1's drills move
//! through it). A move of 2 mm, or the board widened from 45 to 50 mm,
//! carries J1's fan-out with a new track segment (`layout::dimensions::
//! carry`): the new track is made on the design and the model is updated
//! in place too, as a fresh build would be (compared as [`same_board`]
//! compares them: the new track's component takes the design's next id).
use pcbcraft_automation::session::hosted;
use pcbcraft_core::layout::dimensions::DimensionCommand;
use pcbcraft_core::layout::outline::{BoardCommand, BoardEdit};
use pcbcraft_core::library::footprint_source::{
    DimValue, Expr, FootprintParam, ParamName, ParamUnit,
};
use pcbcraft_core::model::dimension::{BoardRef, FeatureRef};
use pcbcraft_core::{Engine, Operation};
use pcbcraft_solidcraft::material::BodyKind;
use pcbcraft_solidcraft::models::imported::SourceCache;
use pcbcraft_solidcraft::project::{self, NativeModel, Rebuilt};
use pcbcraft_solidcraft::scene::Geometry;
use std::path::Path;
use std::time::Instant;

type Fallible<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

fn example() -> Fallible<Engine> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/esp32-s31-devboard/esp32-s31-devboard.pcbcraft");
    Ok(Engine::from_file(pcbcraft_io::read_project(&path)?)?)
}

/// The edits: J1 0.5, 1 and 2 mm from the bottom edge (by its dimension),
/// and the board 50 mm wide.
fn edits(engine: &Engine) -> Fallible<Vec<(String, Operation)>> {
    let j1 = engine
        .project()
        .instance_by_designator("J1")
        .ok_or("J1")?
        .id;
    // The dimension from J1's shell face to the board edge.
    let held = engine
        .project()
        .part_dimensions
        .iter()
        .find(|d| {
            d.refs.iter().any(|r| {
                matches!(r, BoardRef::Part(FeatureRef::Face { placement, .. }) if *placement == j1)
            })
        })
        .cloned()
        .ok_or("no dimension on J1's face")?;
    let mut edits: Vec<(String, Operation)> = [0.5, 1.0, 2.0]
        .into_iter()
        .map(|mm| {
            let mut dimension = held.clone();
            dimension.value = Some(DimValue::Fixed(mm));
            (
                format!("J1 {mm} mm from the edge"),
                Operation::Dimension(DimensionCommand::Edit { dimension }),
            )
        })
        .collect();
    edits.push((
        "the board 50 mm wide".into(),
        Operation::Board(BoardCommand::Edit {
            edits: vec![BoardEdit::SetParameter {
                param: FootprintParam {
                    name: ParamName("width".into()),
                    expr: Expr("50 mm".into()),
                    unit: ParamUnit::Mm,
                    comment: String::new(),
                },
            }],
        }),
    ));
    Ok(edits)
}

/// `a` and `b` alike: every native body's kind, source, layer, box and
/// volume (the laminate's pieces, which an update may number in another
/// order, by where they lie), the display layers, the sources, and what
/// the native design saves of identity and looks.
fn same_model(a: &NativeModel, b: &NativeModel) -> Fallible {
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
    let (p, q) = (&a.native().session().doc, &b.native().session().doc);
    assert_eq!(p.metadata, q.metadata, "saved identity");
    assert_eq!(p.appearances.bodies, q.appearances.bodies, "saved looks");
    Ok(())
}

/// [`same_model`] for an update that made or removed objects: the native
/// bodies as a set, and the sources (in the model and saved) without the
/// native component and occurrence ids, which an added object takes anew.
fn same_board(a: &NativeModel, b: &NativeModel) -> Fallible {
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
    let anonymous = |sources: &[project::Source]| -> Fallible<Vec<serde_json::Value>> {
        let mut out = Vec::new();
        for s in sources {
            let mut v = serde_json::to_value(s)?;
            if let Some(map) = v.as_object_mut() {
                map.remove("component");
                map.remove("occurrence");
            }
            out.push(v);
        }
        Ok(out)
    };
    assert_eq!(anonymous(&a.sources)?, anonymous(&b.sources)?, "sources");
    assert_eq!(
        anonymous(&project::saved_sources(a.native())?)?,
        anonymous(&project::saved_sources(b.native())?)?,
        "saved sources"
    );
    let (p, q) = (&a.native().session().doc, &b.native().session().doc);
    assert_eq!(p.appearances.bodies, q.appearances.bodies, "saved looks");
    Ok(())
}

#[test]
fn the_model_follows_j1_moved_by_its_dimension_or_the_board_width() -> Fallible {
    let mut engine = example()?;
    let sources = SourceCache::default();
    let mut model = Some(project::build_with(engine.project(), &sources, None)?);
    let j1 = engine
        .project()
        .instance_by_designator("J1")
        .ok_or("J1")?
        .id;
    let y = |engine: &Engine| engine.project().placement_of(j1).map(|p| p.position[1]);
    let at = y(&engine).ok_or("J1 unplaced")?;
    for (k, (what, operation)) in edits(&engine)?.into_iter().enumerate() {
        let revision = engine.revision();
        engine.apply_hosted(revision, operation, hosted())?;
        let previous = model.take().ok_or("no model")?;
        let (next, how) = project::update(previous, engine.project(), &sources, None)
            .map_err(|e| format!("{what}: {e}"))?;
        println!("{what}: J1 y {at} -> {:?}, {how:?}", y(&engine));
        match k {
            // Values only: in place, as a fresh build. The first starts from
            // a fresh build, the second from the first's update.
            0 | 1 => {
                assert!(
                    matches!(how, Rebuilt::InPlace(n) if n > 0),
                    "{what}: {how:?}"
                );
                same_model(
                    &next,
                    &project::build_with(engine.project(), &sources, None)?,
                )?;
            }
            // A carried track segment more (and, for the board, from the
            // 2 mm board, which has a carried segment too): the segment is
            // made on the design, in place, as a fresh build would be.
            _ => {
                assert!(
                    matches!(how, Rebuilt::InPlace(n) if n > 0),
                    "{what}: {how:?}"
                );
                same_board(
                    &next,
                    &project::build_with(engine.project(), &sources, None)?,
                )?;
            }
        }
        // The next edit starts from the example again; its update starts
        // from this model.
        engine.undo()?;
        model = Some(next);
    }
    Ok(())
}

/// Each edit's update from a fresh build of the example, best of three
/// (`cargo test --profile ci -p pcbcraft-automation --test
/// esp32_dimension_model -- --ignored --nocapture`).
#[test]
#[ignore = "timing; run by hand"]
fn esp32_edit_update_timings() -> Fallible {
    let mut engine = example()?;
    let sources = SourceCache::default();
    let started = Instant::now();
    let base = project::build_with(engine.project(), &sources, None)?;
    println!("build: {:.1} s", started.elapsed().as_secs_f64());
    for (what, operation) in edits(&engine)? {
        let revision = engine.revision();
        engine.apply_hosted(revision, operation, hosted())?;
        let mut best = f64::INFINITY;
        let mut how = None;
        for _ in 0..3 {
            let previous = base.snapshot();
            let started = Instant::now();
            let (_, rebuilt) = project::update(previous, engine.project(), &sources, None)?;
            best = best.min(started.elapsed().as_secs_f64());
            how = Some(rebuilt);
        }
        println!("{what}: best {best:.2} s, {how:?}");
        engine.undo()?;
    }
    Ok(())
}
