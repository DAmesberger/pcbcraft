//! The command registry: every command the desktop app offers by name, in
//! which context it applies, its default shortcut and what running it
//! does. The command palette (⌘P), the Keyboard shortcuts sheet, ribbon
//! tooltips and the app's key dispatcher all read it, so a shortcut is
//! named in one place (docs/keyboard-shortcuts.md).
//!
//! Running a command never changes the document directly: a ribbon command
//! goes down the same path as its ribbon button, whose editor returns a
//! typed operation for the engine. Keys that only mean something inside an
//! active gesture (Enter finishing a polygon, V changing the routing layer)
//! stay with the canvas that owns the gesture; the registry lists them
//! ([`Handler::InPlace`]) so they are shown and checked for conflicts.
//!
//! Chord names follow Solidcraft's keymap (`keymap::key_name`: `Ctrl+Shift+S`,
//! where Ctrl is ⌘ on macOS), so a user keymap can later store them as
//! Solidcraft does ([`Registry::rebind`]).
use crate::shell::{RibbonEvent, Workspace};
use pcbcraft_core::CopperLayer;
use std::collections::BTreeMap;

/// Where a command applies. Contexts nest: a command of a parent context
/// applies inside its children ([`Context::parent`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Context {
    Global,
    Component,
    /// The Component workspace's Symbol tab.
    Symbol,
    /// The Component workspace's Footprint tab.
    Footprint,
    Schematic,
    /// Both PCB views.
    Pcb,
    PcbLayout,
    /// The Layout view while the Route tool is armed.
    PcbRoute,
    /// The Layout view while the Fill tool is armed.
    PcbFill,
    /// The Layout view while the board shape sketch is open.
    BoardShape,
    Pcb3d,
}

impl Context {
    /// Every context, in sheet order.
    pub const ALL: [Self; 11] = [
        Self::Global,
        Self::Component,
        Self::Symbol,
        Self::Footprint,
        Self::Schematic,
        Self::Pcb,
        Self::PcbLayout,
        Self::PcbRoute,
        Self::PcbFill,
        Self::BoardShape,
        Self::Pcb3d,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Global => "Everywhere",
            Self::Component => "Component",
            Self::Symbol => "Component › Symbol",
            Self::Footprint => "Component › Footprint",
            Self::Schematic => "Schematic",
            Self::Pcb => "PCB",
            Self::PcbLayout => "PCB › Layout",
            Self::PcbRoute => "PCB › Route tool",
            Self::PcbFill => "PCB › Fill tool",
            Self::BoardShape => "PCB › Board shape",
            Self::Pcb3d => "PCB › 3D",
        }
    }

    pub fn parent(self) -> Option<Self> {
        match self {
            Self::Global => None,
            Self::Component | Self::Schematic | Self::Pcb => Some(Self::Global),
            Self::Symbol | Self::Footprint => Some(Self::Component),
            Self::PcbLayout | Self::Pcb3d => Some(Self::Pcb),
            Self::PcbRoute | Self::PcbFill | Self::BoardShape => Some(Self::PcbLayout),
        }
    }

    /// A tool's own mode: its keys override the same keys of the contexts
    /// around it while it is active, by design.
    pub fn modal(self) -> bool {
        matches!(self, Self::PcbRoute | Self::PcbFill | Self::BoardShape)
    }

    /// A mode that replaces its workspace's keys: while it is active only
    /// its own keys and the Everywhere commands apply (the board shape
    /// sketch has its own ribbon).
    pub fn exclusive(self) -> bool {
        self == Self::BoardShape
    }

    /// Whether `self` is `other` or lies inside it.
    pub fn within(self, other: Self) -> bool {
        let mut at = Some(self);
        while let Some(context) = at {
            if context == other {
                return true;
            }
            at = context.parent();
        }
        false
    }
}

/// One key with its modifiers. `command` is ⌘ on macOS and Ctrl elsewhere.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Chord {
    pub key: egui::Key,
    pub command: bool,
    pub shift: bool,
    pub alt: bool,
}

impl Chord {
    pub const fn key(key: egui::Key) -> Self {
        Self {
            key,
            command: false,
            shift: false,
            alt: false,
        }
    }
    pub const fn cmd(key: egui::Key) -> Self {
        Self {
            command: true,
            ..Self::key(key)
        }
    }
    pub const fn with_shift(self) -> Self {
        Self {
            shift: true,
            ..self
        }
    }

    pub fn modifiers(self) -> egui::Modifiers {
        let mut m = egui::Modifiers::NONE;
        if self.command {
            m |= egui::Modifiers::COMMAND;
        }
        if self.shift {
            m |= egui::Modifiers::SHIFT;
        }
        if self.alt {
            m |= egui::Modifiers::ALT;
        }
        m
    }

