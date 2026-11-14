//! The parts of a `.kicad_pcb` board file the footprint import reads, as
//! typed values in KiCad's own frame (millimetre decimals turned into whole
//! nanometres, y growing downward, angles in degrees counter-clockwise as
//! seen on screen). Turning them into Sparksmith's frame is `translate`'s.
//!
//! Format facts come from the founder's files and from what `kicad-cli`
//! plots for synthetic boards (the oracle); KiCad's documentation is used
//! for facts only (decision D1).
use crate::report::Report;
use crate::sexpr::Node;
use sparksmith_core::units::text::{LengthUnit, parse_length};

/// Board file versions accepted: KiCad 7 (20221018) to KiCad 8 (20240108),
/// the versions the test design uses. A newer or older file is refused.
pub const VERSIONS: std::ops::RangeInclusive<u32> = 20221018..=20240108;
/// Most footprints one board may hold (the core's instance bound).
pub const MAX_FOOTPRINTS: usize = 4096;
/// Most pads of one footprint.
pub const MAX_PADS: usize = 4096;
/// Most points of one custom-pad polygon primitive.
pub const MAX_POLY_POINTS: usize = 4096;

/// Why a board was refused as a whole. Messages carry no file content.
#[derive(Debug)]
pub enum BoardError {
    Syntax(crate::sexpr::ParseError),
    NotABoard,
    Version(u32),
    /// A value at a token path could not be read.
    Value(String),
    Limit(&'static str),
}

impl std::fmt::Display for BoardError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Syntax(e) => write!(f, "kicad.syntax: {e}"),
            Self::NotABoard => write!(f, "kicad.not_a_board: the file is not a KiCad board"),
            Self::Version(v) => write!(
                f,
                "kicad.version: board file version {v} is not supported (accepted {}–{})",
                VERSIONS.start(),
                VERSIONS.end()
            ),
            Self::Value(path) => write!(f, "kicad.value: an unreadable value at {path}"),
            Self::Limit(what) => write!(f, "kicad.limit: {what}"),
        }
    }
}

impl std::error::Error for BoardError {}

pub type Result<T> = std::result::Result<T, BoardError>;

/// A length in whole nanometres.
pub type Nm = i64;
pub type PointNm = [Nm; 2];

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Board {
    pub version: u32,
    /// Copper layer names, top to bottom (`F.Cu`, `In1.Cu`, …, `B.Cu`).
    pub copper: Vec<String>,
    /// The auxiliary (drill and place) origin, when set.
    pub aux_origin: Option<PointNm>,
    /// Where our board frame's origin lies in KiCad's: the lower-left
    /// corner of the outline's box (our board outline lies right of and
    /// above its origin), else the auxiliary origin, else the file origin.
    pub origin: PointNm,
    pub setup: Setup,
    pub footprints: Vec<Footprint>,
    /// The outline, stackup, nets, tracks, vias and zones.
    pub items: crate::items::Items,
    /// Board-level objects the importer does not import, by keyword.
    pub other: std::collections::BTreeMap<String, usize>,
}

/// Board-wide pad defaults from `setup`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Setup {
    pub mask_margin: Nm,
    pub paste_margin: Nm,
    pub paste_ratio: f64,
    /// The plot setting that leaves vias out of the mask (`viasonmask`),
    /// when written.
    pub vias_on_mask: Option<bool>,
}

/// Local overrides of the mask and paste margins (a footprint's or a
/// pad's).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Margins {
    pub mask: Option<Nm>,
    pub paste: Option<Nm>,
    pub paste_ratio: Option<f64>,
}

/// How zones join a footprint's or a pad's copper, over the zone's own
/// settings (`zone_connect`, `clearance`, thermal width, gap and angle).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ZoneOverride {
    /// 0 not joined, 1 thermal relief, 2 solid, 3 thermal for
    /// through-hole pads only.
    pub connect: Option<u8>,
    pub clearance: Option<Nm>,
    pub thermal_width: Option<Nm>,
    pub thermal_gap: Option<Nm>,
    /// Spoke angle, degrees (pads only).
    pub thermal_angle: Option<f64>,
}

