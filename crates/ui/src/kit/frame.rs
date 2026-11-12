//! Dock dragging and bounded anchors. Outer hosts and surfaces live in `crate::kit::Container`.

/// Which edge of the viewport a docked panel sits against. The Sketch Palette holds the right
/// edge while a sketch is open, so a dialog started there takes the left.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Right,
    Left,
}

impl Side {
    /// Command dialogs share a position on each side of the viewport.
    pub fn dialog_offset_key(self) -> &'static str {
        match self {
            Side::Right => "sc_command_dialog_right",
            Side::Left => "sc_command_dialog_left",
        }
    }

    /// The x of the viewport edge this side docks against.
    pub fn anchor_x(self, viewport: egui::Rect) -> f32 {
        match self {
            Side::Right => viewport.right(),
            Side::Left => viewport.left(),
        }
    }
}
