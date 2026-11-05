//! Layer focus in Layout: the active copper layer is drawn on top at full
//! strength in every tool, and the focus mode decides how the other copper
//! layers show beneath it (all dimmed, strongly dimmed, or not at all).
//! Keepouts are an overlay over every copper layer, in every mode.
//!
//! Display state only: nothing here changes the document or rebuilds the
//! cached board. The paint order is in docs/pcb-realism.md (PCB 2D).
use super::{BoardView, Layers, MAX_INNER, PcbEditor};
use egui::{Align2, Color32, FontId, Pos2, Shape, Stroke};
use pcbcraft_core::model::fill::KeepoutLayers;
use pcbcraft_core::{CopperLayer, Id, Layer, Point, Project};
use solidcraft_ui_egui::theme::{self, Tokens};

/// How Layout shows the copper layers besides the active one. Cycled by
/// the registry command `pcb.layer-focus`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum LayerFocus {
    /// Every shown layer, dimmed beneath the active one.
    #[default]
    All,
    /// The other layers strongly dimmed.
    Dim,
    /// The active layer alone.
    Only,
}

impl LayerFocus {
    pub(crate) const ALL: [Self; 3] = [Self::All, Self::Dim, Self::Only];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::All => "All layers",
            Self::Dim => "Dim others",
            Self::Only => "Active only",
        }
    }

    /// The next mode of the cycle.
    pub(crate) fn next(self) -> Self {
        match self {
            Self::All => Self::Dim,
            Self::Dim => Self::Only,
            Self::Only => Self::All,
        }
    }

    pub(super) fn options() -> [(Self, String); 3] {
        Self::ALL.map(|f| (f, f.label().to_owned()))
    }
}

/// How one copper layer is drawn: its tracks and pads `shade` of the way
/// from the board; its fills solid (the active layer on the near side) or
/// `tint`-translucent with a thin outline, so a pour never hides the near
/// side's copper or another layer's copper above it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Look {
    pub(super) shade: f32,
    pub(super) tint: Option<f32>,
}

impl Look {
    pub(super) const ACTIVE: Self = Self {
        shade: 1.0,
        tint: None,
    };
}

/// The far side's share of its colour over the board.
pub(super) const FAR_SHADE: f32 = 0.45;
/// The other inner layers' share, and the near side's while another layer
/// is active.
pub(super) const OTHER_SHADE: f32 = 0.6;
/// Every other layer's share under Dim others.
pub(super) const DIM_SHADE: f32 = 0.2;
/// The opacity of the active layer's fill copper when the active layer is
/// not the near side (the far side or an inner layer): the near side's
/// pads and text read through it.
pub(super) const ACTIVE_FILL_TINT: f32 = 0.45;
/// The opacity of another layer's fill copper (All layers).
pub(super) const FILL_TINT: f32 = 0.22;
/// The opacity of another layer's fill copper under Dim others.
pub(super) const DIM_FILL_TINT: f32 = 0.08;
/// The keepout hatching's opacity on the active layer, and on others.
pub(super) const KEEPOUT_HATCH_HERE: f32 = 0.55;
pub(super) const KEEPOUT_HATCH_OTHER: f32 = 0.35;
/// Keepout hatch lines are this many points apart on screen.
const KEEPOUT_HATCH_PX: f32 = 9.0;
/// Most hatch lines across one keepout.
const MAX_KEEPOUT_LINES: usize = 1024;

/// A keepout as Layout draws it, per revision: board keepouts and the
/// placed parts' footprint keepouts.
pub(super) struct KeepoutShape {
    /// The placed part a footprint keepout follows while it is dragged.
    pub(super) instance: Option<Id>,
    pub(super) label: String,
    pub(super) ring: Vec<Point>,
    pub(super) layers: KeepoutLayers,
}

impl KeepoutShape {
    fn on(&self, layer: CopperLayer) -> bool {
        match &self.layers {
            KeepoutLayers::All => true,
            KeepoutLayers::Only(layers) => layers.contains(&layer),
        }
    }
}

