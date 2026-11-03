//! The app's one command path: keys, the command palette and the app bar
//! run registry commands here (`crate::commands`). A ribbon command sends
//! the same event as its button, so its editor returns the typed
//! operation the engine applies, exactly as for a click.
use super::App;
use super::command_palette::{PaletteEvent, PaletteRow};
use crate::commands::{Action, AppCommand, Chord, Command, Context, Handler, Registry};
use crate::file_ui;
use crate::kit::severity::Level;
use crate::shell::{Ribbon, RibbonEvent, Workspace};

/// What a ribbon offers now: its tab ids and its tools (id, enabled, tip).
pub(super) struct RibbonState {
    workspace: Workspace,
    tabs: Vec<&'static str>,
    tools: Vec<(&'static str, &'static str, bool, &'static str)>,
}

impl RibbonState {
    pub(super) fn of(workspace: Workspace, ribbon: &Ribbon) -> Self {
        let mut tools = Vec::new();
        for group in &ribbon.groups {
            for tool in &group.tools {
                tools.push((tool.id, tool.label, tool.enabled, tool.tip));
                for variant in &tool.menu {
                    tools.push((variant.id, variant.label, variant.enabled, variant.tip));
                }
            }
        }
        Self {
            workspace,
            tabs: ribbon.tabs.iter().map(|t| t.id).collect(),
            tools,
        }
    }

    /// Whether the ribbon can run `event` now; `Err` says why not.
    pub(super) fn allows(&self, event: RibbonEvent, context: Context) -> Result<(), String> {
        let elsewhere = || format!("Not available here; it applies in {}", context.label());
        match event {
            RibbonEvent::Tab(id) => self.tabs.contains(&id).then_some(()).ok_or_else(elsewhere),
            RibbonEvent::Tool(id) => match self.tools.iter().find(|t| t.0 == id) {
                Some((_, _, true, _)) => Ok(()),
                Some((_, label, false, "")) => Err(format!("{label} is unavailable now")),
                Some((_, _, false, tip)) => Err((*tip).to_owned()),
                None => Err(elsewhere()),
            },
            RibbonEvent::Workspace(_) | RibbonEvent::Issues => Ok(()),
        }
    }
}

/// Whether a pressed chord may run `command` now: a focused text field
/// keeps plain keys and its own undo, redo and ⌘B; an open menu keeps plain
/// keys; canvas keys run where they are handled.
pub(super) fn dispatchable(command: &Command, chord: Chord, typing: bool, popup: bool) -> bool {
    if command.handler != Handler::Registry || !command.runnable() {
        return false;
    }
    if !chord.command && (typing || popup) {
        return false;
    }
    !(typing
        && matches!(
            command.action,
            Action::App(AppCommand::Undo | AppCommand::Redo | AppCommand::ToggleBrowser)
        ))
}

/// The key presses of this frame no one consumed, as chords.
pub(super) fn presses(ctx: &egui::Context) -> Vec<Chord> {
    ctx.input(|i| {
        i.events
            .iter()
            .filter_map(|event| match event {
                egui::Event::Key {
                    key,
                    pressed: true,
                    repeat: false,
                    modifiers,
                    ..
                } => Some(Chord {
                    key: *key,
                    command: modifiers.command,
                    shift: modifiers.shift,
                    alt: modifiers.alt,
                }),
                _ => None,
            })
            .collect()
    })
}

impl App {
    /// The keyboard contexts in force, most specific first.
    pub(super) fn active_contexts(&self) -> Vec<Context> {
        let inner = match self.workspace {
            Workspace::Part => self.part.shortcut_context(),
            Workspace::Schematic => Context::Schematic,
            Workspace::Pcb => self.pcb.shortcut_context(),
        };
        crate::commands::active_contexts(inner)
    }

    fn ribbon_state(&mut self, workspace: Workspace) -> RibbonState {
        let ribbon = match workspace {
            Workspace::Part => self.part.ribbon(&self.engine),
            Workspace::Schematic => self.schematic.ribbon(&self.engine),
            Workspace::Pcb => self.pcb.ribbon(&self.engine),
        };
        RibbonState::of(workspace, &ribbon)
    }

    /// Whether `command` can run now; `Err` says why not. `ribbons` are
    /// the workspaces' ribbon states, built once per frame by the caller.
    fn availability(&self, command: &Command, ribbons: &[RibbonState]) -> Result<(), String> {
        match command.action {
            Action::Key => Err("Press the key in its tool".to_owned()),
            Action::App(app) => match app {
                AppCommand::Undo if !self.engine.can_undo() => Err("Nothing to undo".into()),
                AppCommand::Redo if !self.engine.can_redo() => Err("Nothing to redo".into()),
                AppCommand::New
                | AppCommand::Open
                | AppCommand::Save
                | AppCommand::SaveAs
                | AppCommand::Export
                | AppCommand::ExportSchematicPdf
                    if self.files.busy() =>
                {
                    Err("Wait for the file operation to finish".into())
                }
                _ => Ok(()),
            },
            Action::PcbLayer(index) => {
                if self.pcb.is_3d() {
                    return Err("Switch to the Layout view to use it".into());
                }
                self.pcb.layer_at(index).map(|_| ())
            }
            Action::Pcb(toggle) => {
                let three_d = self.pcb.is_3d();
                match command.context {
                    Context::PcbLayout if three_d => {
                        return Err("Switch to the Layout view to use it".into());
                    }
                    Context::Pcb3d if !three_d => {
                        return Err("Switch to the 3D view to use it".into());
                    }
                    _ => {}
                }
                self.pcb
                    .view_toggle_state(toggle)
                    .map(|_| ())
                    .map_err(str::to_owned)
            }
            Action::Ribbon(workspace, event) => ribbons
                .iter()
                .find(|r| r.workspace == workspace)
                .map_or(Ok(()), |r| r.allows(event, command.context)),
        }
    }

