//! KiCad schematics (`.kicad_sch`, slice K5): the sheet hierarchy, each
//! symbol's place and pins, wires, junctions, labels, power symbols,
//! no-connect flags and sheet symbols, read for facts only from KiCad's
//! published file-format documentation and confirmed against `kicad-cli`
//! output, never from KiCad's source.
//!
//! **Frames.** Sheets are in millimetres with y down from the page's
//! top-left corner; a library symbol's pins are in its own frame with y
//! up. A pin's place on the sheet is the symbol's `at` plus the pin's
//! `at`, y flipped, mirrored (`mirror x` flips y, `mirror y` flips x) and
//! then turned counter-clockwise on screen by the symbol's angle
//! ([`pin_on_sheet`]). Our sheets have y up with the page's top-left
//! corner at `PAGE_TOP_LEFT`; [`ours`] converts.
//!
//! **Connectivity.** Wires join where their ends meet, where an end,
//! junction, pin or label lies on another wire, and where pins and labels
//! meet; crossing wires do not join. Nothing else of KiCad's connectivity
//! rules is assumed.
use crate::board::{Nm, PointNm};
use crate::sexpr::{self, Node};
use std::collections::BTreeMap;

/// Most sheet instances in one hierarchy.
pub const MAX_SHEETS: usize = 512;
/// Deepest hierarchy.
pub const MAX_DEPTH: usize = 32;
/// Most wire segments on one sheet file.
pub const MAX_WIRES: usize = 50_000;

/// Why a schematic was not read. Messages carry no file content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchematicError {
    Syntax(sexpr::ParseError),
    NotASchematic,
    /// A sheet file the hierarchy names is missing.
    Missing,
    Limit(&'static str),
    Value(&'static str),
}

impl std::fmt::Display for SchematicError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Syntax(e) => write!(f, "kicad.schematic_syntax: {e}"),
            Self::NotASchematic => write!(f, "kicad.not_a_schematic"),
            Self::Missing => write!(f, "kicad.schematic_missing: a sheet file is missing"),
            Self::Limit(what) => write!(f, "kicad.schematic_limit: {what}"),
            Self::Value(what) => write!(f, "kicad.schematic_value: {what}"),
        }
    }
}

impl std::error::Error for SchematicError {}

type Result<T> = std::result::Result<T, SchematicError>;

/// The schematic files of a project: the root's name and every file's
/// bytes by the name sheets use for it (`Sheetfile`).
#[derive(Clone, Copy)]
pub struct SchematicFiles<'a> {
    pub root: &'a str,
    pub files: &'a BTreeMap<String, Vec<u8>>,
}