    /// The canonical name, as Solidcraft's keymap stores it: `Ctrl+Shift+S`.
    #[allow(dead_code, reason = "user keymaps store it; no preferences page yet")]
    pub fn name(self) -> String {
        solidcraft_ui_egui::keymap::key_name(self.key, self.modifiers())
    }

    /// A chord from its canonical name or its macOS glyphs (`Shift+V`,
    /// `Ctrl+Z`, `⇧V`, `⌘Z`). `None` for a key name egui does not know.
    pub fn parse(text: &str) -> Option<Self> {
        let mut chord = Self::key(egui::Key::Escape);
        let mut rest = text.trim();
        loop {
            if let Some(r) = rest.strip_prefix('⌘') {
                chord.command = true;
                rest = r;
            } else if let Some(r) = rest.strip_prefix('⇧') {
                chord.shift = true;
                rest = r;
            } else if let Some(r) = rest.strip_prefix('⌥') {
                chord.alt = true;
                rest = r;
            } else {
                break;
            }
        }
        let mut parts: Vec<&str> = rest.split('+').map(str::trim).collect();
        // `Ctrl++`: the key itself is a plus.
        if rest.ends_with("++") {
            parts.retain(|p| !p.is_empty());
            parts.push("+");
        }
        let key = parts.pop()?;
        for part in parts {
            match part {
                "Ctrl" | "Cmd" | "Command" => chord.command = true,
                "Shift" => chord.shift = true,
                "Alt" | "Option" => chord.alt = true,
                _ => return None,
            }
        }
        chord.key = match key {
            "Esc" => egui::Key::Escape,
            "Del" => egui::Key::Delete,
            "Return" => egui::Key::Enter,
            other => egui::Key::from_name(other)?,
        };
        Some(chord)
    }

    /// How the chord is shown: macOS glyphs in Apple's order (⌃⌥⇧⌘, then
    /// the key), elsewhere `Ctrl+Alt+Shift+K`.
    pub fn display(self, mac: bool) -> String {
        let key = key_label(self.key);
        if mac {
            let mut s = String::new();
            if self.alt {
                s.push('⌥');
            }
            if self.shift {
                s.push('⇧');
            }
            if self.command {
                s.push('⌘');
            }
            s.push_str(key);
            s
        } else {
            let mut parts = Vec::new();
            if self.command {
                parts.push("Ctrl");
            }
            if self.alt {
                parts.push("Alt");
            }
            if self.shift {
                parts.push("Shift");
            }
            parts.push(key);
            parts.join("+")
        }
    }

    /// [`Self::display`] for this platform.
    pub fn shown(self) -> String {
        self.display(cfg!(target_os = "macos"))
    }

    /// Whether the chord was pressed this frame; consumes it if so. Shift
    /// and Alt must match exactly; with neither held, a plain key does not
    /// fire while ⌘/Ctrl is down.
    pub fn consume(self, ctx: &egui::Context) -> bool {
        ctx.input_mut(|i| {
            let held = i.modifiers;
            if held.shift != self.shift || held.alt != self.alt || held.command != self.command {
                return false;
            }
            i.consume_key(self.modifiers(), self.key)
        })
    }
}

/// The key cap text of one key: letters as they are, Esc, arrows by name.
fn key_label(key: egui::Key) -> &'static str {
    match key {
        egui::Key::Escape => "Esc",
        egui::Key::ArrowUp
        | egui::Key::ArrowDown
        | egui::Key::ArrowLeft
        | egui::Key::ArrowRight
        | egui::Key::Minus => key.name(),
        other => other.symbol_or_name(),
    }
}

/// Keys handled in place whose handler and registry entry share one
/// definition.
pub mod keys {
    use super::Chord;
    use egui::Key;
    pub const COMPONENT_NEW: Chord = Chord::cmd(Key::N).with_shift();
    pub const COMPONENT_OPEN: Chord = Chord::cmd(Key::O).with_shift();
    pub const COMPONENT_BACK: Chord = Chord::cmd(Key::OpenBracket);
    pub const COMPONENT_FORWARD: Chord = Chord::cmd(Key::CloseBracket);
}

/// Keys a tool mode deliberately takes from the contexts around it while
/// the tool is armed: (mode, key, the command it shadows). Any other key a
/// mode shares with its surroundings is a conflict
/// (`the_default_keymap_has_no_conflicts`), and every entry must shadow a
/// command that exists (`every_shadow_is_used`).
pub const SHADOWS: &[(Context, Chord, &str)] = &[
    // Routing: V places a via instead of arming the Via tool, F flips the
    // elbow instead of fitting the view, D cycles the via size instead of
    // switching to 3D, and Esc ends the route before it cancels the tool.
    (Context::PcbRoute, Chord::key(egui::Key::V), "pcb.via"),
    (Context::PcbRoute, Chord::key(egui::Key::F), "view.fit"),
    (
        Context::PcbRoute,
        Chord::key(egui::Key::F).with_shift(),
        "pcb.from-bottom",
    ),
    (Context::PcbRoute, Chord::key(egui::Key::D), "pcb.toggle-3d"),
    (
        Context::PcbRoute,
        Chord::key(egui::Key::Escape),
        "pcb.cancel",
    ),
    // The Fill tool's Esc drops its outline first.
    (
        Context::PcbFill,
        Chord::key(egui::Key::Escape),
        "pcb.cancel",
    ),
    // The board shape sketch hides the Layout keys; of the Everywhere
    // keys, F is its Fillet.
    (Context::BoardShape, Chord::key(egui::Key::F), "view.fit"),
];

