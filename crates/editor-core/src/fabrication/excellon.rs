//! Excellon drill files, the Gerber drill map and the drill table.
//!
//! Written from the public Excellon format description: an `M48` header
//! with `METRIC` units and the tool table (`TnnCd.ddd`), `%`, absolute
//! coordinates (`G90`) in drill mode (`G05`), then each tool's hits as
//! `X…Y…` with a decimal point (3 decimals, 1 µm), so no zero-suppression
//! convention is needed. Slots are routed with the canned slot `G85`
//! (`XaYaG85XbYb`, the tool's width). Plated and non-plated holes are
//! separate files. Each file also carries the X2 attributes as `; #@!`
//! comment lines (`TF.FileFunction,Plated,1,4,PTH`), which readers that do
//! not know them skip as comments.
use super::artwork;
use super::drill::{self, DrillHole, DrillTable, HoleFeature};
use super::gerber::{Aperture, Seg, Writer};
use super::{FabFile, FabricationOptions, FileGroup, MAX_FILE_BYTES};
use crate::diagnostics::{Error, Result};
use crate::geometry::Point;
use crate::model::Project;
use std::fmt::Write as _;

/// The drill map's symbol for a tool, by its position in the table.
pub const SYMBOLS: [&str; 8] = [
    "circle", "cross", "square", "triangle", "x", "diamond", "circle-cross", "square-x",
];
/// Size of a drill-map symbol, mm.
pub const SYMBOL_MM: f64 = 1.0;
const PEN_MM: f64 = 0.05;

/// A drill coordinate: 3 decimals with a point.
fn xy(p: Point) -> Result<String> {
    for v in p {
        if !(v.is_finite() && v.abs() < 10_000.0) {
            return Err(Error::Invalid(format!(
                "fabrication.coordinate: {v} mm is outside the drill coordinate range"
            )));
        }
    }
    Ok(format!("X{}Y{}", fixed(p[0]), fixed(p[1])))
}

fn fixed(v: f64) -> String {
    let s = format!("{:.3}", v);
    if s == "-0.000" { "0.000".into() } else { s }
}

/// The drill files, drill map and table of `project`.
pub fn files(project: &Project, options: &FabricationOptions, stem: &str) -> Result<Vec<FabFile>> {
    let holes = drill::holes(project);
    let table = drill::table(&holes);
    let layers = project.stackup.copper.len();
    let mut out = Vec::new();
    for plated in [true, false] {
        if !holes.iter().any(|h| h.plated == plated) {
            continue;
        }
        let (name, description) = if plated {
            ("plated", "Plated drill (PTH, vias, plated slots)")
        } else {
            ("non-plated", "Non-plated drill (NPTH)")
        };
        out.push(FabFile {
            name: format!("{stem}-{name}.drl"),
            group: FileGroup::Drill,
            description: description.into(),
            bytes: excellon(project, options, &holes, &table, plated, layers)?,
        });
    }
    if !holes.is_empty() {
        out.push(FabFile {
            name: format!("{stem}-drill-map.gbr"),
            group: FileGroup::Drill,
            description: "Drill map (one symbol per tool)".into(),
            bytes: drill_map(project, options, &holes, &table)?,
        });
    }
    out.push(FabFile {
        name: format!("{stem}-drill-table.txt"),
        group: FileGroup::Drill,
        description: "Drill table".into(),
        bytes: drill_table_text(project, &table).into_bytes(),
    });
    Ok(out)
}

/// One Excellon file of the plated or non-plated holes.
pub fn excellon(
    project: &Project,
    options: &FabricationOptions,
    holes: &[DrillHole],
    table: &DrillTable,
    plated: bool,
    layers: usize,
) -> Result<Vec<u8>> {
    let mut s = String::new();
    s.push_str("M48\n");
    let _ = writeln!(
        s,
        "; Pcbcraft {} drill file: {}",
        env!("CARGO_PKG_VERSION"),
        if plated { "plated" } else { "non-plated" }
    );
    for (name, value) in artwork::common_attributes(project, options) {
        if value.is_empty() {
            let _ = writeln!(s, "; #@! TF.{name}");
        } else {
            let _ = writeln!(s, "; #@! TF.{name},{value}");
        }
    }
    let _ = writeln!(
        s,
        "; #@! TF.FileFunction,{},1,{layers},{}",
        if plated { "Plated" } else { "NonPlated" },
        if plated { "PTH" } else { "NPTH" }
    );
    s.push_str("; #@! TF.FilePolarity,Positive\n");
    s.push_str("FMAT,2\nMETRIC\n");
    let tools: Vec<_> = table.tools.iter().filter(|t| t.plated == plated).collect();
    for tool in &tools {
        let _ = writeln!(
            s,
            "; #@! TA.AperFunction,{},{},{}",
            if plated { "Plated" } else { "NonPlated" },
            if plated { "PTH" } else { "NPTH" },
            tool.function.aper_function()
        );
        let _ = writeln!(s, "T{}C{}", tool.number, fixed(tool.diameter));
    }
    s.push_str("%\nG90\nG05\n");
    for tool in &tools {
        let _ = writeln!(s, "T{}", tool.number);
        for hole in holes
            .iter()
            .filter(|h| table.tool_of(h).is_some_and(|t| std::ptr::eq(t, *tool)))
        {
            match hole.feature {
                HoleFeature::Round { at, .. } => {
                    let _ = writeln!(s, "{}", xy(at)?);
                }
                HoleFeature::Slot { from, to, .. } => {
                    let _ = writeln!(s, "{}G85{}", xy(from)?, xy(to)?);
                }
            }
            if s.len() > MAX_FILE_BYTES {
                return Err(Error::Limit(
                    "fabrication.too_large: a drill file exceeds the file bound".into(),
                ));
            }
        }
    }
    s.push_str("T0\nM30\n");
    Ok(s.into_bytes())
}

