//! The desktop application: one engine, three editors and the file workflow.
//!
//! Editors and the 3D view never change the document themselves. They return
//! an [`Operation`], which [`App::apply`] sends through the engine, the same
//! path the CLI and MCP use. Long work (3D builds, file I/O, recovery) runs on
//! background threads and reports back with the input it was made from.
mod chrome;

use crate::file_ui::{self, Effect, FileUi};
use crate::scene::{ModelKey, SceneController};
use crate::shell::Workspace;
use crate::{part_editor, pcb_editor, schematic_editor, viewport};
use pcbcraft_core::{Engine, Operation, format::EditorFile};
use solidcraft_ui_egui::{fonts, theme};
use std::path::PathBuf;
use std::sync::Arc;

pub struct App {
    engine: Engine,
    workspace: Workspace,
    part: part_editor::PartEditor,
    schematic: schematic_editor::SchematicEditor,
    pcb: pcb_editor::PcbEditor,
    scene: SceneController,
    files: FileUi,
    status: String,
    show_browser: bool,
    closing: bool,
}

impl App {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        follow_system_theme(&cc.egui_ctx, true);
        fonts::setup_bundled_fonts(&cc.egui_ctx);
        let render_state = cc.wgpu_render_state.as_ref().ok_or("wgpu is required")?;
        let engine = Engine::demo()?;
        let mut files = FileUi::new(cc, &engine);
        // `pcbcraft FILE.pcbcraft` opens that project through the file workflow.
        if let Some(path) = std::env::args_os()
            .skip(1)
            .map(PathBuf::from)
            .find(|p| p.extension().is_some_and(|e| e == "pcbcraft"))
        {
            files.request(file_ui::Action::OpenPath(path));
        }
        Ok(Self {
            scene: SceneController::new(cc.egui_ctx.clone(), viewport::Viewport::new(render_state)),
            files,
            engine,
            workspace: Workspace::Pcb,
            part: Default::default(),
            schematic: Default::default(),
            pcb: Default::default(),
            status: "Ready".into(),
            show_browser: true,
            closing: false,
        })
    }

    /// The one path from UI gestures to document changes.
    fn apply(&mut self, operation: Option<Operation>) {
        let Some(operation) = operation else {
            return;
        };
        if self.files.busy() {
            self.status = "Wait for the file operation to finish".into();
            return;
        }
        self.status = match self.engine.apply(self.engine.revision(), operation) {
            Ok(()) => "Ready".into(),
            Err(error) => error.to_string(),
        };
    }

    fn history(&mut self, redo: bool) {
        let result = if redo {
            self.engine.redo()
        } else {
            self.engine.undo()
        };
        if let Err(error) = result {
            self.status = error.to_string();
        }
    }

    fn replace(&mut self, document: EditorFile, path: Option<PathBuf>, recovered: bool) {
        let name = path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| document.project.name.clone());
        if let Err(error) = self.engine.replace_document(document) {
            self.status = format!("Open failed: {error}");
            return;
        }
        self.pcb.set_selection(None, None, None);
        self.status = match self.files.adopt(&self.engine, path, recovered) {
            Ok(()) if recovered => format!("Recovered {name}; save it to keep it"),
            Ok(()) => format!("Opened {name}"),
            Err(error) => error,
        };
    }

    fn file_effects(&mut self, ctx: &egui::Context) {
        for effect in self.files.tick(ctx) {
            match effect {
                Effect::Replace {
                    document,
                    path,
                    recovered,
                } => self.replace(*document, path, recovered),
                Effect::Close => {
                    self.closing = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }
        if let Some(status) = self.files.take_status() {
            self.status = status;
        }
    }

    /// The model the active view displays, if it is a 3D view.
    fn model_key(&self) -> Option<ModelKey> {
        match self.workspace {
            Workspace::Part if self.part.is_model_view() => {
                self.part.preview_part().and_then(ModelKey::part)
            }
            Workspace::Pcb if self.pcb.is_3d() => {
                Some(ModelKey::Project(Arc::clone(self.engine.project_arc())))
            }
            _ => None,
        }
    }

    fn request_model(&mut self) {
        if let Some(key) = self.model_key() {
            self.scene.request(key, self.part.preview_part());
        }
    }

    fn tools(&mut self, ui: &mut egui::Ui) {
        let operation = match self.workspace {
            Workspace::Part => self.part.tools(ui, &self.engine),
            Workspace::Schematic => self.schematic.tools(ui, &self.engine),
            Workspace::Pcb => self.pcb.tools(ui, &self.engine),
        };
        self.apply(operation);
    }

    fn inspector(&mut self, ui: &mut egui::Ui) {
        let operation = match self.workspace {
            Workspace::Part => self.part.inspector(ui, &self.engine),
            Workspace::Schematic => self.schematic.inspector(ui, &self.engine),
            Workspace::Pcb => self.pcb.inspector(ui, &self.engine),
        };
        self.apply(operation);
    }

    fn canvas(&mut self, ui: &mut egui::Ui) {
        let Some(key) = self.model_key() else {
            let operation = match self.workspace {
                Workspace::Part => self.part.canvas(ui, &self.engine),
                Workspace::Schematic => self.schematic.canvas(ui, &self.engine),
                Workspace::Pcb => self.pcb.canvas(ui, &self.engine),
            };
            self.apply(operation);
            return;
        };
        if !self.scene.showing(&key) {
            let message = match self.scene.status() {
                crate::scene::Status::Failed(error) => error.clone(),
                _ => "Updating 3D model…".into(),
            };
            ui.centered_and_justified(|ui| ui.label(message));
            return;
        }
        // Component previews never change PCB selection.
        if self.workspace == Workspace::Part {
            self.scene.highlight(None, None, None);
            self.scene.viewport.show(ui);
            return;
        }
        self.scene.highlight(
            self.pcb.selected_instance(),
            self.pcb.selected_track(),
            self.pcb.highlighted_net(),
        );
        if let Some(pick) = self.scene.viewport.show(ui) {
            match pick.and_then(|id| self.scene.source(id)) {
                Some(source) => {
                    let (instance, track, net) = (source.instance, source.track, source.net);
                    self.pcb.set_selection(instance, track, net);
                }
                None => self.pcb.set_selection(None, None, None),
            }
        }
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        use egui::{Key, KeyboardShortcut, Modifiers};
        let pressed = |modifiers, key| {
            ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(modifiers, key)))
        };
        let shift = Modifiers::COMMAND | Modifiers::SHIFT;
        if pressed(shift, Key::S) {
            self.files.request(file_ui::Action::SaveAs);
        } else if pressed(Modifiers::COMMAND, Key::S) {
            self.files.request(file_ui::Action::Save);
        }
        if pressed(Modifiers::COMMAND, Key::O) {
            self.files.request(file_ui::Action::Open);
        }
        if pressed(Modifiers::COMMAND, Key::N) {
            self.files.request(file_ui::Action::New);
        }
        if let Some(redo) = history_shortcut(ctx) {
            self.history(redo);
        }
    }

    /// Closing with unsaved work asks first; the file workflow closes the
    /// window once recovery data is cleaned up.
    fn guard_close(&mut self, ctx: &egui::Context) {
        if ctx.input(|i| i.viewport().close_requested()) && !self.closing {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.files.request(file_ui::Action::Close);
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        follow_system_theme(&ctx, false);
        self.scene.receive();
        self.file_effects(&ctx);
        self.app_bar(ui);
        self.ribbon(ui);
        self.status_bar(ui);
        self.workspace_area(ui);
        // Draft edits commit once their gesture ends, as one undo step.
        let draft = self.part.commit(&ctx, &self.engine);
        self.apply(draft);
        self.shortcuts(&ctx);
        self.guard_close(&ctx);
        self.files.observe(&self.engine, ctx.input(|i| i.time));
        self.files.dialogs(&ctx);
        self.request_model();
    }
}

