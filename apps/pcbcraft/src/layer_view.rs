//! Which board layers the 3D view shows. Display-only: changing it changes
//! the draw lists and picking, never the model geometry. PCB 2D's layer
//! toggles share the overlapping entries (outer copper and silkscreen).
//!
//! The popover on the 3D view's floating navigation bar lists the toggles
//! in the order the layers lie in the board, top to bottom.
use crate::shell;
use egui::{Align2, FontId, Sense, Ui, pos2, vec2};
use pcbcraft_core::{CopperLayer, Layer};
use pcbcraft_solidcraft::material::BodyKind;
use pcbcraft_solidcraft::scene::Geometry;
use solidcraft_ui_egui::{
    theme::Tokens,
    ui_kit::{self, Button, Container, Surface},
};
use std::collections::BTreeSet;

/// Width of the layer popover's content.
const POPOVER_WIDTH: f32 = 280.0;
/// Height of a caption row in the popover.
const CAPTION_HEIGHT: f32 = 22.0;

/// One toggle of the 3D layer popover.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ViewLayer {
    /// Component bodies, leads, terminations and solder.
    Bodies,
    Silkscreen(Layer),
    Mask(Layer),
    Copper(CopperLayer),
    /// The laminate and its edge bands.
    Substrate,
    Vias,
}

impl ViewLayer {
    /// The toggle's label in the popover.
    pub fn label(self) -> String {
        let side = |side: Layer| match side {
            Layer::Top => "Top",
            Layer::Bottom => "Bottom",
        };
        match self {
            Self::Bodies => "Components".into(),
            Self::Silkscreen(s) => format!("{} silkscreen", side(s)),
            Self::Mask(s) => format!("{} solder mask", side(s)),
            Self::Copper(layer) => format!("{} copper", layer.name()),
            Self::Substrate => "Substrate".into(),
            Self::Vias => "Vias".into(),
        }
    }

    /// The popover's toggles for a board with the `copper` layers (top to
    /// bottom), in the order the layers lie: components and the top films
    /// above the copper, the laminate between the copper, the bottom films
    /// below it, then the vias that pass through it all.
    pub fn stack(copper: &[CopperLayer]) -> Vec<ViewLayer> {
        let mut out = vec![
            Self::Bodies,
            Self::Silkscreen(Layer::Top),
            Self::Mask(Layer::Top),
        ];
        let mut layers: Vec<CopperLayer> = copper.to_vec();
        for outer in [CopperLayer::Top, CopperLayer::Bottom] {
            if !layers.contains(&outer) {
                layers.push(outer);
            }
        }
        layers.sort();
        let (bottom, above): (Vec<CopperLayer>, Vec<CopperLayer>) =
            layers.into_iter().partition(|l| *l == CopperLayer::Bottom);
        out.extend(above.into_iter().map(Self::Copper));
        out.push(Self::Substrate);
        out.extend(bottom.into_iter().map(Self::Copper));
        out.extend([
            Self::Mask(Layer::Bottom),
            Self::Silkscreen(Layer::Bottom),
            Self::Vias,
        ]);
        out
    }

    /// The toggle a body belongs to. Films and copper read their side or
    /// layer from `layer`; a mask cap without one belongs to the top.
    /// Copper on no single layer (a via's or a through-hole pad's plated
    /// barrel, a via's finished ring) is the Vias toggle's: Vias shows
    /// every plated hole. The foils' lines at the routed edge are part of
    /// the edge's stackup bands, with the substrate.
    pub fn of(kind: BodyKind, layer: Option<CopperLayer>) -> Self {
        let side = layer.and_then(CopperLayer::side).unwrap_or(Layer::Top);
        match kind {
            BodyKind::Substrate | BodyKind::EdgeDielectric | BodyKind::EdgeCopper => {
                Self::Substrate
            }
            BodyKind::MaskTop => Self::Mask(Layer::Top),
            BodyKind::MaskBottom => Self::Mask(Layer::Bottom),
            BodyKind::MaskCap => Self::Mask(side),
            BodyKind::Silkscreen => Self::Silkscreen(side),
            BodyKind::CopperTrack | BodyKind::PadCopper | BodyKind::PadFinish => {
                layer.map_or(Self::Vias, Self::Copper)
            }
            BodyKind::ViaCopper | BodyKind::ViaBore => Self::Vias,
            BodyKind::PackageBody
            | BodyKind::ChipResistor
            | BodyKind::ChipCapacitor
            | BodyKind::Termination
            | BodyKind::Overcoat
            | BodyKind::Solder
            | BodyKind::PartBody
            | BodyKind::ModelBody
            | BodyKind::ModelLead => Self::Bodies,
        }
    }
}

/// The hidden layers; everything else is shown.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LayerView {
    hidden: BTreeSet<ViewLayer>,
}

