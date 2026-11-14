//! Board parity (`cargo xtask board-parity`): every example board's exact
//! native build, body by body, against
//!
//! 1. the record of the build as it was before the board plan existed
//!    (`tests/board_parity/<board>.jsonl`), so extracting the plan changed
//!    no body; and
//! 2. the board plan the build was made from ([`plan::parity`]): each
//!    flat body the plan describes is built once, of its kind, source and
//!    layer, fitting it (its top and bottom faces on the plan's loops, its
//!    holes and heights the plan's, a copper body's volume too), and
//!    nothing flat is built that the plan does not describe; and
//! 3. the live view's model of the board ([`designs::parity`]): each part,
//!    its package revision's own design placed by the document's
//!    transform, lies where the exact build's solved joints hold it, every
//!    vertex within 1 µm.
//!
//! Bodies are compared as sets, by kind, source object, layer, bounding box
//! and volume. `SPARKSMITH_BLESS_PARITY=1` writes the records anew (only
//! when a change to the build is meant to change its bodies; say so in the
//! commit).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use sparksmith_core::Project;
use sparksmith_solidcraft::project::{self, NativeModel, designs, mesh, plan};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// Box corners agree to this, mm.
const BOX_MM: f64 = 1e-6;
/// Volumes agree to this fraction (and [`VOLUME_FLOOR_MM3`]).
const VOLUME_RELATIVE: f64 = 1e-6;
const VOLUME_FLOOR_MM3: f64 = 1e-9;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Every example board: each `.sparksmith` file under `examples/` with
/// placed parts or copper on its board.
fn boards() -> Vec<(String, Project)> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(root().join("examples")).unwrap() {
        let dir = entry.unwrap().path();
        if !dir.is_dir() {
            continue;
        }
        for file in std::fs::read_dir(&dir).unwrap() {
            let path = file.unwrap().path();
            if path.extension().is_some_and(|e| e == "sparksmith") {
                files.push(path);
            }
        }
    }
    files.sort();
    files
        .into_iter()
        .filter_map(|path| {
            let project = sparksmith_io::read_project(&path)
                .unwrap()
                .into_document()
                .project
                .as_ref()
                .clone();
            let board = !project.placements.is_empty()
                || !project.tracks.is_empty()
                || !project.vias.is_empty()
                || !project.fills.is_empty();
            let stem = path.file_stem()?.to_string_lossy().into_owned();
            board.then_some((stem, project))
        })
        .collect()
}

/// One native body as compared.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct Record {
    kind: String,
    source: String,
    layer: String,
    min: [f64; 3],
    max: [f64; 3],
    volume: f64,
}

impl Record {
    /// What must match exactly; the numbers match within tolerance.
    fn key(&self) -> (String, String, String) {
        (self.kind.clone(), self.source.clone(), self.layer.clone())
    }

    fn near(&self, other: &Self) -> bool {
        let corners = self
            .min
            .iter()
            .chain(&self.max)
            .zip(other.min.iter().chain(&other.max))
            .all(|(a, b)| (a - b).abs() <= BOX_MM);
        let volume = (self.volume - other.volume).abs()
            <= VOLUME_FLOOR_MM3 + VOLUME_RELATIVE * self.volume.abs().max(other.volume.abs());
        corners && volume
    }
}

fn layer_name(layer: Option<sparksmith_core::CopperLayer>) -> String {
    layer.map_or_else(|| "-".into(), |l| format!("{l:?}"))
}

/// The source object a body came from, as text.
fn source_name(model: &NativeModel, item: Option<u64>) -> String {
    let Some(source) = item.and_then(|pick| model.sources.iter().find(|s| s.pick_id == pick))
    else {
        return "board".into();
    };
    if let (Some(instance), Some(pad)) = (source.instance, source.pad) {
        format!("pad {instance:?} {pad:?}")
    } else if let Some(instance) = source.instance {
        format!("part {instance:?}")
    } else if let Some(track) = source.track {
        format!("track {track:?}")
    } else if let Some(via) = source.via {
        format!("via {via:?}")
    } else if let Some(fill) = source.fill {
        format!("fill {fill:?}")
    } else {
        format!("other {}", source.label)
    }
}

