//! Parametric footprint family templates (slice F5): pure functions that
//! turn a typed template into ordinary [`FootprintEdit`]s, applied through
//! `library.edit_footprint` exactly like user edits, so the result is an
//! editable sketch with named parameters. Templates add the courtyard and
//! assembly outline as explicit `Derived` graphic roles; nothing is ever
//! inserted automatically elsewhere. Default values are estimated
//! prototype defaults, not copied tables or a compliance claim.
//!
//!
//! A build is a short sequence of steps. Each step's edits are a pure
//! function of the template and of the ids earlier steps created (pad
//! anchors, drill and paste-window curves, the array), because sketch ids
//! are chosen by the sketch and never invented by the caller. The steps
//! run on one clone and the last one evaluates and commits, so the whole
//! template is one `library.footprint_from_template` undo step; a failure
//! anywhere leaves the draft unchanged.
//!
//! Positions are driven by dimensions bound to named parameters (`seed_x`,
//! `pad_x`, …) measured from the sketch origin, so editing a parameter
//! re-solves the footprint. Ratios (`rr`, `paste_cov`) are inlined into
//! the expressions that use them, because parameters carry a length,
//! angle or count unit.
use crate::diagnostics::{Error, Result};
use crate::geometry::Point;
use crate::ids::PadId;
use crate::library::MAX_PASTE_WINDOWS;
use crate::library::PackageDefinition;
use crate::library::footprint::{FootprintDefinition, PadKind, PadSide};
use crate::library::footprint_edit::{self, FootprintEditReport};
use crate::library::footprint_source::{
    ArrayId, ArrayLayout, DerivedGraphic, DimKind, DimValue, Expr, FootprintEdit,
    FootprintEvaluator, GenerationKey, GeomConstraint, GraphicRole, GraphicSource, HoleRole,
    NumberingScheme, PadOverride, PadShapeExpr, PadTemplate, PadTemplateParams, ParamName,
    ParamUnit, PasteRuleExpr, Pin1Position, SketchCurveId, SketchEdit, SketchPointId, SketchRef,
    SkipRule, ViaGridRole,
};
use crate::library::graphics::{DrawingLayer, Fill, Polarity, Side};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The courtyard excess a template's `Derived` courtyard role uses unless
/// the author changes its parameter, in mm (estimated default).
pub const TEMPLATE_COURTYARD_EXCESS: &str = "0.25";
/// The grid a derived courtyard rounds outward to, in mm.
pub const TEMPLATE_COURTYARD_ROUND: &str = "0.05";
/// Courtyard stroke, in mm.
pub const TEMPLATE_COURTYARD_WIDTH: &str = "0.05";
/// Assembly outline stroke, in mm.
pub const TEMPLATE_ASSEMBLY_WIDTH: &str = "0.1";
/// The pin-1 dot: radius and stroke (filled), in mm, and how far its
/// centre sits beyond the pad's outer end (estimated defaults that keep it
/// clear of the land).
pub const PIN1_DOT_RADIUS_MM: f64 = 0.15;
pub const PIN1_DOT_WIDTH: &str = "0.1";
pub const PIN1_DOT_CLEARANCE: &str = "0.4";
/// Paste windows per side of an exposed pad.
pub const MAX_PASTE_GRID: u16 = 8;

/// Which family a template builds.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TemplateKind {
    /// Two-terminal chip, MELF or moulded body.
    Chip,
    /// SOIC/SOP/SSOP/TSSOP gull-wing: two facing rows.
    GullWing,
    /// QFN/DFN: four (or two) sides plus an exposed pad.
    Qfn,
    /// Through-hole pin header or DIP.
    PinHeader,
}

impl TemplateKind {
    pub const ALL: [Self; 4] = [Self::Chip, Self::GullWing, Self::Qfn, Self::PinHeader];

    /// The picker label.
    pub fn label(self) -> &'static str {
        match self {
            Self::Chip => "2-terminal chip",
            Self::GullWing => "SOIC / SOP",
            Self::Qfn => "QFN / DFN",
            Self::PinHeader => "Pin header",
        }
    }
}

/// A 2-terminal chip land pattern. Every size is an expression over the
/// template's own parameters and the package's `pkg_*` values.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ChipTemplate {
    /// Land length along the body axis (`pad_l`).
    pub pad_l: Expr,
    /// Land width across the body axis (`pad_w`).
    pub pad_w: Expr,
    /// Gap between the two lands (`gap`).
    pub gap: Expr,
    /// Rounded-rectangle corner ratio, 0–0.5 (`rr`).
    pub rr: Expr,
    /// Courtyard excess around pads and envelope (`courtyard_excess`).
    pub courtyard_excess: Expr,
    /// Add a pin-1 polarity marker (polarised chips: diodes, tantalum).
    pub polarity: bool,
}

/// Gull-wing dual-row (SOIC-like): a seed pad plus a `DualRow` array with
/// counter-clockwise numbering and a pin-1 marker.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct GullWingTemplate {
    /// Pad pitch (`pitch`, default `pkg_e` once packages carry it).
    pub pitch: Expr,
    /// Pads per row (`per_row`).
    pub per_row: Expr,
    /// Centre-to-centre distance between the rows (`row_span`).
    pub row_span: Expr,
    pub pad_l: Expr,
    pub pad_w: Expr,
    pub courtyard_excess: Expr,
}