impl LayerView {
    #[cfg(test)]
    /// The view the old substrate toggle gave: the laminate and both masks
    /// hidden together, or nothing hidden.
    pub fn substrate_hidden(hidden: bool) -> Self {
        let mut view = Self::default();
        for layer in [
            ViewLayer::Substrate,
            ViewLayer::Mask(Layer::Top),
            ViewLayer::Mask(Layer::Bottom),
        ] {
            view.set(layer, !hidden);
        }
        view
    }

    /// X-ray: only the copper and vias, every film, the laminate and the
    /// components hidden, so the stackup's copper shows layer over layer.
    pub fn x_ray() -> Self {
        let mut view = Self::default();
        for side in [Layer::Top, Layer::Bottom] {
            view.set(ViewLayer::Mask(side), false);
            view.set(ViewLayer::Silkscreen(side), false);
        }
        view.set(ViewLayer::Substrate, false);
        view.set(ViewLayer::Bodies, false);
        view
    }

    /// Only one copper layer (and nothing else) shows.
    #[cfg(test)]
    pub fn only(copper: CopperLayer, stack: &[CopperLayer]) -> Self {
        let mut view = Self::default();
        for layer in ViewLayer::stack(stack) {
            view.set(layer, layer == ViewLayer::Copper(copper));
        }
        view
    }

    /// Whether any layer is hidden.
    pub fn hides_any(&self) -> bool {
        !self.hidden.is_empty()
    }

    pub fn visible(&self, layer: ViewLayer) -> bool {
        !self.hidden.contains(&layer)
    }

    pub fn set(&mut self, layer: ViewLayer, visible: bool) {
        if visible {
            self.hidden.remove(&layer);
        } else {
            self.hidden.insert(layer);
        }
    }

    /// Whether a body is drawn and pickable.
    pub fn shows(&self, geometry: &Geometry) -> bool {
        self.visible(ViewLayer::of(geometry.kind, geometry.layer))
    }
}

/// The navigation bar's layer button and its popover of toggles for a
/// board with the `copper` layers. Returns whether `view` changed. The
/// button is outlined while a layer is hidden.
pub fn button(ui: &mut Ui, view: &mut LayerView, copper: &[CopperLayer]) -> bool {
    let t = Tokens::get();
    let response = shell::icon_button(ui, "eye", "Layers", true);
    if view.hides_any() {
        ui.painter().rect_stroke(
            response.rect,
            t.radius_sm,
            egui::Stroke::new(1.0, t.accent),
            egui::StrokeKind::Inside,
        );
    }
    let mut changed = false;
    Container::new(ui.ctx(), Surface::Popup)
        .menu(&response)
        .align(egui::RectAlign::TOP)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            crate::kit::form(ui, |ui| {
                ui.set_width(POPOVER_WIDTH);
                changed = popover(ui, view, copper);
            });
        });
    changed
}

