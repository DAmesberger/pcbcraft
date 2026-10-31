//! The footprint publish rules and the advisory footprint lint.
use super::geometry::{CHECK_CHORD_MM, TerminalRule, ring_gap, ring_within};
use super::{FootprintDefinition, HoleShape, Pad, PadKind, PadShape, PadSide, PasteRule};
use crate::diagnostics::{Finding, ObjectRef};
use crate::geometry::{polygon_self_intersects, signed_area2, size_valid};
use crate::ids::DefinitionId;
use crate::library::graphics::{self, DrawingLayer, Fill, Item, MAX_POLARITY_MARKERS, TextContent};
use crate::library::publish::{issue, object};
use std::collections::BTreeSet;

/// Smallest solder-mask web between two openings the lint accepts, in mm
/// (an estimated prototype default, not a fabricator qualification).
pub const MIN_MASK_WEB_MM: f64 = 0.1;
/// Pads on which the lint expects an authored pin-1 polarity.
pub(super) const POLARITY_LINT_PADS: usize = 2;

fn pad_object(id: DefinitionId, pad: &Pad) -> Vec<ObjectRef> {
    vec![object(id, pad.id.0)]
}

/// Shape validity of a pad's copper outline (`footprint.pad_size` for a
/// size out of range, `footprint.pad_shape` for anything else). Returns
/// whether the shape is valid.
fn shape_findings(id: DefinitionId, pad: &Pad, out: &mut Vec<Finding>) -> bool {
    let size_ok = match &pad.shape {
        PadShape::Custom { .. } => true,
        shape => size_valid(&shape.extent()),
    };
    if !size_ok {
        out.push(issue(
            "footprint.pad_size",
            format!("Pad {} needs a positive finite size.", pad.id),
            pad_object(id, pad),
        ));
        return false;
    }
    let problem = match &pad.shape {
        PadShape::RoundRect {
            radius_ratio,
            max_radius,
            ..
        } => {
            if !(0.0..=0.5).contains(radius_ratio) {
                Some("a corner ratio of 0–0.5 of the short side")
            } else if max_radius.is_some_and(|r| r < 0.0) {
                Some("a non-negative corner radius cap")
            } else {
                None
            }
        }
        PadShape::Custom { outline } => {
            let ring = outline.flatten(CHECK_CHORD_MM);
            if !outline.closed() {
                Some("a closed outline (the last segment must end at the start)")
            } else if ring.len() < 3
                || signed_area2(&ring).abs() <= 1e-9
                || !crate::geometry::Bounds::of(ring.iter().copied())
                    .is_some_and(|b| size_valid(&b.size()))
            {
                Some("an outline with area")
            } else if polygon_self_intersects(&ring) {
                Some("an outline that does not cross itself")
            } else {
                None
            }
        }
        _ => None,
    };
    if let Some(need) = problem {
        out.push(issue(
            "footprint.pad_shape",
            format!("Pad {} needs {need}.", pad.id),
            pad_object(id, pad),
        ));
        return false;
    }
    true
}

/// Kind, terminal, side and hole consistency.
fn kind_findings(id: DefinitionId, pad: &Pad, out: &mut Vec<Finding>) {
    match (pad.kind.terminal_rule(), pad.terminal) {
        (TerminalRule::Required, None) => out.push(issue(
            "footprint.terminal_required",
            format!(
                "Pad {} ({:?}) lands no package terminal; choose one, or make it Mechanical explicitly.",
                pad.id, pad.kind
            ),
            pad_object(id, pad),
        )),
        (TerminalRule::Forbidden, Some(terminal)) => out.push(issue(
            "footprint.terminal_forbidden",
            format!(
                "Pad {} ({:?}) has no copper connection and cannot land package terminal {terminal}.",
                pad.id, pad.kind
            ),
            pad_object(id, pad),
        )),
        _ => {}
    }
    if pad.kind.through_hole() && pad.side != PadSide::Top {
        out.push(issue(
            "footprint.pad_side",
            format!(
                "Pad {} spans every copper layer and records the top side.",
                pad.id
            ),
            pad_object(id, pad),
        ));
    }
    let hole = match (pad.kind, &pad.hole) {
        (PadKind::PlatedThrough, Some(h)) if !h.plated => Some("a plated hole"),
        (PadKind::PlatedThrough, None) => Some("a plated hole"),
        (PadKind::NonPlatedHole, Some(h)) if h.plated => Some("an unplated hole"),
        (PadKind::NonPlatedHole, None) => Some("an unplated hole"),
        (PadKind::Smd | PadKind::Fiducial, Some(_)) => Some("no hole"),
        _ => None,
    };
    if let Some(need) = hole {
        out.push(issue(
            "footprint.hole_kind",
            format!("Pad {} ({:?}) needs {need}.", pad.id, pad.kind),
            pad_object(id, pad),
        ));
    }
}