/// QFN/DFN: a `Quad` array placed per side (so `nx ≠ ny` works; `ny = 0`
/// is a DFN), an exposed pad whose terminal is chosen explicitly, paste
/// windows and optional thermal vias.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct QfnTemplate {
    pub pitch: Expr,
    /// Pads on each horizontal side.
    pub nx: Expr,
    /// Pads on each vertical side.
    pub ny: Expr,
    /// Pad-centre span between the left and right rows.
    pub span_x: Expr,
    /// Pad-centre span between the bottom and top rows.
    pub span_y: Expr,
    pub pad_l: Expr,
    pub pad_w: Expr,
    /// Exposed pad size; `None` leaves it out.
    pub exposed: Option<ExposedPad>,
    pub courtyard_excess: Expr,
}

/// The exposed (thermal) pad of a QFN template.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ExposedPad {
    pub width: Expr,
    pub height: Expr,
    /// Paste window grid `[rows, cols]`; the windows cover `paste_cov` of
    /// the pad area.
    pub paste_grid: [u16; 2],
    /// Paste coverage, 0–1 (`paste_cov`).
    pub paste_cov: Expr,
    /// Thermal via grid; `None` adds none.
    pub vias: Option<ExposedVias>,
}

/// Pad-owned thermal vias on the exposed pad.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ExposedVias {
    pub rows: Expr,
    pub cols: Expr,
    pub pitch: Expr,
    pub drill: Expr,
}

/// Through-hole header or DIP: a grid or dual row of plated pads; pin 1
/// differs through an explicit array override, never by inference.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct HeaderTemplate {
    pub pitch: Expr,
    pub rows: Expr,
    pub per_row: Expr,
    /// Distance between rows.
    pub row_pitch: Expr,
    pub drill: Expr,
    /// Copper land diameter.
    pub land: Expr,
    pub numbering: NumberingScheme,
    /// Pin 1 as a rounded rectangle (through a `SetOverride`).
    pub square_pin1: bool,
}

/// One family template with its driving values.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum FootprintTemplate {
    Chip(ChipTemplate),
    GullWing(GullWingTemplate),
    Qfn(QfnTemplate),
    PinHeader(HeaderTemplate),
}

fn e(text: &str) -> Expr {
    Expr(text.into())
}

/// `expr` when the package's `n` terminals divide into `divisor` groups;
/// fewer terminals than groups get the family's usual `family` count
/// (the form then warns that the counts differ), else the rounded-up
/// share.
fn count_or(n: usize, divisor: usize, expr: &str, family: usize) -> Expr {
    if n > 0 && n.is_multiple_of(divisor) {
        e(expr)
    } else if n < divisor {
        Expr(family.to_string())
    } else {
        Expr(n.div_ceil(divisor).max(1).to_string())
    }
}

impl FootprintTemplate {
    pub fn kind(&self) -> TemplateKind {
        match self {
            Self::Chip(_) => TemplateKind::Chip,
            Self::GullWing(_) => TemplateKind::GullWing,
            Self::Qfn(_) => TemplateKind::Qfn,
            Self::PinHeader(_) => TemplateKind::PinHeader,
        }
    }

    /// The template form prefilled for `kind` from the package: sizes are
    /// expressions over `pkg_L`, `pkg_W` and `pkg_n` where the package
    /// gives them, else estimated literals (a package without a body size
    /// or with a terminal count that does not divide gets literal values).
    /// Estimated prototype defaults, not copied tables or a compliance
    /// claim.
    pub fn defaults(kind: TemplateKind, package: &PackageDefinition) -> Self {
        let excess = e(TEMPLATE_COURTYARD_EXCESS);
        let [l, w, _] = package.body_size;
        let sized = |v: f64| v.is_finite() && v > 0.0;
        let n = package.terminals.len();
        match kind {
            TemplateKind::Chip => Self::Chip(ChipTemplate {
                pad_l: e("0.6"),
                pad_w: if sized(w) { e("pkg_W") } else { e("0.8") },
                // The lands overlap the body ends and reach 0.3 mm past
                // them.
                gap: if sized(l) && l > 0.9 {
                    e("pkg_L - 2 * (pad_l - 0.3)")
                } else {
                    e("0.4")
                },
                rr: e("0.25"),
                courtyard_excess: excess,
                polarity: false,
            }),
            TemplateKind::GullWing => Self::GullWing(GullWingTemplate {
                pitch: e("1.27"),
                per_row: count_or(n, 2, "pkg_n / 2", 4),
                row_span: if sized(w) { e("pkg_W + 1.5") } else { e("5.4") },
                pad_l: e("1.55"),
                pad_w: e("0.6"),
                courtyard_excess: excess,
            }),
            TemplateKind::Qfn => {
                // A terminal count of 4k + 1 is 4k perimeter pads plus the
                // exposed pad.
                let (perimeter, side) = if n % 4 == 1 {
                    (n - 1, "(pkg_n - 1) / 4")
                } else {
                    (n, "pkg_n / 4")
                };
                let per_side = count_or(perimeter, 4, side, 4);
                let body = |v: f64, axis: &str| {
                    if sized(v) {
                        e(&format!("pkg_{axis} + 2 * 0.3 - pad_l"))
                    } else {
                        e("3.5")
                    }
                };
                let ep = |v: f64, axis: &str| {
                    if sized(v) && v > 2.0 {
                        e(&format!("pkg_{axis} - 1.7"))
                    } else {
                        e("1.0")
                    }
                };
                Self::Qfn(QfnTemplate {
                    pitch: e("0.5"),
                    nx: per_side.clone(),
                    ny: per_side,
                    span_x: body(l, "L"),
                    span_y: body(w, "W"),
                    pad_l: e("0.8"),
                    pad_w: e("0.25"),
                    exposed: Some(ExposedPad {
                        width: ep(l, "L"),
                        height: ep(w, "W"),
                        paste_grid: [2, 2],
                        paste_cov: e("0.6"),
                        vias: None,
                    }),
                    courtyard_excess: excess,
                })
            }
            TemplateKind::PinHeader => Self::PinHeader(HeaderTemplate {
                pitch: e("2.54"),
                rows: e("1"),
                per_row: if n > 0 { e("pkg_n") } else { e("1") },
                row_pitch: e("2.54"),
                drill: e("1.0"),
                land: e("1.7"),
                numbering: NumberingScheme::Sequential,
                square_pin1: true,
            }),
        }
    }

