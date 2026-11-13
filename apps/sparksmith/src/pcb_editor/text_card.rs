//! The Designator section of the Placement card: where this part prints its
//! silkscreen designator. Offset (x and y together) commits on Enter or focus
//! loss, Turn on choice and Reset on click, each as one
//! `instance.set_text_placement` and one undo step. The values belong to the
//! part: they are in the footprint's axes, so the text turns with it.
use super::{Field, PcbEditor, focus_within};
use crate::kit;
use crate::refusal::{self, Origin};
use egui::Ui;
use sparksmith_core::instances::text::{MAX_TEXT_OFFSET_MM, target};
use sparksmith_core::library::graphics::Rotation;
use sparksmith_core::model::TextPlacement;
use sparksmith_core::{Id, InstanceCommand, Operation, Project};

fn turn_options() -> [(Rotation, String); 4] {
    [
        (Rotation::R0, "0°"),
        (Rotation::R90, "90°"),
        (Rotation::R180, "180°"),
        (Rotation::R270, "270°"),
    ]
    .map(|(turn, name)| (turn, name.to_owned()))
}

impl PcbEditor {
    /// DESIGNATOR, for a part whose footprint prints one on the silkscreen.
    pub(super) fn text_section(
        &mut self,
        ui: &mut Ui,
        project: &Project,
        instance: Id,
    ) -> Option<Operation> {
        let graphic = target(project, instance, None).ok()?;
        let stored = project
            .instance(instance)?
            .text_placements
            .get(&graphic)
            .copied()
            .unwrap_or_default();
        if self.sent.get(&Field::TextOffset) != Some(&self.text_offset) {
            self.errors.0.remove(&Field::TextOffset);
        }
        let mut placement = None;
        let mut reset = false;
        let mut focused = false;
        kit::section(ui, "pcb-text", "Designator", true, None, |ui| {
            let offset = ui.scope(|ui| {
                kit::pair_row(
                    ui,
                    "Offset",
                    &mut self.text_offset,
                    "mm",
                    -MAX_TEXT_OFFSET_MM..=MAX_TEXT_OFFSET_MM,
                );
            });
            if let Some(message) = self.errors.0.get(&Field::TextOffset) {
                kit::field_error(ui, &offset.response, message, true);
            }
            focused = focus_within(ui, offset.response.rect);
            let mut turn = stored.turn;
            if kit::choice_row(ui, "Turn", &mut turn, &turn_options()) && turn != stored.turn {
                placement = Some(TextPlacement { turn, ..stored });
            }
            if stored != TextPlacement::default() {
                reset = kit::summary_row(ui, "Moved on this part", Some("Reset"));
            }
        });
        if self.card_escape && focused {
            self.text_offset = stored.offset;
        }
        if !focused
            && placement.is_none()
            && self.text_offset != stored.offset
            && self.sent.get(&Field::TextOffset) != Some(&self.text_offset)
        {
            self.sent.insert(Field::TextOffset, self.text_offset);
            self.errors.0.remove(&Field::TextOffset);
            placement = Some(TextPlacement {
                offset: self.text_offset,
                ..stored
            });
        }
        if !reset && placement.is_none() {
            return None;
        }
        refusal::mark(ui.ctx(), Origin::Field(Field::TextOffset.id()));
        Some(Operation::Instance(InstanceCommand::SetTextPlacement {
            instance,
            graphic: Some(graphic),
            placement: if reset { None } else { placement },
        }))
    }
}
