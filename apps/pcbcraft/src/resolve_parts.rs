//! The Resolve parts window: a table of the `library.resolve_groups` groups
//! of instances that share requirements and footprint, each resolved by one
//! manufacturer part with one `instance.assign_part` (one undo step), and
//! the `project.bom` lines. Why a group is unresolved comes from
//! `project.issues`, in core's words. Everything shown comes from those
//! three engine queries; the window only returns what the user chose.
//!
//! It is a `ui_kit::Window`, like Library sources (docs/sidebar-ia.md, D10),
//! so the sheet stays usable: Select on sheet keeps it open, and Choose…
//! opens the docked Choose part dialog beside it.
use crate::kit::severity::{Level, severity_glyph};
use crate::kit::{self, Tone};
use egui::{Sense, Ui, vec2};
use pcbcraft_core::completeness::{
    BomLine, BomReport, PurchasingState, ResolveGroup, ResolveReport,
};
use pcbcraft_core::diagnostics::ObjectRef;
use pcbcraft_core::fields::{self, FieldValue, Requirement};
use pcbcraft_core::issues::{IssueReport, IssueScope};
use pcbcraft_core::updates::{Change, PartAction, PartResolution, Resolutions};
use pcbcraft_core::{
    ComponentRef, Engine, FieldRef, Id, InstanceCommand, Operation, PartRef, Project, Query,
    QueryResult,
};
use solidcraft_theme::Tokens;
use solidcraft_ui_egui::ui_kit::{self, Button};

/// Designators a group title lists before summarising the rest.
const TITLE_DESIGNATORS: usize = 6;

/// The query text Choose… starts from without a group: every Part.
pub const ANY_PART: &str = "kind:part";

/// The window's widest and narrowest width, in card widths.
const MAX_CARDS: f32 = 2.6;
const MIN_CARDS: f32 = 1.5;

/// What the user chose in the window.
#[derive(Clone, Debug, PartialEq)]
pub enum ResolveAction {
    /// Open the Choose part dialog for these instances.
    Choose {
        query: String,
        instances: Vec<Id>,
    },
    /// Select this instance on the sheet; the window stays open.
    Select(Id),
    /// Author a new Part of `component` rated at the group's values.
    NewPart {
        component: ComponentRef,
        values: Vec<(FieldRef, FieldValue)>,
    },
    Close,
}

/// A query result at one document revision; an error keeps its message.
type Cached<T> = Option<(u64, Result<T, String>)>;

pub struct ResolveParts {
    open: bool,
    /// Show only unresolved groups (the default), or every BOM line too.
    unresolved_only: bool,
    /// The row whose details show under the table.
    expanded: Option<usize>,
    groups: Cached<ResolveReport>,
    bom: Cached<BomReport>,
    /// Core's issues per group, in group order.
    issues: Cached<Vec<GroupIssues>>,
}

impl Default for ResolveParts {
    fn default() -> Self {
        Self {
            open: false,
            unresolved_only: true,
            expanded: Some(0),
            groups: None,
            bom: None,
            issues: None,
        }
    }
}

/// One table row: an unresolved group, or a resolved BOM line.
#[derive(Clone, Debug, PartialEq)]
pub struct TableRow {
    pub designators: String,
    pub component: String,
    pub values: String,
    pub footprint: String,
    /// The current Part, or "None".
    pub part: String,
    /// The state of the Part cell: the worst issue, To do without a Part,
    /// Ok when resolved.
    pub level: Level,
    /// Core's issue titles for the row's instances, most severe first.
    pub issues: Vec<(Level, String)>,
    pub instances: Vec<Id>,
    pub query: String,
    /// The component and values New Part… starts from; `None` for a
    /// resolved line.
    pub new_part: Option<(ComponentRef, Vec<(FieldRef, FieldValue)>)>,
}

impl TableRow {
    pub fn resolved(&self) -> bool {
        self.level == Level::Ok
    }
}

/// `library.resolve_groups`.
fn groups(engine: &Engine) -> Result<ResolveReport, String> {
    match engine.query(Query::ResolveGroups {}) {
        Ok(QueryResult::ResolveGroups(report)) => Ok(report),
        Ok(_) => Err("Unexpected answer from the engine".into()),
        Err(error) => Err(crate::refusal::shown_error(&error)),
    }
}