    /// Every driving expression of the form, for the syntax check.
    fn exprs(&self) -> Vec<&Expr> {
        match self {
            Self::Chip(t) => vec![&t.pad_l, &t.pad_w, &t.gap, &t.rr, &t.courtyard_excess],
            Self::GullWing(t) => vec![
                &t.pitch,
                &t.per_row,
                &t.row_span,
                &t.pad_l,
                &t.pad_w,
                &t.courtyard_excess,
            ],
            Self::Qfn(t) => {
                let mut out = vec![
                    &t.pitch,
                    &t.nx,
                    &t.ny,
                    &t.span_x,
                    &t.span_y,
                    &t.pad_l,
                    &t.pad_w,
                    &t.courtyard_excess,
                ];
                if let Some(ep) = &t.exposed {
                    out.extend([&ep.width, &ep.height, &ep.paste_cov]);
                    if let Some(v) = &ep.vias {
                        out.extend([&v.rows, &v.cols, &v.pitch, &v.drill]);
                    }
                }
                out
            }
            Self::PinHeader(t) => vec![
                &t.pitch,
                &t.rows,
                &t.per_row,
                &t.row_pitch,
                &t.drill,
                &t.land,
            ],
        }
    }

    /// Core checks before any edit: expression syntax and the paste grid.
    fn check(&self) -> Result<()> {
        if let Some(bad) = self.exprs().into_iter().find(|x| !x.valid()) {
            return Err(Error::Invalid(format!(
                "footprint.template_invalid: `{}` is not an expression (non-blank printable text of at most 256 bytes)",
                bad.0.escape_debug()
            )));
        }
        if let Self::Qfn(QfnTemplate {
            exposed: Some(ep), ..
        }) = self
        {
            let [rows, cols] = ep.paste_grid;
            let windows = usize::from(rows) * usize::from(cols);
            if rows > MAX_PASTE_GRID || cols > MAX_PASTE_GRID || windows > MAX_PASTE_WINDOWS {
                return Err(Error::Limit(format!(
                    "footprint.template_invalid: a paste grid has at most {MAX_PASTE_GRID} windows per side"
                )));
            }
        }
        Ok(())
    }
}

/// Build `template` into `footprint`, an empty footprint draft (no pads
/// and no parametric content yet), landing `package`: the template's named
/// parameters, its pads and arrays, the pin-1 polarity with its silkscreen
/// dot (more than two pads, or a polarised chip), and explicit `Derived`
/// courtyard and assembly roles. Terminals stay unassigned; the exposed
/// pad's terminal and every other terminal are explicit choices
/// (`library.assign_terminals`). Pure in its inputs; `footprint` is never
/// changed, and any failure returns an error and nothing.
pub fn build(
    footprint: &FootprintDefinition,
    package: &PackageDefinition,
    template: &FootprintTemplate,
    evaluator: &dyn FootprintEvaluator,
) -> Result<(FootprintDefinition, FootprintEditReport)> {
    template.check()?;
    let occupied = !footprint.pads.is_empty()
        || footprint.source.as_deref().is_some_and(|s| {
            !(s.pads.is_empty()
                && s.arrays.is_empty()
                && s.parameters.is_empty()
                && s.graphics.is_empty())
        });
    if occupied {
        return Err(Error::Conflict(
            "footprint.template_not_empty: a template builds into an empty footprint draft; this one already has pads or parametric content".into(),
        ));
    }
    let mut b = Builder {
        def: footprint.clone(),
        package,
        evaluator,
        report: FootprintEditReport::default(),
    };
    let pin1 = match template {
        FootprintTemplate::Chip(t) => chip(&mut b, t)?,
        FootprintTemplate::GullWing(t) => gull_wing(&mut b, t)?,
        FootprintTemplate::Qfn(t) => qfn(&mut b, t)?,
        FootprintTemplate::PinHeader(t) => header(&mut b, t)?,
    };
    let excess = match template {
        FootprintTemplate::Chip(t) => t.courtyard_excess.clone(),
        FootprintTemplate::GullWing(t) => t.courtyard_excess.clone(),
        FootprintTemplate::Qfn(t) => t.courtyard_excess.clone(),
        FootprintTemplate::PinHeader(_) => e(TEMPLATE_COURTYARD_EXCESS),
    };
    let mut last = vec![param("courtyard_excess", excess, ParamUnit::Mm)];
    last.extend(derived_roles());
    let marker = match &pin1 {
        Some(dot) => {
            last.push(dot_role(dot.curve.clone()));
            Some(dot.pad)
        }
        None => None,
    };
    let (mut def, report) = b.finish(last)?;
    keep_designator_off_copper(&mut def);
    if let Some(pad) = marker {
        // The dot is the newest graphic role; the pad is the authority.
        let markers = report.graphics.last().copied().into_iter().collect();
        def.polarity = Some(Polarity { pad, markers });
    }
    Ok((def, report))
}

