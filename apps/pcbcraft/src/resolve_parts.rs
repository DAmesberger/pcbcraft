//! The Resolve parts card: the `library.resolve_groups` groups of
//! instances that share requirements and footprint, each resolved by one
//! manufacturer part with one `instance.assign_part` (one undo step), and
//! the `project.bom` lines still unresolved. Everything shown comes from
//! those two engine queries; the card only returns what the user chose.
use crate::kit::{self, Tone};
use egui::Ui;
use pcbcraft_core::completeness::{BomReport, PurchasingState, ResolveGroup, ResolveReport};
use pcbcraft_core::fields;
use pcbcraft_core::library::Library;
use pcbcraft_core::{Engine, Id, InstanceCommand, Operation, PartRef, Query, QueryResult};
use solidcraft_ui_egui::ui_kit::{self, Button};

/// Designators a group title lists before summarising the rest.
const TITLE_DESIGNATORS: usize = 6;

/// The query text Choose… starts from without a group: every Part.
pub const ANY_PART: &str = "kind:part";

/// What the user chose in the card.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResolveAction {
    /// Open the Place palette in assign mode for these instances.
    Choose {
        query: String,
        instances: Vec<Id>,
    },
    /// Select this instance on the sheet.
    Select(Id),
    Close,
}

/// A query result at one document revision; an error keeps its message.
type Cached<T> = Option<(u64, Result<T, String>)>;

#[derive(Default)]
pub struct ResolveParts {
    open: bool,
    groups: Cached<ResolveReport>,
    bom: Cached<BomReport>,
}

/// `library.resolve_groups`.
fn groups(engine: &Engine) -> Result<ResolveReport, String> {
    match engine.query(Query::ResolveGroups {}) {
        Ok(QueryResult::ResolveGroups(report)) => Ok(report),
        Ok(_) => Err("unexpected report".into()),
        Err(error) => Err(error.to_string()),
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

fn state_text(state: PurchasingState) -> &'static str {
    match state {
        PurchasingState::NoPart => "no part",
        PurchasingState::Unresolved => "unresolved",
        PurchasingState::Resolved => "resolved",
    }
}

impl ResolveParts {
    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn open(&mut self) {
        self.open = true;
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
                Ok(_) => Err("unexpected report".into()),
                Err(error) => Err(error.to_string()),
            };
            self.bom = Some((revision, bom));
        }
    }

    /// The card's sections: one per group with Choose…, then the BOM.
    pub fn show(&mut self, ui: &mut Ui, engine: &Engine) -> Option<ResolveAction> {
        if !self.open {
            return None;
        }
        self.refresh(engine);
        let project = engine.project();
        let library = &project.library;
        let mut action = None;
        match self.groups.as_ref().map(|(_, g)| g) {
            Some(Ok(report)) if report.groups.is_empty() => {
                kit::section(ui, "resolve-groups", "Groups", true, Some(0), |ui| {
                    kit::status(
                        ui,
                        if project.instances.is_empty() {
                            "Place components on the sheet first."
                        } else {
                            "Every instance has a manufacturer part that agrees with it."
                        },
                        Tone::Dim,
                    );
                });
            }
            Some(Ok(report)) => {
                for (index, group) in report.groups.iter().enumerate() {
                    if let Some(chosen) = group_section(ui, library, index, group) {
                        action = Some(chosen);
                    }
                }
            }
            Some(Err(message)) => {
                kit::section(ui, "resolve-groups", "Groups", true, None, |ui| {
                    kit::status(ui, message, Tone::Error);
                });
            }
            None => {}
        }
        if let Some((_, bom)) = &self.bom {
            bom_section(ui, bom);
        }
        kit::rule(ui);
        kit::actions(ui, |ui| {
            if ui_kit::button(ui, "Close", Button::Normal).clicked() {
                action = Some(ResolveAction::Close);
            }
        });
        if action == Some(ResolveAction::Close) {
            self.open = false;
        }
        action
    }
}

