//! Board colours shared by the 2D layout canvas and the 3D model, so a copper
//! layer reads the same in both views. Selection and highlight come from
//! Solidcraft's tokens; layer and material colours are electronics domain
//! colours with light and dark variants.
use egui::Color32;
use solidcraft_ui_egui::theme::{self, Tokens};

pub struct BoardColors {
    pub copper_top: Color32,
    pub copper_bottom: Color32,
    pub substrate: Color32,
    pub package: Color32,
    pub part_body: Color32,
    pub selected: Color32,
}

pub fn board() -> BoardColors {
    let t = Tokens::get();
    let dark = theme::is_dark();
    BoardColors {
        copper_top: if dark {
            Color32::from_rgb(196, 139, 70)
        } else {
            Color32::from_rgb(176, 112, 40)
        },
        copper_bottom: if dark {
            Color32::from_rgb(83, 160, 208)
        } else {
            Color32::from_rgb(40, 116, 170)
        },
        substrate: if dark {
            Color32::from_rgb(36, 100, 70)
        } else {
            Color32::from_rgb(58, 128, 92)
        },
        package: Color32::from_rgb(75, 83, 95),
        part_body: Color32::from_rgb(112, 132, 153),
        selected: t.body_selected,
    }
}

impl BoardColors {
    pub fn copper(&self, layer: pcbcraft_core::Layer) -> Color32 {
        match layer {
            pcbcraft_core::Layer::Top => self.copper_top,
            pcbcraft_core::Layer::Bottom => self.copper_bottom,
        }
    }
}
