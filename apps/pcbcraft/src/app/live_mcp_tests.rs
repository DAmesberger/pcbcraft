//! Live MCP against a whole app, in process and without a window: an
//! assistant's request over the loopback socket runs on the app's engine
//! between frames, joins the person's undo history and is refused while the
//! person holds the window.
use super::super::*;
use super::Place;
use crate::commands::Registry;
use pcbcraft_automation::Mode;
use pcbcraft_automation::live::{self, Client, ClientToken};
use pcbcraft_ui::fonts;
use serde_json::{Value, json};

type Fallible<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// No token or discovery files: the test never touches the user's folders.
const TEST_APP: live::App = live::App {
    name: pcbcraft_core::product::NAME,
    data_dir: || None,
    env: |_| None,
    utc: |secs| format!("{secs}"),
    default_port: live::DEFAULT_PORT,
};

struct Harness {
    app: App,
    ctx: egui::Context,
    frame: eframe::Frame,
    _dir: tempfile::TempDir,
}

impl Harness {
    /// The demo project in a headless app, with live MCP turned on.
    fn new() -> Fallible<Self> {
        let dir = tempfile::tempdir()?;
        let ctx = egui::Context::default();
        fonts::setup_bundled_fonts(&ctx);
        let engine = Engine::demo()?;
        let files = FileUi::with_directory(
            &ctx,
            rfd::AsyncFileDialog::new(),
            &engine,
            dir.path().to_path_buf(),
        );
        let mut app = App::with_parts(
            &ctx,
            viewport::Viewport::detached(),
            Parts {
                engine,
                files,
                sources: LibrarySources::new(None),
                registry: Arc::new(Registry::standard()),
                keymap_problems: Vec::new(),
                messages: Messages::default(),
                browser_width: chrome::BROWSER_WIDTH,
                #[cfg(target_os = "macos")]
                native_menu: None,
            },
        );
        crate::keys::publish(&ctx, &app.registry);
        app.live.use_place(Place {
            app: TEST_APP,
            listen: live::Listen::Exact(0),
            user_sources: false,
        });
        app.live.set_on(&ctx, true);
        let mut harness = Self {
            app,
            ctx,
            frame: eframe::Frame::_new_kittest(),
            _dir: dir,
        };
        harness.frame(Vec::new());
        Ok(harness)
    }

    fn frame(&mut self, events: Vec<egui::Event>) {
        self.frame_output(events).textures_delta.clear();
    }

    fn frame_output(&mut self, events: Vec<egui::Event>) -> egui::FullOutput {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1380.0, 900.0),
            )),
            events,
            ..Default::default()
        };
        let Self {
            app, ctx, frame, ..
        } = self;
        ctx.run_ui(input, |ui| eframe::App::ui(app, ui, frame))
    }

    fn client(&self) -> Fallible<Client> {
        let endpoint = self.app.live.endpoint().ok_or("live MCP is off")?;
        Ok(Client::with_token(
            endpoint.port(),
            ClientToken {
                token: endpoint.token().to_owned(),
                file: None,
                app: TEST_APP,
            },
        ))
    }

    /// Send one request from another thread, as an MCP bridge does, and run
    /// frames until its reply arrives.
    fn call(&mut self, method: &str, params: Value) -> Fallible<Value> {
        let mut client = self.client()?;
        let method = method.to_owned();
        let asked = std::thread::spawn(move || client.call(&method, params));
        for _ in 0..5000 {
            if asked.is_finished() {
                break;
            }
            self.frame(Vec::new());
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        Ok(asked.join().map_err(|_| "client thread")??)
    }

    /// `layout.add_via` at `revision`.
    fn add_via(&self, revision: u64) -> Value {
        let net = self.app.engine.project().nets.first().map_or(0, |n| n.id);
        json!({"command": "project.apply", "params": {
            "expected_revision": revision,
            "operation": {"editor": "Layout", "operation": {"command": "layout.add_via", "params": {
                "via": {"net": net, "position": [1.0, 1.0], "drill": 0.3, "diameter": 0.6, "span": "through", "tented": true}
            }}},
        }})
    }

    fn said(&self, text: &str) -> bool {
        self.app.messages.entries().any(|m| m.text.contains(text))
    }
}

fn code(reply: &Value) -> &str {
    reply["error"]["code"].as_str().unwrap_or_default()
}

