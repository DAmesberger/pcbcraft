//! Numbered stage navigation in a two-row workspace card. The caller owns commands and state.
use super::{Container, LayoutMode, Surface};
use egui::{Rect, pos2, vec2};

/// The workspace switcher card, the ribbon card beside it and the bottom of the pair, laid out
/// in the space above the viewport.
#[derive(Clone, Copy, Debug)]
pub struct Cards {
    pub workspace: Rect,
    pub ribbon: Rect,
    pub bottom: f32,
}

impl Cards {
    /// The cards for `mode`, the switcher `workspace_width` wide (or as wide as there is room).
    pub fn new(available: Rect, mode: LayoutMode, ribbon_height: f32, workspace_width: f32) -> Self {
        let margin = Container::for_layout(mode, Surface::Chrome).gap();
        let gap = margin;
        let left = available.left() + margin;
        let top = available.top() + margin;
        let right = (available.right() - margin).max(left);
        let workspace = Rect::from_min_size(
            pos2(left, top),
            vec2(
                workspace_width.min((right - left).max(0.0)),
                ribbon_height - if !Container::for_layout(mode, Surface::Chrome).floating() { 1.0 } else { 0.0 },
            ),
        );
        let ribbon = Rect::from_min_max(pos2((workspace.right() + gap).min(right), top), pos2(right, top + ribbon_height));
        Self { workspace, ribbon, bottom: ribbon.bottom() + margin }
    }
}

pub struct StageAction<'a> {
    pub icon: Option<&'a str>,
    pub label: &'a str,
    pub enabled: bool,
    pub primary: bool,
    pub reason: Option<&'a str>,
}

pub struct StageResponse {
    pub stage: Option<usize>,
    pub action: Option<usize>,
    pub chips: Vec<Rect>,
    pub actions: Vec<Rect>,
}

pub struct StageBar;
impl StageBar {}
