//! The Footprint tab's parameters panel (slice F4): one row per parameter
//! with its name and expression as editable cells, its typed value and the
//! dimensions reading it; package values (`pkg_*`) are shown read-only
//! above. Hovering a row highlights its dimensions on the canvas. Each
//! change is tried on the working copy first (a refusal keeps the text and
//! shows why) and then sent as one `library.edit_footprint`
//! (`SetParameter`, `RenameParameter`, `RemoveParameter`).
use std::collections::BTreeMap;

use super::PartEditor;
use super::footprint_params::{ParamRow, readers_phrase, unit_of, value_text};
use crate::kit;
use pcbcraft_core::library::footprint_source::{
    Expr, FootprintEdit, FootprintParam, ParamName, ParamUnit, package_values,
};
use solidcraft_ui_egui::theme::Tokens;
use solidcraft_ui_egui::ui_kit::{self, Button};

/// The panel's text buffers, rebuilt from the rows whenever the source
/// changes; a refused edit keeps its text.
#[derive(Clone, Debug, Default)]
pub struct Panel {
    names: BTreeMap<ParamName, String>,
    exprs: BTreeMap<ParamName, String>,
    new_name: String,
    new_expr: String,
    new_unit: ParamUnit,
    /// The row (or `None`: the new row) a refusal belongs to, and why.
    error: Option<(Option<ParamName>, String)>,
    stale: bool,
}

impl Panel {
    /// Why the last panel edit was refused, if it was.
    #[cfg(test)]
    pub fn error_text(&self) -> Option<&str> {
        self.error.as_ref().map(|(_, why)| why.as_str())
    }

    /// The source changed: refill the buffers on the next frame.
    pub fn reload(&mut self) {
        self.stale = true;
        self.error = None;
    }

    fn fill(&mut self, rows: &[ParamRow]) {
        if !self.stale
            && rows
                .iter()
                .all(|r| r.read_only || self.exprs.contains_key(&r.name))
        {
            return;
        }
        self.stale = false;
        self.names.clear();
        self.exprs.clear();
        for row in rows.iter().filter(|r| !r.read_only) {
            self.names.insert(row.name.clone(), row.name.0.clone());
            self.exprs.insert(row.name.clone(), row.expr.0.clone());
        }
    }
}

/// One edit of the panel.
enum Change {
    Set(ParamName, String, ParamUnit),
    Rename(ParamName, String),
    Remove(ParamName),
    Add,
}

/// The parameter name entered in a field, checked as the core does.
fn name_of(text: &str) -> Result<ParamName, String> {
    let name = ParamName(text.trim().to_string());
    if !name.valid() {
        return Err(format!("`{}` is not a parameter name", name.0));
    }
    if name.is_package() {
        return Err("Names starting pkg_ are the package's values".into());
    }
    Ok(name)
}

fn expr_of(text: &str) -> Result<Expr, String> {
    let expr = Expr(text.trim().to_string());
    if expr.valid() {
        Ok(expr)
    } else {
        Err("The expression is empty or too long".into())
    }
}

/// "used by" as the panel says it.
fn readers_text(row: &ParamRow) -> String {
    readers_phrase(&row.readers).map_or_else(|| "not used".into(), |p| format!("used by {p}"))
}

/// Why a parameter cannot be removed, as a sentence; `None` when nothing
/// reads it.
fn in_use_text(row: &ParamRow) -> Option<String> {
    readers_phrase(&row.readers).map(|p| {
        format!(
            "`{}` cannot be removed while it is used by {p}; change those first.",
            row.name.0
        )
    })
}

