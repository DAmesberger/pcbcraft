//! The Route tool card: the width, copper layer and net the next route
//! uses, while Route is armed in Layout (docs/sidebar-ia.md, "Tool card").
//! The route in progress shows as a canvas pill; view toggles live in the
//! view popover.
use super::{PcbEditor, copper_options};
use crate::kit::{self, ToolCard};
use egui::Ui;
use pcbcraft_core::{Operation, Project, schematic};

/// Route widths the engine accepts (mm).
const WIDTH_RANGE: std::ops::RangeInclusive<f64> = 0.05..=5.0;

impl PcbEditor {
    /// The Route tool card. Done (or Esc on the canvas) returns to Select
    /// and the Properties card of the previous selection. Tool settings
    /// are editor state, not document edits, so it issues no operation.
    pub(super) fn route_card(&mut self, ui: &mut Ui, project: &Project) -> Option<Operation> {
        self.follow_class_width(project);
        let routing = self.route_start.is_some();
        let card = ToolCard {
            icon: "trace",
            name: "Route",
            key: "",
            hint: if routing {
                "Click a point or pad; V changes layer through a via; Esc ends"
            } else {
                "Click a pad, then points or a pad; V changes layer; Esc ends"
            },
            tip: "Routes run on the chosen copper layer at the chosen width, which starts at \
                  the routed net's class width (else the design rules' default). While routing, \
                  V (Shift+V backwards) places a via at the pointer and continues on the next \
                  layer of the stackup; the next click routes to the via, places it and routes \
                  on, as one step. A route the clearance rules refuse is not added, and the \
                  route stays in progress.",
            finish: None,
        };
        let t = solidcraft_ui_egui::theme::Tokens::get();
        // The same side inset as the Properties card's body.
        let margin = egui::Margin::symmetric(t.space_8 as i8, 0);
        let response = egui::Frame::NONE
            .inner_margin(margin)
            .show(ui, |ui| {
                kit::tool_card(ui, &card, |ui| {
                    kit::number_row(ui, "Width", &mut self.width, "mm", WIDTH_RANGE);
                    let options = copper_options(&self.stack);
                    if kit::choice_row(ui, "Layer", &mut self.layer, &options) {
                        self.route_start = None;
                        self.pending_via = None;
                    }
                    kit::check_row(ui, "Tent vias", &mut self.via_tented);
                    let mut nets = vec![(None, "From the first pad".to_owned())];
                    nets.extend(
                        schematic::nets(project)
                            .iter()
                            .map(|net| (Some(net.id), net.name.clone())),
                    );
                    if kit::choice_row(ui, "Net", &mut self.net, &nets) {
                        self.route_start = None;
                        self.route_highlight(self.net);
                    }
                    if let Some(net) = self.net.filter(|n| project.net(*n).is_some()) {
                        let class = pcbcraft_core::layout::net_class(project, net)
                            .and_then(|c| project.rules.class(c))
                            .map_or_else(|| "None".to_owned(), |c| c.name.clone());
                        kit::value_row(ui, "Class", &class);
                    }
                })
            })
            .inner;
        if response.done {
            self.end_route();
            self.discard_staged();
        }
        None
    }
}
