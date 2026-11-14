//! The board-level items of a `.kicad_pcb` the importer reads beyond the
//! footprints: the board outline (`Edge.Cuts` drawings), the physical
//! stackup, nets, tracks (straight and arc), vias and zones, as typed
//! values in KiCad's frame (whole nanometres, y down), like `board`.
//!
//! Format facts come from the founder's files, from `kicad-cli` plots of
//! synthetic boards and from KiCad's file-format documentation for facts
//! only (decision D1).
use crate::board::{BoardError, Nm, PointNm, Result, length};
use crate::sexpr::Node;

/// Most outline drawings, tracks, vias and zones one board may hold.
pub const MAX_EDGE_ITEMS: usize = 16_384;
pub const MAX_TRACKS: usize = 262_144;
pub const MAX_VIAS: usize = 131_072;
pub const MAX_ZONES: usize = 1024;
/// Most points of one zone outline.
pub const MAX_ZONE_POINTS: usize = 65_536;

/// One drawing on `Edge.Cuts`, board frame.
#[derive(Clone, Debug, PartialEq)]
pub enum EdgeItem {
    Line {
        start: PointNm,
        end: PointNm,
    },
    /// Through `mid` from `start` to `end`.
    Arc {
        start: PointNm,
        mid: PointNm,
        end: PointNm,
    },
    Circle {
        center: PointNm,
        radius: f64,
    },
    Rect {
        start: PointNm,
        end: PointNm,
    },
    Poly {
        points: Vec<PointNm>,
    },
}

/// One layer of the physical stackup, top to bottom.
#[derive(Clone, Debug, PartialEq)]
pub enum StackLayer {
    Copper {
        name: String,
        thickness: Option<Nm>,
    },
    /// One dielectric between two copper layers; `sublayers` beyond one
    /// are summed into `thickness`.
    Dielectric {
        core: bool,
        thickness: Option<Nm>,
        epsilon_r: Option<f64>,
        sublayers: usize,
    },
    Mask {
        front: bool,
        thickness: Option<Nm>,
    },
    /// Silkscreen and paste rows.
    Other,
}

/// A straight track or an arc track.
#[derive(Clone, Debug, PartialEq)]
pub struct KTrack {
    pub start: PointNm,
    /// For an arc: the point it passes half way.
    pub mid: Option<PointNm>,
    pub end: PointNm,
    pub width: Nm,
    pub layer: String,
    pub net: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct KVia {
    pub at: PointNm,
    pub size: Nm,
    pub drill: Nm,
    /// The two copper layers it joins.
    pub layers: [String; 2],
    /// `blind` or `micro`; `None` for a through via.
    pub kind: Option<String>,
    pub net: u32,
    /// KiCad's per-via tenting, when written (front, back).
    pub tenting: Option<(bool, bool)>,
    /// Inner layers where unused annular rings are removed.
    pub remove_unused_layers: bool,
}

/// How a zone joins pads of its net.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ZoneConnect {
    /// Thermal reliefs (no keyword written).
    #[default]
    Thermal,
    Solid,
    None,
    ThroughHoleOnly,
}

/// A copper zone or a rule area (keepout).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct KZone {
    pub net: u32,
    pub layers: Vec<String>,
    pub priority: u32,
    pub connect: ZoneConnect,
    pub clearance: Option<Nm>,
    pub min_thickness: Option<Nm>,
    pub thermal_gap: Option<Nm>,
    pub thermal_bridge: Option<Nm>,
    /// `island_removal_mode`: 0 always, 1 never, 2 below an area.
    pub island_mode: Option<u32>,
    /// Square millimetres.
    pub island_area_min: Option<f64>,
    pub hatched: bool,
    /// Corner smoothing (chamfer or fillet) of the fill.
    pub smoothing: bool,
    /// A rule area: what it keeps out (tracks, vias, pads, copper pour,
    /// footprints), or `None` for a copper zone.
    pub keepout: Option<[bool; 5]>,
    /// The outline, one ring per `polygon`.
    pub outlines: Vec<Vec<PointNm>>,
    /// Arcs in the outline (not imported).
    pub arcs: usize,
    /// KiCad's own fill: `filled_polygon` rings per layer.
    pub filled: usize,
}

