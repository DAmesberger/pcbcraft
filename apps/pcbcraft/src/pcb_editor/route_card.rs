//! The Routing section: the Route tool's width, layer and net, and the
//! net highlight and airwire toggles.
use super::{PcbEditor, layer_options};
use crate::kit;
use egui::Ui;
use pcbcraft_core::{Project, schematic};

impl PcbEditor {
    pub(super) fn routing_section(&mut self, ui: &mut Ui, project: &Project) {
        kit::section(ui, "pcb-routing", "Routing", true, None, |ui| {
            kit::number_row(ui, "Width", &mut self.width, "mm", 0.05..=10.0);
            if kit::choice_row(ui, "Layer", &mut self.layer, &layer_options()) {
                self.route_start = None;
            }
            let mut nets = vec![(None, "Select a net or pad".to_owned())];
            nets.extend(
                schematic::nets(project)
                    .iter()
                    .map(|net| (Some(net.id), net.name.clone())),
            );
            if kit::choice_row(ui, "Net", &mut self.net, &nets) {
                self.route_start = None;
                self.highlighted = self.net;
            }
            let mut highlight = self.highlighted.is_some();
            if kit::check_row(ui, "Highlight complete net", &mut highlight) {
                self.highlight_net(if highlight { self.net } else { None });
            }
            kit::check_row(ui, "Show airwires", &mut self.show_airwires);
            kit::note(
                ui,
                if self.route_start.is_some() {
                    "Click a point or pad. Escape ends the route."
                } else {
                    "Route: click a pad, then a point or pad."
                },
            );
        });
    }
}
