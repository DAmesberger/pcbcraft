//! The whole app, headless, driven through its accessibility tree.
//!
//! Each frame turns AccessKit on and keeps the tree egui hands the
//! platform. A test finds a widget by its role and accessible name, as a
//! screen reader or the VM driver does, and clicks it with real pointer
//! events at the centre of its bounds, so hit testing, layering and
//! enablement all take part. Keys and text go in as the window would send
//! them. No window, GPU or file dialog is involved.
use super::*;
use crate::commands::Registry;
use egui::accesskit::{NodeId, Role, Toggled, TreeUpdate};
use sparksmith_ui::fonts;
use std::collections::HashMap;

pub(crate) type Fallible<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// The window size of every harness frame, in points.
pub(crate) const WINDOW: egui::Vec2 = egui::vec2(1380.0, 900.0);

/// Input time between frames: a click's press and release stay within
/// egui's click time, and separate clicks are never a double-click.
const FRAME_SECONDS: f64 = 0.25;

/// One node of the last frame's accessibility tree.
#[derive(Clone, Debug)]
pub(crate) struct Node {
    pub role: Role,
    /// The accessible name: the node's label, a label's text, or the name
    /// of the node it is labelled by.
    pub name: String,
    pub rect: egui::Rect,
    pub disabled: bool,
    pub toggled: Option<bool>,
}

impl Node {
    pub(crate) fn center(&self) -> egui::Pos2 {
        self.rect.center()
    }
}

/// The app and the frames it is driven through.
pub(crate) struct Ax {
    pub app: App,
    pub ctx: egui::Context,
    pub dir: tempfile::TempDir,
    frame: eframe::Frame,
    time: f64,
    /// The window size, [`WINDOW`] unless a test sets another.
    pub window: egui::Vec2,
    /// The last frame's nodes, in tree order.
    nodes: Vec<Node>,
    /// The texts the last frame painted, whole (a cut text's full text).
    pub texts: Vec<String>,
}

impl Ax {
    /// The demo project on `workspace`, saved as a project file in a
    /// temporary folder, settled for a few frames.
    pub(crate) fn new(workspace: Workspace) -> Fallible<Self> {
        Self::with_engine(Engine::demo()?, workspace)
    }

    pub(crate) fn with_engine(engine: Engine, workspace: Workspace) -> Fallible<Self> {
        Self::with_sources(engine, workspace, LibrarySources::new(None))
    }

    /// [`Ax::with_engine`] with the user settings and sources `sources`.
    pub(crate) fn with_sources(
        engine: Engine,
        workspace: Workspace,
        sources: LibrarySources,
    ) -> Fallible<Self> {
        let dir = tempfile::tempdir()?;
        let ctx = egui::Context::default();
        fonts::setup_bundled_fonts(&ctx);
        let mut files = FileUi::with_directory(
            &ctx,
            rfd::AsyncFileDialog::new(),
            &engine,
            dir.path().to_path_buf(),
        );
        files.adopt(&engine, Some(dir.path().join("board.sparksmith")), false)?;
        let mut app = App::with_parts(
            &ctx,
            viewport::Viewport::detached(),
            Parts {
                engine,
                files,
                sources,
                registry: Arc::new(Registry::standard()),
                keymap_problems: Vec::new(),
                messages: Messages::default(),
                browser_width: chrome::BROWSER_WIDTH,
                #[cfg(target_os = "macos")]
                native_menu: None,
            },
        );
        app.workspace = workspace;
        crate::keys::publish(&ctx, &app.registry);
        let mut ax = Self {
            app,
            ctx,
            dir,
            frame: eframe::Frame::_new_kittest(),
            time: 0.0,
            window: WINDOW,
            nodes: Vec::new(),
            texts: Vec::new(),
        };
        ax.settle();
        Ok(ax)
    }

    /// The project file the app saves to.
    pub(crate) fn path(&self) -> std::path::PathBuf {
        self.dir.path().join("board.sparksmith")
    }