/// The text Choose… pre-fills for one instance: its group's query when it
/// is in one, otherwise every Part.
pub fn query_for(engine: &Engine, instance: Id) -> String {
    groups(engine)
        .ok()
        .and_then(|report| {
            report
                .groups
                .into_iter()
                .find(|g| g.instances.contains(&instance))
                .map(|g| g.query)
        })
        .unwrap_or_else(|| ANY_PART.into())
}

/// One `instance.assign_part`: one Part for all `instances`, or clear it.
pub fn assign(instances: Vec<Id>, part: Option<PartRef>) -> Operation {
    Operation::Instance(InstanceCommand::AssignPart { instances, part })
}

/// Update `instances` to the newer component revision `target` that `part`
/// is for, assigning the Part in the same reviewed update.
pub fn update_and_assign(instances: Vec<Id>, part: PartRef, target: ComponentRef) -> Change {
    Change::ComponentRevision {
        resolutions: Resolutions {
            parts: instances
                .iter()
                .map(|instance| PartResolution {
                    instance: *instance,
                    action: PartAction::Assign { part },
                })
                .collect(),
            ..Resolutions::default()
        },
        instances,
        target,
    }
}

/// The values a new Part for a group starts rated at.
pub fn requirement_values(requirements: &[Requirement]) -> Vec<(FieldRef, FieldValue)> {
    requirements
        .iter()
        .map(|r| (r.field, r.value.clone()))
        .collect()
}

/// The designators of a group, the first few then a count.
pub fn designator_summary(designators: &[String]) -> String {
    let shown = designators
        .iter()
        .take(TITLE_DESIGNATORS)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    match designators.len().checked_sub(TITLE_DESIGNATORS) {
        Some(rest) if rest > 0 => format!("{shown} +{rest}"),
        _ => shown,
    }
}

/// Whether an issue code is about purchasing: the Part chosen, or none.
fn purchasing_code(code: &str) -> bool {
    code.starts_with("part.") || code.starts_with("purchasing.")
}

/// Core's issue titles of one group, as glyph level and title.
pub type GroupIssues = Vec<(Level, String)>;

/// Core's purchasing issues of each group of instances, from one
/// `project.issues` query over the schematic (not one per group): a row
/// belongs to every group whose instance it names, most severe first as
/// the report orders them.
pub fn issues_by_group(engine: &Engine, groups: &[&[Id]]) -> Result<Vec<GroupIssues>, String> {
    let report: IssueReport = match engine.query(Query::Issues {
        scope: IssueScope::Area {
            area: pcbcraft_core::issues::IssueArea::Schematic,
        },
    }) {
        Ok(QueryResult::Issues(report)) => report,
        Ok(_) => return Err("Unexpected answer from the engine".into()),
        Err(error) => return Err(crate::refusal::shown_error(&error)),
    };
    let rows: Vec<_> = report
        .rows
        .iter()
        .filter(|r| purchasing_code(&r.group.code))
        .collect();
    Ok(groups
        .iter()
        .map(|instances| {
            rows.iter()
                .filter(|r| {
                    // As the Selection scope does: an instance covers its
                    // pads, units, fields and terminals.
                    r.objects().any(|o| match o {
                        ObjectRef::Instance { id }
                        | ObjectRef::Pad { instance: id, .. }
                        | ObjectRef::Unit { instance: id, .. }
                        | ObjectRef::Property { instance: id, .. } => instances.contains(id),
                        ObjectRef::Terminal { endpoint } => instances.contains(&endpoint.instance),
                        _ => false,
                    })
                })
                .map(|r| (Level::from(r.severity), r.title.clone()))
                .collect()
        })
        .collect())
}

fn component_name(project: &Project, component: ComponentRef) -> String {
    project.library.component(component).map_or_else(
        || component.to_string(),
        |c| format!("{} · {}", c.meta.name, component.revision),
    )
}