/// The Fill tool's keys, handled by the PCB canvas (`fill_tool`).
pub const FILL_KEYS: &[(&str, &str)] = &[
    ("A", "Switch lines and arcs"),
    ("Enter", "Close the outline"),
    ("Backspace", "Take back the last point"),
    ("Esc", "Drop the outline"),
];

/// Who acts on a command's key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handler {
    /// The app's dispatcher, after the canvas had its turn.
    Registry,
    /// The canvas or card that owns the gesture; listed only.
    InPlace,
}

/// App-level commands, run by the app itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppCommand {
    Palette,
    Shortcuts,
    New,
    Open,
    Save,
    SaveAs,
    Export,
    Undo,
    Redo,
    ToggleBrowser,
    ToggleIssues,
    Messages,
    LibrarySources,
    Workspace(Workspace),
    FitView,
    /// Switch the PCB between its Layout and 3D views.
    TogglePcb3d,
}

/// PCB display toggles: view state only, never the document.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PcbView {
    Copper(CopperLayer),
    Vias,
    SilkTop,
    SilkBottom,
    Courtyard,
    Assembly,
    Holes,
    /// The 3D view's component bodies.
    Components,
    FromBottom,
    Airwires,
    HighlightNet,
}

/// What running a command does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    App(AppCommand),
    /// The same event as the workspace's ribbon button or tab.
    Ribbon(Workspace, RibbonEvent),
    Pcb(PcbView),
    /// A key of a gesture: shown, never run from the palette.
    Key,
}

/// One registry entry.
#[derive(Clone, Debug, PartialEq)]
pub struct Command {
    pub id: String,
    pub title: String,
    pub context: Context,
    /// The default chord; [`Registry::chord`] gives the effective one.
    pub chord: Option<Chord>,
    /// A key that is not one chord ("Click", "V, ⇧V", "1–9"), shown as is.
    pub key_text: Option<String>,
    /// Further chords of a key-table row ("V, Shift+V", "1 to 9"); they
    /// count for lookup and conflicts like `chord`.
    pub also: Vec<Chord>,
    pub handler: Handler,
    pub action: Action,
    pub icon: &'static str,
    /// Other words the command is found by.
    pub keywords: &'static str,
}

impl Command {
    fn new(
        id: impl Into<String>,
        title: impl Into<String>,
        context: Context,
        action: Action,
    ) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            context,
            chord: None,
            key_text: None,
            also: Vec::new(),
            handler: Handler::Registry,
            action,
            icon: "params",
            keywords: "",
        }
    }
    fn chord(mut self, chord: Chord) -> Self {
        self.chord = Some(chord);
        self
    }
    fn maybe(mut self, chord: Option<Chord>) -> Self {
        self.chord = chord;
        self
    }
    fn in_place(mut self) -> Self {
        self.handler = Handler::InPlace;
        self
    }
    fn icon(mut self, icon: &'static str) -> Self {
        self.icon = icon;
        self
    }
    fn keywords(mut self, keywords: &'static str) -> Self {
        self.keywords = keywords;
        self
    }

    /// Whether the palette can run it.
    pub fn runnable(&self) -> bool {
        self.action != Action::Key
    }
}

/// A tool of an editor's tool table, for the registry: the tool's ribbon
/// id, label, icon and default key.
#[derive(Clone, Copy, Debug)]
pub struct ToolEntry {
    pub id: &'static str,
    pub label: &'static str,
    pub icon: &'static str,
    pub chord: Option<Chord>,
}

/// Two commands that would take the same chord in overlapping contexts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conflict {
    pub chord: Chord,
    pub first: String,
    pub second: String,
}

/// The registry. Built once per app; user keymaps go through
/// [`Self::rebind`].
#[derive(Clone, Debug, Default)]
pub struct Registry {
    commands: Vec<Command>,
    /// Command id → the user's chord (`None`: no key). Only the changed ones.
    custom: BTreeMap<String, Option<Chord>>,
}