impl PartEditor {
    /// The Parameters section of the Footprint inspector, for a sketched
    /// footprint.
    pub(super) fn footprint_parameters(&mut self, ui: &mut egui::Ui) {
        self.refresh_footprint_sketch();
        let Some(loaded) = &self.footprint_dims.loaded else {
            return;
        };
        let rows = loaded.rows.clone();
        let errors = loaded.errors.clone();
        self.footprint_dims.panel.fill(&rows);
        let own = rows.iter().filter(|r| !r.read_only).count();
        let mut change = None;
        let mut hovered = None;
        kit::section(ui, "fp-parameters", "Parameters", true, Some(own), |ui| {
            kit::form(ui, |ui| {
                let t = Tokens::get();
                for row in rows.iter().filter(|r| r.read_only) {
                    let text = row.value.map_or_else(|| row.expr.0.clone(), value_text);
                    let response = ui
                        .scope(|ui| kit::value_row(ui, &row.name.0, &text))
                        .response;
                    if response.hovered() {
                        hovered = Some(row.name.clone());
                    }
                }
                if rows.iter().any(|r| r.read_only) {
                    kit::note(
                        ui,
                        "Package values (pkg_…) come from the package and are read-only here.",
                    );
                    kit::rule(ui);
                }
                let panel = &mut self.footprint_dims.panel;
                for row in rows.iter().filter(|r| !r.read_only) {
                    let response = ui.scope(|ui| {
                        ui.horizontal(|ui| {
                            let width = ui.available_width();
                            let name_w = (width * 0.34).max(48.0);
                            let remove_w = t.control_height;
                            let expr_w = (width - name_w - remove_w - t.space_4 * 2.0).max(48.0);
                            let failed = panel
                                .error
                                .as_ref()
                                .is_some_and(|(n, _)| n.as_ref() == Some(&row.name));
                            if let Some(name) = panel.names.get_mut(&row.name) {
                                ui.spacing_mut().text_edit_width = name_w;
                                let r = ui
                                    .scope(|ui| {
                                        ui.set_max_width(name_w);
                                        kit::text_field(
                                            ui,
                                            &format!("name-{}", row.name.0),
                                            name,
                                            "name",
                                            failed && name.trim() != row.name.0,
                                        )
                                    })
                                    .inner;
                                if r.lost_focus() && name.trim() != row.name.0 {
                                    change = Some(Change::Rename(row.name.clone(), name.clone()));
                                }
                            }
                            if let Some(expr) = panel.exprs.get_mut(&row.name) {
                                let r = ui
                                    .scope(|ui| {
                                        ui.set_max_width(expr_w);
                                        ui.spacing_mut().text_edit_width = expr_w;
                                        kit::text_field(
                                            ui,
                                            &format!("expr-{}", row.name.0),
                                            expr,
                                            "expression",
                                            (failed && expr.trim() != row.expr.0)
                                                || errors.contains_key(&row.name),
                                        )
                                    })
                                    .inner;
                                if r.lost_focus() && expr.trim() != row.expr.0 {
                                    change =
                                        Some(Change::Set(row.name.clone(), expr.clone(), row.unit));
                                }
                            }
                            if ui_kit::button(ui, "×", Button::Small)
                                .on_hover_text("Remove this parameter (refused while it is read)")
                                .clicked()
                            {
                                change = Some(Change::Remove(row.name.clone()));
                            }
                        });
                        // A refused edit shows only why: the stored value
                        // and its readers belong to the text it replaced.
                        let refused = panel
                            .error
                            .as_ref()
                            .and_then(|(n, why)| (n.as_ref() == Some(&row.name)).then_some(why));
                        if let Some(why) = refused {
                            kit::status(ui, why, kit::Tone::Error);
                        } else {
                            let value = match (&row.value, errors.get(&row.name)) {
                                (Some(v), _) => {
                                    format!("= {} · {}", value_text(*v), readers_text(row))
                                }
                                (None, Some(e)) => e.clone(),
                                (None, None) => readers_text(row),
                            };
                            if row.value.is_none() && errors.contains_key(&row.name) {
                                kit::status(ui, &value, kit::Tone::Error);
                            } else {
                                kit::note(ui, &value);
                            }
                        }
                    });
                    if response.response.hovered() {
                        hovered = Some(row.name.clone());
                    }
                }
                if own == 0 {
                    kit::note(
                        ui,
                        "Name a dimension's value (pitch = 1.27 mm) or add a parameter here; dimensions bound to it follow it.",
                    );
                }
                kit::rule(ui);
                kit::text_row(ui, "New name", &mut panel.new_name);
                kit::text_row(ui, "Expression", &mut panel.new_expr);
                let suggested = unit_of(&panel.new_expr);
                if suggested == ParamUnit::Deg && panel.new_unit == ParamUnit::Mm {
                    panel.new_unit = ParamUnit::Deg;
                }
                kit::row(ui, "Unit", |ui| {
                    kit::dropdown(
                        ui,
                        "fp-param-unit",
                        &mut panel.new_unit,
                        &[
                            (ParamUnit::Mm, "Length (mm)".into()),
                            (ParamUnit::Deg, "Angle (°)".into()),
                            (ParamUnit::Count, "Count".into()),
                        ],
                    );
                });
                if let Some((None, why)) = &panel.error {
                    kit::status(ui, why, kit::Tone::Error);
                }
                kit::actions(ui, |ui| {
                    let ready =
                        !panel.new_name.trim().is_empty() && !panel.new_expr.trim().is_empty();
                    if kit::primary(ui, "Add parameter", ready).clicked() {
                        change = Some(Change::Add);
                    }
                });
            });
        });
        self.footprint_dims.highlight = hovered;
        if let Some(change) = change {
            self.apply_parameter_change(change);
        }
    }