fn footprint_name(project: &Project, footprint: Option<pcbcraft_core::FootprintRef>) -> String {
    footprint.map_or_else(
        || "None".to_owned(),
        |f| {
            project
                .library
                .footprint(f)
                .map_or_else(|| f.to_string(), |p| p.meta.name.clone())
        },
    )
}

fn part_name(project: &Project, part: Option<PartRef>) -> String {
    match part {
        Some(r) => project.library.part(r).map_or_else(
            || r.to_string(),
            |p| {
                format!(
                    "{} {} · {}",
                    p.definition.manufacturer, p.definition.mpn, r.revision
                )
            },
        ),
        None => "None".to_owned(),
    }
}

fn group_row(project: &Project, group: &ResolveGroup, issues: GroupIssues) -> TableRow {
    let library = &project.library;
    let values = group
        .requirements
        .iter()
        .map(|r| {
            let kind = library
                .field_view(r.field)
                .and_then(|v| v.kind_body().cloned());
            fields::display(&r.value, &r.value_type, kind.as_ref())
        })
        .collect::<Vec<_>>()
        .join(" · ");
    let part = group
        .instances
        .first()
        .and_then(|id| project.instance(*id))
        .and_then(|i| i.part);
    let level = issues
        .iter()
        .map(|(level, _)| *level)
        .min()
        .unwrap_or(Level::ToDo);
    TableRow {
        designators: designator_summary(&group.designators),
        component: component_name(project, group.component),
        values: if values.is_empty() {
            "None".into()
        } else {
            values
        },
        footprint: footprint_name(project, group.footprint),
        part: part_name(project, part),
        level,
        issues,
        instances: group.instances.clone(),
        query: group.query.clone(),
        new_part: Some((group.component, requirement_values(&group.requirements))),
    }
}

fn line_row(engine: &Engine, line: &BomLine) -> TableRow {
    let project = engine.project();
    let values = line
        .values
        .iter()
        .map(|v| v.text.clone())
        .collect::<Vec<_>>()
        .join(" · ");
    TableRow {
        designators: designator_summary(&line.designators),
        component: component_name(project, line.component),
        values: if values.is_empty() {
            "None".into()
        } else {
            values
        },
        footprint: footprint_name(project, line.footprint),
        part: part_name(project, line.part),
        level: Level::Ok,
        issues: Vec::new(),
        instances: line.instances.clone(),
        query: line
            .instances
            .first()
            .map_or_else(|| ANY_PART.into(), |id| query_for(engine, *id)),
        new_part: None,
    }
}

/// The table's rows: every unresolved group, then, unless
/// `unresolved_only`, each resolved BOM line. Nothing unresolved is ever
/// left out.
pub fn rows(
    engine: &Engine,
    groups: &ResolveReport,
    bom: Option<&BomReport>,
    issues: &[GroupIssues],
    unresolved_only: bool,
) -> Vec<TableRow> {
    let project = engine.project();
    let mut rows: Vec<TableRow> = groups
        .groups
        .iter()
        .enumerate()
        .map(|(i, g)| group_row(project, g, issues.get(i).cloned().unwrap_or_default()))
        .collect();
    if !unresolved_only && let Some(bom) = bom {
        rows.extend(
            bom.lines
                .iter()
                .filter(|l| l.state == PurchasingState::Resolved)
                .map(|l| line_row(engine, l)),
        );
    }
    rows
}

/// The window's first line: how many groups are unresolved, out of every
/// group and resolved line.
pub fn summary(groups: &ResolveReport, bom: Option<&BomReport>) -> String {
    let unresolved = groups.groups.len();
    let resolved = bom.map_or(0, |b| {
        b.lines
            .iter()
            .filter(|l| l.state == PurchasingState::Resolved)
            .count()
    });
    let total = unresolved + resolved;
    let noun = if total == 1 { "group" } else { "groups" };
    format!("{unresolved} of {total} {noun} unresolved")
}

/// The footer's BOM line.
pub fn bom_summary(bom: &BomReport) -> String {
    let lines = bom.lines.len();
    let noun = if lines == 1 { "line" } else { "lines" };
    format!("BOM: {lines} {noun} · {} unresolved", bom.unresolved)
}