/// Move a silkscreen designator that now sits over the built pads to just
/// above them, so the legend never lands on copper (the draft's text
/// predates the template's pads).
fn keep_designator_off_copper(def: &mut FootprintDefinition) {
    use crate::library::graphics::{Item, TextContent};
    let Some(pads) = def
        .pads
        .iter()
        .map(|p| p.bounds())
        .reduce(crate::geometry::Bounds::union)
    else {
        return;
    };
    for graphic in &mut def.graphics {
        let Item::Text(text) = &mut graphic.item else {
            continue;
        };
        if graphic.layer != DrawingLayer::Silkscreen || text.content != TextContent::Designator {
            continue;
        }
        let half = text.height.max(0.0) / 2.0;
        let extent = crate::geometry::Bounds::point(text.at).inflated(half);
        if extent.intersects(&pads) {
            text.at = [
                pads.center()[0],
                pads.max[1] + half + crate::library::graphics::SILK_STROKE_MM * 2.0,
            ];
        }
    }
}

/// The clone being built and what its steps reported.
struct Builder<'a> {
    def: FootprintDefinition,
    package: &'a PackageDefinition,
    evaluator: &'a dyn FootprintEvaluator,
    report: FootprintEditReport,
}

impl Builder<'_> {
    fn merge(&mut self, step: &FootprintEditReport) {
        self.report.created.extend(step.created.iter().cloned());
        self.report.pads.extend(step.pads.iter().copied());
        self.report.graphics.extend(step.graphics.iter().copied());
        self.report.arrays.extend(step.arrays.iter().copied());
    }

    /// Apply one step without evaluating; its own report.
    fn step(&mut self, edits: Vec<FootprintEdit>) -> Result<FootprintEditReport> {
        let (def, step) =
            footprint_edit::apply_unevaluated(&self.def, self.package, &edits, self.evaluator)?;
        self.def = def;
        self.merge(&step);
        Ok(step)
    }

    /// The last step: evaluate once and commit everything.
    fn finish(
        mut self,
        edits: Vec<FootprintEdit>,
    ) -> Result<(FootprintDefinition, FootprintEditReport)> {
        let (def, step) = footprint_edit::edit(&self.def, self.package, &edits, self.evaluator)?;
        self.merge(&step);
        self.report.findings = step.findings;
        Ok((def, self.report))
    }

    /// The anchor point of a role pad the build added.
    fn anchor(&self, pad: PadId) -> Result<SketchPointId> {
        self.def
            .source
            .as_deref()
            .and_then(|s| s.pads.iter().find(|r| r.pad == pad))
            .map(|r| r.anchor.clone())
            .ok_or_else(|| Error::NotFound(format!("template pad {pad} has no role")))
    }

    /// Add one template pad; its id and anchor.
    fn pad(
        &mut self,
        template: PadTemplate,
        at: Point,
        params: PadTemplateParams,
    ) -> Result<(PadId, SketchPointId)> {
        let report = self.step(vec![FootprintEdit::AddPad {
            template,
            at,
            params,
        }])?;
        let pad = only(&report.pads, "pad")?;
        Ok((pad, self.anchor(pad)?))
    }
}

fn only<T: Copy>(items: &[T], what: &str) -> Result<T> {
    match items {
        [one] => Ok(*one),
        _ => Err(Error::Invalid(format!(
            "footprint.template_internal: expected one new {what}, got {}",
            items.len()
        ))),
    }
}

fn created_points(report: &FootprintEditReport) -> Vec<SketchPointId> {
    report
        .created
        .iter()
        .filter_map(|r| match r {
            SketchRef::Point(p) => Some(p.clone()),
            _ => None,
        })
        .collect()
}

fn created_curves(report: &FootprintEditReport) -> Vec<SketchCurveId> {
    report
        .created
        .iter()
        .filter_map(|r| match r {
            SketchRef::Curve(c) => Some(c.clone()),
            _ => None,
        })
        .collect()
}

fn origin() -> SketchRef {
    SketchRef::Point(SketchPointId("origin".into()))
}

fn point(p: &SketchPointId) -> SketchRef {
    SketchRef::Point(p.clone())
}

fn curve(c: &SketchCurveId) -> SketchRef {
    SketchRef::Curve(c.clone())
}

/// `SetParameter` for a template value.
pub fn param(name: &str, expr: Expr, unit: ParamUnit) -> FootprintEdit {
    FootprintEdit::SetParameter {
        name: ParamName(name.into()),
        expr,
        unit,
    }
}

fn mm(name: &str, text: &str) -> FootprintEdit {
    param(name, e(text), ParamUnit::Mm)
}

fn dim(kind: DimKind, refs: Vec<SketchRef>, name: &str) -> FootprintEdit {
    FootprintEdit::Sketch(SketchEdit::AddDimension {
        kind,
        refs,
        value: DimValue::Param(ParamName(name.into())),
    })
}

fn constrain(kind: GeomConstraint, refs: Vec<SketchRef>) -> FootprintEdit {
    FootprintEdit::Sketch(SketchEdit::AddConstraint { kind, refs })
}

