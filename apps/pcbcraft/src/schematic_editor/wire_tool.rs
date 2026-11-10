//! The Wire tool's drawing (docs/schematic-wires.md, "Drawing"): a wire
//! starts on a pin, label, sheet pin, junction, free end, wire or empty
//! space, takes a corner at each click in empty space, and ends on a
//! connection point or with a free end. Nothing is committed while drawing;
//! the finished wire is one `wire.draw`, one undo step.
use super::*;
use crate::commands::KeyDecl;
use pcbcraft_core::model::FreeEnd;
use pcbcraft_core::schematic::wire_edit::{self, Bend};

/// The Wire tool's keys while a wire is in progress, handled by the canvas
/// (`crate::keys::pressed`).
pub const WIRE_KEYS: &[KeyDecl] = &[
    KeyDecl::new(
        "wire.click",
        "Click",
        "Corner; a pin, label, junction or wire ends the wire",
    ),
    KeyDecl::new(
        "wire.finish",
        "Enter",
        "End here with a free end (or double-click)",
    ),
    KeyDecl::new(
        "wire.end",
        "Esc",
        "End here; with no corner, cancel; again: Select",
    ),
    KeyDecl::new("wire.back", "Backspace", "Remove the last corner"),
    KeyDecl::new("wire.flip", "F", "Flip the elbow corner"),
    KeyDecl::new("wire.arc", "A", "Round new corners, or not"),
];

/// The radius the Wire tool rounds new corners by until the card changes it.
pub(super) const DEFAULT_RADIUS_MM: f64 = 2.5;

/// A drawn path's vertices with every corner rounded by `radius` (0: sharp),
/// each as large as fits: what `wire.draw` stores and the preview draws.
pub(super) fn rounded(vertices: &[Point], radius: f64) -> Vec<Bend> {
    let mut points = vertices.to_vec();
    points.dedup();
    let last = points.len().saturating_sub(1);
    let path: Vec<Bend> = points
        .iter()
        .enumerate()
        .map(|(i, &at)| Bend {
            at,
            radius: if i == 0 || i == last { 0.0 } else { radius },
        })
        .collect();
    wire_edit::fitted(&path)
}

/// `mm` rounded down to the micrometre, as [`wire_edit::fitted`] stores it.
pub(super) fn whole_micrometres(mm: f64) -> f64 {
    ((mm * 1000.0 + 1e-3).floor() / 1000.0).max(0.0)
}

/// Say that corners were rounded by less than the card's radius, because
/// that is the most their segments take: "Rounded with 1 mm, the most that
/// fits". Nothing when `radii` is empty.
pub(super) fn post_reduced(ctx: &egui::Context, radii: &[f64]) {
    let low = radii.iter().copied().reduce(f64::min);
    let high = radii.iter().copied().reduce(f64::max);
    let (Some(low), Some(high)) = (low, high) else {
        return;
    };
    let mm = |v: f64| {
        let text = format!("{v:.3}");
        text.trim_end_matches('0').trim_end_matches('.').to_owned()
    };
    let with = if high - low < 1e-9 {
        mm(low)
    } else {
        format!("{} to {}", mm(low), mm(high))
    };
    kit::toast::post(
        ctx,
        kit::severity::Level::Info,
        format!("Rounded with {with} mm, the most that fits"),
        Vec::new(),
    );
}

/// A refused wire whose engine text cannot be shown.
const WIRE_REFUSED: &str = "A wire cannot end there";

/// A wire being drawn: its start and the points fixed so far.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct WireDraft {
    pub(super) from: WireTarget,
    /// Where it starts on the sheet.
    pub(super) start: Point,
    /// Each click in empty space: its elbow's bend and the click point.
    pub(super) legs: Vec<[Point; 2]>,
}

/// The bend of the 90° elbow from `a` to `b`: horizontal first, or
/// vertical first when `flipped`.
pub(super) fn elbow(a: Point, b: Point, flipped: bool) -> Point {
    if flipped { [a[0], b[1]] } else { [b[0], a[1]] }
}

