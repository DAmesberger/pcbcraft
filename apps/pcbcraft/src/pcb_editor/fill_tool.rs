//! The Fill tool: click the corners of a fill outline on the routing
//! layer, with arcs (A switches; an arc takes its end, then a point it
//! passes through), and close it on its first point (or Enter) to add the
//! fill as one `fill.add`. And the selected fill's outline handles: drag a
//! vertex to move it, a line's midpoint to add a vertex there, an arc's
//! midpoint to bend it through the pointer, double-click a vertex to remove
//! it; each release is one `fill.edit` (docs/sidebar-ia.md, "Tool card").
use super::{BoardView, PcbEditor, Tool, copper_options, fill_paint, snap};
use crate::kit::{self, ToolCard};
use egui::{Pos2, Shape, Stroke, Ui};
use pcbcraft_core::layout::fill::FillCommand;
use pcbcraft_core::library::{Outline, Segment};
use pcbcraft_core::model::fill::{FillSettings, FillSpec};
use pcbcraft_core::{Fill, Id, Operation, Point, Project, schematic};
use solidcraft_ui_egui::theme::Tokens;

/// Screen reach of a handle and of the first point when closing, points.
const REACH_PX: f32 = 8.0;
/// Fill priorities the tool card offers.
const PRIORITY_RANGE: std::ops::RangeInclusive<f64> = 0.0..=1000.0;

/// The outline the Fill tool is drawing and the fill it will add.
#[derive(Clone, Debug, Default)]
pub(super) struct FillDraw {
    pub start: Option<Point>,
    pub segments: Vec<Segment>,
    /// Clicks draw arcs instead of lines.
    pub arc: bool,
    /// The end of the arc being drawn; the next click is a point on it.
    pub arc_end: Option<Point>,
    /// The new fill's net (`None`: unconnected copper).
    pub net: Option<Id>,
    pub priority: f64,
}

impl FillDraw {
    /// Where the outline has got to.
    pub fn last(&self) -> Option<Point> {
        self.segments.last().map(Segment::to).or(self.start)
    }

    /// Drop the outline in progress (the tool's settings stay).
    pub fn clear(&mut self) {
        self.start = None;
        self.segments.clear();
        self.arc_end = None;
    }

    /// A click at `p`; `closes` says it lands on the first point. Returns
    /// the closed outline when this click closes it.
    pub fn click(&mut self, p: Point, closes: bool) -> Option<Outline> {
        let Some(from) = self.last() else {
            self.start = Some(p);
            return None;
        };
        if self.arc {
            match self.arc_end.take() {
                None => {
                    if p != from {
                        self.arc_end = Some(p);
                    }
                    return None;
                }
                Some(end) => {
                    self.segments
                        .push(match fill_paint::bulge_through(from, end, p) {
                            Some(bulge) => Segment::Arc { to: end, bulge },
                            None => Segment::Line { to: end },
                        });
                    let start = self.start?;
                    return (end == start && self.segments.len() >= 2)
                        .then(|| self.finish())
                        .flatten();
                }
            }
        }
        let start = self.start?;
        let to = if closes { start } else { p };
        if to == from {
            return None;
        }
        self.segments.push(Segment::Line { to });
        (to == start).then(|| self.finish()).flatten()
    }

    /// Enter: close the outline with a line back to its first point.
    pub fn close(&mut self) -> Option<Outline> {
        let start = self.start?;
        self.arc_end = None;
        if self.last() != Some(start) {
            self.segments.push(Segment::Line { to: start });
        }
        self.finish()
    }

    /// Backspace: drop the arc end, else the last segment, else the start.
    pub fn back(&mut self) {
        if self.arc_end.take().is_none() && self.segments.pop().is_none() {
            self.start = None;
        }
    }

    /// The closed outline, when it has three segments or more (two when
    /// one is an arc); the drawing is cleared either way once closed.
    fn finish(&mut self) -> Option<Outline> {
        let start = self.start?;
        let arcs = self
            .segments
            .iter()
            .any(|s| matches!(s, Segment::Arc { .. }));
        let enough = self.segments.len() >= 3 || (arcs && self.segments.len() >= 2);
        let outline = Outline {
            start,
            segments: std::mem::take(&mut self.segments),
        };
        self.clear();
        enough.then_some(outline)
    }

