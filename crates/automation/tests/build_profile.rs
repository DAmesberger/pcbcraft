//! Where the board's 3D build spends its time (docs/3d-speed-profile.md):
//! the ESP32-S31 example's native build, each Solidcraft feature kind's
//! evaluation time and Solidcraft's own span profile, then one part turned
//! by 90° and turned back (the edit and its undo) through `project::update`.
//!
//! `nix develop -c cargo test --profile ci -p pcbcraft-automation --test build_profile -- --ignored --nocapture`
//!
//! - `PCBCRAFT_PROFILE_BOARD`: another `.pcbcraft` file.
//! - `PCBCRAFT_PROFILE_VARIANT`: `full` (default), `no-vias`, `no-fills`,
//!   `no-tracks`, `parts=N` (the first N placements), `vias=N` (the first N vias),
//!   `addvias=N` (no fills, N more vias on a grid).
//! - `PCBCRAFT_PROFILE_NOFILLS=1`: leave the fills out (after the variant).
//! - `PCBCRAFT_PROFILE_EDIT=1`: also time the part turn and its undo (`track`: an
//!   outer track 10 µm wider and back instead);
//!   `PCBCRAFT_PROFILE_REFILL=1` then refills the stale fills and updates again.
//! - `PCBCRAFT_PROFILE_PART`: the part turned (default SW1).
//! - `PCBCRAFT_PROFILE_TRACE`: write Solidcraft's Chrome trace there.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use pcbcraft_core::Project;
use pcbcraft_solidcraft::models::imported::SourceCache;
use pcbcraft_solidcraft::project::{self, NativeModel};
use std::collections::BTreeMap;
use std::time::Instant;

fn board() -> Project {
    let path = std::env::var("PCBCRAFT_PROFILE_BOARD").map_or_else(
        |_| {
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../examples/esp32-s31-devboard/esp32-s31-devboard.pcbcraft")
        },
        std::path::PathBuf::from,
    );
    let mut project = pcbcraft_io::read_project(&path)
        .unwrap()
        .into_document()
        .project
        .as_ref()
        .clone();
    let variant = std::env::var("PCBCRAFT_PROFILE_VARIANT").unwrap_or_else(|_| "full".into());
    match variant.as_str() {
        "full" => {}
        "no-vias" => project.vias.clear(),
        "no-fills" => project.fills.clear(),
        "no-tracks" => project.tracks.clear(),
        v if v.starts_with("parts=") => {
            let n: usize = v["parts=".len()..].parse().unwrap();
            project.placements.truncate(n);
        }
        v if v.starts_with("vias=") => {
            let n: usize = v["vias=".len()..].parse().unwrap();
            project.vias.truncate(n);
        }
        // The board without its fills, with N more through vias on a
        // 1.27 mm grid (where they land does not matter to the build).
        v if v.starts_with("addvias=") => {
            let n: usize = v["addvias=".len()..].parse().unwrap();
            project.fills.clear();
            let net = project.nets[0].id;
            let columns = ((project.board_size[0] - 4.0) / 1.27).floor().max(1.0) as usize;
            for k in 0..n {
                let id = project.allocate().unwrap();
                project.vias.push(pcbcraft_core::Via {
                    id,
                    net,
                    position: [
                        2.0 + (k % columns) as f64 * 1.27,
                        2.0 + (k / columns) as f64 * 1.27,
                    ],
                    drill: 0.3,
                    diameter: 0.6,
                    span: pcbcraft_core::ViaSpan::Through,
                    tented: true,
                    locked: false,
                });
            }
        }
        v => panic!("unknown variant {v}"),
    }
    if std::env::var("PCBCRAFT_PROFILE_NOFILLS").is_ok() {
        project.fills.clear();
    }
    eprintln!(
        "board {:?} variant {variant}: {} placements, {} tracks, {} vias, {} fills",
        project.name,
        project.placements.len(),
        project.tracks.len(),
        project.vias.len(),
        project.fills.len()
    );
    project
}

