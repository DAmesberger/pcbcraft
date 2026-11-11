//! Explicit footprint drawings: the assembly outline from the package
//! envelope, silkscreen broken around the pads' mask openings, a courtyard
//! from the pad and envelope extents, and designator placeholders. The
//! values are conservative, estimated prototype defaults (not copied
//! tables or a standard compliance claim); every result is ordinary
//! graphic data the author then edits. The parametric family templates of
//! the footprint design (F5) emit `FootprintEdit`s and arrive with the
//! evaluator.
use crate::diagnostics::{Error, Result};
use crate::geometry::{Bounds, Point, Shape};
use crate::ids::GraphicId;
use crate::library::graphics::{
    DrawingLayer, FOOTPRINT_TEXT_HEIGHT_MM, Fill, FootprintGraphic, Item, SILK_STROKE_MM, ShapeNm,
    Side, designator_placeholder,
};
use crate::library::{FootprintDefinition, PackageDefinition, Pad, PadDefaults};
use crate::units::{Nm, PointNm};

/// Courtyard margin around pads and envelope, in mm (estimated default).
pub const COURTYARD_EXCESS_MM: f64 = 0.25;
/// Courtyard coordinates round outward to this grid, in mm.
pub const COURTYARD_ROUND_MM: f64 = 0.05;
/// Courtyard stroke, in mm.
pub const COURTYARD_STROKE_MM: f64 = 0.05;
/// Assembly-drawing stroke, in mm.
pub const ASSEMBLY_STROKE_MM: f64 = 0.10;
/// How far the silkscreen outline sits outside the envelope, in mm.
pub const SILK_OUTSET_MM: f64 = 0.15;
/// Silkscreen clearance from a pad's mask opening, in mm (estimated
/// default; fabricators state their own).
pub const SILK_PAD_CLEARANCE_MM: f64 = 0.1;
/// Silkscreen pieces shorter than this are left out, in mm.
pub const MIN_SILK_SEGMENT_MM: f64 = 0.2;

fn round_out(v: f64, step: f64, up: bool) -> f64 {
    if !(step.is_finite() && step > 0.0) {
        return v;
    }
    let q = v / step;
    // Within 1e-9 of a grid line counts as on it.
    let n = if up {
        (q - 1e-9).ceil()
    } else {
        (q + 1e-9).floor()
    };
    n * step
}

/// The courtyard rectangle around `pads` and an optional `outline` (the
/// package envelope or the assembly outline), grown by `excess` and rounded
/// outward to `round_to`. `None` when there is nothing to enclose. Pure: it
/// writes nothing.
pub fn courtyard_from(
    pads: &[Pad],
    outline: Option<Bounds>,
    excess: f64,
    round_to: f64,
) -> Option<Shape> {
    let boxed = pads
        .iter()
        .map(Pad::bounds)
        .chain(outline)
        .reduce(Bounds::union)?
        .inflated(excess);
    Some(Shape::rect(
        [
            round_out(boxed.min[0], round_to, false),
            round_out(boxed.min[1], round_to, false),
        ],
        [
            round_out(boxed.max[0], round_to, true),
            round_out(boxed.max[1], round_to, true),
        ],
    ))
}

/// The silkscreen keep-out of a pad: its box grown by its mask expansion,
/// the clearance and half the stroke.
fn keep_out(pad: &Pad, defaults: &PadDefaults, stroke: f64) -> Bounds {
    let grow = pad.mask_expansion(defaults).unwrap_or(0.0);
    pad.bounds()
        .inflated(grow.max(0.0) + SILK_PAD_CLEARANCE_MM + stroke / 2.0)
}

/// The pieces of segment `a`–`b` (horizontal or vertical) outside every box.
fn clipped(a: Point, b: Point, boxes: &[Bounds]) -> Vec<Shape> {
    let axis = usize::from((a[0] - b[0]).abs() < (a[1] - b[1]).abs());
    let other = 1 - axis;
    let (lo, hi) = (a[axis].min(b[axis]), a[axis].max(b[axis]));
    let mut cuts: Vec<(f64, f64)> = boxes
        .iter()
        .filter(|k| (k.min[other]..=k.max[other]).contains(&a[other]))
        .map(|k| (k.min[axis], k.max[axis]))
        .collect();
    cuts.sort_by(|x, y| x.0.total_cmp(&y.0));
    let mut pieces = Vec::new();
    let mut from = lo;
    for (c0, c1) in cuts {
        if c0 > from {
            pieces.push((from, c0.min(hi)));
        }
        from = from.max(c1);
        if from >= hi {
            break;
        }
    }
    if from < hi {
        pieces.push((from, hi));
    }
    pieces
        .into_iter()
        .filter(|(s, e)| e - s >= MIN_SILK_SEGMENT_MM)
        .map(|(s, e)| {
            let mut p = a;
            let mut q = a;
            p[axis] = s;
            q[axis] = e;
            Shape::Line { a: p, b: q }
        })
        .collect()
}

