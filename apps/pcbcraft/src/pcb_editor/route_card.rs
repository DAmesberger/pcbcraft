//! The Route tool card: the mode, width, copper layer, via size and net
//! the route uses, and the Route keys, while Route is armed in Layout
//! (docs/sidebar-ia.md, "Tool card"). Every setting takes effect from the
//! next step, also during a route. The route in progress shows as a canvas
//! pill; view toggles live in the view popover.
use super::{PcbEditor, copper_options};
use crate::kit::{self, ToolCard};
use egui::Ui;
use pcbcraft_core::layout::{posture::Posture, router::RouteMode};
use pcbcraft_core::{Operation, Project, layout, schematic};

/// Route widths the engine accepts (mm).
const WIDTH_RANGE: std::ops::RangeInclusive<f64> = 0.05..=5.0;
/// Via drills and pads the engine accepts (mm).
const VIA_RANGE: std::ops::RangeInclusive<f64> = 0.1..=6.0;

impl PcbEditor {
    /// The Route tool card. Done (or Esc on the canvas without a route)
    /// returns to Select and the Properties card of the previous
    /// selection. Tool settings are editor state, not document edits, so
    /// it issues no operation.
    pub(super) fn route_card(&mut self, ui: &mut Ui, project: &Project) -> Option<Operation> {
        self.follow_class_width(project);
        let routing = self.route_start.is_some();
        let card = ToolCard {
            icon: "trace",
            name: "Route",
            key: "",
            hint: if routing {
                "Click corners; Enter or a pad of the net finishes"
            } else {
                "Click a pad to start; the keys below work while routing"
            },
            tip: "Each click adds one step (an elbow in the chosen mode, at the chosen width) \
                  as one undo step, and the route continues from its end. A click on a pad, \
                  track or via of the routed net, Enter or a double-click finishes; Esc or a \
                  right-click ends the route, and Esc again returns to Select. Backspace \
                  removes the last step. V, Shift+V or a layer's number continues on that \
                  layer through a via at the pointer, which the next click places. Width \
                  and via size start at the net class's (else the design rules' defaults); \
                  W and D step through the rules' presets. M switches the router: Shove \
                  pushes other nets' tracks and vias aside (locked copper and pads stay) \
                  and walks round what cannot move; Walkaround goes round everything; \
                  Ignore routes straight. Where no way exists the head stops at the last \
                  legal point, marked in the warning colour. A step the rules refuse is \
                  not added and the route stays where it was.",
            finish: None,
        };
        let t = solidcraft_ui_egui::theme::Tokens::get();
        // The same side inset as the Properties card's body.
        let margin = egui::Margin::symmetric(t.space_8 as i8, 0);
        let response = egui::Frame::NONE
            .inner_margin(margin)
            .show(ui, |ui| {
                kit::tool_card(ui, &card, |ui| self.route_rows(ui, project))
            })
            .inner;
        if response.done {
            self.end_route();
            self.discard_staged();
        }
        None
    }

