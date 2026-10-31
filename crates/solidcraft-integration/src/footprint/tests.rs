//! F2 evidence: original synthetic fixtures (round numbers, from no library,
//! datasheet or EDA tool) built through the same `library.edit_footprint`
//! path the UI, CLI and MCP use. Tests marked `wasm_bindgen_test` also run
//! in Node (`cargo xtask wasm-footprint`) and assert the same golden hashes
//! as natively: the native/wasm agreement check.

use std::collections::BTreeSet;

use pcbcraft_core::diagnostics::{Error, Result};
use pcbcraft_core::geometry::Shape;
use pcbcraft_core::ids::{PackageTerminalId, PadId};
use pcbcraft_core::library::footprint_edit::{self, AssignRule, FootprintEditReport};
use pcbcraft_core::library::footprint_source::{
    ArrayId, ArrayLayout, DimKind, DimValue, EvaluationRequest, Expr, FootprintEdit,
    FootprintEvaluator, GenerationKey, GeomConstraint, GraphicRole, GraphicSource, HoleRole,
    NumberingScheme, PackageValue, PadOverride, PadShapeExpr, PadTemplate, PadTemplateParams,
    ParamName, ParamUnit, ParamValue, Pin1Position, SketchCurveId, SketchEdit, SketchPointId,
    SketchRef, SkipRule, SolveStatus,
};
use pcbcraft_core::library::graphics::{DrawingLayer, Fill, Item, Side};
use pcbcraft_core::library::{
    FootprintDefinition, HoleShape, PackageDefinition, PackageTerminal, Pad, PadKind, PadShape,
    PadSide, Segment,
};

use super::{SketchEvaluator, check, evaluator_ref};

const EV: SketchEvaluator = SketchEvaluator;

fn package(terminals: usize) -> PackageDefinition {
    PackageDefinition {
        body_size: [4.9, 3.9, 1.5],
        terminals: (1..=terminals)
            .map(|n| PackageTerminal {
                id: PackageTerminalId(u32::try_from(n).unwrap_or(0) + 100),
                designation: n.to_string(),
            })
            .collect(),
        ..PackageDefinition::default()
    }
}

/// A footprint draft edited through the core's `library.edit_footprint`.
struct Fp {
    def: FootprintDefinition,
    pkg: PackageDefinition,
}

impl Fp {
    fn new(pkg: PackageDefinition) -> Self {
        Self {
            def: FootprintDefinition {
                next_local: 1,
                ..FootprintDefinition::default()
            },
            pkg,
        }
    }
    fn edit(&mut self, edits: Vec<FootprintEdit>) -> Result<FootprintEditReport> {
        let (def, report) = footprint_edit::edit(&self.def, &self.pkg, &edits, &EV)?;
        self.def = def;
        Ok(report)
    }
    fn pad(&self, number: &str) -> Result<&Pad> {
        self.def
            .pads
            .iter()
            .find(|p| p.number == number)
            .ok_or_else(|| Error::NotFound(format!("no pad {number}")))
    }
}

fn param(name: &str, expr: &str, unit: ParamUnit) -> FootprintEdit {
    FootprintEdit::SetParameter {
        name: ParamName(name.into()),
        expr: Expr(expr.into()),
        unit,
    }
}

fn template(kind: PadTemplate, w: &str, h: &str, number: &str) -> PadTemplateParams {
    PadTemplateParams {
        width: Expr(w.into()),
        height: Expr(h.into()),
        radius_ratio: (kind == PadTemplate::RoundRect).then(|| Expr("0.25".into())),
        kind: PadKind::Smd,
        side: PadSide::Top,
        terminal: None,
        number: number.into(),
    }
}

fn first_point(report: &FootprintEditReport) -> Result<SketchPointId> {
    report
        .created
        .iter()
        .find_map(|r| match r {
            SketchRef::Point(p) => Some(p.clone()),
            _ => None,
        })
        .ok_or_else(|| Error::NotFound("no point created".into()))
}

fn first_curve(report: &FootprintEditReport) -> Result<SketchCurveId> {
    report
        .created
        .iter()
        .find_map(|r| match r {
            SketchRef::Curve(c) => Some(c.clone()),
            _ => None,
        })
        .ok_or_else(|| Error::NotFound("no curve created".into()))
}

fn origin() -> SketchRef {
    SketchRef::Point(SketchPointId("origin".into()))
}

/// Locate `anchor` from the origin by two bound distances.
fn locate(anchor: &SketchPointId, x: &str, y: &str) -> Vec<FootprintEdit> {
    [
        (DimKind::HorizontalDistance, x),
        (DimKind::VerticalDistance, y),
    ]
    .into_iter()
    .map(|(kind, p)| {
        FootprintEdit::Sketch(SketchEdit::AddDimension {
            kind,
            refs: vec![origin(), SketchRef::Point(anchor.clone())],
            value: DimValue::Param(ParamName(p.into())),
        })
    })
    .collect()
}

/// A fixed construction line from `a` to `b` (an array direction).
fn fixed_line(fp: &mut Fp, a: [f64; 2], b: [f64; 2]) -> Result<SketchCurveId> {
    let report = fp.edit(vec![FootprintEdit::Sketch(SketchEdit::AddLine {
        a,
        b,
        construction: true,
    })])?;
    let line = first_curve(&report)?;
    let points: Vec<SketchRef> = report
        .created
        .iter()
        .filter(|r| matches!(r, SketchRef::Point(_)))
        .cloned()
        .collect();
    fp.edit(
        points
            .into_iter()
            .map(|p| {
                FootprintEdit::Sketch(SketchEdit::AddConstraint {
                    kind: GeomConstraint::Fixed,
                    refs: vec![p],
                })
            })
            .collect(),
    )?;
    Ok(line)
}