/// One pin of a library symbol: its number, connection point (library
/// frame, y up) and unit (0: every unit).
#[derive(Clone, Debug, PartialEq)]
pub struct LibPin {
    pub number: String,
    pub at: PointNm,
    pub unit: u32,
    /// The direction from its connection point to the body, degrees.
    pub angle: f64,
    pub length: Nm,
    /// The pin's name.
    pub name: String,
    /// A hidden power input: KiCad joins it to the global net of its name.
    pub hidden_power: bool,
    /// Of type "not connected": it joins nothing, even where it meets
    /// other pins.
    pub no_connect: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct LibSymbol {
    pub power: bool,
    pub pins: Vec<LibPin>,
    /// Body rectangles `(unit, corner, corner)`, library frame.
    pub rects: Vec<(u32, PointNm, PointNm)>,
    /// The symbol it derives from (`extends`), whose pins it has.
    pub extends: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct KSymbol {
    pub lib_id: String,
    pub at: PointNm,
    pub angle: f64,
    pub mirror_x: bool,
    pub mirror_y: bool,
    pub unit: u32,
    pub reference: String,
    pub value: String,
    /// `(path, reference, unit)` per sheet instance.
    pub instances: Vec<(String, String, u32)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KLabelKind {
    Local,
    Global,
    Hierarchical,
}

#[derive(Clone, Debug, PartialEq)]
pub struct KLabel {
    pub kind: KLabelKind,
    pub name: String,
    pub at: PointNm,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct KSheetSymbol {
    pub at: PointNm,
    pub size: PointNm,
    pub name: String,
    pub file: String,
    pub uuid: String,
    pub pins: Vec<(String, PointNm)>,
}

/// One sheet file, read.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SheetFile {
    pub uuid: String,
    pub paper: String,
    pub portrait: bool,
    pub lib: BTreeMap<String, LibSymbol>,
    pub symbols: Vec<KSymbol>,
    pub wires: Vec<[PointNm; 2]>,
    pub junctions: Vec<PointNm>,
    pub no_connects: Vec<PointNm>,
    pub labels: Vec<KLabel>,
    pub sheets: Vec<KSheetSymbol>,
    /// Bus wires, bus entries and other items not imported, counted.
    pub skipped: usize,
}

/// One sheet of the flattened hierarchy: a file shown once per sheet
/// symbol that uses it.
#[derive(Clone, Debug, PartialEq)]
pub struct FlatSheet {
    /// The file's name.
    pub file: String,
    pub name: String,
    /// The instance path of its symbols: the sheet symbols' uuids from
    /// the root's, `/`-separated.
    pub path: String,
    /// The same path without the root's uuid (older files use it).
    pub short_path: String,
    /// The parent sheet and the index of its sheet symbol there.
    pub parent: Option<(usize, usize)>,
}

/// The schematic: its files and its flattened sheets (the root first).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Schematic {
    pub files: BTreeMap<String, SheetFile>,
    pub sheets: Vec<FlatSheet>,
}

fn nm(text: Option<&str>, what: &'static str) -> Result<Nm> {
    let v: f64 = text
        .and_then(|t| t.parse().ok())
        .filter(|v: &f64| v.is_finite() && v.abs() < 1e6)
        .ok_or(SchematicError::Value(what))?;
    Ok((v * 1e6).round() as Nm)
}

fn point(node: &Node, first: usize, what: &'static str) -> Result<PointNm> {
    Ok([nm(node.arg(first), what)?, nm(node.arg(first + 1), what)?])
}

fn at(node: &Node, what: &'static str) -> Result<(PointNm, f64)> {
    let at = node.child("at").ok_or(SchematicError::Value(what))?;
    let angle = at.arg(2).and_then(|a| a.parse().ok()).unwrap_or(0.0);
    Ok((point(at, 0, what)?, angle))
}

fn property<'a>(node: &'a Node, name: &str) -> Option<&'a str> {
    node.children("property")
        .find(|p| p.arg(0) == Some(name))
        .and_then(|p| p.arg(1))
}

/// The unit and body style of a library sub-symbol named `NAME_U_S`.
fn unit_of(name: &str) -> (u32, u32) {
    let mut parts = name.rsplitn(3, '_');
    let style = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let unit = parts.next().and_then(|u| u.parse().ok()).unwrap_or(0);
    (unit, style)
}

fn lib_symbol(node: &Node) -> Result<LibSymbol> {
    let mut out = LibSymbol {
        power: node.child("power").is_some(),
        extends: node
            .child("extends")
            .and_then(|e| e.arg(0))
            .map(str::to_owned),
        ..LibSymbol::default()
    };
    let mut pins = |unit: u32, n: &Node| -> Result<()> {
        for pin in n.children("pin") {
            let (p, angle) = at(pin, "lib_symbols/pin/at")?;
            let number = pin
                .child("number")
                .and_then(|x| x.arg(0))
                .unwrap_or_default()
                .to_owned();
            let length = match pin.child("length") {
                Some(l) => nm(l.arg(0), "lib_symbols/pin/length")?,
                None => 0,
            };
            let hidden = pin.args().iter().any(|a| a.atom() == Some("hide"))
                || pin.child("hide").and_then(|h| h.arg(0)) == Some("yes");
            out.pins.push(LibPin {
                number,
                at: p,
                unit,
                angle,
                length,
                name: pin
                    .child("name")
                    .and_then(|x| x.arg(0))
                    .unwrap_or_default()
                    .to_owned(),
                hidden_power: hidden && pin.arg(0) == Some("power_in"),
                no_connect: pin.arg(0) == Some("no_connect"),
            });
        }
        for r in n.children("rectangle") {
            if let (Some(a), Some(b)) = (r.child("start"), r.child("end")) {
                out.rects
                    .push((unit, point(a, 0, "rectangle")?, point(b, 0, "rectangle")?));
            }
        }
        Ok(())
    };
    pins(0, node)?;
    // The first body style only: an alternate (De Morgan) body's pins are
    // the same pins drawn elsewhere.
    for sub in node.children("symbol") {
        let (unit, style) = unit_of(sub.arg(0).unwrap_or_default());
        if style <= 1 {
            pins(unit, sub)?;
        }
    }
    Ok(out)
}