    /// Run the command `id` from a key or the palette. One that cannot run
    /// says why in a toast and changes nothing.
    pub(super) fn run_command(&mut self, ctx: &egui::Context, id: &str) {
        let Some(command) = self.registry.get(id).cloned() else {
            return;
        };
        let ribbons = match command.action {
            Action::Ribbon(workspace, _) => vec![self.ribbon_state(workspace)],
            _ => Vec::new(),
        };
        if let Err(reason) = self.availability(&command, &ribbons) {
            self.say(Level::Info, reason);
            return;
        }
        match command.action {
            Action::App(app) => self.app_command(app),
            Action::Ribbon(workspace, event) => {
                self.enter(workspace);
                self.ribbon_event(ctx, event);
            }
            Action::Pcb(toggle) => {
                self.enter(Workspace::Pcb);
                self.pcb.view_toggle(toggle);
            }
            Action::PcbLayer(index) => {
                self.enter(Workspace::Pcb);
                self.pcb.select_layer(self.engine.project(), index);
            }
            Action::Key => {}
        }
    }

    fn enter(&mut self, workspace: Workspace) {
        if workspace != self.workspace {
            self.clear_error();
            self.workspace = workspace;
        }
    }

    fn app_command(&mut self, app: AppCommand) {
        match app {
            AppCommand::Palette => {
                self.shortcut_sheet.close();
                self.palette.toggle();
            }
            AppCommand::Shortcuts => {
                self.palette.close();
                self.shortcut_sheet.toggle();
            }
            AppCommand::New => self.files.request(file_ui::Action::New),
            AppCommand::Open => self.files.request(file_ui::Action::Open),
            AppCommand::Save => self.files.request(file_ui::Action::Save),
            AppCommand::SaveAs => self.files.request(file_ui::Action::SaveAs),
            AppCommand::Export => self.files.request(file_ui::Action::Export),
            AppCommand::ExportSchematicPdf => {
                self.files.request(file_ui::Action::ExportSchematicPdf)
            }
            AppCommand::Undo => self.history(false),
            AppCommand::Redo => self.history(true),
            AppCommand::ToggleBrowser => self.toggle_browser(),
            AppCommand::ToggleIssues => self.issues.toggle(),
            AppCommand::Messages => self.messages_open = true,
            AppCommand::Settings => self.settings_window.toggle(),
            AppCommand::LibrarySources => self.sources.open_card(),
            AppCommand::Workspace(workspace) => self.enter(workspace),
            AppCommand::FitView => self.fit_view(),
            AppCommand::TogglePcb3d => {
                self.enter(Workspace::Pcb);
                let three_d = self.pcb.is_3d();
                self.pcb.set_3d(!three_d);
            }
            AppCommand::View3d(view) => self.view_3d(view),
        }
    }

    /// Turn the PCB 3D view to a standard view, opening it if needed.
    pub(super) fn view_3d(&mut self, view: crate::commands::View3d) {
        use crate::commands::View3d as V;
        use pcbcraft_solidcraft::render::StandardView as S;
        self.enter(Workspace::Pcb);
        if !self.pcb.is_3d() {
            self.pcb.set_3d(true);
        }
        let standard = match view {
            V::Top => S::Top,
            V::Bottom => S::Bottom,
            V::Front => S::Front,
            V::Back => S::Back,
            V::Left => S::Left,
            V::Right => S::Right,
            V::Iso => S::Iso,
            V::Selection => {
                let items = self.scene.selected_items(
                    self.pcb.selected_instance(),
                    self.pcb.selected_track(),
                    self.pcb.selected_via(),
                );
                if !self.scene.viewport.fit_items(&items) {
                    self.say(Level::Info, "Select something to frame");
                }
                return;
            }
        };
        self.scene.viewport.standard_view(standard);
    }

    /// Keys that work everywhere, even over a focused field or an open
    /// palette: the palette's and the sheet's own. Runs before the
    /// workspace, so nothing below sees them.
    pub(super) fn early_keys(&mut self, ctx: &egui::Context) {
        for id in ["app.palette", "app.palette-alt", "app.shortcuts"] {
            let chord = self.registry.get(id).and_then(|c| self.registry.chord(c));
            if let Some(chord) = chord
                && chord.consume(ctx)
            {
                self.run_command(ctx, id);
            }
        }
    }