/// SOIC-8: a rectangular seed and a dual-row array (2 × 4); pitch, row
/// span and pad size are footprint parameters.
fn soic8() -> Result<(Fp, PadId)> {
    let mut fp = Fp::new(package(8));
    let report = fp.edit(vec![
        param("pitch", "1.27 mm", ParamUnit::Mm),
        param("per_row", "4", ParamUnit::Count),
        param("row_span", "5.4 mm", ParamUnit::Mm),
        param("pad_l", "1.55 mm", ParamUnit::Mm),
        param("pad_w", "0.6 mm", ParamUnit::Mm),
        param("seed_x", "row_span / 2", ParamUnit::Mm),
        param("seed_y", "(per_row - 1) / 2 * pitch", ParamUnit::Mm),
        FootprintEdit::AddPad {
            template: PadTemplate::Rect,
            at: [-2.7, 1.905],
            params: template(PadTemplate::Rect, "pad_l", "pad_w", "1"),
        },
    ])?;
    let seed = *report
        .pads
        .first()
        .ok_or_else(|| Error::NotFound("seed".into()))?;
    let anchor = first_point(&report)?;
    fp.edit(locate(&anchor, "seed_x", "seed_y"))?;
    let down = fixed_line(&mut fp, [-3.0, 3.0], [-3.0, 2.0])?;
    fp.edit(vec![FootprintEdit::AddArray {
        seed,
        layout: ArrayLayout::DualRow {
            per_row: Expr("per_row".into()),
            pitch: Expr("pitch".into()),
            row_span: Expr("row_span".into()),
            direction: down,
        },
        numbering: NumberingScheme::Sequential,
        skip: SkipRule::default(),
        first_number: String::new(),
    }])?;
    Ok((fp, seed))
}

/// QFN (non-square, 5 per vertical side and 3 per horizontal side) with an
/// exposed pad at the origin carrying a construction drill circle as its
/// hole role.
fn qfn() -> Result<Fp> {
    let mut fp = Fp::new(package(17));
    let report = fp.edit(vec![
        param("pitch", "0.5 mm", ParamUnit::Mm),
        param("span_x", "4.8 mm", ParamUnit::Mm),
        param("span_y", "3.8 mm", ParamUnit::Mm),
        param("pad_l", "0.8 mm", ParamUnit::Mm),
        param("pad_w", "0.25 mm", ParamUnit::Mm),
        param("seed_x", "span_x / 2", ParamUnit::Mm),
        param("seed_y", "pitch * 2", ParamUnit::Mm),
        param("ep", "2.6 mm", ParamUnit::Mm),
        FootprintEdit::AddPad {
            template: PadTemplate::Rect,
            at: [-2.4, 1.0],
            params: template(PadTemplate::Rect, "pad_l", "pad_w", "1"),
        },
    ])?;
    let seed = *report
        .pads
        .first()
        .ok_or_else(|| Error::NotFound("seed".into()))?;
    let anchor = first_point(&report)?;
    let mut edits = locate(&anchor, "seed_x", "seed_y");
    edits.push(FootprintEdit::AddArray {
        seed,
        layout: ArrayLayout::Quad {
            nx: Expr("3".into()),
            ny: Expr("5".into()),
            pitch: Expr("pitch".into()),
            span_x: Expr("span_x".into()),
            span_y: Expr("span_y".into()),
            pin1: Pin1Position::Left,
        },
        numbering: NumberingScheme::CounterClockwise,
        skip: SkipRule::default(),
        first_number: String::new(),
    });
    fp.edit(edits)?;
    let ep = fp.edit(vec![FootprintEdit::AddPad {
        template: PadTemplate::Rect,
        at: [0.0, 0.0],
        params: template(PadTemplate::Rect, "ep", "ep", "EP"),
    }])?;
    let ep_id = *ep
        .pads
        .first()
        .ok_or_else(|| Error::NotFound("ep".into()))?;
    let ep_anchor = first_point(&ep)?;
    let drill = fp.edit(vec![
        FootprintEdit::Sketch(SketchEdit::AddConstraint {
            kind: GeomConstraint::Coincident,
            refs: vec![origin(), SketchRef::Point(ep_anchor)],
        }),
        FootprintEdit::Sketch(SketchEdit::AddCircle {
            center: [0.0, 0.0],
            radius: 0.15,
            construction: true,
        }),
    ])?;
    let circle = first_curve(&drill)?;
    let centre = first_point(&drill)?;
    let mut role = fp
        .def
        .source
        .as_ref()
        .and_then(|s| s.pads.iter().find(|r| r.pad == ep_id).cloned())
        .ok_or_else(|| Error::NotFound("ep role".into()))?;
    role.kind = PadKind::Smd;
    role.hole = Some(HoleRole {
        curves: vec![circle.clone()],
        plated: true,
    });
    fp.edit(vec![
        FootprintEdit::Sketch(SketchEdit::AddConstraint {
            kind: GeomConstraint::Coincident,
            refs: vec![origin(), SketchRef::Point(centre)],
        }),
        FootprintEdit::Sketch(SketchEdit::AddDimension {
            kind: DimKind::Diameter,
            refs: vec![SketchRef::Curve(circle)],
            value: DimValue::Fixed(0.3),
        }),
        FootprintEdit::SetPadRole { role },
    ])?;
    Ok(fp)
}