fn symbol(node: &Node) -> Result<KSymbol> {
    let (p, angle) = at(node, "symbol/at")?;
    let mirror = node.child("mirror").and_then(|m| m.arg(0));
    let mut instances = Vec::new();
    if let Some(inst) = node.child("instances") {
        for project in inst.children("project") {
            for path in project.children("path") {
                instances.push((
                    path.arg(0).unwrap_or_default().to_owned(),
                    path.child("reference")
                        .and_then(|r| r.arg(0))
                        .unwrap_or_default()
                        .to_owned(),
                    path.child("unit")
                        .and_then(|u| u.arg(0))
                        .and_then(|u| u.parse().ok())
                        .unwrap_or(1),
                ));
            }
        }
    }
    Ok(KSymbol {
        lib_id: node
            .child("lib_id")
            .and_then(|l| l.arg(0))
            .unwrap_or_default()
            .to_owned(),
        at: p,
        angle,
        mirror_x: mirror == Some("x"),
        mirror_y: mirror == Some("y"),
        unit: node
            .child("unit")
            .and_then(|u| u.arg(0))
            .and_then(|u| u.parse().ok())
            .unwrap_or(1),
        reference: property(node, "Reference").unwrap_or_default().to_owned(),
        value: property(node, "Value").unwrap_or_default().to_owned(),
        instances,
    })
}

/// Read one sheet file.
pub fn read_file(bytes: &[u8]) -> Result<SheetFile> {
    let root = sexpr::parse(bytes).map_err(SchematicError::Syntax)?;
    if root.head() != Some("kicad_sch") {
        return Err(SchematicError::NotASchematic);
    }
    let mut out = SheetFile {
        uuid: root
            .child("uuid")
            .and_then(|u| u.arg(0))
            .unwrap_or_default()
            .to_owned(),
        ..SheetFile::default()
    };
    if let Some(paper) = root.child("paper") {
        out.paper = paper.arg(0).unwrap_or_default().to_owned();
        out.portrait = paper.args().iter().any(|a| a.atom() == Some("portrait"));
    }
    if let Some(lib) = root.child("lib_symbols") {
        for s in lib.children("symbol") {
            out.lib
                .insert(s.arg(0).unwrap_or_default().to_owned(), lib_symbol(s)?);
        }
    }
    for item in root.args() {
        match item.head() {
            Some("symbol") => out.symbols.push(symbol(item)?),
            Some("wire") => {
                let pts: Vec<&Node> = item
                    .child("pts")
                    .map(|p| p.children("xy").collect())
                    .unwrap_or_default();
                for w in pts.windows(2) {
                    let (a, b) = (point(w[0], 0, "wire/xy")?, point(w[1], 0, "wire/xy")?);
                    if a != b {
                        out.wires.push([a, b]);
                    }
                }
                if out.wires.len() > MAX_WIRES {
                    return Err(SchematicError::Limit("too many wires on one sheet"));
                }
            }
            Some("junction") => out.junctions.push(at(item, "junction/at")?.0),
            Some("no_connect") => out.no_connects.push(at(item, "no_connect/at")?.0),
            Some(kind @ ("label" | "global_label" | "hierarchical_label")) => {
                out.labels.push(KLabel {
                    kind: match kind {
                        "label" => KLabelKind::Local,
                        "global_label" => KLabelKind::Global,
                        _ => KLabelKind::Hierarchical,
                    },
                    name: item.arg(0).unwrap_or_default().to_owned(),
                    at: at(item, "label/at")?.0,
                });
            }
            Some("sheet") => {
                let (p, _) = at(item, "sheet/at")?;
                let size = item
                    .child("size")
                    .map(|s| point(s, 0, "sheet/size"))
                    .transpose()?
                    .unwrap_or_default();
                let mut pins = Vec::new();
                for pin in item.children("pin") {
                    pins.push((
                        pin.arg(0).unwrap_or_default().to_owned(),
                        at(pin, "sheet/pin/at")?.0,
                    ));
                }
                out.sheets.push(KSheetSymbol {
                    at: p,
                    size,
                    name: property(item, "Sheetname")
                        .or_else(|| property(item, "Sheet name"))
                        .unwrap_or_default()
                        .to_owned(),
                    file: property(item, "Sheetfile")
                        .or_else(|| property(item, "Sheet file"))
                        .unwrap_or_default()
                        .to_owned(),
                    uuid: item
                        .child("uuid")
                        .and_then(|u| u.arg(0))
                        .unwrap_or_default()
                        .to_owned(),
                    pins,
                });
            }
            Some("bus" | "bus_entry" | "bus_alias" | "netclass_flag" | "directive_label") => {
                out.skipped += 1;
            }
            _ => {}
        }
    }
    Ok(out)
}