/// Each Solidcraft feature kind's count and evaluation time in the model's
/// last evaluation (features reused from an earlier one count as reused).
fn features(model: &NativeModel) {
    let session = model.native().session();
    let kinds: BTreeMap<u64, &str> = session
        .doc
        .features
        .iter()
        .map(|f| (f.id, f.kind.type_name()))
        .collect();
    let mut rows: BTreeMap<&str, (usize, usize, f64, f64)> = BTreeMap::new();
    for r in &session.model.results {
        let kind = kinds.get(&r.id).copied().unwrap_or("?");
        let row = rows.entry(kind).or_default();
        row.0 += 1;
        if r.ms > 0.0 {
            row.1 += 1;
        }
        row.2 += r.ms;
        row.3 = row.3.max(r.ms);
    }
    let total: f64 = rows.values().map(|r| r.2).sum();
    eprintln!(
        "{} features, {} bodies in the world, evaluation {:.0} ms in all",
        session.doc.features.len(),
        model.native().world_state().unwrap().bodies.len(),
        total
    );
    let mut rows: Vec<_> = rows.into_iter().collect();
    rows.sort_by(|a, b| b.1.2.total_cmp(&a.1.2));
    eprintln!("| Feature kind | Features | Timed | Total ms | Longest ms |");
    eprintln!("| --- | --- | --- | --- | --- |");
    for (kind, (n, timed, ms, max)) in rows {
        eprintln!("| {kind} | {n} | {timed} | {ms:.0} | {max:.0} |");
    }
    // The ten slowest single features, by name.
    let mut slow: Vec<_> = session.model.results.iter().collect();
    slow.sort_by(|a, b| b.ms.total_cmp(&a.ms));
    let world = model.native().world_state().unwrap();
    for r in slow.iter().take(12) {
        // The faces and edges of the bodies the feature made.
        let (faces, edges) = world
            .bodies
            .iter()
            .filter(|b| b.feature == r.id)
            .fold((0, 0), |(f, e), b| {
                (f + b.body.face_count(), e + b.body.edge_count())
            });
        eprintln!(
            "  slow: {} ({}) {:.0} ms, {faces} faces, {edges} edges",
            r.name,
            kinds.get(&r.id).copied().unwrap_or("?"),
            r.ms
        );
    }
}

fn spans(title: &str) {
    let report = pcbcraft_solidcraft::profile::report();
    eprintln!("--- Solidcraft spans: {title} ---");
    for (category, total) in report.by_category() {
        eprintln!("  category {category}: {:.0} ms", total.as_secs_f64() * 1e3);
    }
    eprintln!("{}", report.table(40));
    if let Ok(path) = std::env::var("PCBCRAFT_PROFILE_TRACE") {
        let mut file = std::fs::File::create(format!("{path}.{title}.json")).unwrap();
        pcbcraft_solidcraft::profile::write_chrome_trace(&mut file).unwrap();
    }
    pcbcraft_solidcraft::profile::reset();
}

#[test]
#[ignore = "benchmark"]
fn board_build_profile() {
    let mut project = board();
    let sources = SourceCache::default();
    pcbcraft_solidcraft::profile::set_enabled(true);
    pcbcraft_solidcraft::profile::reset();
    let start = Instant::now();
    let model = project::build_with(&project, &sources, None).unwrap();
    let built = start.elapsed();
    let kinds = model
        .geometry
        .iter()
        .fold(BTreeMap::<String, usize>::new(), |mut m, g| {
            *m.entry(format!("{:?}", g.kind)).or_default() += 1;
            m
        });
    eprintln!(
        "build {:.1} s, {} drawn bodies, {} sources",
        built.as_secs_f64(),
        model.geometry.len(),
        model.sources.len()
    );
    eprintln!("drawn bodies by kind: {kinds:?}");
    features(&model);
    spans("build");
    let Ok(edit) = std::env::var("PCBCRAFT_PROFILE_EDIT") else {
        return;
    };
    // An outer track 10 µm wider, then as it was (an edit that changes a
    // value only: no fill is stale, nothing is drawn anew).
    if edit == "track" {
        let mut model = model;
        let track = project
            .tracks
            .iter()
            .position(|t| t.layer.side().is_some())
            .unwrap();
        for (what, by) in [("track wider", 0.01), ("undo", -0.01)] {
            project.tracks[track].width += by;
            let start = Instant::now();
            let (next, how) = project::update(model, &project, &sources, None).unwrap();
            eprintln!(
                "{what}: update {:.1} s ({how:?})",
                start.elapsed().as_secs_f64()
            );
            spans(what);
            model = next;
        }
        return;
    }
    let part = std::env::var("PCBCRAFT_PROFILE_PART").unwrap_or_else(|_| "SW1".into());
    let instance = project.instance_by_designator(&part).unwrap().id;
    let mut model = model;
    for (what, turn) in [("turn 90", 90.0), ("undo", -90.0)] {
        let placement = project
            .placements
            .iter_mut()
            .find(|p| p.instance == instance)
            .unwrap();
        placement.rotation_deg = (placement.rotation_deg + turn).rem_euclid(360.0);
        let start = Instant::now();
        let (next, how) = project::update(model, &project, &sources, None).unwrap();
        eprintln!(
            "{what} {part}: update {:.1} s ({how:?})",
            start.elapsed().as_secs_f64()
        );
        features(&next);
        spans(what);
        model = next;
        // The app's fill worker then recomputes the fills the edit made
        // stale, and the model is updated again with their new copper.
        if std::env::var("PCBCRAFT_PROFILE_REFILL").is_err() {
            continue;
        }
        let start = Instant::now();
        let mut cache = pcbcraft_core::layout::fill::FillCache::default();
        let fresh = pcbcraft_core::layout::fill::compute_stale(&project, &mut cache).unwrap();
        eprintln!(
            "{what}: {} stale fills recomputed in {:.1} s",
            fresh.len(),
            start.elapsed().as_secs_f64()
        );
        for (id, copper) in fresh {
            let fill = project.fills.iter_mut().find(|f| f.id == id).unwrap();
            let same = fill
                .computed
                .as_ref()
                .is_some_and(|old| old.regions == copper.regions);
            eprintln!("  fill {id}: regions unchanged {same}");
            fill.computed = Some(copper);
        }
        let start = Instant::now();
        let (next, how) = project::update(model, &project, &sources, None).unwrap();
        eprintln!(
            "{what} {part}, refilled: update {:.1} s ({how:?})",
            start.elapsed().as_secs_f64()
        );
        features(&next);
        spans(&format!("{what}, refilled"));
        model = next;
    }
}