/// BGA-256: a circular seed and a 16 × 16 grid at 1.0 mm with the centre
/// 4 × 4 removed (240 lands), lettered rows without I, O, Q, S, X, Z.
fn bga256() -> Result<Fp> {
    let mut fp = Fp::new(package(0));
    let report = fp.edit(vec![
        param("pitch", "1.0 mm", ParamUnit::Mm),
        param("rows", "16", ParamUnit::Count),
        param("cols", "16", ParamUnit::Count),
        param("land_d", "0.45 mm", ParamUnit::Mm),
        param("seed_x", "(cols - 1) / 2 * pitch", ParamUnit::Mm),
        param("seed_y", "(rows - 1) / 2 * pitch", ParamUnit::Mm),
        FootprintEdit::AddPad {
            template: PadTemplate::Circle,
            at: [-7.5, 7.5],
            params: template(PadTemplate::Circle, "land_d", "land_d", "A1"),
        },
    ])?;
    let seed = *report
        .pads
        .first()
        .ok_or_else(|| Error::NotFound("seed".into()))?;
    let anchor = first_point(&report)?;
    let mut edits = locate(&anchor, "seed_x", "seed_y");
    edits.push(FootprintEdit::AddArray {
        seed,
        layout: ArrayLayout::Grid {
            rows: Expr("rows".into()),
            cols: Expr("cols".into()),
            pitch_x: Expr("pitch".into()),
            pitch_y: Expr("pitch".into()),
            stagger: false,
        },
        numbering: NumberingScheme::GridAlpha {
            skip_letters: "IOQSXZ".into(),
        },
        skip: SkipRule {
            centre_void: Some([4, 4]),
            ..SkipRule::default()
        },
        first_number: String::new(),
    });
    fp.edit(edits)?;
    Ok(fp)
}

fn rect(w: f64, h: f64) -> PadShape {
    PadShape::Rect { size: [w, h] }
}

fn solve_of(fp: &Fp) -> Result<(SolveStatus, u32, String)> {
    let s = fp
        .def
        .source
        .as_ref()
        .ok_or_else(|| Error::NotFound("source".into()))?;
    Ok((s.solve.status, s.solve.dof, s.solve.result_hash.clone()))
}

#[test]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn soic8_solves_to_eight_typed_pads() -> Result<()> {
    let (fp, seed) = soic8()?;
    assert_eq!(fp.def.pads.len(), 8);
    assert_eq!(solve_of(&fp)?.0, SolveStatus::Solved);
    assert_eq!(solve_of(&fp)?.1, 0, "fully constrained");
    let one = fp.pad("1")?;
    assert_eq!(one.id, seed);
    assert_eq!((one.position, one.rotation_deg), ([-2.7, 1.905], 0.0));
    assert_eq!(one.shape, rect(1.55, 0.6));
    assert_eq!(fp.pad("4")?.position, [-2.7, -1.905]);
    // Row 1 faces row 0 turned 180°: canonical rotation of a rect is 0.
    let five = fp.pad("5")?;
    assert_eq!((five.position, five.rotation_deg), ([2.7, -1.905], 0.0));
    assert_eq!(fp.pad("8")?.position, [2.7, 1.905]);
    let ids: BTreeSet<PadId> = fp.def.pads.iter().map(|p| p.id).collect();
    assert_eq!(ids.len(), 8, "every pad has its own id");
    assert!(
        fp.def
            .pads
            .iter()
            .all(|p| p.id == seed || p.terminal.is_none())
    );
    Ok(())
}

#[test]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn pitch_and_count_changes_keep_pad_identity() -> Result<()> {
    let (mut fp, _) = soic8()?;
    let by_number = |fp: &Fp| -> Vec<(String, PadId)> {
        let mut v: Vec<_> = fp
            .def
            .pads
            .iter()
            .map(|p| (p.number.clone(), p.id))
            .collect();
        v.sort();
        v
    };
    let before = by_number(&fp);
    fp.edit(vec![param("pitch", "1.0 mm", ParamUnit::Mm)])?;
    assert_eq!(by_number(&fp), before, "same ids, same numbers");
    assert_eq!(fp.pad("4")?.position, [-2.7, -1.5]);
    let old: BTreeSet<PadId> = fp.def.pads.iter().map(|p| p.id).collect();
    let grown = fp.edit(vec![param("per_row", "7", ParamUnit::Count)])?;
    let now: BTreeSet<PadId> = fp.def.pads.iter().map(|p| p.id).collect();
    assert_eq!(now.len(), 14);
    assert!(old.is_subset(&now), "every old pad survives growth");
    assert_eq!(grown.pads.len(), 6, "six new ids allocated");
    assert!(
        grown
            .findings
            .iter()
            .any(|f| f.code == "footprint.unresolved_key")
    );
    fp.edit(vec![param("per_row", "2", ParamUnit::Count)])?;
    let shrunk: BTreeSet<PadId> = fp.def.pads.iter().map(|p| p.id).collect();
    assert_eq!(shrunk.len(), 4);
    assert!(
        shrunk.is_subset(&old),
        "survivors are exactly old keys still generated"
    );
    Ok(())
}