/// Why a wire can't start on `pin` (device NC or reserved), in the
/// engine's wording for one that would end there; `None` when it can.
fn unwireable(project: &pcbcraft_core::Project, pin: pcbcraft_core::Endpoint) -> Option<String> {
    let instance = project.instance(pin.instance)?;
    let terminal = project
        .library
        .component(instance.component)?
        .definition
        .terminal(pin.terminal)?;
    (!terminal.is_electrical())
        .then(|| pcbcraft_core::instances::unwireable(&instance.designator, &terminal.name))
}

impl WireDraft {
    /// The last fixed point.
    pub(super) fn last(&self) -> Point {
        self.legs.last().map_or(self.start, |leg| leg[1])
    }

    /// The fixed path, then the elbow to `to`.
    pub(super) fn path(&self, to: Point, flipped: bool) -> Vec<Point> {
        let mut path = vec![self.start];
        path.extend(self.legs.iter().flatten());
        path.push(elbow(self.last(), to, flipped));
        path.push(to);
        path
    }
}

impl SchematicEditor {
    /// The keyboard context: the Wire tool's own keys while it is armed.
    pub fn shortcut_context(&self) -> crate::commands::Context {
        match self.tool {
            Tool::Wire => crate::commands::Context::SchematicWire,
            _ => crate::commands::Context::Schematic,
        }
    }

    /// The Wire tool's keys. Esc with no wire in progress is left to
    /// `schematic.cancel` (Select).
    pub(super) fn wire_keys(&mut self, ctx: &egui::Context, engine: &Engine) -> Option<Operation> {
        if self.tool != Tool::Wire || ctx.egui_wants_keyboard_input() {
            return None;
        }
        if crate::keys::pressed(ctx, "wire.flip") {
            self.wire_flipped = !self.wire_flipped;
        }
        if crate::keys::pressed(ctx, "wire.arc") {
            self.wire_arc = !self.wire_arc;
        }
        self.wire.as_ref()?;
        if crate::keys::pressed(ctx, "wire.back") {
            if let Some(draft) = &mut self.wire
                && draft.legs.pop().is_none()
            {
                self.wire = None;
            }
            return None;
        }
        if crate::keys::pressed(ctx, "wire.finish") || crate::keys::pressed(ctx, "wire.end") {
            return self.end_wire_here(ctx, engine);
        }
        None
    }

    /// End the wire at its last fixed point with a free end; with no corner
    /// fixed, drop it.
    pub(super) fn end_wire_here(
        &mut self,
        ctx: &egui::Context,
        engine: &Engine,
    ) -> Option<Operation> {
        let draft = self.wire.take()?;
        let mut corners: Vec<Point> = draft.legs.iter().flatten().copied().collect();
        let end = corners.pop()?;
        let to = WireTarget::Free(FreeEnd::new(end)?);
        self.send_wire(ctx, engine, &draft, corners, (to, end))
    }