    /// The registry's keys, after the canvas had its turn: each unconsumed
    /// press runs the command it names in the active contexts, unless the
    /// canvas owns it ([`Handler::InPlace`]).
    pub(super) fn dispatch_keys(&mut self, ctx: &egui::Context) {
        if self.palette.is_open() {
            return;
        }
        let typing = ctx.egui_wants_keyboard_input();
        let popup = ctx.any_popup_open();
        let active = self.active_contexts();
        for chord in presses(ctx) {
            let Some(id) = self
                .registry
                .resolve(chord, &active)
                .filter(|c| dispatchable(c, chord, typing, popup))
                .map(|c| c.id.clone())
            else {
                continue;
            };
            ctx.input_mut(|i| i.consume_key(chord.modifiers(), chord.key));
            self.run_command(ctx, &id);
        }
    }

    /// The command palette, while open. Runs before the workspace so its
    /// keys (arrows, Enter, Esc) never reach a canvas.
    pub(super) fn command_palette(&mut self, ctx: &egui::Context) {
        if !self.palette.is_open() {
            return;
        }
        let mut palette = std::mem::take(&mut self.palette);
        let active = self.active_contexts();
        let ribbons: Vec<RibbonState> = Workspace::ALL
            .into_iter()
            .map(|w| self.ribbon_state(w))
            .collect();
        let recent = palette.recent().to_vec();
        // Commands about a copper layer list only the board's own layers.
        let stack = self.engine.project().stackup.layers();
        let event = palette.show(ctx, |query| {
            self.registry
                .search(query, &recent, &active)
                .into_iter()
                .filter(|command| command.stackup_layer().is_none_or(|l| l.on(&stack)))
                .map(|command| {
                    let state = self.availability(command, &ribbons);
                    PaletteRow {
                        id: command.id.clone(),
                        title: command.title.clone(),
                        icon: command.icon,
                        detail: match &state {
                            Ok(()) => command.context.label().to_owned(),
                            Err(reason) => reason.clone(),
                        },
                        shortcut: self.registry.shortcut(command),
                        enabled: state.is_ok(),
                        recent: recent.contains(&command.id),
                    }
                })
                .collect()
        });
        if let Some(PaletteEvent::Run(id)) = event {
            palette.used(&id);
            self.palette = palette;
            self.run_command(ctx, &id);
        } else {
            self.palette = palette;
        }
    }

    /// Fill each ribbon tool's shortcut from the registry, for its tooltip.
    pub(super) fn annotate_ribbon(&self, ribbon: &mut Ribbon) {
        annotate(&self.registry, self.workspace, ribbon);
    }

    /// `label (key)` for an app command's button, as Solidcraft names keys.
    pub(super) fn with_shortcut(&self, label: &str, app: AppCommand) -> String {
        match self
            .registry
            .for_app(app)
            .and_then(|c| self.registry.shortcut(c))
        {
            Some(key) => format!("{label} ({key})"),
            None => label.to_owned(),
        }
    }

    /// The ribbon's events: from a click, a key or the palette.
    pub(super) fn ribbon_event(&mut self, ctx: &egui::Context, event: RibbonEvent) {
        match event {
            RibbonEvent::Workspace(workspace) => self.enter(workspace),
            RibbonEvent::Tab(tab) => match self.workspace {
                Workspace::Part => self.part.on_tab(tab),
                Workspace::Schematic => {}
                Workspace::Pcb => self.pcb.on_tab(tab),
            },
            RibbonEvent::Tool(tool) => {
                let operation = match self.workspace {
                    // The file service owns the native picker.
                    Workspace::Part if tool == crate::part_editor::IMPORT_SVG => {
                        self.part.import_svg(self.files.pick_svg());
                        None
                    }
                    Workspace::Part => self.part.on_tool(tool, &self.engine),
                    Workspace::Schematic => self.schematic.on_tool(tool, &self.engine),
                    Workspace::Pcb => self.pcb.on_tool(tool, &self.engine),
                };
                self.apply(ctx, crate::refusal::Origin::Command, operation);
            }
            RibbonEvent::Issues => self.issues.toggle(),
        }
    }

    /// Fit the shown view: the 3D view, or the workspace's 2D canvas.
    pub(super) fn fit_view(&mut self) {
        if self.model_key().is_some() {
            self.scene.viewport.fit();
        } else {
            match self.workspace {
                Workspace::Part => self.part.fit(),
                Workspace::Schematic => self.schematic.fit(),
                Workspace::Pcb => self.pcb.fit(),
            }
        }
    }
}

/// Set each tool's shown shortcut in `workspace`'s ribbon from `registry`.
pub(super) fn annotate(registry: &Registry, workspace: Workspace, ribbon: &mut Ribbon) {
    let shortcut = |id: &str| {
        registry
            .for_tool(workspace, id)
            .and_then(|c| registry.shortcut(c))
    };
    for group in &mut ribbon.groups {
        for tool in &mut group.tools {
            tool.shortcut = shortcut(tool.id);
            for variant in &mut tool.menu {
                variant.shortcut = shortcut(variant.id);
            }
        }
    }
}

#[cfg(test)]
#[path = "command_dispatch_tests.rs"]
mod tests;