/// One extruded copper region of `edges` straight edges: a 40 mm polygon
/// (`PCBCRAFT_SCALING_SHAPE=outline`, default) or a 60 mm square with
/// square holes of four edges each (`holes`), extruded 35 µm by
/// `Native::batched_body` as a fill's region is. Prints the time of each,
/// to show how one extrusion grows with its edges.
/// `PCBCRAFT_SCALING_EDGES=250,500,1000,2000` picks the sizes.
#[test]
#[ignore = "benchmark"]
fn extrusion_scaling() {
    use pcbcraft_core::library::{Outline, Segment};
    use pcbcraft_solidcraft::native::{Native, ProfileBody};
    use pcbcraft_solidcraft::pad_outline::Contour;
    let sizes: Vec<usize> = std::env::var("PCBCRAFT_SCALING_EDGES")
        .ok()
        .map(|v| v.split(',').filter_map(|n| n.trim().parse().ok()).collect())
        .unwrap_or_else(|| vec![250, 500, 1000, 2000]);
    let shape = std::env::var("PCBCRAFT_SCALING_SHAPE").unwrap_or_else(|_| "outline".into());
    let polygon = |points: &[[f64; 2]]| {
        let (first, rest) = points.split_first().unwrap();
        let mut segments: Vec<Segment> = rest.iter().map(|&to| Segment::Line { to }).collect();
        segments.push(Segment::Line { to: *first });
        Contour::Outline(Outline {
            start: *first,
            segments,
        })
    };
    pcbcraft_solidcraft::profile::set_enabled(true);
    for edges in sizes {
        let contours: Vec<Contour> = match shape.as_str() {
            "outline" => {
                // A star of `edges` points, so no edges are collinear.
                let points: Vec<[f64; 2]> = (0..edges)
                    .map(|k| {
                        let a = std::f64::consts::TAU * k as f64 / edges as f64;
                        let r = if k % 2 == 0 { 20.0 } else { 19.9 };
                        [30.0 + r * a.cos(), 30.0 + r * a.sin()]
                    })
                    .collect();
                vec![polygon(&points)]
            }
            "holes" => {
                let holes = edges.saturating_sub(4) / 4;
                let side = (holes as f64).sqrt().ceil().max(1.0) as usize;
                let pitch = 56.0 / side as f64;
                let mut contours = vec![polygon(&[
                    [0.0, 0.0],
                    [60.0, 0.0],
                    [60.0, 60.0],
                    [0.0, 60.0],
                ])];
                for k in 0..holes {
                    let (x, y) = (
                        2.0 + (k % side) as f64 * pitch,
                        2.0 + (k / side) as f64 * pitch,
                    );
                    let s = pitch * 0.4;
                    contours.push(polygon(&[[x, y], [x + s, y], [x + s, y + s], [x, y + s]]));
                }
                contours
            }
            s => panic!("unknown shape {s}"),
        };
        let mut native = Native::new("scaling").unwrap();
        pcbcraft_solidcraft::profile::reset();
        let start = Instant::now();
        native
            .batched_body(ProfileBody {
                name: "region",
                offset: 0.into(),
                contours: &contours,
                height: 0.035.into(),
            })
            .unwrap();
        let took = start.elapsed();
        let report = pcbcraft_solidcraft::profile::report();
        let extrude = report
            .rows
            .iter()
            .find(|r| r.name == "smlib_extrude")
            .map_or(0.0, |r| r.total.as_secs_f64() * 1e3);
        let world = native.world_state().unwrap();
        let (faces, edge_count) = world.bodies.iter().fold((0, 0), |(f, e), b| {
            (f + b.body.face_count(), e + b.body.edge_count())
        });
        eprintln!(
            "{shape} {edges} edges: {:.0} ms ({extrude:.0} ms smlib_extrude), {faces} faces, {edge_count} edges",
            took.as_secs_f64() * 1e3
        );
    }
}
