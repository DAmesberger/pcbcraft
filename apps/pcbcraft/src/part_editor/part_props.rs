//! The Parts section of the Component workspace: the manufacturer parts
//! (S2) of the component being authored. A Part is its own revisioned
//! lineage that pins one component revision and one of its bindings, so it
//! is edited as its own draft beside the five the working set holds.
//!
//! Every change reaches the engine as public library commands:
//! `library.new_draft` and `library.draft_from` start a Part draft,
//! `library.edit` commits a finished edit as one undo step, and
//! `library.publish` publishes it, with the component's cascade when the
//! component has unpublished edits. Rated values are typed through
//! `fields::parse_value`; the core decides what publishes.
use super::FieldText;
use super::working_set::WorkingSet;
use crate::kit::{self, Tone};
use egui::Ui;
use pcbcraft_core::completeness::CheckTarget;
use pcbcraft_core::fields::{self, MAX_RATED_SPECS, RatedSpec, SpecMeaning, ValueType};
use pcbcraft_core::library::{
    Basis, Definition, DefinitionMeta, DistributorOffer, DraftEdit, Drafts, FieldView, Library,
    MAX_OFFERS, MAX_REFERENCES, PartDefinition, Provenance, Reference, ReferenceKind,
};
use pcbcraft_core::{
    BindingRef, ComponentRef, DefinitionId, Engine, FieldRef, LibraryCommand, Operation, PartRef,
    Query, QueryResult, Stage,
};
use solidcraft_ui_egui::ui_kit::{self, Button};
use std::collections::BTreeMap;
use std::sync::Arc;

/// The revisions a Part of the working set pins when it publishes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pins {
    pub component: ComponentRef,
    pub binding: BindingRef,
    /// The component has unpublished edits: the Part publishes with the
    /// cascade footprint → binding → component → Part, pinning the pending
    /// revisions.
    pub cascade: bool,
}

impl Pins {
    /// The pending revisions of the working set's component and binding:
    /// what a Part pins when it publishes with the cascade.
    pub fn pending(set: &WorkingSet, drafts: &Drafts) -> Option<Self> {
        let component = drafts.get(set.ids.component)?.pending()?;
        let binding = drafts.get(set.ids.binding)?.pending()?;
        Some(Self {
            component: ComponentRef::new(set.ids.component, component),
            binding: BindingRef::new(set.ids.binding, binding),
            cascade: true,
        })
    }

    /// The pending revisions while the working set has unpublished edits;
    /// otherwise the latest published component and the revision it lists
    /// for the working set's binding.
    pub fn of(set: &WorkingSet, engine: &Engine) -> Option<Self> {
        if set.unpublished(engine) {
            return Self::pending(set, engine.drafts());
        }
        let component = set.published(engine)?;
        let body = &engine.project().library.component(component)?.definition;
        let binding = body
            .implementations
            .iter()
            .find(|b| b.id == set.ids.binding)
            .or(body.implementations.first())?;
        Some(Self {
            component,
            binding: *binding,
            cascade: false,
        })
    }

    /// `part` pinning these revisions. A Part of another binding lineage
    /// keeps its binding: the working set cannot say which revision of it
    /// the component lists.
    fn apply(&self, part: &mut PartDefinition) {
        part.component = self.component;
        if part.binding.id == self.binding.id || part.binding.id.0 == 0 {
            part.binding = self.binding;
        }
    }
}

/// One Part draft as the section edits it.
#[derive(Clone, Debug, PartialEq)]
struct Edit {
    id: DefinitionId,
    meta: DefinitionMeta,
    part: PartDefinition,
}

impl Edit {
    fn draft_edit(&self) -> DraftEdit {
        DraftEdit {
            id: self.id,
            meta: self.meta.clone(),
            definition: Definition::Part(Box::new(self.part.clone())),
        }
    }
}

/// What the section asks the Component editor to do.
#[derive(Clone, Debug)]
pub enum PartsAction {
    /// Send this operation (boxed: operations are large).
    Apply(Box<Operation>),
    /// Publish the working set's cascade, which includes the Part drafts.
    PublishCascade,
}

/// A Part lineage of the component: its draft and its latest revision.
struct Listed {
    id: DefinitionId,
    draft: Option<PartDefinition>,
    published: Option<(PartRef, PartDefinition)>,
}

impl Listed {
    /// The draft differs from the latest revision, or there is none yet.
    fn unpublished(&self) -> bool {
        match (&self.draft, &self.published) {
            (Some(draft), Some((_, published))) => draft != published,
            (Some(_), None) => true,
            (None, _) => false,
        }
    }

    fn body(&self) -> Option<&PartDefinition> {
        self.draft
            .as_ref()
            .or(self.published.as_ref().map(|(_, p)| p))
    }
}

#[derive(Default)]
pub struct PartProps {
    /// The Part lineage shown under the list.
    open: Option<DefinitionId>,
    /// Working copy of the open Part's draft.
    draft: Option<Edit>,
    /// What the working copy was loaded from; a difference is uncommitted.
    baseline: Option<Edit>,
    source: Option<Arc<Drafts>>,
    /// A command was sent; resync whatever its outcome.
    awaiting: bool,
    /// A `library.new_draft` was sent: open the Part draft it creates.
    creating: bool,
    /// Publish the open draft once the edit before it has been applied.
    publish_next: bool,
    /// Typed rated values by field lineage.
    specs: BTreeMap<DefinitionId, FieldText>,
    findings: Option<(u64, DefinitionId, Vec<String>)>,
    notice: String,
    /// Fields added under Rated values that have no value typed yet. They
    /// join the draft once their text parses.
    pending: Vec<(FieldRef, SpecMeaning)>,
    /// Publish was pressed for the open draft: show its findings even
    /// before anything was typed.
    tried: Option<DefinitionId>,
    /// A New Part was requested from the schematic (Resolve parts): draft
    /// one as soon as the component's pins are known, with these values.
    prefill: Option<Vec<(FieldRef, fields::FieldValue)>>,
}

/// Part drafts and published Parts of one component lineage, by lineage.
fn listed(engine: &Engine, component: DefinitionId) -> Vec<Listed> {
    let mut out: BTreeMap<DefinitionId, Listed> = BTreeMap::new();
    for part in engine.project().library.latest_parts() {
        if part.definition.component.id == component {
            out.insert(
                part.id,
                Listed {
                    id: part.id,
                    draft: None,
                    published: Some((
                        PartRef::new(part.id, part.revision),
                        part.definition.clone(),
                    )),
                },
            );
        }
    }
    for draft in &engine.drafts().items {
        if let Definition::Part(part) = &draft.definition
            && part.component.id == component
        {
            out.entry(draft.id)
                .or_insert(Listed {
                    id: draft.id,
                    draft: None,
                    published: None,
                })
                .draft = Some((**part).clone());
        }
    }
    out.into_values().collect()
}

/// The Part drafts of `component`, with their envelopes.
fn part_drafts(drafts: &Drafts, component: DefinitionId) -> Vec<Edit> {
    drafts
        .items
        .iter()
        .filter_map(|d| match &d.definition {
            Definition::Part(part) if part.component.id == component => Some(Edit {
                id: d.id,
                meta: d.meta.clone(),
                part: (**part).clone(),
            }),
            _ => None,
        })
        .collect()
}

