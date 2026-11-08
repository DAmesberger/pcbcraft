//! The PCB editor's side of the command registry (`crate::commands`): its
//! keyboard context, the board shape tools' keys and the display toggles
//! the command palette runs. Toggles change view state only; nothing here
//! changes the document or rebuilds the board.
use super::{PcbEditor, Tool, board_shape::ShapeTool};
use crate::commands::{Chord, Context, KeyDecl, PcbView, ToolEntry};
use crate::layer_view::ViewLayer;
use pcbcraft_core::CopperLayer;

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
            also: &[],
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
            PcbView::LayerFocus => self.focus != super::LayerFocus::All,
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
            PcbView::LayerFocus => self.cycle_layer_focus(),
            PcbView::HighlightNet => {
                self.highlighted = if shown { None } else { self.net };
            }
        }
        // `layer_view` merges the shared toggles, so 3D follows.
    }

    /// Layer `index` of the stackup (0: the top); `Err` says the board has
    /// fewer layers.
    pub fn layer_at(&self, index: usize) -> Result<CopperLayer, String> {
        self.stack.get(index).copied().ok_or_else(|| {
            format!(
                "The board has {} copper layers; the stackup sets them",
                self.stack.len()
            )
        })
    }

    /// Make layer `index` of the stackup the active layer the Route, Via
    /// and Fill tools use. While routing, the route changes to it through
    /// a via at the pointer (at the head when the pointer is off the
    /// canvas), as V does. A layer the stackup lacks changes nothing.
    pub fn select_layer(&mut self, index: usize) {
        if let Ok(layer) = self.layer_at(index) {
            self.change_layer_to(layer, self.hover_at);
        }
    }

    /// The active layer as shown, with its key when it has one
    /// (`Inner 2 (3)`), else with the next-layer key.
    pub(super) fn layer_label(&self, ctx: &egui::Context) -> String {
        let name = self.layer.name();
        let key = self.layer_key_id().unwrap_or("route.via-next");
        crate::keys::labelled(ctx, &name, key)
    }

    /// The id of the active layer's key, when it has one.
    pub(super) fn layer_key_id(&self) -> Option<&'static str> {
        self.stack
            .iter()
            .position(|l| *l == self.layer)
            .and_then(layer_key)
    }
}

/// The active-layer keys: stackup layers 1 to 10 from the top, on 1 to 9
/// and 0. Registry commands of PCB › Layout (`Action::PcbLayer`), run by
/// the app's dispatcher, so the palette, Settings › Keyboard and the key
/// reveal show them and a rebinding moves them.
pub const LAYER_KEYS: [KeyDecl; 10] = [
    KeyDecl::new("pcb.layer-1", "1", "Switch to stackup layer 1"),
    KeyDecl::new("pcb.layer-2", "2", "Switch to stackup layer 2"),
    KeyDecl::new("pcb.layer-3", "3", "Switch to stackup layer 3"),
    KeyDecl::new("pcb.layer-4", "4", "Switch to stackup layer 4"),
    KeyDecl::new("pcb.layer-5", "5", "Switch to stackup layer 5"),
    KeyDecl::new("pcb.layer-6", "6", "Switch to stackup layer 6"),
    KeyDecl::new("pcb.layer-7", "7", "Switch to stackup layer 7"),
    KeyDecl::new("pcb.layer-8", "8", "Switch to stackup layer 8"),
    KeyDecl::new("pcb.layer-9", "9", "Switch to stackup layer 9"),
    KeyDecl::new("pcb.layer-10", "0", "Switch to stackup layer 10"),
];

/// The id of the active-layer key of stackup layer `index` (0: the top),
/// for the layers that have one.
pub fn layer_key(index: usize) -> Option<&'static str> {
    LAYER_KEYS.get(index).map(|d| d.id)
}
