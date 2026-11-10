//! The Dimension tool, one state machine for both views: pick A, pick B
//! (each view finds what is under the pointer its own way and hands a
//! [`Picked`]), the value opens at the draft's label holding the distance
//! measured now, and Enter commits one `layout.dimension_add`, which is
//! selected once the engine has it. Esc drops the picks, and Esc again (or
//! Done) ends the tool. Clicks on drawn dimensions, Delete and the value
//! fields are shared too.
use super::state::{FIELD_W, Picked, capital, field_id, round_um};
use crate::kit;
use crate::pcb_editor::{PcbEditor, Tool, edge_card};
use crate::refusal::{self, Origin};
use egui::{Pos2, Rect, Ui, vec2};
use pcbcraft_core::layout::dimensions::DimensionCommand;
use pcbcraft_core::layout::dimensions::view::{self, Shown};
use pcbcraft_core::library::footprint_source::DimKind;
use pcbcraft_core::model::dimension::{BoardRef, PartDimensionSpec, PartRelation};
use pcbcraft_core::{Engine, Id, Operation, Project};
use solidcraft_theme::Tokens;

impl PcbEditor {
    /// Arm or end the Dimension tool (Layout and 3D alike).
    pub(in crate::pcb_editor) fn arm_dimension(&mut self, armed: bool) {
        self.dims.clear();
        self.tool = if armed { Tool::Dimension } else { Tool::Select };
    }

    /// The draft's measurement once two picks are made.
    pub(super) fn draft_shown(&self) -> Option<Shown> {
        match self.dims.picks.as_slice() {
            [a, b] => view::shown(&a.geometry, &b.geometry),
            _ => None,
        }
    }

    /// Start a new pick: a click away from an open value drops the picks.
    pub(super) fn begin_pick(&mut self) {
        if self.dims.picks.len() >= 2 {
            self.dims.clear();
        }
    }

    /// Take what a view found under a click: nothing (`None`), a refusal
    /// in words, or a reference. The second pick opens the value.
    pub(super) fn take_pick(&mut self, found: Option<Result<Picked, String>>) {
        self.begin_pick();
        let picked = match found {
            None => {
                // Say so: a click that picks nothing is not lost silently.
                self.dims.error = Some(
                    "Nothing to dimension there: pick a part's face, edge or pad, or a board edge"
                        .into(),
                );
                return;
            }
            Some(Err(why)) => {
                // The host's words follow a code; the card starts a sentence.
                self.dims.error = Some(capital(&why));
                return;
            }
            Some(Ok(p)) => p,
        };
        if self
            .dims
            .picks
            .first()
            .is_some_and(|p| p.reference == picked.reference)
        {
            self.dims.error = Some("Pick another face, edge, hole or board edge".into());
            return;
        }
        self.dims.error = None;
        self.dims.picks.push(picked);
        if self.dims.picks.len() < 2 {
            return;
        }
        let any_part = self
            .dims
            .picks
            .iter()
            .any(|p| matches!(p.reference, BoardRef::Part(_)));
        if !any_part {
            self.dims.picks.pop();
            self.dims.error =
                Some("A dimension holds a part: pick a part's face, edge, hole or pad".into());
            return;
        }
        match self.draft_shown() {
            Some(shown) => {
                self.dims.value = Some(format!("{} mm", kit::number_text(round_um(shown.value))));
                self.dims.fresh = 2;
            }
            None => {
                self.dims.picks.pop();
                self.dims.error = Some("These two cannot be measured; pick another".into());
            }
        }
    }

    /// The dimension the typed value makes, or why it makes none.
    fn draft_spec(&self, project: &Project) -> Result<PartDimensionSpec, String> {
        let text = self.dims.value.as_deref().unwrap_or_default();
        let value = edge_card::length_value(project, text)?;
        Ok(PartDimensionSpec {
            kind: PartRelation::Dim(DimKind::Distance),
            refs: self
                .dims
                .picks
                .iter()
                .map(|p| p.reference.clone())
                .collect(),
            value: Some(value),
            reference: false,
        })
    }