impl ZoneOverride {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

fn zone_override(node: &Node, path: &str) -> Result<ZoneOverride> {
    let connect = opt_num(node, "zone_connect", path)?
        .map(|v| {
            if (0.0..=3.0).contains(&v) && v.fract() == 0.0 {
                Ok(v as u8)
            } else {
                Err(BoardError::Value(format!("{path}/zone_connect")))
            }
        })
        .transpose()?;
    Ok(ZoneOverride {
        connect,
        clearance: opt_len(node, "clearance", path)?,
        thermal_width: match opt_len(node, "thermal_bridge_width", path)? {
            Some(w) => Some(w),
            None => opt_len(node, "thermal_width", path)?,
        },
        thermal_gap: opt_len(node, "thermal_gap", path)?,
        thermal_angle: opt_num(node, "thermal_bridge_angle", path)?,
    })
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Footprint {
    /// `library:name` as written.
    pub lib_id: String,
    pub back: bool,
    pub at: PointNm,
    pub angle: f64,
    pub reference: String,
    pub margins: Margins,
    pub pads: Vec<KPad>,
    pub net_tie: bool,
    /// Footprint drawings on copper, mask or paste layers.
    pub fab_graphics: usize,
    /// Other drawings and texts (silkscreen, courtyard, fabrication).
    pub drawings: usize,
    pub models: usize,
    /// Drawings on `Edge.Cuts` (not imported).
    pub edge_cuts: usize,
    /// Zones and rule areas inside the footprint (KiCad writes their
    /// outlines in board coordinates).
    pub zones: Vec<crate::items::KZone>,
    /// Zone connection overrides for every pad of the footprint.
    pub zone: ZoneOverride,
    /// KiCad's "not in schematic" attribute: a part on the board only.
    pub board_only: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PadType {
    #[default]
    Smd,
    ThroughHole,
    NpThroughHole,
    Connect,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PadShapeK {
    #[default]
    Rect,
    RoundRect,
    Circle,
    Oval,
    Trapezoid,
    Custom,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Drill {
    pub oval: bool,
    /// Width and height (equal for a round drill).
    pub size: PointNm,
    pub offset: PointNm,
}

/// The four corners a chamfer may cut, in the pad frame as KiCad names
/// them (top is toward −y in KiCad's frame).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Chamfer {
    pub top_left: bool,
    pub top_right: bool,
    pub bottom_left: bool,
    pub bottom_right: bool,
}

/// A custom pad's drawn primitive, pad frame.
#[derive(Clone, Debug, PartialEq)]
pub enum Primitive {
    Poly {
        points: Vec<PointNm>,
        width: Nm,
    },
    Circle {
        center: PointNm,
        radius: f64,
        width: Nm,
        fill: bool,
    },
    Rect {
        start: PointNm,
        end: PointNm,
        width: Nm,
        fill: bool,
    },
    Line {
        start: PointNm,
        end: PointNm,
        width: Nm,
    },
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct KPad {
    pub number: String,
    pub kind: PadType,
    pub shape: PadShapeK,
    /// Footprint-frame position (unrotated by the footprint).
    pub at: PointNm,
    /// The pad's orientation on the board (the footprint's included).
    pub angle: f64,
    pub size: PointNm,
    pub drill: Option<Drill>,
    pub layers: Vec<String>,
    pub rratio: f64,
    pub chamfer_ratio: f64,
    pub chamfer: Chamfer,
    pub rect_delta: PointNm,
    pub margins: Margins,
    /// A custom pad's anchor is a circle (else a rectangle).
    pub anchor_circle: bool,
    pub primitives: Vec<Primitive>,
    /// Primitives this importer cannot draw (arcs, curves).
    pub unsupported_primitives: usize,
    pub fiducial: bool,
    /// The pad's net number and name (`None`: no net).
    pub net: Option<(u32, String)>,
    /// Per-pad zone connection, clearance and thermal overrides.
    pub zone: ZoneOverride,
    /// Tokens this importer does not know, by path.
    pub unknown: Vec<String>,
}

impl KPad {
    /// Whether the pad is on a layer: an exact name, a `*.Cu`-style
    /// wildcard matching the suffix, or `F&B.Cu` for either outer side.
    pub fn on(&self, layer: &str) -> bool {
        let (side, suffix) = layer.split_once('.').unwrap_or((layer, ""));
        self.layers.iter().any(|l| {
            l == layer
                || l.strip_prefix("*.") == Some(suffix)
                || (l.strip_prefix("F&B.") == Some(suffix) && matches!(side, "F" | "B"))
        })
    }
}

/// Read a board from its file bytes.
pub fn read(bytes: &[u8], report: &mut Report) -> Result<Board> {
    let root = crate::sexpr::parse(bytes).map_err(BoardError::Syntax)?;
    if root.head() != Some("kicad_pcb") {
        return Err(BoardError::NotABoard);
    }
    let version = root
        .child("version")
        .and_then(|v| v.arg(0))
        .and_then(|v| v.parse::<u32>().ok())
        .ok_or(BoardError::Value("kicad_pcb/version".into()))?;
    if !VERSIONS.contains(&version) {
        return Err(BoardError::Version(version));
    }
    let mut board = Board {
        version,
        ..Board::default()
    };
    if let Some(layers) = root.child("layers") {
        let mut inner = Vec::new();
        let (mut front, mut back) = (None, None);
        for layer in layers.args() {
            let Some(name) = layer.arg(0) else { continue };
            match name {
                "F.Cu" => front = Some(name.to_owned()),
                "B.Cu" => back = Some(name.to_owned()),
                n if n.starts_with("In") && n.ends_with(".Cu") => inner.push(n.to_owned()),
                _ => {}
            }
        }
        board.copper.extend(front);
        board.copper.extend(inner);
        board.copper.extend(back);
    }
    if let Some(setup) = root.child("setup") {
        board.setup.mask_margin = opt_len(setup, "pad_to_mask_clearance", "setup")?.unwrap_or(0);
        board.setup.paste_margin = opt_len(setup, "pad_to_paste_clearance", "setup")?.unwrap_or(0);
        board.setup.paste_ratio =
            opt_num(setup, "pad_to_paste_clearance_ratio", "setup")?.unwrap_or(0.0);
        if let Some(origin) = setup.child("aux_axis_origin") {
            board.aux_origin = Some(point(origin, 0, "setup/aux_axis_origin")?);
        }
        board.setup.vias_on_mask = setup
            .child("pcbplotparams")
            .and_then(|p| p.child("viasonmask"))
            .and_then(|v| v.arg(0))
            .map(|v| matches!(v, "yes" | "true"));
        if let Some(stackup) = setup.child("stackup") {
            board.items.stackup = crate::items::stackup(stackup)?;
        }
    }
    for item in root.args() {
        let Some(head) = item.head() else { continue };
        match head {
            "footprint" => {
                if board.footprints.len() >= MAX_FOOTPRINTS {
                    return Err(BoardError::Limit("more than 4096 footprints"));
                }
                board.footprints.push(footprint(item, report)?);
            }
            "net" => {
                let (n, name) = crate::items::net(item)?;
                board.items.nets.insert(n, name);
            }
            "segment" | "arc" => {
                if board.items.tracks.len() >= crate::items::MAX_TRACKS {
                    return Err(BoardError::Limit("too many tracks"));
                }
                board.items.tracks.push(crate::items::track(item)?);
            }
            "via" => {
                if board.items.vias.len() >= crate::items::MAX_VIAS {
                    return Err(BoardError::Limit("too many vias"));
                }
                board.items.vias.push(crate::items::via(item)?);
            }
            "zone" => {
                if board.items.zones.len() >= crate::items::MAX_ZONES {
                    return Err(BoardError::Limit("too many zones"));
                }
                board.items.zones.push(crate::items::zone(item)?);
            }
            h if h.starts_with("gr_") && crate::items::on_edge(item) => {
                if board.items.edges.len() >= crate::items::MAX_EDGE_ITEMS {
                    return Err(BoardError::Limit("too many outline drawings"));
                }
                match crate::items::edge(item)? {
                    Some(edge) => board.items.edges.push(edge),
                    None => board.items.edge_curves += 1,
                }
            }
            "version" | "generator" | "generator_version" | "general" | "paper" | "title_block"
            | "layers" | "setup" | "property" | "net_class" | "embedded_fonts"
            | "embedded_files" => {}
            other => *board.other.entry(other.to_owned()).or_default() += 1,
        }
    }
    board.items.footprint_edges = board.footprints.iter().map(|f| f.edge_cuts).sum();
    let footprint_zones: Vec<crate::items::KZone> = board
        .footprints
        .iter()
        .flat_map(|f| f.zones.iter().cloned())
        .collect();
    if board.items.zones.len() + footprint_zones.len() > crate::items::MAX_ZONES {
        return Err(BoardError::Limit("too many zones"));
    }
    board.items.zones.extend(footprint_zones);
    board.origin = match crate::items::edge_bounds(&board.items.edges) {
        // KiCad's y grows downward: the lower-left corner has the
        // largest y. Whole micrometres, outward, so coordinates written to
        // the micrometre (drill files) round as KiCad's do.
        Some([min, max]) => [
            min[0].div_euclid(1000) * 1000,
            -((-max[1]).div_euclid(1000) * 1000),
        ],
        None => board.aux_origin.unwrap_or_default(),
    };
    Ok(board)
}

/// A decimal millimetre length as whole nanometres (exact for up to six
/// decimals, otherwise rounded half away from zero).
pub fn length(text: &str, path: &str) -> Result<Nm> {
    if !text
        .bytes()
        .all(|b| b.is_ascii_digit() || matches!(b, b'.' | b'-' | b'+' | b'e' | b'E'))
    {
        return Err(BoardError::Value(path.into()));
    }
    parse_length(text, LengthUnit::Mm)
        .map(|n| n.get())
        .map_err(|_| BoardError::Value(path.into()))
}

fn number(text: &str, path: &str) -> Result<f64> {
    text.parse::<f64>()
        .ok()
        .filter(|v| v.is_finite())
        .ok_or_else(|| BoardError::Value(path.into()))
}

fn arg_len(node: &Node, index: usize, path: &str) -> Result<Nm> {
    length(
        node.arg(index)
            .ok_or_else(|| BoardError::Value(path.into()))?,
        path,
    )
}

fn point(node: &Node, first: usize, path: &str) -> Result<PointNm> {
    Ok([arg_len(node, first, path)?, arg_len(node, first + 1, path)?])
}

fn opt_len(node: &Node, head: &str, path: &str) -> Result<Option<Nm>> {
    node.child(head)
        .map(|c| arg_len(c, 0, &format!("{path}/{head}")))
        .transpose()
}

fn opt_num(node: &Node, head: &str, path: &str) -> Result<Option<f64>> {
    node.child(head)
        .map(|c| {
            let p = format!("{path}/{head}");
            number(c.arg(0).ok_or_else(|| BoardError::Value(p.clone()))?, &p)
        })
        .transpose()
}

/// `(at x y [angle])`.
fn at(node: &Node, path: &str) -> Result<(PointNm, f64)> {
    let at = node
        .child("at")
        .ok_or_else(|| BoardError::Value(format!("{path}/at")))?;
    let p = point(at, 0, &format!("{path}/at"))?;
    let angle = match at.arg(2) {
        Some(a) => number(a, &format!("{path}/at"))?,
        None => 0.0,
    };
    Ok((p, angle))
}

fn margins(node: &Node, path: &str) -> Result<Margins> {
    Ok(Margins {
        mask: opt_len(node, "solder_mask_margin", path)?,
        paste: opt_len(node, "solder_paste_margin", path)?,
        paste_ratio: match opt_num(node, "solder_paste_margin_ratio", path)? {
            Some(r) => Some(r),
            None => opt_num(node, "solder_paste_ratio", path)?,
        },
    })
}

/// Drawings on these layers make copper, mask or paste.
fn fab_layer(layer: &str) -> bool {
    layer.ends_with(".Cu") || layer.ends_with(".Mask") || layer.ends_with(".Paste")
}

fn footprint(node: &Node, report: &mut Report) -> Result<Footprint> {
    let path = "footprint";
    let (at_point, angle) = at(node, path)?;
    let mut fp = Footprint {
        lib_id: node.arg(0).unwrap_or_default().to_owned(),
        back: node.child("layer").and_then(|l| l.arg(0)) == Some("B.Cu"),
        at: at_point,
        angle,
        margins: margins(node, path)?,
        zone: zone_override(node, path)?,
        ..Footprint::default()
    };
    for item in node.args() {
        let Some(head) = item.head() else { continue };
        match head {
            "pad" => {
                if fp.pads.len() >= MAX_PADS {
                    return Err(BoardError::Limit("a footprint with more than 4096 pads"));
                }
                fp.pads.push(pad(item)?);
            }
            "property" if item.arg(0) == Some("Reference") => {
                fp.reference = item.arg(1).unwrap_or_default().to_owned();
            }
            "fp_text" if item.arg(0) == Some("reference") => {
                fp.reference = item.arg(1).unwrap_or_default().to_owned();
                fp.drawings += 1;
            }
            "fp_line" | "fp_rect" | "fp_circle" | "fp_arc" | "fp_poly" | "fp_curve" => {
                let layer = item
                    .child("layer")
                    .and_then(|l| l.arg(0))
                    .unwrap_or_default();
                if layer == "Edge.Cuts" {
                    fp.edge_cuts += 1;
                } else if fab_layer(layer) {
                    fp.fab_graphics += 1;
                } else {
                    fp.drawings += 1;
                }
            }
            "fp_text" | "fp_text_box" | "dimension" => fp.drawings += 1,
            "model" => fp.models += 1,
            "zone" => {
                if fp.zones.len() >= crate::items::MAX_ZONES {
                    return Err(BoardError::Limit("too many zones in a footprint"));
                }
                fp.zones.push(crate::items::zone(item)?);
            }
            "net_tie_pad_groups" => fp.net_tie = true,
            "attr" => {
                let has = |flag: &str| item.args().iter().any(|a| a.atom() == Some(flag));
                if has("net_tie") {
                    fp.net_tie = true;
                }
                if has("board_only") {
                    fp.board_only = true;
                }
            }
            "layer"
            | "uuid"
            | "tstamp"
            | "at"
            | "descr"
            | "tags"
            | "property"
            | "path"
            | "sheetname"
            | "sheetfile"
            | "solder_mask_margin"
            | "solder_paste_margin"
            | "solder_paste_margin_ratio"
            | "solder_paste_ratio"
            | "clearance"
            | "zone_connect"
            | "thermal_width"
            | "thermal_gap"
            | "locked"
            | "placed"
            | "autoplace_cost90"
            | "autoplace_cost180"
            | "group"
            | "private_layers"
            | "embedded_fonts"
            | "embedded_files"
            | "version"
            | "generator"
            | "generator_version" => {}
            other => report.unknown(&format!("footprint/{other}")),
        }
    }
    Ok(fp)
}

fn pad(node: &Node) -> Result<KPad> {
    let path = "footprint/pad";
    let kind = match node.arg(1) {
        Some("smd") => PadType::Smd,
        Some("thru_hole") => PadType::ThroughHole,
        Some("np_thru_hole") => PadType::NpThroughHole,
        Some("connect") => PadType::Connect,
        _ => return Err(BoardError::Value(format!("{path}/type"))),
    };
    let shape = match node.arg(2) {
        Some("rect") => PadShapeK::Rect,
        Some("roundrect") => PadShapeK::RoundRect,
        Some("circle") => PadShapeK::Circle,
        Some("oval") => PadShapeK::Oval,
        Some("trapezoid") => PadShapeK::Trapezoid,
        Some("custom") => PadShapeK::Custom,
        _ => return Err(BoardError::Value(format!("{path}/shape"))),
    };
    let (at_point, angle) = at(node, path)?;
    let size = node
        .child("size")
        .map(|s| point(s, 0, &format!("{path}/size")))
        .transpose()?
        .unwrap_or_default();
    let mut pad = KPad {
        number: node.arg(0).unwrap_or_default().to_owned(),
        kind,
        shape,
        at: at_point,
        angle,
        size,
        margins: margins(node, path)?,
        rratio: opt_num(node, "roundrect_rratio", path)?.unwrap_or(0.0),
        chamfer_ratio: opt_num(node, "chamfer_ratio", path)?.unwrap_or(0.0),
        zone: zone_override(node, path)?,
        ..KPad::default()
    };
    for item in node.args() {
        let Some(head) = item.head() else { continue };
        match head {
            "drill" => pad.drill = Some(drill(item)?),
            "layers" => {
                pad.layers = item
                    .args()
                    .iter()
                    .filter_map(Node::atom)
                    .map(str::to_owned)
                    .collect();
            }
            "rect_delta" => pad.rect_delta = point(item, 0, &format!("{path}/rect_delta"))?,
            "chamfer" => {
                for corner in item.args().iter().filter_map(Node::atom) {
                    match corner {
                        "top_left" => pad.chamfer.top_left = true,
                        "top_right" => pad.chamfer.top_right = true,
                        "bottom_left" => pad.chamfer.bottom_left = true,
                        "bottom_right" => pad.chamfer.bottom_right = true,
                        _ => pad.unknown.push(format!("{path}/chamfer/{corner}")),
                    }
                }
            }
            "options" => {
                pad.anchor_circle = item.child("anchor").and_then(|a| a.arg(0)) == Some("circle");
            }
            "primitives" => primitives(item, &mut pad)?,
            "property" => {
                pad.fiducial = matches!(
                    item.arg(0),
                    Some("pad_prop_fiducial_glob" | "pad_prop_fiducial_loc")
                );
            }
            "zone_connect"
            | "clearance"
            | "thermal_width"
            | "thermal_gap"
            | "thermal_bridge_width"
            | "thermal_bridge_angle" => {}
            "net" => {
                let n = item.arg(0).and_then(|a| a.parse::<u32>().ok());
                pad.net = n
                    .filter(|n| *n != 0)
                    .map(|n| (n, item.arg(1).unwrap_or_default().to_owned()));
            }
            "at"
            | "size"
            | "pinfunction"
            | "pintype"
            | "die_length"
            | "uuid"
            | "tstamp"
            | "roundrect_rratio"
            | "chamfer_ratio"
            | "solder_mask_margin"
            | "solder_paste_margin"
            | "solder_paste_margin_ratio"
            | "solder_paste_ratio"
            | "remove_unused_layers"
            | "keep_end_layers"
            | "locked"
            | "teardrops"
            | "zone_layer_connections" => {}
            other => pad.unknown.push(format!("{path}/{other}")),
        }
    }
    Ok(pad)
}

/// `(drill [oval] d [h] [(offset x y)])`.
fn drill(node: &Node) -> Result<Drill> {
    let path = "footprint/pad/drill";
    let atoms: Vec<&str> = node.args().iter().filter_map(Node::atom).collect();
    let oval = atoms.first() == Some(&"oval");
    let values: Vec<Nm> = atoms
        .iter()
        .skip(usize::from(oval))
        .map(|a| length(a, path))
        .collect::<Result<_>>()?;
    let w = *values.first().unwrap_or(&0);
    let h = *values.get(1).unwrap_or(&w);
    let offset = node
        .child("offset")
        .map(|o| point(o, 0, &format!("{path}/offset")))
        .transpose()?
        .unwrap_or_default();
    Ok(Drill {
        oval,
        size: [w, h],
        offset,
    })
}

fn width_of(node: &Node, path: &str) -> Result<Nm> {
    if let Some(w) = node.child("width") {
        return arg_len(w, 0, path);
    }
    if let Some(w) = node.child("stroke").and_then(|s| s.child("width")) {
        return arg_len(w, 0, path);
    }
    Ok(0)
}

fn filled(node: &Node) -> bool {
    matches!(
        node.child("fill").and_then(|f| f.arg(0)),
        Some("yes" | "solid")
    )
}

fn primitives(node: &Node, pad: &mut KPad) -> Result<()> {
    let path = "footprint/pad/primitives";
    for item in node.args() {
        let Some(head) = item.head() else { continue };
        let p = format!("{path}/{head}");
        match head {
            "gr_poly" => {
                let pts = item
                    .child("pts")
                    .ok_or_else(|| BoardError::Value(p.clone()))?;
                let mut points = Vec::new();
                for xy in pts.args() {
                    if xy.head() != Some("xy") {
                        pad.unsupported_primitives += 1;
                        continue;
                    }
                    if points.len() >= MAX_POLY_POINTS {
                        return Err(BoardError::Limit("a pad polygon of more than 4096 points"));
                    }
                    points.push(point(xy, 0, &p)?);
                }
                pad.primitives.push(Primitive::Poly {
                    points,
                    width: width_of(item, &p)?,
                });
            }
            "gr_circle" => {
                let center = point(
                    item.child("center")
                        .ok_or_else(|| BoardError::Value(p.clone()))?,
                    0,
                    &p,
                )?;
                let end = point(
                    item.child("end")
                        .ok_or_else(|| BoardError::Value(p.clone()))?,
                    0,
                    &p,
                )?;
                let radius = ((end[0] - center[0]) as f64).hypot((end[1] - center[1]) as f64);
                pad.primitives.push(Primitive::Circle {
                    center,
                    radius,
                    width: width_of(item, &p)?,
                    fill: filled(item),
                });
            }
            "gr_rect" => {
                let get = |h: &str| -> Result<PointNm> {
                    point(
                        item.child(h).ok_or_else(|| BoardError::Value(p.clone()))?,
                        0,
                        &p,
                    )
                };
                pad.primitives.push(Primitive::Rect {
                    start: get("start")?,
                    end: get("end")?,
                    width: width_of(item, &p)?,
                    fill: filled(item),
                });
            }
            "gr_line" => {
                let get = |h: &str| -> Result<PointNm> {
                    point(
                        item.child(h).ok_or_else(|| BoardError::Value(p.clone()))?,
                        0,
                        &p,
                    )
                };
                pad.primitives.push(Primitive::Line {
                    start: get("start")?,
                    end: get("end")?,
                    width: width_of(item, &p)?,
                });
            }
            _ => pad.unsupported_primitives += 1,
        }
    }
    Ok(())
}