/// Read a schematic's hierarchy from its root file.
pub fn read(files: SchematicFiles<'_>) -> Result<Schematic> {
    /// A sheet instance still to read: its file, name, paths, parent
    /// (sheet and sheet-symbol index) and depth.
    type Pending = (
        String,
        String,
        String,
        String,
        Option<(usize, usize)>,
        usize,
    );
    let mut out = Schematic::default();
    let mut stack: Vec<Pending> = vec![(
        files.root.to_owned(),
        String::new(),
        String::new(),
        String::new(),
        None,
        0,
    )];
    while let Some((file, name, path, short, parent, depth)) = stack.pop() {
        if depth > MAX_DEPTH {
            return Err(SchematicError::Limit("a hierarchy deeper than 32 sheets"));
        }
        if out.sheets.len() >= MAX_SHEETS {
            return Err(SchematicError::Limit("more than 512 sheets"));
        }
        if !out.files.contains_key(&file) {
            let bytes = files.files.get(&file).ok_or(SchematicError::Missing)?;
            out.files.insert(file.clone(), read_file(bytes)?);
        }
        let read = out.files.get(&file).ok_or(SchematicError::Missing)?;
        let path = if parent.is_none() {
            format!("/{}", read.uuid)
        } else {
            path
        };
        let index = out.sheets.len();
        let children: Vec<(usize, KSheetSymbol)> =
            read.sheets.iter().cloned().enumerate().collect();
        out.sheets.push(FlatSheet {
            file,
            name,
            path: path.clone(),
            short_path: short.clone(),
            parent,
        });
        // Children in reverse, so they are read in file order.
        for (k, child) in children.into_iter().rev() {
            stack.push((
                child.file.clone(),
                child.name.clone(),
                format!("{path}/{}", child.uuid),
                format!("{short}/{}", child.uuid),
                Some((index, k)),
                depth + 1,
            ));
        }
    }
    Ok(out)
}

impl Schematic {
    /// A symbol's reference and unit on `sheet`: its instance for the
    /// sheet's path, else its own Reference property.
    pub fn reference(&self, sheet: &FlatSheet, symbol: &KSymbol) -> (String, u32) {
        let path = |p: &str| p.trim_end_matches('/').to_owned();
        let want = [path(&sheet.path), path(&sheet.short_path)];
        symbol
            .instances
            .iter()
            .find(|(p, _, _)| {
                let p = path(p);
                want.contains(&p) || (p.is_empty() && want[1].is_empty())
            })
            .map(|(_, r, u)| (r.clone(), *u))
            .unwrap_or_else(|| (symbol.reference.clone(), symbol.unit))
    }