/// The board-level items, with counts of what was not read.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Items {
    pub edges: Vec<EdgeItem>,
    /// `Edge.Cuts` drawings that are curves (Béziers), not imported.
    pub edge_curves: usize,
    /// `Edge.Cuts` drawings inside footprints, not imported.
    pub footprint_edges: usize,
    pub stackup: Vec<StackLayer>,
    pub nets: std::collections::BTreeMap<u32, String>,
    pub tracks: Vec<KTrack>,
    pub vias: Vec<KVia>,
    pub zones: Vec<KZone>,
}

fn point(node: &Node, path: &str) -> Result<PointNm> {
    let get = |k: usize| {
        node.arg(k)
            .ok_or_else(|| BoardError::Value(path.into()))
            .and_then(|a| length(a, path))
    };
    Ok([get(0)?, get(1)?])
}

fn child_point(node: &Node, head: &str, path: &str) -> Result<PointNm> {
    let p = format!("{path}/{head}");
    point(
        node.child(head)
            .ok_or_else(|| BoardError::Value(p.clone()))?,
        &p,
    )
}

fn child_len(node: &Node, head: &str, path: &str) -> Result<Option<Nm>> {
    let p = format!("{path}/{head}");
    node.child(head)
        .map(|c| {
            c.arg(0)
                .ok_or_else(|| BoardError::Value(p.clone()))
                .and_then(|a| length(a, &p))
        })
        .transpose()
}

fn child_num(node: &Node, head: &str, path: &str) -> Result<Option<f64>> {
    let p = format!("{path}/{head}");
    node.child(head)
        .map(|c| {
            c.arg(0)
                .and_then(|a| a.parse::<f64>().ok())
                .filter(|v| v.is_finite())
                .ok_or_else(|| BoardError::Value(p.clone()))
        })
        .transpose()
}

fn layer_of(node: &Node) -> Option<&str> {
    node.child("layer").and_then(|l| l.arg(0))
}

/// Read an `Edge.Cuts` drawing (`gr_line`, `gr_arc`, `gr_circle`,
/// `gr_rect`, `gr_poly`); `Ok(None)` for a curve.
pub fn edge(node: &Node) -> Result<Option<EdgeItem>> {
    let head = node.head().unwrap_or_default();
    let path = head;
    Ok(Some(match head {
        "gr_line" => EdgeItem::Line {
            start: child_point(node, "start", path)?,
            end: child_point(node, "end", path)?,
        },
        "gr_arc" => EdgeItem::Arc {
            start: child_point(node, "start", path)?,
            mid: child_point(node, "mid", path)?,
            end: child_point(node, "end", path)?,
        },
        "gr_circle" => {
            let center = child_point(node, "center", path)?;
            let end = child_point(node, "end", path)?;
            EdgeItem::Circle {
                center,
                radius: ((end[0] - center[0]) as f64).hypot((end[1] - center[1]) as f64),
            }
        }
        "gr_rect" => EdgeItem::Rect {
            start: child_point(node, "start", path)?,
            end: child_point(node, "end", path)?,
        },
        "gr_poly" => {
            let pts = node
                .child("pts")
                .ok_or_else(|| BoardError::Value(format!("{path}/pts")))?;
            let mut points = Vec::new();
            for xy in pts.args() {
                if xy.head() != Some("xy") {
                    return Ok(None);
                }
                if points.len() >= MAX_ZONE_POINTS {
                    return Err(BoardError::Limit("an outline polygon of too many points"));
                }
                points.push(point(xy, path)?);
            }
            EdgeItem::Poly { points }
        }
        _ => return Ok(None),
    }))
}