    /// The outline so far with the segment the pointer would add.
    pub fn preview(&self, cursor: Point) -> Option<Outline> {
        let start = self.start?;
        let from = self.last()?;
        let mut segments = self.segments.clone();
        match self.arc_end {
            Some(end) => segments.push(match fill_paint::bulge_through(from, end, cursor) {
                Some(bulge) => Segment::Arc { to: end, bulge },
                None => Segment::Line { to: end },
            }),
            None if cursor != from => segments.push(Segment::Line { to: cursor }),
            None => {}
        }
        Some(Outline { start, segments })
    }
}

/// A handle of the selected fill's outline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Handle {
    /// Vertex `i`: 0 the start, else the end of segment `i − 1`.
    Vertex(usize),
    /// The midpoint of segment `i`.
    Mid(usize),
}

/// A handle being dragged.
#[derive(Clone, Debug)]
pub(super) struct HandleDrag {
    pub fill: Id,
    pub handle: Handle,
    pub to: Point,
}

impl HandleDrag {
    /// The outline the drag makes of `fill`, if it drags that fill.
    pub fn preview(&self, fill: &Fill) -> Option<Outline> {
        (fill.id == self.fill)
            .then(|| edited(&fill.outline, self.handle, self.to))
            .flatten()
    }
}

/// `outline` with `handle` moved to `to`: a vertex moves, a line's midpoint
/// becomes a new vertex, an arc bends through `to` (straight when `to` is
/// in line with its ends). `None` when the handle does not exist.
pub(super) fn edited(outline: &Outline, handle: Handle, to: Point) -> Option<Outline> {
    let mut out = outline.clone();
    let n = out.segments.len();
    match handle {
        Handle::Vertex(0) => {
            out.start = to;
            let last = out.segments.last_mut()?;
            set_end(last, to);
        }
        Handle::Vertex(i) => {
            let segment = out.segments.get_mut(i - 1).filter(|_| i < n)?;
            set_end(segment, to);
        }
        Handle::Mid(i) => {
            let from = fill_paint::vertices(outline).get(i).copied()?;
            let segment = *out.segments.get(i)?;
            let end = segment.to();
            match segment {
                Segment::Line { .. } => {
                    out.segments[i] = Segment::Line { to };
                    out.segments.insert(i + 1, Segment::Line { to: end });
                }
                Segment::Arc { .. } => {
                    out.segments[i] = match fill_paint::bulge_through(from, end, to) {
                        Some(bulge) => Segment::Arc { to: end, bulge },
                        None => Segment::Line { to: end },
                    };
                }
            }
        }
    }
    Some(out)
}

fn set_end(segment: &mut Segment, to: Point) {
    match segment {
        Segment::Line { to: end } | Segment::Arc { to: end, .. } => *end = to,
    }
}

/// `outline` without vertex `i`; `None` when three segments would not
/// remain or the vertex does not exist.
pub(super) fn without_vertex(outline: &Outline, i: usize) -> Option<Outline> {
    let n = outline.segments.len();
    if n <= 3 || i >= n {
        return None;
    }
    let mut out = outline.clone();
    if i == 0 {
        let first = out.segments.remove(0);
        out.start = first.to();
        let last = out.segments.last_mut()?;
        set_end(last, out.start);
    } else {
        // The segment ending at the vertex goes; the next one starts at
        // the vertex before.
        out.segments.remove(i - 1);
    }
    Some(out)
}

impl PcbEditor {
    /// Select fill `id` and light its net, as a 2D or 3D pick does.
    pub fn select_fill(&mut self, id: Id, net: Option<Id>) {
        self.set_selection(None, None, net);
        self.fill = Some(id);
    }