/// Locate point `p` from `from` by an x and a y distance bound to
/// parameters (unsigned: the side is the one the point starts on), or
/// aligned with `from` on an axis whose parameter is `None`.
pub fn locate(
    from: SketchRef,
    p: &SketchPointId,
    x: Option<&str>,
    y: Option<&str>,
) -> Vec<FootprintEdit> {
    let mut out = Vec::new();
    match x {
        Some(name) => out.push(dim(
            DimKind::HorizontalDistance,
            vec![from.clone(), point(p)],
            name,
        )),
        None => out.push(constrain(
            GeomConstraint::Vertical,
            vec![from.clone(), point(p)],
        )),
    }
    match y {
        Some(name) => out.push(dim(DimKind::VerticalDistance, vec![from, point(p)], name)),
        None => out.push(constrain(GeomConstraint::Horizontal, vec![from, point(p)])),
    }
    out
}

/// Locate a seed anchor from the origin by `seed_x` and `seed_y`. An axis
/// whose value is zero at build (one pad in that direction) goes through
/// a construction reference point 1 mm before the anchor, at
/// `seed_<axis>_ref = seed_<axis> + 1` from the origin: Solidcraft's
/// distances are unsigned, so a distance that starts at zero could not
/// later grow, and the seed must still re-centre when the count changes.
fn place_seed(
    b: &mut Builder<'_>,
    anchor: &SketchPointId,
    guess: Point,
    zero: [bool; 2],
) -> Result<Vec<FootprintEdit>> {
    let mut out = Vec::new();
    for (axis, (name, kind, align)) in [
        (
            "seed_x",
            DimKind::HorizontalDistance,
            GeomConstraint::Horizontal,
        ),
        (
            "seed_y",
            DimKind::VerticalDistance,
            GeomConstraint::Vertical,
        ),
    ]
    .into_iter()
    .enumerate()
    {
        if !zero.get(axis).copied().unwrap_or(false) {
            out.push(dim(kind, vec![origin(), point(anchor)], name));
            continue;
        }
        let mut at = guess;
        if let Some(v) = at.get_mut(axis) {
            *v -= 1.0;
        }
        let drawn = b.step(vec![FootprintEdit::Sketch(SketchEdit::AddPoint { at })])?;
        let reference = created_points(&drawn).into_iter().next().ok_or_else(|| {
            Error::Invalid("footprint.template_internal: no reference point".into())
        })?;
        let shifted = format!("{name}_ref");
        out.extend([
            mm(&shifted, &format!("{name} + 1")),
            constrain(align, vec![point(anchor), point(&reference)]),
            FootprintEdit::Sketch(SketchEdit::AddDimension {
                kind,
                refs: vec![point(&reference), point(anchor)],
                value: DimValue::Fixed(1.0),
            }),
            dim(kind, vec![origin(), point(&reference)], &shifted),
        ]);
    }
    Ok(out)
}

/// The explicit courtyard (pads and envelope grown by `courtyard_excess`)
/// and assembly (package envelope) roles every template adds.
pub fn derived_roles() -> Vec<FootprintEdit> {
    let role = |layer, source, width: &str| FootprintEdit::SetGraphicRole {
        role: GraphicRole {
            // A new role: the engine allocates its id.
            id: crate::ids::GraphicId(0),
            layer,
            side: Side::Component,
            source: GraphicSource::Derived(source),
            width: e(width),
            fill: Fill::None,
        },
    };
    vec![
        role(
            DrawingLayer::Courtyard,
            DerivedGraphic::CourtyardFromExtents {
                excess: e("courtyard_excess"),
                round: e(TEMPLATE_COURTYARD_ROUND),
            },
            TEMPLATE_COURTYARD_WIDTH,
        ),
        role(
            DrawingLayer::Assembly,
            DerivedGraphic::AssemblyFromEnvelope,
            TEMPLATE_ASSEMBLY_WIDTH,
        ),
    ]
}

fn dot_role(circle: SketchCurveId) -> FootprintEdit {
    FootprintEdit::SetGraphicRole {
        role: GraphicRole {
            id: crate::ids::GraphicId(0),
            layer: DrawingLayer::Silkscreen,
            side: Side::Component,
            source: GraphicSource::Curves(vec![circle]),
            width: e(PIN1_DOT_WIDTH),
            fill: Fill::Solid,
        },
    }
}

/// The pin-1 pad and its silkscreen dot's circle.
struct Pin1 {
    pad: PadId,
    curve: SketchCurveId,
}

/// Where the pin-1 dot sits relative to its pad's anchor.
#[derive(Clone, Copy)]
enum Beyond {
    /// Left of the pad (`-x`).
    Left,
    /// Below the pad (`-y`).
    Below,
}

/// A filled silkscreen dot beyond the pin-1 pad's outer end, `half` (a
/// parameter expression: the pad's half length on that axis) plus
/// [`PIN1_DOT_CLEARANCE`] from its anchor.
fn pin1_dot(
    b: &mut Builder<'_>,
    pad: PadId,
    anchor: &SketchPointId,
    guess: Point,
    beyond: Beyond,
    half: &str,
) -> Result<Pin1> {
    let at = match beyond {
        Beyond::Left => [guess[0] - 1.5, guess[1]],
        Beyond::Below => [guess[0], guess[1] - 1.5],
    };
    let drawn = b.step(vec![FootprintEdit::Sketch(SketchEdit::AddCircle {
        center: at,
        radius: PIN1_DOT_RADIUS_MM,
        construction: false,
    })])?;
    let centre = created_points(&drawn).into_iter().next().ok_or_else(|| {
        Error::Invalid("footprint.template_internal: the dot has no centre".into())
    })?;
    let circle = created_curves(&drawn)
        .into_iter()
        .next()
        .ok_or_else(|| Error::Invalid("footprint.template_internal: no dot circle".into()))?;
    let mut edits = vec![mm("pin1_dot", &format!("{half} + {PIN1_DOT_CLEARANCE}"))];
    edits.extend(match beyond {
        Beyond::Left => locate(point(anchor), &centre, Some("pin1_dot"), None),
        Beyond::Below => locate(point(anchor), &centre, None, Some("pin1_dot")),
    });
    edits.push(FootprintEdit::Sketch(SketchEdit::AddDimension {
        kind: DimKind::Radius,
        refs: vec![curve(&circle)],
        value: DimValue::Fixed(PIN1_DOT_RADIUS_MM),
    }));
    b.step(edits)?;
    Ok(Pin1 { pad, curve: circle })
}

