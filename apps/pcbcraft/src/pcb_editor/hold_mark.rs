//! What holds a placed part in place, and the small mark that says so on
//! the Layout canvas without selecting it.
//!
//! A part is free, held by an edge constraint (its position follows the
//! board's dimensions), or locked by hand. The two fixed states use
//! Solidcraft's constraint-glyph convention (`sketch_tools.rs`: a
//! `GLYPH`-sized box in `glyph_bg`, its icon, and a rim): a locked part
//! shows the Fix glyph (`c_fix`, Solidcraft's mark of fixed geometry) with
//! a `sketch_locked` rim; a constrained part shows the Dimension glyph with
//! a `dimension` rim, the colour its edge dimensions are drawn in. A free
//! part shows nothing, so a dense board gains marks only on the few parts
//! that are fixed. The mark keeps its screen size at every zoom, sits just
//! outside the top-right corner of the part's envelope, clear of its pads,
//! and shrinks to a dot of the same colour on the corner when the part is
//! smaller on screen than the glyph.
//! The Placement card's state row draws the same glyph beside its words.
use super::edge_card::{constraint_of, evaluate};
use super::{BoardView, body_polygon};
use pcbcraft_core::layout::outline::held_at;
use pcbcraft_core::{Placement, Project};
use solidcraft_ui_egui::{icons, theme::Tokens};

/// The glyph's side, in points (Solidcraft's `GLYPH`).
pub(super) const GLYPH: f32 = 15.0;
/// The dot's radius when the part is too small on screen for the glyph.
const DOT: f32 = 3.0;

/// What holds a placed part.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Hold {
    Locked,
    Constrained,
}

impl Hold {
    /// The part's hold: locked wins over a constraint (it refuses first).
    pub(super) fn of(project: &Project, placement: &Placement) -> Option<Self> {
        if placement.locked {
            Some(Self::Locked)
        } else if constraint_of(project, placement.instance).is_some() {
            Some(Self::Constrained)
        } else {
            None
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Self::Locked => "c_fix",
            Self::Constrained => "dimension",
        }
    }

    /// The rim and dot colour, from the shared tokens.
    pub(super) fn ink(self) -> egui::Color32 {
        let t = Tokens::get();
        match self {
            Self::Locked => t.sketch_locked,
            Self::Constrained => t.dimension,
        }
    }
}

/// Paint the glyph of `hold` centred on `at`, `size` points square.
pub(super) fn paint_glyph(painter: &egui::Painter, at: egui::Pos2, size: f32, hold: Hold) {
    let t = Tokens::get();
    let r = egui::Rect::from_center_size(at, egui::vec2(size, size));
    let radius = t.radius_sm.saturating_sub(1);
    painter.rect_filled(r, radius, t.glyph_bg);
    painter.rect_stroke(
        r,
        radius,
        egui::Stroke::new(1.0, hold.ink()),
        egui::StrokeKind::Inside,
    );
    icons::paint_icon(painter, r.shrink(2.0), hold.icon(), 1.0);
}

/// Where the mark of a part whose envelope covers `bounds` on screen sits:
/// just outside its top-right corner, clear of its pads, or on the corner
/// as a dot when the part is smaller on screen than the glyph.
pub(super) fn mark_at(bounds: egui::Rect) -> (egui::Pos2, bool) {
    let corner = bounds.right_top();
    if bounds.width().max(bounds.height()) < GLYPH {
        (corner, false)
    } else {
        let off = GLYPH / 2.0 + 1.0;
        (corner + egui::vec2(off, -off), true)
    }
}

/// Paint the mark of `hold` for a part whose envelope covers `bounds` on
/// screen ([`mark_at`]).
pub(super) fn paint_mark(painter: &egui::Painter, bounds: egui::Rect, hold: Hold) {
    match mark_at(bounds) {
        (at, true) => paint_glyph(painter, at, GLYPH, hold),
        (at, false) => {
            painter.circle_filled(at, DOT, hold.ink());
            painter.circle_stroke(at, DOT, egui::Stroke::new(1.0, Tokens::get().glyph_bg));
        }
    }
}

/// Mark every fixed part on the canvas, after the board is drawn: a part
/// being dragged is free by definition, so every mark stays put.
pub(super) fn paint_marks(painter: &egui::Painter, view: &BoardView, project: &Project) {
    for placement in &project.placements {
        let Some(hold) = Hold::of(project, placement) else {
            continue;
        };
        let Some(corners) = body_polygon(project, placement) else {
            continue;
        };
        let bounds = egui::Rect::from_points(&corners.map(|p| view.screen(p)));
        if painter.clip_rect().intersects(bounds.expand(GLYPH)) {
            paint_mark(painter, bounds, hold);
        }
    }
}

/// The Placement card's plain statement of what holds the part, in the
/// engine's words ("Held 2 mm from the right edge", "Locked in place").
/// `None` for a free part.
pub(super) fn state_words(project: &Project, placement: &Placement) -> Option<String> {
    let hold = Hold::of(project, placement)?;
    let held = constraint_of(project, placement.instance)
        .map(|c| held_at(&c.edge, evaluate(project, &c.offset)));
    Some(match (hold, held) {
        (Hold::Locked, Some(at)) => format!("Locked · held {at}"),
        (Hold::Locked, None) => "Locked in place".to_owned(),
        (Hold::Constrained, Some(at)) => format!("Held {at}"),
        (Hold::Constrained, None) => "Held to a board edge".to_owned(),
    })
}