/// Every keepout of `project`, its outline flattened.
pub(super) fn keepout_shapes(project: &Project) -> Vec<KeepoutShape> {
    let name = |name: &str, layers: &KeepoutLayers| {
        let what = if name.is_empty() { "Keepout" } else { name };
        let on = match layers {
            KeepoutLayers::All => "all layers".to_owned(),
            KeepoutLayers::Only(layers) => layers
                .iter()
                .map(|l| l.name())
                .collect::<Vec<_>>()
                .join(", "),
        };
        format!("{what} · {on}")
    };
    let board = project.keepouts.iter().map(|k| KeepoutShape {
        instance: None,
        label: name(&k.name, &k.layers),
        ring: super::fill_paint::outline_ring(&k.outline),
        layers: k.layers.clone(),
    });
    let placed = pcbcraft_core::layout::placed_keepouts(project)
        .into_iter()
        .map(|p| KeepoutShape {
            instance: Some(p.instance),
            label: name(&p.keepout.name, &p.keepout.layers),
            ring: super::fill_paint::outline_ring(&p.keepout.outline),
            layers: p.keepout.layers,
        });
    board.chain(placed).collect()
}

/// The keepout overlay's colour: a domain colour apart from every copper
/// layer's.
pub(super) fn keepout_color() -> Color32 {
    if theme::is_dark() {
        Color32::from_rgb(236, 92, 92)
    } else {
        Color32::from_rgb(196, 36, 36)
    }
}

impl PcbEditor {
    /// The copper the canvas draws and picks: the layer toggles, without
    /// inner layers the stackup lacks, and under Active only without the
    /// other copper layers.
    pub(super) fn seen(&self) -> Layers {
        let mut l = self.layers;
        for (n, flag) in (1..=MAX_INNER).zip(l.inner.iter_mut()) {
            let layer = u8::try_from(n).ok().map(CopperLayer::Inner);
            let in_stack = layer.is_some_and(|l| self.stack.contains(&l));
            let other = self.focus == LayerFocus::Only && layer != Some(self.layer);
            if !in_stack || other {
                *flag = false;
            }
        }
        if self.focus == LayerFocus::Only {
            l.copper_top &= self.layer == CopperLayer::Top;
            l.copper_bottom &= self.layer == CopperLayer::Bottom;
        }
        l
    }

    /// How `layer` is drawn now: the active layer at full strength (its
    /// pours translucent unless it is the near side); the others by the
    /// focus mode, deeper layers darker under All layers; `None` when it
    /// is not drawn.
    pub(super) fn look(&self, layer: CopperLayer, near: Layer) -> Option<Look> {
        if layer == self.layer {
            return Some(if layer == CopperLayer::from(near) {
                Look::ACTIVE
            } else {
                Look {
                    shade: 1.0,
                    tint: Some(ACTIVE_FILL_TINT),
                }
            });
        }
        match self.focus {
            LayerFocus::Only => None,
            LayerFocus::Dim => Some(Look {
                shade: DIM_SHADE,
                tint: Some(DIM_FILL_TINT),
            }),
            LayerFocus::All => {
                let far = CopperLayer::from(if near == Layer::Top {
                    Layer::Bottom
                } else {
                    Layer::Top
                });
                Some(Look {
                    shade: if layer == far { FAR_SHADE } else { OTHER_SHADE },
                    tint: Some(FILL_TINT),
                })
            }
        }
    }

    /// The copper layers in paint order: every one but the active layer
    /// from the farthest from the viewer to the nearest, then the active
    /// layer.
    pub(super) fn paint_order(&self) -> Vec<CopperLayer> {
        let mut order: Vec<CopperLayer> = self
            .stack
            .iter()
            .copied()
            .filter(|l| *l != self.layer)
            .collect();
        // The stack runs top to bottom; from the top, the bottom is far.
        if !self.from_bottom {
            order.reverse();
        }
        if self.stack.contains(&self.layer) {
            order.push(self.layer);
        }
        order
    }

    /// Cycle the focus mode (`pcb.layer-focus`).
    pub(crate) fn cycle_layer_focus(&mut self) {
        self.focus = self.focus.next();
    }

    /// The view popover's Focus row, with its key chip.
    pub(super) fn focus_row(&mut self, ui: &mut egui::Ui) {
        let key = crate::keys::shown(ui.ctx(), "pcb.layer-focus");
        crate::kit::keyed(ui, key, |ui| {
            crate::kit::choice_row(ui, "Focus", &mut self.focus, &LayerFocus::options())
        });
    }

