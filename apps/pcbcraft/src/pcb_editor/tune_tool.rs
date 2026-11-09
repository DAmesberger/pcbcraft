//! The Tune tool: length tuning by meanders. Hovering a straight track
//! segment previews the meander that brings its net to the target (its
//! length group's, its pair partner's for the skew limit, or a length
//! typed in the card) with a live length / target readout; a click
//! commits it as one `layout.tune_length`. The preview and the commit
//! come from the same core planner (`layout::meander::plan`), which keeps
//! the meander clear of other nets' copper by the rules.
//!
//! The obstacle index is built once per document revision and a preview
//! is planned only when the hovered segment, the revision or a setting
//! changes; planning one segment takes well under a frame.
use super::{BoardInk, BoardView, PcbEditor, Tool, capsule};
use crate::commands::KeyDecl;
use crate::keys;
use crate::kit::{self, ToolCard};
use egui::{Shape, Stroke, Ui};
use pcbcraft_core::layout::meander::{self, MeanderStyle, Side, TuneAmount, TuneRequest};
use pcbcraft_core::layout::{self, LayoutCommand, length, obstacles::Obstacles};
use pcbcraft_core::{Id, Operation, Point, Project};

/// The Tune tool's keys while the canvas is hovered, as registry commands
/// read through `crate::keys`.
pub const TUNE_KEYS: &[KeyDecl] = &[
    KeyDecl::new("tune.click", "Click", "Tune the segment under the pointer"),
    KeyDecl::new("tune.style", "/", "Switch trombone and accordion"),
    KeyDecl::new("tune.amplitude-up", "A", "Raise the amplitude limit"),
    KeyDecl::new(
        "tune.amplitude-down",
        "Shift+A",
        "Lower the amplitude limit",
    ),
    KeyDecl::new("tune.spacing-wider", "S", "Widen the leg spacing"),
    KeyDecl::new("tune.spacing-narrower", "Shift+S", "Narrow the leg spacing"),
    KeyDecl::new("tune.flip", "F", "Flip the trombone's side"),
];

/// Amplitude limits the card accepts, mm.
const AMPLITUDE_RANGE: std::ops::RangeInclusive<f64> = 0.05..=50.0;

/// A planned preview: its key and the plan or why there is none.
type Planned = (PlanKey, Result<meander::TunePreview, String>);

#[derive(Clone, Debug, PartialEq)]
struct PlanKey {
    revision: u64,
    track: Id,
    request: TuneRequest,
}

/// The tool's settings and cached state.
pub(super) struct TuneState {
    pub(super) style: MeanderStyle,
    /// Highest bump, mm.
    pub(super) amplitude: f64,
    /// Leg spacing, mm; `None` follows the rules' default for the width.
    pub(super) spacing: Option<f64>,
    pub(super) side: Option<Side>,
    /// A target typed in the card; `None` uses the net's own target.
    pub(super) target: Option<f64>,
    /// The track under the pointer.
    hover: Option<Id>,
    obstacles: Option<(u64, Obstacles)>,
    planned: Option<Planned>,
    /// The hovered net's length and target, and the revision they are of.
    net_target: Option<(u64, NetTarget)>,
    pub(super) keys_all: bool,
}

/// A net, its routed length and its target with tolerance, mm.
type NetTarget = (Id, f64, Option<(f64, f64)>);

impl Default for TuneState {
    fn default() -> Self {
        Self {
            style: MeanderStyle::Trombone,
            amplitude: 1.0,
            spacing: None,
            side: None,
            target: None,
            hover: None,
            obstacles: None,
            planned: None,
            net_target: None,
            keys_all: false,
        }
    }
}

impl PcbEditor {
    /// The hovered track's net, its length and target (with tolerance).
    fn tune_net(&mut self, project: &Project, revision: u64) -> Option<NetTarget> {
        let track = project
            .tracks
            .iter()
            .find(|t| Some(t.id) == self.tune.hover)?;
        let net = track.net;
        match self.tune.net_target {
            Some((r, cached)) if r == revision && cached.0 == net => Some(cached),
            _ => {
                let graph = self.revision_graph(project, revision);
                let length = length::signal_length_with(project, &graph, net);
                let target = length::tune_target_with(project, &graph, net);
                let cached = (net, length, target);
                self.tune.net_target = Some((revision, cached));
                Some(cached)
            }
        }
    }

    /// The request a click on the hovered segment sends, if there is a
    /// target to tune to.
    fn tune_request(&mut self, project: &Project, revision: u64) -> Option<TuneRequest> {
        let track = self.tune.hover?;
        let (_, _, target) = self.tune_net(project, revision)?;
        let mm = self.tune.target.or(target.map(|(t, _)| t))?;
        Some(TuneRequest {
            track,
            amount: TuneAmount::Target { mm },
            style: self.tune.style,
            amplitude: Some(self.tune.amplitude),
            spacing: self.tune.spacing,
            side: self.tune.side,
        })
    }