impl Registry {
    /// Pcbcraft's default keymap (docs/keyboard-shortcuts.md) with the
    /// editors' tool tables.
    pub fn standard() -> Self {
        let mut commands = defaults();
        commands.extend(tools(
            Context::Symbol,
            "symbol",
            Workspace::Part,
            &crate::part_editor::symbol_tool_entries(),
        ));
        commands.extend(tools(
            Context::Footprint,
            "footprint",
            Workspace::Part,
            &crate::part_editor::footprint_tool_entries(),
        ));
        commands.extend(tools(
            Context::BoardShape,
            "shape",
            Workspace::Pcb,
            &crate::pcb_editor::shape_tool_entries(),
        ));
        Self {
            commands,
            custom: BTreeMap::new(),
        }
        .with_key_table(Context::PcbRoute, crate::pcb_editor::ROUTE_KEYS)
        .with_key_table(Context::PcbFill, FILL_KEYS)
    }

    /// Add a gesture's key table (key, what it does) for display, e.g. a
    /// tool's `ROUTE_KEYS`. A key cell names one or more keys as
    /// [`key_cell`] reads them; a cell it cannot read ("Click") is shown
    /// as written and takes no key.
    pub fn with_key_table(mut self, context: Context, table: &[(&str, &str)]) -> Self {
        let prefix = format!("keys.{context:?}").to_lowercase();
        for (index, (key, what)) in table.iter().enumerate() {
            let mut command =
                Command::new(format!("{prefix}.{index}"), *what, context, Action::Key)
                    .in_place()
                    .icon("params");
            match key_cell(key) {
                Some((chords, shown)) => {
                    let mut chords = chords.into_iter();
                    command.chord = chords.next();
                    command.also = chords.collect();
                    // One chord shows as a chord; several as their cell.
                    if !command.also.is_empty() {
                        command.key_text = Some(shown);
                    }
                }
                None => command.key_text = Some((*key).to_owned()),
            }
            self.commands.push(command);
        }
        self
    }

    pub fn commands(&self) -> &[Command] {
        &self.commands
    }

    pub fn get(&self, id: &str) -> Option<&Command> {
        self.commands.iter().find(|c| c.id == id)
    }

    /// The effective chord of a command: the user's, else the default.
    pub fn chord(&self, command: &Command) -> Option<Chord> {
        match self.custom.get(&command.id) {
            Some(custom) => *custom,
            None => command.chord,
        }
    }

    /// Every chord the command takes: its effective one and a key-table
    /// row's others.
    pub fn chords(&self, command: &Command) -> Vec<Chord> {
        self.chord(command)
            .into_iter()
            .chain(command.also.iter().copied())
            .collect()
    }

    /// The shortcut as shown: a several-key cell as its text, else the
    /// chord, else the text.
    pub fn shortcut(&self, command: &Command) -> Option<String> {
        if !command.also.is_empty() {
            return command.key_text.clone();
        }
        self.chord(command)
            .map(Chord::shown)
            .or_else(|| command.key_text.clone())
    }

    /// The command a ribbon button runs in `workspace`, for its tooltip.
    pub fn for_tool(&self, workspace: Workspace, tool: &str) -> Option<&Command> {
        self.commands.iter().find(|c| {
            matches!(c.action, Action::Ribbon(w, RibbonEvent::Tool(id)) if w == workspace && id == tool)
        })
    }

    /// The command an app command runs (the app bar's buttons).
    pub fn for_app(&self, app: AppCommand) -> Option<&Command> {
        self.commands.iter().find(|c| c.action == Action::App(app))
    }

    /// Change a command's key; `None` clears it. Refused when the id is
    /// unknown or the chord conflicts with another command.
    #[allow(dead_code, reason = "user keymaps: no preferences page yet")]
    pub fn rebind(&mut self, id: &str, chord: Option<Chord>) -> Result<(), String> {
        let Some(command) = self.get(id) else {
            return Err(format!("No command {id}"));
        };
        if let Some(chord) = chord
            && let Some(other) = self
                .commands
                .iter()
                .find(|c| c.id != id && self.chords(c).contains(&chord) && clash(command, c, chord))
        {
            return Err(format!("{} already uses {}", other.title, chord.shown()));
        }
        let default = command.chord;
        if chord == default {
            self.custom.remove(id);
        } else {
            self.custom.insert(id.to_owned(), chord);
        }
        Ok(())
    }

    /// The command `chord` runs while `active` holds (most specific first,
    /// as [`active_contexts`] builds it). A modal context's keys win over
    /// its parents'; an exclusive one hides its workspace's keys.
    pub fn resolve(&self, chord: Chord, active: &[Context]) -> Option<&Command> {
        let exclusive = active.iter().find(|c| c.exclusive()).copied();
        active
            .iter()
            .filter(|context| match exclusive {
                Some(mode) => context.within(mode) || **context == Context::Global,
                None => true,
            })
            .find_map(|context| {
                self.commands
                    .iter()
                    .find(|c| c.context == *context && self.chords(c).contains(&chord))
            })
    }