    /// Every keepout over all copper, in every focus mode: its outline
    /// (solid on the active layer, dashed on others), diagonal hatching
    /// and a label naming the layers it applies to.
    pub(super) fn paint_keepouts(&self, painter: &egui::Painter, view: BoardView) {
        let color = keepout_color();
        let t = Tokens::get();
        for keepout in &self.keepouts {
            let delta = keepout.instance.map_or([0.0, 0.0], |i| self.drag_delta(i));
            let ring: Vec<Pos2> = keepout
                .ring
                .iter()
                .map(|p| view.screen([p[0] + delta[0], p[1] + delta[1]]))
                .collect();
            let Some(first) = ring.first().copied() else {
                continue;
            };
            let here = keepout.on(self.layer);
            let line = Stroke::new(if here { 1.5 } else { 1.0 }, color);
            let hatch = Stroke::new(
                1.0,
                color.gamma_multiply(if here {
                    KEEPOUT_HATCH_HERE
                } else {
                    KEEPOUT_HATCH_OTHER
                }),
            );
            for [a, b] in screen_hatch(&ring, painter.clip_rect(), KEEPOUT_HATCH_PX) {
                painter.line_segment([a, b], hatch);
            }
            let mut closed = ring.clone();
            closed.push(first);
            if here {
                painter.add(Shape::line(closed, line));
            } else {
                painter.extend(Shape::dashed_line(&closed, line, 6.0, 4.0));
            }
            // The label just inside the topmost corner.
            if let Some(corner) = ring.iter().copied().min_by(|a, b| {
                (a.y, a.x)
                    .partial_cmp(&(b.y, b.x))
                    .unwrap_or(std::cmp::Ordering::Equal)
            }) {
                painter.text(
                    corner + egui::vec2(t.space_4, t.space_4),
                    Align2::LEFT_TOP,
                    &keepout.label,
                    FontId::proportional(t.font_small),
                    color,
                );
            }
        }
    }
}

/// Diagonal (45°) hatch segments `spacing` points apart inside the closed
/// screen polygon `ring` (even-odd), only where `clip` shows them.
pub(super) fn screen_hatch(ring: &[Pos2], clip: egui::Rect, spacing: f32) -> Vec<[Pos2; 2]> {
    // Lines x − y = c: in u = x − y, v = x + y they are u = const.
    let u = |p: Pos2| p.x - p.y;
    let n = ring.len();
    if n < 3 || spacing <= 0.0 {
        return Vec::new();
    }
    let span = |points: &mut dyn Iterator<Item = f32>| {
        points.fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), v| {
            (lo.min(v), hi.max(v))
        })
    };
    let (lo, hi) = span(&mut ring.iter().map(|p| u(*p)));
    let corners = [
        clip.left_top(),
        clip.right_top(),
        clip.left_bottom(),
        clip.right_bottom(),
    ];
    let (clip_lo, clip_hi) = span(&mut corners.iter().map(|p| u(*p)));
    let (lo, hi) = (lo.max(clip_lo), hi.min(clip_hi));
    if !(lo.is_finite() && hi.is_finite()) || lo > hi {
        return Vec::new();
    }
    let first = (lo / spacing).ceil() as i64;
    let last = (hi / spacing).floor() as i64;
    if usize::try_from(last.saturating_sub(first)).map_or(true, |k| k > MAX_KEEPOUT_LINES) {
        return Vec::new();
    }
    let mut out = Vec::new();
    for k in first..=last {
        let c = k as f32 * spacing;
        // Where each edge crosses u = c, by its v = x + y.
        let mut vs: Vec<f32> = Vec::new();
        for i in 0..n {
            let (a, b) = (ring[i], ring[(i + 1) % n]);
            let (ua, ub) = (u(a), u(b));
            if (ua > c) != (ub > c) {
                let s = (c - ua) / (ub - ua);
                vs.push((a.x + a.y) + s * ((b.x + b.y) - (a.x + a.y)));
            }
        }
        vs.sort_by(f32::total_cmp);
        for pair in vs.chunks_exact(2) {
            let at = |v: f32| Pos2::new((c + v) / 2.0, (v - c) / 2.0);
            out.push([at(pair[0]), at(pair[1])]);
        }
    }
    out
}