/// Holes, pad vias and paste windows inside the land.
fn containment_findings(id: DefinitionId, pad: &Pad, out: &mut Vec<Finding>) {
    let land = pad.shape.flatten(CHECK_CHORD_MM);
    let tol = 2.0 * CHECK_CHORD_MM;
    if let Some(hole) = &pad.hole {
        let size_ok = match hole.shape {
            HoleShape::Round { diameter } => size_valid(&[diameter]),
            HoleShape::Slot { width, length, .. } => {
                size_valid(&[width, length]) && length >= width
            }
        };
        if !size_ok {
            out.push(issue(
                "footprint.hole_shape",
                format!(
                    "Pad {} needs a positive drill (a slot at least as long as it is wide).",
                    pad.id
                ),
                pad_object(id, pad),
            ));
        } else if !ring_within(&hole.ring(CHECK_CHORD_MM), &land, tol) {
            out.push(issue(
                "footprint.hole_outside_land",
                format!("The hole of pad {} is not inside its land.", pad.id),
                pad_object(id, pad),
            ));
        }
    }
    let mut apertures = BTreeSet::new();
    for via in &pad.vias {
        if !apertures.insert(via.id) {
            out.push(issue(
                "footprint.aperture_duplicate",
                format!("Pad {} uses aperture {} more than once.", pad.id, via.id.0),
                pad_object(id, pad),
            ));
        }
        let drill = super::Hole {
            shape: HoleShape::Round {
                diameter: via.drill,
            },
            plated: true,
            offset: via.at,
        };
        if !size_valid(&[via.drill]) || !ring_within(&drill.ring(CHECK_CHORD_MM), &land, tol) {
            out.push(issue(
                "footprint.hole_outside_land",
                format!(
                    "Via {} of pad {} needs a positive drill inside the land.",
                    via.id.0, pad.id
                ),
                pad_object(id, pad),
            ));
        }
    }
    if let PasteRule::Windows(windows) = &pad.paste {
        for window in windows {
            if !apertures.insert(window.id) {
                out.push(issue(
                    "footprint.aperture_duplicate",
                    format!(
                        "Pad {} uses aperture {} more than once.",
                        pad.id, window.id.0
                    ),
                    pad_object(id, pad),
                ));
            }
            let ring = window.shape.flatten(CHECK_CHORD_MM);
            if !window.shape.is_closed() || ring.len() < 3 || signed_area2(&ring).abs() <= 1e-9 {
                out.push(issue(
                    "footprint.paste_window_shape",
                    format!(
                        "Paste window {} of pad {} needs a closed shape with area.",
                        window.id.0, pad.id
                    ),
                    pad_object(id, pad),
                ));
            } else if !ring_within(&ring, &land, tol) {
                out.push(issue(
                    "footprint.paste_window_outside_pad",
                    format!(
                        "Paste window {} of pad {} is not inside the pad.",
                        window.id.0, pad.id
                    ),
                    pad_object(id, pad),
                ));
            }
        }
    }
}