    /// The selected fill's handle within reach of the screen point `at`:
    /// vertices before midpoints. A locked or hidden fill has none.
    pub(super) fn fill_handle_at(
        &self,
        project: &Project,
        view: BoardView,
        at: Pos2,
    ) -> Option<(Id, Handle)> {
        let fill = self
            .fill
            .and_then(|id| project.fill(id))
            .filter(|f| !f.locked && self.fill_shown(f))?;
        let near = |p: &Point| view.screen(*p).distance(at) <= REACH_PX;
        let vertices = fill_paint::vertices(&fill.outline);
        if let Some(i) = vertices.iter().position(near) {
            return Some((fill.id, Handle::Vertex(i)));
        }
        fill_paint::midpoints(&fill.outline)
            .iter()
            .position(near)
            .map(|i| (fill.id, Handle::Mid(i)))
    }

    /// The `fill.edit` that gives `fill` the `outline`.
    pub(super) fn fill_outline_edit(fill: &Fill, outline: Outline) -> Option<Operation> {
        (outline != fill.outline).then(|| {
            Operation::Fill(FillCommand::Edit {
                fill: Box::new(Fill {
                    outline,
                    ..fill.clone()
                }),
            })
        })
    }

    /// A Fill tool click at board point `at` (`screen` on the canvas):
    /// the next corner, snapped to the grid, or the first point when
    /// within reach of it, which closes the outline and adds the fill.
    pub(super) fn fill_click(&mut self, view: BoardView, at: Point) -> Option<Operation> {
        let screen = view.screen(at);
        let closes = self
            .fill_draw
            .start
            .is_some_and(|s| view.screen(s).distance(screen) <= REACH_PX)
            && self.fill_draw.segments.len() >= 2;
        let p = match (closes, self.fill_draw.start) {
            (true, Some(start)) => start,
            _ => snap(at),
        };
        let outline = self.fill_draw.click(p, closes)?;
        Some(self.add_fill(outline))
    }

    /// The `fill.add` of a closed outline on the routing layer.
    pub(super) fn add_fill(&mut self, outline: Outline) -> Operation {
        Operation::Fill(FillCommand::Add {
            fill: Box::new(FillSpec {
                name: String::new(),
                net: self.fill_draw.net,
                layer: self.layer,
                outline,
                priority: self.fill_draw.priority.round().clamp(0.0, 1000.0) as u32,
                settings: FillSettings::default(),
            }),
        })
    }

    /// The Fill tool's keys over the canvas: A switches lines and arcs,
    /// Enter closes the outline, Backspace takes back the last click, Esc
    /// drops the outline. Returns the `fill.add` Enter makes and whether
    /// Esc was taken (an outline was in progress).
    pub(super) fn fill_keys(&mut self, ui: &Ui) -> (Option<Operation>, bool) {
        if self.tool != Tool::Fill || ui.ctx().egui_wants_keyboard_input() {
            return (None, false);
        }
        let pressed = |id| crate::keys::pressed(ui.ctx(), id);
        if pressed("fill.arc") {
            self.fill_draw.arc = !self.fill_draw.arc;
            self.fill_draw.arc_end = None;
        }
        if pressed("fill.back") {
            self.fill_draw.back();
        }
        let operation = if pressed("fill.close") {
            self.fill_draw.close().map(|o| self.add_fill(o))
        } else {
            None
        };
        // Esc stays with the Layout's cancel unless an outline is open.
        let escaped = self.fill_draw.start.is_some() && pressed("fill.drop");
        if escaped {
            self.fill_draw.clear();
        }
        (operation, escaped)
    }

    /// The outline being drawn, its next segment to the pointer and its
    /// corners.
    pub(super) fn paint_fill_draw(
        &self,
        painter: &egui::Painter,
        view: BoardView,
        cursor: Option<Point>,
    ) {
        if self.tool != Tool::Fill {
            return;
        }
        let t = Tokens::get();
        let at = cursor.map(snap);
        let outline = at.and_then(|c| self.fill_draw.preview(c)).or_else(|| {
            self.fill_draw.start.map(|start| Outline {
                start,
                segments: self.fill_draw.segments.clone(),
            })
        });
        if let Some(outline) = &outline {
            let ring: Vec<Pos2> = flat_open(outline)
                .into_iter()
                .map(|p| view.screen(p))
                .collect();
            painter.add(Shape::line(ring, Stroke::new(1.5, t.body_selected)));
            for p in fill_paint::vertices(outline) {
                painter.circle_filled(view.screen(p), 3.0, t.body_selected);
            }
        }
        if let Some(end) = self.fill_draw.arc_end {
            painter.circle_stroke(view.screen(end), 4.0, Stroke::new(1.5, t.body_selected));
        }
        if let Some(at) = at {
            painter.circle_stroke(view.screen(at), 5.0, Stroke::new(1.5, t.body_selected));
        }
    }