    /// Every pair of commands sharing a chord where both could apply: the
    /// same context, or one inside the other, unless an exclusive mode
    /// hides the outer one or [`SHADOWS`] lists the pair.
    pub fn conflicts(&self) -> Vec<Conflict> {
        let mut out = Vec::new();
        for (i, a) in self.commands.iter().enumerate() {
            for chord in self.chords(a) {
                for b in self.commands.iter().skip(i + 1) {
                    if self.chords(b).contains(&chord) && clash(a, b, chord) {
                        out.push(Conflict {
                            chord,
                            first: a.id.clone(),
                            second: b.id.clone(),
                        });
                    }
                }
            }
        }
        out
    }

    /// Runnable commands matching `query`, best first. An empty query lists
    /// `recent` first (most recent first), then the commands of the active
    /// contexts, then the rest, each by title.
    pub fn search(&self, query: &str, recent: &[String], active: &[Context]) -> Vec<&Command> {
        let q = query.trim().to_lowercase();
        let recency = |c: &Command| recent.iter().position(|r| *r == c.id);
        let here = |c: &Command| active.contains(&c.context);
        let mut hits: Vec<(u32, &Command)> = self
            .commands
            .iter()
            .filter(|c| c.runnable())
            .filter_map(|c| {
                let score = match_score(&q, c);
                (score > 0).then_some((score, c))
            })
            .collect();
        hits.sort_by(|(sa, a), (sb, b)| {
            let rank = |s: &u32, c: &Command| {
                // Recent and in-context commands rank first among equals;
                // with an empty query, recent ones lead outright.
                let recent_rank = recency(c).map_or(usize::MAX, |p| p);
                (
                    std::cmp::Reverse(if q.is_empty() { 0 } else { *s }),
                    recent_rank,
                    !here(c),
                )
            };
            rank(sa, a)
                .cmp(&rank(sb, b))
                .then_with(|| a.title.cmp(&b.title))
                .then_with(|| a.context.cmp(&b.context))
        });
        hits.into_iter().map(|(_, c)| c).collect()
    }
}

/// How well `q` (lower case) finds `command`: its title first, then its
/// keywords, context and id. 0: no match.
pub fn match_score(q: &str, command: &Command) -> u32 {
    use solidcraft_ui_egui::shortcut_box::score;
    if q.is_empty() {
        return 1;
    }
    let title = score(q, &command.title);
    // Every word of the query found in the title, keywords or context.
    let words = format!(
        "{} {} {}",
        command.title,
        command.keywords,
        command.context.label()
    )
    .to_lowercase();
    let all_words = q.split_whitespace().all(|w| {
        words
            .split(|c: char| !c.is_alphanumeric())
            .any(|k| k.starts_with(w))
    });
    let other = if all_words { 700 } else { 0 };
    let id = score(q, &command.id) / 2;
    title.max(other).max(id)
}

/// Whether `a` and `b` sharing `chord` is a conflict: both could see the
/// key and no [`SHADOWS`] entry says the inner one takes it on purpose.
fn clash(a: &Command, b: &Command, chord: Chord) -> bool {
    let (inner, outer) = if a.context.within(b.context) {
        (a, b)
    } else if b.context.within(a.context) {
        (b, a)
    } else {
        return false;
    };
    if inner.context == outer.context {
        return true;
    }
    match mode_between(inner.context, outer.context) {
        None => true,
        // An exclusive mode hides everything around it but Everywhere.
        Some(mode) if mode.exclusive() && outer.context != Context::Global => false,
        Some(mode) => !SHADOWS
            .iter()
            .any(|(m, c, id)| *m == mode && *c == chord && *id == outer.id),
    }
}

/// The tool mode between `inner` (inclusive) and `outer` (exclusive).
fn mode_between(inner: Context, outer: Context) -> Option<Context> {
    let mut at = Some(inner);
    while let Some(context) = at {
        if context == outer {
            return None;
        }
        if context.modal() {
            return Some(context);
        }
        at = context.parent();
    }
    None
}

/// The keys of a key-table cell and how the cell is shown: chords joined by
/// commas or "or" (`V, Shift+V`), or a digit range (`1 to 9`, `1–9`).
/// `None` when a part is not a key ("Click").
pub fn key_cell(cell: &str) -> Option<(Vec<Chord>, String)> {
    let mut chords = Vec::new();
    let mut shown = Vec::new();
    for part in cell.split(',').flat_map(|p| p.split(" or ")) {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if let Some((from, to)) = part.split_once(" to ").or_else(|| part.split_once('–')) {
            let digit = |s: &str| s.trim().parse::<u8>().ok().filter(|d| *d <= 9);
            let (from, to) = (digit(from)?, digit(to)?);
            if from > to {
                return None;
            }
            for d in from..=to {
                chords.push(Chord::key(egui::Key::from_name(&d.to_string())?));
            }
            shown.push(format!("{from}–{to}"));
        } else {
            let chord = Chord::parse(part)?;
            shown.push(chord.shown());
            chords.push(chord);
        }
    }
    (!chords.is_empty()).then(|| (chords, shown.join(", ")))
}

