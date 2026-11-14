//! The Net card of a picked wire: the net's name (committed live), its net
//! class, its members, and the WIRE's two ends. Disconnect wire… is the card's footer.
use super::{SchematicEditor, end_label, endpoint_label, live};
use crate::kit;
use crate::refusal::{self, Origin};
use sparksmith_core::{
    Endpoint, Engine, Id, Operation,
    schematic::{self, SchematicCommand},
};

impl SchematicEditor {
    pub(super) fn net_section(
        &mut self,
        ui: &mut egui::Ui,
        engine: &Engine,
        wire: &sparksmith_core::Wire,
    ) -> Option<Operation> {
        let project = engine.project();
        let mut output = None;
        let mut select: Option<Id> = None;
        let net = schematic::wire_net(project, wire);
        kit::section(ui, "net", "Net", true, None, |ui| {
            let Some(net) = net else {
                kit::value_row(ui, "Name", "—");
                return;
            };
            if self.net_label.as_ref().is_none_or(|(id, _)| *id != net.id) {
                self.net_label = Some((net.id, net.name.clone()));
            }
            if let Some((id, name)) = &mut self.net_label {
                let response = kit::text_row(ui, "Name", name);
                match live(ui, &response, &mut self.refused, *name != net.name, None) {
                    super::Live::Commit => {
                        refusal::mark(ui.ctx(), Origin::Field(response.id));
                        output = Some(Operation::Schematic(SchematicCommand::RenameNet {
                            net: *id,
                            name: name.trim().to_string(),
                        }));
                    }
                    super::Live::Revert => *name = net.name.clone(),
                    super::Live::Idle => {}
                }
            }
            // The net's class: explicit, or by a name pattern; assigning
            // here is the same `net_class.assign` as in Layout.
            let class = crate::pcb_editor::net_classes::class_row(ui, project, net.id, false);
            if class.operation.is_some() {
                output = class.operation;
            }
            let members: Vec<Endpoint> = net.terminals.clone();
            kit::value_row(
                ui,
                "Members",
                &kit::counted(members.len(), "terminal", "terminals"),
            );
            let max = if self.all_members {
                members.len()
            } else {
                kit::LIST_MAX_ROWS
            };
            let shown = kit::list_frame(ui, members.len(), max, |ui, index| {
                let Some(endpoint) = members.get(index) else {
                    return;
                };
                let label = endpoint_label(project, *endpoint);
                let row = kit::Row {
                    icon: "component",
                    label: &label,
                    selected: self.selected == Some(endpoint.instance),
                    tip: "Select this instance",
                    ..Default::default()
                };
                if kit::tree_row(ui, ui.scope_id().with(("member", index)), &row).clicked {
                    select = Some(endpoint.instance);
                }
            });
            if shown.show_all {
                self.all_members = true;
            }
        });
        kit::section(ui, "wire", "Wire", true, None, |ui| {
            kit::value_row(ui, "From", &end_label(project, wire.from));
            kit::value_row(ui, "To", &end_label(project, wire.to));
            kit::value_row(ui, "Corners", &wire.corners.len().to_string());
            if let Some(op) = self.corner_radius_row(ui, wire) {
                output = Some(op);
            }
        });
        if let Some(instance) = select {
            self.select_instance(Some(instance), project);
        }
        output
    }

    /// The picked corner's radius (0: sharp), committed live as one
    /// `wire.set_radius`; a radius that does not fit is refused on the field.
    fn corner_radius_row(
        &mut self,
        ui: &mut egui::Ui,
        wire: &sparksmith_core::Wire,
    ) -> Option<Operation> {
        let index = self.picked_corner(wire)?;
        let current = wire.corners.get(index)?.radius_mm();
        let key = (wire.id, index);
        let mut value = self
            .corner_radius
            .filter(|(k, _)| *k == key)
            .map_or(current, |(_, v)| v);
        let input = kit::row(ui, "Radius", |ui| {
            kit::number_input(
                ui,
                "corner-radius",
                &mut value,
                "mm",
                0.0..=schematic::MAX_CORNER_RADIUS_MM,
            )
        });
        let changed = (value - current).abs() > 1e-9;
        let state = live(
            ui,
            &input.response,
            &mut self.refused,
            changed,
            input.invalid.as_deref(),
        );
        self.corner_radius = input.response.has_focus().then_some((key, value));
        match state {
            super::Live::Commit => {
                refusal::mark(ui.ctx(), Origin::Field(input.response.id));
                Some(Operation::Schematic(SchematicCommand::SetWireRadius {
                    wire: wire.id,
                    index,
                    radius: sparksmith_core::units::Nm::from_mm_f64_rounded(value).ok()?,
                }))
            }
            super::Live::Revert | super::Live::Idle => None,
        }
    }
}
