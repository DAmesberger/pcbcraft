//! Which board layers the 3D view shows. Display-only: changing it changes
//! the draw lists and picking, never the model geometry. PCB 2D's layer
//! toggles share the overlapping entries (copper, silkscreen, mask).
use pcbcraft_core::{CopperLayer, Layer};
use pcbcraft_solidcraft::material::BodyKind;
use pcbcraft_solidcraft::scene::Geometry;
use std::collections::BTreeSet;

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
    /// The toggle a body belongs to. Films and copper read their side or
    /// layer from `layer`; a mask cap or copper without one belongs to the
    /// top.
    pub fn of(kind: BodyKind, layer: Option<CopperLayer>) -> Self {
        let side = layer.and_then(CopperLayer::side).unwrap_or(Layer::Top);
        match kind {
            BodyKind::Substrate | BodyKind::EdgeDielectric => Self::Substrate,
            BodyKind::MaskTop => Self::Mask(Layer::Top),
            BodyKind::MaskBottom => Self::Mask(Layer::Bottom),
            BodyKind::MaskCap => Self::Mask(side),
            BodyKind::Silkscreen => Self::Silkscreen(side),
            BodyKind::CopperTrack
            | BodyKind::PadCopper
            | BodyKind::PadFinish
            | BodyKind::EdgeCopper => Self::Copper(layer.unwrap_or(CopperLayer::Top)),
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
                    !(kind.is_board() || kind == BodyKind::EdgeDielectric),
                    "{kind:?}"
                );
            }
        }
        assert_eq!(LayerView::substrate_hidden(false), LayerView::default());
    }
}
