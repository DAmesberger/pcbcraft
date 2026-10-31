//! The Net card of the selected wire: the net's name, the wire's ends
//! and Disconnect.
use super::SchematicEditor;
use crate::kit;
use pcbcraft_core::{Endpoint, Engine, Operation, schematic::SchematicCommand, updates::Change};
use solidcraft_ui_egui::ui_kit::{self, Button};

impl SchematicEditor {
    pub(super) fn net_section(
        &mut self,
        ui: &mut egui::Ui,
        engine: &Engine,
        wire: &pcbcraft_core::Wire,
    ) -> Option<Operation> {
        let project = engine.project();
        let mut output = None;
        kit::section(ui, "net", "Net", true, None, |ui| {
            if let Some(net) = project.net_of(wire.from) {
                if self.net_label.as_ref().is_none_or(|(id, _)| *id != net.id) {
                    self.net_label = Some((net.id, net.name.clone()));
                }
                let mut renamed = None;
                if let Some((id, name)) = &mut self.net_label {
                    let response = kit::text_row(ui, "Net name", name);
                    if response.lost_focus() && *name != net.name {
                        renamed = Some((*id, name.trim().to_string()));
                    }
                }
                if let Some((net, name)) = renamed {
                    output = Some(Operation::Schematic(SchematicCommand::RenameNet {
                        net,
                        name,
                    }));
                    self.net_label = None;
                }
            }
            let label = |e: Endpoint| {
                format!(
                    "{}.{}",
                    project
                        .instance(e.instance)
                        .map_or("?", |i| i.designator.as_str()),
                    e.terminal
                )
            };
            kit::value_row(
                ui,
                "Wire",
                &format!("{} → {}", label(wire.from), label(wire.to)),
            );
            if let Some(net) = project.net_of(wire.from) {
                kit::value_row(ui, "Members", &format!("{} terminals", net.terminals.len()));
            }
            kit::actions(ui, |ui| {
                if ui_kit::button(ui, "Disconnect wire", Button::Normal)
                    .on_hover_text("Preview the change before it applies.")
                    .clicked()
                {
                    output = self.stage_update(
                        engine,
                        Change::Schematic {
                            command: SchematicCommand::Disconnect { wire: wire.id },
                        },
                    );
                }
            });
        });
        output
    }
}