fn smd(width: &str, height: &str, ratio: Option<Expr>, number: &str) -> PadTemplateParams {
    PadTemplateParams {
        width: e(width),
        height: e(height),
        radius_ratio: ratio,
        kind: PadKind::Smd,
        side: PadSide::Top,
        terminal: None,
        number: number.into(),
    }
}

/// A literal that is exactly zero (a ratio of 0 makes a plain rectangle).
fn is_zero(x: &Expr) -> bool {
    x.0.trim().parse::<f64>().is_ok_and(|v| v == 0.0)
}

fn is_literal(x: &Expr, v: f64) -> bool {
    x.0.trim().parse::<f64>().is_ok_and(|n| n == v)
}

/// A fixed construction line from `a` to `b`: the direction of a
/// dual-row array. Its fixing constraints are returned for the next step.
fn axis_line(
    b: &mut Builder<'_>,
    from: Point,
    to: Point,
) -> Result<(SketchCurveId, Vec<FootprintEdit>)> {
    let drawn = b.step(vec![FootprintEdit::Sketch(SketchEdit::AddLine {
        a: from,
        b: to,
        construction: true,
    })])?;
    let line = created_curves(&drawn)
        .into_iter()
        .next()
        .ok_or_else(|| Error::Invalid("footprint.template_internal: no direction line".into()))?;
    let fixed = created_points(&drawn)
        .iter()
        .map(|p| constrain(GeomConstraint::Fixed, vec![point(p)]))
        .collect();
    Ok((line, fixed))
}

fn add_array(seed: PadId, layout: ArrayLayout, numbering: NumberingScheme) -> FootprintEdit {
    FootprintEdit::AddArray {
        seed,
        layout,
        numbering,
        skip: SkipRule::default(),
        first_number: String::new(),
    }
}

/// Pads 1 and 2 on the x axis, centred on the origin.
fn chip(b: &mut Builder<'_>, t: &ChipTemplate) -> Result<Option<Pin1>> {
    b.step(vec![
        param("pad_l", t.pad_l.clone(), ParamUnit::Mm),
        param("pad_w", t.pad_w.clone(), ParamUnit::Mm),
        param("gap", t.gap.clone(), ParamUnit::Mm),
        mm("pad_x", "(gap + pad_l) / 2"),
    ])?;
    let (template, ratio) = if is_zero(&t.rr) {
        (PadTemplate::Rect, None)
    } else {
        (PadTemplate::RoundRect, Some(t.rr.clone()))
    };
    let (one, a1) = b.pad(
        template,
        [-1.0, 0.0],
        smd("pad_l", "pad_w", ratio.clone(), "1"),
    )?;
    let (_, a2) = b.pad(template, [1.0, 0.0], smd("pad_l", "pad_w", ratio, "2"))?;
    let mut edits = locate(origin(), &a1, Some("pad_x"), None);
    edits.extend(locate(origin(), &a2, Some("pad_x"), None));
    b.step(edits)?;
    if t.polarity {
        return pin1_dot(b, one, &a1, [-1.0, 0.0], Beyond::Left, "pad_l / 2").map(Some);
    }
    Ok(None)
}

/// Two rows along the body length (x): a seed at pin 1 (bottom left) and
/// a dual row running right, numbered counter-clockwise (row 1 runs back
/// along the top).
fn gull_wing(b: &mut Builder<'_>, t: &GullWingTemplate) -> Result<Option<Pin1>> {
    b.step(vec![
        param("pitch", t.pitch.clone(), ParamUnit::Mm),
        param("per_row", t.per_row.clone(), ParamUnit::Count),
        param("row_span", t.row_span.clone(), ParamUnit::Mm),
        param("pad_l", t.pad_l.clone(), ParamUnit::Mm),
        param("pad_w", t.pad_w.clone(), ParamUnit::Mm),
        mm("seed_x", "(per_row - 1) / 2 * pitch"),
        mm("seed_y", "row_span / 2"),
    ])?;
    let guess = [-2.0, -2.5];
    let (seed, anchor) = b.pad(PadTemplate::Rect, guess, smd("pad_w", "pad_l", None, "1"))?;
    let (line, mut edits) = axis_line(b, [-0.5, 0.0], [0.5, 0.0])?;
    let single = is_literal(&t.per_row, 1.0);
    edits.extend(place_seed(b, &anchor, guess, [single, false])?);
    edits.push(add_array(
        seed,
        ArrayLayout::DualRow {
            per_row: e("per_row"),
            pitch: e("pitch"),
            row_span: e("row_span"),
            direction: line,
        },
        NumberingScheme::CounterClockwise,
    ));
    b.step(edits)?;
    pin1_dot(b, seed, &anchor, guess, Beyond::Below, "pad_l / 2").map(Some)
}