    fn route_rows(&mut self, ui: &mut Ui, project: &Project) {
        let modes: Vec<(Posture, String)> = Posture::ALL
            .iter()
            .map(|p| (*p, p.label().to_owned()))
            .collect();
        let routers: Vec<(RouteMode, String)> = RouteMode::ALL
            .iter()
            .map(|m| (*m, m.label().to_owned()))
            .collect();
        let key = |id: &str| crate::keys::shown(ui.ctx(), id);
        let (router_key, mode_key, flip_key, width_key, layer_key) = (
            key("route.router-mode-next"),
            key("route.posture"),
            key("route.flip"),
            key("route.width-next"),
            key(self.layer_key_id().unwrap_or("route.via-next")),
        );
        kit::keyed(ui, router_key, |ui| {
            kit::choice_row(ui, "Router", &mut self.route.mode, &routers)
        });
        if self.route.mode == RouteMode::Shove {
            let mut move_vias = !self.route.vias_stay;
            if kit::check_row(ui, "Shove vias", &mut move_vias) {
                self.route.vias_stay = !move_vias;
            }
        }
        if self.route.mode == RouteMode::Ignore {
            kit::note(
                ui,
                "Ignore goes straight to the click without shoving; a step too close to other copper is still refused",
            );
        }
        kit::keyed(ui, mode_key, |ui| {
            kit::choice_row(ui, "Mode", &mut self.route.posture, &modes)
        });
        kit::keyed(ui, flip_key, |ui| {
            kit::check_row(ui, "Flip elbow", &mut self.route.flipped)
        });
        kit::keyed(ui, width_key, |ui| {
            kit::number_row(ui, "Width", &mut self.width, "mm", WIDTH_RANGE)
        });
        let options = copper_options(&self.stack);
        let mut layer = self.layer;
        if kit::keyed(ui, layer_key, |ui| {
            kit::choice_row(ui, "Layer", &mut layer, &options)
        }) {
            // During a route this is a layer change through a via at the
            // head, as a layer key is.
            self.change_layer_to(project, layer, None);
        }
        self.via_rows(ui, project);
        kit::check_row(ui, "Tent vias", &mut self.via_tented);
        let mut nets = vec![(None, "From the first pad".to_owned())];
        nets.extend(
            schematic::nets(project)
                .iter()
                .map(|net| (Some(net.id), net.name.clone())),
        );
        if kit::choice_row(ui, "Net", &mut self.net, &nets) {
            self.reset_route();
            self.route.via_draft = None;
            self.route_highlight(self.net);
        }
        if let Some(net) = self.net.filter(|n| project.net(*n).is_some()) {
            let class_id = layout::net_class(project, net);
            let class = class_id
                .and_then(|c| project.rules.class(c))
                .map_or_else(|| "None".to_owned(), |c| c.name.clone());
            kit::value_row(ui, "Class", &class);
            let pair = self.pair_width_gap(project);
            if let Some((w, g)) = pair {
                kit::value_row(
                    ui,
                    "Pair",
                    &format!(
                        "{} · width {} · gap {}",
                        self.route
                            .pair
                            .as_ref()
                            .and_then(|p| project.rules.diff_pair(p.pair))
                            .map_or("", |p| p.name.as_str()),
                        super::impedance_view::mm(w),
                        super::impedance_view::mm(g)
                    ),
                );
            } else if let Some(pair) = project.rules.pair_of(net) {
                let state = if self.route.single {
                    crate::keys::labelled(ui.ctx(), "off", "route.pair")
                } else {
                    "on: a pad starts both".to_owned()
                };
                kit::value_row(ui, "Pair", &format!("{} · {state}", pair.name));
            }
            super::impedance_view::route_rows(ui, project, class_id, self.layer, self.width, pair);
        }
        let rows = crate::keys::rows(ui.ctx(), &super::route_tool::route_key_table());
        kit::section(ui, "route-keys", "Keys", true, None, |ui| {
            let max = if self.route.keys_all {
                rows.len()
            } else {
                kit::LIST_MAX_ROWS
            };
            let shown = kit::list_frame(ui, rows.len(), max, |ui, index| {
                if let Some((key, action)) = rows.get(index) {
                    kit::value_row(ui, key, action);
                }
            });
            if shown.show_all {
                self.route.keys_all = true;
            }
        });
    }

    /// Via drill and pad of the next layer change: the size chosen while
    /// routing the net, else its class's. A size below the rules' drill or
    /// annular ring is marked and not used.
    fn via_rows(&mut self, ui: &mut Ui, project: &Project) {
        let (mut drill, mut pad) = self
            .route
            .via_draft
            .unwrap_or_else(|| self.route_via_size(project));
        let size_key = crate::keys::shown(ui.ctx(), "route.via-size-next");
        let drill_input = kit::keyed(ui, size_key, |ui| {
            kit::row(ui, "Via drill", |ui| {
                kit::number_input(ui, "Via drill", &mut drill, "mm", VIA_RANGE)
            })
        });
        if let Some(message) = &drill_input.invalid {
            kit::field_error(ui, &drill_input.response, message, true);
        }
        let pad_input = kit::row(ui, "Via pad", |ui| {
            kit::number_input(ui, "Via pad", &mut pad, "mm", VIA_RANGE)
        });
        if let Some(message) = &pad_input.invalid {
            kit::field_error(ui, &pad_input.response, message, true);
        }
        if drill_input.changed || pad_input.changed {
            match (self.net, layout::via_size_fits(project, drill, pad)) {
                (Some(net), Ok(())) => {
                    self.route.via_size = Some((net, (drill, pad)));
                    self.route.via_draft = None;
                }
                _ => self.route.via_draft = Some((drill, pad)),
            }
        }
        if self.route.via_draft.is_some()
            && let Err(error) = layout::via_size_fits(project, drill, pad)
        {
            let field = if drill_input.changed {
                &drill_input.response
            } else {
                &pad_input.response
            };
            kit::field_error(ui, field, &crate::refusal::shown_error(&error), true);
        }
    }
}