/// The drill map: the profile and one symbol per hole, its shape set by
/// the hole's tool (`SYMBOLS`, in table order); slots are drawn as their
/// centre line between two symbols.
fn drill_map(
    project: &Project,
    options: &FabricationOptions,
    holes: &[DrillHole],
    table: &DrillTable,
) -> Result<Vec<u8>> {
    let mut attributes = artwork::common_attributes(project, options);
    attributes.push(("FileFunction", "Drillmap".into()));
    attributes.push(("FilePolarity", "Positive".into()));
    let mut w = Writer::new(&attributes);
    let pen = w.aperture(Aperture::circle(PEN_MM)?, None);
    let board = project.board();
    let (start, segs) = artwork::outline_path(&board.outer, &Default::default());
    w.stroke(pen, start, &segs)?;
    for cutout in &board.cutouts {
        let (start, segs) = artwork::outline_path(&cutout.outline, &Default::default());
        w.stroke(pen, start, &segs)?;
    }
    for hole in holes {
        let Some(index) = table
            .tool_of(hole)
            .and_then(|t| table.tools.iter().position(|u| std::ptr::eq(t, u)))
        else {
            continue;
        };
        let symbol = SYMBOLS[index % SYMBOLS.len()];
        match hole.feature {
            HoleFeature::Round { at, .. } => mark(&mut w, pen, symbol, at)?,
            HoleFeature::Slot { from, to, .. } => {
                mark(&mut w, pen, symbol, from)?;
                mark(&mut w, pen, symbol, to)?;
                w.stroke(pen, from, &[Seg::Line(to)])?;
            }
        }
    }
    w.finish()
}

/// One drill-map symbol centred on `at`.
fn mark(w: &mut Writer, pen: u32, symbol: &str, at: Point) -> Result<()> {
    let h = SYMBOL_MM / 2.0;
    let p = |dx: f64, dy: f64| [at[0] + dx, at[1] + dy];
    let line = |w: &mut Writer, a: Point, b: Point| w.stroke(pen, a, &[Seg::Line(b)]);
    let poly = |w: &mut Writer, pts: &[Point]| -> Result<()> {
        let Some((first, rest)) = pts.split_first() else {
            return Ok(());
        };
        let mut segs: Vec<Seg> = rest.iter().map(|q| Seg::Line(*q)).collect();
        segs.push(Seg::Line(*first));
        w.stroke(pen, *first, &segs)
    };
    let circle = |w: &mut Writer| {
        let s = p(h, 0.0);
        w.stroke(
            pen,
            s,
            &[Seg::Arc {
                to: s,
                center: at,
                ccw: true,
            }],
        )
    };
    match symbol {
        "circle" => circle(w),
        "cross" => {
            line(w, p(-h, 0.0), p(h, 0.0))?;
            line(w, p(0.0, -h), p(0.0, h))
        }
        "square" => poly(w, &[p(-h, -h), p(h, -h), p(h, h), p(-h, h)]),
        "triangle" => poly(w, &[p(-h, -h), p(h, -h), p(0.0, h)]),
        "x" => {
            line(w, p(-h, -h), p(h, h))?;
            line(w, p(-h, h), p(h, -h))
        }
        "diamond" => poly(w, &[p(0.0, -h), p(h, 0.0), p(0.0, h), p(-h, 0.0)]),
        "circle-cross" => {
            circle(w)?;
            line(w, p(-h, 0.0), p(h, 0.0))?;
            line(w, p(0.0, -h), p(0.0, h))
        }
        _ => {
            poly(w, &[p(-h, -h), p(h, -h), p(h, h), p(-h, h)])?;
            line(w, p(-h, -h), p(h, h))?;
            line(w, p(-h, h), p(h, -h))
        }
    }
}

/// The drill table as aligned text.
pub fn drill_table_text(project: &Project, table: &DrillTable) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "Drill table: {}", project.name);
    let _ = writeln!(s, "Units: mm. Diameters are finished hole sizes as drawn.");
    let _ = writeln!(s);
    let _ = writeln!(
        s,
        "{:<6}{:<10}{:<12}{:<7}{:<12}{:>7}  {}",
        "Tool", "Diameter", "Plating", "Kind", "Function", "Count", "Symbol"
    );
    let (mut plated, mut other, mut slots) = (0usize, 0usize, 0usize);
    for (i, tool) in table.tools.iter().enumerate() {
        let _ = writeln!(
            s,
            "{:<6}{:<10}{:<12}{:<7}{:<12}{:>7}  {}",
            format!("T{}", tool.number),
            fixed(tool.diameter),
            if tool.plated { "plated" } else { "non-plated" },
            if tool.slot { "slot" } else { "hit" },
            tool.function.label(),
            tool.count,
            SYMBOLS[i % SYMBOLS.len()]
        );
        if tool.plated {
            plated += tool.count;
        } else {
            other += tool.count;
        }
        if tool.slot {
            slots += tool.count;
        }
    }
    let _ = writeln!(s);
    let _ = writeln!(
        s,
        "Holes: {} ({} plated, {} non-plated), of which {} routed slots.",
        plated + other,
        plated,
        other,
        slots
    );
    s
}