/// The context stack for `innermost` (most specific first, Everywhere last).
pub fn active_contexts(innermost: Context) -> Vec<Context> {
    let mut out = Vec::new();
    let mut at = Some(innermost);
    while let Some(context) = at {
        out.push(context);
        at = context.parent();
    }
    out
}

fn tools(
    context: Context,
    prefix: &str,
    workspace: Workspace,
    entries: &[ToolEntry],
) -> Vec<Command> {
    entries
        .iter()
        .map(|t| {
            Command::new(
                format!("{prefix}.{}", t.id),
                t.label,
                context,
                Action::Ribbon(workspace, RibbonEvent::Tool(t.id)),
            )
            .maybe(t.chord)
            .in_place()
            .icon(t.icon)
        })
        .collect()
}

/// The default keymap: application, workspace and PCB commands, and the
/// keys of gestures that stay with their canvas.
fn defaults() -> Vec<Command> {
    use AppCommand as A;
    use Context as C;
    use egui::Key as K;
    let app = |id: &str, title: &str, command: AppCommand| {
        Command::new(id, title, C::Global, Action::App(command))
    };
    let tool = |id: &str, title: &str, context: Context, workspace: Workspace, tool| {
        Command::new(
            id,
            title,
            context,
            Action::Ribbon(workspace, RibbonEvent::Tool(tool)),
        )
    };
    let tab = |id: &str, title: &str, context: Context, workspace: Workspace, tab| {
        Command::new(
            id,
            title,
            context,
            Action::Ribbon(workspace, RibbonEvent::Tab(tab)),
        )
    };
    let key = |id: &str, title: &str, context: Context, chord: Chord| {
        Command::new(id, title, context, Action::Key)
            .chord(chord)
            .in_place()
    };
    let view = |id: &str, title: &str, context: Context, toggle: PcbView| {
        Command::new(id, title, context, Action::Pcb(toggle))
            .icon("display")
            .keywords("show hide toggle layer visibility view")
    };
    let mut v = vec![
        // Everywhere.
        app("app.palette", "Command palette", A::Palette)
            .chord(Chord::cmd(K::P))
            .icon("zoom")
            .keywords("search find run commands"),
        app("app.palette-alt", "Command palette", A::Palette)
            .chord(Chord::cmd(K::P).with_shift())
            .icon("zoom")
            .keywords("search find run commands"),
        app("app.shortcuts", "Keyboard shortcuts", A::Shortcuts)
            .chord(Chord::cmd(K::Slash))
            .icon("list")
            .keywords("keys keymap hotkeys help cheat sheet"),
        app("file.new", "New project", A::New)
            .chord(Chord::cmd(K::N))
            .icon("new"),
        app("file.open", "Open project…", A::Open)
            .chord(Chord::cmd(K::O))
            .icon("open"),
        app("file.save", "Save project", A::Save)
            .chord(Chord::cmd(K::S))
            .icon("save"),
        app("file.save-as", "Save project as…", A::SaveAs)
            .chord(Chord::cmd(K::S).with_shift())
            .icon("save"),
        app("file.export", "Export to Solidcraft…", A::Export)
            .chord(Chord::cmd(K::E).with_shift())
            .icon("save")
            .keywords("solidcraft 3d model native parametric"),
        app("edit.undo", "Undo", A::Undo)
            .chord(Chord::cmd(K::Z))
            .icon("undo"),
        app("edit.redo", "Redo", A::Redo)
            .chord(Chord::cmd(K::Z).with_shift())
            .icon("redo"),
        app("view.browser", "Show or hide the Browser", A::ToggleBrowser)
            .chord(Chord::cmd(K::B))
            .icon("list")
            .keywords("project tree sidebar panel"),
        app("view.issues", "Show or hide Issues", A::ToggleIssues)
            .chord(Chord::cmd(K::I).with_shift())
            .icon("list")
            .keywords("problems checks erc drc errors warnings"),
        app("view.messages", "Messages…", A::Messages)
            .chord(Chord::cmd(K::M).with_shift())
            .icon("list")
            .keywords("log history notifications"),
        app("library.sources", "Library sources…", A::LibrarySources)
            .icon("component")
            .keywords("folders libraries rescan"),
        app("view.fit", "Fit view", A::FitView)
            .chord(Chord::key(K::F))
            .icon("fit")
            .keywords("zoom extents frame all"),
        app(
            "workspace.component",
            "Go to Component",
            A::Workspace(Workspace::Part),
        )
        .chord(Chord::cmd(K::Num1))
        .icon("component")
        .keywords("workspace symbol footprint part editor"),
        app(
            "workspace.schematic",
            "Go to Schematic",
            A::Workspace(Workspace::Schematic),
        )
        .chord(Chord::cmd(K::Num2))
        .icon("sketch")
        .keywords("workspace"),
        app("workspace.pcb", "Go to PCB", A::Workspace(Workspace::Pcb))
            .chord(Chord::cmd(K::Num3))
            .icon("plane")
            .keywords("workspace board layout"),
        // Component (in place: the start page and history keys).
        Command::new(
            "component.new",
            "New component…",
            C::Component,
            Action::Ribbon(Workspace::Part, RibbonEvent::Tool("component-new")),
        )
        .chord(keys::COMPONENT_NEW)
        .in_place()
        .icon("new")
        .keywords("template gallery part"),
        Command::new(
            "component.open",
            "Open component…",
            C::Component,
            Action::Ribbon(Workspace::Part, RibbonEvent::Tool("component-open")),
        )
        .chord(keys::COMPONENT_OPEN)
        .in_place()
        .icon("open")
        .keywords("catalog library find part"),
        tool(
            "component.publish",
            "Publish component",
            C::Component,
            Workspace::Part,
            "publish",
        )
        .icon("save")
        .keywords("revision release"),
        key(
            "component.back",
            "Previous component",
            C::Component,
            keys::COMPONENT_BACK,
        ),
        key(
            "component.forward",
            "Next component",
            C::Component,
            keys::COMPONENT_FORWARD,
        ),
        // Symbol and Footprint canvas keys.
        key(
            "symbol.repeat",
            "Repeat the last tool",
            C::Symbol,
            Chord::key(K::R).with_shift(),
        ),
        key(
            "symbol.finish",
            "Finish the chain",
            C::Symbol,
            Chord::key(K::Enter),
        ),
        key(
            "symbol.cancel",
            "Cancel the tool",
            C::Symbol,
            Chord::key(K::Escape),
        ),
        key(
            "footprint.repeat",
            "Repeat the last tool",
            C::Footprint,
            Chord::key(K::R).with_shift(),
        ),
        key(
            "footprint.layer",
            "Switch construction and silkscreen",
            C::Footprint,
            Chord::key(K::X),
        ),
        key(
            "footprint.turn",
            "Turn the next pad a quarter",
            C::Footprint,
            Chord::key(K::Space),
        ),
        key(
            "footprint.delete",
            "Delete selection",
            C::Footprint,
            Chord::key(K::Delete),
        ),
        key(
            "footprint.cancel",
            "Cancel the tool",
            C::Footprint,
            Chord::key(K::Escape),
        ),
        // Schematic.
        tool(
            "schematic.select",
            "Select",
            C::Schematic,
            Workspace::Schematic,
            "select",
        )
        .icon("select"),
        tool(
            "schematic.place",
            "Place component",
            C::Schematic,
            Workspace::Schematic,
            "place",
        )
        .chord(Chord::key(K::P))
        .in_place()
        .icon("component")
        .keywords("add symbol part"),
        tool(
            "schematic.wire",
            "Wire",
            C::Schematic,
            Workspace::Schematic,
            "wire",
        )
        .chord(Chord::key(K::W))
        .icon("line")
        .keywords("connect net"),
        tool(
            "schematic.rotate",
            "Rotate symbol",
            C::Schematic,
            Workspace::Schematic,
            "rotate",
        )
        .chord(Chord::cmd(K::R))
        .in_place()
        .icon("spin"),
        tool(
            "schematic.mirror",
            "Mirror symbol",
            C::Schematic,
            Workspace::Schematic,
            "mirror",
        )
        .chord(Chord::key(K::X).with_shift())
        .in_place()
        .icon("mirror"),
        tool(
            "schematic.resolve",
            "Resolve parts",
            C::Schematic,
            Workspace::Schematic,
            "resolve",
        )
        .icon("component")
        .keywords("manufacturer part bom"),
        key(
            "schematic.cancel",
            "Cancel the tool",
            C::Schematic,
            Chord::key(K::Escape),
        ),
        // PCB, both views.
        Command::new(
            "pcb.toggle-3d",
            "Switch Layout and 3D",
            C::Pcb,
            Action::App(A::TogglePcb3d),
        )
        .chord(Chord::key(K::D))
        .icon("plane")
        .keywords("view 2d 3d layout"),
        tab(
            "pcb.layout",
            "Layout view",
            C::Pcb,
            Workspace::Pcb,
            "layout",
        )
        .icon("sketch")
        .keywords("2d view"),
        tab("pcb.3d", "3D view", C::Pcb, Workspace::Pcb, "3d")
            .icon("plane")
            .keywords("view model"),
        tool(
            "pcb.update",
            "Update PCB from schematic",
            C::Pcb,
            Workspace::Pcb,
            "update",
        )
        .chord(Chord::cmd(K::U).with_shift())
        .icon("component")
        .keywords("sync forward annotate"),
        // PCB Layout tools.
        tool(
            "pcb.select",
            "Select",
            C::PcbLayout,
            Workspace::Pcb,
            "select",
        )
        .icon("move"),
        tool("pcb.route", "Route", C::PcbLayout, Workspace::Pcb, "route")
            .chord(Chord::key(K::X))
            .icon("trace")
            .keywords("track trace copper wire"),
        tool("pcb.via", "Via", C::PcbLayout, Workspace::Pcb, "via")
            .chord(Chord::key(K::V))
            .icon("circle")
            .keywords("hole through"),
        tool("pcb.fill", "Fill", C::PcbLayout, Workspace::Pcb, "fill")
            .chord(Chord::key(K::P))
            .icon("polygon")
            .keywords("pour zone plane copper area"),
        tool(
            "pcb.refill",
            "Refill all fills",
            C::Pcb,
            Workspace::Pcb,
            "refill",
        )
        .chord(Chord::key(K::P).with_shift())
        .icon("update")
        .keywords("pour zone recompute"),
        tool(
            "pcb.production",
            "Production files…",
            C::Pcb,
            Workspace::Pcb,
            "production",
        )
        .icon("export")
        .keywords("gerber drill excellon fabrication manufacturing bom pick place zip"),
        tool(
            "pcb.board-shape",
            "Board shape",
            C::PcbLayout,
            Workspace::Pcb,
            "board-shape",
        )
        .chord(Chord::key(K::B))
        .icon("sketch")
        .keywords("outline edge cutout sketch"),
        key(
            "pcb.cancel",
            "Cancel the tool or gesture",
            C::PcbLayout,
            Chord::key(K::Escape),
        ),
        view(
            "pcb.from-bottom",
            "View from bottom",
            C::PcbLayout,
            PcbView::FromBottom,
        )
        .chord(Chord::key(K::F).with_shift())
        .keywords("flip side underside"),
        view(
            "pcb.highlight-net",
            "Highlight complete net",
            C::PcbLayout,
            PcbView::HighlightNet,
        )
        .chord(Chord::key(K::H))
        .keywords("net highlight selection"),
        view(
            "pcb.airwires",
            "Show or hide airwires",
            C::PcbLayout,
            PcbView::Airwires,
        )
        .keywords("ratsnest unrouted connections"),
        // The Route tool reads F with any Shift, so ⇧F flips the elbow too
        // while routing (and must not also flip the board view).
        key(
            "route.flip-shift",
            "Flip the elbow corner",
            C::PcbRoute,
            Chord::key(K::F).with_shift(),
        ),
        // Board shape keys besides its tools.
        key(
            "shape.construction",
            "Construction geometry on or off",
            C::BoardShape,
            Chord::key(K::X),
        ),
        key(
            "shape.delete",
            "Delete selection",
            C::BoardShape,
            Chord::key(K::Delete),
        ),
        key(
            "shape.cancel",
            "Cancel the tool",
            C::BoardShape,
            Chord::key(K::Escape),
        ),
    ];
    // Layer and display toggles: in the palette; no default keys.
    for (n, layer) in [
        CopperLayer::Top,
        CopperLayer::Bottom,
        CopperLayer::Inner(1),
        CopperLayer::Inner(2),
        CopperLayer::Inner(3),
        CopperLayer::Inner(4),
    ]
    .into_iter()
    .enumerate()
    {
        v.push(view(
            &format!("pcb.layer.copper-{n}"),
            &format!("Show or hide {} copper", layer.name()),
            C::Pcb,
            PcbView::Copper(layer),
        ));
    }
    for (id, title, toggle, context) in [
        ("vias", "Show or hide vias", PcbView::Vias, C::Pcb),
        (
            "silk-top",
            "Show or hide Top silkscreen",
            PcbView::SilkTop,
            C::Pcb,
        ),
        (
            "silk-bottom",
            "Show or hide Bottom silkscreen",
            PcbView::SilkBottom,
            C::Pcb,
        ),
        (
            "courtyard",
            "Show or hide courtyards",
            PcbView::Courtyard,
            C::PcbLayout,
        ),
        (
            "assembly",
            "Show or hide the assembly drawing",
            PcbView::Assembly,
            C::PcbLayout,
        ),
        ("holes", "Show or hide holes", PcbView::Holes, C::PcbLayout),
        (
            "components",
            "Show or hide component bodies",
            PcbView::Components,
            C::Pcb3d,
        ),
    ] {
        v.push(view(&format!("pcb.layer.{id}"), title, context, toggle));
    }
    v
}

#[cfg(test)]
#[path = "commands_tests.rs"]
mod tests;