    /// A Wire tool click at the snapped sheet point `at`, landing on
    /// `target` (a connection point, free end or wire) or in empty space.
    pub(super) fn wire_click(
        &mut self,
        ctx: &egui::Context,
        engine: &Engine,
        target: Option<(WireTarget, Point)>,
        at: Point,
    ) -> Option<Operation> {
        let Some(mut draft) = self.wire.take() else {
            // Start: on a target, or in empty space with a free end. A pin
            // that can never be wired refuses here, not at the wire's end.
            let (from, start) = match target {
                Some((WireTarget::Terminal(pin), _))
                    if let Some(why) = unwireable(engine.project(), pin) =>
                {
                    kit::toast::post(ctx, kit::severity::Level::Warning, why, Vec::new());
                    return None;
                }
                Some(hit) => hit,
                None => (WireTarget::Free(FreeEnd::new(at)?), at),
            };
            self.wire = Some(WireDraft {
                from,
                start,
                legs: Vec::new(),
            });
            return None;
        };
        match target {
            // A target ends the wire there.
            Some((to, end)) => {
                if to == draft.from && draft.legs.is_empty() {
                    self.wire = Some(draft);
                    return None;
                }
                let mut corners: Vec<Point> = draft.legs.iter().flatten().copied().collect();
                corners.push(elbow(draft.last(), end, self.wire_flipped));
                self.send_wire(ctx, engine, &draft, corners, (to, end))
            }
            // Empty space fixes the elbow and continues from the click.
            None => {
                let last = draft.last();
                if at != last && draft.legs.len() < (schematic::MAX_CORNERS - 1) / 2 {
                    draft.legs.push([elbow(last, at, self.wire_flipped), at]);
                }
                self.wire = Some(draft);
                None
            }
        }
    }

    /// The finished wire as one `wire.draw`: applied at once, or reviewed
    /// when it touches routed copper or merges two named nets
    /// ([`Self::join`]). A wire that cannot exist (it ends on a pin the part
    /// leaves unconnected) has nothing to review: a toast says why and the
    /// drawing goes on. Cancelling a review goes back to the drawing too.
    fn send_wire(
        &mut self,
        ctx: &egui::Context,
        engine: &Engine,
        draft: &WireDraft,
        corners: Vec<Point>,
        (to, end): (WireTarget, Point),
    ) -> Option<Operation> {
        let mut vertices = vec![draft.start];
        vertices.extend(corners);
        vertices.push(end);
        let path = rounded(&vertices, self.new_radius());
        let command = SchematicCommand::DrawWire {
            sheet: Some(self.sheet),
            from: draft.from,
            corners: wire_edit::stored(
                path.get(1..path.len().saturating_sub(1))
                    .unwrap_or_default(),
            )
            .ok()?,
            to,
        };
        let plan = engine.preview_update(Change::Schematic { command });
        if let Some(blocker) = plan.blocker_lines.first() {
            kit::toast::post(
                ctx,
                kit::severity::Level::Warning,
                refusal::presentable(&blocker.detail).unwrap_or_else(|| WIRE_REFUSED.to_owned()),
                Vec::new(),
            );
            self.wire = Some(draft.clone());
            return None;
        }
        let radius = self.new_radius();
        let reduced: Vec<f64> = path
            .iter()
            .map(|c| c.radius)
            .filter(|&r| r > 0.0 && r < radius)
            .collect();
        let operation = self.join(engine, plan);
        if self.update.is_open() {
            self.wire_review = Some((draft.clone(), engine.revision()));
        }
        if operation.is_some() || self.update.is_open() {
            post_reduced(ctx, &reduced);
        }
        operation
    }

    /// The radius new corners get: the card's with arcs on (A), else 0.
    pub(super) fn new_radius(&self) -> f64 {
        if self.wire_arc { self.wire_radius } else { 0.0 }
    }

    /// Back to drawing the wire whose review was cancelled. A review that
    /// applied moved the revision, and the wire is done.
    pub(super) fn resume_wire(&mut self, engine: &Engine) {
        if self.update.is_open() {
            return;
        }
        if let Some((draft, revision)) = self.wire_review.take()
            && revision == engine.revision()
        {
            self.tool = Tool::Wire;
            self.wire = Some(draft);
        }
    }

    /// The free wire ends drawn on the active sheet, as Wire tool targets.
    pub(super) fn free_targets(&self, project: &Project) -> Vec<(WireTarget, Point)> {
        project
            .wires
            .iter()
            .filter(|w| w.sheet == self.sheet)
            .flat_map(|w| [w.from, w.to])
            .filter_map(|end| match end {
                WireEnd::Free(f) => Some((WireTarget::Free(f), f.point())),
                _ => None,
            })
            .collect()
    }
}