/// Follow the operating system's light or dark appearance, like Solidcraft's
/// default "system" theme.
fn follow_system_theme(ctx: &egui::Context, force: bool) {
    let dark = ctx.system_theme().is_none_or(|t| t == egui::Theme::Dark);
    if force || dark != theme::is_dark() {
        theme::set_dark(dark);
        theme::apply(ctx);
    }
}

/// `Some(true)` for redo, `Some(false)` for undo. A focused text field owns
/// its own undo keys, as in Solidcraft.
fn history_shortcut(ctx: &egui::Context) -> Option<bool> {
    use egui::{Key, KeyboardShortcut, Modifiers};
    if ctx.egui_wants_keyboard_input() {
        return None;
    }
    if ctx.input_mut(|i| {
        i.consume_shortcut(&KeyboardShortcut::new(
            Modifiers::COMMAND | Modifiers::SHIFT,
            Key::Z,
        ))
    }) {
        Some(true)
    } else if ctx
        .input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::Z)))
    {
        Some(false)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn undo_input() -> egui::RawInput {
        egui::RawInput {
            events: vec![egui::Event::Key {
                key: egui::Key::Z,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::COMMAND,
            }],
            ..Default::default()
        }
    }

    #[test]
    fn typing_owns_undo_instead_of_undoing_the_project() {
        let ctx = egui::Context::default();
        let mut text = "10k".to_owned();
        let _ = ctx.run_ui(Default::default(), |ui| {
            ui.add(egui::TextEdit::singleline(&mut text).id_salt("value"))
                .request_focus();
        });
        let mut shortcut = Some(false);
        let _ = ctx.run_ui(undo_input(), |ui| {
            ui.add(egui::TextEdit::singleline(&mut text).id_salt("value"));
            shortcut = history_shortcut(ui.ctx());
        });
        assert_eq!(shortcut, None);
        let unfocused = egui::Context::default();
        let _ = unfocused.run_ui(undo_input(), |ui| {
            shortcut = history_shortcut(ui.ctx());
        });
        assert_eq!(shortcut, Some(false));
    }
}