/// The BOM as tab-separated text, one header row then one row per line,
/// for pasting into a spreadsheet.
pub fn bom_text(bom: &BomReport) -> String {
    let mut out = String::from("Quantity\tDesignators\tManufacturer\tMPN\tValues\tState\n");
    for line in &bom.lines {
        let values = line
            .values
            .iter()
            .map(|v| format!("{} {}", v.label, v.text))
            .collect::<Vec<_>>()
            .join(", ");
        let state = match line.state {
            PurchasingState::NoPart => "No part",
            PurchasingState::Unresolved => "Unresolved",
            PurchasingState::Resolved => "Resolved",
        };
        out.push_str(&format!(
            "{}\t{}\t{}\t{}\t{}\t{}\n",
            line.quantity,
            line.designators.join(", "),
            line.manufacturer.as_deref().unwrap_or(""),
            line.mpn.as_deref().unwrap_or(""),
            values,
            state
        ));
    }
    out
}

/// The table's columns.
const COLUMNS: [kit::TableColumn<'static>; 6] = [
    kit::TableColumn {
        title: "Designators",
        weight: 1.2,
    },
    kit::TableColumn {
        title: "Component",
        weight: 1.3,
    },
    kit::TableColumn {
        title: "Values",
        weight: 1.4,
    },
    kit::TableColumn {
        title: "Footprint",
        weight: 1.0,
    },
    kit::TableColumn {
        title: "Part",
        weight: 1.5,
    },
    kit::TableColumn {
        title: "",
        weight: 0.9,
    },
];

/// A severity glyph, then `text` clipped to the rest of the line.
fn glyph_line(ui: &mut Ui, level: Level, text: &str) {
    let t = Tokens::get();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = t.space_4;
        let size = t.font_label;
        let (rect, _) = ui.allocate_exact_size(vec2(size, t.control_height), Sense::hover());
        severity_glyph(ui.painter(), rect.center(), size, level);
        kit::summary_row(ui, text, None);
    });
}

impl ResolveParts {
    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn open(&mut self) {
        self.open = true;
        self.expanded = Some(0);
    }

    pub fn close(&mut self) {
        self.open = false;
    }

    fn refresh(&mut self, engine: &Engine) {
        let revision = engine.revision();
        if self.groups.as_ref().is_none_or(|(r, _)| *r != revision) {
            self.groups = Some((revision, groups(engine)));
        }
        if self.bom.as_ref().is_none_or(|(r, _)| *r != revision) {
            let bom = match engine.query(Query::Bom {}) {
                Ok(QueryResult::Bom(report)) => Ok(report),
                Ok(_) => Err("Unexpected answer from the engine".into()),
                Err(error) => Err(crate::refusal::shown_error(&error)),
            };
            self.bom = Some((revision, bom));
        }
        if self.issues.as_ref().is_none_or(|(r, _)| *r != revision) {
            let issues = match self.groups.as_ref().map(|(_, g)| g) {
                Some(Ok(report)) => {
                    let groups: Vec<&[Id]> = report
                        .groups
                        .iter()
                        .map(|g| g.instances.as_slice())
                        .collect();
                    issues_by_group(engine, &groups)
                }
                _ => Ok(Vec::new()),
            };
            self.issues = Some((revision, issues));
        }
    }

    /// The window, while open. Returns what the user chose; Close (the
    /// header's close, Esc or the footer's Close) closes it.
    pub fn window(&mut self, ctx: &egui::Context, engine: &Engine) -> Option<ResolveAction> {
        if !self.open {
            return None;
        }
        self.refresh(engine);
        let t = Tokens::get();
        let cards = ((ctx.content_rect().width() - t.space_24 * 2.0) / t.form_card_width.max(1.0))
            .clamp(MIN_CARDS, MAX_CARDS);
        let mut action = None;
        let closed = ui_kit::Window::new(
            egui::Id::unique("resolve-parts"),
            "component",
            "Resolve parts",
        )
        .cards(cards)
        .show_body(ctx, |ui| {
            kit::form(ui, |ui| action = self.body(ui, engine));
        });
        if closed {
            action = Some(ResolveAction::Close);
        }
        if action == Some(ResolveAction::Close) {
            self.open = false;
        }
        action
    }