/// The popover's content: a caption, one check per layer and Show all.
fn popover(ui: &mut Ui, view: &mut LayerView, copper: &[CopperLayer]) -> bool {
    let t = Tokens::get();
    let (rect, _) = ui.allocate_exact_size(vec2(POPOVER_WIDTH, CAPTION_HEIGHT), Sense::hover());
    ui.painter().text(
        pos2(rect.left() + t.space_4, rect.center().y),
        Align2::LEFT_CENTER,
        "Layers",
        FontId::proportional(t.font_caption),
        t.text_dim,
    );
    let mut changed = false;
    for layer in ViewLayer::stack(copper) {
        let mut shown = view.visible(layer);
        if crate::kit::check_row(ui, &layer.label(), &mut shown) {
            view.set(layer, shown);
            changed = true;
        }
    }
    ui.horizontal(|ui| {
        let x_ray = LayerView::x_ray();
        let preset = ui_kit::button(ui, "X-ray", Button::Enabled(*view != x_ray))
            .on_hover_text("Show only the copper layers and vias, through the board");
        if preset.clicked() && *view != x_ray {
            *view = x_ray;
            changed = true;
        }
        let all = ui_kit::button(ui, "Show all", Button::Enabled(view.hides_any()));
        if all.clicked() && view.hides_any() {
            *view = LayerView::default();
            changed = true;
        }
    });
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_substrate_view_hides_exactly_the_board_level_bodies() {
        let view = LayerView::substrate_hidden(true);
        for kind in BodyKind::ALL {
            for layer in [None, Some(CopperLayer::Top), Some(CopperLayer::Bottom)] {
                let shown = view.visible(ViewLayer::of(kind, layer));
                assert_eq!(
                    shown,
                    !(kind.is_board()
                        || kind == BodyKind::EdgeDielectric
                        || kind == BodyKind::EdgeCopper),
                    "{kind:?}"
                );
            }
        }
        assert_eq!(LayerView::substrate_hidden(false), LayerView::default());
    }

    #[test]
    fn the_popover_lists_every_layer_once_in_board_order() {
        let four = [
            CopperLayer::Top,
            CopperLayer::Inner(1),
            CopperLayer::Inner(2),
            CopperLayer::Bottom,
        ];
        let stack = ViewLayer::stack(&four);
        let names: Vec<String> = stack.iter().map(|l| l.label()).collect();
        assert_eq!(
            names,
            [
                "Components",
                "Top silkscreen",
                "Top solder mask",
                "Top copper",
                "Inner 1 copper",
                "Inner 2 copper",
                "Substrate",
                "Bottom copper",
                "Bottom solder mask",
                "Bottom silkscreen",
                "Vias",
            ]
        );
        // Every body kind on every layer of the board has its toggle.
        for kind in BodyKind::ALL {
            for layer in four.iter().copied().map(Some).chain([None]) {
                let toggle = ViewLayer::of(kind, layer);
                assert!(stack.contains(&toggle), "{kind:?} on {layer:?}");
            }
        }
        // The outer layers are always listed, however the list is given.
        let two = ViewLayer::stack(&[CopperLayer::Top, CopperLayer::Bottom]);
        assert_eq!(ViewLayer::stack(&[]), two);
        assert_eq!(two.len(), stack.len() - 2);
    }

    /// Where the popover drew `label`, after one frame.
    fn text_at(output: &egui::FullOutput, label: &str) -> Option<egui::Pos2> {
        output
            .shapes
            .iter()
            .find_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text) if text.galley.text() == label => {
                    Some(text.pos + text.galley.rect.center().to_vec2())
                }
                _ => None,
            })
    }

    /// Hover, press and release on `label`; whether the view changed.
    fn click(ctx: &egui::Context, view: &mut LayerView, label: &str) -> Result<bool, String> {
        let copper = [CopperLayer::Top, CopperLayer::Bottom];
        let frame = |view: &mut LayerView, events: Vec<egui::Event>| {
            let mut changed = false;
            let input = egui::RawInput {
                events,
                ..Default::default()
            };
            let output = ctx.run_ui(input, |ui| changed = popover(ui, view, &copper));
            (output, changed)
        };
        let (output, _) = frame(view, vec![]);
        let at = text_at(&output, label).ok_or_else(|| format!("no {label}"))?;
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        let mut changed = false;
        for events in [
            vec![egui::Event::PointerMoved(at)],
            vec![button(true)],
            vec![button(false)],
        ] {
            changed |= frame(view, events).1;
        }
        Ok(changed)
    }

    /// Copper on no single layer is the Vias toggle's; X-ray shows only
    /// copper and vias; `only` one copper layer.
    #[test]
    fn plated_barrels_follow_vias_and_x_ray_shows_only_copper() {
        for kind in [
            BodyKind::PadCopper,
            BodyKind::PadFinish,
            BodyKind::CopperTrack,
        ] {
            assert_eq!(ViewLayer::of(kind, None), ViewLayer::Vias);
            assert_eq!(
                ViewLayer::of(kind, Some(CopperLayer::Bottom)),
                ViewLayer::Copper(CopperLayer::Bottom)
            );
        }
        let four = [
            CopperLayer::Top,
            CopperLayer::Inner(1),
            CopperLayer::Inner(2),
            CopperLayer::Bottom,
        ];
        let x_ray = LayerView::x_ray();
        for layer in ViewLayer::stack(&four) {
            let copper = matches!(layer, ViewLayer::Copper(_) | ViewLayer::Vias);
            assert_eq!(x_ray.visible(layer), copper, "{layer:?}");
        }
        let inner = LayerView::only(CopperLayer::Inner(2), &four);
        for layer in ViewLayer::stack(&four) {
            assert_eq!(
                inner.visible(layer),
                layer == ViewLayer::Copper(CopperLayer::Inner(2)),
                "{layer:?}"
            );
        }
    }

    #[test]
    fn the_popover_toggles_layers_and_shows_all_again() -> Result<(), String> {
        let ctx = egui::Context::default();
        let mut view = LayerView::default();
        assert!(click(&ctx, &mut view, "Top solder mask")?);
        assert!(!view.visible(ViewLayer::Mask(Layer::Top)));
        assert!(click(&ctx, &mut view, "Substrate")?);
        assert!(!view.visible(ViewLayer::Substrate));
        assert!(view.visible(ViewLayer::Copper(CopperLayer::Top)));
        assert!(click(&ctx, &mut view, "Show all")?);
        assert_eq!(view, LayerView::default());
        assert!(
            !click(&ctx, &mut view, "Show all")?,
            "nothing hidden: Show all is disabled"
        );
        Ok(())
    }
}