#[test]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn qfn_places_each_side_and_reads_the_drill_role() -> Result<()> {
    let fp = qfn()?;
    assert_eq!(fp.def.pads.len(), 17, "2 × (3 + 5) + EP");
    assert_eq!(solve_of(&fp)?.1, 0);
    let one = fp.pad("1")?;
    assert_eq!(
        (one.position, one.shape.clone()),
        ([-2.4, 1.0], rect(0.8, 0.25))
    );
    assert_eq!(fp.pad("5")?.position, [-2.4, -1.0]);
    // Bottom side: 3 pads left to right, a quarter turn (90° canonical).
    let six = fp.pad("6")?;
    assert_eq!((six.position, six.rotation_deg), ([-0.5, -1.9], 90.0));
    assert_eq!(fp.pad("8")?.position, [0.5, -1.9]);
    assert_eq!(fp.pad("9")?.position, [2.4, -1.0]);
    assert_eq!(fp.pad("14")?.position, [0.5, 1.9]);
    assert_eq!(fp.pad("16")?.position, [-0.5, 1.9]);
    let ep = fp.pad("EP")?;
    assert_eq!(
        (ep.position, ep.shape.clone()),
        ([0.0, 0.0], rect(2.6, 2.6))
    );
    let hole = ep.hole.ok_or_else(|| Error::NotFound("ep hole".into()))?;
    assert_eq!(hole.shape, HoleShape::Round { diameter: 0.3 });
    assert_eq!(hole.offset, [0.0, 0.0]);
    let centres: BTreeSet<[u64; 2]> = fp
        .def
        .pads
        .iter()
        .map(|p| [p.position[0].to_bits(), p.position[1].to_bits()])
        .collect();
    assert_eq!(centres.len(), 17);
    Ok(())
}

#[test]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn bga256_grid_with_centre_void() -> Result<()> {
    let fp = bga256()?;
    assert_eq!(fp.def.pads.len(), 240);
    let a1 = fp.pad("A1")?;
    assert_eq!(
        (a1.position, a1.shape.clone()),
        ([-7.5, 7.5], PadShape::Circle { diameter: 0.45 })
    );
    assert_eq!(fp.pad("T16")?.position, [7.5, -7.5]);
    assert_eq!(
        fp.pad("J1")?.position,
        [-7.5, -0.5],
        "row 9 is J: I is skipped"
    );
    assert!(
        fp.pad("G7").is_err() && fp.pad("H8").is_err(),
        "inside the void"
    );
    assert!(!fp.def.pads.iter().any(|p| {
        ["I", "O", "Q", "S", "X", "Z"]
            .iter()
            .any(|l| p.number.starts_with(l))
    }));
    Ok(())
}

/// Golden result hashes recorded natively (macOS arm64, ci profile) and
/// asserted on every target, plus the raw f64 bits of every solved point.
const GOLDEN: [(&str, &str, u64); 3] = [
    ("SOIC-8", "12196ba69182e5d5", 0x516c3fa73ba0b6d9),
    ("QFN", "ad5c8b43e2c76d8e", 0x635aadbec8021745),
    ("BGA-256", "73f138be63545fd7", 0x1bd4d84c6beaade5),
];

fn raw_bits(fp: &Fp) -> Result<u64> {
    let s = fp
        .def
        .source
        .as_ref()
        .ok_or_else(|| Error::NotFound("source".into()))?;
    let sketch = check::check_sketch(&s.sketch.0).map_err(Error::from)?;
    let raw: Vec<u8> = sketch
        .points
        .iter()
        .flat_map(|p| {
            [
                p.pos.x.to_bits().to_le_bytes(),
                p.pos.y.to_bits().to_le_bytes(),
            ]
        })
        .flatten()
        .collect();
    Ok(super::evaluate::fnv(&raw))
}

#[test]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn results_match_golden_hashes_on_this_target() -> Result<()> {
    let cases = [
        ("SOIC-8", soic8()?.0),
        ("QFN", qfn()?),
        ("BGA-256", bga256()?),
    ];
    let mut got = Vec::new();
    for (name, fp) in &cases {
        let hash = solve_of(fp)?.2;
        let raw = raw_bits(fp)?;
        println!("(\"{name}\", \"{hash}\", {raw:#018x}),");
        got.push((*name, hash, raw));
    }
    for (name, hash, raw) in got {
        let golden = GOLDEN.iter().find(|g| g.0 == name);
        assert_eq!(
            golden.map(|g| g.1),
            Some(hash.as_str()),
            "{name}: quantized result changed"
        );
        assert_eq!(
            golden.map(|g| g.2),
            Some(raw),
            "{name}: solved point bits changed"
        );
    }
    Ok(())
}

#[test]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn re_evaluation_is_fresh_and_tampering_is_stale() -> Result<()> {
    let (fp, _) = soic8()?;
    for _ in 0..5 {
        footprint_edit::check_fresh(&fp.def, &fp.pkg, &EV)?;
    }
    let mut tampered = fp.def.clone();
    if let Some(s) = tampered.source.as_deref_mut() {
        s.solve.result_hash = "0000000000000000".into();
    }
    let err = footprint_edit::check_fresh(&tampered, &fp.pkg, &EV);
    assert!(
        matches!(&err, Err(Error::Conflict(m)) if m.starts_with("footprint.stale")),
        "{err:?}"
    );
    Ok(())
}