/// The meanings a value type can be compared by.
fn meanings(value_type: &ValueType) -> Vec<(SpecMeaning, String)> {
    let all: &[SpecMeaning] = match value_type {
        ValueType::Quantity { .. } | ValueType::Integer { .. } => {
            &[SpecMeaning::ExactNominal, SpecMeaning::MinimumRating]
        }
        ValueType::Tolerance { .. } => &[SpecMeaning::MaximumTolerance],
        ValueType::Range { .. } => &[SpecMeaning::ExactNominal, SpecMeaning::MinimumRating],
        ValueType::Boolean | ValueType::Enum { .. } | ValueType::Text { .. } => {
            &[SpecMeaning::ExactNominal]
        }
    };
    all.iter().map(|m| (*m, meaning_label(*m).into())).collect()
}

/// The meaning a new rated value of this type starts with.
fn default_meaning(value_type: &ValueType) -> SpecMeaning {
    match value_type {
        ValueType::Tolerance { .. } => SpecMeaning::MaximumTolerance,
        _ => SpecMeaning::ExactNominal,
    }
}

pub fn meaning_label(meaning: SpecMeaning) -> &'static str {
    match meaning {
        SpecMeaning::ExactNominal => "Exact value",
        SpecMeaning::MinimumRating => "Rated at least",
        SpecMeaning::MaximumTolerance => "No wider than",
    }
}

fn reference_kind_label(kind: ReferenceKind) -> &'static str {
    match kind {
        ReferenceKind::Datasheet => "Datasheet",
        ReferenceKind::ApplicationNote => "Application note",
        ReferenceKind::Other => "Other",
    }
}

/// A rated value as its field displays it.
pub fn spec_text(library: &Library, spec: &RatedSpec) -> String {
    match library.field_view(spec.field) {
        Some(view) => fields::display(
            &spec.value,
            &view.field.definition.value_type,
            view.kind_body(),
        ),
        None => format!("{:?}", spec.value),
    }
}

fn spec_label(library: &Library, spec: &RatedSpec) -> String {
    library
        .field_view(spec.field)
        .map_or_else(|| spec.field.to_string(), |v| v.field.meta.name.clone())
}

/// Parse a rated value; empty text is an error, a spec always has a value.
fn parse_spec(text: &str, view: &FieldView<'_>) -> Result<fields::FieldValue, String> {
    if text.trim().is_empty() {
        return Err("A rated value needs a value.".into());
    }
    fields::parse_value(text, &view.field.definition.value_type, view.kind_body())
        .map_err(|e| e.to_string())
}

/// A new Part of the working set: its pins, and a rated value for every
/// component field with a default, compared by the field type's usual
/// meaning. Manufacturer and MPN are left for the author.
pub fn new_part(set: &WorkingSet, pins: &Pins) -> PartDefinition {
    let specs = set
        .component
        .fields
        .iter()
        .filter_map(|selection| {
            let value = selection.default.clone()?;
            Some((selection.field, value))
        })
        .take(MAX_RATED_SPECS)
        .collect::<Vec<_>>();
    let mut part = PartDefinition {
        component: pins.component,
        binding: pins.binding,
        ..Default::default()
    };
    part.specs = specs
        .into_iter()
        .map(|(field, value)| RatedSpec {
            meaning: match &value {
                fields::FieldValue::Tolerance { .. } => SpecMeaning::MaximumTolerance,
                _ => SpecMeaning::ExactNominal,
            },
            field,
            value,
        })
        .collect();
    part
}

/// Allocate one reference or offer ID.
fn allocate(part: &mut PartDefinition) -> Option<u32> {
    let id = part.next_local;
    part.next_local = id.checked_add(1)?;
    Some(id)
}

/// An optional text in a text row: empty is `None`.
fn optional_row(ui: &mut Ui, label: &str, value: &mut Option<String>, hint: &str) {
    let mut text = value.clone().unwrap_or_default();
    if kit::checked_text_row(ui, label, &mut text, hint, false).changed() {
        *value = (!text.trim().is_empty()).then_some(text);
    }
}

/// `new_part` with requested values in place of the component defaults,
/// for every field the component selects.
fn prefilled(
    set: &WorkingSet,
    pins: &Pins,
    library: &Library,
    values: &[(FieldRef, fields::FieldValue)],
) -> PartDefinition {
    let mut part = new_part(set, pins);
    for (field, value) in values {
        let Some(selection) = set.component.fields.iter().find(|f| f.field.id == field.id) else {
            continue;
        };
        let meaning = library
            .field_view(selection.field)
            .map_or(SpecMeaning::ExactNominal, |v| {
                default_meaning(&v.field.definition.value_type)
            });
        let full = part.specs.len() >= MAX_RATED_SPECS;
        match part.specs.iter_mut().find(|s| s.field.id == field.id) {
            Some(spec) => spec.value = value.clone(),
            None if !full => part.specs.push(RatedSpec {
                field: selection.field,
                value: value.clone(),
                meaning,
            }),
            None => {}
        }
    }
    part
}

impl PartProps {
    /// Draft a new Part with `values` as its rated values once the section
    /// is shown with known pins.
    pub fn request_new(&mut self, values: Vec<(FieldRef, fields::FieldValue)>) {
        self.prefill = Some(values);
    }

    /// Reload the working copy when the engine's drafts changed.
    fn sync(&mut self, engine: &Engine, component: DefinitionId) {
        let current = Arc::clone(&engine.document().drafts);
        let changed = self
            .source
            .as_ref()
            .is_none_or(|s| !Arc::ptr_eq(s, &current));
        if !changed && !self.awaiting {
            return;
        }
        self.awaiting = false;
        let drafts = part_drafts(&current, component);
        if std::mem::take(&mut self.creating) {
            // Lineages are allocated in increasing order: the newest draft
            // is the one just created.
            if let Some(newest) = drafts.iter().map(|d| d.id).max() {
                self.open = Some(newest);
            }
        }
        let listed = listed(engine, component);
        if self
            .open
            .is_some_and(|id| !listed.iter().any(|l| l.id == id))
        {
            self.open = None;
        }
        if self.open.is_none() {
            self.open = listed.first().map(|l| l.id);
        }
        let draft = self
            .open
            .and_then(|id| drafts.into_iter().find(|d| d.id == id));
        if draft.as_ref().map(|d| d.id) != self.draft.as_ref().map(|d| d.id) {
            self.specs.clear();
        }
        self.baseline = draft.clone();
        self.draft = draft;
        self.source = Some(current);
    }

    /// The Part draft IDs of `component`, for the cascade's publish set.
    pub fn draft_ids(engine: &Engine, component: DefinitionId) -> Vec<DefinitionId> {
        part_drafts(engine.drafts(), component)
            .into_iter()
            .map(|d| d.id)
            .collect()
    }

    /// Part drafts of `component` with an identity typed (maker or MPN).
    /// A blank new Part's findings wait in its own section until the author
    /// types an identity or presses Publish, so lists elsewhere skip it.
    pub fn typed_draft_ids(engine: &Engine, component: DefinitionId) -> Vec<DefinitionId> {
        part_drafts(engine.drafts(), component)
            .into_iter()
            .filter(|d| !d.part.mpn.trim().is_empty() || !d.part.manufacturer.trim().is_empty())
            .map(|d| d.id)
            .collect()
    }