    fn apply_parameter_change(&mut self, change: Change) {
        let panel = &self.footprint_dims.panel;
        let (row, edit) = match change {
            Change::Set(name, text, unit) => (
                Some(name.clone()),
                expr_of(&text).map(|expr| FootprintEdit::SetParameter { name, expr, unit }),
            ),
            Change::Rename(from, text) => (
                Some(from.clone()),
                name_of(&text).map(|to| FootprintEdit::RenameParameter { from, to }),
            ),
            Change::Remove(name) => {
                let in_use = self
                    .footprint_dims
                    .loaded
                    .as_ref()
                    .and_then(|l| l.rows.iter().find(|r| r.name == name))
                    .and_then(in_use_text);
                (
                    Some(name.clone()),
                    match in_use {
                        Some(why) => Err(why),
                        None => Ok(FootprintEdit::RemoveParameter { name }),
                    },
                )
            }
            Change::Add => (
                None,
                name_of(&panel.new_name).and_then(|name| {
                    Ok(FootprintEdit::SetParameter {
                        name,
                        expr: expr_of(&panel.new_expr)?,
                        unit: panel.new_unit,
                    })
                }),
            ),
        };
        let edits = match edit {
            Ok(edit) => vec![edit],
            Err(why) => {
                self.footprint_dims.panel.error = Some((row, why));
                return;
            }
        };
        if let (None, Some(FootprintEdit::SetParameter { name, .. })) = (&row, edits.first())
            && self
                .footprint_dims
                .loaded
                .as_ref()
                .is_some_and(|l| l.rows.iter().any(|r| r.name == *name))
        {
            self.footprint_dims.panel.error = Some((None, format!("`{}` already exists", name.0)));
            return;
        }
        match self.try_footprint_edits(&edits) {
            Ok(()) => {
                let panel = &mut self.footprint_dims.panel;
                panel.error = None;
                if row.is_none() {
                    panel.new_name.clear();
                    panel.new_expr.clear();
                    panel.new_unit = ParamUnit::Mm;
                }
                self.queue_footprint_edits(edits);
            }
            Err(why) => {
                // The evaluator reports the first failing parameter by
                // name; a new expression is refused for its own cause when
                // it has one ("pad_l: …", not a cycle through `gap`).
                let why = match edits.first() {
                    Some(FootprintEdit::SetParameter { name, expr, unit }) => self
                        .own_parameter_error(name, expr, *unit)
                        .map_or(why, |own| format!("`{}`: {own}", name.0)),
                    _ => why,
                };
                self.footprint_dims.panel.error = Some((row, why));
            }
        }
    }

    /// Why `name` would have no value with `expr`: its own syntax, names
    /// or units first, else what evaluating the list with it says about
    /// `name` (a cycle through it).
    fn own_parameter_error(
        &self,
        name: &ParamName,
        expr: &Expr,
        unit: ParamUnit,
    ) -> Option<String> {
        let set = self.draft.as_ref()?;
        let package = package_values(&set.package);
        let stored = set.footprint.source.as_deref()?;
        if let Some(why) = pcbcraft_solidcraft::footprint::view::expression_error(
            stored, &package, name, &expr.0, unit,
        ) {
            return Some(why);
        }
        let mut source = stored.clone();
        match source.parameters.iter_mut().find(|p| p.name == *name) {
            Some(param) => {
                param.expr = expr.clone();
                param.unit = unit;
            }
            None => source.parameters.push(FootprintParam {
                name: name.clone(),
                expr: expr.clone(),
                unit,
                comment: String::new(),
            }),
        }
        super::footprint_params::errors(&source, &package).remove(name)
    }
}

#[cfg(test)]
impl PartEditor {
    /// Commit `name`'s expression as the panel does on leaving its cell.
    pub(super) fn apply_parameter_change_for_test(&mut self, name: &str, expr: &str) {
        let unit = self
            .footprint_dims
            .loaded
            .as_ref()
            .and_then(|l| l.rows.iter().find(|r| r.name.0 == name))
            .map_or(ParamUnit::Mm, |r| r.unit);
        self.apply_parameter_change(Change::Set(ParamName(name.into()), expr.into(), unit));
    }

    /// The row's remove button.
    pub(super) fn remove_parameter_for_test(&mut self, name: &str) {
        self.apply_parameter_change(Change::Remove(ParamName(name.into())));
    }
}