    /// The summary, the table, the open row's details, then the BOM line
    /// and the window's actions.
    pub fn body(&mut self, ui: &mut Ui, engine: &Engine) -> Option<ResolveAction> {
        let project = engine.project();
        let mut action = None;
        // Owned copies: the table below borrows the window mutably.
        let bom_result = self.bom.as_ref().map(|(_, b)| b.clone());
        let bom = bom_result.as_ref().and_then(|b| b.as_ref().ok());
        let issues = self.issues.as_ref().map(|(_, i)| i.clone());
        let groups = self.groups.as_ref().map(|(_, g)| g.clone());
        match &groups {
            Some(Ok(_)) if project.instances.is_empty() => {
                kit::hint(ui, "Place components on the sheet first", "");
            }
            Some(Ok(report)) => {
                if report.groups.is_empty() {
                    glyph_line(ui, Level::Ok, "Every instance has a part that fits");
                } else {
                    glyph_line(ui, Level::ToDo, &summary(report, bom));
                }
                kit::check_row(ui, "Unresolved only", &mut self.unresolved_only);
                if let Some(Err(message)) = &issues {
                    kit::refusal(ui, message);
                }
                let issues = issues.and_then(Result::ok).unwrap_or_default();
                let rows = rows(engine, report, bom, &issues, self.unresolved_only);
                action = self.table(ui, &rows);
                if let Some(row) = self.expanded.and_then(|i| rows.get(i)) {
                    action = action.or(details(ui, row));
                }
            }
            Some(Err(message)) => {
                kit::refusal(ui, message);
            }
            None => {}
        }
        kit::rule(ui);
        let mut copy = false;
        if let Some(Err(message)) = &bom_result {
            kit::refusal(ui, message);
        }
        let bom_line = bom.map(bom_summary);
        kit::actions(ui, |ui| {
            if ui_kit::button(ui, "Close", Button::Normal).clicked() {
                action = Some(ResolveAction::Close);
            }
            if bom_line.is_some() {
                copy = ui_kit::button(ui, "Copy BOM", Button::Normal)
                    .on_hover_text("Copy every BOM line as tab-separated text")
                    .clicked();
            }
            if let Some(line) = &bom_line {
                kit::summary_row(ui, line, None);
            }
        });
        if copy && let Some(bom) = bom {
            ui.ctx().copy_text(bom_text(bom));
        }
        action
    }

    /// The table of rows; a click on a row's designators opens its details.
    fn table(&mut self, ui: &mut Ui, rows: &[TableRow]) -> Option<ResolveAction> {
        if rows.is_empty() {
            return None;
        }
        let t = Tokens::get();
        let mut action = None;
        let mut toggle = None;
        let expanded = self.expanded;
        kit::table(
            ui,
            &COLUMNS,
            rows.len(),
            |ui, index, column| {
                let Some(row) = rows.get(index) else {
                    return;
                };
                match column {
                    0 => {
                        let cell = ui.max_rect();
                        let click = ui
                            .interact(cell, ui.scope_id().with("fold"), Sense::click())
                            .on_hover_text("Show why and what to do");
                        if click.clicked() {
                            toggle = Some(index);
                        }
                        let (chevron, _) = ui.allocate_exact_size(
                            vec2(t.form_section_icon_size, t.control_height),
                            Sense::hover(),
                        );
                        kit::disclosure(
                            ui.painter(),
                            chevron.center(),
                            expanded == Some(index),
                            t.form_dim,
                        );
                        kit::summary_row(ui, &row.designators, None);
                    }
                    1 => {
                        kit::summary_row(ui, &row.component, None);
                    }
                    2 => {
                        kit::summary_row(ui, &row.values, None);
                    }
                    3 => {
                        kit::summary_row(ui, &row.footprint, None);
                    }
                    4 => glyph_line(ui, row.level, &row.part),
                    _ => {
                        let label = if row.resolved() {
                            "Change…"
                        } else {
                            "Choose…"
                        };
                        if kit::summary_row(ui, "", Some(label)) {
                            action = Some(ResolveAction::Choose {
                                query: row.query.clone(),
                                instances: row.instances.clone(),
                            });
                        }
                    }
                }
            },
            None,
        );
        if let Some(index) = toggle {
            self.expanded = if self.expanded == Some(index) {
                None
            } else {
                Some(index)
            };
        }
        action
    }
}