    /// Draft edits that make every Part draft of `component` pin `pins`,
    /// including the uncommitted working copy. The section then waits for
    /// the engine, so the next sync shows what it accepted.
    pub fn retarget_edits(
        &mut self,
        engine: &Engine,
        component: DefinitionId,
        pins: &Pins,
    ) -> Vec<DraftEdit> {
        let mut edits = Vec::new();
        for mut edit in part_drafts(engine.drafts(), component) {
            let working = self.draft.as_ref().filter(|d| d.id == edit.id);
            if let Some(working) = working {
                edit = working.clone();
            }
            let before = edit.clone();
            pins.apply(&mut edit.part);
            let committed = engine
                .drafts()
                .get(edit.id)
                .map(|d| (&d.meta, &d.definition));
            let same = committed.is_some_and(|(meta, definition)| {
                *meta == edit.meta && matches!(definition, Definition::Part(p) if **p == edit.part)
            });
            if working.is_some() {
                self.draft = Some(edit.clone());
                self.baseline = self.draft.clone();
            }
            if !same || before != edit {
                edits.push(edit.draft_edit());
            }
        }
        if !edits.is_empty() {
            self.awaiting = true;
        }
        edits
    }

    /// A deferred `library.publish` of the open draft, once the edit sent
    /// before it has been applied.
    pub fn publish_pending(
        &mut self,
        engine: &Engine,
        component: DefinitionId,
    ) -> Option<Operation> {
        self.sync(engine, component);
        if !self.publish_next || self.awaiting || self.draft != self.baseline {
            return None;
        }
        self.publish_next = false;
        let id = self.draft.as_ref()?.id;
        self.awaiting = true;
        Some(Operation::Library(LibraryCommand::Publish {
            drafts: vec![id],
        }))
    }

    /// The draft edit committing a finished edit of the working copy.
    pub fn take_edit(&mut self, engine: &Engine, component: DefinitionId) -> Option<DraftEdit> {
        self.sync(engine, component);
        if self.draft == self.baseline {
            return None;
        }
        let edit = self.draft.as_ref()?.draft_edit();
        self.baseline = self.draft.clone();
        self.awaiting = true;
        Some(edit)
    }

    /// Blocking publish findings of the open draft, from `component.check`.
    fn findings(&mut self, engine: &Engine) -> Vec<String> {
        let Some(id) = self.draft.as_ref().map(|d| d.id) else {
            return Vec::new();
        };
        if let Some((revision, open, findings)) = &self.findings
            && *revision == engine.revision()
            && *open == id
        {
            return findings.clone();
        }
        let messages: Vec<String> = match engine.query(Query::CheckDrafts {}) {
            Ok(QueryResult::Drafts(reports)) => reports
                .into_iter()
                .filter(|r| r.target == CheckTarget::Draft(id))
                .flat_map(|r| r.findings)
                .filter(|f| f.blocks_stage(Stage::Publish))
                .map(|f| f.message)
                .collect(),
            Ok(_) => Vec::new(),
            Err(error) => vec![error.to_string()],
        };
        self.findings = Some((engine.revision(), id, messages.clone()));
        messages
    }

    /// Publish the open draft alone, retargeting it first when its pins are
    /// not the published component's.
    fn publish_alone(
        &mut self,
        engine: &Engine,
        component: DefinitionId,
        pins: &Pins,
    ) -> Option<Operation> {
        let edits = self.retarget_edits(engine, component, pins);
        let id = self.draft.as_ref()?.id;
        if edits.is_empty() {
            self.awaiting = true;
            return Some(Operation::Library(LibraryCommand::Publish {
                drafts: vec![id],
            }));
        }
        self.publish_next = true;
        Some(Operation::Library(LibraryCommand::Edit { edits }))
    }

    /// The Parts list and, for the open Part, its identity, rated values,
    /// references, offers and provenance.
    ///
    /// `blocked` is the first finding that blocks publishing the component
    /// drafts, which a cascade publish would publish too.
    pub fn section(
        &mut self,
        ui: &mut Ui,
        engine: &Engine,
        set: &WorkingSet,
        blocked: Option<&str>,
    ) -> Option<PartsAction> {
        let component = set.ids.component;
        self.sync(engine, component);
        let library = &engine.project().library;
        let pins = Pins::of(set, engine);
        let listed = listed(engine, component);
        let mut action = None;
        let mut open = self.open;
        kit::section(ui, "parts", "Parts", true, Some(listed.len()), |ui| {
            if listed.is_empty() {
                kit::note(
                    ui,
                    "No manufacturer part yet. A Part is one exact orderable item: \
                     manufacturer, MPN and rated values, pinned to this component.",
                );
            }
            for item in &listed {
                let body = item.body();
                let mpn = body
                    .map(|p| {
                        if p.mpn.is_empty() {
                            "New part".to_owned()
                        } else if p.manufacturer.is_empty() {
                            p.mpn.clone()
                        } else {
                            format!("{} {}", p.manufacturer, p.mpn)
                        }
                    })
                    .unwrap_or_default();
                let state = match (&item.draft, &item.published) {
                    (Some(_), None) => "draft · new".to_owned(),
                    (Some(_), Some((r, _))) if item.unpublished() => {
                        format!("{} · draft", r.revision)
                    }
                    (Some(_), Some((r, _))) => r.revision.to_string(),
                    (None, Some((r, _))) => r.revision.to_string(),
                    (None, None) => String::new(),
                };
                let label = format!("{mpn}   {state}");
                if ui.selectable_label(open == Some(item.id), label).clicked() {
                    open = Some(item.id);
                }
            }
            kit::actions(ui, |ui| {
                let enabled = pins.is_some();
                let clicked = ui_kit::button(ui, "New Part", Button::Enabled(enabled))
                    .on_hover_text("Draft a manufacturer part of this component")
                    .clicked();
                let requested = enabled && self.prefill.is_some() && !self.awaiting;
                if (clicked || requested)
                    && let Some(pins) = &pins
                {
                    let values = self.prefill.take().unwrap_or_default();
                    self.creating = true;
                    self.awaiting = true;
                    action = Some(PartsAction::Apply(Box::new(Operation::Library(
                        LibraryCommand::NewDraft {
                            meta: DefinitionMeta::default(),
                            definition: Definition::Part(Box::new(prefilled(
                                set, pins, library, &values,
                            ))),
                        },
                    ))))
                }
            });
        });
        if open != self.open {
            self.open = open;
            self.source = None;
            self.sync(engine, component);
        }
        let Some(item) = listed.iter().find(|l| Some(l.id) == self.open) else {
            return action;
        };
        if self.draft.is_none() {
            if let Some((r, part)) = &item.published {
                action = action.or(self.published_view(ui, library, *r, part));
            }
            return action;
        }
        let findings = self.findings(engine);
        let notice = std::mem::take(&mut self.notice);
        let tried = self
            .draft
            .as_ref()
            .is_some_and(|d| Some(d.id) == self.tried);
        let Some(edit) = &mut self.draft else {
            return action;
        };
        let published = item.published.as_ref().map(|(r, _)| *r);
        let unpublished = item
            .published
            .as_ref()
            .is_none_or(|(_, latest)| *latest != edit.part);
        let mut publish = false;
        let mut discard = false;
        let title = if edit.part.mpn.is_empty() {
            "Part · new".to_owned()
        } else {
            format!("Part · {}", edit.part.mpn)
        };
        // Findings wait until the author typed an identity or tried to
        // publish; the name finding repeats the MPN one while the name
        // follows the MPN.
        let typed = !edit.part.mpn.trim().is_empty() || !edit.part.manufacturer.trim().is_empty();
        let follows = edit.meta.name == edit.part.mpn.trim();
        let shown: Vec<&String> = findings
            .iter()
            .filter(|_| typed || tried)
            .filter(|m| !(follows && m.contains("printable name")))
            .take(4)
            .collect();
        kit::section(ui, "part-identity", &title, true, None, |ui| {
            for message in shown {
                kit::status(ui, message, Tone::Error);
            }
            if !notice.is_empty() {
                kit::status(ui, &notice, Tone::Note);
            }
            kit::checked_text_row(
                ui,
                "Maker",
                &mut edit.part.manufacturer,
                "Manufacturer",
                false,
            )
            .on_hover_text("The manufacturer's name, as text");
            let mpn = kit::checked_text_row(ui, "MPN", &mut edit.part.mpn, "Full MPN", false)
                .on_hover_text("The complete manufacturer part number, ordering suffixes included");
            if mpn.changed() {
                edit.meta.name = edit.part.mpn.trim().to_owned();
            }
            let component = library.component(edit.part.component).map_or_else(
                || {
                    format!(
                        "{} · {} · pending",
                        set.metas.component.name, edit.part.component.revision
                    )
                },
                |c| format!("{} · {}", c.meta.name, edit.part.component.revision),
            );
            kit::value_row(ui, "Implements", &component);
            let footprint = if library.binding(edit.part.binding).is_some() {
                pcbcraft_core::instances::binding_name(library, edit.part.binding)
            } else {
                format!("{} · pending", edit.part.binding.revision)
            };
            kit::value_row(ui, "Footprint", &footprint);
            let next = |r: PartRef| {
                r.revision
                    .next()
                    .map_or_else(String::new, |n| n.to_string())
            };
            let revision = match published {
                None => "Draft · new".to_owned(),
                Some(r) if unpublished => {
                    format!("{} · unpublished edits → {}", r.revision, next(r))
                }
                Some(r) => format!("{} · published; edits create {}", r.revision, next(r)),
            };
            kit::value_row(ui, "Revision", &revision);
            if let Some(pins) = &pins
                && (edit.part.component != pins.component || edit.part.binding != pins.binding)
            {
                kit::note(
                    ui,
                    &format!(
                        "Publishing pins component {} and binding {}.",
                        pins.component.revision, pins.binding.revision
                    ),
                );
            }
            kit::actions(ui, |ui| {
                let label = if pins.is_some_and(|p| p.cascade) {
                    "Publish with component"
                } else {
                    "Publish Part"
                };
                // A draft that cannot publish disables the button and says
                // why on hover, instead of offering a refused publish.
                let why = findings
                    .first()
                    .map(String::as_str)
                    .or_else(|| pins.is_some_and(|p| p.cascade).then_some(blocked).flatten());
                let response =
                    kit::primary(ui, label, pins.is_some() && unpublished && why.is_none())
                        .on_hover_text("Publish the Part as a new immutable revision");
                publish = match why {
                    Some(why) if pins.is_some() && unpublished => response
                        .on_disabled_hover_text(format!("Fix first: {why}"))
                        .clicked(),
                    _ => response.clicked(),
                };
                discard = ui_kit::button(ui, "Discard", Button::Normal)
                    .on_hover_text(match published {
                        Some(r) => format!(
                            "Drop this draft; the published {} stays as it is",
                            r.revision
                        ),
                        None => "Drop this new Part draft".to_owned(),
                    })
                    .clicked();
            });
        });
        if publish {
            self.tried = Some(edit.id);
        }
        Self::specs_section(ui, library, set, edit, &mut self.specs, &mut self.pending);
        Self::references_section(ui, &mut edit.part);
        Self::offers_section(ui, &mut edit.part);
        Self::provenance_section(ui, &mut edit.part);
        let id = edit.id;
        if discard {
            self.awaiting = true;
            return Some(PartsAction::Apply(Box::new(Operation::Library(
                LibraryCommand::DiscardDraft { id },
            ))));
        }
        if publish
            && unpublished
            && findings.is_empty()
            && let Some(pins) = pins
        {
            if pins.cascade {
                return Some(PartsAction::PublishCascade);
            }
            return self
                .publish_alone(engine, component, &pins)
                .map(|op| PartsAction::Apply(Box::new(op)));
        }
        action
    }