    /// The Fill tool card: the net, layer and priority of the fill it
    /// adds, and whether clicks draw arcs. Tool settings are editor state,
    /// so it issues no operation, except Finish (closing the outline).
    pub(super) fn fill_tool_card(&mut self, ui: &mut Ui, project: &Project) -> Option<Operation> {
        let drawing = self.fill_draw.start.is_some();
        let card = ToolCard {
            icon: "polygon",
            name: "Fill",
            key: "",
            hint: if self.fill_draw.arc_end.is_some() {
                "Click a point the arc passes through; Esc drops the outline"
            } else if drawing {
                "Click corners; click the first point or Enter closes; A arcs"
            } else {
                "Click the first corner of the fill outline; A draws arcs"
            },
            tip: "Draws a fill outline on the chosen copper layer. Each click adds a corner on \
                  the 0.5 mm grid; with arcs on (A), a click sets an arc's end and the next a \
                  point it passes through. Clicking the first point, Enter or Finish closes \
                  the outline and adds the fill; Backspace takes back the last click and Esc \
                  drops the outline. The fill's copper is computed by Refill.",
            finish: Some("Finish"),
        };
        let t = Tokens::get();
        let margin = egui::Margin::symmetric(t.space_8 as i8, 0);
        let response = egui::Frame::NONE
            .inner_margin(margin)
            .show(ui, |ui| {
                kit::tool_card(ui, &card, |ui| {
                    let mut nets = vec![(None, "No net".to_owned())];
                    nets.extend(
                        schematic::nets(project)
                            .iter()
                            .map(|net| (Some(net.id), net.name.clone())),
                    );
                    if kit::choice_row(ui, "Net", &mut self.fill_draw.net, &nets) {
                        self.route_highlight(self.fill_draw.net);
                    }
                    let options = copper_options(&self.stack);
                    kit::choice_row(ui, "Layer", &mut self.layer, &options);
                    kit::number_row(
                        ui,
                        "Priority",
                        &mut self.fill_draw.priority,
                        "",
                        PRIORITY_RANGE,
                    );
                    let arcs = crate::keys::labelled(ui.ctx(), "Arcs", "fill.arc");
                    kit::check_row(ui, &arcs, &mut self.fill_draw.arc);
                })
            })
            .inner;
        if response.cancel {
            self.fill_draw.clear();
        }
        if response.done {
            self.end_route();
            self.discard_staged();
        }
        if response.finish {
            return self.fill_draw.close().map(|o| self.add_fill(o));
        }
        None
    }
}