    /// The pointer is over a drawn label or the draft's value field.
    pub(super) fn over_dimension_label(&self, p: Pos2) -> bool {
        self.dims.hit_text(p).is_some()
            || self
                .dims
                .draft_at
                .is_some_and(|c| Rect::from_center_size(c, vec2(FIELD_W, 24.0)).contains(p))
    }

    /// A click at `at` on a drawn dimension selects it, and a double-click
    /// opens its value; with the tool, a click on a label opens it (its
    /// lines are left to the picks). `true` when the click was on one.
    pub(super) fn click_drawn_dimension(
        &mut self,
        project: &Project,
        at: Pos2,
        double: bool,
    ) -> bool {
        let armed = self.tool == Tool::Dimension;
        let on = if armed {
            self.dims.hit_text(at)
        } else {
            self.dims.hit(at)
        };
        let Some(id) = on else {
            return false;
        };
        self.select_dimension(project, id);
        if armed || double {
            self.open_in_place(project, id);
        }
        true
    }

    /// Esc with the tool on: close the chooser, else drop the picks, else
    /// end the tool.
    pub(super) fn cancel_dimension(&mut self) {
        if self.dims.layout.choosing.take().is_some() {
            return;
        }
        if self.dims.picks.is_empty() {
            self.arm_dimension(false);
        } else {
            self.dims.clear();
        }
    }

    /// Delete pressed: the selected dimension's `layout.dimension_delete`
    /// (never while its value or a new one's is typed: the field keeps the
    /// key).
    pub(super) fn delete_dimension(&mut self) -> Option<Operation> {
        let id = self.dimension?;
        self.dims.edit = None;
        Some(Operation::Dimension(DimensionCommand::Delete { id }))
    }

    /// The start of a view's frame: refusals of the values sent last frame.
    pub(super) fn dimension_frame_start(&mut self, ui: &Ui, engine: &Engine) {
        if let Some(text) = refusal::claim(ui.ctx(), Origin::Field(field_id())) {
            self.dims.error = Some(text);
            self.dims.sent = None;
            self.dims.fresh = 2;
        }
        self.answered(ui.ctx(), engine);
    }

    /// The fields over the view after it is drawn: the value edited in
    /// place, then the new dimension's value.
    pub(super) fn dimension_fields(&mut self, ui: &mut Ui, engine: &Engine) -> Option<Operation> {
        let edited = self.in_place_field(ui, engine);
        let added = self.value_field(ui, engine.project());
        edited.or(added)
    }

    /// The value field at the draft's text: Enter commits the dimension
    /// (one `layout.dimension_add`), Esc drops the picks.
    fn value_field(&mut self, ui: &mut Ui, project: &Project) -> Option<Operation> {
        let mut text = self.dims.value.clone()?;
        let centre = self.dims.draft_at?;
        let t = Tokens::get();
        let r = Rect::from_center_size(
            centre + vec2(0.0, t.control_height),
            vec2(FIELD_W, t.control_height),
        );
        let invalid = self.dims.error.is_some();
        let response = ui
            .scope_builder(egui::UiBuilder::new().max_rect(r), |ui| {
                kit::text_field(
                    ui,
                    "pcb-3d-dimension-value",
                    &mut text,
                    "distance, like 5 mm or width / 2",
                    invalid,
                )
            })
            .inner;
        if text != self.dims.value.clone().unwrap_or_default() {
            self.dims.error = None;
        }
        self.dims.value = Some(text);
        let lost = response.lost_focus();
        if self.dims.fresh > 0 {
            self.dims.fresh -= 1;
            if !lost {
                response.request_focus();
            }
        }
        let (enter, escape) = super::edit::field_keys(ui);
        if escape {
            self.dims.clear();
            return None;
        }
        if !(lost && enter) {
            return None;
        }
        match self.draft_spec(project) {
            Ok(spec) => {
                self.dims.sent = Some(spec.clone());
                self.dims.clear();
                refusal::mark(ui.ctx(), Origin::Field(field_id()));
                Some(Operation::Dimension(DimensionCommand::Add {
                    dimension: spec,
                }))
            }
            Err(why) => {
                self.dims.error = Some(why);
                self.dims.fresh = 2;
                None
            }
        }
    }