    /// A published Part without a draft: read-only, with Edit.
    fn published_view(
        &mut self,
        ui: &mut Ui,
        library: &Library,
        r: PartRef,
        part: &PartDefinition,
    ) -> Option<PartsAction> {
        let mut edit = false;
        kit::section(
            ui,
            "part-identity",
            &format!("Part · {}", part.mpn),
            true,
            None,
            |ui| {
                kit::value_row(ui, "Maker", &part.manufacturer);
                kit::value_row(ui, "MPN", &part.mpn);
                let component = library.component(part.component).map_or_else(
                    || part.component.to_string(),
                    |c| format!("{} · {}", c.meta.name, part.component.revision),
                );
                kit::value_row(ui, "Implements", &component);
                kit::value_row(ui, "Revision", &format!("{} · published", r.revision));
                for spec in &part.specs {
                    kit::value_row(
                        ui,
                        &spec_label(library, spec),
                        &format!(
                            "{} · {}",
                            spec_text(library, spec),
                            meaning_label(spec.meaning)
                        ),
                    );
                }
                for reference in &part.references {
                    kit::value_row(ui, reference_kind_label(reference.kind), &reference.title);
                }
                for offer in &part.offers {
                    kit::value_row(ui, &offer.distributor, &offer.sku);
                }
                kit::actions(ui, |ui| {
                    edit = ui_kit::button(ui, "Edit", Button::Normal)
                        .on_hover_text("Draft a new revision of this Part")
                        .clicked();
                });
            },
        );
        if edit {
            self.awaiting = true;
            return Some(PartsAction::Apply(Box::new(Operation::Library(
                LibraryCommand::DraftFrom {
                    revision: r.erased(),
                },
            ))));
        }
        None
    }

