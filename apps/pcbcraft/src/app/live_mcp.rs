//! Live MCP (Help › Connect AI assistants…): an AI assistant edits the
//! project open in this window, on the window's engine and undo history.
//!
//! Off at every start. Turning it on opens `pcbcraft_automation::live`'s
//! loopback endpoint with a new token; its requests run here, on the UI
//! thread at the start of a frame, through the same `Session::respond` the
//! CLI and headless MCP use. Changes are refused while the person types,
//! drags or has a menu, dialog or review open; Ask-class requests wait for
//! Allow or Deny in the window. Each assistant edit is a toast with Undo and
//! a Messages line, and undoing one says it was the assistant's.
//!
//! The shape (a socket worker handing requests to the UI thread, refusals
//! while the editor is held, opt-in from Help) follows tangency-labs/pcbcraft
//! (MIT OR Apache-2.0); see ATTRIBUTION.md. The channel is Solidcraft's.
use super::App;
use crate::kit::{self, severity::Level, toast::ToastAction};
use pcbcraft_automation::live::{self, Admitted, ControlRequest, Endpoint, Verdict};
use pcbcraft_automation::{Mode, Policy, Question, Session};
use solidcraft_ui_egui::{ui_kit, ui_kit::Button};
use std::collections::VecDeque;
use std::path::PathBuf;

/// What a person reads for an assistant's request: its catalog title
/// ("Add a via"), never its wire name (`layout.add_via`).
fn spoken(subject: &'static str) -> &'static str {
    pcbcraft_core::Operation::titles()
        .chain(pcbcraft_core::Query::TITLES)
        .chain(pcbcraft_automation::Request::TITLES)
        .find(|(name, _)| *name == subject)
        .map_or("a change", |(_, title)| *title)
}

/// Requests answered per frame, so a flood cannot starve drawing.
const PER_FRAME: usize = 16;
/// Assistant edits remembered for labelling an undo (the engine's depth).
const REMEMBERED: usize = pcbcraft_core::MAX_UNDO;

/// A request held for the person's Allow or Deny.
struct Held {
    request: ControlRequest,
    admitted: Admitted,
    question: Question,
}

/// Where the endpoint listens and keeps its token, and whether requests see
/// the user's library sources; tests use no files.
#[derive(Clone, Copy)]
pub(super) struct Place {
    pub app: live::App,
    pub listen: live::Listen,
    pub user_sources: bool,
}

pub(super) struct LiveMcp {
    endpoint: Option<Endpoint>,
    /// The session requests run in, over the window's engine; it keeps the
    /// compact replies' baseline between requests.
    session: Session,
    /// What runs without asking (the gate's mode).
    mode: Mode,
    held: Option<Held>,
    /// The Connect AI assistants window is open.
    open: bool,
    /// Why turning on failed, shown in the window.
    failure: Option<String>,
    /// Revisions assistant edits made, newest last.
    edits: VecDeque<u64>,
    place: Place,
}

impl Default for LiveMcp {
    fn default() -> Self {
        Self {
            endpoint: None,
            session: Session::default(),
            mode: Mode::AutoAllowEdits,
            held: None,
            open: false,
            failure: None,
            edits: VecDeque::new(),
            place: Place {
                app: live::APP,
                listen: live::Listen::Default,
                user_sources: true,
            },
        }
    }
}

impl LiveMcp {
    pub fn toggle_window(&mut self) {
        self.open = !self.open;
    }
    pub fn is_open(&self) -> bool {
        self.open
    }
    pub fn close(&mut self) {
        self.open = false;
    }
    pub fn is_on(&self) -> bool {
        self.endpoint.is_some()
    }
    /// Connections open now.
    pub fn connections(&self) -> usize {
        self.endpoint.as_ref().map_or(0, Endpoint::connections)
    }
    #[cfg(test)]
    pub fn endpoint(&self) -> Option<&Endpoint> {
        self.endpoint.as_ref()
    }
    #[cfg(test)]
    pub fn use_place(&mut self, place: Place) {
        self.place = place;
    }

    /// Listen, or stop: stopping refuses a held request and closes every
    /// connection.
    pub fn set_on(&mut self, ctx: &egui::Context, on: bool) {
        self.failure = None;
        if !on {
            if let Some(held) = self.held.take() {
                let _ = held.request.reply.send(held.question.denied().reply());
            }
            self.endpoint = None;
            return;
        }
        if self.endpoint.is_some() {
            return;
        }
        let wake = ctx.clone();
        match Endpoint::start(self.place.app, self.place.listen, move || {
            wake.request_repaint()
        }) {
            Ok(endpoint) => {
                // Folder searches see the user's library sources, as in
                // the headless session.
                if self.place.user_sources {
                    self.session = match pcbcraft_automation::session::sources::Sources::from_env()
                    {
                        Ok(sources) => Session::with_sources(Default::default(), sources),
                        Err(_) => Session::default(),
                    };
                }
                self.endpoint = Some(endpoint);
            }
            Err(error) => self.failure = Some(format!("Could not start listening: {error}")),
        }
    }
}

/// The CLI the configuration starts: the one beside this app when it is
/// there, else whatever `pcbcraft-cli` the client's PATH finds.
fn cli_path() -> String {
    let name = if cfg!(windows) {
        "pcbcraft-cli.exe"
    } else {
        "pcbcraft-cli"
    };
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(name)))
        .filter(|path| path.is_file())
        .map_or_else(|| name.to_owned(), |path| path.display().to_string())
}