/// The assistant's via lands in the open project with a small reply; the
/// person's Undo takes it back, says it was the assistant's, and the
/// assistant's own undo needs the revision it saw.
#[test]
fn an_assistant_edits_the_open_project_with_shared_undo() -> Fallible {
    let mut h = Harness::new()?;
    let revision = h.app.engine.revision();
    let before = h.app.engine.document().clone();
    let reply = h.call(live::EXECUTE, h.add_via(revision))?;
    assert_eq!(reply["ok"], true, "{reply}");
    let bytes = reply.to_string().len();
    assert!(bytes < 4 * 1024, "the reply is {bytes} bytes: {reply}");
    assert_eq!(h.app.engine.revision(), revision + 1);
    assert_eq!(
        h.app.engine.project().vias.len(),
        before.project.vias.len() + 1
    );
    // Its title, never its wire name (night round 5).
    assert!(h.said("AI assistant edit: Add a via"));

    // The person undoes it in the window: the same history.
    h.app.history(false);
    assert_eq!(h.app.engine.document(), &before);
    assert!(h.said("Undid AI assistant edit: Add a via"));

    // The assistant redoes at the revision it last saw: refused, stale.
    let stale = h.call(
        live::EXECUTE,
        json!({"command": "project.redo", "params": {"expected_revision": revision + 1}}),
    )?;
    assert_eq!(code(&stale), "stale_revision", "{stale}");
    let now = h.call(live::REVISION, json!({}))?;
    let current = now["result"]["revision"].as_u64().ok_or("revision")?;
    assert_eq!(current, h.app.engine.revision());
    let redone = h.call(
        live::EXECUTE,
        json!({"command": "project.redo", "params": {"expected_revision": current}}),
    )?;
    assert_eq!(redone["ok"], true, "{redone}");
    assert_eq!(
        h.app.engine.project().vias.len(),
        before.project.vias.len() + 1
    );
    // The window says what the assistant's redo applied, by its title.
    assert!(h.said("AI assistant redid: Add a via · PCB"));
    assert_eq!(redone["result"]["step"]["title"], "Add a via", "{redone}");
    Ok(())
}

/// While the person holds the window (the command palette open, a pointer
/// held down), a change is refused with the reason and nothing changes; a
/// read still answers. The window's own file requests are refused.
#[test]
fn changes_wait_while_the_person_holds_the_window() -> Fallible {
    let mut h = Harness::new()?;
    let revision = h.app.engine.revision();
    let before = h.app.engine.document().clone();

    h.app.palette.open();
    h.frame(Vec::new());
    let refused = h.call(live::EXECUTE, h.add_via(revision))?;
    assert_eq!(code(&refused), "live.busy", "{refused}");
    let read = h.call(live::EXECUTE, json!({"command": "project.inspect"}))?;
    assert_eq!(read["ok"], true, "{read}");
    h.app.palette.close();
    h.frame(vec![egui::Event::Key {
        key: egui::Key::Escape,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    }]);

    let at = egui::pos2(700.0, 450.0);
    h.frame(vec![
        egui::Event::PointerMoved(at),
        egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        },
    ]);
    let dragging = h.call(live::EXECUTE, h.add_via(revision))?;
    assert_eq!(code(&dragging), "live.busy", "{dragging}");
    assert!(
        dragging["error"]["message"]
            .as_str()
            .is_some_and(|m| m.contains("dragging")),
        "{dragging}"
    );
    h.frame(vec![egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    }]);
    assert_eq!(h.app.engine.document(), &before);
    assert_eq!(h.app.engine.revision(), revision);

    let open = h.call(
        live::EXECUTE,
        json!({"command": "project.open", "params": {"expected_revision": revision, "path": "/tmp/x.pcbcraft"}}),
    )?;
    assert_eq!(code(&open), "live.window_only", "{open}");
    let missing = h.call(live::EXECUTE, json!({"command": "project.undo"}))?;
    assert_eq!(code(&missing), "live.revision_required", "{missing}");
    assert_eq!(h.app.engine.revision(), revision);
    Ok(())
}