    /// The planned meander for the hovered segment, planned again only
    /// when its key changes.
    fn tune_plan(&mut self, project: &Project, revision: u64) -> Option<&Planned> {
        let request = self.tune_request(project, revision)?;
        let key = PlanKey {
            revision,
            track: request.track,
            request: request.clone(),
        };
        if self.tune.planned.as_ref().map(|(k, _)| k) != Some(&key) {
            if self.tune.obstacles.as_ref().map(|(r, _)| *r) != Some(revision) {
                self.tune.obstacles = Some((revision, Obstacles::of(project)));
            }
            let graph = self.revision_graph(project, revision);
            let plan = match &self.tune.obstacles {
                Some((_, obstacles)) => meander::plan(project, obstacles, &graph, &request)
                    .map(|p| p.preview())
                    .map_err(|e| crate::refusal::shown_error(&e)),
                None => Err("No board".to_owned()),
            };
            self.tune.planned = Some((key, plan));
        }
        self.tune.planned.as_ref()
    }

    /// Hover: the visible track under `at`.
    pub(super) fn tune_hover(&mut self, project: &Project, at: Option<Point>, tolerance: f64) {
        let layers = self.seen();
        self.tune.hover = at.and_then(|at| {
            project
                .tracks
                .iter()
                .rev()
                .find(|t| layers.copper_layer(t.layer) && layout::track_contains(t, at, tolerance))
                .map(|t| t.id)
        });
    }

    /// A click: tune the hovered segment as one `layout.tune_length`.
    pub(super) fn tune_click(&mut self, project: &Project, revision: u64) -> Option<Operation> {
        let request = self.tune_request(project, revision)?;
        Some(Operation::Layout(LayoutCommand::TuneLength { request }))
    }

    /// The Tune keys while the canvas is hovered.
    pub(super) fn tune_keys(&mut self, ui: &Ui, project: &Project) {
        if self.tool != Tool::Tune || ui.ctx().egui_wants_keyboard_input() {
            return;
        }
        let pressed = |id| keys::pressed(ui.ctx(), id);
        if pressed("tune.style") {
            self.tune.style = match self.tune.style {
                MeanderStyle::Trombone => MeanderStyle::Accordion,
                MeanderStyle::Accordion => MeanderStyle::Trombone,
            };
        }
        for (id, factor) in [
            ("tune.amplitude-up", 1.25),
            ("tune.amplitude-down", 1.0 / 1.25),
        ] {
            if pressed(id) {
                self.tune.amplitude = (self.tune.amplitude * factor)
                    .clamp(*AMPLITUDE_RANGE.start(), *AMPLITUDE_RANGE.end());
            }
        }
        let wider = pressed("tune.spacing-wider");
        let narrower = pressed("tune.spacing-narrower");
        if wider || narrower {
            let width = project
                .tracks
                .iter()
                .find(|t| Some(t.id) == self.tune.hover)
                .map_or(self.width, |t| t.width);
            let min = meander::min_spacing(project, width);
            let current = self
                .tune
                .spacing
                .unwrap_or_else(|| meander::default_spacing(project, width));
            let step = if narrower { -0.05 } else { 0.05 };
            self.tune.spacing = Some((current + step).max(min));
        }
        if pressed("tune.flip") {
            self.tune.side = Some(self.tune.side.unwrap_or(Side::Left).other());
        }
    }

    /// The meander preview and the length readout at the pointer.
    pub(super) fn paint_tune_preview(
        &mut self,
        painter: &egui::Painter,
        view: BoardView,
        project: &Project,
        revision: u64,
        pointer: Option<egui::Pos2>,
    ) {
        let t = solidcraft_theme::Tokens::get();
        let Some(track) = project
            .tracks
            .iter()
            .find(|tr| Some(tr.id) == self.tune.hover)
            .cloned()
        else {
            return;
        };
        let (a, b) = (view.screen(track.start), view.screen(track.end));
        painter.line_segment([a, b], Stroke::new(2.0, t.body_selected));
        let Some((net, length_now, target)) = self.tune_net(project, revision) else {
            return;
        };
        let wanted = self.tune.target.or(target.map(|(t, _)| t));
        let planned = self.tune_plan(project, revision).map(|(_, p)| p.clone());
        let scale = view.view.scale;
        let mut readout = match wanted {
            Some(w) => format!(
                "{} {} / {} mm ({:+.3})",
                project.net_name(net),
                kit::number_text((length_now * 1000.0).round() / 1000.0),
                kit::number_text((w * 1000.0).round() / 1000.0),
                length_now - w
            ),
            None => format!(
                "{} {} mm · no target",
                project.net_name(net),
                kit::number_text((length_now * 1000.0).round() / 1000.0)
            ),
        };
        match planned {
            Some(Ok(preview)) => {
                let color = BoardInk::get()
                    .on_board(crate::palette::board().copper_layer(track.layer), 0.8);
                let width = (track.width as f32 * scale).max(1.0);
                for w in preview.points.windows(2) {
                    let (p, q) = (view.screen(w[0]), view.screen(w[1]));
                    painter.add(Shape::convex_polygon(
                        capsule(p, q, width / 2.0),
                        color,
                        Stroke::NONE,
                    ));
                    painter.line_segment([p, q], Stroke::new(1.0, t.body_selected));
                }
                readout.push_str(&format!(
                    " · +{} mm",
                    kit::number_text((preview.added * 1000.0).round() / 1000.0)
                ));
            }
            Some(Err(reason)) => readout = format!("{readout} · {reason}"),
            None => {}
        }
        if let Some(at) = pointer {
            painter.text(
                at + egui::vec2(12.0, -12.0),
                egui::Align2::LEFT_BOTTOM,
                readout,
                egui::FontId::proportional(t.font_label),
                t.text,
            );
        }
    }

