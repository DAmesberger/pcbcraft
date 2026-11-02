//! Component authoring content for the shared shell. Form spacing, controls and
//! typography follow Solidcraft ui_kit/layout.rs and fields.rs at the pinned revision.
//!
//! The editor authors a working set of five drafts (component, symbol,
//! package, footprint, binding). Widgets edit a working copy; a finished
//! gesture commits it as one `library.edit`, so every finished edit is one
//! shared undo step. Publishing sends one `library.publish` of the five.
mod arrange;
#[cfg(test)]
mod card_tests;
mod datasheet_assist;
mod details_card;
mod drawing;
#[cfg(test)]
mod drawing_tests;
mod family_rows;
mod folder_preview;
mod footprint_canvas;
mod footprint_card;
pub(crate) mod footprint_dims;
mod footprint_dims_ui;
mod footprint_geometry;
pub(crate) mod footprint_params;
mod footprint_params_ui;
mod footprint_props;
mod footprint_template_picker;
#[cfg(test)]
mod footprint_tests;
mod footprint_tools;
mod footprint_tools_canvas;
#[cfg(test)]
mod footprint_tools_tests;
mod identity;
#[cfg(test)]
mod identity_tests;
mod mapping_card;
mod marking_place;
mod marking_props;
mod model_body;
mod model_card;
mod model_import;
mod model_place;
#[cfg(test)]
pub(crate) use model_place::overlay as land_overlay;
mod model_props;
#[cfg(test)]
mod model_tests;
mod model_view;
mod part_props;
mod pins_tab;
mod pins_table;
mod rules_editor;
mod snap;
mod start;
mod svg_import;
pub use svg_import::IMPORT_SVG;
mod symbol_canvas;
mod symbol_card;
mod symbol_props;
mod working_set;

use crate::kit::{self, severity::Level};
use egui::{Align2, FontId, Pos2, Rect, Sense, Stroke, Ui};
#[cfg(test)]
pub use folder_preview::sample_item;
pub use folder_preview::{edit_copy, spawn as spawn_preview};
pub use identity::FolderAction;
pub use model_view::reset_model_view;
pub use part_props::{PartReturn, meaning_label, spec_text};
use pcbcraft_core::functions::{Direction, TerminalFunctions};
use pcbcraft_core::geometry::{Bounds, Shape};
use pcbcraft_core::library::graphics::{
    self, Fill, HAlign, Justify, PIN_TEXT_HEIGHT_MM, SYMBOL_MODULE_MM, SYMBOL_STROKE_MM,
    TextContent, VAlign,
};
use pcbcraft_core::library::{
    Drafts, Item, PackageTerminal, Pad, Pin, PinDirection, Terminal, TerminalClass, TerminalMap,
    Unit,
};
use pcbcraft_core::{
    ComponentRef, DefinitionId, Engine, FunctionId, GraphicId, LibraryCommand, ObjectRef,
    Operation, PackageTerminalId, PadId, Point, Query, QueryResult, TerminalId, UnitId,
};
use solidcraft_ui_egui::{fonts, theme::Tokens};
use std::sync::Arc;
#[cfg(test)]
pub use working_set::Template;
pub use working_set::WorkingSet;

/// One selected object of the Symbol view: a pin by its terminal or a
/// graphic by its stable ID.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Selected {
    Pin(TerminalId),
    Graphic(GraphicId),
}