/// One group: what its instances share and Choose….
fn group_section(
    ui: &mut Ui,
    library: &Library,
    index: usize,
    group: &ResolveGroup,
) -> Option<ResolveAction> {
    let title = designator_summary(&group.designators);
    kit::section(
        ui,
        ("resolve-group", index),
        &title,
        true,
        Some(group.instances.len()),
        |ui| {
            let component = library.component(group.component).map_or_else(
                || group.component.to_string(),
                |c| format!("{} · {}", c.meta.name, group.component.revision),
            );
            kit::value_row(ui, "Component", &component);
            if group.requirements.is_empty() {
                kit::value_row(ui, "Values", "None");
            }
            for requirement in &group.requirements {
                let kind = library
                    .field_view(requirement.field)
                    .and_then(|v| v.kind_body().cloned());
                kit::value_row(
                    ui,
                    &requirement.label,
                    &fields::display(&requirement.value, &requirement.value_type, kind.as_ref()),
                );
            }
            let footprint = group.footprint.map_or_else(
                || "None".to_owned(),
                |f| {
                    library
                        .footprint(f)
                        .map_or_else(|| f.to_string(), |p| p.meta.name.clone())
                },
            );
            kit::value_row(ui, "Footprint", &footprint);
            kit::value_row(ui, "Count", &group.instances.len().to_string());
            let mut action = None;
            kit::actions(ui, |ui| {
                if kit::primary(ui, "Choose…", true)
                    .on_hover_text("Find one manufacturer part for the whole group")
                    .clicked()
                {
                    action = Some(ResolveAction::Choose {
                        query: group.query.clone(),
                        instances: group.instances.clone(),
                    });
                }
                if ui_kit::button(ui, "Select", Button::Normal)
                    .on_hover_text("Select the group's first instance on the sheet")
                    .clicked()
                    && let Some(first) = group.instances.first()
                {
                    action = Some(ResolveAction::Select(*first));
                }
            });
            action
        },
    )
    .flatten()
}

/// The BOM lines, unresolved ones marked.
fn bom_section(ui: &mut Ui, bom: &Result<BomReport, String>) {
    let count = bom.as_ref().ok().map(|b| b.lines.len());
    kit::section(ui, "resolve-bom", "BOM", false, count, |ui| match bom {
        Ok(report) => {
            if report.lines.is_empty() {
                kit::note(ui, "No BOM lines yet.");
            } else if report.unresolved == 0 {
                kit::status(ui, "Every line names a resolved Part.", Tone::Note);
            } else {
                kit::status(
                    ui,
                    &format!(
                        "{} of {} lines are unresolved.",
                        report.unresolved,
                        report.lines.len()
                    ),
                    Tone::Warning,
                );
            }
            for line in &report.lines {
                let part = match (&line.manufacturer, &line.mpn) {
                    (Some(m), Some(p)) => format!("{m} {p}"),
                    (None, Some(p)) => p.clone(),
                    _ => "—".to_owned(),
                };
                let shown = if line.unresolved {
                    format!("{} × {part} · {}", line.quantity, state_text(line.state))
                } else {
                    format!("{} × {part}", line.quantity)
                };
                kit::value_row(ui, &designator_summary(&line.designators), &shown);
            }
        }
        Err(message) => {
            kit::status(ui, message, Tone::Error);
        }
    });
}

/// A published `ExampleCo` Part implementing the demo's R1, for tests.
#[cfg(test)]
pub fn demo_part(engine: &mut Engine, mpn: &str) -> pcbcraft_core::Result<PartRef> {
    use pcbcraft_core::library::{Definition, DefinitionMeta, PartDefinition};
    use pcbcraft_core::{Error, LibraryCommand, Outcome};
    let instance = engine
        .project()
        .instances
        .first()
        .ok_or_else(|| Error::NotFound("an instance".into()))?;
    let component = instance.component;
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
                ..Default::default()
            })),
        }),
    )?;
    let Outcome::DraftCreated { id } = created else {
        return Err(Error::Invalid("no draft".into()));
    };
    engine.apply(
        engine.revision(),
        Operation::Library(LibraryCommand::Publish { drafts: vec![id] }),
    )?;
    Ok(PartRef::new(id, pcbcraft_core::Revision::FIRST))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pcbcraft_core::Error;

    /// Without a Part, the card lists the demo's group with Choose… and
    /// does not claim that every instance is resolved; drawing it returns
    /// no action.
    #[test]
    fn the_card_lists_unresolved_groups_without_claiming_resolution() -> pcbcraft_core::Result<()> {
        let ctx = egui::Context::default();
        solidcraft_ui_egui::fonts::setup_bundled_fonts(&ctx);
        let engine = Engine::demo()?;
        let mut card = ResolveParts::default();
        card.open();
        let mut texts = Vec::new();
        let mut action = None;
        let mut output = ctx.run_ui(Default::default(), |ui| {
            action = card.show(ui, &engine);
        });
        output.textures_delta.clear();
        fn collect(shape: &egui::Shape, out: &mut Vec<String>) {
            match shape {
                egui::Shape::Text(t) => out.push(t.galley.job.text.clone()),
                egui::Shape::Vec(v) => v.iter().for_each(|s| collect(s, out)),
                _ => {}
            }
        }
        for shape in &output.shapes {
            collect(&shape.shape, &mut texts);
        }
        assert_eq!(action, None);
        let Ok(QueryResult::ResolveGroups(report)) = engine.query(Query::ResolveGroups {}) else {
            return Err(Error::Invalid("library.resolve_groups".into()));
        };
        // The demo's instances have no Part: one group, one Choose….
        assert!(!report.groups.is_empty());
        assert!(texts.iter().any(|t| t == "Choose…"), "{texts:?}");
        assert!(!texts.iter().any(|t| t.contains("Every instance has")));
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