/// The native bodies of `model` (its display-only layers left out).
fn records(model: &NativeModel) -> Vec<Record> {
    let count = model.native().unwrap().world_state().unwrap().bodies.len();
    model.geometry[..count]
        .iter()
        .map(|g| Record {
            kind: format!("{:?}", g.kind),
            source: source_name(model, g.item),
            layer: layer_name(g.layer),
            min: g.min,
            max: g.max,
            volume: g.volume.get(),
        })
        .collect()
}

/// Match `left` against `right` as sets; returns what each side has that
/// the other lacks.
fn unmatched<'a>(left: &'a [Record], right: &'a [Record]) -> (Vec<&'a Record>, Vec<&'a Record>) {
    let mut groups: BTreeMap<(String, String, String), Vec<&Record>> = BTreeMap::new();
    for r in right {
        groups.entry(r.key()).or_default().push(r);
    }
    let mut missing = Vec::new();
    for l in left {
        let group = groups.entry(l.key()).or_default();
        match group.iter().position(|r| l.near(r)) {
            Some(at) => {
                group.swap_remove(at);
            }
            None => missing.push(l),
        }
    }
    let extra = groups.into_values().flatten().collect();
    (missing, extra)
}

fn record_path(board: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/board_parity")
        .join(format!("{board}.jsonl"))
}

fn report(what: &str, list: &[&Record]) -> String {
    let mut out = format!("{} {what}:\n", list.len());
    for r in list.iter().take(20) {
        out.push_str(&format!("  {r:?}\n"));
    }
    out
}

