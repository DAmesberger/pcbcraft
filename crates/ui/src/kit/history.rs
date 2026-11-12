//! Shared history chips, dense name rows, drop bars and rollback markers.
use crate::kit::BackingTooltipExt as _;
use crate::tokens::Tokens;
use egui::{Rect, pos2, vec2};

/// Browser and named history rows use the same density.
pub const TREE_ROW_HEIGHT: f32 = 26.0;

#[derive(Clone, Copy, PartialEq)]
pub enum HistoryStyle {
    Bottom,
    Icons,
    Names,
}

impl HistoryStyle {}

/// The host's names for the Play control: playing History, and stopping it while it plays.
#[derive(Clone, Copy, Debug)]
pub struct PlayLabels<'a> {
    pub play: &'a str,
    pub stop: &'a str,
}

/// The same controls are used in every History orientation: first, previous, Play History, next
/// and last, or the four without Play when `history_play_button` is off.
#[derive(Clone, Copy, PartialEq)]
pub enum HistoryTransport {
    First,
    Back,
    Play,
    Forward,
    Last,
}

impl HistoryTransport {
    pub fn id(self) -> &'static str {
        match self {
            Self::First => "first",
            Self::Back => "back",
            Self::Play => "play",
            Self::Forward => "forward",
            Self::Last => "last",
        }
    }

    pub fn label(self, playing: bool, play: PlayLabels<'_>) -> &str {
        match self {
            Self::First => solidcraft_ui_text::tr("ui.history_first"),
            Self::Back => solidcraft_ui_text::tr("ui.history_back"),
            Self::Play if playing => play.stop,
            Self::Play => play.play,
            Self::Forward => solidcraft_ui_text::tr("ui.history_forward"),
            Self::Last => solidcraft_ui_text::tr("ui.history_last"),
        }
    }

    /// `playing` turns the Play button into Stop, named by `play`; the other controls ignore
    /// both. Published as `history:` and its [`HistoryTransport::id`].
    pub fn show(self, ui: &mut egui::Ui, rect: Rect, playing: bool, play: PlayLabels<'_>) -> egui::Response {
        self.show_as(ui, rect, playing, "history:", play)
    }

    /// [`HistoryTransport::show`], published in the widget registry as `prefix` followed by its
    /// [`HistoryTransport::id`].
    pub fn show_as(self, ui: &mut egui::Ui, rect: Rect, playing: bool, prefix: &str, play: PlayLabels<'_>) -> egui::Response {
        let t = Tokens::get();
        let response = ui.interact(rect, ui.scope_id().with(("history_transport", self.id())), egui::Sense::click());
        solidcraft_ui_registry::publish_target(&format!("{prefix}{}", self.id()), rect);
        if response.hovered() {
            ui.painter().rect_filled(rect, t.radius_sm, t.hover);
        }
        let glyph = if self == Self::Play && playing { "pause" } else { self.id() };
        crate::kit::icons::glyphs::transport_glyph(ui.painter(), rect.center(), glyph, t.icon);
        let label = self.label(playing, play);
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), label));
        response.on_hover_cursor(egui::CursorIcon::PointingHand).backing_on_hover_text(label)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum HistoryHandleState {
    Rest,
    Hover,
    Drag,
}

/// Seam geometry is independent of the strip's contents and consumes no layout space.
pub struct HistorySeam {
    pub grip: Rect,
    pub pill: Rect,
    pub edge: Rect,
}

impl HistorySeam {
    pub fn new(card: Rect) -> Self {
        let t = Tokens::get();
        Self {
            grip: Rect::from_center_size(pos2(card.right() - t.space_2 - t.space_3 / 2.0, card.center().y), vec2(t.space_3, TREE_ROW_HEIGHT)),
            pill: Rect::from_center_size(card.right_center(), vec2(t.space_16 + t.space_2, t.control_height + t.space_16)),
            edge: Rect::from_min_max(pos2(card.right() - t.space_10, card.top()), card.right_bottom()),
        }
    }
}

pub struct HistorySeamResponse {
    pub toggle: bool,
    pub width: Option<f32>,
    pub released: bool,
    #[cfg(feature = "test-support")]
    pub response: egui::Response,
}
