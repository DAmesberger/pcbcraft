//! The footprint publish rules and the advisory footprint lint.
use super::geometry::{CHECK_CHORD_MM, TerminalRule, ring_gap, ring_within};
use super::{FootprintDefinition, HoleShape, Pad, PadKind, PadShape, PadSide, PasteRule};
use crate::diagnostics::{Finding, ObjectRef};
use crate::geometry::{polygon_self_intersects, signed_area2, size_valid};
use crate::ids::DefinitionId;
use crate::library::graphics::{self, DrawingLayer, Fill, Item, MAX_POLARITY_MARKERS, TextContent};
use crate::library::publish::{issue, object};
use crate::rules::{DesignRules, ProjectRules, RuleKey};
use std::collections::BTreeSet;

/// The default profile's smallest solder-mask web between two openings, in
/// mm (an estimated prototype default, not a fabricator qualification);
/// the lint reads the `min mask web` design rule.
pub const MIN_MASK_WEB_MM: f64 = 0.1;
/// Rounding allowance of the mask-web comparison, in mm (1 nm, far below
/// any fabrication tolerance).
const WEB_TOLERANCE_MM: f64 = 1e-6;
/// The default profile's thinnest silkscreen stroke
/// (`footprint.silk_width`), in mm (an estimated prototype default;
/// fabricators state their own); the lint reads `min silkscreen width`.
pub const MIN_SILK_WIDTH_MM: f64 = 0.1;
/// The default profile's smallest silkscreen text height
/// (`footprint.text_height`), in mm (estimated); the lint reads `min
/// silkscreen text height`.
pub const MIN_SILK_TEXT_HEIGHT_MM: f64 = 0.6;

/// The limits the footprint lint applies, each with the text naming its
/// rule and source (`Fine 4/4: min mask web`).
struct LintLimits {
    mask_web: (f64, String),
    silk_width: (f64, String),
    text_height: (f64, String),
    silk_pad: (f64, String),
}

impl LintLimits {
    fn of(rules: &ProjectRules) -> Self {
        let effective: DesignRules = rules.effective();
        let pick = |key: RuleKey, mm: f64| {
            (
                mm,
                format!("{}: {}", rules.source_of(key).describe(), key.label()),
            )
        };
        Self {
            mask_web: pick(RuleKey::MaskWebMin, effective.mask.web_min),
            silk_width: pick(
                RuleKey::SilkLineWidthMin,
                effective.silkscreen.line_width_min,
            ),
            text_height: pick(
                RuleKey::SilkTextHeightMin,
                effective.silkscreen.text_height_min,
            ),
            silk_pad: pick(
                RuleKey::SilkPadClearance,
                effective.silkscreen.pad_clearance,
            ),
        }
    }
}

/// `needs 0.1 mm (Pcbcraft default: min mask web)`.
fn needs((mm, cite): &(f64, String)) -> String {
    let text = format!("{mm:.3}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    format!(
        "needs {} mm ({cite})",
        if text.is_empty() { "0" } else { text }
    )
}
/// Pads on which the lint expects an authored pin-1 polarity.
pub(super) const POLARITY_LINT_PADS: usize = 2;

fn pad_object(id: DefinitionId, pad: &Pad) -> Vec<ObjectRef> {
    vec![object(id, pad.id.0)]
}