    /// One frame with `events`; keeps its accessibility tree.
    pub(crate) fn frame(&mut self, events: Vec<egui::Event>) {
        // Far enough apart that two clicks are never a double-click.
        self.time += FRAME_SECONDS;
        self.ctx.enable_accesskit();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, self.window)),
            time: Some(self.time),
            events,
            ..Default::default()
        };
        let Self {
            app, ctx, frame, ..
        } = self;
        let mut out = ctx.run_ui(input, |ui| eframe::App::ui(app, ui, frame));
        out.textures_delta.clear();
        self.texts = crate::kit::text::tests::painted(&out)
            .into_iter()
            .map(|(text, ..)| text)
            .collect();
        if let Some(update) = out.platform_output.accesskit_update.take() {
            self.nodes = nodes(&update);
        }
    }

    /// Frames until animations and deferred work have had their turn.
    pub(crate) fn settle(&mut self) {
        for _ in 0..6 {
            self.frame(vec![]);
        }
    }

    /// The tree as text, one node per line, for failure messages.
    pub(crate) fn dump(&self) -> String {
        self.nodes
            .iter()
            .filter(|n| !n.name.is_empty())
            .map(|n| {
                format!(
                    "{:?} {:?} @{:.0},{:.0}{}",
                    n.role,
                    n.name,
                    n.rect.center().x,
                    n.rect.center().y,
                    if n.disabled { " (disabled)" } else { "" }
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Every node of `role` named exactly `name`.
    pub(crate) fn all(&self, role: Role, name: &str) -> Vec<&Node> {
        self.nodes
            .iter()
            .filter(|n| n.role == role && n.name == name)
            .collect()
    }

    /// Whether a node of `role` named `name` is shown.
    pub(crate) fn has(&self, role: Role, name: &str) -> bool {
        !self.all(role, name).is_empty()
    }

    /// The one node of `role` named `name`; an error lists the tree.
    pub(crate) fn find(&self, role: Role, name: &str) -> Fallible<Node> {
        match self.all(role, name).as_slice() {
            [one] => Ok((*one).clone()),
            [] => Err(format!("no {role:?} named {name:?} in:\n{}", self.dump()).into()),
            many => Err(format!("{} {role:?}s named {name:?}", many.len()).into()),
        }
    }

    /// Every node of `role` whose name starts with `prefix`.
    pub(crate) fn all_prefix(&self, role: Role, prefix: &str) -> Vec<&Node> {
        self.nodes
            .iter()
            .filter(|n| n.role == role && n.name.starts_with(prefix))
            .collect()
    }

    /// The first node of `role` whose name starts with `prefix`.
    pub(crate) fn find_prefix(&self, role: Role, prefix: &str) -> Fallible<Node> {
        self.nodes
            .iter()
            .find(|n| n.role == role && n.name.starts_with(prefix))
            .cloned()
            .ok_or_else(|| format!("no {role:?} named {prefix:?}… in:\n{}", self.dump()).into())
    }

    /// Move the pointer to `at` for a frame, so the widget there is hovered.
    pub(crate) fn hover(&mut self, at: egui::Pos2) {
        self.frame(vec![egui::Event::PointerMoved(at)]);
    }

    /// A press and a release of `button` at `at`, with `modifiers`, each
    /// in its own frame after the pointer moved there.
    pub(crate) fn click_at_with(
        &mut self,
        at: egui::Pos2,
        button: egui::PointerButton,
        modifiers: egui::Modifiers,
    ) {
        self.hover(at);
        for pressed in [true, false] {
            self.frame(vec![egui::Event::PointerButton {
                pos: at,
                button,
                pressed,
                modifiers,
            }]);
        }
        // What the click opened (a menu, a review) is laid out and shown.
        self.frame(vec![]);
        self.frame(vec![]);
    }

    pub(crate) fn click_at(&mut self, at: egui::Pos2) {
        self.click_at_with(at, egui::PointerButton::Primary, egui::Modifiers::NONE);
    }

    pub(crate) fn right_click_at(&mut self, at: egui::Pos2) {
        self.click_at_with(at, egui::PointerButton::Secondary, egui::Modifiers::NONE);
    }

    /// Click the one enabled node of `role` named `name`.
    pub(crate) fn click(&mut self, role: Role, name: &str) -> Fallible {
        let node = self.find(role, name)?;
        if node.disabled {
            return Err(format!("{role:?} {name:?} is disabled").into());
        }
        self.click_at(node.center());
        Ok(())
    }

    /// Click the button named `name`.
    pub(crate) fn button(&mut self, name: &str) -> Fallible {
        self.click(Role::Button, name)
    }

    /// One key press and release with `modifiers`.
    pub(crate) fn key_with(&mut self, key: egui::Key, modifiers: egui::Modifiers) {
        let event = |pressed| egui::Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers,
        };
        self.frame(vec![event(true)]);
        self.frame(vec![event(false)]);
        self.frame(vec![]);
    }

    pub(crate) fn key(&mut self, key: egui::Key) {
        self.key_with(key, egui::Modifiers::NONE);
    }

    pub(crate) fn command_key(&mut self, key: egui::Key) {
        self.key_with(key, egui::Modifiers::COMMAND);
    }

    /// Text typed into whatever has the keyboard.
    pub(crate) fn type_text(&mut self, text: &str) {
        self.frame(vec![egui::Event::Text(text.into())]);
    }

    /// Frames until `done`, or an error naming `what`.
    pub(crate) fn until(&mut self, what: &str, mut done: impl FnMut(&Self) -> bool) -> Fallible {
        for _ in 0..500 {
            self.frame(vec![]);
            if done(self) {
                return Ok(());
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        Err(format!("{what} did not happen").into())
    }
}

/// The nodes of `update` in tree order, each with its accessible name.
fn nodes(update: &TreeUpdate) -> Vec<Node> {
    let index: HashMap<NodeId, usize> = update
        .nodes
        .iter()
        .enumerate()
        .map(|(i, (id, _))| (*id, i))
        .collect();
    let own = |node: &egui::accesskit::Node| -> Option<String> {
        node.label()
            .or_else(|| (node.role() == Role::Label).then(|| node.value()).flatten())
            .map(str::to_owned)
    };
    let name = |node: &egui::accesskit::Node| -> String {
        own(node)
            .or_else(|| {
                let names: Vec<String> = node
                    .labelled_by()
                    .iter()
                    .filter_map(|id| index.get(id))
                    .filter_map(|i| own(&update.nodes[*i].1))
                    .collect();
                (!names.is_empty()).then(|| names.join(" "))
            })
            .unwrap_or_default()
    };
    // Tree order, from the root down.
    let mut order = Vec::new();
    let root = update.tree.as_ref().map(|t| t.root);
    let mut stack: Vec<NodeId> = root.into_iter().collect();
    while let Some(id) = stack.pop() {
        let Some(&i) = index.get(&id) else { continue };
        order.push(i);
        stack.extend(update.nodes[i].1.children().iter().rev().copied());
    }
    order
        .into_iter()
        .map(|i| {
            let node = &update.nodes[i].1;
            let rect = node.bounds().map_or(egui::Rect::NOTHING, |b| {
                egui::Rect::from_min_max(
                    egui::pos2(b.x0 as f32, b.y0 as f32),
                    egui::pos2(b.x1 as f32, b.y1 as f32),
                )
            });
            Node {
                role: node.role(),
                name: name(node),
                rect,
                disabled: node.is_disabled(),
                toggled: node.toggled().map(|t| t == Toggled::True),
            }
        })
        .collect()
}