    /// The Tune tool card: the hovered net's length and target, the
    /// meander settings and the keys.
    pub(super) fn tune_card(&mut self, ui: &mut Ui, project: &Project, revision: u64) {
        let card = ToolCard {
            icon: "dimension",
            name: "Tune",
            key: "T",
            hint: "Point at a segment; a click adds the meander shown",
            tip: "The meander brings the net to its target: its length group's target (or \
                  longest member), or its pair partner's length when the pair has a skew \
                  limit, or the target typed here. Lengths include via barrels. The \
                  fewest bumps within the amplitude are used, on the side with room; legs \
                  are at least the track clearance apart. One click is one undo step.",
            finish: None,
        };
        let t = solidcraft_theme::Tokens::get();
        let margin = egui::Margin::symmetric(t.space_8 as i8, 0);
        let response = egui::Frame::NONE
            .inner_margin(margin)
            .show(ui, |ui| {
                kit::tool_card(ui, &card, |ui| self.tune_rows(ui, project, revision))
            })
            .inner;
        if response.done {
            self.end_route();
        }
    }

    fn tune_rows(&mut self, ui: &mut Ui, project: &Project, revision: u64) {
        let styles: Vec<(MeanderStyle, String)> = MeanderStyle::ALL
            .iter()
            .map(|s| (*s, s.label().to_owned()))
            .collect();
        kit::choice_row(ui, "Style", &mut self.tune.style, &styles);
        kit::number_row(
            ui,
            "Amplitude",
            &mut self.tune.amplitude,
            "mm",
            AMPLITUDE_RANGE,
        );
        let sides = [
            (None, "Side with room".to_owned()),
            (Some(Side::Left), "Left".to_owned()),
            (Some(Side::Right), "Right".to_owned()),
        ];
        kit::choice_row(ui, "Side", &mut self.tune.side, &sides);
        if let Some((net, now, target)) = self.tune_net(project, revision) {
            kit::value_row(ui, "Net", project.net_name(net));
            kit::value_row(ui, "Length", &super::impedance_view::mm(now));
            let own = match target {
                Some((t, tol)) => format!(
                    "{} ± {}",
                    super::impedance_view::mm(t),
                    super::impedance_view::mm(tol)
                ),
                None => "None".to_owned(),
            };
            kit::value_row(ui, "Net target", &own);
        }
        let mut typed = self.tune.target.is_some();
        if kit::check_row(ui, "Own target", &mut typed) {
            self.tune.target = typed.then_some(self.tune.target.unwrap_or(0.0));
        }
        if let Some(target) = &mut self.tune.target {
            kit::number_row(ui, "Target", target, "mm", 0.0..=10_000.0);
        }
        if let Some((_, plan)) = self.tune_plan(project, revision).cloned() {
            match plan {
                Ok(p) => kit::value_row(
                    ui,
                    "Meander",
                    &format!(
                        "+{} · {} bumps · {} high",
                        super::impedance_view::mm(p.added),
                        p.bumps,
                        super::impedance_view::mm(p.amplitude)
                    ),
                ),
                // Why no meander fits is the preview's state, not a
                // document problem: it reads in the row, as the canvas
                // readout says it at the pointer.
                Err(reason) => kit::value_row(ui, "Meander", &format!("None: {reason}")),
            }
        }
        let rows = keys::rows(ui.ctx(), TUNE_KEYS);
        kit::section(ui, "tune-keys", "Keys", true, None, |ui| {
            let max = if self.tune.keys_all {
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
                self.tune.keys_all = true;
            }
        });
    }
}