/// Folders an assistant may name paths in: the home folder, the temporary
/// folder and the open project's folder. Writing anywhere still asks.
fn folders(project: Option<&std::path::Path>) -> Vec<PathBuf> {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from);
    home.into_iter()
        .chain([std::env::temp_dir()])
        .chain(
            project
                .and_then(|p| p.parent())
                .map(std::path::Path::to_path_buf),
        )
        .filter(|folder| folder.is_absolute())
        .collect()
}

impl App {
    /// Why the person holds the window now, so an assistant's change waits.
    fn live_busy(&mut self, ctx: &egui::Context) -> Option<String> {
        let reviewing = self.part.review_mut().is_open()
            || self.schematic.review_mut().is_open()
            || self.pcb.review_mut().is_open();
        let reason = if self.live.held.is_some() {
            "The window is waiting for the person to answer another assistant request"
        } else if self.files.busy() || self.files.prompting() {
            "A file operation or file question is open in the window"
        } else if ctx.egui_wants_keyboard_input() {
            "The person is editing text in a field"
        } else if ctx.dragged_id().is_some() || ctx.input(|i| i.pointer.any_down()) {
            "The person is dragging in the window"
        } else if ctx.any_popup_open()
            || crate::context_menu::is_open(ctx)
            || self.palette.is_open()
            || self.settings_window.is_recording()
        {
            "A menu or the command palette is open in the window"
        } else if reviewing {
            "A review is open in the window"
        } else {
            return None;
        };
        Some(reason.to_owned())
    }

    /// Answer the requests that arrived since the last frame, before the
    /// editors draw, so this frame shows what they did.
    pub(super) fn serve_live(&mut self, ctx: &egui::Context) {
        for _ in 0..PER_FRAME {
            let Some(request) = self.live.endpoint.as_ref().and_then(Endpoint::next) else {
                return;
            };
            request
                .started
                .store(true, std::sync::atomic::Ordering::Relaxed);
            let busy = self.live_busy(ctx);
            let policy = Policy::new(self.live.mode, folders(self.files.path()));
            let path = self.files.path().map(std::path::Path::to_path_buf);
            let verdict = self
                .live
                .session
                .lend(&mut self.engine, path.as_deref(), |s| {
                    live::admit(
                        s,
                        &policy,
                        busy.as_deref(),
                        &request.method,
                        &request.params,
                    )
                });
            match verdict {
                Verdict::Reply(reply) => {
                    let _ = request.reply.send(reply);
                }
                Verdict::Run(admitted) => self.run_live(request, &admitted),
                Verdict::Ask(admitted, question) => {
                    self.live.held = Some(Held {
                        request,
                        admitted,
                        question,
                    });
                }
            }
        }
        // More are waiting: answer them next frame.
        ctx.request_repaint();
    }

    /// Run one admitted request on the window's engine and answer it. A
    /// change is the assistant's: a toast with Undo, and a Messages line.
    fn run_live(&mut self, request: ControlRequest, admitted: &Admitted) {
        let before = self.engine.revision();
        // An undo or redo says the step it took, as the person's own does.
        let history = match admitted.subject() {
            "project.undo" => Some(("AI assistant undid", self.engine.undo_step())),
            "project.redo" => Some(("AI assistant redid", self.engine.redo_step())),
            _ => None,
        };
        let path = self.files.path().map(std::path::Path::to_path_buf);
        let reply = self
            .live
            .session
            .lend(&mut self.engine, path.as_deref(), |s| {
                live::run(s, admitted)
            });
        let _ = request.reply.send(reply);
        let now = self.engine.revision();
        if now != before
            && let Some((done, step)) = history
        {
            if let Some(step) = step {
                self.say_step(done, step);
            }
        } else if now != before {
            let subject = spoken(admitted.subject());
            self.live.edits.push_back(now);
            while self.live.edits.len() > REMEMBERED {
                self.live.edits.pop_front();
            }
            self.say_with(
                Level::Info,
                format!("AI assistant edit: {subject}"),
                vec![ToastAction::Undo],
            );
        }
    }

    /// Whether an undo at `revision` takes back an assistant edit.
    pub(super) fn live_edit_at(&self, revision: u64) -> bool {
        self.live.edits.contains(&revision)
    }