/// The exact box of `Edge.Cuts` drawings in KiCad's frame (`[min, max]`,
/// nm), arcs by their swept extremes, or `None` without any.
pub fn edge_bounds(edges: &[EdgeItem]) -> Option<[PointNm; 2]> {
    let mut points: Vec<[f64; 2]> = Vec::new();
    let f = |p: PointNm| [p[0] as f64, p[1] as f64];
    for item in edges {
        match item {
            EdgeItem::Line { start, end } | EdgeItem::Rect { start, end } => {
                points.extend([f(*start), f(*end)]);
            }
            EdgeItem::Poly { points: ring } => points.extend(ring.iter().map(|p| f(*p))),
            EdgeItem::Circle { center, radius } => {
                let c = f(*center);
                points.extend([[c[0] - radius, c[1] - radius], [c[0] + radius, c[1] + radius]]);
            }
            EdgeItem::Arc { start, mid, end } => {
                let (s, m, e) = (f(*start), f(*mid), f(*end));
                points.extend([s, e]);
                let Some((c, r)) = crate::shape::circle_through(s, m, e) else {
                    continue;
                };
                // The cardinal points the sweep from s through m to e passes.
                let angle = |q: [f64; 2]| (q[1] - c[1]).atan2(q[0] - c[0]);
                let tau = std::f64::consts::TAU;
                let (a0, am, a1) = (angle(s), angle(m), angle(e));
                let ccw = (am - a0).rem_euclid(tau) < (a1 - a0).rem_euclid(tau);
                let (from, sweep) = if ccw {
                    (a0, (a1 - a0).rem_euclid(tau))
                } else {
                    (a1, (a0 - a1).rem_euclid(tau))
                };
                for k in 0..4 {
                    let t = f64::from(k) * std::f64::consts::FRAC_PI_2;
                    if (t - from).rem_euclid(tau) <= sweep {
                        points.push([c[0] + r * t.cos(), c[1] + r * t.sin()]);
                    }
                }
            }
        }
    }
    let first = *points.first()?;
    let (mut min, mut max) = (first, first);
    for p in &points {
        min = [min[0].min(p[0]), min[1].min(p[1])];
        max = [max[0].max(p[0]), max[1].max(p[1])];
    }
    // Whole nanometres: an arc's centre is fitted through points written
    // to the nanometre, so its extremes are good to about that.
    Some([
        [min[0].round() as Nm, min[1].round() as Nm],
        [max[0].round() as Nm, max[1].round() as Nm],
    ])
}

/// Whether a board-level drawing is on `Edge.Cuts`.
pub fn on_edge(node: &Node) -> bool {
    layer_of(node) == Some("Edge.Cuts")
}

/// Read `setup/stackup`.
pub fn stackup(node: &Node) -> Result<Vec<StackLayer>> {
    let path = "setup/stackup";
    let mut out = Vec::new();
    for layer in node.children("layer") {
        let name = layer.arg(0).unwrap_or_default();
        let kind = layer
            .child("type")
            .and_then(|t| t.arg(0))
            .unwrap_or_default();
        // Sublayers repeat `thickness`, `epsilon_r` and the rest after an
        // `addsublayer` atom: thicknesses add up, the first Dk stands.
        let thicknesses: Vec<Nm> = layer
            .children("thickness")
            .map(|t| {
                t.arg(0)
                    .ok_or_else(|| BoardError::Value(format!("{path}/thickness")))
                    .and_then(|a| length(a, path))
            })
            .collect::<Result<_>>()?;
        let thickness = if thicknesses.is_empty() {
            None
        } else {
            Some(thicknesses.iter().sum())
        };
        out.push(match kind {
            "copper" => StackLayer::Copper {
                name: name.to_owned(),
                thickness,
            },
            "core" | "prepreg" => StackLayer::Dielectric {
                core: kind == "core",
                thickness,
                epsilon_r: child_num(layer, "epsilon_r", path)?,
                sublayers: thicknesses.len().max(1),
            },
            "Top Solder Mask" | "Bottom Solder Mask" => StackLayer::Mask {
                front: kind.starts_with("Top"),
                thickness,
            },
            _ => StackLayer::Other,
        });
    }
    Ok(out)
}

/// Read `(net n "name")` at the top level.
pub fn net(node: &Node) -> Result<(u32, String)> {
    let n = node
        .arg(0)
        .and_then(|a| a.parse::<u32>().ok())
        .ok_or_else(|| BoardError::Value("net".into()))?;
    Ok((n, node.arg(1).unwrap_or_default().to_owned()))
}

fn net_number(node: &Node, path: &str) -> Result<u32> {
    match node.child("net") {
        None => Ok(0),
        Some(n) => n
            .arg(0)
            .and_then(|a| a.parse::<u32>().ok())
            .ok_or_else(|| BoardError::Value(format!("{path}/net"))),
    }
}

/// Read a `segment` or an `arc` track.
pub fn track(node: &Node) -> Result<KTrack> {
    let path = node.head().unwrap_or("segment");
    let arc = path == "arc";
    Ok(KTrack {
        start: child_point(node, "start", path)?,
        mid: if arc {
            Some(child_point(node, "mid", path)?)
        } else {
            None
        },
        end: child_point(node, "end", path)?,
        width: child_len(node, "width", path)?
            .ok_or_else(|| BoardError::Value(format!("{path}/width")))?,
        layer: layer_of(node).unwrap_or_default().to_owned(),
        net: net_number(node, path)?,
    })
}

