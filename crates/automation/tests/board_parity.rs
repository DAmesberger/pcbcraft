//! Board parity (`cargo xtask board-parity`): every example board's exact
//! native build, body by body, against
//!
//! 1. the record of the build as it was before the board plan existed
//!    (`tests/board_parity/<board>.jsonl`), so extracting the plan changed
//!    no body; and
//! 2. the board plan the build was made from: each flat body the plan
//!    describes is built once, of its kind, source, layer, box and volume,
//!    and nothing flat is built that the plan does not describe.
//!
//! Bodies are compared as sets, by kind, source object, layer, bounding box
//! and volume. `SPARKSMITH_BLESS_PARITY=1` writes the records anew (only
//! when a change to the build is meant to change its bodies; say so in the
//! commit).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use pcbcraft_core::Project;
use pcbcraft_solidcraft::project::{self, NativeModel};
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

/// Every example board: each `.pcbcraft` file under `examples/` with
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
            if path.extension().is_some_and(|e| e == "pcbcraft") {
                files.push(path);
            }
        }
    }
    files.sort();
    files
        .into_iter()
        .filter_map(|path| {
            let project = pcbcraft_io::read_project(&path)
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

fn layer_name(layer: Option<pcbcraft_core::CopperLayer>) -> String {
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
    let count = model.native().world_state().unwrap().bodies.len();
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
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