    /// Rated values: one typed row per field lineage, with its meaning.
    fn specs_section(
        ui: &mut Ui,
        library: &Library,
        set: &WorkingSet,
        edit: &mut Edit,
        buffers: &mut BTreeMap<DefinitionId, FieldText>,
        pending: &mut Vec<(FieldRef, SpecMeaning)>,
    ) {
        let part = &mut edit.part;
        pending.retain(|(field, _)| {
            !part.specs.iter().any(|s| s.field.id == field.id)
                && set.component.fields.iter().any(|f| f.field.id == field.id)
        });
        buffers.retain(|lineage, _| {
            part.specs.iter().any(|s| s.field.id == *lineage)
                || pending.iter().any(|(f, _)| f.id == *lineage)
        });
        let count = part.specs.len() + pending.len();
        kit::section(ui, "part-specs", "Rated values", true, Some(count), |ui| {
            let mut remove = None;
            for index in 0..part.specs.len() {
                let Some(spec) = part.specs.get_mut(index) else {
                    continue;
                };
                let lineage = spec.field.id;
                if index > 0 {
                    kit::rule(ui);
                }
                ui.push_id(("spec", lineage), |ui| {
                    let Some(view) = library.field_view(spec.field) else {
                        kit::value_row(ui, "Field", &spec.field.to_string());
                        kit::status(
                            ui,
                            "This field revision is not in the project library.",
                            Tone::Error,
                        );
                        return;
                    };
                    let value_type = &view.field.definition.value_type;
                    let shown = fields::display(&spec.value, value_type, view.kind_body());
                    let buffer = buffers
                        .entry(lineage)
                        .or_insert_with(|| FieldText::new(shown.clone()));
                    if buffer.loaded != shown {
                        *buffer = FieldText::new(shown);
                    }
                    let invalid = parse_spec(&buffer.text, &view).is_err();
                    let response = kit::row(ui, &view.field.meta.name, |ui| {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui_kit::button(ui, "Remove", Button::Small)
                                .on_hover_text("Remove this rated value")
                                .clicked()
                            {
                                remove = Some(index);
                            }
                            ui.spacing_mut().text_edit_width = ui.available_width().max(0.0);
                            kit::text_field(ui, "spec", &mut buffer.text, "value", invalid)
                                .on_hover_text("The rated value, typed as the field reads it")
                        })
                        .inner
                    });
                    if response.changed()
                        && let Ok(value) = parse_spec(&buffer.text, &view)
                    {
                        buffer.loaded = fields::display(&value, value_type, view.kind_body());
                        spec.value = value;
                    }
                    let error = parse_spec(&buffer.text, &view).err();
                    if !response.has_focus() && error.is_none() && buffer.text != buffer.loaded {
                        buffer.text = buffer.loaded.clone();
                    }
                    if let Some(error) = &error {
                        kit::status(ui, error, Tone::Error);
                    }
                    let options = meanings(value_type);
                    kit::choice_row(ui, "Meaning", &mut spec.meaning, &options);
                });
            }
            if let Some(index) = remove
                && index < part.specs.len()
            {
                part.specs.remove(index);
            }
            // Added fields without a value yet: typed text that parses
            // joins the draft.
            let mut drop = None;
            let mut filled = None;
            for (index, (field, meaning)) in pending.iter_mut().enumerate() {
                let Some(view) = library.field_view(*field) else {
                    continue;
                };
                if index > 0 || !part.specs.is_empty() {
                    kit::rule(ui);
                }
                ui.push_id(("pending-spec", field.id), |ui| {
                    let buffer = buffers
                        .entry(field.id)
                        .or_insert_with(|| FieldText::new(String::new()));
                    let invalid =
                        !buffer.text.trim().is_empty() && parse_spec(&buffer.text, &view).is_err();
                    let response = kit::row(ui, &view.field.meta.name, |ui| {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui_kit::button(ui, "Remove", Button::Small)
                                .on_hover_text("Remove this rated value")
                                .clicked()
                            {
                                drop = Some(index);
                            }
                            ui.spacing_mut().text_edit_width = ui.available_width().max(0.0);
                            kit::text_field(
                                ui,
                                "spec",
                                &mut buffer.text,
                                "type the rated value",
                                invalid,
                            )
                            .on_hover_text("The rated value, typed as the field reads it")
                        })
                        .inner
                    });
                    match parse_spec(&buffer.text, &view) {
                        Ok(value) if response.changed() || response.lost_focus() => {
                            buffer.loaded = fields::display(
                                &value,
                                &view.field.definition.value_type,
                                view.kind_body(),
                            );
                            filled = Some((index, value));
                        }
                        Err(error) if !buffer.text.trim().is_empty() => {
                            kit::status(ui, &error, Tone::Error);
                        }
                        _ => {
                            kit::note(ui, "Not part of the draft until a value is typed.");
                        }
                    }
                    let options = meanings(&view.field.definition.value_type);
                    kit::choice_row(ui, "Meaning", meaning, &options);
                });
            }
            if let Some((index, value)) = filled
                && index < pending.len()
            {
                let (field, meaning) = pending.remove(index);
                part.specs.push(RatedSpec {
                    field,
                    value,
                    meaning,
                });
            } else if let Some(index) = drop
                && index < pending.len()
            {
                pending.remove(index);
            }
            if count > 0 {
                kit::rule(ui);
            }
            let addable: Vec<_> = set
                .component
                .fields
                .iter()
                .filter(|f| !part.specs.iter().any(|s| s.field.id == f.field.id))
                .filter(|f| !pending.iter().any(|(p, _)| p.id == f.field.id))
                .filter_map(|f| Some((f.field, f.default.clone(), library.field_view(f.field)?)))
                .collect();
            const ADD_FIELD: &str = "To rate another property (a tolerance, a power or voltage rating), add the field to the component under Fields → Add field.";
            if set.component.fields.is_empty() {
                kit::note(
                    ui,
                    "The component selects no fields yet. Add them under Fields → Add field to rate them here.",
                );
            } else if addable.is_empty() {
                kit::note(
                    ui,
                    &format!("Every field of the component has a rated value. {ADD_FIELD}"),
                );
            } else if part.specs.len() + pending.len() < MAX_RATED_SPECS {
                let mut chosen = None;
                kit::row(ui, "Add value", |ui| {
                    kit::dropdown_menu(ui, "add-spec", "Choose a field", |ui| {
                        for (field, default, view) in &addable {
                            if ui.selectable_label(false, &view.field.meta.name).clicked() {
                                let value =
                                    default.clone().or(view.field.definition.default.clone());
                                chosen = Some((
                                    *field,
                                    value,
                                    default_meaning(&view.field.definition.value_type),
                                ));
                            }
                        }
                    });
                });
                match chosen {
                    Some((field, Some(value), meaning)) => part.specs.push(RatedSpec {
                        field,
                        value,
                        meaning,
                    }),
                    // No default to start from: an empty row to type into.
                    Some((field, None, meaning)) => pending.push((field, meaning)),
                    None => {}
                }
                kit::note(ui, ADD_FIELD);
            }
        });
    }

    fn references_section(ui: &mut Ui, part: &mut PartDefinition) {
        let count = part.references.len();
        kit::section(
            ui,
            "part-references",
            "References",
            false,
            Some(count),
            |ui| {
                let mut remove = None;
                for (index, reference) in part.references.iter_mut().enumerate() {
                    if index > 0 {
                        kit::rule(ui);
                    }
                    ui.push_id(("reference", reference.id), |ui| {
                        let kinds = [
                            ReferenceKind::Datasheet,
                            ReferenceKind::ApplicationNote,
                            ReferenceKind::Other,
                        ]
                        .map(|k| (k, reference_kind_label(k).to_owned()));
                        kit::choice_row(ui, "Kind", &mut reference.kind, &kinds);
                        kit::checked_text_row(
                            ui,
                            "Title",
                            &mut reference.title,
                            "Datasheet",
                            false,
                        );
                        optional_row(ui, "Link", &mut reference.url, "https://…");
                        optional_row(ui, "Doc rev", &mut reference.document_revision, "Rev. A");
                        optional_row(ui, "Licence", &mut reference.licence, "None stated");
                        let mut asset = reference.asset.clone().unwrap_or_default();
                        let invalid = !asset.is_empty()
                            && (asset.len() != 64 || !asset.bytes().all(|b| b.is_ascii_hexdigit()));
                        if kit::checked_text_row(
                            ui,
                            "Copy",
                            &mut asset,
                            "No attached copy",
                            invalid,
                        )
                        .changed()
                        {
                            let asset = asset.trim().to_ascii_lowercase();
                            reference.asset = (!asset.is_empty()).then_some(asset);
                        }
                        if invalid {
                            kit::status(
                                ui,
                                "An attached copy is named by 64 hex digits.",
                                Tone::Error,
                            );
                        }
                        if reference.url.is_none() && reference.asset.is_some() {
                            kit::note(ui, "No link: the attached copy is kept.");
                        }
                        kit::actions(ui, |ui| {
                            if ui_kit::button(ui, "Remove", Button::Small).clicked() {
                                remove = Some(index);
                            }
                        });
                    });
                }
                if let Some(index) = remove
                    && index < part.references.len()
                {
                    part.references.remove(index);
                }
                kit::actions(ui, |ui| {
                    let enabled = part.references.len() < MAX_REFERENCES;
                    if ui_kit::button(ui, "Add reference", Button::Enabled(enabled))
                        .on_hover_text(
                            "A datasheet or application note: a link, an attached copy or both",
                        )
                        .clicked()
                        && enabled
                        && let Some(id) = allocate(part)
                    {
                        part.references.push(Reference {
                            id,
                            kind: ReferenceKind::Datasheet,
                            title: "Datasheet".into(),
                            url: None,
                            document_revision: None,
                            asset: None,
                            licence: None,
                            provenance: Provenance::default(),
                        });
                    }
                });
            },
        );
    }

    fn offers_section(ui: &mut Ui, part: &mut PartDefinition) {
        let count = part.offers.len();
        kit::section(
            ui,
            "part-offers",
            "Distributor offers",
            false,
            Some(count),
            |ui| {
                let mut remove = None;
                for (index, offer) in part.offers.iter_mut().enumerate() {
                    if index > 0 {
                        kit::rule(ui);
                    }
                    ui.push_id(("offer", offer.id), |ui| {
                        kit::checked_text_row(
                            ui,
                            "Distributor",
                            &mut offer.distributor,
                            "Distributor",
                            false,
                        );
                        kit::checked_text_row(ui, "SKU", &mut offer.sku, "Order number", false);
                        optional_row(ui, "Link", &mut offer.url, "https://…");
                        optional_row(ui, "Packaging", &mut offer.packaging, "Cut tape");
                        kit::actions(ui, |ui| {
                            if ui_kit::button(ui, "Remove", Button::Small).clicked() {
                                remove = Some(index);
                            }
                        });
                    });
                }
                if let Some(index) = remove
                    && index < part.offers.len()
                {
                    part.offers.remove(index);
                }
                kit::note(ui, "Identity only: no price or stock is stored.");
                kit::actions(ui, |ui| {
                    let enabled = part.offers.len() < MAX_OFFERS;
                    if ui_kit::button(ui, "Add offer", Button::Enabled(enabled))
                        .on_hover_text("A distributor and its order number for this Part")
                        .clicked()
                        && enabled
                        && let Some(id) = allocate(part)
                    {
                        part.offers.push(DistributorOffer {
                            id,
                            distributor: String::new(),
                            sku: String::new(),
                            url: None,
                            packaging: None,
                        });
                    }
                });
            },
        );
    }

    fn provenance_section(ui: &mut Ui, part: &mut PartDefinition) {
        kit::section(
            ui,
            "part-provenance",
            "Part provenance",
            false,
            None,
            |ui| {
                let mut options = vec![
                    (Basis::Authored, "Authored".to_owned()),
                    (Basis::Imported, "Imported".to_owned()),
                ];
                options.extend(part.references.iter().map(|r| {
                    (
                        Basis::FromReference(r.id),
                        format!(
                            "From {}",
                            if r.title.is_empty() {
                                "reference"
                            } else {
                                &r.title
                            }
                        ),
                    )
                }));
                kit::choice_row(ui, "Basis", &mut part.provenance.basis, &options);
                optional_row(ui, "Licence", &mut part.provenance.licence, "None stated");
                if let Some(origin) = &part.provenance.origin {
                    kit::value_row(
                        ui,
                        "Origin",
                        &format!(
                            "{} · {} {}",
                            origin.library, origin.lineage, origin.revision
                        ),
                    );
                }
            },
        );
    }
}