/// Polarity, footprint graphic and courtyard rules.
fn graphic_findings(footprint: &FootprintDefinition, id: DefinitionId, out: &mut Vec<Finding>) {
    for graphic in &footprint.graphics {
        let this = vec![object(id, graphic.id.0)];
        graphics::item_findings(id, graphic.id, &graphic.item, out);
        match &graphic.item {
            Item::Shape { fill, shape, .. } => {
                if *fill == Fill::Body {
                    out.push(issue(
                        "graphic.fill_body",
                        format!(
                            "Footprint graphic {} uses the symbol body fill; use none or solid.",
                            graphic.id
                        ),
                        this.clone(),
                    ));
                }
                if graphic.layer == DrawingLayer::Courtyard && !shape.is_closed() {
                    out.push(issue(
                        "footprint.courtyard_open",
                        format!("Courtyard graphic {} is not a closed shape.", graphic.id),
                        this,
                    ));
                }
            }
            Item::Text(text) => {
                if matches!(
                    text.content,
                    TextContent::ComponentName | TextContent::UnitLabel
                ) {
                    out.push(issue(
                        "text.content_kind",
                        format!(
                            "Footprint text {} shows a symbol-only value; use the designator, a field or literal text.",
                            graphic.id
                        ),
                        this.clone(),
                    ));
                }
                if graphic.layer == DrawingLayer::Courtyard {
                    out.push(issue(
                        "footprint.courtyard_open",
                        format!(
                            "Courtyard graphic {} is text, not a closed shape.",
                            graphic.id
                        ),
                        this,
                    ));
                }
            }
        }
    }
    let Some(polarity) = &footprint.polarity else {
        return;
    };
    if footprint.pad(polarity.pad).is_none() {
        out.push(issue(
            "footprint.polarity_pad_unknown",
            format!("Polarity pad {} is not in this footprint.", polarity.pad),
            vec![ObjectRef::Definition { id }],
        ));
    }
    let mut seen = BTreeSet::new();
    for marker in &polarity.markers {
        let ok = footprint.graphics.iter().any(|g| {
            g.id == *marker && matches!(g.layer, DrawingLayer::Silkscreen | DrawingLayer::Assembly)
        });
        if !ok || !seen.insert(*marker) {
            out.push(issue(
                "footprint.polarity_marker_unknown",
                format!(
                    "Polarity marker {marker} is not a silkscreen or assembly graphic of this footprint, or is listed twice."
                ),
                vec![object(id, marker.0)],
            ));
        }
    }
    if polarity.markers.len() > MAX_POLARITY_MARKERS {
        out.push(issue(
            "footprint.polarity_marker_unknown",
            format!("A footprint lists at most {MAX_POLARITY_MARKERS} polarity markers."),
            vec![ObjectRef::Definition { id }],
        ));
    }
}

/// Source structure the core can check without the evaluator: parameter
/// syntax and the reserved package namespace, expressions, the stored
/// solve status and the pad identities roles, keys and overrides name.
fn source_findings(footprint: &FootprintDefinition, id: DefinitionId, out: &mut Vec<Finding>) {
    let Some(source) = &footprint.source else {
        return;
    };
    let this = vec![ObjectRef::Definition { id }];
    let mut names = BTreeSet::new();
    for param in &source.parameters {
        if !param.name.valid() {
            out.push(issue(
                "footprint.param_name",
                format!(
                    "Parameter ‘{}’ needs an identifier name ([A-Za-z_][A-Za-z0-9_]*).",
                    param.name.0
                ),
                this.clone(),
            ));
        } else if param.name.is_package() {
            out.push(issue(
                "footprint.param_reserved",
                format!(
                    "Parameter ‘{}’ uses the reserved pkg_ prefix of package values.",
                    param.name.0
                ),
                this.clone(),
            ));
        }
        if !names.insert(param.name.0.as_str()) {
            out.push(issue(
                "footprint.param_duplicate",
                format!("Parameter ‘{}’ is defined twice.", param.name.0),
                this.clone(),
            ));
        }
    }
    for expr in source.expressions() {
        if !expr.valid() {
            out.push(issue(
                "footprint.param_expr",
                format!(
                    "Expression ‘{}’ must be non-empty printable text of at most 256 bytes.",
                    expr.0.escape_debug()
                ),
                this.clone(),
            ));
        }
    }
    match source.solve.status {
        crate::library::footprint_source::SolveStatus::Failed => out.push(issue(
            "footprint.solve_failed",
            "The footprint source did not solve; its stored result is not a solution.".into(),
            this.clone(),
        )),
        crate::library::footprint_source::SolveStatus::OverConstrained => out.push(issue(
            "footprint.over_constrained",
            "The footprint source is over-constrained.".into(),
            this.clone(),
        )),
        _ => {}
    }
    let pads: BTreeSet<_> = footprint.pads.iter().map(|p| p.id).collect();
    let mut owned = BTreeSet::new();
    for role in &source.pads {
        if !pads.contains(&role.pad) || !owned.insert(role.pad) {
            out.push(issue(
                "footprint.source_pad_unknown",
                format!(
                    "Pad role {} names no stored pad, or a pad another role or key owns.",
                    role.pad
                ),
                vec![object(id, role.pad.0)],
            ));
        }
    }
    let mut keys = BTreeSet::new();
    for keyed in &source.keys {
        if !pads.contains(&keyed.pad) || !owned.insert(keyed.pad) || !keys.insert(keyed.key) {
            out.push(issue(
                "footprint.source_pad_unknown",
                format!(
                    "Array key {:?} names no stored pad, or a pad another role or key owns.",
                    keyed.key
                ),
                vec![object(id, keyed.pad.0)],
            ));
        }
    }
    let arrays: BTreeSet<_> = source.arrays.iter().map(|a| a.id).collect();
    for array in &source.arrays {
        if !source.pads.iter().any(|r| r.pad == array.seed) {
            out.push(issue(
                "footprint.source_pad_unknown",
                format!(
                    "Array {} copies seed pad {}, which has no pad role.",
                    array.id.0, array.seed
                ),
                vec![object(id, array.id.0)],
            ));
        }
    }
    for keyed in &source.keys {
        let crate::library::footprint_source::GenerationKey::Array { array, .. } = keyed.key;
        if !arrays.contains(&array) {
            out.push(issue(
                "footprint.source_pad_unknown",
                format!("Array key {:?} names no array.", keyed.key),
                vec![object(id, keyed.pad.0)],
            ));
        }
    }
    let mut overridden = BTreeSet::new();
    for o in &source.overrides {
        if !keys.contains(&o.key) || !overridden.insert(o.key) {
            out.push(issue(
                "footprint.override_orphaned",
                format!(
                    "The override of {:?} has no generated instance, or is listed twice.",
                    o.key
                ),
                this.clone(),
            ));
        }
    }
}

