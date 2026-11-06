//! The Wire tool's drawing (docs/schematic-wires.md, "Drawing"): a wire
//! starts on a pin, label, sheet pin, junction, free end, wire or empty
//! space, takes a corner at each click in empty space, and ends on a
//! connection point or with a free end. Nothing is committed while drawing;
//! the finished wire is one `wire.draw`, one undo step.
use super::*;
use crate::commands::KeyDecl;
use pcbcraft_core::model::{Corner, FreeEnd};

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
];

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
            return self.end_wire_here(engine);
        }
        None
    }

    /// End the wire at its last fixed point with a free end; with no corner
    /// fixed, drop it.
    pub(super) fn end_wire_here(&mut self, engine: &Engine) -> Option<Operation> {
        let draft = self.wire.take()?;
        let mut corners: Vec<Point> = draft.legs.iter().flatten().copied().collect();
        let end = corners.pop()?;
        let to = WireTarget::Free(FreeEnd::new(end)?);
        self.send_wire(engine, &draft, corners, to)
    }

    /// A Wire tool click at the snapped sheet point `at`, landing on
    /// `target` (a connection point, free end or wire) or in empty space.
    pub(super) fn wire_click(
        &mut self,
        engine: &Engine,
        target: Option<(WireTarget, Point)>,
        at: Point,
    ) -> Option<Operation> {
        let Some(mut draft) = self.wire.take() else {
            // Start: on a target, or in empty space with a free end.
            let (from, start) = match target {
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
                self.send_wire(engine, &draft, corners, to)
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
    /// ([`Self::join`]).
    fn send_wire(
        &mut self,
        engine: &Engine,
        draft: &WireDraft,
        corners: Vec<Point>,
        to: WireTarget,
    ) -> Option<Operation> {
        let command = SchematicCommand::DrawWire {
            sheet: Some(self.sheet),
            from: draft.from,
            corners: corners.into_iter().map(|at| Corner { at }).collect(),
            to,
        };
        self.join(engine, command, draft.from, to)
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
