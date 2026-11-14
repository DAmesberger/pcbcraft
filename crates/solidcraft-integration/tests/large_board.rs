//! Scale benchmark: a synthetic large board built here (original: a grid
//! of the showcase's parts, a via field and routed channels; no real
//! design), timed through the paths the app takes. It reports; it holds no
//! time bar, so a loaded machine cannot fail it. The founder's targets
//! (10,000 vias in seconds, a 3D view that opens instantly) are targets
//! until a quiet machine measures them.
//!
//! `cargo test --profile ci -p sparksmith-solidcraft --test large_board -- --ignored --nocapture`
//! (release-like; a debug build is several times slower).
use sparksmith_core::issues::IssueScope;
use sparksmith_core::layout::fill::RefillTarget;
use sparksmith_core::layout::{self, LayoutCommand, RouteRequest};
use sparksmith_core::model::{Layer, Placement, Stackup};
use sparksmith_core::{Engine, Id, Operation, Project, Query, Track, Via, ViaSpan, format};
use sparksmith_solidcraft::models::imported::SourceCache;
use sparksmith_solidcraft::project::{self, Flat, plan};
use std::time::Instant;

type Fallible<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// Via field: `VIA_GRID` × `VIA_GRID` through vias at `PITCH`.
const VIA_GRID: usize = 100;
const PITCH: f64 = 1.27;
/// Clear of the showcase's own parts and copper, which stay where they are.
const VIA_ORIGIN: [f64; 2] = [80.0, 10.0];
/// Routed channels between via rows, on every copper layer, of segments
/// two pitches long.
const SEGMENT_PITCHES: usize = 2;
const TRACK_WIDTH: f64 = 0.15;
const PLACEMENTS: usize = 2_000;
/// Placement grid pitch, mm; above the via field.
const PART_PITCH: f64 = 9.0;
const PART_ORIGIN: [f64; 2] = [12.0, 150.0];
const BOARD: [f64; 2] = [500.0, 560.0];

fn ms(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1e3
}

/// The large board: the showcase's 4-layer copy with its small parts
/// repeated `PLACEMENTS` times, `VIA_GRID`² vias on two nets and about
/// 20,000 tracks in the channels between them. Legal by construction:
/// via gap 0.67 mm, track to via 0.26 mm, parts 9 mm apart.
fn large_board() -> Fallible<Project> {
    let mut p = Engine::showcase()?.project().clone();
    p.stackup = Stackup::preset(4)?;
    p.board_size = BOARD;
    for fill in p.fills.iter_mut() {
        fill.computed = None;
    }
    let nets: Vec<Id> = p.nets.iter().map(|n| n.id).collect();
    let [a, b] = [nets[0], nets[1 % nets.len()]];
    // Parts: the showcase's placed parts whose pads fit a pitch cell.
    let pads = layout::pads(&p);
    let template: Vec<Placement> = p
        .placements
        .iter()
        .filter(|placement| {
            pads.iter()
                .filter(|pad| pad.instance == placement.instance)
                .all(|pad| {
                    (pad.position[0] - placement.position[0]).abs() < PART_PITCH * 0.3
                        && (pad.position[1] - placement.position[1]).abs() < PART_PITCH * 0.3
                })
        })
        .cloned()
        .collect();
    assert!(!template.is_empty(), "no small parts to repeat");
    let columns = ((BOARD[0] - 2.0 * PART_ORIGIN[0]) / PART_PITCH) as usize;
    for k in 0..PLACEMENTS {
        let source = &template[k % template.len()];
        let mut instance = p
            .instance(source.instance)
            .ok_or("template instance")?
            .clone();
        let id = p.allocate()?;
        instance.id = id;
        instance.designator = format!("{}S{k}", instance.designator);
        p.instances.push(instance);
        p.placements.push(Placement {
            instance: id,
            position: [
                PART_ORIGIN[0] + (k % columns) as f64 * PART_PITCH,
                PART_ORIGIN[1] + (k / columns) as f64 * PART_PITCH,
            ],
            rotation_deg: 0.0,
            layer: Layer::Top,
            locked: false,
            fill_override: Default::default(),
            pad_fill_overrides: Vec::new(),
        });
    }
    // Each new part's terminals start on nets of their own, as placing a
    // part from the library leaves them.
    sparksmith_core::nets::reconcile(&mut p)?;
    for i in 0..VIA_GRID {
        for j in 0..VIA_GRID {
            let id = p.allocate()?;
            p.vias.push(Via {
                id,
                net: if (i + j) % 2 == 0 { a } else { b },
                position: [
                    VIA_ORIGIN[0] + i as f64 * PITCH,
                    VIA_ORIGIN[1] + j as f64 * PITCH,
                ],
                drill: 0.3,
                diameter: 0.6,
                span: ViaSpan::Through,
                tented: true,
                locked: false,
            });
        }
    }
    let layers = p.stackup.layers();
    let length = SEGMENT_PITCHES as f64 * PITCH;
    for (l, layer) in layers.iter().enumerate() {
        for row in 0..VIA_GRID - 1 {
            let y = VIA_ORIGIN[1] + (row as f64 + 0.5) * PITCH;
            let net = if (row + l) % 2 == 0 { a } else { b };
            for s in 0..(VIA_GRID - 1) / SEGMENT_PITCHES {
                let x = VIA_ORIGIN[0] + s as f64 * length;
                let id = p.allocate()?;
                p.tracks.push(Track {
                    id,
                    net,
                    start: [x, y],
                    end: [x + length, y],
                    width: TRACK_WIDTH,
                    layer: *layer,
                    locked: false,
                });
            }
        }
    }
    Ok(p)
}