#[test]
#[ignore = "board parity: run by `cargo xtask board-parity`"]
fn example_boards_match_their_record_and_plan() {
    // The 3D build reads the shipped material library.
    let _context = sparksmith_core::library::standards::Context::fixture().map(|c| c.enter());
    let bless = std::env::var_os("SPARKSMITH_BLESS_PARITY").is_some();
    let boards = boards();
    assert!(!boards.is_empty(), "no example boards found");
    let mut failures = Vec::new();
    for (name, project) in &boards {
        let start = Instant::now();
        let model = project::build(project).unwrap();
        let built = records(&model);
        eprintln!(
            "{name}: {} native bodies built in {:.1} s",
            built.len(),
            start.elapsed().as_secs_f64()
        );
        let path = record_path(name);
        if bless {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            let text: String = built
                .iter()
                .map(|r| serde_json::to_string(r).unwrap() + "\n")
                .collect();
            std::fs::write(&path, text).unwrap();
        } else {
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            let recorded: Vec<Record> = text
                .lines()
                .map(|l| serde_json::from_str(l).unwrap())
                .collect();
            let (missing, extra) = unmatched(&recorded, &built);
            if !missing.is_empty() || !extra.is_empty() {
                failures.push(format!(
                    "{name} against its record: {}{}",
                    report("recorded bodies not built", &missing),
                    report("built bodies not recorded", &extra)
                ));
            }
        }
        let start = Instant::now();
        let plan = plan::of(project).unwrap();
        eprintln!(
            "{name}: plan of {} flat bodies in {:.0} ms",
            plan.bodies().count(),
            start.elapsed().as_secs_f64() * 1e3
        );
        let differences = plan::parity(&plan, &model).unwrap();
        if !differences.is_empty() {
            failures.push(format!(
                "{name} against its plan, {} differences:\n  {}",
                differences.len(),
                differences
                    .iter()
                    .take(20)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("\n  ")
            ));
        }
        // The live view's model: each part its package revision's own
        // design, placed by the document's transform where the exact
        // build's solved joints hold it.
        let start = Instant::now();
        let live = project::build_flat(
            project,
            &sparksmith_solidcraft::models::imported::SourceCache::default(),
            None,
            project::Flat::Meshed,
        )
        .unwrap();
        let live_ms = start.elapsed().as_secs_f64() * 1e3;
        let placed = designs::parity(&model, &live).unwrap();
        eprintln!(
            "{name}: live model ({} part designs) in {live_ms:.0} ms; {} part bodies, {} vertices, \
             at most {:.2e} mm from the exact build's",
            live.part_designs(),
            placed.bodies,
            placed.vertices,
            placed.worst_mm
        );
        if !placed.differences.is_empty() {
            failures.push(format!(
                "{name}'s parts in the live model against the exact build, {} differences:\n  {}",
                placed.differences.len(),
                placed
                    .differences
                    .iter()
                    .take(20)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("\n  ")
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Every example board's flat bodies meshed straight from its plan (the
/// live view's path, [`mesh`]) are the plan's bodies ([`mesh::parity`]):
/// one mesh per plan body, of its kind, source and layer, its box and
/// volume within what flattening explains, each laminate slab its own
/// body as the exact build makes it. Prints the meshing times (targets:
/// the ESP32 board's bodies in under 100 ms; one track's width edit meshed
/// again in a few ms) and the laminate's share: its slabs, against one
/// dielectric slab alone (what one merged laminate body cost).
#[test]
fn example_boards_mesh_as_their_plan() {
    // The 3D build reads the shipped material library.
    let _context = sparksmith_core::library::standards::Context::fixture().map(|c| c.enter());
    let mut failures = Vec::new();
    for (name, project) in &boards() {
        let plan = plan::of(project).unwrap();
        let sources = mesh::plan_sources(&plan);
        let mut memo = mesh::Memo::default();
        let start = Instant::now();
        let (meshed, cold) = mesh::bodies(&plan, &sources, &mut memo).unwrap();
        let cold_ms = start.elapsed().as_secs_f64() * 1e3;
        let triangles: usize = meshed.iter().map(|m| m.geometry.mesh.triangles.len()).sum();
        let held: usize = {
            let mut seen = std::collections::HashSet::new();
            meshed
                .iter()
                .filter(|m| seen.insert(std::sync::Arc::as_ptr(&m.geometry.mesh)))
                .map(|m| m.geometry.mesh.triangles.len())
                .sum()
        };
        let differences = mesh::parity(&plan, &meshed).unwrap();
        if !differences.is_empty() {
            failures.push(format!(
                "{name} meshed against its plan, {} differences:\n  {}",
                differences.len(),
                differences
                    .iter()
                    .take(20)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("\n  ")
            ));
        }
        // The laminate alone, cold: its slabs, and one slab through the
        // board's drills (a dielectric's, as one merged body was).
        let laminate_ms = |slabs: Vec<plan::PlanBody>| {
            let only = plan::BoardPlan {
                fills: Vec::new(),
                pads: Vec::new(),
                tracks: Vec::new(),
                vias: Vec::new(),
                laminate: slabs,
            };
            let start = Instant::now();
            let (bodies, _) = mesh::bodies(&only, &[], &mut mesh::Memo::default()).unwrap();
            let ms = start.elapsed().as_secs_f64() * 1e3;
            let triangles: usize = bodies.iter().map(|m| m.geometry.mesh.triangles.len()).sum();
            (ms, triangles)
        };
        let slabs = laminate_ms(plan.laminate.clone());
        let one = plan
            .laminate
            .iter()
            .find(|s| !matches!(s.source, plan::Origin::Laminate(plan::Level::Foil { .. })))
            .cloned()
            .map(|s| laminate_ms(vec![s]));
        // One track made wider: that track alone is meshed again, and on an
        // inner layer its foil level's slab, whose holes are its copper.
        let mut edited = project.clone();
        let mut width_ms = None;
        if let Some(track) = edited.tracks.first_mut() {
            track.width += 0.05;
            let inner = usize::from(track.layer.side().is_none());
            let start = Instant::now();
            let plan = plan::of(&edited).unwrap();
            let plan_ms = start.elapsed().as_secs_f64() * 1e3;
            let start = Instant::now();
            let (_, stats) = mesh::bodies(&plan, &sources, &mut memo).unwrap();
            width_ms = Some((plan_ms, start.elapsed().as_secs_f64() * 1e3));
            assert_eq!(stats.meshed, 1 + inner, "{name}: {stats:?}");
        }
        eprintln!(
            "{name}: {} flat bodies ({} meshes) meshed in {cold_ms:.1} ms, {triangles} triangles drawn \
             ({held} held); laminate alone: {} slabs {:.1} ms ({} triangles), one slab {:.1} ms ({} \
             triangles); a track width edit: plan {:.1} ms, mesh {:.1} ms",
            cold.bodies,
            cold.meshed,
            plan.laminate.len(),
            slabs.0,
            slabs.1,
            one.map_or(f64::NAN, |o| o.0),
            one.map_or(0, |o| o.1),
            width_ms.map_or(f64::NAN, |w| w.0),
            width_ms.map_or(f64::NAN, |w| w.1),
        );
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