/// Capitalise the first letter of a core label for a dropdown.
fn sentence(label: &str) -> String {
    let mut chars = label.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// Text typed into a field default, kept across frames by field lineage.
#[derive(Clone, Debug, Default, PartialEq)]
struct FieldText {
    /// The default's text when the buffer last matched the working copy.
    loaded: String,
    text: String,
}

impl FieldText {
    fn new(text: String) -> Self {
        Self {
            loaded: text.clone(),
            text,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
enum View {
    /// Component-level data: definition, fields, Parts, library and
    /// provenance (docs/sidebar-ia.md, "Properties card by subject").
    #[default]
    Details,
    Symbol,
    /// C5: the pin table, peripherals and rule scripts.
    Pins,
    Footprint,
    Mapping,
    Model,
}

/// A request the editor cannot express as one operation; the app runs it
/// through the engine with public commands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Request {
    /// Tests: reserve five new lineages and fill them from a sample
    /// template (people use the template gallery's
    /// `library.new_from_template`).
    #[cfg(test)]
    Create(Template),
    /// Draft the lineages of a published component revision.
    Draft(ComponentRef),
}

pub struct PartEditor {
    /// Working copy edited by widgets.
    draft: Option<WorkingSet>,
    /// What the working copy was loaded from; a difference is uncommitted work.
    baseline: Option<WorkingSet>,
    /// The engine drafts last loaded, compared by identity to notice undo,
    /// redo, open and publish.
    source: Option<Arc<Drafts>>,
    /// The component lineage being authored: the workspace's subject,
    /// always chosen explicitly (or the start page when unset).
    lineage: Option<DefinitionId>,
    /// The lineage whose working set the views were last reset for.
    shown: Option<DefinitionId>,
    /// A commit was sent; resync from the engine whatever its outcome.
    awaiting: bool,
    publishing: bool,
    /// Publish once the edit sent before it has been applied.
    publish_next: bool,
    request: Option<Request>,
    view: View,
    selected: Option<TerminalId>,
    /// Toasts to post on the next frame with a context: command results
    /// and gesture refusals (the editor `notice` string's replacement).
    posts: Vec<kit::toast::Posted>,
    /// A terminal shown on the Symbol tab that has no pin on the drawn
    /// unit (its TERMINAL card).
    terminal_card: Option<TerminalId>,
    /// The issue slot of the inspected subject and what it was counted for.
    issue_slot: Option<(IssueKey, kit::card::IssueSlot)>,
    /// The pad the Footprint view and its properties edit.
    selected_pad: Option<PadId>,
    /// Footprint view pan and zoom; fitted when unset.
    footprint_view: Option<symbol_canvas::ViewState>,
    pad_drag: Option<footprint_canvas::PadDrag>,
    footprint_layers: footprint_canvas::Layers,
    /// The armed Footprint sketch tool and its pending points.
    footprint_tool: footprint_tools::ToolState,
    /// The open "New from template" form.
    template_picker: footprint_template_picker::TemplatePicker,
    /// Fit the Footprint view once a template's pads arrive.
    fit_on_build: bool,
    /// Footprint sketch dimensions, constraints, drags and parameters.
    footprint_dims: footprint_dims::DimsState,
    /// The footprint graphic a clicked finding names, outlined on the
    /// canvas.
    finding_graphic: Option<GraphicId>,
    update: crate::update_preview::UpdatePreview,
    /// Typed component defaults by field lineage.
    field_buffers: std::collections::BTreeMap<DefinitionId, FieldText>,
    /// The armed Symbol tool and its pending points.
    drawing: drawing::Drawing,
    /// Selected pins and graphics of the unit being drawn.
    selection: arrange::Selection,
    /// The unit being drawn; the first when unset.
    unit: Option<UnitId>,
    /// Symbol view pan and zoom; fitted when unset.
    symbol_view: Option<symbol_canvas::ViewState>,
    /// The Symbol canvas's last size, to tell whether a located object is
    /// in view.
    symbol_canvas_size: Option<egui::Vec2>,
    gesture: Option<symbol_canvas::Gesture>,
    /// A gesture finished by a key or button, placed on the next canvas frame.
    pending_place: Option<drawing::Placed>,
    snap: bool,
    show_grid: bool,
    /// Stroke width and fill for new shapes.
    stroke_width: f64,
    fill: Fill,
    /// What the Field tool places, when chosen.
    field_choice: Option<TextContent>,
    /// Focus the text content field once (after placing text).
    focus_text: bool,
    /// Space was pressed on the canvas (momentary pan) and is still down.
    space_held: bool,
    /// Two-finger scrolling across frames (symbol and footprint canvases).
    trackpad: crate::navigation::Trackpad,
    /// The drawn unit and its snap targets, kept while the working copy and
    /// engine revision are unchanged so idle frames redo nothing.
    drawn_cache: Option<symbol_canvas::DrawnCache>,
    /// The body rectangle a new terminal's pin row grows: the one the
    /// template or first unit laid out, tracked by ID so a rectangle the
    /// author drew is never stretched. `None` once it is gone.
    body: Option<(UnitId, GraphicId)>,
    /// The component's manufacturer parts.
    parts: part_props::PartProps,
    /// The 3D tab's placement gestures.
    model: model_place::ModelTab,
    /// A running Import SVG….
    svg: svg_import::SvgImport,
    /// A1: the identity bar's history and recents.
    identity: identity::Identity,
    /// A1: the start page's search.
    start: start::StartPage,
    /// Create from datasheet… (the datasheet assistant).
    assist: datasheet_assist::DatasheetAssist,
    /// C5: the Pins tab.
    pins: pins_tab::PinsTab,
    /// B4: the New-from-template gallery sheet.
    gallery: crate::template_gallery::TemplateGallery,
    /// D3: the model chooser sheet and import wizard.
    model_chooser: crate::model_library::ModelChooser,
    model_import: model_import::ModelImport,
    /// D6: the 3D tab's Marking section.
    marking: marking_props::MarkingProps,
}

impl Default for PartEditor {
    fn default() -> Self {
        Self {
            draft: None,
            baseline: None,
            source: None,
            lineage: None,
            shown: None,
            awaiting: false,
            publishing: false,
            publish_next: false,
            request: None,
            view: View::Details,
            selected: Some(TerminalId(1)),
            posts: Vec::new(),
            terminal_card: None,
            issue_slot: None,
            selected_pad: None,
            footprint_view: None,
            pad_drag: None,
            footprint_layers: Default::default(),
            footprint_tool: Default::default(),
            template_picker: Default::default(),
            fit_on_build: false,
            footprint_dims: Default::default(),
            finding_graphic: None,
            update: Default::default(),
            field_buffers: Default::default(),
            drawing: Default::default(),
            selection: Default::default(),
            unit: None,
            symbol_view: None,
            symbol_canvas_size: None,
            gesture: None,
            pending_place: None,
            snap: true,
            show_grid: true,
            stroke_width: SYMBOL_STROKE_MM,
            fill: Fill::None,
            field_choice: None,
            focus_text: false,
            space_held: false,
            trackpad: Default::default(),
            drawn_cache: None,
            body: None,
            parts: Default::default(),
            model: Default::default(),
            svg: Default::default(),
            identity: Default::default(),
            start: Default::default(),
            assist: Default::default(),
            pins: Default::default(),
            gallery: Default::default(),
            model_chooser: Default::default(),
            model_import: Default::default(),
            marking: Default::default(),
        }
    }
}

impl PartEditor {
    /// The package the 3D preview builds, in its own model frame: the
    /// placement is a display transform ([`Self::model_view_prepare`]), so
    /// moving or turning the model never rebuilds it.
    pub fn preview_package(&self) -> Option<pcbcraft_core::library::PackageDefinition> {
        self.draft
            .as_ref()
            .map(|set| pcbcraft_core::library::PackageDefinition {
                placement: Default::default(),
                ..set.package.clone()
            })
    }
    pub fn is_model_view(&self) -> bool {
        self.view == View::Model
    }
    /// A multi-command request for the app to run, if one is pending.
    pub fn take_request(&mut self) -> Option<Request> {
        self.request.take()
    }

    fn unpublished(&self, engine: &Engine) -> bool {
        self.draft
            .as_ref()
            .is_some_and(|set| set.unpublished(engine))
    }

    /// Reload the working copy when the engine's drafts changed elsewhere.
    fn sync(&mut self, engine: &Engine) {
        let current = Arc::clone(&engine.document().drafts);
        let changed = self
            .source
            .as_ref()
            .is_none_or(|s| !Arc::ptr_eq(s, &current));
        if !changed && !self.awaiting {
            return;
        }
        self.awaiting = false;
        #[cfg(test)]
        self.open_created(engine);
        // Only the lineage opened explicitly; never another draft.
        let set = self
            .lineage
            .and_then(|lineage| WorkingSet::of(&current, lineage));
        let shown = set.as_ref().map(|s| s.ids.component);
        if shown != self.shown {
            self.symbol_view = None;
            self.footprint_view = None;
            self.selected_pad = None;
            self.selection.clear();
            self.unit = None;
            self.drawing.arm(drawing::Tool::Select);
            self.body = set.as_ref().and_then(layout_body);
        }
        self.shown = shown;
        self.gesture = None;
        if let Some(set) = &set
            && self
                .selected
                .is_none_or(|id| set.component.terminal(id).is_none())
        {
            self.selected = set.component.terminals.first().map(|t| t.id);
        }
        if let (Some(set), Some(id)) = (&set, self.terminal_card)
            && set.component.terminal(id).is_none()
        {
            self.terminal_card = None;
        }
        if self.publishing
            && let Some(set) = &set
            && !set.unpublished(engine)
            && let Some(published) = set.published(engine)
        {
            let older = engine
                .project()
                .instances
                .iter()
                .filter(|i| {
                    i.component.id == published.id && i.component.revision != published.revision
                })
                .count();
            let name = &set.metas.component.name;
            let text = match older {
                0 => format!("Published {name} r{}", published.revision.0),
                n => format!(
                    "Published {name} r{} · {n} placed {} keep older revisions",
                    published.revision.0,
                    if n == 1 { "instance" } else { "instances" }
                ),
            };
            self.tell(Level::Ok, text);
        }
        self.publishing = false;
        if self.fit_on_build && set.as_ref().is_some_and(|s| !s.footprint.pads.is_empty()) {
            self.fit_on_build = false;
            self.footprint_view = None;
        }
        self.baseline = set.clone();
        self.draft = set;
        self.source = Some(current);
        self.pad_drag = None;
    }

    /// The operation committing a finished edit, if any. Call once per frame
    /// after the editor's widgets ran.
    fn commit_inner(&mut self, ctx: &egui::Context, engine: &Engine) -> Option<Operation> {
        self.sync(engine);
        self.flush(ctx);
        if let Some(publish) = self.publish_pending(engine) {
            return Some(publish);
        }
        let interacting =
            ctx.memory(|m| m.focused().is_some()) || ctx.input(|i| i.pointer.any_down());
        if let Some(lineage) = self.lineage
            && let Some(publish) = self.parts.publish_pending(engine, lineage)
        {
            return Some(publish);
        }
        if interacting {
            return None;
        }
        if let Some(sketch) = self.footprint_tool_operation(engine) {
            return Some(sketch);
        }
        // A finished Part edit travels with the working set's, so neither
        // reload discards the other's uncommitted work.
        let part = self
            .lineage
            .and_then(|lineage| self.parts.take_edit(engine, lineage));
        if self.draft == self.baseline {
            let edit =
                part.map(|edit| Operation::Library(LibraryCommand::Edit { edits: vec![edit] }));
            return self.marking.with_artwork(edit);
        }
        let mut operation = self.send(engine);
        if let Some(Operation::Library(LibraryCommand::Edit { edits })) = &mut operation {
            edits.extend(part);
        }
        // SVG artwork a marking now names packs with this edit.
        self.marking.with_artwork(operation)
    }

    /// Send the working copy with sibling references at their pending
    /// revisions; the next sync shows what the engine accepted.
    fn send(&mut self, engine: &Engine) -> Option<Operation> {
        let set = self.draft.as_ref()?.retargeted(engine.drafts());
        self.baseline = self.draft.clone();
        self.awaiting = true;
        Some(Operation::Library(LibraryCommand::Edit {
            edits: set.edits(),
        }))
    }

    /// Send a deferred publish once the edit before it was applied.
    fn publish_pending(&mut self, engine: &Engine) -> Option<Operation> {
        if self.publish_next && !self.awaiting {
            self.publish_next = false;
            if self.draft == self.baseline
                && let Some(publish) = self.publish_set(engine)
            {
                self.publishing = true;
                self.awaiting = true;
                return Some(publish);
            }
        }
        None
    }

    /// Ribbon tabs are the four views of one component; groups are commands.
    fn ribbon_inner(&mut self, engine: &Engine) -> crate::shell::Ribbon {
        use crate::shell::{Group, Ribbon, Tab, Tool};
        self.sync(engine);
        let tabs = [
            ("details", "Details", View::Details),
            ("symbol", "Symbol", View::Symbol),
            ("pins", "Pins", View::Pins),
            ("footprint", "Footprint", View::Footprint),
            ("mapping", "Mapping", View::Mapping),
            ("model", "3D", View::Model),
        ]
        .into_iter()
        .map(|(id, label, view)| Tab {
            id,
            label,
            active: self.view == view,
        })
        .collect();
        if self.update.is_open() {
            return Ribbon {
                tabs,
                groups: vec![],
                notice: Some("Review the instance update in Properties"),
            };
        }
        let has_draft = self.draft.is_some();
        let selected_mapped = self.draft.as_ref().is_some_and(|set| {
            set.binding
                .mappings
                .iter()
                .any(|m| Some(m.terminal) == self.selected)
        });
        let library = self.component_group(engine);
        if self.is_read_only() {
            // A library item is shown, never edited: only the subject's
            // commands.
            return Ribbon {
                tabs,
                groups: vec![library],
                notice: None,
            };
        }
        if self.view == View::Symbol {
            return Ribbon {
                tabs,
                groups: self.symbol_groups(has_draft, library),
                notice: None,
            };
        }
        if self.view == View::Details {
            return Ribbon {
                tabs,
                groups: vec![library],
                notice: None,
            };
        }
        let mut edit = vec![
            Tool::new("terminal", "point", "Terminal")
                .enabled(has_draft)
                .tip("Add a terminal with its symbol pin, package terminal and pad"),
        ];
        edit.push(
            Tool::new("remove-terminal", "delete", "Remove")
                .enabled(has_draft && self.selected.is_some())
                .tip("Remove the selected terminal everywhere in this component"),
        );
        let terminals = Group {
            label: "Terminals",
            tools: edit,
        };
        let mut groups = Vec::new();
        if self.view == View::Footprint {
            // Publish stays ahead of the layer toggles, so at narrow widths
            // the toggles fold into the overflow first.
            let [pads, show] = self.footprint_groups(has_draft, selected_mapped);
            let artwork = Group {
                label: "Artwork",
                tools: vec![self.import_svg_tool()],
            };
            if self.footprint_sketching() {
                // The pad and constraint tools lead: at narrow widths the
                // drawing, layer and terminal groups fold away first.
                let mut sketch = self.footprint_sketch_groups();
                let take = |sketch: &mut Vec<Group>, label: &str| {
                    sketch
                        .iter()
                        .position(|g| g.label == label)
                        .map(|i| sketch.remove(i))
                };
                if let Some(mut group) = take(&mut sketch, "Pads") {
                    group.tools.insert(0, self.template_tool(has_draft));
                    groups.push(group);
                }
                groups.push(self.constrain_group());
                groups.push(library);
                groups.extend(sketch);
                groups.extend([terminals, artwork, show]);
            } else {
                groups.extend([terminals, pads, artwork, library, show]);
            }
        } else {
            groups.extend([terminals, library]);
        }
        Ribbon {
            tabs,
            groups,
            notice: None,
        }
    }

    /// The Component group: New and Open lead to the start page (template
    /// gallery, catalog search and recents); Publish publishes the five
    /// drafts. A read-only library item offers Edit a copy instead.
    fn component_group(&self, engine: &Engine) -> crate::shell::Group {
        use crate::shell::{Group, Tool};
        let mut tools = vec![
            Tool::new("component-new", "new", "New")
                .selected(
                    self.gallery.scope() == Some(&crate::template_gallery::Scope::NewComponent),
                )
                .tip("Start a new component from a package template"),
            Tool::new("component-open", "open", "Open")
                .tip("Open a component of this project or a library source"),
        ];
        if self.is_read_only() {
            tools.push(
                Tool::new("component-edit-copy", "copy", "Edit a copy")
                    .tip("Import this library revision into the project and draft it"),
            );
        }
        tools.push(
            Tool::new("publish", "save", "Publish")
                .enabled(!self.is_read_only() && self.unpublished(engine))
                .tip("Publish the five drafts as new immutable revisions"),
        );
        Group {
            label: "Component",
            tools,
        }
    }

    /// The Footprint tab's pad tools and layer toggles.
    fn footprint_groups(&self, has_draft: bool, selected_mapped: bool) -> [crate::shell::Group; 2] {
        use crate::shell::{Group, Tool};
        let explicit = self
            .draft
            .as_ref()
            .is_some_and(|set| set.footprint.source.is_none());
        let has_pad = self.draft.as_ref().is_some_and(|set| {
            footprint_props::current_pad(
                &set.footprint,
                &set.binding,
                self.selected_pad,
                self.selected,
            )
            .is_some()
        });
        let pads = vec![
            Tool::new("pad", "rect", "SMD")
                .enabled(selected_mapped && explicit)
                .tip("Add a surface pad to the selected terminal"),
            Tool::new("pad-pth", "circle", "Through-hole")
                .enabled(selected_mapped && explicit)
                .tip("Add a plated through-hole pad to the selected terminal"),
            Tool::new("pad-npth", "hole", "Hole")
                .enabled(has_draft && explicit)
                .tip("Add a non-plated mounting hole (lands no terminal)"),
            Tool::new("pad-fiducial", "point_center", "Fiducial")
                .enabled(has_draft && explicit)
                .tip("Add an optical fiducial (lands no terminal, no paste)"),
            Tool::new("remove-pad", "delete", "Remove pad")
                .enabled(has_pad && explicit)
                .tip("Remove the selected pad; its terminal stays"),
        ];
        let layers = footprint_canvas::LAYER_TOOLS
            .into_iter()
            .map(|(id, icon, label, tip)| {
                Tool::new(id, icon, label)
                    .selected(self.footprint_layers.get(id).unwrap_or(false))
                    .tip(tip)
            })
            .collect();
        let mut pads = pads;
        pads.insert(0, self.template_tool(has_draft));
        [
            Group {
                label: "Pads",
                tools: pads,
            },
            Group {
                label: "Show",
                tools: layers,
            },
        ]
    }

    /// "From template", first in whichever Pads group the Footprint tab
    /// shows (a template builds into an empty, sketched footprint).
    fn template_tool(&self, has_draft: bool) -> crate::shell::Tool {
        crate::shell::Tool::new(
            footprint_template_picker::TOOL,
            "component",
            "From template",
        )
        .enabled(has_draft)
        .selected(
            self.template_picker.is_open()
                || matches!(
                    self.gallery.scope(),
                    Some(crate::template_gallery::Scope::Package { .. })
                ),
        )
        .tip("Build the package, footprint and model from a package row or family template")
    }

    /// The Symbol tab's drawing ribbon: Select; Draw; Pins; Arrange; Grid
    /// and Snap; then the component's templates and Publish.
    fn symbol_groups(
        &self,
        has_draft: bool,
        library: crate::shell::Group,
    ) -> Vec<crate::shell::Group> {
        use crate::shell::{Group, Tool};
        use drawing::Tool as Draw;
        let armed = self.drawing.tool();
        let tool = |t: Draw| {
            Tool::new(t.id(), t.icon(), t.label())
                .enabled(has_draft)
                .selected(armed == t)
                .tip(t.tip())
        };
        let any = !self.selection.is_empty();
        vec![
            Group {
                label: "Select",
                tools: vec![tool(Draw::Select)],
            },
            Group {
                label: "Draw",
                // Variants fold into split buttons (Line ▾, Rectangle ▾,
                // Circle ▾, Text ▾) so Arrange stays on the ribbon at tiled
                // widths.
                tools: {
                    let family = |i: usize| {
                        let variants = Draw::FAMILIES[i];
                        let shown = if variants.contains(&armed) {
                            armed
                        } else {
                            self.drawing.shown(i)
                        };
                        crate::shell::Tool::family(
                            tool(shown),
                            variants.into_iter().map(tool).collect(),
                        )
                    };
                    vec![
                        family(0),
                        family(1),
                        family(2),
                        tool(Draw::SINGLE),
                        family(3),
                        self.import_svg_tool(),
                    ]
                },
            },
            Group {
                label: "Pins",
                tools: Draw::PINS.into_iter().map(tool).collect(),
            },
            Group {
                label: "Arrange",
                tools: symbol_canvas::Arrange::ALL
                    .into_iter()
                    .map(|a| {
                        Tool::new(a.id(), a.icon(), a.label())
                            .enabled(any)
                            .tip(a.tip())
                    })
                    .collect(),
            },
            Group {
                label: "Grid",
                tools: vec![
                    Tool::new("grid", "grid", "Grid")
                        .selected(self.show_grid)
                        .tip("Show the symbol grid; set its step in Properties"),
                    Tool::new("snap", "snap", "Snap").selected(self.snap).tip(
                        "Snap to ends, pins, midpoints, centres, curves, alignment and the grid",
                    ),
                ],
            },
            library,
        ]
    }

    /// Arm a Symbol tool. A drawing tool clears the selection, as
    /// Solidcraft's sketch tools do, so Properties edits the tool's options
    /// and nothing else looks editable.
    fn arm_tool(&mut self, tool: drawing::Tool) {
        self.drawing.arm(tool);
        if tool != drawing::Tool::Select {
            self.selection.clear();
            self.gesture = None;
        }
    }

    /// The keyboard context of the open view, for the command registry.
    pub fn shortcut_context(&self) -> crate::commands::Context {
        use crate::commands::Context;
        match self.view {
            View::Symbol => Context::Symbol,
            View::Footprint => Context::Footprint,
            _ => Context::Component,
        }
    }

    pub fn on_tab(&mut self, tab: &str) {
        self.view = match tab {
            "details" => View::Details,
            "pins" => View::Pins,
            "footprint" => View::Footprint,
            "mapping" => View::Mapping,
            "model" => View::Model,
            _ => View::Symbol,
        };
    }

    /// Queue a toast for the next frame with a context: a command result
    /// or a gesture refusal, one line naming objects.
    fn tell(&mut self, level: Level, text: impl Into<String>) {
        // A burst of refusals in one gesture needs only its first few.
        if self.posts.len() < 8 {
            self.posts.push(kit::toast::Posted {
                level,
                text: text.into(),
                actions: Vec::new(),
            });
        }
    }

    /// Post the queued toasts; the app drains them into the toast stack
    /// and Messages.
    fn flush(&mut self, ctx: &egui::Context) {
        for post in self.posts.drain(..) {
            kit::toast::post(ctx, post.level, post.text, post.actions);
        }
    }

    pub fn on_tool(&mut self, tool: &str, engine: &Engine) -> Option<Operation> {
        match tool {
            "component-new" => {
                self.open_template_gallery();
                return None;
            }
            "component-open" => {
                self.show_start();
                return None;
            }
            "component-edit-copy" => {
                if self.is_read_only() {
                    self.identity_edit_copy();
                }
                return None;
            }
            _ => {}
        }
        if self.is_read_only() {
            // The read-only ribbon offers nothing else; a stray command
            // never edits a library item.
            return None;
        }
        self.on_tool_inner(tool, engine)
    }

    fn on_tool_inner(&mut self, tool: &str, engine: &Engine) -> Option<Operation> {
        self.sync(engine);
        if let Some(t) = drawing::Tool::from_id(tool) {
            self.arm_tool(t);
            return None;
        }
        if let Some(t) = footprint_tools::FootprintTool::from_id(tool) {
            self.footprint_arm(t);
            return None;
        }
        if self.footprint_layer_tool(tool) {
            return None;
        }
        if tool == footprint_dims::ALL_DIMS_TOOL {
            let d = &mut self.footprint_dims.detail;
            *d = match d {
                footprint_dims::DimDetail::All => footprint_dims::DimDetail::Key,
                footprint_dims::DimDetail::Key => footprint_dims::DimDetail::All,
            };
            return None;
        }
        if let Some(action) = symbol_canvas::Arrange::from_id(tool) {
            self.arrange(action, engine.drafts().grid.symbol_mm);
            return None;
        }
        if self.footprint_layers.toggle(tool) {
            return None;
        }
        match tool {
            "grid" => self.show_grid = !self.show_grid,
            "snap" => self.snap = !self.snap,
            "terminal" => self.add_terminal(),
            "pad" => self.add_pad(footprint_geometry::NewPad::Smd),
            "pad-pth" => self.add_pad(footprint_geometry::NewPad::PlatedThrough),
            "pad-npth" => self.add_pad(footprint_geometry::NewPad::Hole),
            "pad-fiducial" => self.add_pad(footprint_geometry::NewPad::Fiducial),
            "remove-pad" => self.remove_pad(),
            "remove-terminal" => self.remove_selected(),
            footprint_template_picker::TOOL => self.open_template_picker(),
            "publish" => return self.publish(engine),
            _ => {}
        }
        None
    }

    /// What the card inspects now: the review, the open tab's document
    /// object, an armed Symbol tool, or the Symbol selection.
    fn inspected(&self) -> Inspected {
        if self.update.is_open() {
            return Inspected::Review;
        }
        if self.draft.is_none() {
            return Inspected::Empty;
        }
        if self.view != View::Symbol {
            return Inspected::Tab(self.view);
        }
        let tool = self.drawing.tool();
        if tool != drawing::Tool::Select {
            return Inspected::Tool(tool);
        }
        if let Some(terminal) = self.selected_pin() {
            return Inspected::Pin(terminal);
        }
        if let Some(graphic) = self.selected_graphic() {
            return Inspected::Graphic(graphic);
        }
        if self.selection.len() > 1 {
            return Inspected::Selection(self.selection.len());
        }
        if let Some(terminal) = self.terminal_card {
            return Inspected::Terminal(terminal);
        }
        Inspected::Tab(View::Symbol)
    }

    /// The properties title band: it names the subject, so the body never
    /// repeats it.
    pub fn header(&self, _engine: &Engine) -> crate::shell::Header {
        use crate::shell::Header;
        let component = || {
            self.draft
                .as_ref()
                .map(|set| set.metas.component.name.clone())
                .unwrap_or_default()
        };
        match self.inspected() {
            Inspected::Review => Header::new("component", "Update placed instances", "Review"),
            Inspected::Empty => Header::new("component", "No component", ""),
            Inspected::Tab(View::Footprint) => self
                .footprint_header()
                .unwrap_or_else(|| Header::new("component", component(), "Footprint")),
            Inspected::Tab(_) => Header::new("component", component(), "Component"),
            Inspected::Tool(tool) => Header::new(tool.icon(), tool.label(), "Tool"),
            Inspected::Pin(terminal) => Header::new("point", self.pin_title(terminal), "Pin"),
            Inspected::Terminal(terminal) => {
                let name = self
                    .draft
                    .as_ref()
                    .and_then(|set| set.component.terminal(terminal))
                    .map(|t| t.name.clone())
                    .unwrap_or_default();
                Header::new("point", format!("Terminal {name}"), "Terminal")
            }
            Inspected::Graphic(graphic) => {
                let (icon, title) = self.graphic_title(graphic);
                let kind = if icon == "text" { "Text" } else { "Graphic" };
                Header::new(icon, title, kind)
            }
            Inspected::Selection(count) => {
                Header::new("select", format!("{count} objects"), "Selection")
            }
        }
    }

    /// "Pin 2 · VOUT": the package designation the terminal lands, then its
    /// name; just the name when they agree or nothing lands it.
    fn pin_title(&self, terminal: TerminalId) -> String {
        let Some(set) = &self.draft else {
            return String::new();
        };
        let name = set
            .component
            .terminal(terminal)
            .map(|t| t.name.clone())
            .unwrap_or_default();
        let number = set
            .binding
            .mappings
            .iter()
            .find(|m| m.terminal == terminal)
            .and_then(|m| set.package.terminal(m.package_terminal))
            .map(|t| t.designation.clone())
            .filter(|d| !d.is_empty() && *d != name);
        match number {
            Some(number) => format!("Pin {number} · {name}"),
            None => format!("Pin {name}"),
        }
    }

    /// The icon and title of a graphic of the drawn unit.
    fn graphic_title(&self, graphic: GraphicId) -> (&'static str, String) {
        let item = self
            .draft
            .as_ref()
            .and_then(|set| set.symbol.units.get(self.unit_index()?))
            .and_then(|unit| unit.graphic(graphic))
            .map(|g| &g.item);
        match item {
            Some(Item::Shape { shape, .. }) => (
                symbol_props::shape_icon(shape),
                symbol_props::shape_name(shape).to_owned(),
            ),
            Some(Item::Text(_)) => ("text", "Text".to_owned()),
            None => ("select", "Graphic".to_owned()),
        }
    }

    /// The component lineage being authored.
    /// The tab shown and whether it selects anything, for locate tests:
    /// ("symbol", true) after a pin was located.
    #[cfg(test)]
    pub fn located(&self) -> (&'static str, bool) {
        let tab = match self.view {
            View::Details => "details",
            View::Symbol => "symbol",
            View::Pins => "pins",
            View::Footprint => "footprint",
            View::Mapping => "mapping",
            View::Model => "3d",
        };
        let selected = match self.view {
            View::Symbol => !self.selection.is_empty() || self.terminal_card.is_some(),
            View::Footprint => self.selected_pad.is_some(),
            View::Mapping => self.selected.is_some(),
            View::Details | View::Model | View::Pins => true,
        };
        (tab, selected)
    }

    /// The project component lineage being authored; `None` on the start
    /// page and while a library-folder item is shown read-only (its
    /// lineage lives in a scratch engine, not the project).
    pub fn lineage(&self) -> Option<DefinitionId> {
        if self.is_read_only() {
            return None;
        }
        self.lineage
    }

    /// Author a component lineage, drafting it from its latest revision when
    /// it has no draft yet. The lineage becomes the workspace's subject and
    /// the newest entry of its open history.
    pub fn open(&mut self, id: DefinitionId, engine: &Engine) {
        let same = self.lineage == Some(id) && !self.is_read_only();
        self.identity.visit(identity::Subject::Project(id));
        if !same {
            self.leave_subject();
        }
        if engine.drafts().get(id).is_none()
            && let Some(published) = engine
                .project()
                .library
                .latest_components()
                .into_iter()
                .find(|c| c.id == id)
        {
            self.request = Some(Request::Draft(ComponentRef::new(id, published.revision)));
        }
        self.lineage = Some(id);
        self.source = None;
        self.view = View::Details;
    }

    /// Open the component lineage `id` and draft a new Part of it rated at
    /// `values` (Resolve parts' New Part…).
    pub fn new_part_for(
        &mut self,
        id: DefinitionId,
        values: Vec<(pcbcraft_core::FieldRef, pcbcraft_core::fields::FieldValue)>,
        engine: &Engine,
    ) {
        self.open(id, engine);
        self.parts.request_new(values);
    }

    /// Return the 2D views to their fitted scale.
    pub fn fit(&mut self) {
        self.footprint_view = None;
        self.symbol_view = None;
    }

    fn publish(&mut self, engine: &Engine) -> Option<Operation> {
        let set = self.draft.as_ref()?;
        let committed = engine.drafts();
        let pending_refs = set.retargeted(committed) != *set;
        // Part drafts publish with the cascade, pinning the pending revisions.
        let parts = match part_props::Pins::pending(set, committed) {
            Some(pins) => self.parts.retarget_edits(engine, set.ids.component, &pins),
            None => Vec::new(),
        };
        if self.draft != self.baseline || pending_refs || !parts.is_empty() {
            self.publish_next = true;
            let mut operation = self.send(engine);
            if let Some(Operation::Library(LibraryCommand::Edit { edits })) = &mut operation {
                edits.extend(parts);
            }
            return operation;
        }
        let publish = self.publish_set(engine)?;
        self.publishing = true;
        self.awaiting = true;
        Some(publish)
    }

    /// The working set's five drafts and its Part drafts, as one publish set.
    fn publish_set(&self, engine: &Engine) -> Option<Operation> {
        let set = self.draft.as_ref()?;
        let mut publish = set.publish();
        if let Operation::Library(LibraryCommand::Publish { drafts }) = &mut publish {
            drafts.extend(part_props::PartProps::draft_ids(engine, set.ids.component));
        }
        Some(publish)
    }

    /// Replace the working copy with a template; the next commit makes it
    /// one undoable step. Without a working set the app creates the lineages.
    /// The ribbon no longer offers this; tests drive templates with it.
    #[cfg(test)]
    fn load_template(&mut self, template: Template) {
        match &mut self.draft {
            Some(set) => {
                set.load_template(template);
                self.selected = set.component.terminals.first().map(|t| t.id);
                self.body = layout_body(set);
            }
            None => self.create_new(template),
        }
        self.footprint_view = None;
        self.selected_pad = None;
        self.symbol_view = None;
        self.selection.clear();
        self.unit = None;
    }

    /// Add a terminal with its pin on the first unit's left edge, growing
    /// the body to meet it.
    fn add_terminal(&mut self) {
        let Some(electrical) = self.new_terminal() else {
            return;
        };
        let Some(set) = &mut self.draft else {
            return;
        };
        let row = set.component.terminals.len().saturating_sub(1) as f64;
        let tracked = self.body;
        if let Some(unit) = set.symbol.units.first_mut() {
            let position = [-2.0 * SYMBOL_MODULE_MM, -row * SYMBOL_MODULE_MM];
            // Grow the tracked body only; without it nothing is resized and
            // the pin faces what is drawn.
            let body = tracked
                .filter(|(u, _)| *u == unit.id)
                .and_then(|(_, body)| grow_body_to(unit, body, position[1]))
                .or_else(|| {
                    unit.graphics
                        .iter()
                        .filter_map(|g| match &g.item {
                            Item::Shape { shape, .. } => Some(shape.bounds(0.0)),
                            Item::Text(_) => None,
                        })
                        .reduce(Bounds::union)
                        .filter(|b| !b.contains(position))
                });
            let pin = match body {
                Some(body) => graphics::pin_toward(electrical, position, body),
                None => Pin::new(electrical, position, PinDirection::Right, SYMBOL_MODULE_MM),
            };
            unit.pins.push(pin);
        }
    }

    /// Add a passive terminal with its package terminal, pad and mapping,
    /// but no pin; the first terminal of an empty symbol also gets a first
    /// unit. Returns the terminal, or `None` with a notice at a limit.
    fn new_terminal(&mut self) -> Option<TerminalId> {
        let set = self.draft.as_mut()?;
        if set.component.terminals.len() >= pcbcraft_core::library::MAX_TERMINALS {
            let most = pcbcraft_core::library::MAX_TERMINALS;
            self.tell(
                Level::Error,
                format!("No terminal added: a component holds at most {most}"),
            );
            return None;
        }
        let allocate = |next: &mut u32| {
            let id = *next;
            next.checked_add(1).map(|n| {
                *next = n;
                id
            })
        };
        let electrical = allocate(&mut set.component.next_local);
        let function = allocate(&mut set.component.next_local);
        let physical = allocate(&mut set.package.next_local);
        let pad = allocate(&mut set.footprint.next_local);
        let (Some(electrical), Some(function), Some(physical), Some(pad)) =
            (electrical, function, physical, pad)
        else {
            self.tell(Level::Error, NO_MORE_TERMINALS);
            return None;
        };
        if set.symbol.units.is_empty() {
            let (Some(unit), Some(body), Some(designator), Some(name)) = (
                allocate(&mut set.symbol.next_local),
                allocate(&mut set.symbol.next_local),
                allocate(&mut set.symbol.next_local),
                allocate(&mut set.symbol.next_local),
            ) else {
                self.tell(Level::Error, NO_MORE_TERMINALS);
                return None;
            };
            set.symbol.pin_text_height = PIN_TEXT_HEIGHT_MM;
            set.symbol.units.push(first_unit(
                UnitId(unit),
                [GraphicId(body), GraphicId(designator), GraphicId(name)],
            ));
            self.body = Some((UnitId(unit), GraphicId(body)));
        }
        let row = set.component.terminals.len() as f64;
        let electrical = TerminalId(electrical);
        let physical = PackageTerminalId(physical);
        // Named after its pin number: the next number no terminal or
        // package terminal uses yet, not the internal ID.
        let taken = |n: &String| {
            set.package.terminals.iter().any(|t| t.designation == *n)
                || set.component.terminals.iter().any(|t| t.name == *n)
        };
        let number = (set.package.terminals.len() + 1..)
            .map(|n| n.to_string())
            .find(|n| !taken(n))
            .unwrap_or_else(|| physical.to_string());
        let chip = set.package.chip_kind().is_some() && set.package.terminals.len() >= 2;
        set.component.terminals.push(Terminal {
            id: electrical,
            name: number.clone(),
            class: TerminalClass::Electrical(TerminalFunctions::fixed(
                FunctionId(function),
                Direction::Passive,
            )),
        });
        set.footprint.pads.push(Pad::smd_rect(
            PadId(pad),
            physical,
            number.clone(),
            [0.0, row * 2.0],
            [1.0, 1.4],
        ));
        set.package.terminals.push(PackageTerminal {
            id: physical,
            designation: number,
        });
        set.binding.mappings.push(TerminalMap {
            package_terminal: physical,
            terminal: electrical,
        });
        self.selected = Some(electrical);
        if chip {
            self.tell(
                Level::Warning,
                "A chip body has two terminals: change Body on the 3D tab or remove this one",
            );
        }
        Some(electrical)
    }

    fn remove_selected(&mut self) {
        let (Some(selected), Some(set)) = (self.selected, &mut self.draft) else {
            return;
        };
        if set.component.terminals.len() <= 1 {
            self.tell(Level::Error, "A component needs at least one terminal");
            return;
        }
        let physical: Vec<_> = set
            .binding
            .mappings
            .iter()
            .filter(|m| m.terminal == selected)
            .map(|m| m.package_terminal)
            .collect();
        set.component.terminals.retain(|t| t.id != selected);
        for unit in &mut set.symbol.units {
            unit.pins.retain(|p| p.terminal != selected);
        }
        set.binding.mappings.retain(|m| m.terminal != selected);
        set.package.terminals.retain(|t| !physical.contains(&t.id));
        set.footprint
            .pads
            .retain(|p| p.terminal.is_none_or(|t| !physical.contains(&t)));
        self.selected = set.component.terminals.first().map(|t| t.id);
    }

    /// Add a pad of `which`: copper pads land the selected terminal's
    /// package terminal when no pad lands it yet (otherwise none, which
    /// publishing reports); holes and fiducials land none. Numbers never
    /// repeat an existing pad's. The pad goes to the right of the existing
    /// lands and becomes the selected pad.
    fn add_pad(&mut self, which: footprint_geometry::NewPad) {
        let Some(set) = &mut self.draft else {
            return;
        };
        if set.footprint.source.is_some() {
            self.tell(
                Level::Info,
                "Pads come from the footprint sketch: add them there",
            );
            return;
        }
        let terminal = set
            .binding
            .mappings
            .iter()
            .find(|m| Some(m.terminal) == self.selected)
            .map(|m| m.package_terminal)
            .filter(|t| !set.footprint.pads.iter().any(|p| p.terminal == Some(*t)));
        if set.footprint.pads.len() >= pcbcraft_core::library::MAX_PADS {
            let most = pcbcraft_core::library::MAX_PADS;
            self.tell(
                Level::Error,
                format!("No pad added: a footprint holds at most {most}"),
            );
            return;
        }
        let id = set.footprint.next_local;
        let Some(next) = id.checked_add(1) else {
            self.tell(Level::Error, "No more pads can be added to this footprint");
            return;
        };
        set.footprint.next_local = next;
        let used = |n: &str| set.footprint.pads.iter().any(|p| p.number == n);
        let designation = terminal
            .filter(|_| which.lands_terminal())
            .and_then(|terminal| set.package.terminals.iter().find(|t| t.id == terminal))
            .map(|t| t.designation.clone())
            .filter(|d| !d.is_empty() && !used(d));
        let number = match designation {
            Some(designation) => designation,
            None if which.lands_terminal() => (1..)
                .map(|n: usize| n.to_string())
                .find(|n| !used(n))
                .unwrap_or_default(),
            None => {
                let same = set
                    .footprint
                    .pads
                    .iter()
                    .filter(|p| p.kind == which.kind())
                    .count();
                format!("{}{}", which.prefix(), same + 1)
            }
        };
        let pad = footprint_geometry::new_pad(
            which,
            PadId(id),
            terminal,
            number,
            footprint_geometry::free_spot(&set.footprint.pads),
        );
        set.footprint.pads.push(pad);
        self.selected_pad = Some(PadId(id));
    }

    /// Remove the selected pad. Its package terminal stays; a terminal left
    /// without pads is reported when publishing.
    fn remove_pad(&mut self) {
        let Some(set) = &mut self.draft else {
            return;
        };
        if set.footprint.source.is_some() {
            return;
        }
        let Some(id) = footprint_props::current_pad(
            &set.footprint,
            &set.binding,
            self.selected_pad,
            self.selected,
        ) else {
            return;
        };
        set.footprint.pads.retain(|p| p.id != id);
        if set.footprint.polarity.as_ref().is_some_and(|p| p.pad == id) {
            set.footprint.polarity = None;
        }
        self.selected_pad = None;
    }

    /// The Properties card's frame for the current subject: its header,
    /// the identity whose change resets the scroll, whether it has a pinned
    /// footer, and its issue slot (docs/sidebar-ia.md, "Contract"). The
    /// app's column draws it with [`kit::card::card`] and asks
    /// [`Self::inspector_region`] for the body and footer.
    fn inspector_card_inner(&mut self, engine: &Engine) -> kit::card::Inspector {
        self.sync(engine);
        let inspected = self.inspected();
        kit::card::Inspector {
            header: self.header(engine),
            subject: egui::Id::new(("component-card", self.lineage, inspected)),
            footer: self.has_footer(engine, inspected),
            issues: self.issue_slot(engine, inspected),
        }
    }

    /// One region of the Properties card. The body is the only scroll;
    /// the footer holds object actions (Remove pin), never Apply.
    fn inspector_region_inner(
        &mut self,
        ui: &mut Ui,
        engine: &Engine,
        region: kit::card::Region,
    ) -> Option<Operation> {
        self.sync(engine);
        self.flush(ui.ctx());
        let inspected = self.inspected();
        match region {
            kit::card::Region::Body => self.card_body(ui, engine, inspected),
            kit::card::Region::Footer => self.card_footer(ui, engine, inspected),
        }
    }

    /// The whole subject slot for the right column, exactly one of: the
    /// docked instance review, the Part dialog, the Footprint tab's
    /// template or Assign dialog, the armed tool's card
    /// ([`kit::tool_card`]), or the Properties card with its issue slot and
    /// pinned footer (docs/sidebar-ia.md, "Layout").
    fn subject_slot_inner(
        &mut self,
        ui: &mut Ui,
        engine: &Engine,
    ) -> kit::card::CardResponse<Operation> {
        self.sync(engine);
        self.flush(ui.ctx());
        if let Some(operation) = self.slot_subject(ui, engine) {
            return kit::card::CardResponse::output(operation);
        }
        let inspector = self.inspector_card(engine);
        kit::card::card(ui, &inspector, |ui, region| {
            self.inspector_region(ui, engine, region)
        })
    }

    /// What the subject slot holds: the review, a dialog (the Part dialog,
    /// or the Footprint tab's template or Assign dialog), the armed tool's
    /// card, or Properties.
    pub fn subject_kind(&self) -> kit::card::Subject {
        use kit::card::Subject;
        if self.update.is_open() {
            Subject::Review
        } else if self.parts.is_open() {
            Subject::Dialog
        } else if self.draft.is_some() && self.view == View::Footprint {
            self.footprint_subject()
        } else if matches!(self.inspected(), Inspected::Tool(_)) {
            Subject::Tool
        } else {
            Subject::Properties
        }
    }

    /// A subject that fills the slot without a Properties card: `Some`
    /// with the operation it sends this frame, `None` for the card.
    fn slot_subject(&mut self, ui: &mut Ui, engine: &Engine) -> Option<Option<Operation>> {
        use kit::card::Subject;
        if self.update.is_open() {
            return Some(self.update.show(ui, engine));
        }
        // It also drafts a New Part requested from the schematic.
        if let Some(operation) = self.part_dialog(ui, engine) {
            return Some(operation);
        }
        match (self.subject_kind(), self.view) {
            (Subject::Dialog, _) => {
                self.post_footprint_refusals(ui.ctx());
                Some(if self.template_picker.is_open() {
                    self.template_dialog(ui)
                } else {
                    self.assign_dialog(ui)
                })
            }
            (Subject::Tool, View::Footprint) => {
                self.post_footprint_refusals(ui.ctx());
                self.footprint_tool_card(ui);
                Some(None)
            }
            (Subject::Tool, _) => {
                let Inspected::Tool(tool) = self.inspected() else {
                    return None;
                };
                let key = tool_key(tool);
                let card = kit::ToolCard {
                    icon: tool.icon(),
                    name: tool.label(),
                    key: &key,
                    hint: tool.tip(),
                    tip: "",
                    finish: tool.is_chain().then_some("Finish"),
                };
                let response = kit::tool_card(ui, &card, |ui| self.tool_rows(ui, engine));
                self.tool_card_response(response);
                Some(None)
            }
            (Subject::Properties | Subject::Review, _) => None,
        }
    }

    /// The subject slot in one body, for tests that drive the card without
    /// the column: a slot subject as the slot draws it, else the card's
    /// body, then its issue slot and footer actions.
    #[cfg(test)]
    pub fn inspector(&mut self, ui: &mut Ui, engine: &Engine) -> Option<Operation> {
        self.sync(engine);
        self.flush(ui.ctx());
        if let Some(operation) = self.slot_subject(ui, engine) {
            return operation;
        }
        let inspector = self.inspector_card(engine);
        let operation = self.inspector_region(ui, engine, kit::card::Region::Body);
        let slot = inspector.issues.unwrap_or_default();
        kit::issue_line(ui, &slot.counts, &slot.label);
        let mut footer = None;
        if inspector.footer {
            kit::rule(ui);
            kit::actions(ui, |ui| {
                footer = self.inspector_region(ui, engine, kit::card::Region::Footer);
            });
        }
        operation.or(footer)
    }

    /// The card body for `inspected`.
    fn card_body(
        &mut self,
        ui: &mut Ui,
        engine: &Engine,
        inspected: Inspected,
    ) -> Option<Operation> {
        match inspected {
            // These fill the slot without the card ([`Self::slot_subject`]).
            Inspected::Review | Inspected::Tool(_) => None,
            Inspected::Empty => {
                kit::hint(
                    ui,
                    "Open a component, or start a new one, on the start page",
                    "Search finds this project's components and every library source",
                );
                None
            }
            Inspected::Tab(View::Details) => self.details_card(ui, engine),
            Inspected::Tab(View::Symbol) => self.symbol_card(ui, engine),
            Inspected::Tab(View::Pins) => self.pins_card(ui, engine),
            Inspected::Tab(View::Footprint) => self.footprint_card(ui),
            Inspected::Tab(View::Mapping) => {
                self.mapping_card(ui);
                None
            }
            Inspected::Tab(View::Model) => self.model_card(ui, engine),
            Inspected::Pin(terminal) => {
                self.pin_card(ui, terminal);
                None
            }
            Inspected::Terminal(terminal) => {
                self.terminal_without_pin_card(ui, terminal);
                None
            }
            Inspected::Graphic(graphic) => {
                self.graphic_card(ui, engine, graphic);
                None
            }
            Inspected::Selection(count) => {
                kit::section(
                    ui,
                    "symbol-selection",
                    "Selection",
                    true,
                    Some(count),
                    |ui| {
                        kit::value_row(ui, "Objects", &count.to_string());
                        kit::hint(
                            ui,
                            "Drag to move; Rotate, Mirror and Duplicate act on all",
                            "Shift+click adds or removes an object; Esc clears the selection",
                        );
                    },
                );
                None
            }
        }
    }

    /// Whether `inspected` has a pinned footer of object actions.
    fn has_footer(&self, engine: &Engine, inspected: Inspected) -> bool {
        match inspected {
            Inspected::Tab(View::Details) => !self.update_candidates(engine).is_empty(),
            Inspected::Tool(_)
            | Inspected::Pin(_)
            | Inspected::Terminal(_)
            | Inspected::Graphic(_)
            | Inspected::Selection(_) => true,
            _ => false,
        }
    }

    /// The footer's actions for `inspected`, primary first (the footer is
    /// laid out right to left, so the primary ends up last).
    fn card_footer(
        &mut self,
        ui: &mut Ui,
        engine: &Engine,
        inspected: Inspected,
    ) -> Option<Operation> {
        use solidcraft_ui_egui::ui_kit::{self, Button};
        let remove = |ui: &mut Ui, label: &str, tip: &str| {
            ui_kit::button(ui, label, Button::Normal)
                .on_hover_text(tip)
                .clicked()
        };
        match inspected {
            Inspected::Tab(View::Details) => {
                let candidates = self.update_candidates(engine);
                if ui_kit::button(ui, "Update placed instances…", Button::Normal)
                    .on_hover_text(format!(
                        "Review moving {} placed {} to the published revision",
                        candidates.len(),
                        if candidates.len() == 1 {
                            "instance"
                        } else {
                            "instances"
                        }
                    ))
                    .clicked()
                {
                    self.begin_instance_update(engine, candidates);
                }
            }
            Inspected::Tool(tool) => {
                // The tool card's own buttons, for a column that draws the
                // header itself: Done, and Cancel/Finish for a chain.
                let mut response = kit::ToolCardResponse::default();
                if tool.is_chain() {
                    let needed = if tool == drawing::Tool::Polygon { 3 } else { 2 };
                    let ready = self.drawing.pending().len() >= needed;
                    response.finish = kit::primary(ui, "Finish", ready)
                        .on_disabled_hover_text(format!("Click at least {needed} points first"))
                        .clicked();
                    response.cancel = ui_kit::button(ui, "Cancel", Button::Normal).clicked();
                }
                response.done = ui_kit::button(ui, "Done", Button::Normal)
                    .on_hover_text("Back to Select")
                    .clicked();
                self.tool_card_response(response);
            }
            Inspected::Pin(_) => {
                let clicked = remove(
                    ui,
                    "Remove pin",
                    "Remove the pin from this unit; its terminal stays",
                );
                self.delete_selection(engine, clicked);
            }
            Inspected::Terminal(terminal) => {
                let clicked = remove(
                    ui,
                    "Remove terminal",
                    "Remove the terminal everywhere in this component",
                );
                if clicked {
                    self.selected = Some(terminal);
                    self.terminal_card = None;
                    self.remove_selected();
                }
            }
            Inspected::Graphic(_) | Inspected::Selection(_) => {
                let clicked = remove(ui, "Remove", "Remove the selection from this unit (Delete)");
                self.delete_selection(engine, clicked);
            }
            _ => {}
        }
        None
    }

    /// Delete the Symbol selection when `clicked` (the footer's Remove).
    fn delete_selection(&mut self, engine: &Engine, clicked: bool) {
        if clicked {
            self.arrange(
                symbol_canvas::Arrange::Delete,
                engine.drafts().grid.symbol_mm,
            );
        }
    }

    /// The objects whose issues the card's slot counts, and the words after
    /// the counts.
    fn slot_objects(
        &self,
        engine: &Engine,
        inspected: Inspected,
    ) -> Option<(Vec<ObjectRef>, &'static str)> {
        let set = self.draft.as_ref()?;
        let ids = set.ids;
        let definition = |id| ObjectRef::Definition { id };
        let local = |definition, local| ObjectRef::LibraryObject { definition, local };
        Some(match inspected {
            Inspected::Empty | Inspected::Review | Inspected::Tool(_) => return None,
            Inspected::Tab(View::Details) => {
                let mut objects: Vec<_> = ids.all().into_iter().map(definition).collect();
                objects.extend(
                    part_props::PartProps::draft_ids(engine, ids.component)
                        .into_iter()
                        .map(definition),
                );
                (objects, "on this component")
            }
            Inspected::Tab(View::Symbol) => (vec![definition(ids.symbol)], "on this symbol"),
            Inspected::Tab(View::Pins) => (vec![definition(ids.component)], "on these pins"),
            Inspected::Tab(View::Footprint) => {
                (vec![definition(ids.footprint)], "on this footprint")
            }
            Inspected::Tab(View::Mapping) => (vec![definition(ids.binding)], "on the mapping"),
            Inspected::Tab(View::Model) => (vec![definition(ids.package)], "on this package"),
            Inspected::Pin(terminal) => (vec![local(ids.component, terminal.0)], "on this pin"),
            Inspected::Terminal(terminal) => {
                (vec![local(ids.component, terminal.0)], "on this terminal")
            }
            Inspected::Graphic(graphic) => (vec![local(ids.symbol, graphic.0)], "on this graphic"),
            Inspected::Selection(_) => (
                self.selection
                    .iter()
                    .map(|s| match s {
                        Selected::Pin(t) => local(ids.component, t.0),
                        Selected::Graphic(g) => local(ids.symbol, g.0),
                    })
                    .take(pcbcraft_core::issues::MAX_SCOPE_OBJECTS)
                    .collect(),
                "on the selection",
            ),
        })
    }

    /// The card's issue slot: the `project.issues` counts of the inspected
    /// objects, computed once per document state and subject. A failed
    /// query says so rather than read as "no issues".
    fn issue_slot(
        &mut self,
        engine: &Engine,
        inspected: Inspected,
    ) -> Option<kit::card::IssueSlot> {
        let (objects, label) = self.slot_objects(engine, inspected)?;
        let key = IssueKey {
            revision: engine.revision(),
            drafts: Arc::as_ptr(&engine.document().drafts) as usize,
            objects: objects.clone(),
        };
        if let Some((cached, slot)) = &self.issue_slot
            && *cached == key
        {
            return Some(slot.clone());
        }
        let slot = match engine.query(Query::Issues {
            scope: pcbcraft_core::issues::IssueScope::Selection { objects },
        }) {
            Ok(QueryResult::Issues(report)) => kit::card::IssueSlot {
                counts: report.counts,
                // A tab's slot says where its issues are; an object's
                // slot speaks of that object.
                label: if matches!(inspected, Inspected::Tab(_)) {
                    slot_label(&report, label)
                } else {
                    label.to_owned()
                },
            },
            _ => kit::card::IssueSlot {
                counts: Default::default(),
                label: "Issues not counted".to_owned(),
            },
        };
        self.issue_slot = Some((key, slot.clone()));
        Some(slot)
    }

    fn canvas_inner(&mut self, ui: &mut Ui, engine: &Engine) -> Option<Operation> {
        self.svg_frame(ui.ctx(), engine);
        if self.update.is_open() {
            ui.disable();
        }
        // The library, pins and 3D call sites, one module each.
        let sheet = self.gallery.show(ui.ctx(), engine);
        if let Some(id) = self.gallery.take_created() {
            // The gallery's create applied: open what it made.
            self.open(id, engine);
        }
        let chooser = self.model_chooser.show(ui.ctx(), engine);
        if sheet.is_some() || chooser.is_some() {
            return sheet.or(chooser);
        }
        // The identity bar is drawn above every view by the app
        // ([`Self::identity_strip`]), the 3D view included.
        if self.draft.is_none() && self.start_page(ui, engine) {
            return None;
        }
        if self.is_read_only() {
            // A library item: its views show, nothing on them edits.
            ui.disable();
        }
        if self.view == View::Model {
            ui.label("Native Solidcraft package preview");
            return None;
        }
        let (rect, response) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        if rect.width() < 1.0 || rect.height() < 1.0 {
            return None;
        }
        if self.view == View::Mapping {
            self.mapping_canvas(ui, rect);
            return None;
        }
        if self.view == View::Pins {
            return self.pins_canvas(ui, rect, engine);
        }
        if self.draft.is_none() {
            ui.put(rect, egui::Label::new("No component draft"));
            return None;
        }
        if self.view == View::Symbol {
            self.symbol_canvas(ui, rect, &response, engine);
            self.flush(ui.ctx());
            return self.grid_popover(ui, rect, engine);
        }
        if self.view == View::Details {
            // A preview of the symbol: nothing on it is picked or edited
            // from the Details tab.
            let preview = ui.interact(rect, ui.id().with("details-preview"), Sense::hover());
            ui.add_enabled_ui(false, |ui| {
                self.symbol_canvas(ui, rect, &preview, engine);
            });
            return None;
        }
        // A finished sketch gesture is sent by `commit`, after the frame.
        self.footprint_canvas(ui, rect, &response, engine.drafts().grid.footprint_mm);
        None
    }

    fn mapping_canvas(&mut self, ui: &mut Ui, rect: Rect) {
        let Some(set) = &self.draft else {
            return;
        };
        let t = Tokens::get();
        let painter = ui.painter().with_clip_rect(rect);
        let left = rect.left() + t.space_24;
        let right = rect.right() - t.space_24;
        painter.text(
            Pos2::new(left, rect.top() + t.space_16),
            Align2::LEFT_TOP,
            "Electrical terminal",
            fonts::semibold(t.font_panel),
            t.text,
        );
        painter.text(
            Pos2::new(right, rect.top() + t.space_16),
            Align2::RIGHT_TOP,
            "Package terminal / copper pads",
            fonts::semibold(t.font_panel),
            t.text,
        );
        let top = rect.top() + t.form_header_height + t.space_16;
        let mut clicked = None;
        for (index, physical) in set.package.terminals.iter().enumerate() {
            let y = top + index as f32 * (t.control_height + t.space_12);
            let electrical = set
                .binding
                .terminal_of(physical.id)
                .and_then(|id| set.component.terminal(id));
            // Only a mapped row is linked; an unmapped one reads "Unmapped".
            if let Some(electrical) = electrical {
                let ink = if Some(electrical.id) == self.selected {
                    t.body_selected
                } else {
                    t.accent
                };
                painter.line_segment(
                    [Pos2::new(left + 120.0, y), Pos2::new(right - 150.0, y)],
                    Stroke::new(2.0, ink),
                );
            }
            // Pads by their numbers, as the footprint shows them.
            let numbers: Vec<&str> = set
                .footprint
                .pads
                .iter()
                .filter(|p| p.terminal == Some(physical.id))
                .map(|p| p.number.as_str())
                .collect();
            let pad_ids = match numbers.as_slice() {
                [] => "no pad".to_owned(),
                [one] => format!("pad {one}"),
                many => format!("pads {}", many.join(", ")),
            };
            painter.text(
                Pos2::new(left, y),
                Align2::LEFT_CENTER,
                electrical.map(|e| e.name.as_str()).unwrap_or("Unmapped"),
                FontId::proportional(t.font_label),
                t.text,
            );
            painter.text(
                Pos2::new(right, y),
                Align2::RIGHT_CENTER,
                format!("{}  ·  {pad_ids}", physical.designation),
                FontId::proportional(t.font_label),
                t.text,
            );
            if ui
                .interact(
                    Rect::from_min_max(
                        Pos2::new(left, y - t.control_height / 2.0),
                        Pos2::new(right, y + t.control_height / 2.0),
                    ),
                    ui.id().with(("mapping-row", physical.id.0)),
                    Sense::click(),
                )
                .clicked()
            {
                clicked = Some(electrical.map(|e| e.id));
            }
        }
        if let Some(selected) = clicked {
            self.selected = selected;
        }
    }
}

/// Run a request through the engine with public commands. Each command is its
/// own undo step. Returns what the app should tell the author: opening a
/// published component for editing creates drafts, which are saved with the
/// project, so the project becomes modified and says why.
pub fn run(engine: &mut Engine, request: Request) -> pcbcraft_core::Result<Option<String>> {
    match request {
        #[cfg(test)]
        Request::Create(template) => working_set::create(engine, template).map(|_| None),
        Request::Draft(component) => {
            let created = working_set::draft_closure(engine, component)?;
            let name = engine
                .project()
                .library
                .component(component)
                .map(|c| c.meta.name.clone())
                .unwrap_or_default();
            Ok((created > 0).then(|| {
                // The toast shows the first line; Messages keeps both.
                let next = component
                    .revision
                    .next()
                    .map_or_else(String::new, |r| format!(" · {r} on publish"));
                format!(
                    "Draft created{next}\nDraft of {name} {}: edits are saved with the project until you publish or discard them",
                    component.revision
                )
            }))
        }
    }
}

/// A terminal or unit refused because the component ran out of local IDs.
const NO_MORE_TERMINALS: &str = "No more terminals can be added to this component";

/// What the Component card inspects (docs/sidebar-ia.md, "Properties card
/// by subject"). Its value identifies the subject, so the card's scroll
/// resets when it changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Inspected {
    /// No working set yet.
    Empty,
    /// The placed-instance update review.
    Review,
    /// A tab's document object: the component (Details), the symbol with
    /// nothing selected, the footprint, the mapping or the package.
    Tab(View),
    /// An armed Symbol drawing tool: its tool card.
    Tool(drawing::Tool),
    /// One selected pin, with its terminal's electrical properties.
    Pin(TerminalId),
    /// A terminal without a pin on the drawn unit.
    Terminal(TerminalId),
    /// One selected shape or text.
    Graphic(GraphicId),
    /// Several selected objects.
    Selection(usize),
}

/// What an issue slot was counted for.
#[derive(Clone, Debug, PartialEq)]
struct IssueKey {
    revision: u64,
    drafts: usize,
    objects: Vec<ObjectRef>,
}

/// A tool-table key as a registry chord.
fn tool_chord(key: egui::Key, shift: bool) -> crate::commands::Chord {
    let chord = crate::commands::Chord::key(key);
    if shift { chord.with_shift() } else { chord }
}

/// The Symbol tab's drawing and arrange tools for the command registry,
/// with the keys `symbol_keys` handles.
pub(crate) fn symbol_tool_entries() -> Vec<crate::commands::ToolEntry> {
    use crate::commands::{Chord, ToolEntry};
    use symbol_canvas::Arrange;
    let mut entries: Vec<ToolEntry> = drawing::Tool::ALL
        .into_iter()
        .map(|t| ToolEntry {
            id: t.id(),
            label: t.label(),
            icon: t.icon(),
            chord: t.key().map(|k| tool_chord(k.key, k.shift)),
        })
        .collect();
    entries.extend(Arrange::ALL.into_iter().map(|a| ToolEntry {
        id: a.id(),
        label: a.label(),
        icon: a.icon(),
        chord: Some(match a {
            Arrange::Rotate => Chord::cmd(egui::Key::R),
            Arrange::Mirror => Chord::key(egui::Key::X).with_shift(),
            Arrange::Duplicate => Chord::cmd(egui::Key::D),
            Arrange::Delete => Chord::key(egui::Key::Delete),
        }),
    }));
    entries
}

/// The Footprint tab's tools for the command registry, with the keys
/// `footprint_tool_keys` handles.
pub(crate) fn footprint_tool_entries() -> Vec<crate::commands::ToolEntry> {
    footprint_tools::FootprintTool::ALL
        .into_iter()
        .map(|t| {
            let (icon, label) = footprint_dims_ui::constrain_look(t)
                .map_or((t.icon(), t.label()), |(icon, label, _)| (icon, label));
            crate::commands::ToolEntry {
                id: t.id(),
                label,
                icon,
                chord: t.key().map(|k| tool_chord(k.key, k.shift)),
            }
        })
        .collect()
}

/// The key cap a tool card shows ("R", "⇧L").
fn tool_key(tool: drawing::Tool) -> String {
    tool.key()
        .map(|k| {
            let name = k.key.name();
            if k.shift {
                format!("\u{21e7}{name}")
            } else {
                name.to_owned()
            }
        })
        .unwrap_or_default()
}

/// Where one referenced object shows in the Component workspace.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Target {
    Tab(View),
    Terminal(TerminalId),
    Unit(UnitId),
    SymbolGraphic(UnitId, GraphicId),
    Pad(PadId),
    FootprintGraphic(GraphicId),
    PackageTerminal(PackageTerminalId),
}

impl Target {
    fn view(self) -> View {
        match self {
            Self::Tab(view) => view,
            Self::Terminal(_) | Self::Unit(_) | Self::SymbolGraphic(..) => View::Symbol,
            Self::Pad(_) | Self::FootprintGraphic(_) => View::Footprint,
            Self::PackageTerminal(_) => View::Mapping,
        }
    }
}

/// Where `object` shows in the working set `set`, if it belongs to it.
/// `parts` are the component's Part drafts (shown on Details).
fn target(set: &WorkingSet, parts: &[DefinitionId], object: &ObjectRef) -> Option<Target> {
    let ids = set.ids;
    match *object {
        ObjectRef::Definition { id } => Some(Target::Tab(match id {
            id if id == ids.component || parts.contains(&id) => View::Details,
            id if id == ids.symbol => View::Symbol,
            id if id == ids.footprint => View::Footprint,
            id if id == ids.binding => View::Mapping,
            id if id == ids.package => View::Model,
            _ => return None,
        })),
        ObjectRef::LibraryObject { definition, local } => {
            if definition == ids.component {
                let id = TerminalId(local);
                return set.component.terminal(id).map(|_| Target::Terminal(id));
            }
            if definition == ids.symbol {
                if let Some(unit) = set.symbol.units.iter().find(|u| u.id.0 == local) {
                    return Some(Target::Unit(unit.id));
                }
                return set.symbol.units.iter().find_map(|u| {
                    u.graphic(GraphicId(local))
                        .map(|g| Target::SymbolGraphic(u.id, g.id))
                });
            }
            if definition == ids.footprint {
                if let Some(pad) = set.footprint.pads.iter().find(|p| p.id.0 == local) {
                    return Some(Target::Pad(pad.id));
                }
                return set
                    .footprint
                    .graphics
                    .iter()
                    .any(|g| g.id.0 == local)
                    .then_some(Target::FootprintGraphic(GraphicId(local)));
            }
            if definition == ids.package {
                let id = PackageTerminalId(local);
                return match set.package.terminal(id) {
                    Some(_) => Some(Target::PackageTerminal(id)),
                    None => Some(Target::Tab(View::Model)),
                };
            }
            if definition == ids.binding {
                return Some(Target::Tab(View::Mapping));
            }
            parts
                .contains(&definition)
                .then_some(Target::Tab(View::Details))
        }
        _ => None,
    }
}

/// The words after the slot's counts: where the issues are when they all
/// sit in another definition than the tab's ("1 on the mapping" on the 3D
/// tab, for a package terminal with no mapping), else `label`.
fn slot_label(report: &pcbcraft_core::issues::IssueReport, label: &str) -> String {
    let place = |code: &str| match code.split('.').next().unwrap_or_default() {
        "binding" => Some("on the mapping"),
        "footprint" => Some("on the footprint"),
        "package" => Some("on the package"),
        "symbol" => Some("on the symbol"),
        "part" => Some("on the Part"),
        _ => None,
    };
    let mut places = report.rows.iter().map(|r| place(&r.group.code));
    match places.next() {
        Some(Some(first)) if places.all(|p| p == Some(first)) => {
            // "on this footprint" already says it on its own tab.
            let tab = label.rsplit(' ').next().unwrap_or_default();
            if first.ends_with(tab) {
                label.to_owned()
            } else {
                first.to_owned()
            }
        }
        _ => label.to_owned(),
    }
}

impl crate::issues::Locate for PartEditor {
    /// Show what an issue names: switch to the tab that shows the first
    /// object this component owns and select every object of that tab
    /// (pins and graphics on the Symbol tab, a pad on the Footprint tab).
    /// A definition of another component draft opens it. Selection and
    /// camera change only; the document never does.
    fn locate(&mut self, engine: &Engine, refs: &[ObjectRef]) -> bool {
        self.sync(engine);
        // An issue of another component's drafts opens that component.
        let own = |set: &WorkingSet, id: DefinitionId| set.ids.all().contains(&id);
        if let Some(other) = refs.iter().find_map(|r| match *r {
            ObjectRef::Definition { id } | ObjectRef::LibraryObject { definition: id, .. } => {
                Some(id)
            }
            _ => None,
        }) && self.draft.as_ref().is_none_or(|set| !own(set, other))
            && let Some(lineage) = component_of(engine, other)
            && Some(lineage) != self.lineage
        {
            self.open(lineage, engine);
            self.sync(engine);
        }
        let Some(set) = self.draft.as_ref() else {
            return false;
        };
        let parts = part_props::PartProps::draft_ids(engine, set.ids.component);
        let targets: Vec<Target> = refs.iter().filter_map(|r| target(set, &parts, r)).collect();
        let Some(first) = targets.first() else {
            return false;
        };
        let view = first.view();
        let targets: Vec<Target> = targets.into_iter().filter(|t| t.view() == view).collect();
        self.view = view;
        self.finding_graphic = None;
        // The Footprint, Mapping and 3D tabs locate through their own
        // targets (footprint_card.rs, mapping_card.rs, model_card.rs).
        match view {
            View::Footprint => return self.locate_footprint(refs),
            View::Mapping => return self.locate_mapping(refs),
            View::Model => return self.locate_model(refs),
            View::Details | View::Pins => return true,
            View::Symbol => {
                self.drawing.arm(drawing::Tool::Select);
                self.gesture = None;
            }
        }
        let Some(set) = self.draft.as_ref() else {
            return false;
        };
        let mut selection = arrange::Selection::new();
        let mut unit = None;
        let mut card = None;
        for target in targets {
            match target {
                Target::Tab(_) => {}
                Target::Terminal(terminal) => {
                    self.selected = Some(terminal);
                    let on = set
                        .symbol
                        .units
                        .iter()
                        .find(|u| u.pin(terminal).is_some())
                        .map(|u| u.id);
                    match on {
                        Some(on) if unit.is_none_or(|u| u == on) => {
                            unit = Some(on);
                            selection.insert(Selected::Pin(terminal));
                        }
                        Some(_) => {}
                        None => card = card.or(Some(terminal)),
                    }
                }
                Target::Unit(id) => unit = unit.or(Some(id)),
                Target::SymbolGraphic(on, graphic) => {
                    if unit.is_none_or(|u| u == on) {
                        unit = Some(on);
                        selection.insert(Selected::Graphic(graphic));
                    }
                }
                // Other tabs returned above.
                Target::Pad(_) | Target::FootprintGraphic(_) | Target::PackageTerminal(_) => {}
            }
        }
        {
            if unit.is_some()
                && unit != self.unit.or_else(|| set.symbol.units.first().map(|u| u.id))
            {
                self.symbol_view = None;
            }
            if let Some(unit) = unit {
                self.unit = Some(unit);
            }
            self.terminal_card = if selection.is_empty() { card } else { None };
            self.selection = selection;
            self.frame_selection();
        }
        true
    }
}

/// The component lineage whose working set holds the draft `id`, if any.
fn component_of(engine: &Engine, id: DefinitionId) -> Option<DefinitionId> {
    engine
        .drafts()
        .items
        .iter()
        .filter(|d| d.definition.kind() == pcbcraft_core::DefinitionKind::Component)
        .map(|d| d.id)
        .find(|component| {
            WorkingSet::find(engine.drafts(), Some(*component))
                .is_some_and(|set| set.ids.component == *component && set.ids.all().contains(&id))
        })
}

/// The first unit of a part built by hand: a one-module body with the
/// designator above it and the component name below it, as the templates
/// draw them. `ids` are the body, designator and name graphics.
fn first_unit(id: UnitId, ids: [GraphicId; 3]) -> Unit {
    let body = Bounds {
        min: [-SYMBOL_MODULE_MM, -SYMBOL_MODULE_MM],
        max: [SYMBOL_MODULE_MM, SYMBOL_MODULE_MM],
    };
    let gap = 0.5;
    Unit {
        id,
        label: "A".into(),
        graphics: vec![
            graphics::body_rect(ids[0], body),
            graphics::text(
                ids[1],
                TextContent::Designator,
                [body.center()[0], body.max[1] + gap],
                Justify {
                    h: HAlign::Center,
                    v: VAlign::Bottom,
                },
            ),
            graphics::text(
                ids[2],
                TextContent::ComponentName,
                [body.center()[0], body.min[1] - gap],
                Justify {
                    h: HAlign::Center,
                    v: VAlign::Top,
                },
            ),
        ],
        pins: vec![],
    }
}

/// The body rectangle a template or first unit lays out: the first unit's
/// first graphic when it is a body-filled rectangle.
fn layout_body(set: &WorkingSet) -> Option<(UnitId, GraphicId)> {
    let unit = set.symbol.units.first()?;
    let first = unit.graphics.first()?;
    matches!(
        first.item,
        Item::Shape {
            shape: Shape::Rect { .. },
            fill: Fill::Body,
            ..
        }
    )
    .then_some((unit.id, first.id))
}

/// Grow the body rectangle `body` downward so a pin row at `y` meets it one
/// module above its lower edge, moving text below the body along with it.
/// Returns the body, or `None` when the unit has no such rectangle.
fn grow_body_to(unit: &mut Unit, body: GraphicId, y: f64) -> Option<Bounds> {
    fn rect(g: &mut pcbcraft_core::library::Graphic) -> Option<(&mut Point, &mut Point)> {
        match &mut g.item {
            Item::Shape {
                shape: Shape::Rect { a, b, .. },
                ..
            } => Some((a, b)),
            _ => None,
        }
    }
    let (a, b) = unit
        .graphics
        .iter_mut()
        .find(|g| g.id == body)
        .and_then(rect)?;
    let bottom = a[1].min(b[1]);
    let wanted = y - SYMBOL_MODULE_MM;
    let shift = wanted - bottom;
    if shift < 0.0 {
        if a[1] <= b[1] {
            a[1] = wanted;
        } else {
            b[1] = wanted;
        }
        for graphic in &mut unit.graphics {
            if let Item::Text(text) = &mut graphic.item
                && text.at[1] < bottom
            {
                text.at[1] += shift;
            }
        }
    }
    unit.graphics
        .iter_mut()
        .find(|g| g.id == body)
        .and_then(rect)
        .and_then(|(a, b)| Bounds::of([*a, *b]))
}

/// One draft finding, as the tests read the engine's checks.
#[cfg(test)]
#[derive(Clone, Debug, PartialEq)]
struct Listed {
    text: String,
    /// It blocks publishing; otherwise it is advisory lint.
    blocking: bool,
    objects: Vec<pcbcraft_core::diagnostics::ObjectRef>,
}

#[cfg(test)]
impl PartEditor {
    /// Every finding of the committed component drafts, from the engine:
    /// blocking ones first, then the advisory lint. The card shows none of
    /// them (Issues does); tests read them to locate and to check wording.
    fn listed_findings(&self, engine: &Engine) -> Vec<Listed> {
        let Some(set) = &self.draft else {
            return Vec::new();
        };
        use pcbcraft_core::{Stage, completeness::CheckTarget};
        let Ok(QueryResult::Drafts(reports)) = engine.query(Query::CheckDrafts {}) else {
            return Vec::new();
        };
        let ids = set.ids.all();
        let mut listed: Vec<Listed> = reports
            .into_iter()
            .filter(|r| matches!(r.target, CheckTarget::Draft(id) if ids.contains(&id)))
            .flat_map(|r| r.findings)
            .map(|f| Listed {
                blocking: f.blocks_stage(Stage::Publish),
                text: f.message,
                objects: f.objects,
            })
            .collect();
        listed.sort_by_key(|f| !f.blocking);
        listed
    }

    /// Blocking publish findings of the committed drafts.
    fn findings(&self, engine: &Engine) -> Vec<String> {
        self.listed_findings(engine)
            .into_iter()
            .filter(|f| f.blocking)
            .map(|f| f.text)
            .collect()
    }
}

#[cfg(test)]
mod draft_tests {
    use super::details_card::set_field_default;
    use super::symbol_card::add_function;
    use super::*;
    use pcbcraft_core::fields::FieldValue;
    use pcbcraft_core::library::TextMetric;
    use pcbcraft_core::schematic;

    fn component_name(engine: &Engine, editor: &PartEditor) -> Option<String> {
        let id = editor.draft.as_ref()?.ids.component;
        engine.drafts().get(id).map(|d| d.meta.name.clone())
    }

    #[test]
    fn finished_edits_commit_once_and_engine_changes_reload_the_working_copy()
    -> pcbcraft_core::Result<()> {
        let ctx = egui::Context::default();
        let mut engine = Engine::demo()?;
        let mut editor = PartEditor::default();
        editor.open_first(&engine);
        assert!(
            editor.commit(&ctx, &engine).is_none(),
            "loading is not an edit"
        );
        let original = editor.draft.clone();
        assert!(original.is_some());
        if let Some(set) = &mut editor.draft {
            set.metas.component.name = "Edited".into();
        }
        let operation = editor.commit(&ctx, &engine);
        assert!(editor.commit(&ctx, &engine).is_none(), "sent once");
        if let Some(operation) = operation {
            engine.apply(engine.revision(), operation)?;
        }
        assert_eq!(component_name(&engine, &editor).as_deref(), Some("Edited"));
        engine.undo()?;
        assert!(editor.commit(&ctx, &engine).is_none());
        assert_eq!(editor.draft, original);
        Ok(())
    }

    #[test]
    fn a_part_built_terminal_by_terminal_lints_clean_with_pins_on_the_body()
    -> pcbcraft_core::Result<()> {
        let ctx = egui::Context::default();
        let mut engine = Engine::demo()?;
        let mut editor = PartEditor::default();
        editor.open_first(&engine);
        editor.commit(&ctx, &engine);
        if let Some(set) = &mut editor.draft {
            set.component.terminals.clear();
            set.symbol.units.clear();
            set.package.terminals.clear();
            set.footprint.pads.clear();
            set.binding.mappings.clear();
        }
        for _ in 0..4 {
            editor.add_terminal();
        }
        let symbol = editor
            .draft
            .as_ref()
            .map(|set| set.ids.symbol)
            .ok_or(pcbcraft_core::Error::Limit("no working set".into()))?;
        if let Some(operation) = editor.commit(&ctx, &engine) {
            engine.apply(engine.revision(), operation)?;
        }
        let draft = engine
            .drafts()
            .get(symbol)
            .ok_or(pcbcraft_core::Error::Limit("no symbol draft".into()))?;
        let findings = pcbcraft_core::library::lint(
            draft,
            &engine.project().library,
            &pcbcraft_core::library::Grid::default(),
        );
        assert!(findings.is_empty(), "{findings:?}");
        let pcbcraft_core::library::Definition::Symbol(definition) = &draft.definition else {
            return Err(pcbcraft_core::Error::Limit("not a symbol".into()));
        };
        let unit = &definition.units[0];
        assert_eq!(unit.pins.len(), 4);
        let body = unit
            .graphics
            .iter()
            .find_map(|g| match &g.item {
                Item::Shape {
                    shape: Shape::Rect { a, b, .. },
                    ..
                } => Bounds::of([*a, *b]),
                _ => None,
            })
            .ok_or(pcbcraft_core::Error::Limit("no body".into()))?;
        for pin in &unit.pins {
            let end = pin.body_end();
            assert!(
                (end[0] - body.min[0]).abs() < 1e-9 && end[1] > body.min[1] && end[1] < body.max[1],
                "pin {pin:?} ends off the body {body:?}"
            );
        }
        // The component name stays below the grown body.
        assert!(unit.graphics.iter().any(|g| matches!(
            &g.item,
            Item::Text(t) if t.content == TextContent::ComponentName && t.at[1] < body.min[1]
        )));
        Ok(())
    }

    #[test]
    fn loading_a_template_is_an_undoable_draft_change() -> pcbcraft_core::Result<()> {
        let ctx = egui::Context::default();
        let mut engine = Engine::demo()?;
        let mut editor = PartEditor::default();
        editor.open_first(&engine);
        editor.commit(&ctx, &engine);
        editor.load_template(Template::Header);
        if let Some(operation) = editor.commit(&ctx, &engine) {
            engine.apply(engine.revision(), operation)?;
        }
        assert_eq!(
            component_name(&engine, &editor).as_deref(),
            Some("Connector")
        );
        engine.undo()?;
        editor.commit(&ctx, &engine);
        assert_eq!(
            component_name(&engine, &editor).as_deref(),
            Some("Resistor")
        );
        Ok(())
    }

    #[test]
    fn publishing_the_working_set_is_one_cascade_through_public_commands()
    -> pcbcraft_core::Result<()> {
        let ctx = egui::Context::default();
        let mut engine = Engine::demo()?;
        let mut editor = PartEditor::default();
        editor.open_first(&engine);
        editor.commit(&ctx, &engine);
        assert!(
            !editor.unpublished(&engine),
            "the demo drafts are published"
        );
        if let Some(set) = &mut editor.draft {
            set.footprint.pads[0].position = [-1.5, 0.0];
        }
        if let Some(operation) = editor.commit(&ctx, &engine) {
            engine.apply(engine.revision(), operation)?;
        }
        editor.sync(&engine);
        assert!(editor.unpublished(&engine));
        let operation = editor.publish(&engine);
        assert!(matches!(
            operation,
            Some(Operation::Library(LibraryCommand::Publish { .. }))
        ));
        if let Some(operation) = operation {
            engine.apply(engine.revision(), operation)?;
        }
        editor.sync(&engine);
        assert!(!editor.unpublished(&engine));
        assert_eq!(engine.project().library.components.len(), 2);
        assert_eq!(engine.project().library.footprints.len(), 2);
        editor.flush(&ctx);
        let posted = crate::kit::toast::take_posted(&ctx);
        assert!(
            posted
                .iter()
                .any(|p| p.level == Level::Ok && p.text.starts_with("Published Resistor r2")),
            "{posted:?}"
        );
        Ok(())
    }

    /// B-d: opening a published component for editing drafts its lineages.
    /// Drafts are project state (saved and undoable), so the project really
    /// changes; the request says "Draft created" rather than hiding it, and
    /// opening it again creates and says nothing.
    #[test]
    fn opening_a_published_component_creates_drafts_and_says_so()
    -> Result<(), Box<dyn std::error::Error>> {
        let ctx = egui::Context::default();
        let mut engine = Engine::demo()?;
        let mut editor = PartEditor::default();
        editor.open_first(&engine);
        editor.commit(&ctx, &engine);
        let set = editor.draft.as_ref().ok_or("no working set")?;
        let component = set.ids.component;
        for id in set.ids.all() {
            if engine.drafts().get(id).is_some() {
                engine.apply(
                    engine.revision(),
                    Operation::Library(LibraryCommand::DiscardDraft { id }),
                )?;
            }
        }
        let published = engine
            .project()
            .library
            .latest_components()
            .into_iter()
            .find(|c| c.id == component)
            .ok_or("the demo component is published")?;
        let open = ComponentRef::new(component, published.revision);
        let before = engine.revision();
        let said = run(&mut engine, Request::Draft(open))?.ok_or("nothing said")?;
        assert!(engine.revision() > before, "a real, undoable change");
        assert!(engine.drafts().get(component).is_some());
        assert!(said.starts_with("Draft created · r"), "{said}");
        assert!(said.lines().next().is_some_and(|l| l.chars().count() <= 80));
        let after = engine.revision();
        assert_eq!(run(&mut engine, Request::Draft(open))?, None);
        assert_eq!(engine.revision(), after);
        Ok(())
    }

    #[test]
    fn a_new_component_is_created_from_a_template_in_an_empty_project() -> pcbcraft_core::Result<()>
    {
        let ctx = egui::Context::default();
        let mut engine = Engine::default();
        let mut editor = PartEditor::default();
        editor.open_first(&engine);
        editor.commit(&ctx, &engine);
        assert!(editor.draft.is_none());
        editor.load_template(Template::TwoTerminal);
        let request = editor.take_request();
        assert_eq!(request, Some(Request::Create(Template::TwoTerminal)));
        if let Some(request) = request {
            run(&mut engine, request)?;
        }
        editor.commit(&ctx, &engine);
        assert_eq!(
            component_name(&engine, &editor).as_deref(),
            Some("Resistor")
        );
        if let Some(operation) = editor.publish(&engine) {
            engine.apply(engine.revision(), operation)?;
        }
        assert_eq!(engine.project().library.components.len(), 1);
        Ok(())
    }

    fn resistance(editor: &PartEditor) -> Option<DefinitionId> {
        editor
            .draft
            .as_ref()?
            .component
            .fields
            .first()
            .map(|f| f.field.id)
    }

    #[test]
    fn a_field_default_commits_once_as_one_undoable_edit() -> pcbcraft_core::Result<()> {
        let ctx = egui::Context::default();
        let mut engine = Engine::demo()?;
        let mut editor = PartEditor::default();
        editor.open_first(&engine);
        editor.commit(&ctx, &engine);
        let lineage = resistance(&editor).ok_or(pcbcraft_core::Error::Limit("no field".into()))?;
        let original = editor.draft.clone();
        if let Some(set) = &mut editor.draft {
            let stored = set_field_default(
                &mut set.component,
                &engine.project().library,
                lineage,
                "10k",
            );
            assert!(stored.is_ok_and(|text| !text.is_empty()));
        }
        let operation = editor.commit(&ctx, &engine);
        assert!(matches!(
            operation,
            Some(Operation::Library(LibraryCommand::Edit { .. }))
        ));
        assert!(editor.commit(&ctx, &engine).is_none(), "sent once");
        if let Some(operation) = operation {
            engine.apply(engine.revision(), operation)?;
        }
        editor.commit(&ctx, &engine);
        let committed = engine
            .drafts()
            .get(
                editor
                    .lineage
                    .ok_or(pcbcraft_core::Error::Limit("no lineage".into()))?,
            )
            .and_then(|d| match &d.definition {
                pcbcraft_core::library::Definition::Component(c) => c.field(lineage).cloned(),
                _ => None,
            });
        assert!(matches!(
            committed.and_then(|f| f.default),
            Some(FieldValue::Quantity { si, .. }) if (si - 10_000.0).abs() < 1e-9
        ));
        engine.undo()?;
        editor.commit(&ctx, &engine);
        assert_eq!(editor.draft, original);
        Ok(())
    }

    #[test]
    fn invalid_default_text_leaves_the_draft_unchanged() -> pcbcraft_core::Result<()> {
        let ctx = egui::Context::default();
        let engine = Engine::demo()?;
        let mut editor = PartEditor::default();
        editor.open_first(&engine);
        editor.commit(&ctx, &engine);
        let lineage = resistance(&editor).ok_or(pcbcraft_core::Error::Limit("no field".into()))?;
        let original = editor.draft.clone();
        if let Some(set) = &mut editor.draft {
            let result = set_field_default(
                &mut set.component,
                &engine.project().library,
                lineage,
                "ten volts",
            );
            assert!(result.is_err());
        }
        assert_eq!(editor.draft, original);
        assert!(editor.commit(&ctx, &engine).is_none(), "nothing to commit");
        Ok(())
    }

    #[test]
    fn adding_a_function_allocates_a_fresh_id_and_commits() -> pcbcraft_core::Result<()> {
        let ctx = egui::Context::default();
        let mut engine = Engine::demo()?;
        let mut editor = PartEditor::default();
        editor.open_first(&engine);
        editor.commit(&ctx, &engine);
        let terminal = editor
            .selected
            .ok_or(pcbcraft_core::Error::Limit("no terminal".into()))?;
        let Some(set) = &mut editor.draft else {
            return Err(pcbcraft_core::Error::Limit("no draft".into()));
        };
        let next = set.component.next_local;
        let added = add_function(&mut set.component, terminal);
        assert_eq!(added, Ok(FunctionId(next)));
        assert_eq!(set.component.next_local, next + 1);
        let table = set
            .component
            .terminal(terminal)
            .and_then(Terminal::functions);
        assert_eq!(table.map(|t| t.functions.len()), Some(2));
        let labels: Vec<_> = table
            .map(|t| t.functions.iter().map(|f| f.label.clone()).collect())
            .unwrap_or_default();
        assert_ne!(labels[0], labels[1], "labels stay unique");
        if let Some(operation) = editor.commit(&ctx, &engine) {
            engine.apply(engine.revision(), operation)?;
        }
        editor.commit(&ctx, &engine);
        let committed = engine
            .drafts()
            .get(
                editor
                    .lineage
                    .ok_or(pcbcraft_core::Error::Limit("no lineage".into()))?,
            )
            .and_then(|d| match &d.definition {
                pcbcraft_core::library::Definition::Component(c) => c
                    .terminal(terminal)
                    .and_then(Terminal::functions)
                    .map(|f| f.functions.len()),
                _ => None,
            });
        assert_eq!(committed, Some(2));
        let findings = editor.findings(&engine);
        assert!(
            findings.is_empty(),
            "a second passive function is valid: {findings:?}"
        );
        Ok(())
    }

    /// Run one inspector frame. Tests have no renderer, so the texture
    /// deltas are cleared rather than applied.
    fn frame(
        editor: &mut PartEditor,
        engine: &Engine,
        ctx: &egui::Context,
        events: Vec<egui::Event>,
    ) -> Vec<(String, Pos2)> {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(360.0, 2400.0))),
                events,
                ..Default::default()
            },
            |ui| {
                editor.inspector(ui, engine);
            },
        );
        output.textures_delta.clear();
        let mut out = Vec::new();
        fn texts(shape: &egui::Shape, out: &mut Vec<(String, Pos2)>) {
            match shape {
                egui::Shape::Text(text) => out.push((
                    text.galley.job.text.clone(),
                    text.pos + text.galley.size() * 0.5,
                )),
                egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| texts(s, out)),
                _ => {}
            }
        }
        for shape in &output.shapes {
            texts(&shape.shape, &mut out);
        }
        out
    }

    /// Run one canvas frame in a 600 × 400 area; returns the drawn texts.
    fn canvas_frame(
        editor: &mut PartEditor,
        engine: &Engine,
        ctx: &egui::Context,
        events: Vec<egui::Event>,
    ) -> Vec<String> {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(600.0, 400.0))),
                events,
                ..Default::default()
            },
            |ui| {
                editor.canvas(ui, engine);
            },
        );
        output.textures_delta.clear();
        let mut out = Vec::new();
        fn texts(shape: &egui::Shape, out: &mut Vec<String>) {
            match shape {
                egui::Shape::Text(text) => out.push(text.galley.job.text.clone()),
                egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| texts(s, out)),
                _ => {}
            }
        }
        for shape in &output.shapes {
            texts(&shape.shape, &mut out);
        }
        out
    }

    /// The Symbol view draws what core resolves for the working set and
    /// picks pins through the core hit test; a dragged pin end lands on the
    /// symbol grid.
    #[test]
    fn the_symbol_view_draws_and_picks_through_core_queries()
    -> Result<(), Box<dyn std::error::Error>> {
        let ctx = egui::Context::default();
        fonts::setup_bundled_fonts(&ctx);
        let engine = Engine::demo()?;
        let mut editor = PartEditor::default();
        editor.open_first(&engine);
        editor.commit(&ctx, &engine);
        editor.on_tab("symbol");
        let shown = canvas_frame(&mut editor, &engine, &ctx, vec![]);
        // The designator placeholder reads prefix + "?"; the shown field
        // reads its name (no default). A two-terminal passive hides its pin
        // numbers.
        for text in ["R?", "Resistor", "Resistance"] {
            assert!(shown.iter().any(|t| t == text), "{text} in {shown:?}");
        }
        assert!(!shown.iter().any(|t| t == "1" || t == "2"), "{shown:?}");
        // The view fits the drawn bounds about the origin (3 mm margin).
        let set = editor.draft.as_ref().ok_or("no draft")?;
        let bounds = schematic::definition_unit(
            &engine.project().library,
            schematic::DefinitionUnit {
                symbol: &set.symbol,
                unit: set.symbol.units[0].id,
                component: &set.component,
                component_name: &set.metas.component.name,
                physical: Some((&set.binding, &set.package)),
            },
        )?
        .bounds(TextMetric::default())
        .ok_or("nothing drawn")?;
        let half = |lo: f64, hi: f64| lo.abs().max(hi.abs());
        let scale = (600.0 / (2.0 * half(bounds.min[0], bounds.max[0]) + 6.0))
            .min(400.0 / (2.0 * half(bounds.min[1], bounds.max[1]) + 6.0))
            as f32;
        let at = |p: Point| Pos2::new(300.0 + p[0] as f32 * scale, 200.0 - p[1] as f32 * scale);
        let button = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        // Click pin 2's connection end.
        let end = at([5.0, 0.0]);
        for events in [
            vec![egui::Event::PointerMoved(end)],
            vec![button(end, true)],
            vec![button(end, false)],
        ] {
            canvas_frame(&mut editor, &engine, &ctx, events);
        }
        assert_eq!(editor.selected, Some(TerminalId(2)));
        // Drag pin 1's end to (−6.6, 1.4) mm: it lands on (−7.5, 2.5).
        let (from, to) = (at([-5.0, 0.0]), at([-6.6, 1.4]));
        for events in [
            vec![egui::Event::PointerMoved(from)],
            vec![button(from, true)],
            vec![egui::Event::PointerMoved(from + (to - from) * 0.5)],
            vec![egui::Event::PointerMoved(to)],
            vec![button(to, false)],
        ] {
            canvas_frame(&mut editor, &engine, &ctx, events);
        }
        assert_eq!(editor.selected, Some(TerminalId(1)));
        let set = editor.draft.as_ref().ok_or("no draft")?;
        let pin = set.symbol.units[0]
            .pin(TerminalId(1))
            .ok_or("pin 1 missing")?;
        assert_eq!(pin.position, [-7.5, 2.5]);
        Ok(())
    }

    /// Click the text `label`, type `typed`, then leave the field.
    fn type_into(
        editor: &mut PartEditor,
        engine: &Engine,
        ctx: &egui::Context,
        label: &str,
        typed: &str,
    ) -> Result<Vec<(String, Pos2)>, String> {
        let shown = frame(editor, engine, ctx, vec![]);
        let point = shown
            .iter()
            .find(|(text, _)| text == label)
            .map(|(_, p)| *p)
            .ok_or_else(|| format!("missing {label}"))?;
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
        let typed = frame(editor, engine, ctx, vec![egui::Event::Text(typed.into())]);
        let key = egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        frame(editor, engine, ctx, vec![key]);
        Ok(typed)
    }

    #[test]
    fn typing_a_default_in_the_fields_section_commits_it() -> Result<(), Box<dyn std::error::Error>>
    {
        let ctx = egui::Context::default();
        fonts::setup_bundled_fonts(&ctx);
        let mut engine = Engine::demo()?;
        let mut editor = PartEditor::default();
        editor.open_first(&engine);
        editor.commit(&ctx, &engine);
        let lineage = resistance(&editor).ok_or("no field")?;
        type_into(&mut editor, &engine, &ctx, "No default", "10k")?;
        let operation = editor.commit(&ctx, &engine).ok_or("no edit")?;
        engine.apply(engine.revision(), operation)?;
        editor.commit(&ctx, &engine);
        let default = editor
            .draft
            .as_ref()
            .and_then(|s| s.component.field(lineage))
            .and_then(|f| f.default.clone());
        assert!(matches!(
            default,
            Some(FieldValue::Quantity { si, .. }) if (si - 10_000.0).abs() < 1e-9
        ));
        Ok(())
    }

    /// The Designator row edits the component's prefix like the Name row:
    /// letters are uppercased as typed, the edit commits through
    /// `library.edit`, and a prefix with a digit is a publish finding.
    #[test]
    fn the_designator_row_edits_the_prefix() -> Result<(), Box<dyn std::error::Error>> {
        let ctx = egui::Context::default();
        fonts::setup_bundled_fonts(&ctx);
        let mut engine = Engine::demo()?;
        let mut editor = PartEditor::default();
        editor.open_first(&engine);
        editor.commit(&ctx, &engine);
        let prefix = |editor: &PartEditor| {
            editor
                .draft
                .as_ref()
                .map(|s| s.component.designator_prefix.clone())
        };
        assert_eq!(prefix(&editor).as_deref(), Some("R"));
        type_into(&mut editor, &engine, &ctx, "R", "n")?;
        let typed = prefix(&editor).ok_or("no draft")?;
        assert!(typed == "RN" || typed == "NR", "{typed}");
        let operation = editor.commit(&ctx, &engine).ok_or("no edit")?;
        engine.apply(engine.revision(), operation)?;
        editor.commit(&ctx, &engine);
        let id = editor.draft.as_ref().ok_or("no draft")?.ids.component;
        let stored = match engine.drafts().get(id).map(|d| &d.definition) {
            Some(pcbcraft_core::library::Definition::Component(c)) => c.designator_prefix.clone(),
            _ => return Err("no component draft".into()),
        };
        assert_eq!(stored, typed);
        // The finding is an issue of the component: Issues lists it and the
        // Details card's slot counts it; the card never repeats its text.
        let finding = "A component needs a designator prefix of 1–8 uppercase letters A–Z, such as R, C or U.";
        let listed = |engine: &Engine| -> Result<bool, Box<dyn std::error::Error>> {
            let scope = pcbcraft_core::issues::IssueScope::Area {
                area: pcbcraft_core::issues::IssueArea::Component,
            };
            match engine.query(Query::Issues { scope })? {
                QueryResult::Issues(report) => Ok(report.rows.iter().any(|r| {
                    r.finding.message == finding
                        && r.blocks() == Some(pcbcraft_core::Stage::Publish)
                })),
                _ => Err("not an issue report".into()),
            }
        };
        assert!(!listed(&engine)?);
        let before = editor.inspector_card(&engine).issues.ok_or("slot")?.counts;
        type_into(&mut editor, &engine, &ctx, &typed, "1")?;
        let operation = editor.commit(&ctx, &engine).ok_or("no edit")?;
        engine.apply(engine.revision(), operation)?;
        let shown = frame(&mut editor, &engine, &ctx, vec![]);
        assert!(listed(&engine)?);
        let after = editor.inspector_card(&engine).issues.ok_or("slot")?.counts;
        assert_eq!(after.error, before.error + 1, "{before:?} → {after:?}");
        assert!(!shown.iter().any(|(text, _)| text == finding), "one home");
        Ok(())
    }

    #[test]
    fn invalid_typed_default_shows_an_error_and_sends_nothing()
    -> Result<(), Box<dyn std::error::Error>> {
        let ctx = egui::Context::default();
        fonts::setup_bundled_fonts(&ctx);
        let engine = Engine::demo()?;
        let mut editor = PartEditor::default();
        editor.open_first(&engine);
        editor.commit(&ctx, &engine);
        let original = editor.draft.clone();
        type_into(&mut editor, &engine, &ctx, "No default", "ten volts")?;
        let shown = frame(&mut editor, &engine, &ctx, vec![]);
        assert!(
            shown.iter().any(|(text, _)| text == "ten volts"),
            "text kept"
        );
        assert_eq!(editor.draft, original);
        assert!(editor.commit(&ctx, &engine).is_none());
        Ok(())
    }
}