    /// The library symbol `symbol` uses, its pins those of what it
    /// extends.
    pub fn lib<'a>(
        &self,
        file: &'a SheetFile,
        symbol: &KSymbol,
    ) -> Option<(bool, Vec<&'a LibPin>)> {
        let (power, base) = self.library(file, symbol)?;
        Some((power, base.pins.iter().collect()))
    }

    /// The library symbol with the pins and body: its own, or what it
    /// extends.
    pub fn library<'a>(
        &self,
        file: &'a SheetFile,
        symbol: &KSymbol,
    ) -> Option<(bool, &'a LibSymbol)> {
        let own = file.lib.get(&symbol.lib_id)?;
        if own.pins.is_empty()
            && let Some(base) = own.extends.as_ref().and_then(|b| {
                let prefix = symbol.lib_id.split_once(':').map_or("", |(l, _)| l);
                file.lib
                    .get(&format!("{prefix}:{b}"))
                    .or_else(|| file.lib.get(b))
            })
        {
            return Some((own.power, base));
        }
        Some((own.power, own))
    }
}

/// A library pin's point on the sheet (KiCad frame, y down).
pub fn pin_on_sheet(symbol: &KSymbol, pin: PointNm) -> PointNm {
    // Library y is up: flip it into the sheet's y-down frame.
    let (mut x, mut y) = (pin[0] as f64, -(pin[1] as f64));
    if symbol.mirror_x {
        y = -y;
    }
    if symbol.mirror_y {
        x = -x;
    }
    // Counter-clockwise on screen, with y down.
    let (s, c) = symbol.angle.to_radians().sin_cos();
    let (rx, ry) = (x * c + y * s, -x * s + y * c);
    [
        symbol.at[0] + rx.round() as Nm,
        symbol.at[1] + ry.round() as Nm,
    ]
}

/// A KiCad sheet point in our sheet frame, mm.
pub fn ours(p: PointNm) -> [f64; 2] {
    let top_left = sparksmith_core::model::sheet::PAGE_TOP_LEFT;
    [
        top_left[0] + p[0] as f64 / 1e6,
        top_left[1] - p[1] as f64 / 1e6,
    ]
}

/// Whether `p` lies on segment `a`–`b` strictly between its ends
/// (exactly, on the nanometre grid).
pub fn inside(p: PointNm, a: PointNm, b: PointNm) -> bool {
    let (dx, dy) = ((b[0] - a[0]) as i128, (b[1] - a[1]) as i128);
    let (px, py) = ((p[0] - a[0]) as i128, (p[1] - a[1]) as i128);
    if dx * py - dy * px != 0 {
        return false;
    }
    let dot = px * dx + py * dy;
    dot > 0 && dot < dx * dx + dy * dy
}

