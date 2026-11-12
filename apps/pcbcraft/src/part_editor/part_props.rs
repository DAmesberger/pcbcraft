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
use crate::kit;
use crate::kit::dialog::{DialogAction, DialogPanel};
use crate::refusal::Origin;
use egui::Ui;
use pcbcraft_core::diagnostics::ObjectRef;
use pcbcraft_core::fields::{self, MAX_RATED_SPECS, RatedSpec, SpecMeaning, ValueType};
use pcbcraft_core::issues::{IssueCounts, IssueScope};
use pcbcraft_core::library::{
    Basis, Definition, DefinitionMeta, DistributorOffer, DraftEdit, Drafts, FieldView, Library,
    MAX_OFFERS, MAX_REFERENCES, PartDefinition, Provenance, Reference, ReferenceKind,
};
use pcbcraft_core::{
    BindingRef, ComponentRef, DefinitionId, Engine, FieldRef, LibraryCommand, Operation, PartRef,
    Query, QueryResult, Revision, Stage,
};
use pcbcraft_ui::ui_kit::{self, Button};
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

    /// The list row: MPN · maker · state.
    fn label(&self) -> String {
        let (mpn, maker) = self
            .body()
            .map_or(("", ""), |p| (p.mpn.trim(), p.manufacturer.trim()));
        let mpn = if mpn.is_empty() { "New Part" } else { mpn };
        let state = match (&self.draft, &self.published) {
            (Some(_), None) => "draft \u{b7} new".to_owned(),
            (Some(_), Some((r, _))) if self.unpublished() => format!("{} \u{b7} draft", r.revision),
            (_, Some((r, _))) => r.revision.to_string(),
            (None, None) => String::new(),
        };
        [mpn, maker, &state]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" \u{b7} ")
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
    /// Issues blocking the primary, by revision, Part lineage and cascade.
    blockers: Option<(u64, DefinitionId, bool, Option<usize>)>,
    /// Fields added under Rated values that have no value typed yet. They
    /// join the draft once their text parses.
    pending: Vec<(FieldRef, SpecMeaning)>,
    /// The Part dialog holds the subject slot.
    dialog: bool,
    /// The dialog opened this frame: its body scrolls to the top.
    reset: bool,
    /// A publish the dialog sent, until it lands or is refused.
    publishing: Option<Publishing>,
    /// The revision the dialog published.
    done: Option<PartRef>,
    /// The engine's refusal of the dialog's last operation.
    refusal: Option<String>,
    /// The Discard confirmation is open.
    discarding: bool,
    /// Where the return link goes after publishing.
    back: Option<PartReturn>,
    /// The return link was followed; the app takes it.
    returning: Option<PartReturn>,
    /// The Parts list shows every row.
    show_all: bool,
    /// A New Part was requested from the schematic (Resolve parts): draft
    /// one as soon as the component's pins are known, with these values.
    prefill: Option<Vec<(FieldRef, fields::FieldValue)>>,
    /// Publish leaves the published Parts on the old component revision
    /// (Also publish N Parts… turned off; on by default).
    keep_parts: bool,
}

/// The published Parts a Publish of `set` carries to the component's new
/// revision (`library.publish_with_parts`), with the revision they move to.
pub fn republished(engine: &Engine, set: &WorkingSet) -> (Vec<PartRef>, Option<Revision>) {
    let parts = pcbcraft_core::commands::parts_to_republish(
        &engine.project().library,
        engine.drafts(),
        &set.ids.all(),
    );
    let pending = engine
        .drafts()
        .get(set.ids.component)
        .and_then(|d| d.pending());
    (parts, pending)
}

/// "Also publish 2 Parts for r3".
fn republish_label(count: usize, revision: Option<Revision>) -> String {
    let parts = if count == 1 { "Part" } else { "Parts" };
    match revision {
        Some(r) => format!("Also publish {count} {parts} for {r}"),
        None => format!("Also publish {count} {parts}"),
    }
}

/// Part drafts and published Parts of one component lineage, by lineage.
/// One Parts row: its label, shortened with "…" where the column is
/// narrow, with the whole label and what a click does on hover.
fn part_row(ui: &mut Ui, label: &str, selected: bool) -> egui::Response {
    let response = ui.selectable_label(selected, label);
    let room = response.rect.width() - 2.0 * ui.spacing().button_padding.x;
    let width = ui
        .painter()
        .layout_no_wrap(
            label.to_owned(),
            egui::TextStyle::Button.resolve(ui.style()),
            ui.visuals().text_color(),
        )
        .size()
        .x;
    if width > room + 0.5 {
        response.on_hover_text(format!("{label}\nOpen this Part"))
    } else {
        response.on_hover_text("Open this Part")
    }
}

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
        return Err("Enter a value".into());
    }
    fields::parse_value(text, &view.field.definition.value_type, view.kind_body()).map_err(|e| {
        crate::refusal::presentable(&e.to_string()).unwrap_or_else(|| "Enter a valid value".into())
    })
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

/// The Part dialog's panel id: its refusals are handed back to it.
fn dialog_id() -> egui::Id {
    egui::Id::unique("part-dialog")
}

const ICON: &str = "library";

/// The dialog body fills the slot; the slot caps it.
const BODY_ROWS: f32 = 32.0;

/// The dialog header: "Part · MPN", or "Part · New" before an MPN is typed.
fn title(mpn: &str) -> String {
    match mpn.trim() {
        "" => "Part \u{b7} New".to_owned(),
        mpn => format!("Part \u{b7} {mpn}"),
    }
}

