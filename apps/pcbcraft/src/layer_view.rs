//! Which board layers the 3D view shows. Display-only: changing it changes
//! the draw lists and picking, never the model geometry. PCB 2D's layer
//! toggles share the overlapping entries (outer copper and silkscreen).
//!
//! The popover on the 3D view's floating navigation bar lists the toggles
//! in the order the layers lie in the board, top to bottom.
//!
//! Markings (founder decision F7) are display-only decals on the component
//! bodies: only the appearance kind [`BodyKind::Marking`] maps to their
//! toggle, and no saved body has it. They are drawn while
//! [`LayerView::shows_markings`]: hiding the bodies hides them too.
use crate::shell;
use egui::{Align2, FontId, Sense, Ui, pos2, vec2};
use pcbcraft_core::{CopperLayer, Layer};
use pcbcraft_solidcraft::material::BodyKind;
use pcbcraft_solidcraft::scene::Geometry;
use pcbcraft_solidcraft::stack::{Slot, Stack};
use solidcraft_theme::Tokens;
use solidcraft_ui_egui::ui_kit::{self, Button, Container, Surface};
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
    /// The display-only marking decals on the bodies (top marks, value
    /// codes, polarity marks and bands). Never saved, never fabricated.
    Markings,
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
            Self::Markings => "Markings".into(),
            Self::Silkscreen(s) => format!("{} silkscreen", side(s)),
            Self::Mask(s) => format!("{} solder mask", side(s)),
            Self::Copper(layer) => format!("{} copper", layer.name()),
            Self::Substrate => "Substrate".into(),
            Self::Vias => "Vias".into(),
        }
    }

    /// The popover's toggles for a board of `stack`, in the order the
    /// layers lie: components and their markings, the stack's films and
    /// foils top to bottom (a finish shows with its copper), then the vias
    /// that pass through it all.
    pub fn stack(stack: &Stack) -> Vec<ViewLayer> {
        let mut out = vec![Self::Bodies, Self::Markings];
        for (slot, _) in stack.slots() {
            let layer = Self::from(*slot);
            if !out.contains(&layer) {
                out.push(layer);
            }
        }
        out.push(Self::Vias);
        out
    }

    /// The toggle a body belongs to: its stack slot's
    /// ([`BodyKind::slot`]). Vias shows every plated hole; the foils'
    /// lines at the routed edge are part of the edge's stackup bands, with
    /// the substrate.
    pub fn of(kind: BodyKind, layer: Option<CopperLayer>) -> Self {
        Self::from(kind.slot(layer))
    }
}

impl From<Slot> for ViewLayer {
    /// The toggle of a slot: a body's films show with its bodies, a finish
    /// with the copper it plates.
    fn from(slot: Slot) -> Self {
        match slot {
            Slot::Bodies | Slot::BodyFilms => Self::Bodies,
            Slot::Marking => Self::Markings,
            Slot::Silk(side) => Self::Silkscreen(side),
            Slot::Mask(side) => Self::Mask(side),
            Slot::Finish(side) => Self::Copper(side.into()),
            Slot::Copper(layer) => Self::Copper(layer),
            Slot::Substrate => Self::Substrate,
            Slot::Vias => Self::Vias,
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

    /// X-ray: only the copper and vias, every film, the laminate, the
    /// components and their markings hidden, so the stackup's copper shows
    /// layer over layer.
    pub fn x_ray() -> Self {
        let mut view = Self::default();
        for side in [Layer::Top, Layer::Bottom] {
            view.set(ViewLayer::Mask(side), false);
            view.set(ViewLayer::Silkscreen(side), false);
        }
        view.set(ViewLayer::Substrate, false);
        view.set(ViewLayer::Bodies, false);
        view.set(ViewLayer::Markings, false);
        view
    }

    /// Only one copper layer (and nothing else) shows.
    #[cfg(test)]
    pub fn only(copper: CopperLayer, stack: &Stack) -> Self {
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
        self.draws(ViewLayer::of(geometry.kind, geometry.layer))
    }

    /// Whether the bodies of `layer` are drawn: its toggle shown, and for
    /// the markings also the bodies they sit on ([`Self::visible`] is the
    /// toggle alone, as the popover shows it).
    pub fn draws(&self, layer: ViewLayer) -> bool {
        match layer {
            ViewLayer::Markings => self.shows_markings(),
            layer => self.visible(layer),
        }
    }

    /// Whether the marking decals are drawn: their own toggle and the
    /// bodies they sit on both shown. Display only, so turning it changes
    /// what is drawn and never rebuilds a model or a decal.
    pub fn shows_markings(&self) -> bool {
        self.visible(ViewLayer::Markings) && self.visible(ViewLayer::Bodies)
    }
}

/// The navigation bar's layer button and its popover of toggles for a
/// board of `stack`. Returns whether `view` changed. The button is
/// outlined while a layer is hidden.
pub fn button(ui: &mut Ui, view: &mut LayerView, stack: &Stack) -> bool {
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
                changed = popover(ui, view, stack);
            });
        });
    changed
}