/// A sheet's wires as a graph: segments split wherever an interesting
/// point lies on them, then joined into strokes. Each stroke runs between
/// key points (`key`), through any other point; a point of three or more
/// wires that is not key is passed through by the first stroke that
/// reaches it, so every stroke end is a key point or lies on an earlier
/// stroke.
pub fn strokes(
    wires: &[[PointNm; 2]],
    extra: &[PointNm],
    key: impl Fn(PointNm) -> bool,
) -> Vec<Vec<PointNm>> {
    let mut points: Vec<PointNm> = wires.iter().flat_map(|w| [w[0], w[1]]).collect();
    points.extend_from_slice(extra);
    points.sort_unstable();
    points.dedup();
    let mut edges: Vec<(PointNm, PointNm)> = Vec::new();
    for w in wires {
        let (lo, hi) = (
            [w[0][0].min(w[1][0]), w[0][1].min(w[1][1])],
            [w[0][0].max(w[1][0]), w[0][1].max(w[1][1])],
        );
        let mut on: Vec<PointNm> = points
            .iter()
            .filter(|p| p[0] >= lo[0] && p[0] <= hi[0] && p[1] >= lo[1] && p[1] <= hi[1])
            .filter(|p| inside(**p, w[0], w[1]))
            .copied()
            .collect();
        let d = |p: &PointNm| ((p[0] - w[0][0]) as i128).pow(2) + ((p[1] - w[0][1]) as i128).pow(2);
        on.sort_by_key(d);
        let mut prev = w[0];
        for p in on.into_iter().chain([w[1]]) {
            let e = if prev <= p { (prev, p) } else { (p, prev) };
            edges.push(e);
            prev = p;
        }
    }
    edges.sort_unstable();
    edges.dedup();
    let mut adjacent: BTreeMap<PointNm, Vec<usize>> = BTreeMap::new();
    for (k, (a, b)) in edges.iter().enumerate() {
        adjacent.entry(*a).or_default().push(k);
        adjacent.entry(*b).or_default().push(k);
    }
    // Key: named by the caller, or a dangling end.
    let is_key = |p: PointNm| key(p) || adjacent.get(&p).is_none_or(|e| e.len() < 2);
    let mut used = vec![false; edges.len()];
    let other = |k: usize, p: PointNm| {
        let (a, b) = edges[k];
        if a == p { b } else { a }
    };
    let mut out = Vec::new();
    for start in 0..edges.len() {
        if used[start] {
            continue;
        }
        used[start] = true;
        let (a, b) = edges[start];
        let mut path = std::collections::VecDeque::from([a, b]);
        // Extend at both ends while the end is not key and an unused
        // edge leaves it, preferring the straight one.
        for forward in [true, false] {
            loop {
                let (end, before) = if forward {
                    (path[path.len() - 1], path[path.len() - 2])
                } else {
                    (path[0], path[1])
                };
                if is_key(end) {
                    break;
                }
                let free: Vec<usize> = adjacent
                    .get(&end)
                    .map(|e| e.iter().copied().filter(|k| !used[*k]).collect())
                    .unwrap_or_default();
                let straight = free.iter().copied().find(|k| {
                    let next = other(*k, end);
                    inside(end, before, next)
                });
                let Some(k) = straight.or_else(|| free.first().copied()) else {
                    break;
                };
                let next = other(k, end);
                if path.contains(&next) {
                    break;
                }
                used[k] = true;
                if forward {
                    path.push_back(next);
                } else {
                    path.push_front(next);
                }
            }
        }
        out.push(path.into_iter().collect());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tee_is_two_strokes_one_through_the_branch() {
        // A horizontal wire 0–20 with a vertical one from its middle; the
        // middle is no key point.
        let mm = |v: i64| v * 1_000_000;
        let wires = [
            [[0, 0], [mm(10), 0]],
            [[mm(10), 0], [mm(20), 0]],
            [[mm(10), 0], [mm(10), mm(5)]],
        ];
        let keys = [[0, 0], [mm(20), 0], [mm(10), mm(5)]];
        let s = strokes(&wires, &[], |p| keys.contains(&p));
        assert_eq!(s.len(), 2);
        assert_eq!(s[0], vec![[0, 0], [mm(10), 0], [mm(20), 0]]);
        assert_eq!(s[1].len(), 2);
        // A point on a wire splits it.
        let s = strokes(&[[[0, 0], [mm(20), 0]]], &[[mm(5), 0]], |p| p == [mm(5), 0]);
        assert_eq!(s.len(), 2);
    }

    #[test]
    fn pins_turn_and_mirror_with_their_symbol() {
        let mut s = KSymbol {
            at: [100, 100],
            ..KSymbol::default()
        };
        // Library (10, 5) is y-up: (10, −5) on the sheet.
        assert_eq!(pin_on_sheet(&s, [10, 5]), [110, 95]);
        s.angle = 90.0;
        // Turned counter-clockwise on screen: right becomes up (−y).
        assert_eq!(pin_on_sheet(&s, [10, 0]), [100, 90]);
        s.angle = 0.0;
        s.mirror_y = true;
        assert_eq!(pin_on_sheet(&s, [10, 0]), [90, 100]);
    }
}