/// A publish the dialog sent for `lineage`, while `before` was its latest
/// published revision.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Publishing {
    lineage: DefinitionId,
    before: Option<PartRef>,
}

/// Where the dialog's return link goes after a Part requested from the
/// schematic publishes ("Back to R1 ›").
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PartReturn {
    /// The instance to select again, when the request came from one.
    pub instance: Option<pcbcraft_core::Id>,
    /// Its designator; empty for the sheet.
    pub designator: String,
}

impl PartReturn {
    /// The return link's label.
    pub fn label(&self) -> String {
        if self.designator.is_empty() {
            "Back to the schematic \u{203a}".to_owned()
        } else {
            format!("Back to {} \u{203a}", self.designator)
        }
    }
}

/// An identity text's inline error: empty, or longer than core accepts.
fn identity_error(text: &str, empty: &'static str) -> Option<&'static str> {
    if text.trim().is_empty() {
        Some(empty)
    } else if text.len() > 256 {
        Some("At most 256 bytes")
    } else {
        None
    }
}

/// The draft's identity: Maker and MPN (marked inline once an identity is
/// typed), what it implements, its footprint and revision, and the pins it
/// publishes with when they differ.
fn identity_section(
    ui: &mut Ui,
    library: &Library,
    set: &WorkingSet,
    edit: &mut Edit,
    published: Option<PartRef>,
    unpublished: bool,
    pins: Option<&Pins>,
) {
    kit::section(ui, "part-identity", "Identity", true, None, |ui| {
        let typed = !edit.part.mpn.trim().is_empty() || !edit.part.manufacturer.trim().is_empty();
        let maker_error =
            identity_error(&edit.part.manufacturer, "Enter the manufacturer").filter(|_| typed);
        let mpn_error = identity_error(&edit.part.mpn, "Enter the MPN").filter(|_| typed);
        let maker = kit::checked_text_row(
            ui,
            "Maker",
            &mut edit.part.manufacturer,
            "Manufacturer",
            false,
        )
        .on_hover_text("The manufacturer's name, as text");
        if let Some(error) = maker_error {
            kit::field_error(ui, &maker, error, false);
        }
        let mpn = kit::checked_text_row(ui, "MPN", &mut edit.part.mpn, "Full MPN", false)
            .on_hover_text("The complete manufacturer part number, ordering suffixes included");
        if let Some(error) = mpn_error {
            kit::field_error(ui, &mpn, error, false);
        }
        if mpn.changed() {
            edit.meta.name = edit.part.mpn.trim().to_owned();
        }
        let component = library.component(edit.part.component).map_or_else(
            || {
                format!(
                    "{} \u{b7} {} \u{b7} pending",
                    set.metas.component.name, edit.part.component.revision
                )
            },
            |c| format!("{} \u{b7} {}", c.meta.name, edit.part.component.revision),
        );
        kit::value_row(ui, "Implements", &component);
        // The row names the footprint people chose, by its own name.
        let footprint_name = library
            .binding(edit.part.binding)
            .and_then(|b| b.definition.footprints.first().copied())
            .and_then(|f| library.footprint(f))
            .map(|f| f.meta.name.trim().to_owned())
            .filter(|name| !name.is_empty());
        let footprint = if let Some(name) = footprint_name {
            format!("{name} {}", edit.part.binding.revision)
        } else if library.binding(edit.part.binding).is_some() {
            pcbcraft_core::instances::binding_name(library, edit.part.binding)
        } else {
            format!("{} \u{b7} pending", edit.part.binding.revision)
        };
        kit::value_row(ui, "Footprint", &footprint);
        let next = |r: PartRef| {
            r.revision
                .next()
                .map_or_else(String::new, |n| n.to_string())
        };
        let revision = match published {
            None => "Draft \u{b7} new".to_owned(),
            Some(r) if unpublished => {
                format!(
                    "{} \u{b7} unpublished edits \u{2192} {}",
                    r.revision,
                    next(r)
                )
            }
            Some(r) => format!("{} \u{b7} published; edits create {}", r.revision, next(r)),
        };
        kit::value_row(ui, "Revision", &revision);
        if let Some(pins) = pins
            && (edit.part.component != pins.component || edit.part.binding != pins.binding)
        {
            kit::hint(
                ui,
                &format!(
                    "Publishing pins {} {}",
                    set.metas.component.name, pins.component.revision
                ),
                &format!(
                    "The Part is pinned to component {} and footprint binding {} when it publishes",
                    pins.component.revision, pins.binding.revision
                ),
            );
        }
    });
}

/// A published Part, read-only.
fn published_rows(ui: &mut Ui, library: &Library, r: PartRef, part: &PartDefinition) {
    kit::value_row(ui, "Maker", &part.manufacturer);
    kit::value_row(ui, "MPN", &part.mpn);
    let component = library.component(part.component).map_or_else(
        || part.component.revision.to_string(),
        |c| format!("{} \u{b7} {}", c.meta.name, part.component.revision),
    );
    kit::value_row(ui, "Implements", &component);
    kit::value_row(ui, "Revision", &format!("{} \u{b7} published", r.revision));
    for spec in &part.specs {
        kit::value_row(
            ui,
            &spec_label(library, spec),
            &format!(
                "{} \u{b7} {}",
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
                self.dialog = true;
                self.reset = true;
                self.done = None;
                self.refusal = None;
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

    /// The published Parts the component's Publish also publishes for its
    /// new revision: none while Also publish N Parts… is off.
    pub fn republish_ids(&self, engine: &Engine, set: &WorkingSet) -> Vec<DefinitionId> {
        if self.keep_parts {
            return Vec::new();
        }
        republished(engine, set).0.iter().map(|p| p.id).collect()
    }

    /// The Part draft IDs of `component`, for the cascade's publish set.
    pub fn draft_ids(engine: &Engine, component: DefinitionId) -> Vec<DefinitionId> {
        part_drafts(engine.drafts(), component)
            .into_iter()
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
            close: false,
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
                close: false,
            }));
        }
        self.publish_next = true;
        Some(Operation::Library(LibraryCommand::Edit { edits }))
    }

    /// The Parts list of the Details card: one row per Part lineage
    /// (MPN · maker · state); opening one opens the Part dialog, and New
    /// Part… drafts one and opens it.
    fn list(&mut self, ui: &mut Ui, engine: &Engine, set: &WorkingSet) -> Option<PartsAction> {
        let component = set.ids.component;
        self.sync(engine, component);
        let pins = Pins::of(set, engine);
        let listed = listed(engine, component);
        let mut action = None;
        let mut opened = None;
        kit::section(ui, "parts", "Parts", true, Some(listed.len()), |ui| {
            if listed.is_empty() {
                kit::hint(
                    ui,
                    "No manufacturer part yet",
                    "A Part is one exact orderable item: manufacturer, MPN and rated \
                     values, pinned to this component",
                );
            }
            let rows = if self.show_all {
                listed.len()
            } else {
                kit::LIST_MAX_ROWS
            };
            let shown = kit::list_frame(ui, listed.len(), rows, |ui, index| {
                if let Some(item) = listed.get(index)
                    && part_row(ui, &item.label(), self.dialog && self.open == Some(item.id))
                        .clicked()
                {
                    opened = Some(item.id);
                }
            });
            if shown.show_all {
                self.show_all = true;
            }
            // Night finding N4: Publish can carry the published Parts to
            // the new revision in the same step.
            let (parts, pending) = republished(engine, set);
            if set.unpublished(engine) && !parts.is_empty() {
                let mut on = !self.keep_parts;
                if kit::check_row(ui, &republish_label(parts.len(), pending), &mut on) {
                    self.keep_parts = !on;
                }
                let library = &engine.project().library;
                let names: Vec<&str> = parts
                    .iter()
                    .filter_map(|r| library.part(*r))
                    .map(|p| p.definition.mpn.as_str())
                    .collect();
                kit::hint(
                    ui,
                    &names.join(", "),
                    "Publish pins these Parts to the new component revision in the same \
                     step; turned off, they stay on their revision",
                );
            }
            kit::actions(ui, |ui| {
                let enabled = pins.is_some();
                let response = ui_kit::button(ui, "New Part\u{2026}", Button::Enabled(enabled))
                    .on_hover_text("Draft a manufacturer part of this component");
                let clicked = if enabled {
                    response.clicked()
                } else {
                    response
                        .on_disabled_hover_text("Publish the component first")
                        .clicked()
                };
                if clicked
                    && enabled
                    && let Some(pins) = &pins
                {
                    action = Some(self.create(set, pins, &engine.project().library, &[]));
                }
            });
        });
        if let Some(id) = opened {
            self.open_dialog(id);
            self.sync(engine, component);
        }
        action.map(|op| PartsAction::Apply(Box::new(op)))
    }

    /// `library.new_draft` of a new Part with `values` as its rated values;
    /// the next sync opens it in the dialog.
    fn create(
        &mut self,
        set: &WorkingSet,
        pins: &Pins,
        library: &Library,
        values: &[(FieldRef, fields::FieldValue)],
    ) -> Operation {
        self.creating = true;
        self.awaiting = true;
        Operation::Library(LibraryCommand::NewDraft {
            meta: DefinitionMeta::default(),
            definition: Definition::Part(Box::new(prefilled(set, pins, library, values))),
        })
    }

    /// The New Part requested from the schematic, once the component's pins
    /// are known: it opens in the dialog directly.
    fn take_prefill(&mut self, engine: &Engine, set: &WorkingSet) -> Option<Operation> {
        if self.prefill.is_none() || self.awaiting {
            return None;
        }
        let pins = Pins::of(set, engine)?;
        let values = self.prefill.take()?;
        Some(self.create(set, &pins, &engine.project().library, &values))
    }

    /// Open the dialog on the Part lineage `id`.
    fn open_dialog(&mut self, id: DefinitionId) {
        if self.open != Some(id) {
            self.open = Some(id);
            self.source = None;
        }
        self.dialog = true;
        self.reset = true;
        self.done = None;
        self.refusal = None;
    }

    /// Close the dialog. The draft keeps every committed edit.
    fn close(&mut self) {
        self.dialog = false;
        self.done = None;
        self.refusal = None;
        self.publishing = None;
        self.discarding = false;
        self.back = None;
    }

    /// Whether the Part dialog holds the subject slot.
    pub fn is_open(&self) -> bool {
        self.dialog
    }

    /// A publish sent from the dialog has landed: a newer revision is
    /// published and the draft equals it. The dialog then says so, and a toast
    /// offers Undo.
    fn settle(&mut self, ctx: &egui::Context, listed: &[Listed]) {
        let Some(publishing) = self.publishing else {
            return;
        };
        let Some(item) = listed.iter().find(|l| l.id == publishing.lineage) else {
            self.publishing = None;
            return;
        };
        // A publish keeps the draft, equal to the new revision.
        if let Some((r, part)) = &item.published
            && Some(*r) != publishing.before
            && !item.unpublished()
        {
            self.publishing = None;
            self.done = Some(*r);
            kit::toast::post(
                ctx,
                kit::severity::Level::Ok,
                format!("Published {} {}", part.mpn, r.revision),
                vec![kit::toast::ToastAction::Undo],
            );
        }
    }

    /// Issues that block publishing what the primary would publish: the
    /// Part draft and, with the cascade, the component's drafts. From the
    /// one `project.issues` query, once per revision; `None` when the checks
    /// could not run, which never reads as "no issues".
    fn blockers(&mut self, engine: &Engine, set: &WorkingSet, cascade: bool) -> Option<usize> {
        let id = self.draft.as_ref()?.id;
        if let Some((revision, open, with_cascade, count)) = self.blockers
            && revision == engine.revision()
            && open == id
            && with_cascade == cascade
        {
            return count;
        }
        let mut objects = vec![ObjectRef::Definition { id }];
        if cascade {
            objects.extend(
                set.ids
                    .all()
                    .into_iter()
                    .map(|id| ObjectRef::Definition { id }),
            );
        }
        let count = match engine.query(Query::Issues {
            scope: IssueScope::Selection { objects },
        }) {
            Ok(QueryResult::Issues(report)) => Some(
                report
                    .rows
                    .iter()
                    .filter(|r| r.blocks() == Some(Stage::Publish))
                    .count(),
            ),
            _ => None,
        };
        self.blockers = Some((engine.revision(), id, cascade, count));
        count
    }

    /// The Part dialog docked in the subject slot (docs/sidebar-ia.md,
    /// "Dialogs and reviews"): a draft with Publish Part, or Publish with
    /// component while the component has unpublished edits, gated on the
    /// issues that block it; a published Part with Edit Part; after a
    /// publish, the return link.
    fn dialog(&mut self, ui: &mut Ui, engine: &Engine, set: &WorkingSet) -> Option<PartsAction> {
        let ctx = ui.ctx().clone();
        let component = set.ids.component;
        self.sync(engine, component);
        if let Some(refusal) = crate::refusal::claim(&ctx, Origin::Dialog(dialog_id())) {
            self.refusal = Some(refusal);
            self.publishing = None;
        }
        let listed = listed(engine, component);
        self.settle(&ctx, &listed);
        let Some(item) = listed.iter().find(|l| Some(l.id) == self.open) else {
            self.close();
            return None;
        };
        let reset = std::mem::take(&mut self.reset);
        let library = &engine.project().library;
        if let Some(r) = self.done
            && let Some((_, part)) = &item.published
        {
            let back = self.back.as_ref().map(PartReturn::label);
            let heading = title(&part.mpn);
            let mut panel = DialogPanel::new(
                dialog_id(),
                ICON,
                &heading,
                back.as_deref().unwrap_or("Done"),
            );
            panel.kind = "Published";
            panel.body_rows = BODY_ROWS;
            panel.reset_scroll = reset;
            let action = panel.show(ui, |ui| {
                kit::summary_row(ui, &format!("Published {} {}", part.mpn, r.revision), None);
                published_rows(ui, library, r, part);
            });
            match action {
                Some(DialogAction::Primary) => {
                    self.returning = self.back.take();
                    self.close();
                }
                Some(DialogAction::Cancel) => self.close(),
                None => {}
            }
            return None;
        }
        if self.draft.is_none() {
            let (r, part) = item.published.as_ref()?;
            let kind = r.revision.to_string();
            let heading = title(&part.mpn);
            let mut panel = DialogPanel::new(dialog_id(), ICON, &heading, "Edit Part");
            panel.kind = &kind;
            panel.body_rows = BODY_ROWS;
            panel.reset_scroll = reset;
            panel.refusal = self.refusal.as_deref();
            let action = panel.show(ui, |ui| {
                published_rows(ui, library, *r, part);
                kit::hint(
                    ui,
                    "Editing drafts a new revision of this Part",
                    "Placed instances keep the published revision until you choose the new one",
                );
            });
            return match action {
                Some(DialogAction::Primary) => {
                    self.awaiting = true;
                    self.refusal = None;
                    Some(PartsAction::Apply(Box::new(Operation::Library(
                        LibraryCommand::DraftFrom {
                            revision: r.erased(),
                        },
                    ))))
                }
                Some(DialogAction::Cancel) => {
                    self.close();
                    None
                }
                None => None,
            };
        }
        self.draft_dialog(ui, engine, set, item, reset)
    }

    /// The dialog on a Part draft.
    fn draft_dialog(
        &mut self,
        ui: &mut Ui,
        engine: &Engine,
        set: &WorkingSet,
        item: &Listed,
        reset: bool,
    ) -> Option<PartsAction> {
        let ctx = ui.ctx().clone();
        let library = &engine.project().library;
        let pins = Pins::of(set, engine);
        let cascade = pins.is_some_and(|p| p.cascade);
        let blockers = self.blockers(engine, set, cascade);
        let Some(edit) = &mut self.draft else {
            return None;
        };
        let published = item.published.as_ref().map(|(r, _)| *r);
        let unpublished = item
            .published
            .as_ref()
            .is_none_or(|(_, latest)| *latest != edit.part);
        let why = match (&pins, blockers) {
            (None, _) => Some("Publish the component first".to_owned()),
            (_, None) => Some("The publish checks could not run".to_owned()),
            (_, Some(0)) if !unpublished => Some("No changes to publish".to_owned()),
            (_, Some(0)) => None,
            (_, Some(1)) => Some("1 issue blocks publishing".to_owned()),
            (_, Some(n)) => Some(format!("{n} issues block publishing")),
        };
        let primary = if cascade {
            "Publish with component"
        } else {
            "Publish Part"
        };
        let kind = if published.is_some() { "Draft" } else { "New" };
        let heading = title(&edit.part.mpn);
        let mut panel = DialogPanel::new(dialog_id(), ICON, &heading, primary);
        panel.kind = kind;
        panel.body_rows = BODY_ROWS;
        panel.reset_scroll = reset;
        panel.primary_disabled = why.as_deref();
        panel.refusal = self.refusal.as_deref();
        let counts = IssueCounts {
            error: blockers.unwrap_or(0),
            ..IssueCounts::default()
        };
        let mut discard = false;
        let mut show_issues = false;
        let specs = &mut self.specs;
        let pending = &mut self.pending;
        let action = panel.show(ui, |ui| {
            identity_section(
                ui,
                library,
                set,
                edit,
                published,
                unpublished,
                pins.as_ref(),
            );
            Self::specs_section(ui, library, set, edit, specs, pending);
            Self::references_section(ui, &mut edit.part);
            Self::offers_section(ui, &mut edit.part);
            Self::provenance_section(ui, &mut edit.part);
            let line = kit::issue_line(ui, &counts, "block publishing");
            show_issues = line.clicked() && counts.error > 0;
            let summary = match published {
                Some(r) => format!("{} stays published", r.revision),
                None => String::new(),
            };
            discard = kit::summary_row(ui, &summary, Some("Discard draft\u{2026}"));
        });
        if show_issues {
            // The column's Issues card (app/chrome.rs) opens on this flag.
            ctx.data_mut(|d| d.insert_temp(egui::Id::unique("issues-expanded"), true));
        }
        if discard {
            self.discarding = true;
        }
        let id = edit.id;
        if self.discarding {
            let text = match published {
                Some(r) => format!(
                    "Drop this draft? The published {} stays as it is.",
                    r.revision
                ),
                None => "Drop this new Part draft?".to_owned(),
            };
            match kit::dialog::confirm(
                &ctx,
                dialog_id().with("discard"),
                "Discard Part draft",
                &text,
                "Discard",
            ) {
                Some(true) => {
                    self.discarding = false;
                    self.awaiting = true;
                    self.refusal = None;
                    return Some(PartsAction::Apply(Box::new(Operation::Library(
                        LibraryCommand::DiscardDraft { id },
                    ))));
                }
                Some(false) => self.discarding = false,
                None => {}
            }
        }
        match action {
            Some(DialogAction::Cancel) => {
                self.close();
                None
            }
            Some(DialogAction::Primary) if why.is_none() => {
                let pins = pins?;
                self.refusal = None;
                self.publishing = Some(Publishing {
                    lineage: id,
                    before: published,
                });
                if pins.cascade {
                    return Some(PartsAction::PublishCascade);
                }
                self.publish_alone(engine, set.ids.component, &pins)
                    .map(|op| PartsAction::Apply(Box::new(op)))
            }
            _ => None,
        }
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
                        // Issues lists the missing field revision.
                        kit::value_row(ui, "Field", "Not in the project library");
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
                        kit::field_error(ui, &response, error, true);
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
                            kit::field_error(ui, &response, &error, true);
                        }
                        _ => {
                            kit::hint(ui, "Not in the draft until a value is typed", "");
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
            const ADD_FIELD: &str = "To rate another property (a tolerance, a power or voltage rating), add the field to the component under Fields \u{2192} Add field";
            if set.component.fields.is_empty() {
                kit::hint(
                    ui,
                    "The component has no fields to rate yet",
                    "Add them under Fields \u{2192} Add field to rate them here",
                );
            } else if addable.is_empty() {
                kit::hint(ui, "Every field of the component is rated", ADD_FIELD);
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
                kit::hint(ui, "Rate another property by adding a field", ADD_FIELD);
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
                        let copy = kit::checked_text_row(
                            ui,
                            "Copy",
                            &mut asset,
                            "No attached copy",
                            invalid,
                        );
                        if copy.changed() {
                            let asset = asset.trim().to_ascii_lowercase();
                            reference.asset = (!asset.is_empty()).then_some(asset);
                        }
                        if invalid {
                            kit::field_error(
                                ui,
                                &copy,
                                "Enter the copy's 64 hex digits, or leave it empty",
                                false,
                            );
                        }
                        if reference.url.is_none() && reference.asset.is_some() {
                            kit::hint(ui, "No link: the attached copy is kept", "");
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
                kit::hint(ui, "Identity only: no price or stock is stored", "");
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
    /// The Parts list of the Properties card. `pending` is the open card's
    /// operation this frame; a list action is sent only when the card sent
    /// nothing.
    pub(super) fn part_section(
        &mut self,
        ui: &mut Ui,
        engine: &Engine,
        pending: Option<Operation>,
    ) -> Option<Operation> {
        let mut operation = pending;
        if let Some(set) = &self.draft
            && let Some(PartsAction::Apply(op)) = self.parts.list(ui, engine, set)
        {
            operation = operation.or(Some(*op));
        }
        operation
    }

    /// The Part dialog, when it holds the subject slot: `Some` with the
    /// operation it sends this frame, `None` when the card shows instead.
    /// A New Part requested from the schematic is drafted here, so it opens
    /// in the dialog on whatever tab is showing.
    pub(super) fn part_dialog(
        &mut self,
        ui: &mut Ui,
        engine: &Engine,
    ) -> Option<Option<Operation>> {
        let set = self.draft.as_ref()?;
        if let Some(operation) = self.parts.take_prefill(engine, set) {
            return Some(Some(operation));
        }
        if !self.parts.is_open() {
            return None;
        }
        let action = self.parts.dialog(ui, engine, set);
        let operation = match action {
            Some(PartsAction::Apply(op)) => Some(*op),
            Some(PartsAction::PublishCascade) => self.publish(engine),
            None => None,
        };
        // The dialog's own operations, and the edit or publish that commit
        // sends for it later this frame, are refused back to its row.
        let deferred =
            self.publish_next || self.parts.publish_next || self.parts.draft != self.parts.baseline;
        if self.parts.is_open() && (operation.is_some() || deferred) {
            crate::refusal::mark(ui.ctx(), Origin::Dialog(dialog_id()));
        }
        Some(operation)
    }

    /// Where the Part dialog's return link goes once the requested Part
    /// publishes; `None` for no link.
    pub fn part_return_to(&mut self, back: Option<PartReturn>) {
        self.parts.back = back;
    }

    /// The return link the Part dialog followed, once.
    pub fn take_part_return(&mut self) -> Option<PartReturn> {
        self.parts.returning.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::part_editor::PartEditor;
    use crate::schematic_editor::pt;

    fn ready() -> pcbcraft_core::Result<(egui::Context, Engine, PartEditor)> {
        let ctx = egui::Context::default();
        let engine = Engine::demo()?;
        let mut editor = PartEditor::default();
        editor.open_first(&engine);
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
            Some(Operation::Library(LibraryCommand::Publish { drafts, close: false })) if drafts.len() == 1
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

    /// Night finding N4: with a published Part (no draft) and unpublished
    /// component edits, the Parts list offers "Also publish 1 Part for r2",
    /// on by default and naming the Part. Publish then sends one
    /// `library.publish_with_parts` pinning the Part to r2; turned off, it
    /// sends the plain `library.publish`, which leaves the Part on r1.
    #[test]
    fn publish_offers_to_carry_the_published_parts() -> Result<(), Box<dyn std::error::Error>> {
        let (ctx, mut engine, mut editor) = ready()?;
        pcbcraft_ui::fonts::setup_bundled_fonts(&ctx);
        let set = editor.draft.clone().ok_or("no working set")?;
        let published = Pins::of(&set, &engine).ok_or("no pins")?;
        let mut part = new_part(&set, &published);
        part.manufacturer = "ExampleCo".into();
        part.mpn = "EX-R0603-10K0-F".into();
        let created = engine.apply(
            engine.revision(),
            Operation::Library(LibraryCommand::NewDraft {
                meta: DefinitionMeta {
                    name: part.mpn.clone(),
                    ..Default::default()
                },
                definition: Definition::Part(Box::new(part)),
            }),
        )?;
        let pcbcraft_core::Outcome::DraftCreated { id } = created else {
            return Err(format!("expected a draft, got {created:?}").into());
        };
        apply(
            &mut engine,
            Some(Operation::Library(LibraryCommand::Publish {
                drafts: vec![id],
                close: true,
            })),
        )?;
        editor.commit(&ctx, &engine);
        let label = "Also publish 1 Part for r2";
        let shown = frame(&mut editor, &engine, &ctx, vec![]);
        assert!(!shown.iter().any(|(t, _)| t == label), "nothing to publish");
        if let Some(set) = &mut editor.draft {
            set.footprint.pads[0].position = pt([-1.5, 0.0]);
        }
        let operation = editor.commit(&ctx, &engine);
        apply(&mut engine, operation)?;
        editor.commit(&ctx, &engine);
        let shown = frame(&mut editor, &engine, &ctx, vec![]);
        assert!(shown.iter().any(|(t, _)| t == label), "{shown:?}");
        assert!(
            shown.iter().any(|(t, _)| t == "EX-R0603-10K0-F"),
            "{shown:?}"
        );
        editor.parts.keep_parts = true;
        assert!(
            matches!(
                editor.publish_set(&engine),
                Some(Operation::Library(LibraryCommand::Publish { .. }))
            ),
            "turned off: the plain publish"
        );
        editor.parts.keep_parts = false;
        let operation = editor.publish_set(&engine);
        assert!(
            matches!(
                &operation,
                Some(Operation::Library(LibraryCommand::PublishWithParts { parts, .. })) if *parts == [id]
            ),
            "{operation:?}"
        );
        let before = engine.revision();
        apply(&mut engine, operation)?;
        assert_eq!(engine.revision(), before + 1, "one undo step");
        let library = &engine.project().library;
        let parts = library.latest_parts();
        let [part] = parts.as_slice() else {
            return Err("one Part lineage".into());
        };
        assert_eq!(part.revision.0, 2);
        assert_eq!(part.definition.component.revision.0, 2);
        assert!(library.component(part.definition.component).is_some());
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
            set.footprint.pads[0].position = pt([-1.5, 0.0]);
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
        let Some(Operation::Library(LibraryCommand::Publish { drafts, .. })) = &operation else {
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
        pcbcraft_ui::fonts::setup_bundled_fonts(&ctx);
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
        let id = PartProps::draft_ids(&engine, set.ids.component)
            .first()
            .copied()
            .ok_or("no Part draft")?;
        editor.parts.open_dialog(id);
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

    /// Press and release the pointer on the drawn text `label` in a
    /// `width` × `height` window; returns what the inspector sent.
    fn click_at(
        editor: &mut PartEditor,
        engine: &Engine,
        ctx: &egui::Context,
        label: &str,
        size: egui::Vec2,
    ) -> Result<Option<Operation>, String> {
        let shown = frame_sized(editor, engine, ctx, vec![], size);
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
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                    events,
                    ..Default::default()
                },
                |ui| sent = sent.take().or(editor.inspector(ui, engine)),
            );
            output.textures_delta.clear();
        }
        Ok(sent)
    }

    const TALL: egui::Vec2 = egui::vec2(360.0, 4000.0);
    const SCREEN: egui::Vec2 = egui::vec2(360.0, 700.0);

    fn has(shown: &[(String, egui::Pos2)], label: &str) -> bool {
        shown.iter().any(|(t, _)| t == label)
    }

    /// New Part… in the card drafts a Part and opens it in the dialog, with
    /// the footer pinned to the slot's bottom. Publish Part is gated while
    /// issues block it (the dialog counts them and sends nothing); once an
    /// identity is typed it publishes with one `library.publish`, the
    /// dialog says so with a toast offering Undo, and its return link goes
    /// back to the instance the request came from.
    #[test]
    fn the_part_dialog_gates_publish_and_returns_to_the_instance()
    -> Result<(), Box<dyn std::error::Error>> {
        let (ctx, mut engine, mut editor) = ready()?;
        pcbcraft_ui::fonts::setup_bundled_fonts(&ctx);
        let new = click_at(&mut editor, &engine, &ctx, "New Part\u{2026}", TALL)?;
        assert!(
            matches!(
                &new,
                Some(Operation::Library(LibraryCommand::NewDraft { .. }))
            ),
            "{new:?}"
        );
        apply(&mut engine, new)?;
        editor.commit(&ctx, &engine);
        assert!(editor.parts.is_open(), "the new Part opens in the dialog");
        let back = PartReturn {
            instance: engine.project().instances.first().map(|i| i.id),
            designator: "R1".into(),
        };
        editor.part_return_to(Some(back.clone()));
        let shown = frame_sized(&mut editor, &engine, &ctx, vec![], SCREEN);
        assert!(has(&shown, "Part \u{b7} New"), "{shown:?}");
        assert!(!has(&shown, "PARTS"), "the dialog holds the slot alone");
        let primary = shown
            .iter()
            .find(|(t, _)| t == "Publish Part")
            .map(|(_, p)| *p)
            .ok_or("no Publish Part")?;
        assert!(
            primary.y > SCREEN.y - crate::kit::card::FOOTER_HEIGHT,
            "the footer is pinned to the bottom: {primary:?}"
        );
        assert!(has(&shown, "block publishing"), "{shown:?}");
        // A blank identity is not marked until something is typed.
        assert!(!has(&shown, "Enter the MPN"));
        let sent = click_at(&mut editor, &engine, &ctx, "Publish Part", SCREEN)?;
        assert!(sent.is_none(), "a blocked Part does not publish: {sent:?}");
        // Type the maker only: the MPN is marked inline.
        if let Some(edit) = &mut editor.parts.draft {
            edit.part.manufacturer = "ExampleCo".into();
        }
        let shown = frame_sized(&mut editor, &engine, &ctx, vec![], SCREEN);
        assert!(has(&shown, "Enter the MPN"), "{shown:?}");
        assert!(!has(&shown, "Enter the manufacturer"));
        if let Some(edit) = &mut editor.parts.draft {
            edit.part.mpn = "EX-R0603-10K0-F".into();
            edit.meta.name = edit.part.mpn.clone();
        }
        let edit = editor.commit(&ctx, &engine);
        apply(&mut engine, edit)?;
        editor.commit(&ctx, &engine);
        let shown = frame_sized(&mut editor, &engine, &ctx, vec![], SCREEN);
        assert!(has(&shown, "Part \u{b7} EX-R0603-10K0-F"), "{shown:?}");
        assert!(!has(&shown, "Enter the MPN"));
        let publish = click_at(&mut editor, &engine, &ctx, "Publish Part", SCREEN)?;
        assert!(
            matches!(
                &publish,
                Some(Operation::Library(LibraryCommand::Publish { drafts, close: false })) if drafts.len() == 1
            ),
            "{publish:?}"
        );
        let before = engine.revision();
        apply(&mut engine, publish)?;
        assert_eq!(engine.revision(), before + 1, "one undo step");
        crate::kit::toast::take_posted(&ctx);
        let shown = frame_sized(&mut editor, &engine, &ctx, vec![], SCREEN);
        assert!(has(&shown, "Published EX-R0603-10K0-F r1"), "{shown:?}");
        let toasts = crate::kit::toast::take_posted(&ctx);
        assert!(
            toasts
                .iter()
                .any(|t| t.text == "Published EX-R0603-10K0-F r1"
                    && t.actions.contains(&crate::kit::toast::ToastAction::Undo)),
            "a toast offers Undo"
        );
        let label = back.label();
        assert_eq!(label, "Back to R1 \u{203a}");
        let sent = click_at(&mut editor, &engine, &ctx, &label, SCREEN)?;
        assert!(sent.is_none());
        assert_eq!(editor.take_part_return(), Some(back));
        assert!(editor.take_part_return().is_none(), "taken once");
        assert!(!editor.parts.is_open(), "the dialog closes");
        let shown = frame_sized(&mut editor, &engine, &ctx, vec![], TALL);
        assert!(has(&shown, "PARTS"), "the card is back: {shown:?}");
        Ok(())
    }

    /// A schematic request drafts the Part with its values and opens the
    /// dialog directly, whatever the card showed; Cancel closes it, keeps
    /// the draft and drops the return link.
    #[test]
    fn a_requested_part_opens_in_the_dialog_and_cancel_keeps_the_draft()
    -> Result<(), Box<dyn std::error::Error>> {
        let (ctx, mut engine, mut editor) = ready()?;
        pcbcraft_ui::fonts::setup_bundled_fonts(&ctx);
        let set = editor.draft.clone().ok_or("no working set")?;
        let field = set.component.fields.first().ok_or("no field")?.field;
        editor.parts.request_new(vec![(
            field,
            fields::FieldValue::Quantity {
                si: pcbcraft_core::quantity::Decimal::from_f64(4_700.0)?,
                display_unit: None,
            },
        )]);
        editor.part_return_to(Some(PartReturn::default()));
        let mut sent = None;
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            sent = editor.inspector(ui, &engine);
        });
        output.textures_delta.clear();
        assert!(matches!(
            &sent,
            Some(Operation::Library(LibraryCommand::NewDraft { definition: Definition::Part(p), .. }))
                if p.specs.iter().any(|s| matches!(s.value, fields::FieldValue::Quantity { si, .. } if si.to_f64() == 4_700.0))
        ));
        apply(&mut engine, sent)?;
        editor.commit(&ctx, &engine);
        let shown = frame_sized(&mut editor, &engine, &ctx, vec![], SCREEN);
        assert!(has(&shown, "Part \u{b7} New"), "{shown:?}");
        let sent = click_at(&mut editor, &engine, &ctx, "Cancel", SCREEN)?;
        assert!(sent.is_none());
        assert!(!editor.parts.is_open());
        assert!(editor.parts.back.is_none(), "the return link is dropped");
        assert_eq!(
            PartProps::draft_ids(&engine, set.ids.component).len(),
            1,
            "the draft is kept"
        );
        Ok(())
    }

    /// A refusal handed back to the dialog shows once, as its last body row,
    /// and not as a toast.
    #[test]
    fn a_refusal_shows_in_the_dialog_row() -> Result<(), Box<dyn std::error::Error>> {
        let (ctx, mut engine, mut editor) = ready()?;
        pcbcraft_ui::fonts::setup_bundled_fonts(&ctx);
        let new = click_at(&mut editor, &engine, &ctx, "New Part\u{2026}", TALL)?;
        apply(&mut engine, new)?;
        editor.commit(&ctx, &engine);
        crate::refusal::hand_back(
            &ctx,
            crate::refusal::Refused {
                origin: Origin::Dialog(dialog_id()),
                error: pcbcraft_core::Error::Conflict("ExampleCo EX-1 is already published".into()),
            },
            false,
        );
        frame_sized(&mut editor, &engine, &ctx, vec![], SCREEN);
        let shown = frame_sized(&mut editor, &engine, &ctx, vec![], SCREEN);
        assert!(
            has(&shown, "ExampleCo EX-1 is already published"),
            "{shown:?}"
        );
        assert!(
            crate::refusal::unclaimed(&ctx).is_empty(),
            "claimed, so no toast"
        );
        Ok(())
    }

    /// A published Part without a draft opens read-only with Edit Part,
    /// which drafts a new revision of it in the same dialog.
    #[test]
    fn a_published_part_opens_read_only_with_edit() -> Result<(), Box<dyn std::error::Error>> {
        let (ctx, mut engine, mut editor) = ready()?;
        pcbcraft_ui::fonts::setup_bundled_fonts(&ctx);
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
        let id = PartProps::draft_ids(&engine, set.ids.component)
            .first()
            .copied()
            .ok_or("no Part draft")?;
        apply(
            &mut engine,
            Some(Operation::Library(LibraryCommand::Publish {
                drafts: vec![id],
                close: false,
            })),
        )?;
        // Publishing keeps a draft equal to the revision; without one the
        // Part is read-only.
        apply(
            &mut engine,
            Some(Operation::Library(LibraryCommand::DiscardDraft { id })),
        )?;
        editor.commit(&ctx, &engine);
        let opened = click_at(
            &mut editor,
            &engine,
            &ctx,
            "EX-R0603-10K0-F \u{b7} ExampleCo \u{b7} r1",
            TALL,
        )?;
        assert!(opened.is_none());
        assert!(editor.parts.is_open());
        let edit = click_at(&mut editor, &engine, &ctx, "Edit Part", SCREEN)?;
        assert!(matches!(
            &edit,
            Some(Operation::Library(LibraryCommand::DraftFrom { .. }))
        ));
        apply(&mut engine, edit)?;
        editor.commit(&ctx, &engine);
        let shown = frame_sized(&mut editor, &engine, &ctx, vec![], SCREEN);
        assert!(has(&shown, "Draft"), "{shown:?}");
        assert!(has(&shown, "Publish Part"));
        Ok(())
    }

    /// One inspector frame; returns the drawn texts and their centres.
    fn frame(
        editor: &mut PartEditor,
        engine: &Engine,
        ctx: &egui::Context,
        events: Vec<egui::Event>,
    ) -> Vec<(String, egui::Pos2)> {
        frame_sized(editor, engine, ctx, events, TALL)
    }

    /// One inspector frame in a `size` window.
    fn frame_sized(
        editor: &mut PartEditor,
        engine: &Engine,
        ctx: &egui::Context,
        events: Vec<egui::Event>,
        size: egui::Vec2,
    ) -> Vec<(String, egui::Pos2)> {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
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
        pcbcraft_ui::fonts::setup_bundled_fonts(&ctx);
        let set = editor.draft.clone().ok_or("no working set")?;
        let pins = Pins::of(&set, &engine).ok_or("no pins")?;
        let field = set.component.fields.first().ok_or("no field")?.field;
        let mut part = new_part(&set, &pins);
        part.specs.push(RatedSpec {
            field,
            value: fields::FieldValue::Quantity {
                si: pcbcraft_core::quantity::Decimal::from_f64(10_000.0)?,
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
        // The new Part opens in the dialog, not in the card.
        for label in [
            "Part \u{b7} New",
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
            Some(fields::FieldValue::Quantity { si, .. }) if si.to_f64() == 22_000.0
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
