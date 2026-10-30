//! Board colours of the 2D layout canvas and the 3D selection. Selection and
//! highlight come from Solidcraft's tokens; layer colours are electronics
//! domain colours with light and dark variants. The 3D model's materials are
//! physical looks stored in the native file (`pcbcraft_solidcraft::material`).
use egui::Color32;
use solidcraft_ui_egui::theme::{self, Tokens};

pub struct BoardColors {
    pub copper_top: Color32,
    pub copper_bottom: Color32,
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