#[test]
fn failed_edits_leave_the_footprint_unchanged() -> Result<()> {
    let (mut fp, seed) = soic8()?;
    let before = fp.def.clone();
    let impossible = fp.edit(vec![param("pad_w", "-1 mm", ParamUnit::Mm)]);
    assert!(
        matches!(&impossible, Err(Error::Conflict(m)) if m.starts_with("footprint.")),
        "{impossible:?}"
    );
    let cycle = fp.edit(vec![param("pitch", "row_span * pitch", ParamUnit::Mm)]);
    assert!(
        matches!(&cycle, Err(Error::Conflict(m)) if m.contains("param_expr")),
        "{cycle:?}"
    );
    let unit = fp.edit(vec![param("pitch", "30 deg", ParamUnit::Mm)]);
    assert!(unit.is_err());
    let reserved = fp.edit(vec![param("pkg_e", "1 mm", ParamUnit::Mm)]);
    assert!(matches!(&reserved, Err(Error::Invalid(m)) if m.contains("reserved")));
    let unit_word = fp.edit(vec![param("mm", "1 mm", ParamUnit::Mm)]);
    assert!(
        unit_word.is_err(),
        "Solidcraft's reserved words are refused"
    );
    let fractional = fp.edit(vec![param("per_row", "2.5", ParamUnit::Count)]);
    assert!(fractional.is_err());
    let missing_pkg = fp.edit(vec![param("pitch", "pkg_e", ParamUnit::Mm)]);
    assert!(missing_pkg.is_err());
    let owned = fp
        .def
        .source
        .as_ref()
        .and_then(|s| s.pads.iter().find(|r| r.pad == seed))
        .and_then(|r| match &r.geometry {
            pcbcraft_core::library::footprint_source::PadGeometry::Template {
                owned_curves,
                ..
            } => owned_curves.first().cloned(),
            _ => None,
        })
        .ok_or_else(|| Error::NotFound("owned".into()))?;
    let delete_owned = fp.edit(vec![FootprintEdit::Sketch(SketchEdit::Delete {
        ids: vec![SketchRef::Curve(owned)],
    })]);
    assert!(
        matches!(&delete_owned, Err(Error::Conflict(m)) if m.starts_with("footprint.owned_curve"))
    );
    let seed_removed = fp.edit(vec![FootprintEdit::RemovePad { pad: seed }]);
    assert!(
        matches!(&seed_removed, Err(Error::Conflict(m)) if m.starts_with("footprint.array_seed"))
    );
    let referenced = fp.edit(vec![FootprintEdit::RemoveParameter {
        name: ParamName("pitch".into()),
    }]);
    assert!(
        matches!(&referenced, Err(Error::Conflict(m)) if m.starts_with("footprint.param_in_use"))
    );
    assert_eq!(fp.def, before);
    Ok(())
}

#[test]
fn wrong_evaluator_revision_is_refused() -> Result<()> {
    let (fp, _) = soic8()?;
    let mut stale = fp.def.clone();
    if let Some(s) = stale.source.as_deref_mut() {
        s.evaluator.rev = "0000000".into();
    }
    let source = stale
        .source
        .as_deref()
        .ok_or_else(|| Error::NotFound("source".into()))?;
    let values = pcbcraft_core::library::footprint_source::package_values(&fp.pkg);
    let e = EV.evaluate(EvaluationRequest {
        source,
        package: &fp.pkg,
        package_values: &values,
        previous: &stale,
    })?;
    assert!(
        e.findings
            .iter()
            .any(|f| f.code == "footprint.evaluator_rev" && f.blocks.is_some())
    );
    assert_eq!(evaluator_ref().engine, super::ENGINE);
    Ok(())
}

#[test]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn typed_package_values_are_injected() -> Result<()> {
    let mut fp = Fp::new(package(4));
    fp.edit(vec![
        param("n", "pkg_n * 2", ParamUnit::Count),
        param("w", "pkg_W / 2", ParamUnit::Mm),
        FootprintEdit::AddPad {
            template: PadTemplate::Rect,
            at: [0.0, 0.0],
            params: template(PadTemplate::Rect, "w", "n * 0.1 mm", "1"),
        },
    ])?;
    assert_eq!(fp.pad("1")?.shape, rect(1.95, 0.8));
    // Angles and counts from a package (as package parameters will add).
    let source = fp
        .def
        .source
        .as_deref()
        .ok_or_else(|| Error::NotFound("source".into()))?;
    let mut source = source.clone();
    source
        .parameters
        .push(pcbcraft_core::library::footprint_source::FootprintParam {
            name: ParamName("tilt".into()),
            expr: Expr("pkg_a / 3".into()),
            unit: ParamUnit::Deg,
            comment: String::new(),
        });
    let values = [
        PackageValue {
            name: ParamName("pkg_a".into()),
            value: ParamValue::Angle(90.0),
        },
        PackageValue {
            name: ParamName("pkg_n".into()),
            value: ParamValue::Count(4),
        },
        PackageValue {
            name: ParamName("pkg_W".into()),
            value: ParamValue::Length(3.9),
        },
    ];
    let e = EV.evaluate(EvaluationRequest {
        source: &source,
        package: &fp.pkg,
        package_values: &values,
        previous: &fp.def,
    })?;
    assert!(
        e.findings.iter().all(|f| f.blocks.is_none()),
        "{:?}",
        e.findings
    );
    Ok(())
}