/// Read a `via`.
pub fn via(node: &Node) -> Result<KVia> {
    let path = "via";
    let layers: Vec<String> = node
        .child("layers")
        .map(|l| {
            l.args()
                .iter()
                .filter_map(Node::atom)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let [a, b] = <[String; 2]>::try_from(layers)
        .map_err(|_| BoardError::Value(format!("{path}/layers")))?;
    let kind = node
        .args()
        .iter()
        .filter_map(Node::atom)
        .find(|a| matches!(*a, "blind" | "micro"))
        .map(str::to_owned);
    let tenting = node.child("tenting").map(|t| {
        let atoms: Vec<&str> = t.args().iter().filter_map(Node::atom).collect();
        (atoms.contains(&"front"), atoms.contains(&"back"))
    });
    Ok(KVia {
        at: child_point(node, "at", path)?,
        size: child_len(node, "size", path)?
            .ok_or_else(|| BoardError::Value(format!("{path}/size")))?,
        drill: child_len(node, "drill", path)?
            .ok_or_else(|| BoardError::Value(format!("{path}/drill")))?,
        layers: [a, b],
        kind,
        net: net_number(node, path)?,
        tenting,
        remove_unused_layers: node.child("remove_unused_layers").is_some()
            || node.args().iter().any(|a| a.atom() == Some("remove_unused_layers")),
    })
}

fn yes(node: Option<&Node>) -> bool {
    node.and_then(|n| n.arg(0)).is_some_and(|a| a == "not_allowed")
}

/// Read a `zone`.
pub fn zone(node: &Node) -> Result<KZone> {
    let path = "zone";
    let mut zone = KZone {
        net: net_number(node, path)?,
        priority: child_num(node, "priority", path)?.map_or(0, |p| p.max(0.0) as u32),
        min_thickness: child_len(node, "min_thickness", path)?,
        ..KZone::default()
    };
    if let Some(layer) = layer_of(node) {
        zone.layers.push(layer.to_owned());
    }
    if let Some(layers) = node.child("layers") {
        zone.layers
            .extend(layers.args().iter().filter_map(Node::atom).map(str::to_owned));
    }
    if let Some(connect) = node.child("connect_pads") {
        zone.connect = match connect.arg(0) {
            Some("yes") => ZoneConnect::Solid,
            Some("no") => ZoneConnect::None,
            Some("thru_hole_only") => ZoneConnect::ThroughHoleOnly,
            _ => ZoneConnect::Thermal,
        };
        zone.clearance = child_len(connect, "clearance", path)?;
    }
    if let Some(fill) = node.child("fill") {
        zone.thermal_gap = child_len(fill, "thermal_gap", path)?;
        zone.thermal_bridge = child_len(fill, "thermal_bridge_width", path)?;
        zone.island_mode = child_num(fill, "island_removal_mode", path)?.map(|m| m as u32);
        zone.island_area_min = child_num(fill, "island_area_min", path)?;
        zone.hatched = fill.child("mode").and_then(|m| m.arg(0)) == Some("hatch");
        zone.smoothing = fill
            .child("smoothing")
            .and_then(|s| s.arg(0))
            .is_some_and(|s| s != "none");
    }
    if let Some(k) = node.child("keepout") {
        zone.keepout = Some([
            yes(k.child("tracks")),
            yes(k.child("vias")),
            yes(k.child("pads")),
            yes(k.child("copperpour")),
            yes(k.child("footprints")),
        ]);
    }
    for polygon in node.children("polygon") {
        let Some(pts) = polygon.child("pts") else {
            continue;
        };
        let mut ring = Vec::new();
        for xy in pts.args() {
            match xy.head() {
                Some("xy") => {
                    if ring.len() >= MAX_ZONE_POINTS {
                        return Err(BoardError::Limit("a zone outline of too many points"));
                    }
                    ring.push(point(xy, path)?);
                }
                _ => zone.arcs += 1,
            }
        }
        zone.outlines.push(ring);
    }
    zone.filled = node.children("filled_polygon").count();
    Ok(zone)
}