/// The envelope outline `SILK_OUTSET_MM` outside `envelope`, as lines
/// broken around the pads' mask openings plus clearance.
pub fn silk_outline(footprint: &FootprintDefinition, envelope: Bounds) -> Vec<Shape> {
    let boxes: Vec<Bounds> = footprint
        .pads
        .iter()
        .map(|p| keep_out(p, &footprint.defaults, SILK_STROKE_MM))
        .collect();
    let [c0, c1, c2, c3] = envelope.inflated(SILK_OUTSET_MM).corners();
    [(c0, c1), (c1, c2), (c2, c3), (c3, c0)]
        .into_iter()
        .flat_map(|(a, b)| clipped(a, b, &boxes))
        .collect()
}

/// A computed outline stored on `layer`, rounded to nanometres.
fn shape(layer: DrawingLayer, shape: Shape, width: f64) -> Result<(DrawingLayer, Item)> {
    Ok((
        layer,
        Item::Shape {
            shape: ShapeNm::from_mm_rounded(&shape)?,
            width_nm: Nm::from_mm_f64_rounded(width)?,
            fill: Fill::None,
        },
    ))
}

/// The copper keep-outs a typed form asks of the host board: a module's
/// antenna region ([`ModuleForm::antenna_keepout`]) on every copper layer.
///
/// [`ModuleForm::antenna_keepout`]: crate::library::package_form::ModuleForm::antenna_keepout
pub fn form_keepouts(
    form: &crate::library::package_form::PackageForm,
) -> Vec<crate::library::FootprintKeepout> {
    match form {
        crate::library::package_form::PackageForm::Module(m) => m
            .antenna_keepout()
            .map(|[min, max]| crate::library::FootprintKeepout::rect("antenna", min, max))
            .into_iter()
            .collect(),
        _ => Vec::new(),
    }
}

/// Append the standard drawings to `footprint`, allocating graphic IDs from
/// its `next_local`: the assembly outline and designator, the silkscreen
/// outline and designator, and the courtyard, all on the component side.
/// Polarity is never added (it needs explicit data). The package form's
/// copper keep-outs (a module's antenna clearance) replace the footprint's
/// keep-outs: they are form-derived like the lands. Returns the new IDs in
/// order; on error the footprint is unchanged.
pub fn add_standard_graphics(
    footprint: &mut FootprintDefinition,
    package: &PackageDefinition,
) -> Result<Vec<GraphicId>> {
    let envelope = footprint.envelope_outline(package);
    add_standard_graphics_over(footprint, package, envelope)
}