/// A quad array placed per side from a seed at pin 1 (the top of the
/// left side, or the left of the bottom side when there are no left and
/// right pads), and an optional exposed pad at the origin with paste
/// windows and vias.
fn qfn(b: &mut Builder<'_>, t: &QfnTemplate) -> Result<Option<Pin1>> {
    b.step(vec![
        param("pitch", t.pitch.clone(), ParamUnit::Mm),
        param("nx", t.nx.clone(), ParamUnit::Count),
        param("ny", t.ny.clone(), ParamUnit::Count),
        param("span_x", t.span_x.clone(), ParamUnit::Mm),
        param("span_y", t.span_y.clone(), ParamUnit::Mm),
        param("pad_l", t.pad_l.clone(), ParamUnit::Mm),
        param("pad_w", t.pad_w.clone(), ParamUnit::Mm),
    ])?;
    let bottom = is_literal(&t.ny, 0.0);
    let (pin1, guess, size, seed_at) = if bottom {
        (
            Pin1Position::Bottom,
            [-1.0, -2.0],
            ("pad_w", "pad_l"),
            ("(nx - 1) / 2 * pitch", "span_y / 2"),
        )
    } else {
        (
            Pin1Position::Left,
            [-2.0, 1.0],
            ("pad_l", "pad_w"),
            ("span_x / 2", "(ny - 1) / 2 * pitch"),
        )
    };
    b.step(vec![mm("seed_x", seed_at.0), mm("seed_y", seed_at.1)])?;
    let (seed, anchor) = b.pad(PadTemplate::Rect, guess, smd(size.0, size.1, None, "1"))?;
    let along = if bottom { &t.nx } else { &t.ny };
    let single = is_literal(along, 1.0);
    let mut edits = place_seed(b, &anchor, guess, [bottom && single, !bottom && single])?;
    edits.push(add_array(
        seed,
        ArrayLayout::Quad {
            nx: e("nx"),
            ny: e("ny"),
            pitch: e("pitch"),
            span_x: e("span_x"),
            span_y: e("span_y"),
            pin1,
        },
        NumberingScheme::CounterClockwise,
    ));
    b.step(edits)?;
    if let Some(ep) = &t.exposed {
        exposed_pad(b, ep)?;
    }
    let beyond = if bottom { Beyond::Below } else { Beyond::Left };
    pin1_dot(b, seed, &anchor, guess, beyond, "pad_l / 2").map(Some)
}

/// The exposed pad at the origin, its paste windows (one rectangle per
/// grid cell, centred in it, together covering `paste_cov` of the pad)
/// and its vias. Its terminal is left for an explicit choice.
fn exposed_pad(b: &mut Builder<'_>, ep: &ExposedPad) -> Result<()> {
    b.step(vec![
        param("ep_w", ep.width.clone(), ParamUnit::Mm),
        param("ep_h", ep.height.clone(), ParamUnit::Mm),
    ])?;
    let (pad, anchor) = b.pad(
        PadTemplate::Rect,
        [0.0, 0.0],
        smd("ep_w", "ep_h", None, "EP"),
    )?;
    b.step(vec![constrain(
        GeomConstraint::Coincident,
        vec![origin(), point(&anchor)],
    )])?;
    let [rows, cols] = ep.paste_grid;
    let mut windows: Vec<SketchCurveId> = Vec::new();
    if rows > 0 && cols > 0 {
        let cov = format!("sqrt({})", ep.paste_cov.0);
        let mut edits = vec![
            mm("paste_w", &format!("ep_w / {cols} * {cov}")),
            mm("paste_h", &format!("ep_h / {rows} * {cov}")),
        ];
        // A cell centre at `f` × the pad size; the window corner on the
        // far side of the pad centre is |f| × size + half the window
        // away, so each distance is positive.
        let factor = |k: u16, n: u16| (2 * i32::from(k) + 1 - i32::from(n), 2 * i32::from(n));
        for j in 0..cols {
            let (a, d) = factor(j, cols);
            edits.push(mm(
                &format!("paste_x{}", j + 1),
                &format!("{} / {d} * ep_w + paste_w / 2", a.abs()),
            ));
        }
        for i in 0..rows {
            let (a, d) = factor(i, rows);
            edits.push(mm(
                &format!("paste_y{}", i + 1),
                &format!("{} / {d} * ep_h + paste_h / 2", a.abs()),
            ));
        }
        b.step(edits)?;
        // A nominal 2 mm pad sizes the first drawing; the dimensions
        // re-solve it, keeping each corner on its side.
        const GUESS: f64 = 2.0;
        for i in 0..rows {
            for j in 0..cols {
                let (ax, dx) = factor(j, cols);
                // Row 1 is the top row.
                let (ay, dy) = factor(i, rows);
                let cx = f64::from(ax) / f64::from(dx) * GUESS;
                let cy = -f64::from(ay) / f64::from(dy) * GUESS;
                let (hw, hh) = (
                    GUESS / f64::from(cols) * 0.35,
                    GUESS / f64::from(rows) * 0.35,
                );
                let drawn = b.step(vec![FootprintEdit::Sketch(SketchEdit::AddRect {
                    a: [cx - hw, cy - hh],
                    b: [cx + hw, cy + hh],
                    construction: true,
                })])?;
                // Corners 0–3 run lower-left, lower-right, upper-right,
                // upper-left; sides 0 (bottom) and 1 (right) follow them.
                let corners = created_points(&drawn);
                let sides = created_curves(&drawn);
                let (Some(bottom), Some(right)) = (sides.first(), sides.get(1)) else {
                    return Err(Error::Invalid(
                        "footprint.template_internal: a paste window has no sides".into(),
                    ));
                };
                let corner = match (ax >= 0, ay <= 0) {
                    (false, false) => 0,
                    (true, false) => 1,
                    (true, true) => 2,
                    (false, true) => 3,
                };
                let at = corners.get(corner).ok_or_else(|| {
                    Error::Invalid(
                        "footprint.template_internal: a paste window has no corners".into(),
                    )
                })?;
                let mut edits = vec![
                    dim(DimKind::HorizontalDistance, vec![curve(bottom)], "paste_w"),
                    dim(DimKind::VerticalDistance, vec![curve(right)], "paste_h"),
                ];
                edits.extend(locate(
                    point(&anchor),
                    at,
                    Some(&format!("paste_x{}", j + 1)),
                    Some(&format!("paste_y{}", i + 1)),
                ));
                b.step(edits)?;
                windows.extend(sides);
            }
        }
    }
    let mut role = b
        .def
        .source
        .as_deref()
        .and_then(|s| s.pads.iter().find(|r| r.pad == pad))
        .cloned()
        .ok_or_else(|| Error::NotFound(format!("template pad {pad} has no role")))?;
    if !windows.is_empty() {
        role.paste = PasteRuleExpr::Windows(windows);
    }
    role.vias = ep.vias.as_ref().map(|v| ViaGridRole {
        rows: v.rows.clone(),
        cols: v.cols.clone(),
        pitch: v.pitch.clone(),
        drill: v.drill.clone(),
    });
    b.step(vec![FootprintEdit::SetPadRole { role }])?;
    Ok(())
}