impl FootprintDefinition {
    /// Every F1 rule on pads, holes, apertures, graphics, polarity and the
    /// source that does not need the package or the evaluator.
    pub(crate) fn local_findings(&self, id: DefinitionId, out: &mut Vec<Finding>) {
        for pad in &self.pads {
            if pad.number.len() > crate::ids::MAX_LABEL {
                out.push(issue(
                    "footprint.pad_number",
                    format!("Pad {} has a number longer than 256 bytes.", pad.id),
                    pad_object(id, pad),
                ));
            }
            let valid = shape_findings(id, pad, out);
            kind_findings(id, pad, out);
            if valid {
                containment_findings(id, pad, out);
            }
        }
        graphic_findings(self, id, out);
        source_findings(self, id, out);
    }

    /// Advisory footprint lint: missing courtyard, missing pin-1 polarity
    /// on more than two pads, and solder-mask webs below
    /// [`MIN_MASK_WEB_MM`].
    pub fn lint(&self, id: DefinitionId) -> Vec<Finding> {
        let this = ObjectRef::Definition { id };
        let mut out = Vec::new();
        if !self
            .graphics
            .iter()
            .any(|g| g.layer == DrawingLayer::Courtyard)
        {
            out.push(Finding::new(
                "footprint.courtyard_missing",
                "The footprint has no courtyard; generate or draw one (it is never inferred)."
                    .into(),
                vec![this],
            ));
        }
        if self.pads.len() > POLARITY_LINT_PADS && self.polarity.is_none() {
            out.push(Finding::new(
                "footprint.polarity_missing",
                format!(
                    "The footprint has {} pads and no pin-1 polarity pad.",
                    self.pads.len()
                ),
                vec![this],
            ));
        }
        self.mask_web_lint(id, &mut out);
        out
    }

    /// Pairs of mask openings on a shared copper side closer than
    /// [`MIN_MASK_WEB_MM`] (merged openings included).
    fn mask_web_lint(&self, id: DefinitionId, out: &mut Vec<Finding>) {
        let openings: Vec<_> = self
            .pads
            .iter()
            .filter_map(|pad| {
                let grow = pad.mask_expansion(&self.defaults)?;
                if !pad.shape.finite() || (!size_valid(&pad.shape.extent())) {
                    return None;
                }
                let ring = pad.outline(super::geometry::CHECK_CHORD_MM * 10.0);
                let bounds = crate::geometry::Bounds::of(ring.iter().copied())?;
                Some((pad, grow, ring, bounds))
            })
            .collect();
        let mut reported = 0usize;
        for (i, (a, ga, ra, ba)) in openings.iter().enumerate() {
            for (b, gb, rb, bb) in openings.iter().skip(i + 1) {
                let shared = a.kind.through_hole() || b.kind.through_hole() || a.side == b.side;
                if !shared || !ba.inflated(ga + gb + MIN_MASK_WEB_MM).intersects(bb) {
                    continue;
                }
                let web = ring_gap(ra, rb) - ga - gb;
                if web < MIN_MASK_WEB_MM && reported < 64 {
                    reported += 1;
                    out.push(Finding::new(
                        "footprint.mask_web",
                        format!(
                            "The solder-mask web between pads {} and {} is {:.3} mm, below {MIN_MASK_WEB_MM} mm (estimated default).",
                            a.id,
                            b.id,
                            web.max(0.0)
                        ),
                        vec![object(id, a.id.0), object(id, b.id.0)],
                    ));
                }
            }
        }
    }
}