/// [`add_standard_graphics`] over an explicit body outline in the footprint
/// frame (a mechanical part's bounds, which need not be centred).
pub fn add_standard_graphics_over(
    footprint: &mut FootprintDefinition,
    package: &PackageDefinition,
    envelope: Bounds,
) -> Result<Vec<GraphicId>> {
    let mut items = vec![shape(
        DrawingLayer::Assembly,
        Shape::rect(envelope.min, envelope.max),
        ASSEMBLY_STROKE_MM,
    )?];
    for s in silk_outline(footprint, envelope) {
        items.push(shape(DrawingLayer::Silkscreen, s, SILK_STROKE_MM)?);
    }
    let courtyard = courtyard_from(
        &footprint.pads,
        Some(envelope),
        COURTYARD_EXCESS_MM,
        COURTYARD_ROUND_MM,
    );
    let top = courtyard
        .as_ref()
        .map_or(envelope.max[1], |c| c.bounds(0.0).max[1]);
    if let Some(courtyard) = courtyard {
        items.push(shape(
            DrawingLayer::Courtyard,
            courtyard,
            COURTYARD_STROKE_MM,
        )?);
    }
    let mut next = footprint.clone();
    next.keepouts = form_keepouts(&package.form);
    let mut added = Vec::with_capacity(items.len() + 2);
    let mut allocate = |next: &mut FootprintDefinition| -> Result<GraphicId> {
        let id = GraphicId(next.next_local);
        next.next_local = next
            .next_local
            .checked_add(1)
            .ok_or_else(|| Error::Limit("footprint local identities exhausted".into()))?;
        added.push(id);
        Ok(id)
    };
    for (layer, item) in items {
        let id = allocate(&mut next)?;
        next.graphics.push(FootprintGraphic {
            id,
            layer,
            side: Side::Component,
            item,
        });
    }
    let centre = envelope.center();
    let silk_at = [
        centre[0],
        top + FOOTPRINT_TEXT_HEIGHT_MM / 2.0 + SILK_STROKE_MM,
    ];
    let id = allocate(&mut next)?;
    next.graphics.push(designator_placeholder(
        id,
        PointNm::from_mm_f64_rounded(silk_at)?,
        DrawingLayer::Silkscreen,
    ));
    let id = allocate(&mut next)?;
    let mut assembly = designator_placeholder(
        id,
        PointNm::from_mm_f64_rounded(centre)?,
        DrawingLayer::Assembly,
    );
    if let Item::Text(text) = &mut assembly.item {
        let [w, h] = envelope.size();
        text.height_nm =
            Nm::from_mm_f64_rounded((0.5 * w.min(h)).clamp(0.1, FOOTPRINT_TEXT_HEIGHT_MM))?;
    }
    next.graphics.push(assembly);
    *footprint = next;
    Ok(added)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{PackageTerminalId, PadId};
    use crate::tests::support::{nm, shape_nm};

    fn chip() -> (FootprintDefinition, PackageDefinition) {
        let footprint = FootprintDefinition {
            next_local: 3,
            pads: vec![
                Pad::smd_rect(PadId(1), PackageTerminalId(1), "1", [-1.2, 0.0], [1.0, 1.4]),
                Pad::smd_rect(PadId(2), PackageTerminalId(2), "2", [1.2, 0.0], [1.0, 1.4]),
            ],
            ..FootprintDefinition::default()
        };
        let package = PackageDefinition {
            envelope_nm: [nm(2.0), nm(1.2), nm(0.6)],
            ..PackageDefinition::default()
        };
        (footprint, package)
    }

    fn rect(shape: Option<Shape>) -> Result<(Point, Point)> {
        match shape {
            Some(Shape::Rect { a, b, .. }) => Ok((a, b)),
            other => Err(Error::Invalid(format!("not a rectangle: {other:?}"))),
        }
    }

    #[test]
    fn courtyard_encloses_pads_and_envelope_rounded_outward() -> Result<()> {
        let (footprint, package) = chip();
        let envelope = footprint.envelope_outline(&package);
        // Pads span x ±1.7, y ±0.7; +0.25 → ±1.95, ±0.95 (already on grid).
        let (a, b) = rect(courtyard_from(&footprint.pads, Some(envelope), 0.25, 0.05))?;
        assert!((a[0] + 1.95).abs() < 1e-9 && (b[0] - 1.95).abs() < 1e-9);
        assert!((a[1] + 0.95).abs() < 1e-9 && (b[1] - 0.95).abs() < 1e-9);
        // 1.93 rounds out to 1.95; 0.93 to 0.95.
        let (a, b) = rect(courtyard_from(&footprint.pads, None, 0.23, 0.05))?;
        assert!((b[0] - 1.95).abs() < 1e-9 && (a[1] + 0.95).abs() < 1e-9);
        assert_eq!(courtyard_from(&[], None, 0.25, 0.05), None);
        Ok(())
    }

    #[test]
    fn silkscreen_breaks_around_mask_openings() -> Result<()> {
        let (footprint, package) = chip();
        let lines = silk_outline(&footprint, footprint.envelope_outline(&package));
        // Only the top and bottom runs between the pads survive.
        assert_eq!(lines.len(), 2, "{lines:?}");
        for line in &lines {
            let Shape::Line { a, b } = line else {
                return Err(Error::Invalid(format!("not a line: {line:?}")));
            };
            assert!((a[1] - b[1]).abs() < 1e-12);
            assert!((a[1].abs() - 0.75).abs() < 1e-9, "0.15 mm outside the body");
            // Pad keep-out starts at 0.7 - 0.05 - 0.1 - 0.075 = 0.475.
            assert!((a[0].abs() - 0.475).abs() < 1e-9 && (b[0].abs() - 0.475).abs() < 1e-9);
        }
        Ok(())
    }

    #[test]
    fn lint_flags_pads_outside_the_courtyard_and_silk_over_copper() -> Result<()> {
        let (mut footprint, package) = chip();
        add_standard_graphics(&mut footprint, &package)?;
        let rules = crate::rules::fixtures::rules()?;
        let codes = |f: &FootprintDefinition| -> Vec<String> {
            f.lint_with(crate::ids::DefinitionId(1), &rules)
                .into_iter()
                .map(|f| f.code)
                .collect()
        };
        let mut outside = footprint.clone();
        outside.pads[0].position[0] += 20.0;
        assert!(
            codes(&outside)
                .iter()
                .any(|c| c == "footprint.pad_outside_courtyard")
        );
        let mut silk = footprint.clone();
        let centre = silk.pads[0].position;
        silk.graphics.push(crate::library::FootprintGraphic {
            id: GraphicId(silk.next_local),
            layer: crate::library::DrawingLayer::Silkscreen,
            side: crate::library::graphics::Side::Component,
            item: crate::library::graphics::Item::Shape {
                shape: shape_nm(crate::geometry::Shape::Line {
                    a: [centre[0] - 5.0, centre[1]],
                    b: [centre[0] + 5.0, centre[1]],
                }),
                width_nm: nm(0.15),
                fill: crate::library::graphics::Fill::None,
            },
        });
        silk.next_local += 1;
        assert!(codes(&silk).iter().any(|c| c == "footprint.silk_over_pad"));
        Ok(())
    }

    #[test]
    fn standard_graphics_allocate_ids_and_pass_rules_and_lint() -> Result<()> {
        let (mut footprint, package) = chip();
        let added = add_standard_graphics(&mut footprint, &package)?;
        assert_eq!(added.len(), footprint.graphics.len());
        assert_eq!(added.first(), Some(&GraphicId(3)));
        assert_eq!(footprint.next_local as usize, 3 + footprint.graphics.len());
        let lint = footprint.lint(crate::ids::DefinitionId(1));
        assert!(lint.is_empty(), "{lint:?}");
        let mut out = Vec::new();
        footprint.local_findings(crate::ids::DefinitionId(1), &mut out);
        assert!(out.is_empty(), "{out:?}");
        let mut full = FootprintDefinition {
            next_local: u32::MAX,
            ..footprint.clone()
        };
        let before = full.clone();
        assert!(add_standard_graphics(&mut full, &package).is_err());
        assert_eq!(full, before, "unchanged on error");
        Ok(())
    }

    #[test]
    fn lint_flags_thin_silk_strokes_and_small_text_only() -> Result<()> {
        use crate::library::graphics::{Fill, Item};
        let (mut footprint, package) = chip();
        add_standard_graphics(&mut footprint, &package)?;
        let rules = crate::rules::fixtures::rules()?;
        let codes = |f: &FootprintDefinition| -> Vec<String> {
            f.lint_with(crate::ids::DefinitionId(1), &rules)
                .into_iter()
                .map(|f| f.code)
                .collect()
        };
        assert!(codes(&footprint).is_empty());
        let mut thin = footprint.clone();
        for g in &mut thin.graphics {
            match &mut g.item {
                Item::Shape { width_nm, .. } => *width_nm = nm(0.05),
                Item::Text(text) => text.height_nm = nm(0.5),
            }
        }
        let found = codes(&thin);
        assert!(
            found.iter().any(|c| c == "footprint.silk_width"),
            "{found:?}"
        );
        assert!(
            found.iter().any(|c| c == "footprint.text_height"),
            "{found:?}"
        );
        // The courtyard and assembly strokes are not silkscreen, a filled
        // silkscreen shape prints its area, and the limits themselves pass.
        let silk = |f: &FootprintDefinition| {
            f.graphics
                .iter()
                .filter(|g| {
                    g.layer == DrawingLayer::Silkscreen && matches!(g.item, Item::Shape { .. })
                })
                .count()
        };
        assert_eq!(
            found
                .iter()
                .filter(|c| *c == "footprint.silk_width")
                .count(),
            silk(&thin)
        );
        let mut edge = footprint.clone();
        for g in &mut edge.graphics {
            match &mut g.item {
                Item::Shape { width_nm, fill, .. } if g.layer == DrawingLayer::Silkscreen => {
                    *width_nm = nm(0.0);
                    *fill = Fill::Solid;
                }
                Item::Shape { width_nm, .. } => *width_nm = nm(0.01),
                Item::Text(text) => text.height_nm = nm(crate::library::MIN_SILK_TEXT_HEIGHT_MM),
            }
        }
        let found = codes(&edge);
        assert!(
            !found
                .iter()
                .any(|c| c == "footprint.silk_width" || c == "footprint.text_height"),
            "{found:?}"
        );
        Ok(())
    }
}
