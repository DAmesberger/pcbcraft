//! The PCB editor's side of the command registry (`crate::commands`): its
//! keyboard context, the board shape tools' keys and the display toggles
//! the command palette runs. Toggles change view state only; nothing here
//! changes the document or rebuilds the board.
use super::{PcbEditor, Tool, board_shape::ShapeTool};
use crate::commands::{Chord, Context, PcbView, ToolEntry};
use crate::layer_view::ViewLayer;

/// The board shape sketch's tools for the registry, with the keys its
/// canvas handles (`board_shape_ui`).
pub(crate) fn shape_tool_entries() -> Vec<ToolEntry> {
    ShapeTool::ALL
        .into_iter()
        .map(|t| ToolEntry {
            id: t.id(),
            label: t.label(),
            icon: t.icon(),
            chord: t.key().map(|(key, shift)| {
                let chord = Chord::key(key);
                if shift { chord.with_shift() } else { chord }
            }),
        })
        .collect()
}

impl PcbEditor {
    /// The innermost keyboard context: the view, then an armed tool's mode.
    pub fn shortcut_context(&self) -> Context {
        if self.view_3d {
            Context::Pcb3d
        } else if self.shape.active {
            Context::BoardShape
        } else {
            match self.tool {
                Tool::Route => Context::PcbRoute,
                Tool::Fill => Context::PcbFill,
                Tool::Tune => Context::PcbTune,
                Tool::Select | Tool::Via => Context::PcbLayout,
            }
        }
    }

    /// Whether a toggle can apply now: `Err` names why not.
    pub fn view_toggle_state(&self, toggle: PcbView) -> Result<bool, &'static str> {
        let l = &self.layers;
        Ok(match toggle {
            PcbView::Copper(layer) => {
                if !self.stack.contains(&layer) {
                    return Err("The board's stackup has no such layer");
                }
                l.copper_layer(layer)
            }
            PcbView::Vias => l.vias,
            PcbView::SilkTop => l.silk_top,
            PcbView::SilkBottom => l.silk_bottom,
            PcbView::Courtyard => l.courtyard,
            PcbView::Assembly => l.assembly,
            PcbView::Holes => l.holes,
            PcbView::Components => self.view_layers.visible(ViewLayer::Bodies),
            PcbView::FromBottom => self.from_bottom,
            PcbView::Airwires => self.show_airwires,
            PcbView::HighlightNet => {
                if self.highlighted.is_none() && self.net.is_none() {
                    return Err("Select a pad or track to choose its net");
                }
                self.highlighted.is_some()
            }
        })
    }

    /// Flip a display toggle. Unavailable toggles change nothing.
    pub fn view_toggle(&mut self, toggle: PcbView) {
        let Ok(shown) = self.view_toggle_state(toggle) else {
            return;
        };
        let l = &mut self.layers;
        match toggle {
            PcbView::Copper(layer) => {
                if let Some(flag) = l.copper_layer_mut(layer) {
                    *flag = !shown;
                }
            }
            PcbView::Vias => l.vias = !shown,
            PcbView::SilkTop => l.silk_top = !shown,
            PcbView::SilkBottom => l.silk_bottom = !shown,
            PcbView::Courtyard => l.courtyard = !shown,
            PcbView::Assembly => l.assembly = !shown,
            PcbView::Holes => l.holes = !shown,
            PcbView::Components => self.view_layers.set(ViewLayer::Bodies, !shown),
            PcbView::FromBottom => self.from_bottom = !shown,
            PcbView::Airwires => self.show_airwires = !shown,
            PcbView::HighlightNet => {
                self.highlighted = if shown { None } else { self.net };
            }
        }
        // `layer_view` merges the shared toggles, so 3D follows.
    }
}
