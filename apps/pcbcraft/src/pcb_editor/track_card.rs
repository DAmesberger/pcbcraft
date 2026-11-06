//! The Track card of the selected track: net, layer and width (edited
//! live through `layout.track_set`), length, ends and lock, with Delete
//! track in the footer (docs/sidebar-ia.md, "Properties card by subject").
use super::{Field, PcbEditor, focus_within};
use crate::kit;
use crate::refusal::{self, Origin};
use egui::Ui;
use pcbcraft_core::{Id, Operation, Point, Project, Track, layout::LayoutCommand};
use solidcraft_ui_egui::ui_kit::{self, Button};

/// Track widths the engine accepts (mm), as `layout.track_set` documents.
const WIDTH_RANGE: std::ops::RangeInclusive<f64> = 0.05..=5.0;

/// The board's copper layers, top to bottom, by their names.
fn copper_layer_options(project: &Project) -> Vec<(pcbcraft_core::CopperLayer, String)> {
    project
        .stackup
        .layers()
        .into_iter()
        .map(|layer| (layer, layer.name()))
        .collect()
}

/// Why a locked track's fields and Delete are disabled.
const LOCKED: &str = "Unlock the track to change it";

impl PcbEditor {
    /// TRACK: net (a link that highlights it), layer, width, length, ends
    /// and lock.
    pub(super) fn track_section(
        &mut self,
        ui: &mut Ui,
        project: &Project,
        track: &Track,
    ) -> Option<Operation> {
        let locked = track.locked;
        let escape = ui.input(|i| i.key_pressed(egui::Key::Escape));
        if self.sent.get(&Field::TrackWidth) != Some(&[self.track_width, 0.0]) {
            self.errors.0.remove(&Field::TrackWidth);
        }
        let mut action: Option<(LayoutCommand, Option<Field>)> = None;
        let mut class_action = None;
        let mut open_classes = false;
        let mut focused = false;
        kit::section(ui, "pcb-track", "Track", true, None, |ui| {
            kit::row(ui, "Net", |ui| {
                if ui_kit::button(ui, project.net_name(track.net), Button::Small)
                    .on_hover_text("Highlight this net")
                    .clicked()
                {
                    self.net = Some(track.net);
                    self.highlighted = Some(track.net);
                }
            });
            let class = super::net_classes::class_row(ui, project, track.net, true);
            class_action = class.operation;
            open_classes = class.edit;
            let mut layer = track.layer;
            let layer_row = ui
                .add_enabled_ui(!locked, |ui| {
                    kit::choice_row(ui, "Layer", &mut layer, &copper_layer_options(project))
                })
                .response
                .on_disabled_hover_text(LOCKED);
            if let Some(message) = self.errors.0.get(&Field::TrackLayer) {
                kit::field_error(ui, &layer_row, message, false);
            }
            if layer != track.layer {
                action = Some((
                    LayoutCommand::SetTrack {
                        id: track.id,
                        width: None,
                        layer: Some(layer),
                    },
                    Some(Field::TrackLayer),
                ));
            }
            let width = ui
                .add_enabled_ui(!locked, |ui| {
                    kit::number_row(ui, "Width", &mut self.track_width, "mm", WIDTH_RANGE);
                })
                .response
                .on_disabled_hover_text(LOCKED);
            if let Some(message) = self.errors.0.get(&Field::TrackWidth) {
                kit::field_error(ui, &width, message, true);
            }
            focused = focus_within(ui, width.rect);
            kit::value_row(
                ui,
                "Length",
                &format!("{} mm", kit::number_text(round(length(track)))),
            );
            kit::value_row(ui, "Ends", &self.track_ends(project, track));
            let at = |p: Point| {
                format!(
                    "{}, {} mm",
                    kit::number_text(round(p[0])),
                    kit::number_text(round(p[1]))
                )
            };
            kit::value_row(ui, "Start", &at(track.start));
            kit::value_row(ui, "End", &at(track.end));
            let mut lock = locked;
            if kit::check_row(ui, "Locked", &mut lock) {
                action = Some((
                    LayoutCommand::SetTrackLock {
                        id: track.id,
                        locked: lock,
                    },
                    None,
                ));
            }
        });
        if escape && !focused && self.editing == Some(Field::TrackWidth) {
            self.track_width = track.width;
        }
        let staged = [self.track_width, 0.0];
        if action.is_none()
            && !locked
            && !focused
            && self.track_width != track.width
            && self.sent.get(&Field::TrackWidth) != Some(&staged)
        {
            self.sent.insert(Field::TrackWidth, staged);
            self.errors.0.remove(&Field::TrackWidth);
            action = Some((
                LayoutCommand::SetTrack {
                    id: track.id,
                    width: Some(self.track_width),
                    layer: None,
                },
                Some(Field::TrackWidth),
            ));
        }
        self.editing = focused.then_some(Field::TrackWidth);
        if open_classes {
            self.open_net_classes(pcbcraft_core::layout::net_class(project, track.net));
        }
        if class_action.is_some() {
            return class_action;
        }
        let (command, field) = action?;
        let origin = field.map_or(Origin::Command, |f| Origin::Field(f.id()));
        refusal::mark(ui.ctx(), origin);
        Some(Operation::Layout(command))
    }

    /// What each end of `track` meets: a pad of its net ("R1 pad 2"), or
    /// nothing ("open").
    pub(super) fn track_ends(&self, project: &Project, track: &Track) -> String {
        let end = |at: Point| {
            self.board_pads
                .iter()
                .find(|b| b.placed.net == Some(track.net) && b.contains(at))
                .map_or_else(
                    || "open".to_owned(),
                    |b| pad_name(project, b.placed.instance, &b.placed.pad.number),
                )
        };
        format!("{} → {}", end(track.start), end(track.end))
    }
}

/// "R1 pad 2", or "R1" for an unnumbered pad.
fn pad_name(project: &Project, instance: Id, number: &str) -> String {
    let designator = project.instance(instance).map_or("", |i| &i.designator);
    if number.is_empty() {
        designator.to_owned()
    } else {
        format!("{designator} pad {number}")
    }
}

fn length(track: &Track) -> f64 {
    pcbcraft_core::geometry::distance(track.start, track.end)
}

/// To a hundredth of a millimetre, as shown.
fn round(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

/// The track footer: Delete track, disabled while the track is locked.
pub(super) fn track_footer(ui: &mut Ui, track: &Track) -> Option<Operation> {
    let enabled = !track.locked;
    let clicked = ui_kit::button(ui, "Delete track", Button::Enabled(enabled))
        .on_disabled_hover_text(LOCKED)
        .clicked();
    (clicked && enabled).then_some(Operation::Layout(LayoutCommand::DeleteTrack {
        id: track.id,
    }))
}