/// An Ask-class request waits for the person: Deny answers the assistant
/// and writes nothing; Allow runs it.
#[test]
fn an_ask_request_waits_for_the_person() -> Fallible {
    let mut h = Harness::new()?;
    let revision = h.app.engine.revision();
    let dir = tempfile::tempdir_in(std::env::temp_dir())?;
    let path = dir.path().join("board.solidcraft");
    let export = json!({"command": "project.export_native",
        "params": {"path": path.display().to_string()}});

    let mut client = h.client()?;
    let sent = export.clone();
    let asked = std::thread::spawn(move || client.call(live::EXECUTE, sent));
    for _ in 0..5000 {
        if h.app.live_held().is_some() {
            break;
        }
        h.frame(Vec::new());
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert_eq!(h.app.live_held(), Some("project.export_native"));
    // Meanwhile another change is refused: the window waits for an answer.
    let waiting = h.call(live::EXECUTE, h.add_via(revision))?;
    assert_eq!(code(&waiting), "live.busy", "{waiting}");
    h.app.live_answer(false);
    let denied = asked.join().map_err(|_| "client thread")??;
    assert_eq!(code(&denied), "gate.denied", "{denied}");
    assert!(!path.exists());

    let mut client = h.client()?;
    let asked = std::thread::spawn(move || client.call(live::EXECUTE, export));
    for _ in 0..5000 {
        if h.app.live_held().is_some() {
            break;
        }
        h.frame(Vec::new());
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    h.app.live_answer(true);
    let allowed = asked.join().map_err(|_| "client thread")??;
    assert_eq!(allowed["ok"], true, "{allowed}");
    assert!(path.exists());
    Ok(())
}

/// Turning access off closes the socket: a bridge gets a clear failure.
#[test]
fn turning_access_off_closes_the_channel() -> Fallible {
    let mut h = Harness::new()?;
    let mut client = h.client()?;
    assert!(h.app.live.is_on());
    h.app.live.set_on(&h.ctx, false);
    assert!(!h.app.live.is_on());
    h.frame(Vec::new());
    assert!(client.call(live::REVISION, json!({})).is_err());
    Ok(())
}

/// I-A1: over live MCP, `library.source.add` is refused in every mode the
/// window offers, with nothing held for the person and the sources as they
/// were.
#[test]
fn live_mcp_never_adds_a_library_source() -> Fallible {
    let mut h = Harness::new()?;
    let dir = tempfile::tempdir_in(std::env::temp_dir())?;
    let add = json!({"command": "library.source.add",
        "params": {"path": dir.path().display().to_string()}});
    for mode in [Mode::AskEveryTime, Mode::AutoAllowEdits, Mode::AllowAll] {
        h.app.live.mode = mode;
        let reply = h.call(live::EXECUTE, add.clone())?;
        assert_eq!(code(&reply), "gate.never", "{mode:?}: {reply}");
        assert!(h.app.live_held().is_none(), "{mode:?}");
        assert!(h.app.live.session.sources().list().is_empty(), "{mode:?}");
    }
    Ok(())
}

/// I-A2: a read that names a path asks the person and, on Allow, runs: it
/// is not refused for a revision it has no field for.
#[test]
fn live_mcp_runs_a_path_read_on_allow() -> Fallible {
    let mut h = Harness::new()?;
    let dir = tempfile::tempdir_in(std::env::temp_dir())?;
    let part = dir.path().join("smd-nut.solidcraft");
    std::fs::copy(
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../library/parts/smd-nut.solidcraft"
        ),
        &part,
    )?;
    let check = json!({"command": "library.check_part_file",
        "params": {"path": part.display().to_string()}});
    let mut client = h.client()?;
    let asked = std::thread::spawn(move || client.call(live::EXECUTE, check));
    for _ in 0..5000 {
        if h.app.live_held().is_some() {
            break;
        }
        h.frame(Vec::new());
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert_eq!(h.app.live_held(), Some("library.check_part_file"));
    h.app.live_answer(true);
    let reply = asked.join().map_err(|_| "client thread")??;
    assert_eq!(reply["ok"], true, "{reply}");
    assert_eq!(reply["result"]["report"]["ok"], true, "{reply}");
    Ok(())
}

/// The access checkbox starts at the window's left edge, under the state
/// note, not after an empty label column.
#[test]
fn the_access_checkbox_starts_at_the_left() -> Fallible {
    let mut h = Harness::new()?;
    h.app.live.open = true;
    h.frame(Vec::new());
    let output = h.frame_output(Vec::new());
    let left = |pick: &dyn Fn(&str) -> bool| {
        output.shapes.iter().find_map(|s| match &s.shape {
            egui::Shape::Text(t) if pick(&t.galley.job.text) => Some(t.pos.x),
            _ => None,
        })
    };
    let check = left(&|t| t == "Let AI assistants edit this project").ok_or("no checkbox")?;
    let note = left(&|t| t.starts_with("On, on this computer only")).ok_or("no state note")?;
    let t = pcbcraft_ui::theme::Tokens::get();
    assert!(
        check - note < t.form_label_width / 2.0,
        "checkbox text at {check}, note at {note}"
    );
    Ok(())
}