/// An outline drawn so far (not closed), flattened.
fn flat_open(outline: &Outline) -> Vec<Point> {
    let mut ring = fill_paint::outline_ring(outline);
    if outline.closed()
        && let Some(first) = ring.first().copied()
    {
        ring.push(first);
    }
    ring
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square() -> Outline {
        Outline {
            start: [0.0, 0.0],
            segments: vec![
                Segment::Line { to: [4.0, 0.0] },
                Segment::Line { to: [4.0, 4.0] },
                Segment::Line { to: [0.0, 4.0] },
                Segment::Line { to: [0.0, 0.0] },
            ],
        }
    }

    #[test]
    fn clicks_draw_lines_and_arcs_and_close_on_the_first_point() -> Result<(), String> {
        let mut draw = FillDraw::default();
        assert!(draw.click([0.0, 0.0], false).is_none());
        assert!(draw.click([4.0, 0.0], false).is_none());
        // An arc to (4, 4) bulging out through (5, 2): counter-clockwise.
        draw.arc = true;
        assert!(draw.click([4.0, 4.0], false).is_none());
        assert_eq!(draw.arc_end, Some([4.0, 4.0]));
        assert!(draw.click([6.0, 2.0], false).is_none());
        draw.arc = false;
        assert!(draw.click([0.0, 4.0], false).is_none());
        let outline = draw.click([0.1, 0.1], true).ok_or("the outline closes")?;
        assert!(outline.closed());
        assert_eq!(outline.segments.len(), 4);
        let Segment::Arc { to, bulge } = outline.segments[1] else {
            return Err("an arc".into());
        };
        assert_eq!(to, [4.0, 4.0]);
        // A semicircle: through (6, 2), the chord's far end of a 2 mm radius.
        assert!((bulge - 1.0).abs() < 1e-9, "{bulge}");
        assert!(draw.start.is_none() && draw.segments.is_empty());
        Ok(())
    }

    #[test]
    fn two_lines_do_not_close_and_backspace_takes_back() {
        let mut draw = FillDraw::default();
        draw.click([0.0, 0.0], false);
        draw.click([4.0, 0.0], false);
        assert!(
            draw.click([0.0, 0.0], true).is_none(),
            "two segments are no fill"
        );
        let mut draw = FillDraw::default();
        draw.click([0.0, 0.0], false);
        draw.click([4.0, 0.0], false);
        draw.click([4.0, 4.0], false);
        draw.back();
        assert_eq!(draw.segments.len(), 1);
        draw.click([2.0, 3.0], false);
        assert!(
            draw.close()
                .is_some_and(|o| o.closed() && o.segments.len() == 3)
        );
    }

    #[test]
    fn handles_move_vertices_split_lines_and_bend_arcs() -> Result<(), String> {
        let o = square();
        let moved = edited(&o, Handle::Vertex(0), [-1.0, -1.0]).ok_or("vertex 0")?;
        assert_eq!(moved.start, [-1.0, -1.0]);
        assert!(moved.closed());
        let moved = edited(&o, Handle::Vertex(2), [5.0, 5.0]).ok_or("vertex 2")?;
        assert_eq!(moved.segments[1].to(), [5.0, 5.0]);
        assert!(edited(&o, Handle::Vertex(4), [5.0, 5.0]).is_none());
        let split = edited(&o, Handle::Mid(0), [2.0, -1.0]).ok_or("mid 0")?;
        assert_eq!(split.segments.len(), 5);
        assert_eq!(split.segments[0].to(), [2.0, -1.0]);
        assert_eq!(split.segments[1].to(), [4.0, 0.0]);
        // An arc's midpoint bends it: through (2, -2) on the bottom edge
        // makes a semicircle bulging out (counter-clockwise from (0,0)).
        let mut arced = o.clone();
        arced.segments[0] = Segment::Arc {
            to: [4.0, 0.0],
            bulge: 0.2,
        };
        let bent = edited(&arced, Handle::Mid(0), [2.0, -2.0]).ok_or("arc mid")?;
        let Segment::Arc { bulge, .. } = bent.segments[0] else {
            return Err("still an arc".into());
        };
        assert!((bulge - 1.0).abs() < 1e-9, "{bulge}");
        let mid = fill_paint::midpoints(&bent)[0];
        assert!((mid[0] - 2.0).abs() < 1e-9 && (mid[1] + 2.0).abs() < 1e-9);
        Ok(())
    }

    #[test]
    fn removing_a_vertex_keeps_the_outline_closed() -> Result<(), String> {
        let mut o = square();
        o.segments.insert(1, Segment::Line { to: [4.0, 2.0] });
        let a = without_vertex(&o, 2).ok_or("vertex 2")?;
        assert_eq!(a.segments.len(), 4);
        assert!(a.closed());
        let b = without_vertex(&o, 0).ok_or("vertex 0")?;
        assert_eq!(b.start, [4.0, 0.0]);
        assert!(b.closed());
        let triangle = without_vertex(&square(), 1).ok_or("square to triangle")?;
        assert_eq!(triangle.segments.len(), 3);
        assert!(without_vertex(&triangle, 1).is_none(), "a triangle stays");
        Ok(())
    }
}