/// The open row's details: core's issue titles, then New Part… and Select
/// on sheet (both secondary).
fn details(ui: &mut Ui, row: &TableRow) -> Option<ResolveAction> {
    let mut action = None;
    kit::section(
        ui,
        "resolve-details",
        &row.designators,
        true,
        Some(row.issues.len()),
        |ui| {
            if row.issues.is_empty() {
                let text = if row.resolved() {
                    "Resolved: the part fits every instance"
                } else {
                    "No part chosen yet"
                };
                kit::status(ui, text, Tone::Dim);
            }
            kit::list_frame(ui, row.issues.len(), row.issues.len(), |ui, index| {
                if let Some((level, title)) = row.issues.get(index) {
                    glyph_line(ui, *level, title);
                }
            });
            kit::actions(ui, |ui| {
                if let Some((component, values)) = &row.new_part
                    && ui_kit::button(ui, "New Part…", Button::Normal)
                        .on_hover_text("Author a manufacturer part rated at these values")
                        .clicked()
                {
                    action = Some(ResolveAction::NewPart {
                        component: *component,
                        values: values.clone(),
                    });
                }
                if ui_kit::button(ui, "Select on sheet", Button::Normal)
                    .on_hover_text("Select the first of these instances; the window stays open")
                    .clicked()
                    && let Some(first) = row.instances.first()
                {
                    action = Some(ResolveAction::Select(*first));
                }
            });
        },
    );
    action
}