/// Most copper rows a layer list shows before it scrolls.
const COPPER_ROWS: u8 = 10;

/// `len` copper layer rows, top to bottom; past [`COPPER_ROWS`] they
/// scroll within that height, so a many-layer board's list stays on
/// screen. Every row stays reachable.
pub fn copper_rows(ui: &mut Ui, len: usize, mut row: impl FnMut(&mut Ui, usize)) {
    if len <= usize::from(COPPER_ROWS) {
        for index in 0..len {
            row(ui, index);
        }
        return;
    }
    let t = Tokens::get();
    let height = t.control_height + t.space_4;
    egui::ScrollArea::vertical()
        .id_salt("copper-rows")
        .max_height(height * f32::from(COPPER_ROWS))
        .show(ui, |ui| {
            for index in 0..len {
                row(ui, index);
            }
        });
}

/// The popover's content: a caption, one check per layer and Show all.
fn popover(ui: &mut Ui, view: &mut LayerView, stack: &Stack) -> bool {
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
    let mut toggle = |ui: &mut Ui, layer: ViewLayer| {
        let mut shown = view.visible(layer);
        if crate::kit::check(ui, &layer.label(), &mut shown) {
            view.set(layer, shown);
            changed = true;
        }
    };
    // The films above, the copper and laminate (scrolled on a many-layer
    // board), the films below and the vias.
    let stack = ViewLayer::stack(stack);
    let first = stack
        .iter()
        .position(|l| matches!(l, ViewLayer::Copper(_)))
        .unwrap_or(stack.len());
    let last = stack
        .iter()
        .rposition(|l| matches!(l, ViewLayer::Copper(_)))
        .map_or(first, |i| i + 1);
    let (above, rest) = stack.split_at(first.min(stack.len()));
    let (board, below) = rest.split_at(last.saturating_sub(first).min(rest.len()));
    for layer in above {
        toggle(ui, *layer);
    }
    copper_rows(ui, board.len(), |ui, index| {
        if let Some(layer) = board.get(index) {
            toggle(ui, *layer);
        }
    });
    for layer in below {
        toggle(ui, *layer);
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

    fn four() -> Result<Stack, String> {
        pcbcraft_core::Stackup::preset(4)
            .map(|s| Stack::of(&s))
            .map_err(|e| e.to_string())
    }

    #[test]
    fn the_popover_lists_every_layer_once_in_board_order() -> Result<(), String> {
        let four = four()?;
        let copper = [
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
                "Markings",
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
            for layer in copper.iter().copied().map(Some).chain([None]) {
                let toggle = ViewLayer::of(kind, layer);
                assert!(stack.contains(&toggle), "{kind:?} on {layer:?}");
            }
        }
        let two = ViewLayer::stack(&Stack::default());
        assert_eq!(two.len(), stack.len() - 2);
        // Only the films the stackup has: no bottom mask or top legend
        // toggle on a board without them.
        let bare = ViewLayer::stack(&Stack::of(&pcbcraft_core::Stackup {
            mask_bottom: false,
            silkscreen_top: false,
            ..pcbcraft_core::Stackup::default()
        }));
        assert_eq!(bare.len(), two.len() - 2);
        assert!(!bare.contains(&ViewLayer::Mask(Layer::Bottom)));
        assert!(!bare.contains(&ViewLayer::Silkscreen(Layer::Top)));
        Ok(())
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

    /// The popover's checkboxes start at its left, under the "Layers"
    /// caption, not after an empty label column.
    #[test]
    fn the_popover_checkboxes_start_under_the_caption() -> Result<(), String> {
        let ctx = egui::Context::default();
        let mut view = LayerView::default();
        let stack = Stack::default();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            popover(ui, &mut view, &stack);
        });
        output.textures_delta.clear();
        let left = |label: &str| {
            output
                .shapes
                .iter()
                .find_map(|clipped| match &clipped.shape {
                    egui::Shape::Text(text) if text.galley.text() == label => Some(text.pos.x),
                    _ => None,
                })
        };
        let caption = left("Layers").ok_or("no caption")?;
        let row = left("Top solder mask").ok_or("no row")?;
        let t = Tokens::get();
        assert!(
            row - caption < t.form_label_width / 2.0,
            "row text at {row}, caption at {caption}"
        );
        Ok(())
    }

    /// Hover, press and release on `label`; whether the view changed.
    fn click(ctx: &egui::Context, view: &mut LayerView, label: &str) -> Result<bool, String> {
        let stack = Stack::default();
        let frame = |view: &mut LayerView, events: Vec<egui::Event>| {
            let mut changed = false;
            let input = egui::RawInput {
                events,
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| changed = popover(ui, view, &stack));
            // Tests have no renderer to apply texture deltas.
            output.textures_delta.clear();
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
    fn plated_barrels_follow_vias_and_x_ray_shows_only_copper() -> Result<(), String> {
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
        let four = four()?;
        let x_ray = LayerView::x_ray();
        for layer in ViewLayer::stack(&four) {
            let copper = matches!(layer, ViewLayer::Copper(_) | ViewLayer::Vias);
            assert_eq!(x_ray.visible(layer), copper, "{layer:?}");
        }
        assert!(!x_ray.shows_markings());
        let inner = LayerView::only(CopperLayer::Inner(2), &four);
        for layer in ViewLayer::stack(&four) {
            assert_eq!(
                inner.visible(layer),
                layer == ViewLayer::Copper(CopperLayer::Inner(2)),
                "{layer:?}"
            );
        }
        Ok(())
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

    /// The Markings toggle hides the decals and nothing else; hiding the
    /// components hides their markings too; only the display-only marking
    /// kind belongs to the toggle, so no saved body is a marking.
    #[test]
    fn the_markings_toggle_hides_only_the_decals() -> Result<(), String> {
        let ctx = egui::Context::default();
        let mut view = LayerView::default();
        assert!(view.shows_markings());
        assert!(click(&ctx, &mut view, "Markings")?);
        assert!(!view.visible(ViewLayer::Markings));
        assert!(!view.shows_markings());
        for kind in BodyKind::ALL {
            for layer in [None, Some(CopperLayer::Top), Some(CopperLayer::Bottom)] {
                let toggle = ViewLayer::of(kind, layer);
                assert_eq!(
                    toggle == ViewLayer::Markings,
                    kind == BodyKind::Marking,
                    "{kind:?}"
                );
                assert_eq!(
                    view.visible(toggle),
                    kind != BodyKind::Marking,
                    "{kind:?} on {layer:?}"
                );
            }
        }
        assert!(BodyKind::Marking.is_appearance(), "never saved");
        assert!(click(&ctx, &mut view, "Markings")?);
        assert!(view.shows_markings());
        assert!(click(&ctx, &mut view, "Components")?);
        assert!(view.visible(ViewLayer::Markings));
        assert!(!view.shows_markings(), "no decals without their bodies");
        assert!(!view.draws(ViewLayer::Markings));
        Ok(())
    }
}