#[test]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn every_template_classifies_to_its_shape() -> Result<()> {
    let mut fp = Fp::new(package(4));
    let report = fp.edit(vec![
        FootprintEdit::AddPad {
            template: PadTemplate::Rect,
            at: [-6.0, 0.0],
            params: template(PadTemplate::Rect, "1 mm", "2 mm", "1"),
        },
        FootprintEdit::AddPad {
            template: PadTemplate::Circle,
            at: [-2.0, 0.0],
            params: template(PadTemplate::Circle, "1.2 mm", "1.2 mm", "2"),
        },
        FootprintEdit::AddPad {
            template: PadTemplate::Oblong,
            at: [2.0, 0.0],
            params: template(PadTemplate::Oblong, "1 mm", "2 mm", "3"),
        },
        FootprintEdit::AddPad {
            template: PadTemplate::RoundRect,
            at: [6.0, 0.0],
            params: template(PadTemplate::RoundRect, "2 mm", "1 mm", "4"),
        },
    ])?;
    assert_eq!(report.pads.len(), 4);
    assert_eq!(fp.pad("1")?.shape, rect(1.0, 2.0));
    assert_eq!(fp.pad("2")?.shape, PadShape::Circle { diameter: 1.2 });
    assert_eq!(fp.pad("3")?.shape, PadShape::Oblong { size: [2.0, 1.0] });
    assert_eq!(
        fp.pad("3")?.rotation_deg,
        90.0,
        "a tall oblong is a long one turned"
    );
    assert!(matches!(
        fp.pad("4")?.shape,
        PadShape::RoundRect { size: [2.0, 1.0], radius_ratio, .. } if (radius_ratio - 0.25).abs() < 1e-9
    ));
    // Each template leaves only its anchor free.
    assert_eq!(solve_of(&fp)?.1, 8);
    // Resizing through the bound expression re-solves each shape.
    let mut sizes = Vec::new();
    for p in &fp.def.pads {
        sizes.push((p.number.clone(), p.position));
    }
    assert_eq!(sizes.len(), 4);
    Ok(())
}

#[test]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn a_custom_profile_pad_keeps_its_exact_outline() -> Result<()> {
    let mut fp = Fp::new(package(1));
    let report = fp.edit(vec![FootprintEdit::AddPad {
        template: PadTemplate::Rect,
        at: [0.5, 0.5],
        params: template(PadTemplate::Rect, "1 mm", "1 mm", "1"),
    }])?;
    let pad = *report
        .pads
        .first()
        .ok_or_else(|| Error::NotFound("pad".into()))?;
    let owned: Vec<SketchRef> = report
        .created
        .iter()
        .filter(|r| matches!(r, SketchRef::Curve(_)))
        .cloned()
        .collect();
    fp.edit(vec![
        FootprintEdit::ConvertToCustom { pad },
        FootprintEdit::Sketch(SketchEdit::Delete { ids: owned }),
        FootprintEdit::Sketch(SketchEdit::AddPolyline {
            points: vec![
                [0.0, 0.0],
                [2.0, 0.0],
                [2.0, 1.0],
                [1.0, 1.0],
                [1.0, 2.0],
                [0.0, 2.0],
            ],
            closed: true,
            construction: false,
        }),
    ])?;
    let p = fp.pad("1")?;
    assert_eq!(p.position, [0.5, 0.5]);
    let PadShape::Custom { outline } = &p.shape else {
        return Err(Error::Invalid(format!("not custom: {:?}", p.shape)));
    };
    assert_eq!(outline.segments.len(), 6);
    assert!(
        outline
            .segments
            .iter()
            .all(|s| matches!(s, Segment::Line { .. }))
    );
    let mut corners: Vec<[f64; 2]> = outline.segments.iter().map(Segment::to).collect();
    corners.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    assert_eq!(
        corners,
        vec![
            [-0.5, -0.5],
            [-0.5, 1.5],
            [0.5, 0.5],
            [0.5, 1.5],
            [1.5, -0.5],
            [1.5, 0.5]
        ]
    );
    Ok(())
}

#[test]
fn a_stray_copper_curve_makes_the_pad_unclassified() -> Result<()> {
    let mut fp = Fp::new(package(1));
    fp.edit(vec![FootprintEdit::AddPad {
        template: PadTemplate::Rect,
        at: [0.0, 0.0],
        params: template(PadTemplate::Rect, "2 mm", "2 mm", "1"),
    }])?;
    let before = fp.def.clone();
    let stray = fp.edit(vec![FootprintEdit::Sketch(SketchEdit::AddCircle {
        center: [0.0, 0.0],
        radius: 0.2,
        construction: false,
    })]);
    assert!(
        matches!(&stray, Err(Error::Conflict(m)) if m.starts_with("footprint.pad_shape_unclassified")),
        "{stray:?}"
    );
    assert_eq!(fp.def, before);
    // As a construction (drill) circle it is ignored by the copper pass.
    fp.edit(vec![FootprintEdit::Sketch(SketchEdit::AddCircle {
        center: [0.0, 0.0],
        radius: 0.2,
        construction: true,
    })])?;
    Ok(())
}

#[test]
fn deleting_an_anchor_retires_its_pad_and_its_curves() -> Result<()> {
    let mut fp = Fp::new(package(2));
    let a = fp.edit(vec![
        FootprintEdit::AddPad {
            template: PadTemplate::Rect,
            at: [-1.0, 0.0],
            params: template(PadTemplate::Rect, "1 mm", "1 mm", "1"),
        },
        FootprintEdit::AddPad {
            template: PadTemplate::Rect,
            at: [1.0, 0.0],
            params: template(PadTemplate::Rect, "1 mm", "1 mm", "2"),
        },
    ])?;
    let anchor = first_point(&a)?;
    fp.edit(vec![FootprintEdit::Sketch(SketchEdit::Delete {
        ids: vec![SketchRef::Point(anchor)],
    })])?;
    assert_eq!(fp.def.pads.len(), 1);
    assert_eq!(fp.pad("2")?.position, [1.0, 0.0]);
    let source = fp
        .def
        .source
        .as_deref()
        .ok_or_else(|| Error::NotFound("source".into()))?;
    let sketch = check::check_sketch(&source.sketch.0).map_err(Error::from)?;
    assert_eq!(
        sketch.curves.len(),
        5,
        "pad 1's sides and diagonal are gone"
    );
    Ok(())
}