#[test]
#[ignore = "scale benchmark; run with --ignored --nocapture"]
fn large_board_timings() -> Fallible {
    let load = std::fs::read_to_string("/proc/loadavg").ok().or_else(|| {
        std::process::Command::new("sysctl")
            .args(["-n", "vm.loadavg"])
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
    });
    eprintln!("load average: {}", load.unwrap_or_default().trim());
    let materials =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../library/materials");
    sparksmith_solidcraft::looks::ensure(|| {
        sparksmith_core::materials::MaterialLibrary::load(|name| {
            std::fs::read_to_string(materials.join(name))
                .map_err(|error| sparksmith_core::Error::Invalid(error.to_string()))
        })
    })?;
    let start = Instant::now();
    let board = large_board()?;
    eprintln!(
        "built in {:.0} ms: {} placements, {} pads, {} nets, {} vias, {} tracks",
        ms(start),
        board.placements.len(),
        layout::placed_pads(&board).len(),
        board.nets.len(),
        board.vias.len(),
        board.tracks.len()
    );

    // Open: decode and validate the saved file.
    let mut file = Engine::showcase()?.to_file();
    file.project = board;
    let start = Instant::now();
    let encoded = format::encode(&file);
    let saved = ms(start);
    let start = Instant::now();
    let opened = match encoded {
        Ok(bytes) => {
            eprintln!(
                "save (encode): {saved:.0} ms, {:.1} MiB",
                bytes.len() as f64 / f64::from(1 << 20)
            );
            let decoded = format::decode(&bytes)?;
            Engine::from_file(decoded)?
        }
        Err(error) => {
            eprintln!("file not encodable ({error}); opening from memory");
            Engine::from_file(file)?
        }
    };
    eprintln!("open (decode and validate): {:.0} ms", ms(start));
    let mut engine = opened;

    // The copper graph and the full board check.
    let start = Instant::now();
    let graph = engine.copper_graph();
    eprintln!("copper graph: {:.0} ms", ms(start));
    let start = Instant::now();
    let findings = layout::checks_with(engine.project(), &graph, engine.check_settings());
    eprintln!(
        "board checks: {:.0} ms, {} findings ({} clearance)",
        ms(start),
        findings.len(),
        findings
            .iter()
            .filter(|f| f.code == "board.clearance")
            .count()
    );
    let mut codes = std::collections::BTreeMap::<&str, usize>::new();
    for finding in &findings {
        *codes.entry(finding.code.as_str()).or_default() += 1;
    }
    eprintln!("  by code: {codes:?}");
    if let Some(f) = findings.iter().find(|f| f.code == "board.clearance") {
        eprintln!("  e.g. {}", f.message);
    }
    assert!(
        findings.iter().all(|f| f.code != "board.check_limit"),
        "the checks stopped at a limit"
    );
    let start = Instant::now();
    engine.query(Query::Issues {
        scope: IssueScope::All,
    })?;
    eprintln!("project.issues (every area): {:.0} ms", ms(start));

    // Typical edits, each a committed operation through the engine.
    let via = engine.project().vias[VIA_GRID * VIA_GRID / 2].clone();
    let track = engine.project().tracks[engine.project().tracks.len() / 2].clone();
    let edits = [
        (
            "via_tent",
            Operation::Layout(LayoutCommand::SetViaTented {
                id: via.id,
                tented: false,
            }),
        ),
        (
            "route (new track)",
            Operation::Layout(LayoutCommand::Route {
                request: RouteRequest {
                    net: track.net,
                    start: track.end,
                    end: [track.end[0] - 0.6, track.end[1]],
                    width: TRACK_WIDTH,
                    layer: track.layer,
                },
            }),
        ),
        (
            "track_set width",
            Operation::Layout(LayoutCommand::SetTrack {
                id: track.id,
                width: Some(0.16),
                layer: None,
            }),
        ),
        (
            "move a part",
            Operation::Layout(LayoutCommand::Move {
                instance: engine.project().placements.last().ok_or("part")?.instance,
                position: [BOARD[0] - 20.0, BOARD[1] - 20.0],
                rotation_deg: 90.0,
                layer: Layer::Top,
            }),
        ),
    ];
    for (what, operation) in edits {
        let at = engine.revision();
        let start = Instant::now();
        let outcome = engine.apply(at, operation);
        let applied = ms(start);
        let start = Instant::now();
        let _ = engine.copper_graph();
        eprintln!(
            "apply {what}: {applied:.0} ms ({}), then copper graph {:.0} ms",
            match &outcome {
                Ok(_) => "applied".to_owned(),
                Err(error) => format!("refused: {error}"),
            },
            ms(start)
        );
    }
    // A typical edit repeated: toggling one via's tenting, best and mean
    // of 20 (the machine is shared).
    let mut times = Vec::new();
    for k in 0..20 {
        let at = engine.revision();
        let start = Instant::now();
        engine.apply(
            at,
            Operation::Layout(LayoutCommand::SetViaTented {
                id: via.id,
                tented: k % 2 == 1,
            }),
        )?;
        times.push(ms(start));
    }
    eprintln!(
        "via_tent x20: best {:.0} ms, mean {:.0} ms",
        times.iter().copied().fold(f64::MAX, f64::min),
        times.iter().sum::<f64>() / times.len() as f64
    );
    let start = Instant::now();
    engine.undo()?;
    eprintln!("undo: {:.0} ms", ms(start));

    // Refill every pour.
    let start = Instant::now();
    let filled = layout::fill::refill(engine.project(), RefillTarget::All)?;
    eprintln!("refill {} pours: {:.0} ms", filled.len(), ms(start));

    // The 3D view: the board plan, the live model, and its update after
    // an edit.
    let start = Instant::now();
    let planned = plan::of(engine.project())?;
    eprintln!(
        "board plan: {:.0} ms, {} bodies",
        ms(start),
        planned.bodies().count()
    );
    let sources = SourceCache::default();
    let start = Instant::now();
    let model = project::build_flat(engine.project(), &sources, None, Flat::Meshed)?;
    eprintln!("3D live model (meshed): {:.0} ms", ms(start));
    let at = engine.revision();
    engine.apply(
        at,
        Operation::Layout(LayoutCommand::SetViaTented {
            id: via.id,
            tented: true,
        }),
    )?;
    let start = Instant::now();
    let (_, rebuilt) = project::update(model, engine.project(), &sources, None)?;
    eprintln!(
        "3D update after a via edit: {:.0} ms ({rebuilt:?})",
        ms(start)
    );
    Ok(())
}