    /// The Dimension tool's card: what is picked, the next step, and why
    /// the last pick or value was refused.
    pub(in crate::pcb_editor) fn dimension_card(&mut self, ui: &mut Ui) -> Option<Operation> {
        let hint = match self.dims.picks.len() {
            0 => "Pick a part's face, edge, hole or pad, or a board edge",
            1 => "Pick what to dimension it to",
            _ => "Type the distance and press Enter; Esc starts again",
        };
        let card = kit::ToolCard {
            icon: "dimension",
            name: "Dimension",
            key: "",
            hint,
            tip: "Holds a part at a distance from a board edge or another part's feature. \
                  An edge is taken within a few points of it. Sloped faces and \
                  faces parallel to the board have no single trace on the board: pick one \
                  of their edges instead (in 3D).",
            finish: None,
        };
        let picks: Vec<String> = self.dims.picks.iter().map(|p| p.words.clone()).collect();
        let error = self.dims.error.clone();
        let shown = self.draft_shown();
        let t = Tokens::get();
        let margin = egui::Margin::symmetric(t.space_8 as i8, 0);
        let response = egui::Frame::NONE
            .inner_margin(margin)
            .show(ui, |ui| {
                kit::tool_card(ui, &card, |ui| {
                    for (i, p) in picks.iter().enumerate() {
                        kit::value_row(ui, if i == 0 { "From" } else { "To" }, &capital(p));
                    }
                    if let Some(shown) = shown {
                        kit::value_row(
                            ui,
                            "Now",
                            &format!("{} mm", kit::number_text(round_um(shown.value))),
                        );
                    }
                    if let Some(why) = &error {
                        kit::refusal_wrapped(ui, why);
                    }
                })
            })
            .inner;
        if response.done {
            self.arm_dimension(false);
        }
        None
    }

    /// The Dimension tool's picks, each with what its card calls it.
    #[cfg(test)]
    pub fn dimension_picks(&self) -> Vec<(BoardRef, String)> {
        self.dims
            .picks
            .iter()
            .map(|p| (p.reference.clone(), p.words.clone()))
            .collect()
    }

    /// How many references the Dimension tool has picked, the value
    /// being typed and why the last pick was refused.
    #[cfg(test)]
    pub fn dimension_draft(&self) -> (usize, Option<String>, Option<String>) {
        (
            self.dims.picks.len(),
            self.dims.value.clone(),
            self.dims.error.clone(),
        )
    }

    /// Whether the Dimension tool is on.
    #[cfg(test)]
    pub fn dimension_tool_on(&self) -> bool {
        self.tool == Tool::Dimension
    }

    /// The dimension selected in 2D, 3D and the Properties card.
    #[cfg(test)]
    pub fn selected_dimension(&self) -> Option<Id> {
        self.dimension
    }

    /// The dimensions drawn last frame (in whichever view): id, label, the
    /// label's text box on screen, and where the extension lines start on
    /// the two references (board mm).
    #[cfg(test)]
    pub fn dimensions_drawn(&self) -> Vec<(Id, String, Rect, [[f64; 3]; 2])> {
        self.dims
            .drawn
            .iter()
            .map(|d| (d.id, d.label.clone(), d.text, d.ends))
            .collect()
    }

    /// The dimensions drawn lit last frame (in whichever view): selected,
    /// edited, or hovered in the view or in their Properties row.
    #[cfg(test)]
    pub fn dimensions_lit(&self) -> Vec<Id> {
        self.dims
            .drawn
            .iter()
            .filter(|d| d.hot)
            .map(|d| d.id)
            .collect()
    }

    /// The value open at a dimension's label: the dimension, the text and
    /// why it was refused.
    #[cfg(test)]
    pub fn dimension_in_place(&self) -> Option<(Id, String, Option<String>)> {
        self.dims
            .edit
            .as_ref()
            .map(|e| (e.id, e.text.clone(), e.error.clone()))
    }
}