impl super::PartEditor {
    /// The Parts section of the Properties card. `pending` is the open
    /// card's operation this frame; a Part's edit or the cascade publish
    /// is sent only when the card sent nothing.
    pub(super) fn part_section(
        &mut self,
        ui: &mut Ui,
        engine: &Engine,
        pending: Option<Operation>,
    ) -> Option<Operation> {
        let blocked = self
            .listed_findings(engine)
            .into_iter()
            .find(|f| f.blocking)
            .map(|f| f.text);
        let mut operation = pending;
        if let Some(set) = &self.draft {
            match self.parts.section(ui, engine, set, blocked.as_deref()) {
                Some(PartsAction::Apply(op)) => operation = operation.or(Some(*op)),
                Some(PartsAction::PublishCascade) => {
                    operation = operation.or_else(|| self.publish(engine));
                }
                None => {}
            }
        }
        operation
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::part_editor::PartEditor;

    fn ready() -> pcbcraft_core::Result<(egui::Context, Engine, PartEditor)> {
        let ctx = egui::Context::default();
        let engine = Engine::demo()?;
        let mut editor = PartEditor::default();
        editor.commit(&ctx, &engine);
        Ok((ctx, engine, editor))
    }

    fn apply(engine: &mut Engine, operation: Option<Operation>) -> pcbcraft_core::Result<()> {
        if let Some(operation) = operation {
            engine.apply(engine.revision(), operation)?;
        }
        Ok(())
    }

    #[test]
    fn meanings_fit_the_value_type() {
        let tolerance = ValueType::Tolerance {
            basis: fields::ToleranceBasis::Relative,
        };
        assert_eq!(
            meanings(&tolerance).iter().map(|m| m.0).collect::<Vec<_>>(),
            [SpecMeaning::MaximumTolerance]
        );
        assert_eq!(default_meaning(&tolerance), SpecMeaning::MaximumTolerance);
        assert_eq!(
            meanings(&ValueType::Boolean)
                .iter()
                .map(|m| m.0)
                .collect::<Vec<_>>(),
            [SpecMeaning::ExactNominal]
        );
    }

    /// New Part, typed identity and a rated value, then Publish Part on a
    /// published component: each step is one public command, and the Part
    /// publishes pinning the published component and its binding.
    #[test]
    fn a_part_is_authored_and_published_through_library_commands()
    -> Result<(), Box<dyn std::error::Error>> {
        let (ctx, mut engine, mut editor) = ready()?;
        let set = editor.draft.clone().ok_or("no working set")?;
        let pins = Pins::of(&set, &engine).ok_or("no pins")?;
        assert!(!pins.cascade, "the demo component is published");
        let new = Operation::Library(LibraryCommand::NewDraft {
            meta: DefinitionMeta::default(),
            definition: Definition::Part(Box::new(new_part(&set, &pins))),
        });
        editor.parts.creating = true;
        editor.parts.awaiting = true;
        apply(&mut engine, Some(new))?;
        assert!(
            editor.commit(&ctx, &engine).is_none(),
            "creating is not an edit"
        );
        let open = editor
            .parts
            .draft
            .as_ref()
            .ok_or("the new draft is not open")?;
        assert_eq!(open.part.component, pins.component);
        assert_eq!(open.part.binding, pins.binding);
        // Type identity: one edit, sent once.
        if let Some(edit) = &mut editor.parts.draft {
            edit.part.manufacturer = "ExampleCo".into();
            edit.part.mpn = "EX-R0603-10K0-F".into();
            edit.meta.name = edit.part.mpn.clone();
        }
        let operation = editor.commit(&ctx, &engine);
        assert!(matches!(
            &operation,
            Some(Operation::Library(LibraryCommand::Edit { edits })) if edits.len() == 1
        ));
        assert!(editor.commit(&ctx, &engine).is_none(), "sent once");
        apply(&mut engine, operation)?;
        editor.commit(&ctx, &engine);
        let operation = editor
            .parts
            .publish_alone(&engine, set.ids.component, &pins);
        assert!(matches!(
            &operation,
            Some(Operation::Library(LibraryCommand::Publish { drafts })) if drafts.len() == 1
        ));
        apply(&mut engine, operation)?;
        let parts = engine.project().library.latest_parts();
        let [part] = parts.as_slice() else {
            return Err("one Part is published".into());
        };
        assert_eq!(part.definition.mpn, "EX-R0603-10K0-F");
        assert_eq!(part.definition.component, pins.component);
        assert_eq!(part.meta.name, "EX-R0603-10K0-F");
        Ok(())
    }

    /// A blank new Part's findings wait until an identity is typed: the
    /// Component workspace's top list leaves it out, then includes it.
    #[test]
    fn a_blank_new_part_is_left_out_of_the_top_findings() -> Result<(), Box<dyn std::error::Error>>
    {
        let (_ctx, mut engine, editor) = ready()?;
        let set = editor.draft.clone().ok_or("no working set")?;
        let pins = Pins::of(&set, &engine).ok_or("no pins")?;
        apply(
            &mut engine,
            Some(Operation::Library(LibraryCommand::NewDraft {
                meta: DefinitionMeta::default(),
                definition: Definition::Part(Box::new(new_part(&set, &pins))),
            })),
        )?;
        let component = set.ids.component;
        let all = PartProps::draft_ids(&engine, component);
        assert_eq!(all.len(), 1);
        assert!(PartProps::typed_draft_ids(&engine, component).is_empty());
        let mut part = part_drafts(engine.drafts(), component)
            .pop()
            .ok_or("the draft")?;
        part.part.mpn = "EX-R0603-10K0-F".into();
        apply(
            &mut engine,
            Some(Operation::Library(LibraryCommand::Edit {
                edits: vec![DraftEdit {
                    id: part.id,
                    meta: part.meta,
                    definition: Definition::Part(Box::new(part.part)),
                }],
            })),
        )?;
        assert_eq!(PartProps::typed_draft_ids(&engine, component), all);
        Ok(())
    }

    /// With unpublished component edits, Publish sends the cascade: the
    /// Part drafts retarget to the pending revisions, then one
    /// `library.publish` publishes footprint → binding → component → Part.
    #[test]
    fn the_cascade_publishes_component_and_part_in_one_set()
    -> Result<(), Box<dyn std::error::Error>> {
        let (ctx, mut engine, mut editor) = ready()?;
        let set = editor.draft.clone().ok_or("no working set")?;
        let published = Pins::of(&set, &engine).ok_or("no pins")?;
        let mut part = new_part(&set, &published);
        part.manufacturer = "ExampleCo".into();
        part.mpn = "EX-R0603-10K0-F".into();
        apply(
            &mut engine,
            Some(Operation::Library(LibraryCommand::NewDraft {
                meta: DefinitionMeta {
                    name: part.mpn.clone(),
                    ..Default::default()
                },
                definition: Definition::Part(Box::new(part)),
            })),
        )?;
        editor.commit(&ctx, &engine);
        // A footprint edit makes the component's cascade unpublished.
        if let Some(set) = &mut editor.draft {
            set.footprint.pads[0].position = [-1.5, 0.0];
        }
        let operation = editor.commit(&ctx, &engine);
        apply(&mut engine, operation)?;
        editor.commit(&ctx, &engine);
        let set = editor.draft.clone().ok_or("no working set")?;
        let pending = Pins::of(&set, &engine).ok_or("no pins")?;
        assert!(pending.cascade);
        assert_eq!(pending.component.revision.0, 2);
        // Publish: first the retargeting edit, then the publish set.
        let operation = editor.publish(&engine);
        let Some(Operation::Library(LibraryCommand::Edit { edits })) = &operation else {
            return Err(format!("expected a retargeting edit, got {operation:?}").into());
        };
        assert!(edits.iter().any(|e| matches!(
            &e.definition,
            Definition::Part(p) if p.component == pending.component && p.binding == pending.binding
        )));
        apply(&mut engine, operation)?;
        let operation = editor.commit(&ctx, &engine);
        let Some(Operation::Library(LibraryCommand::Publish { drafts })) = &operation else {
            return Err(format!("expected the publish set, got {operation:?}").into());
        };
        assert_eq!(drafts.len(), 6, "five drafts and the Part");
        let before = engine.revision();
        apply(&mut engine, operation)?;
        assert_eq!(engine.revision(), before + 1, "one undo step");
        let library = &engine.project().library;
        let parts = library.latest_parts();
        let [part] = parts.as_slice() else {
            return Err("one Part is published".into());
        };
        assert_eq!(part.definition.component, pending.component);
        assert!(library.component(pending.component).is_some());
        Ok(())
    }

    /// Unfinished edits of the Part and of the working set travel in one
    /// `library.edit`, so the reload after it discards neither.
    #[test]
    fn part_and_component_edits_commit_together() -> Result<(), Box<dyn std::error::Error>> {
        let (ctx, mut engine, mut editor) = ready()?;
        let set = editor.draft.clone().ok_or("no working set")?;
        let pins = Pins::of(&set, &engine).ok_or("no pins")?;
        editor.parts.creating = true;
        editor.parts.awaiting = true;
        apply(
            &mut engine,
            Some(Operation::Library(LibraryCommand::NewDraft {
                meta: DefinitionMeta::default(),
                definition: Definition::Part(Box::new(new_part(&set, &pins))),
            })),
        )?;
        editor.commit(&ctx, &engine);
        let id = editor.parts.open.ok_or("no open Part")?;
        if let Some(edit) = &mut editor.parts.draft {
            edit.part.mpn = "EX-R0603-10K0-F".into();
        }
        if let Some(set) = &mut editor.draft {
            set.metas.component.name = "Chip resistor".into();
        }
        let operation = editor.commit(&ctx, &engine);
        let Some(Operation::Library(LibraryCommand::Edit { edits })) = &operation else {
            return Err(format!("expected one edit, got {operation:?}").into());
        };
        assert_eq!(edits.len(), 6, "five drafts and the Part");
        apply(&mut engine, operation)?;
        editor.commit(&ctx, &engine);
        assert!(matches!(
            engine.drafts().get(id).map(|d| &d.definition),
            Some(Definition::Part(p)) if p.mpn == "EX-R0603-10K0-F"
        ));
        assert_eq!(
            editor
                .draft
                .as_ref()
                .map(|s| s.metas.component.name.as_str()),
            Some("Chip resistor")
        );
        assert_eq!(
            editor.parts.draft.as_ref().map(|d| d.part.mpn.as_str()),
            Some("EX-R0603-10K0-F")
        );
        Ok(())
    }

    /// B-j: while the component drafts it would publish are invalid,
    /// "Publish with component" is disabled with the reason on hover, and a
    /// click sends nothing; once they are valid it publishes.
    #[test]
    fn publish_with_component_is_disabled_while_the_component_is_invalid()
    -> Result<(), Box<dyn std::error::Error>> {
        let (ctx, mut engine, mut editor) = ready()?;
        solidcraft_ui_egui::fonts::setup_bundled_fonts(&ctx);
        let set = editor.draft.clone().ok_or("no working set")?;
        let pins = Pins::of(&set, &engine).ok_or("no pins")?;
        let mut part = new_part(&set, &pins);
        part.manufacturer = "ExampleCo".into();
        part.mpn = "EX-R0603-10K0-F".into();
        apply(
            &mut engine,
            Some(Operation::Library(LibraryCommand::NewDraft {
                meta: DefinitionMeta {
                    name: part.mpn.clone(),
                    ..Default::default()
                },
                definition: Definition::Part(Box::new(part)),
            })),
        )?;
        editor.commit(&ctx, &engine);
        editor.parts.open = PartProps::draft_ids(&engine, set.ids.component)
            .first()
            .copied();
        // A designator with a digit: an unpublished, invalid component.
        if let Some(set) = &mut editor.draft {
            set.component.designator_prefix = "R1".into();
        }
        let operation = editor.commit(&ctx, &engine);
        apply(&mut engine, operation)?;
        editor.commit(&ctx, &engine);
        let label = "Publish with component";
        let click = |editor: &mut PartEditor, engine: &Engine| -> Result<_, String> {
            let shown = frame(editor, engine, &ctx, vec![]);
            let at = shown
                .iter()
                .find(|(t, _)| t == label)
                .map(|(_, p)| *p)
                .ok_or_else(|| format!("no {label}: {shown:?}"))?;
            let press = |pressed| egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            };
            let mut sent = None;
            for events in [
                vec![egui::Event::PointerMoved(at)],
                vec![press(true)],
                vec![press(false)],
            ] {
                let mut output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(360.0, 4000.0),
                        )),
                        events,
                        ..Default::default()
                    },
                    |ui| sent = sent.take().or(editor.inspector(ui, engine)),
                );
                output.textures_delta.clear();
            }
            Ok(sent)
        };
        assert!(
            click(&mut editor, &engine)?.is_none(),
            "an invalid component does not publish"
        );
        // Valid again: the same button publishes (first retargeting the Part).
        if let Some(set) = &mut editor.draft {
            set.component.designator_prefix = "RN".into();
        }
        let operation = editor.commit(&ctx, &engine);
        apply(&mut engine, operation)?;
        editor.commit(&ctx, &engine);
        let sent = click(&mut editor, &engine)?;
        assert!(
            matches!(sent, Some(Operation::Library(LibraryCommand::Edit { .. }))),
            "{sent:?}"
        );
        Ok(())
    }

    /// One inspector frame; returns the drawn texts and their centres.
    fn frame(
        editor: &mut PartEditor,
        engine: &Engine,
        ctx: &egui::Context,
        events: Vec<egui::Event>,
    ) -> Vec<(String, egui::Pos2)> {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(360.0, 4000.0),
                )),
                events,
                ..Default::default()
            },
            |ui| {
                editor.inspector(ui, engine);
            },
        );
        output.textures_delta.clear();
        fn texts(shape: &egui::Shape, out: &mut Vec<(String, egui::Pos2)>) {
            match shape {
                egui::Shape::Text(text) => out.push((
                    text.galley.job.text.clone(),
                    text.pos + text.galley.size() * 0.5,
                )),
                egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| texts(s, out)),
                _ => {}
            }
        }
        let mut out = Vec::new();
        for shape in &output.shapes {
            texts(&shape.shape, &mut out);
        }
        out
    }

    fn key(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    /// Click the text `label`, replace the field's text with `typed`, then
    /// leave the field.
    fn replace_text(
        editor: &mut PartEditor,
        engine: &Engine,
        ctx: &egui::Context,
        label: &str,
        typed: &str,
    ) -> Result<Vec<(String, egui::Pos2)>, String> {
        let shown = frame(editor, engine, ctx, vec![]);
        let point = shown
            .iter()
            .find(|(text, _)| text == label)
            .map(|(_, p)| *p)
            .ok_or_else(|| format!("missing {label}: {shown:?}"))?;
        let press = |pressed| egui::Event::PointerButton {
            pos: point,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        frame(
            editor,
            engine,
            ctx,
            vec![egui::Event::PointerMoved(point), press(true)],
        );
        frame(editor, engine, ctx, vec![press(false)]);
        frame(
            editor,
            engine,
            ctx,
            vec![key(egui::Key::A, egui::Modifiers::COMMAND)],
        );
        let typed = frame(editor, engine, ctx, vec![egui::Event::Text(typed.into())]);
        frame(
            editor,
            engine,
            ctx,
            vec![key(egui::Key::Escape, egui::Modifiers::NONE)],
        );
        Ok(typed)
    }

    fn rated(engine: &Engine, id: DefinitionId) -> Option<Vec<RatedSpec>> {
        match &engine.drafts().get(id)?.definition {
            Definition::Part(p) => Some(p.specs.clone()),
            _ => None,
        }
    }

    /// Typing into the rated value row commits a parsed value as one
    /// `library.edit`; invalid text shows the field's error and sends
    /// nothing.
    #[test]
    fn rated_values_are_typed_through_the_field() -> Result<(), Box<dyn std::error::Error>> {
        let (ctx, mut engine, mut editor) = ready()?;
        solidcraft_ui_egui::fonts::setup_bundled_fonts(&ctx);
        let set = editor.draft.clone().ok_or("no working set")?;
        let pins = Pins::of(&set, &engine).ok_or("no pins")?;
        let field = set.component.fields.first().ok_or("no field")?.field;
        let mut part = new_part(&set, &pins);
        part.specs.push(RatedSpec {
            field,
            value: fields::FieldValue::Quantity {
                si: 10_000.0,
                display_unit: None,
            },
            meaning: SpecMeaning::ExactNominal,
        });
        editor.parts.creating = true;
        editor.parts.awaiting = true;
        apply(
            &mut engine,
            Some(Operation::Library(LibraryCommand::NewDraft {
                meta: DefinitionMeta::default(),
                definition: Definition::Part(Box::new(part)),
            })),
        )?;
        editor.commit(&ctx, &engine);
        let id = editor.parts.open.ok_or("no open Part")?;
        let shown = frame(&mut editor, &engine, &ctx, vec![]);
        for label in [
            "PARTS",
            "RATED VALUES",
            "Maker",
            "MPN",
            "Resistance",
            "Exact value",
        ] {
            assert!(
                shown.iter().any(|(t, _)| t == label),
                "{label} is shown: {shown:?}"
            );
        }
        let current = shown
            .iter()
            .find(|(t, _)| t.starts_with("10"))
            .map(|(t, _)| t.clone())
            .ok_or("the rated value is shown")?;
        replace_text(&mut editor, &engine, &ctx, &current, "22k")?;
        let operation = editor.commit(&ctx, &engine).ok_or("no edit")?;
        assert!(editor.commit(&ctx, &engine).is_none(), "sent once");
        apply(&mut engine, Some(operation))?;
        let specs = rated(&engine, id).ok_or("no Part draft")?;
        assert!(matches!(
            specs.first().map(|s| &s.value),
            Some(fields::FieldValue::Quantity { si, .. }) if (si - 22_000.0).abs() < 1e-6
        ));
        editor.commit(&ctx, &engine);
        let shown = frame(&mut editor, &engine, &ctx, vec![]);
        let current = shown
            .iter()
            .find(|(t, _)| t.starts_with("22"))
            .map(|(t, _)| t.clone())
            .ok_or("the new rated value is shown")?;
        let typed = replace_text(&mut editor, &engine, &ctx, &current, "22 parsecs")?;
        assert!(typed.iter().any(|(t, _)| t == "22 parsecs"));
        assert!(
            editor.commit(&ctx, &engine).is_none(),
            "invalid text sends nothing"
        );
        assert_eq!(rated(&engine, id), Some(specs));
        Ok(())
    }
}