    /// The Connect AI assistants window and a held request's question.
    pub(super) fn live_windows(&mut self, ctx: &egui::Context) {
        self.live_question(ctx);
        if !self.live.open {
            return;
        }
        let mut on = self.live.is_on();
        let mut mode = self.live.mode;
        let (port, connections) = self
            .live
            .endpoint
            .as_ref()
            .map_or((0, 0), |e| (e.port(), e.connections()));
        let failure = self.live.failure.clone();
        let cli = cli_path();
        let config = live::client_config(&cli);
        let claude = format!("claude mcp add pcbcraft -- \"{cli}\" mcp --live");
        let mut copy = None;
        let closed = ui_kit::Window::new(
            egui::Id::unique("app-live-mcp"),
            "assistant",
            "Connect AI assistants",
        )
        .show_body(ctx, |ui| {
            kit::section(ui, "live-access", "Access", true, None, |ui| {
                kit::check(ui, "Let AI assistants edit this project", &mut on);
                if let Some(failure) = &failure {
                    kit::refusal(ui, failure);
                }
                let state = match (port, connections) {
                    (0, _) => "Off. Nothing listens.".to_owned(),
                    (port, 0) => format!("On, on this computer only (port {port}). No assistant connected."),
                    (port, 1) => format!("On, on this computer only (port {port}). 1 assistant connected."),
                    (port, n) => format!("On, on this computer only (port {port}). {n} assistants connected."),
                };
                kit::note(ui, &state);
                kit::choice_row(
                    ui,
                    "Runs without asking",
                    &mut mode,
                    &[
                        (Mode::AskEveryTime, "Reading only".to_owned()),
                        (Mode::AutoAllowEdits, "Reading and edits".to_owned()),
                        (Mode::AllowAll, "Everything except settings".to_owned()),
                    ],
                );
                kit::hint(
                    ui,
                    "Edits join your undo history. Files and publishing ask you here.",
                    "An assistant's change is refused while you type, drag or have a menu, dialog or review open. A new token is made each time you turn this on; it stays in a file only you can read.",
                );
            });
            kit::section(ui, "live-config", "Configuration", true, None, |ui| {
                kit::note(ui, "Add this server to your assistant's MCP settings:");
                ui.label(egui::RichText::new(&config).monospace());
                kit::actions(ui, |ui| {
                    if ui_kit::button(ui, "Copy", Button::Enabled(true)).clicked() {
                        copy = Some(config.clone());
                    }
                });
                kit::note(ui, "Claude Code:");
                ui.label(egui::RichText::new(&claude).monospace());
                kit::actions(ui, |ui| {
                    if ui_kit::button(ui, "Copy command", Button::Enabled(true)).clicked() {
                        copy = Some(claude.clone());
                    }
                });
            });
        });
        if let Some(text) = copy {
            ctx.copy_text(text);
        }
        self.live.mode = mode;
        if on != self.live.is_on() {
            self.live.set_on(ctx, on);
        }
        if closed {
            self.live.open = false;
        }
    }

    /// A held request: Allow runs it (if the project has not moved), Deny
    /// tells the assistant so.
    fn live_question(&mut self, ctx: &egui::Context) {
        let Some(held) = &self.live.held else {
            return;
        };
        let question = &held.question;
        let mut answer = None;
        let closed = ui_kit::Window::new(
            egui::Id::unique("app-live-question"),
            "assistant",
            "AI assistant asks",
        )
        .show_body(ctx, |ui| {
            kit::value_row(ui, "Command", &question.command);
            for path in &question.paths {
                kit::value_row(ui, "Path", path);
            }
            kit::note(ui, &question.why);
            kit::actions(ui, |ui| {
                if ui_kit::button(ui, "Deny", Button::Enabled(true)).clicked() {
                    answer = Some(false);
                }
                if kit::primary(ui, "Allow", true).clicked() {
                    answer = Some(true);
                }
            });
        });
        if let Some(allow) = if closed { Some(false) } else { answer } {
            self.live_answer(allow);
        }
    }

    /// The person's answer to the held request.
    fn live_answer(&mut self, allow: bool) {
        let Some(held) = self.live.held.take() else {
            return;
        };
        if allow {
            self.run_live(held.request, &held.admitted);
        } else {
            let _ = held.request.reply.send(held.question.denied().reply());
            self.say(
                Level::Info,
                format!("Denied the AI assistant's {}", held.question.command),
            );
        }
    }

    /// The command a held request waits on.
    #[cfg(test)]
    fn live_held(&self) -> Option<&str> {
        self.live.held.as_ref().map(|h| h.question.command.as_str())
    }

    /// The application bar's pill while live MCP is on: it names whether an
    /// assistant is connected, opens the window, and its × turns access off.
    pub(super) fn live_indicator(&mut self, ui: &mut egui::Ui) {
        if !self.live.is_on() {
            return;
        }
        let text = match self.live.connections() {
            0 => "AI access on",
            _ => "AI assistant connected",
        };
        let pill = kit::pill(ui, Some(Level::Info), text);
        if pill.closed {
            self.live.set_on(ui.ctx(), false);
            self.say(Level::Info, "AI assistant access turned off");
        } else if pill.clicked {
            self.live.open = true;
        }
    }
}

#[cfg(test)]
#[path = "live_mcp_tests.rs"]
mod tests;