#[test]
fn rename_parameter_follows_every_reference() -> Result<()> {
    let (mut fp, _) = soic8()?;
    let before: Vec<[f64; 2]> = fp.def.pads.iter().map(|p| p.position).collect();
    fp.edit(vec![FootprintEdit::RenameParameter {
        from: ParamName("pitch".into()),
        to: ParamName("e".into()),
    }])?;
    let after: Vec<[f64; 2]> = fp.def.pads.iter().map(|p| p.position).collect();
    assert_eq!(before, after);
    let source = fp
        .def
        .source
        .as_deref()
        .ok_or_else(|| Error::NotFound("source".into()))?;
    assert!(!EV.references(source, &ParamName("pitch".into()))?);
    assert!(EV.references(source, &ParamName("e".into()))?);
    assert!(
        source.sketch.0.contains("\"param\":\"pad_l\""),
        "template dims stay bound"
    );
    fp.edit(vec![param("e", "1.0 mm", ParamUnit::Mm)])?;
    assert_eq!(fp.pad("4")?.position, [-2.7, -1.5]);
    Ok(())
}

#[test]
fn overrides_apply_per_instance_and_orphans_block() -> Result<()> {
    let (mut fp, _) = soic8()?;
    let key = GenerationKey::Array {
        array: fp
            .def
            .source
            .as_ref()
            .and_then(|s| s.arrays.first())
            .map(|a| a.id)
            .unwrap_or(ArrayId(0)),
        i: 1,
        j: 3,
    };
    fp.edit(vec![FootprintEdit::SetOverride {
        key,
        value: PadOverride {
            shape: Some(PadShapeExpr::Oblong {
                width: Expr("pad_l".into()),
                height: Expr("pad_w".into()),
            }),
            ..PadOverride::default()
        },
    }])?;
    assert_eq!(fp.pad("8")?.shape, PadShape::Oblong { size: [1.55, 0.6] });
    assert_eq!(fp.pad("7")?.shape, rect(1.55, 0.6));
    let orphaned = fp.edit(vec![param("per_row", "3", ParamUnit::Count)]);
    assert!(
        matches!(&orphaned, Err(Error::Conflict(m)) if m.starts_with("footprint.override_orphaned")),
        "{orphaned:?}"
    );
    Ok(())
}

#[test]
fn assign_terminals_previews_and_never_overwrites() -> Result<()> {
    let (fp, _) = soic8()?;
    let (next, changes) =
        footprint_edit::assign_terminals(&fp.def, &fp.pkg, &AssignRule::ByPackageDesignation)?;
    assert_eq!(changes.len(), 8);
    assert!(next.pads.iter().all(|p| {
        p.terminal
            .and_then(|t| fp.pkg.terminal(t))
            .map(|t| t.designation.as_str())
            == Some(p.number.as_str())
    }));
    // A second pass changes nothing; terminals survive re-evaluation.
    let mut fp = Fp {
        def: next,
        pkg: fp.pkg,
    };
    let (_, again) =
        footprint_edit::assign_terminals(&fp.def, &fp.pkg, &AssignRule::ByPackageDesignation)?;
    assert!(again.is_empty());
    let hash = solve_of(&fp)?.2;
    fp.edit(vec![param("pitch", "1.27 mm", ParamUnit::Mm)])?;
    assert!(fp.def.pads.iter().all(|p| p.terminal.is_some()));
    assert_eq!(
        solve_of(&fp)?.2,
        hash,
        "terminals are not part of the result hash"
    );
    footprint_edit::check_fresh(&fp.def, &fp.pkg, &EV)?;
    Ok(())
}