/// A published `ExampleCo` Part implementing the demo's R1, for tests.
#[cfg(test)]
pub fn demo_part(engine: &mut Engine, mpn: &str) -> pcbcraft_core::Result<PartRef> {
    use pcbcraft_core::library::{Definition, DefinitionMeta, PartDefinition};
    use pcbcraft_core::{Error, LibraryCommand, Outcome, instances};
    let instance = engine
        .project()
        .instances
        .first()
        .ok_or_else(|| Error::NotFound("an instance".into()))?;
    let component = instance.component;
    // Rated at the instance's values: an unrated requirement never passes.
    let specs = instances::requirements(&engine.project().library, instance)?
        .into_iter()
        .map(|r| fields::RatedSpec {
            meaning: match r.value_type {
                fields::ValueType::Tolerance { .. } => fields::SpecMeaning::MaximumTolerance,
                _ => fields::SpecMeaning::ExactNominal,
            },
            field: r.field,
            value: r.value,
        })
        .collect();
    let binding = engine
        .project()
        .library
        .component(component)
        .and_then(|c| c.definition.implementations.first().copied())
        .unwrap_or_default();
    let created = engine.apply(
        engine.revision(),
        Operation::Library(LibraryCommand::NewDraft {
            meta: DefinitionMeta {
                name: mpn.into(),
                ..Default::default()
            },
            definition: Definition::Part(Box::new(PartDefinition {
                manufacturer: "ExampleCo".into(),
                mpn: mpn.into(),
                component,
                binding,
                specs,
                ..Default::default()
            })),
        }),
    )?;
    let Outcome::DraftCreated { id } = created else {
        return Err(Error::Invalid("no draft".into()));
    };
    engine.apply(
        engine.revision(),
        Operation::Library(LibraryCommand::Publish {
            drafts: vec![id],
            close: false,
        }),
    )?;
    Ok(PartRef::new(id, pcbcraft_core::Revision::FIRST))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pcbcraft_core::Error;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// Every text drawn in one frame, with its centre.
    fn frame_texts(output: &egui::FullOutput) -> Vec<(String, egui::Pos2)> {
        fn walk(shape: &egui::Shape, out: &mut Vec<(String, egui::Pos2)>) {
            match shape {
                egui::Shape::Text(t) => {
                    out.push((t.galley.job.text.clone(), t.pos + t.galley.size() * 0.5))
                }
                egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, out)),
                _ => {}
            }
        }
        let mut out = Vec::new();
        for shape in &output.shapes {
            walk(&shape.shape, &mut out);
        }
        out
    }

    /// One frame of the window at the audit's narrow size.
    fn run(
        ctx: &egui::Context,
        card: &mut ResolveParts,
        engine: &Engine,
        events: Vec<egui::Event>,
    ) -> (Vec<(String, egui::Pos2)>, Option<ResolveAction>) {
        let mut action = None;
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(860.0, 1081.0),
                )),
                events,
                ..Default::default()
            },
            |ui| action = card.window(ui.ctx(), engine),
        );
        output.textures_delta.clear();
        (frame_texts(&output), action)
    }

    fn click_at(point: egui::Pos2) -> [Vec<egui::Event>; 2] {
        let press = |pressed| egui::Event::PointerButton {
            pos: point,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        [
            vec![egui::Event::PointerMoved(point), press(true)],
            vec![press(false)],
        ]
    }

    /// Without a Part, the window lists the demo's groups as a table with
    /// Choose…, counts them unresolved and never claims resolution; the
    /// open row shows core's own issue titles, which follow the message
    /// style. Choose… returns that group's instances and query.
    #[test]
    fn the_window_tables_unresolved_groups_and_choose_returns_the_group() -> TestResult {
        let ctx = egui::Context::default();
        solidcraft_ui_egui::fonts::setup_bundled_fonts(&ctx);
        let engine = Engine::demo()?;
        let Ok(QueryResult::ResolveGroups(report)) = engine.query(Query::ResolveGroups {}) else {
            return Err(Box::new(Error::Invalid("library.resolve_groups".into())));
        };
        let first = report.groups.first().ok_or("the demo has a group")?.clone();
        let mut card = ResolveParts::default();
        card.open();
        let mut shown = Vec::new();
        for _ in 0..3 {
            let (texts, action) = run(&ctx, &mut card, &engine, vec![]);
            assert_eq!(action, None);
            shown = texts;
        }
        let texts: Vec<&str> = shown.iter().map(|(t, _)| t.as_str()).collect();
        assert!(texts.contains(&"Resolve parts"), "{texts:?}");
        assert!(texts.contains(&"DESIGNATORS"), "{texts:?}");
        assert!(texts.contains(&"Copy BOM"), "{texts:?}");
        let summary = summary(&report, None);
        assert!(
            texts
                .iter()
                .any(|t| t.starts_with(&format!("{} of ", report.groups.len()))),
            "{summary} in {texts:?}"
        );
        assert!(!texts.iter().any(|t| t.contains("Every instance has")));
        // The open row's reasons are core's titles, one line each.
        let issues = card
            .issues
            .as_ref()
            .and_then(|(_, i)| i.clone().ok())
            .ok_or("issues")?;
        let rows = rows(&engine, &report, None, &issues, true);
        let row = rows.first().ok_or("a row")?;
        assert_eq!(row.level, Level::ToDo, "no part is To do, not an error");
        assert!(!row.issues.is_empty(), "the reason is shown");
        for (_, title) in &row.issues {
            assert!(texts.contains(&title.as_str()), "{title} in {texts:?}");
            assert!(
                pcbcraft_core::issues::style::check(title).is_empty(),
                "{title}"
            );
        }
        let choose = shown
            .iter()
            .find(|(t, _)| t == "Choose…")
            .map(|(_, p)| *p)
            .ok_or("Choose…")?;
        let [press, release] = click_at(choose);
        run(&ctx, &mut card, &engine, press);
        let (_, action) = run(&ctx, &mut card, &engine, release);
        assert_eq!(
            action,
            Some(ResolveAction::Choose {
                query: first.query,
                instances: first.instances,
            })
        );
        assert!(card.is_open(), "Choose… keeps the table open");
        Ok(())
    }

    /// Turning off "Unresolved only" adds the resolved BOM lines after
    /// every unresolved group, marked resolved; no unresolved group is
    /// ever left out.
    #[test]
    fn resolved_lines_join_the_table_only_on_request() -> TestResult {
        let mut engine = Engine::demo()?;
        let part = demo_part(&mut engine, "EX-R0603-10K0-F")?;
        let r1 = engine
            .project()
            .instance_by_designator("R1")
            .ok_or("R1")?
            .id;
        engine.apply(engine.revision(), assign(vec![r1], Some(part)))?;
        let Ok(QueryResult::ResolveGroups(groups)) = engine.query(Query::ResolveGroups {}) else {
            return Err("library.resolve_groups".into());
        };
        let Ok(QueryResult::Bom(bom)) = engine.query(Query::Bom {}) else {
            return Err("project.bom".into());
        };
        let only = rows(&engine, &groups, Some(&bom), &[], true);
        let all = rows(&engine, &groups, Some(&bom), &[], false);
        assert_eq!(only.len(), groups.groups.len());
        assert!(only.iter().all(|r| !r.resolved()));
        assert_eq!(&all[..only.len()], &only[..], "unresolved rows lead");
        let resolved: Vec<&TableRow> = all.iter().filter(|r| r.resolved()).collect();
        assert!(
            resolved
                .iter()
                .any(|r| r.instances.contains(&r1) && r.part.contains("EX-R0603-10K0-F")),
            "{all:?}"
        );
        assert!(resolved.iter().all(|r| r.new_part.is_none()));
        let total = groups.groups.len() + resolved.len();
        assert_eq!(
            summary(&groups, Some(&bom)),
            format!(
                "{} of {total} {} unresolved",
                groups.groups.len(),
                if total == 1 { "group" } else { "groups" }
            )
        );
        Ok(())
    }

    /// Copy BOM gives one header row and one tab-separated row per line;
    /// the footer counts lines and unresolved ones.
    #[test]
    fn the_bom_copies_as_tab_separated_rows() -> TestResult {
        let engine = Engine::demo()?;
        let Ok(QueryResult::Bom(bom)) = engine.query(Query::Bom {}) else {
            return Err("project.bom".into());
        };
        let text = bom_text(&bom);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), bom.lines.len() + 1);
        assert!(
            lines
                .first()
                .is_some_and(|l| l.starts_with("Quantity\tDesignators"))
        );
        assert!(lines.iter().skip(1).all(|l| l.split('\t').count() == 6));
        assert!(bom_summary(&bom).starts_with(&format!("BOM: {} ", bom.lines.len())));
        Ok(())
    }

    /// The header's close and the footer's Close both close the window.
    #[test]
    fn close_closes_the_window() -> TestResult {
        let ctx = egui::Context::default();
        solidcraft_ui_egui::fonts::setup_bundled_fonts(&ctx);
        let engine = Engine::demo()?;
        let mut card = ResolveParts::default();
        card.open();
        let mut shown = Vec::new();
        for _ in 0..3 {
            shown = run(&ctx, &mut card, &engine, vec![]).0;
        }
        let close = shown
            .iter()
            .find(|(t, _)| t == "Close")
            .map(|(_, p)| *p)
            .ok_or("Close")?;
        let [press, release] = click_at(close);
        run(&ctx, &mut card, &engine, press);
        let (_, action) = run(&ctx, &mut card, &engine, release);
        assert_eq!(action, Some(ResolveAction::Close));
        assert!(!card.is_open());
        Ok(())
    }

    #[test]
    fn one_assignment_is_one_command_for_the_whole_group() {
        let part = PartRef::new(
            pcbcraft_core::DefinitionId(9),
            pcbcraft_core::Revision::FIRST,
        );
        let operation = assign(vec![1, 2, 3], Some(part));
        assert!(matches!(
            operation,
            Operation::Instance(InstanceCommand::AssignPart { instances, part: Some(p) })
                if instances == [1, 2, 3] && p == part
        ));
    }

    #[test]
    fn group_titles_list_the_first_designators_then_a_count() {
        let designators: Vec<String> = (1..=8).map(|i| format!("R{i}")).collect();
        assert_eq!(
            designator_summary(&designators),
            "R1, R2, R3, R4, R5, R6 +2"
        );
        assert_eq!(designator_summary(&designators[..2]), "R1, R2");
    }
}