/// The user's name for a pad in a message: its number, or "(no number)"
/// when it has none.
pub(super) fn pad_label(pad: &Pad) -> String {
    if pad.number.trim().is_empty() {
        "(no number)".into()
    } else {
        pad.number.clone()
    }
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
            format!("Pad {} needs a positive finite size.", pad_label(pad)),
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
            format!("Pad {} needs {need}.", pad_label(pad)),
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
                "Pad {} lands no package terminal; choose one, or make it mechanical.",
                pad_label(pad)
            ),
            pad_object(id, pad),
        )),
        (TerminalRule::Forbidden, Some(_)) => out.push(issue(
            "footprint.terminal_forbidden",
            format!(
                "Pad {} is a {} pad and cannot land a package terminal.",
                pad_label(pad),
                pad.kind.label()
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
                pad_label(pad)
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
            format!(
                "Pad {} is a {} pad and needs {need}.",
                pad_label(pad),
                pad.kind.label()
            ),
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
                    pad_label(pad)
                ),
                pad_object(id, pad),
            ));
        } else if !ring_within(&hole.ring(CHECK_CHORD_MM), &land, tol) {
            out.push(issue(
                "footprint.hole_outside_land",
                format!("The hole of pad {} is not inside its land.", pad_label(pad)),
                pad_object(id, pad),
            ));
        }
    }
    let mut apertures = BTreeSet::new();
    for via in &pad.vias {
        if !apertures.insert(via.id) {
            out.push(issue(
                "footprint.aperture_duplicate",
                format!("Pad {} uses one aperture more than once.", pad_label(pad)),
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
                    "A via of pad {} needs a positive drill inside the land.",
                    pad_label(pad)
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
                    format!("Pad {} uses one aperture more than once.", pad_label(pad)),
                    pad_object(id, pad),
                ));
            }
            let ring = window.shape.to_mm().flatten(CHECK_CHORD_MM);
            if !window.shape.is_closed() || ring.len() < 3 || signed_area2(&ring).abs() <= 1e-9 {
                out.push(issue(
                    "footprint.paste_window_shape",
                    format!(
                        "A paste window of pad {} needs a closed shape with area.",
                        pad_label(pad)
                    ),
                    pad_object(id, pad),
                ));
            } else if !ring_within(&ring, &land, tol) {
                out.push(issue(
                    "footprint.paste_window_outside_pad",
                    format!(
                        "A paste window of pad {} is not inside the pad.",
                        pad_label(pad)
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
                        "A footprint graphic uses the symbol body fill; use none or solid.".into(),
                        this.clone(),
                    ));
                }
                if graphic.layer == DrawingLayer::Courtyard && !shape.is_closed() {
                    out.push(issue(
                        "footprint.courtyard_open",
                        "A courtyard graphic is not a closed shape.".into(),
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
                        "A footprint text shows a symbol-only value; use the designator, a field or literal text.".into(),
                        this.clone(),
                    ));
                }
                if graphic.layer == DrawingLayer::Courtyard {
                    out.push(issue(
                        "footprint.courtyard_open",
                        "A courtyard graphic is text, not a closed shape.".into(),
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
            "The pin-1 polarity pad is not in this footprint.".into(),
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
                "A polarity marker is not a silkscreen or assembly graphic of this footprint, or is listed twice.".into(),
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
                "An expression must be non-empty printable text of at most 256 bytes.".into(),
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
                "A pad role names no stored pad, or a pad another role or key owns.".into(),
                vec![object(id, role.pad.0)],
            ));
        }
    }
    let mut keys = BTreeSet::new();
    for keyed in &source.keys {
        if !pads.contains(&keyed.pad) || !owned.insert(keyed.pad) || !keys.insert(keyed.key) {
            out.push(issue(
                "footprint.source_pad_unknown",
                "An array pad names no stored pad, or a pad another role or key owns.".into(),
                vec![object(id, keyed.pad.0)],
            ));
        }
    }
    let arrays: BTreeSet<_> = source.arrays.iter().map(|a| a.id).collect();
    for array in &source.arrays {
        if !source.pads.iter().any(|r| r.pad == array.seed) {
            out.push(issue(
                "footprint.source_pad_unknown",
                "An array copies a seed pad that has no pad role.".into(),
                vec![object(id, array.id.0)],
            ));
        }
    }
    for keyed in &source.keys {
        let crate::library::footprint_source::GenerationKey::Array { array, .. } = keyed.key;
        if !arrays.contains(&array) {
            out.push(issue(
                "footprint.source_pad_unknown",
                "An array pad names an array that does not exist.".into(),
                vec![object(id, keyed.pad.0)],
            ));
        }
    }
    let mut overridden = BTreeSet::new();
    for o in &source.overrides {
        if !keys.contains(&o.key) || !overridden.insert(o.key) {
            out.push(issue(
                "footprint.override_orphaned",
                "An array pad override has no generated pad, or is listed twice.".into(),
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
                    "A pad has a number longer than 256 bytes.".into(),
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

    /// [`Self::lint_with`] under the default profile, for a footprint
    /// outside any project.
    pub fn lint(&self, id: DefinitionId) -> Vec<Finding> {
        self.lint_with(id, &ProjectRules::default())
    }

    /// Advisory footprint lint: missing courtyard, missing pin-1 polarity
    /// on more than two pads, solder-mask webs below the `min mask web`
    /// rule, pads outside the courtyard, silkscreen over (or within the
    /// `silkscreen to pad clearance` of) pads, and silkscreen strokes and
    /// text below the `min silkscreen width` and `min silkscreen text
    /// height` rules, all from `rules` (the project's, or the default
    /// profile in a library context).
    pub fn lint_with(&self, id: DefinitionId, rules: &ProjectRules) -> Vec<Finding> {
        let limits = LintLimits::of(rules);
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
        self.mask_web_lint(id, &limits, &mut out);
        self.courtyard_lint(id, &mut out);
        self.silk_lint(id, &limits, &mut out);
        self.silk_size_lint(id, &limits, &mut out);
        out
    }

    /// Silkscreen strokes thinner than the silkscreen width rule
    /// (`footprint.silk_width`; a filled shape prints its area, so only
    /// unfilled strokes count) and visible text shorter than the text
    /// height rule (`footprint.text_height`).
    fn silk_size_lint(&self, id: DefinitionId, limits: &LintLimits, out: &mut Vec<Finding>) {
        let mut reported = 0usize;
        let (min_width, min_height) = (limits.silk_width.0, limits.text_height.0);
        for graphic in self
            .graphics
            .iter()
            .filter(|g| g.layer == DrawingLayer::Silkscreen)
        {
            if reported >= 64 {
                return;
            }
            let finding = match &graphic.item {
                Item::Shape { width_nm, fill, .. }
                    if *fill == Fill::None && width_nm.to_mm_f64() < min_width =>
                {
                    let width = width_nm.to_mm_f64();
                    Finding::new(
                        "footprint.silk_width",
                        format!(
                            "A silkscreen line is {width:.3} mm wide and may not print; {}.",
                            needs(&limits.silk_width)
                        ),
                        vec![object(id, graphic.id.0)],
                    )
                }
                Item::Text(text) if text.visible && text.height_nm.to_mm_f64() < min_height => {
                    Finding::new(
                        "footprint.text_height",
                        format!(
                            "A silkscreen text is {:.3} mm high and may not be legible; {}.",
                            text.height_nm.to_mm_f64(),
                            needs(&limits.text_height)
                        ),
                        vec![object(id, graphic.id.0)],
                    )
                }
                _ => continue,
            };
            reported += 1;
            out.push(finding);
        }
    }

    /// Copper pads not inside any closed courtyard shape
    /// (`footprint.pad_outside_courtyard`). Open courtyard strokes decide
    /// nothing, so a footprint without a closed courtyard is not checked.
    fn courtyard_lint(&self, id: DefinitionId, out: &mut Vec<Finding>) {
        let yards: Vec<crate::geometry::Shape> = self
            .graphics
            .iter()
            .filter(|g| g.layer == DrawingLayer::Courtyard)
            .filter_map(|g| match &g.item {
                Item::Shape { shape, .. } if shape.is_closed() => Some(shape.to_mm()),
                _ => None,
            })
            .collect();
        if yards.is_empty() {
            return;
        }
        let mut reported = 0usize;
        for pad in &self.pads {
            if reported >= 64 || !pad.shape.finite() || !size_valid(&pad.shape.extent()) {
                continue;
            }
            let ring = pad.outline(CHECK_CHORD_MM * 10.0);
            let inside = ring
                .iter()
                .all(|p| yards.iter().any(|y| y.distance(*p, true) <= 1e-6));
            if !inside {
                reported += 1;
                out.push(Finding::new(
                    "footprint.pad_outside_courtyard",
                    format!("Pad {} extends past the courtyard.", pad_label(pad)),
                    pad_object(id, pad),
                ));
            }
        }
    }

    /// Silkscreen strokes over copper on the same side, or closer to it
    /// than the `silkscreen to pad clearance` rule
    /// (`footprint.silk_over_pad`): fabricators clip them, so the legend
    /// would not print as drawn. Sampled every 0.05 mm, within a budget.
    fn silk_lint(&self, id: DefinitionId, limits: &LintLimits, out: &mut Vec<Finding>) {
        const STEP_MM: f64 = 0.05;
        let gap = limits.silk_pad.0.max(0.0);
        let mut budget = 1_000_000usize;
        let lands: Vec<_> = self
            .pads
            .iter()
            .filter(|p| {
                p.kind != PadKind::NonPlatedHole
                    && p.shape.finite()
                    && size_valid(&p.shape.extent())
            })
            .map(|p| (p, p.land(CHECK_CHORD_MM * 10.0), p.bounds()))
            .collect();
        let mut flagged = BTreeSet::new();
        for graphic in self
            .graphics
            .iter()
            .filter(|g| g.layer == DrawingLayer::Silkscreen)
        {
            let Item::Shape {
                shape, width_nm, ..
            } = &graphic.item
            else {
                continue;
            };
            let shape = &shape.to_mm();
            let half = width_nm.to_mm_f64().max(0.0) / 2.0;
            let mut points = shape.flatten(CHECK_CHORD_MM * 10.0);
            if shape.is_closed()
                && let Some(first) = points.first().copied()
            {
                points.push(first);
            }
            let Some(extent) = crate::geometry::Bounds::of(points.iter().copied()) else {
                continue;
            };
            let extent = extent.inflated(half + gap);
            let opposite = graphic.side == graphics::Side::Opposite;
            for (pad, land, bounds) in &lands {
                let same_side =
                    pad.kind.through_hole() || (pad.side == PadSide::Bottom) == opposite;
                if !same_side || flagged.contains(&pad.id) || !bounds.intersects(&extent) {
                    continue;
                }
                let mut overlaps = false;
                let over = points.windows(2).any(|w| {
                    let length = crate::geometry::distance(w[0], w[1]);
                    let steps = ((length / STEP_MM).ceil() as usize).clamp(1, 4096);
                    (0..=steps).any(|k| {
                        if budget == 0 {
                            return false;
                        }
                        budget -= 1;
                        let t = k as f64 / steps as f64;
                        let p = [
                            w[0][0] + (w[1][0] - w[0][0]) * t,
                            w[0][1] + (w[1][1] - w[0][1]) * t,
                        ];
                        let d = land.distance(p, true);
                        overlaps |= d < half - 1e-9;
                        overlaps || d < half + gap - 1e-9
                    })
                });
                if over && flagged.len() < 64 {
                    flagged.insert(pad.id);
                    let message = if overlaps {
                        format!(
                            "Silkscreen lies over pad {}; it would be clipped at fabrication.",
                            pad_label(pad)
                        )
                    } else {
                        format!(
                            "Silkscreen is too close to pad {}; {}.",
                            pad_label(pad),
                            needs(&limits.silk_pad)
                        )
                    };
                    out.push(Finding::new(
                        "footprint.silk_over_pad",
                        message,
                        vec![object(id, pad.id.0), object(id, graphic.id.0)],
                    ));
                }
            }
        }
    }

    /// Pairs of mask openings on a shared copper side closer than the
    /// `min mask web` rule (merged openings included).
    fn mask_web_lint(&self, id: DefinitionId, limits: &LintLimits, out: &mut Vec<Finding>) {
        let min_web = limits.mask_web.0;
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
                if !shared || !ba.inflated(ga + gb + min_web).intersects(bb) {
                    continue;
                }
                let web = ring_gap(ra, rb) - ga - gb;
                // A web of exactly the minimum passes: sums of decimal
                // sizes and positions land a few ulps either side of it.
                if web < min_web - WEB_TOLERANCE_MM && reported < 64 {
                    reported += 1;
                    out.push(Finding::new(
                        "footprint.mask_web",
                        format!(
                            "The solder-mask web between pads {} and {} is {:.3} mm; {}.",
                            pad_label(a),
                            pad_label(b),
                            web.max(0.0),
                            needs(&limits.mask_web)
                        ),
                        vec![object(id, a.id.0), object(id, b.id.0)],
                    ));
                }
            }
        }
    }
}

#[cfg(test)]
mod rules_tests {
    use super::*;
    use crate::ids::{GraphicId, PackageTerminalId, PadId};
    use crate::library::graphics::{FootprintGraphic, Side};
    use crate::rules::{RuleValue, RulesCommand};
    use crate::tests::support::{nm, shape_nm};

    /// Two 0.5 mm square lands 0.3 mm apart (copper), each opening grown
    /// by the default 0.05 mm: a 0.2 mm mask web; and a 0.12 mm silk line.
    fn footprint() -> FootprintDefinition {
        FootprintDefinition {
            pads: vec![
                Pad::smd_rect(PadId(1), PackageTerminalId(1), "1", [0.0, 0.0], [0.5, 0.5]),
                Pad::smd_rect(PadId(2), PackageTerminalId(2), "2", [0.8, 0.0], [0.5, 0.5]),
            ],
            graphics: vec![FootprintGraphic {
                id: GraphicId(10),
                layer: DrawingLayer::Silkscreen,
                side: Side::Component,
                item: Item::Shape {
                    shape: shape_nm(crate::geometry::Shape::polyline(vec![
                        [-1.0, 1.0],
                        [2.0, 1.0],
                    ])),
                    width_nm: nm(0.12),
                    fill: Fill::None,
                },
            }],
            next_local: 11,
            ..FootprintDefinition::default()
        }
    }

    fn messages(f: &FootprintDefinition, rules: &ProjectRules, code: &str) -> Vec<String> {
        f.lint_with(DefinitionId(1), rules)
            .into_iter()
            .filter(|x| x.code == code)
            .map(|x| x.message)
            .collect()
    }

    #[test]
    fn the_lint_reads_the_project_rules_and_names_them() -> crate::diagnostics::Result<()> {
        let f = footprint();
        let default = ProjectRules::default();
        assert!(messages(&f, &default, "footprint.mask_web").is_empty());
        assert!(messages(&f, &default, "footprint.silk_width").is_empty());
        assert_eq!(
            f.lint(DefinitionId(1)),
            f.lint_with(DefinitionId(1), &default)
        );

        let mut fine = default.clone();
        let base = crate::rules::fixtures::prototype()?;
        let mut values = base.rules().clone();
        values.mask.web_min = 0.25;
        let profile = crate::rules::RuleProfile::from_values(
            "fine-test",
            "Fine Test",
            base.source.clone(),
            base.drill_files.clone(),
            &values,
            "",
        )?;
        crate::rules::apply(
            &mut fine,
            RulesCommand::SetProfile {
                profile: Box::new(profile),
                keep_overrides: false,
            },
        )?;
        assert_eq!(
            messages(&f, &fine, "footprint.mask_web"),
            vec![
                "The solder-mask web between pads 1 and 2 is 0.200 mm; needs 0.25 mm (Fine Test: min mask web)."
                    .to_string()
            ]
        );

        let mut strict = crate::rules::fixtures::rules()?;
        for (key, mm) in [
            (RuleKey::SilkLineWidthMin, 0.15),
            (RuleKey::SilkPadClearance, 0.6),
        ] {
            crate::rules::apply(
                &mut strict,
                RulesCommand::SetOverride {
                    key,
                    value: RuleValue::Length(mm),
                },
            )?;
        }
        assert_eq!(
            messages(&f, &strict, "footprint.silk_width"),
            vec![
                "A silkscreen line is 0.120 mm wide and may not print; needs 0.15 mm (project override: min silkscreen width)."
                    .to_string()
            ]
        );
        // The stroke's lower edge (y 1.0 less its 0.06 mm half-width) is
        // 0.69 mm above the lands' copper (top at y 0.25): clear of 0.6 mm.
        assert!(messages(&f, &strict, "footprint.silk_over_pad").is_empty());
        let mut closer = f.clone();
        if let Some(FootprintGraphic {
            item: Item::Shape { shape, .. },
            ..
        }) = closer.graphics.first_mut()
        {
            *shape = shape_nm(crate::geometry::Shape::polyline(vec![
                [-1.0, 0.7],
                [2.0, 0.7],
            ]));
        }
        // At y 0.7 the edge is 0.39 mm from the copper: under 0.6 mm.
        let near = messages(&closer, &strict, "footprint.silk_over_pad");
        assert_eq!(near.len(), 2, "{near:?}");
        assert!(
            near[0].ends_with("needs 0.6 mm (project override: silkscreen to pad clearance)."),
            "{near:?}"
        );
        assert!(messages(&closer, &default, "footprint.silk_over_pad").is_empty());
        Ok(())
    }
}