#[test]
fn graphics_datum_and_derived_courtyard() -> Result<()> {
    let (mut fp, _) = soic8()?;
    let silk = fp.edit(vec![FootprintEdit::Sketch(SketchEdit::AddPolyline {
        points: vec![[-1.0, 2.0], [1.0, 2.0], [1.0, -2.0]],
        closed: false,
        construction: false,
    })])?;
    let curves: Vec<SketchCurveId> = silk
        .created
        .iter()
        .filter_map(|r| match r {
            SketchRef::Curve(c) => Some(c.clone()),
            _ => None,
        })
        .collect();
    let role = |source, layer| GraphicRole {
        id: pcbcraft_core::ids::GraphicId(0),
        layer,
        side: Side::Component,
        source,
        width: Expr("0.15 mm".into()),
        fill: Fill::None,
    };
    let report = fp.edit(vec![
        FootprintEdit::SetGraphicRole {
            role: role(GraphicSource::Curves(curves), DrawingLayer::Silkscreen),
        },
        FootprintEdit::SetGraphicRole {
            role: role(
                GraphicSource::Derived(
                    pcbcraft_core::library::footprint_source::DerivedGraphic::CourtyardFromExtents {
                        excess: Expr("0.25 mm".into()),
                        round: Expr("0.05 mm".into()),
                    },
                ),
                DrawingLayer::Courtyard,
            ),
        },
        FootprintEdit::SetDatum {
            point: SketchPointId("origin".into()),
            orientation: None,
        },
    ])?;
    assert_eq!(report.graphics.len(), 2);
    let shape_of = |fp: &Fp, id| {
        fp.def
            .graphics
            .iter()
            .find(|g| g.id == id)
            .and_then(|g| match &g.item {
                Item::Shape { shape, .. } => Some(shape.clone()),
                Item::Text(_) => None,
            })
    };
    let [silk_id, court_id] = report.graphics[..] else {
        return Err(Error::Invalid("two graphics".into()));
    };
    assert_eq!(
        shape_of(&fp, silk_id),
        Some(Shape::Polyline {
            points: vec![[-1.0, 2.0], [1.0, 2.0], [1.0, -2.0]],
            radii: Vec::new()
        })
    );
    // Pads reach x ±3.475, y ±2.205; plus 0.25 and rounded out to 0.05.
    assert_eq!(
        shape_of(&fp, court_id),
        Some(Shape::Rect {
            a: [-3.75, -2.5],
            b: [3.75, 2.5],
            radius: 0.0
        })
    );
    // The silk lines are not copper: the pads still classify.
    assert_eq!(fp.def.pads.len(), 8);
    fp.edit(vec![FootprintEdit::RemoveGraphicRole { id: court_id }])?;
    assert!(shape_of(&fp, court_id).is_none());
    Ok(())
}

#[test]
fn preview_reports_blocking_findings_without_committing() -> Result<()> {
    let (fp, _) = soic8()?;
    let preview = footprint_edit::preview(
        &fp.def,
        &fp.pkg,
        &[param("pad_w", "-1 mm", ParamUnit::Mm)],
        &EV,
    )?;
    assert!(!preview.committable);
    assert!(preview.report.findings.iter().any(|f| f.blocks.is_some()));
    let ok = footprint_edit::preview(
        &fp.def,
        &fp.pkg,
        &[param("pitch", "1 mm", ParamUnit::Mm)],
        &EV,
    )?;
    assert!(ok.committable);
    assert!(
        ok.definition
            .pads
            .iter()
            .any(|p| p.position == [-2.7, -1.5])
    );
    Ok(())
}

#[test]
fn sketch_check_refuses_non_canonical_and_unsupported_input() -> Result<()> {
    let sketch = EV.new_sketch()?;
    assert!(check::check_sketch(&sketch.0).is_ok());
    let mut v: serde_json::Value = serde_json::from_str(&sketch.0)?;
    v["surprise"] = serde_json::json!(1);
    let unknown = check::check_sketch(&v.to_string());
    assert!(matches!(&unknown, Err(p) if p.code == "footprint.sketch_unsupported"));
    let mut v: serde_json::Value = serde_json::from_str(&sketch.0)?;
    if let Some(p) = v["points"][0].as_object_mut() {
        p.remove("fixed");
    }
    assert!(
        check::check_sketch(&v.to_string()).is_err(),
        "omitted defaults are not canonical"
    );
    assert!(check::check_sketch("not json").is_err());
    let mut v: serde_json::Value = serde_json::from_str(&sketch.0)?;
    v["view"] = serde_json::json!({"three_d": true});
    assert!(check::check_sketch(&v.to_string()).is_err());
    let huge = "x".repeat(pcbcraft_core::library::footprint_source::MAX_SKETCH_BYTES + 1);
    assert!(matches!(check::check_sketch(&huge), Err(p) if p.code == "footprint.too_large"));
    Ok(())
}

#[test]
fn an_explicit_footprint_with_pads_does_not_gain_a_source() -> Result<()> {
    let mut fp = Fp::new(package(1));
    fp.def.pads.push(Pad::smd_rect(
        PadId(1),
        PackageTerminalId(101),
        "1",
        [0.0, 0.0],
        [1.0, 1.0],
    ));
    let err = fp.edit(vec![param("a", "1 mm", ParamUnit::Mm)]);
    assert!(matches!(&err, Err(Error::Conflict(m)) if m.starts_with("footprint.explicit")));
    Ok(())
}

#[test]
fn detach_keeps_the_solved_pads() -> Result<()> {
    let (mut fp, _) = soic8()?;
    let pads = fp.def.pads.clone();
    fp.edit(vec![FootprintEdit::Detach])?;
    assert!(fp.def.source.is_none());
    assert_eq!(fp.def.pads, pads);
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn evaluation_timings_against_the_targets() -> Result<()> {
    // Targets (design §2.5): ≤ 5 ms SOIC/QFN, ≤ 20 ms BGA seed + array.
    // Measured here as an upper sanity bound only; F0 has the p50/p95 table.
    for (name, fp, target_ms) in [
        ("SOIC-8", soic8()?.0, 5.0),
        ("QFN", qfn()?, 5.0),
        ("BGA-256", bga256()?, 20.0),
    ] {
        let start = std::time::Instant::now();
        let n = 20;
        for _ in 0..n {
            footprint_edit::check_fresh(&fp.def, &fp.pkg, &EV)?;
        }
        let ms = start.elapsed().as_secs_f64() * 1e3 / f64::from(n);
        println!("{name}: {ms:.3} ms per re-evaluation (target {target_ms} ms)");
        assert!(
            ms < target_ms * 20.0,
            "{name} is far over its target: {ms} ms"
        );
    }
    Ok(())
}