/// Plated pads with a drill, pin 1 at the top left: a grid (rows across,
/// `per_row` down) or, numbered counter-clockwise, a dual row (DIP).
fn header(b: &mut Builder<'_>, t: &HeaderTemplate) -> Result<Option<Pin1>> {
    let dual = matches!(t.numbering, NumberingScheme::CounterClockwise);
    if dual && !is_literal(&t.rows, 2.0) {
        return Err(Error::Invalid(
            "footprint.template_invalid: counter-clockwise numbering is a dual row; set rows to 2"
                .into(),
        ));
    }
    let one_row = !dual && is_literal(&t.rows, 1.0);
    let single = is_literal(&t.per_row, 1.0);
    b.step(vec![
        param("pitch", t.pitch.clone(), ParamUnit::Mm),
        param("rows", t.rows.clone(), ParamUnit::Count),
        param("per_row", t.per_row.clone(), ParamUnit::Count),
        param("row_pitch", t.row_pitch.clone(), ParamUnit::Mm),
        param("drill", t.drill.clone(), ParamUnit::Mm),
        param("land", t.land.clone(), ParamUnit::Mm),
        mm("seed_x", "(rows - 1) / 2 * row_pitch"),
        mm("seed_y", "(per_row - 1) / 2 * pitch"),
    ])?;
    let guess = [
        if one_row { 0.0 } else { -1.0 },
        if single { 0.0 } else { 2.0 },
    ];
    let (seed, anchor) = b.pad(
        PadTemplate::Circle,
        guess,
        PadTemplateParams {
            kind: PadKind::PlatedThrough,
            ..smd("land", "land", None, "1")
        },
    )?;
    let drawn = b.step(vec![FootprintEdit::Sketch(SketchEdit::AddCircle {
        center: guess,
        radius: 0.3,
        construction: true,
    })])?;
    let (Some(centre), Some(drill)) = (
        created_points(&drawn).into_iter().next(),
        created_curves(&drawn).into_iter().next(),
    ) else {
        return Err(Error::Invalid(
            "footprint.template_internal: no drill circle".into(),
        ));
    };
    let mut edits = vec![
        constrain(
            GeomConstraint::Coincident,
            vec![point(&anchor), point(&centre)],
        ),
        dim(DimKind::Diameter, vec![curve(&drill)], "drill"),
    ];
    let mut role = b
        .def
        .source
        .as_deref()
        .and_then(|s| s.pads.iter().find(|r| r.pad == seed))
        .cloned()
        .ok_or_else(|| Error::NotFound(format!("template pad {seed} has no role")))?;
    role.hole = Some(HoleRole {
        curves: vec![drill],
        plated: true,
    });
    edits.push(FootprintEdit::SetPadRole { role });
    edits.extend(place_seed(b, &anchor, guess, [one_row, single])?);
    let layout = if dual {
        let (line, fixed) = axis_line(b, [0.0, 0.5], [0.0, -0.5])?;
        edits.extend(fixed);
        ArrayLayout::DualRow {
            per_row: e("per_row"),
            pitch: e("pitch"),
            row_span: e("row_pitch"),
            direction: line,
        }
    } else {
        ArrayLayout::Grid {
            rows: e("per_row"),
            cols: e("rows"),
            pitch_x: e("row_pitch"),
            pitch_y: e("pitch"),
            stagger: false,
        }
    };
    edits.push(add_array(seed, layout, t.numbering.clone()));
    let step = b.step(edits)?;
    if t.square_pin1 {
        let array: ArrayId = only(&step.arrays, "array")?;
        b.step(vec![FootprintEdit::SetOverride {
            key: GenerationKey::Array { array, i: 0, j: 0 },
            value: PadOverride {
                shape: Some(PadShapeExpr::RoundRect {
                    width: e("land"),
                    height: e("land"),
                    radius_ratio: e("0.25"),
                }),
                ..PadOverride::default()
            },
        }])?;
    }
    pin1_dot(b, seed, &anchor, guess, Beyond::Left, "land / 2").map(Some)
}

#[cfg(test)]
mod tests;
