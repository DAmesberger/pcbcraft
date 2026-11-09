//! Board colours of the 2D layout canvas and the 3D selection. Selection and
//! highlight come from Solidcraft's tokens; layer colours are electronics
//! domain colours with light and dark variants. The 3D model's materials are
//! physical looks stored in the native file (`pcbcraft_solidcraft::material`).
use egui::Color32;
use solidcraft_theme::{self as theme, Tokens};

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
    /// A copper layer's colour: the outer layers' side colours, and a
    /// distinct hue per inner layer.
    pub fn copper_layer(&self, layer: pcbcraft_core::CopperLayer) -> Color32 {
        use pcbcraft_core::CopperLayer;
        let dark = theme::is_dark();
        match layer {
            CopperLayer::Top => self.copper_top,
            CopperLayer::Bottom => self.copper_bottom,
            CopperLayer::Inner(n @ 1..=4) => {
                let (d, l) = match n {
                    1 => ((206, 190, 76), (150, 132, 20)),
                    2 => ((112, 186, 104), (52, 128, 52)),
                    3 => ((196, 120, 204), (140, 60, 150)),
                    _ => ((86, 186, 186), (30, 124, 124)),
                };
                let (r, g, b) = if dark { d } else { l };
                Color32::from_rgb(r, g, b)
            }
            // Further inner layers: hues a golden angle apart, so
            // neighbouring layers stay apart on any count.
            CopperLayer::Inner(n) => {
                let hue = (f32::from(n) * 0.381_966).fract();
                let value = if dark { 0.80 } else { 0.56 };
                egui::ecolor::Hsva::new(hue, 0.55, value, 1.0).into()
            }
        }
    }

    pub fn copper(&self, layer: pcbcraft_core::Layer) -> Color32 {
        match layer {
            pcbcraft_core::Layer::Top => self.copper_top,
            pcbcraft_core::Layer::Bottom => self.copper_bottom,
        }
    }
}
