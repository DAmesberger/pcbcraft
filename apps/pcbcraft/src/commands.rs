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
    /// The Schematic canvas while the Wire tool is armed.
    SchematicWire,
    /// Both PCB views.
    Pcb,
    PcbLayout,
    /// The Layout view while the Route tool is armed.
    PcbRoute,
    /// The Layout view while the Fill tool is armed.
    PcbFill,
    /// The Layout view while the Tune tool is armed.
    PcbTune,
    /// The Layout view while the board shape sketch is open.
    BoardShape,
    Pcb3d,
}

impl Context {
    /// Every context, in sheet order.
    pub const ALL: [Self; 13] = [
        Self::Global,
        Self::Component,
        Self::Symbol,
        Self::Footprint,
        Self::Schematic,
        Self::SchematicWire,
        Self::Pcb,
        Self::PcbLayout,
        Self::PcbRoute,
        Self::PcbFill,
        Self::PcbTune,
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
            Self::SchematicWire => "Schematic › Wire tool",
            Self::Pcb => "PCB",
            Self::PcbLayout => "PCB › Layout",
            Self::PcbRoute => "PCB › Route tool",
            Self::PcbFill => "PCB › Fill tool",
            Self::PcbTune => "PCB › Tune tool",
            Self::BoardShape => "PCB › Board shape",
            Self::Pcb3d => "PCB › 3D",
        }
    }

    pub fn parent(self) -> Option<Self> {
        match self {
            Self::Global => None,
            Self::Component | Self::Schematic | Self::Pcb => Some(Self::Global),
            Self::Symbol | Self::Footprint => Some(Self::Component),
            Self::SchematicWire => Some(Self::Schematic),
            Self::PcbLayout | Self::Pcb3d => Some(Self::Pcb),
            Self::PcbRoute | Self::PcbFill | Self::PcbTune | Self::BoardShape => {
                Some(Self::PcbLayout)
            }
        }
    }

    /// A tool's own mode: its keys override the same keys of the contexts
    /// around it while it is active, by design.
    pub fn modal(self) -> bool {
        matches!(
            self,
            Self::SchematicWire | Self::PcbRoute | Self::PcbFill | Self::PcbTune | Self::BoardShape
        )
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

    /// Whether a key event with `held` modifiers is this chord: ⌘/Ctrl,
    /// Shift and Alt match exactly, and on macOS a held ⌃ (not ⌘) is never
    /// a plain key.
    pub fn matches(self, held: egui::Modifiers) -> bool {
        held.command == self.command
            && held.shift == self.shift
            && held.alt == self.alt
            && (held.command || !held.ctrl)
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
    // switching to 3D (⇧D instead of arming Dimension), and Esc ends the
    // route before it cancels the tool.
    (Context::PcbRoute, Chord::key(egui::Key::V), "pcb.via"),
    (Context::PcbRoute, Chord::key(egui::Key::F), "view.fit"),
    (
        Context::PcbRoute,
        Chord::key(egui::Key::F).with_shift(),
        "pcb.from-bottom",
    ),
    (Context::PcbRoute, Chord::key(egui::Key::D), "pcb.toggle-3d"),
    // ⇧D steps the via size back instead of arming the Dimension tool.
    (
        Context::PcbRoute,
        Chord::key(egui::Key::D).with_shift(),
        "pcb.dimension",
    ),
    // P switches pair routing instead of arming the Fill tool.
    (Context::PcbRoute, Chord::key(egui::Key::P), "pcb.fill"),
    // M cycles the router instead of placing a 3D part.
    (
        Context::PcbRoute,
        Chord::key(egui::Key::M),
        "pcb.place-part",
    ),
    // Wiring: F flips the elbow instead of fitting the view, and Esc ends
    // the wire before it cancels the tool.
    (Context::SchematicWire, Chord::key(egui::Key::F), "view.fit"),
    (
        Context::SchematicWire,
        Chord::key(egui::Key::Escape),
        "schematic.cancel",
    ),
    // Backspace takes back the last corner instead of deleting the
    // selection.
    (
        Context::SchematicWire,
        Chord::key(egui::Key::Backspace),
        "schematic.delete",
    ),
    // A switches arc corners for new corners instead of rounding the picked
    // corner of the selected wire.
    (
        Context::SchematicWire,
        Chord::key(egui::Key::A),
        "schematic.arc-corner",
    ),
    // Tuning: F flips the meander's side instead of fitting the view.
    (Context::PcbTune, Chord::key(egui::Key::F), "view.fit"),
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
    // Backspace takes back the last route step or outline point instead
    // of deleting the selection.
    (
        Context::PcbRoute,
        Chord::key(egui::Key::Backspace),
        "pcb.delete",
    ),
    (
        Context::PcbFill,
        Chord::key(egui::Key::Backspace),
        "pcb.delete",
    ),
];

/// The Fill tool's keys, handled by the PCB canvas (`fill_tool`).
pub const FILL_KEYS: &[KeyDecl] = &[
    KeyDecl::new("fill.arc", "A", "Switch lines and arcs"),
    KeyDecl::new("fill.close", "Enter", "Close the outline"),
    KeyDecl::new("fill.back", "Backspace", "Take back the last point"),
    KeyDecl::new("fill.drop", "Esc", "Drop the outline"),
];

/// One key of a tool mode or canvas gesture, declared by the editor that
/// handles it: a stable command id, its default key cell (as
/// [`key_cell`] reads it: `Enter`, `Shift+W`, `Delete or Backspace`) and
/// what it does. The editor asks [`crate::keys::pressed`] for the id, so
/// a rebinding changes what the key does, not only its label. A cell
/// that names no key (`Click`) is listed as written and cannot be bound.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyDecl {
    pub id: &'static str,
    pub keys: &'static str,
    pub title: &'static str,
}

impl KeyDecl {
    pub const fn new(id: &'static str, keys: &'static str, title: &'static str) -> Self {
        Self { id, keys, title }
    }
}

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
    /// Print the schematic's sheets to a PDF.
    ExportSchematicPdf,
    Undo,
    Redo,
    /// The sheet's selection or, while typing, the field's own text.
    Cut,
    Copy,
    Paste,
    /// Everything on the sheet or, while typing, the field's own text.
    SelectAll,
    ToggleBrowser,
    ToggleIssues,
    Messages,
    LibrarySources,
    /// The Settings window (its Keyboard section).
    Settings,
    /// The Connect AI assistants window (live MCP).
    ConnectAssistants,
    Workspace(Workspace),
    FitView,
    /// Switch the PCB between its Layout and 3D views.
    TogglePcb3d,
    /// Turn the PCB 3D view to a standard view.
    View3d(View3d),
    /// Switch the PCB 3D view between the Realistic look (the studio,
    /// reflections and materials) and Solidcraft's Standard modelling look.
    ToggleRealistic,
    /// Quit the app: unsaved work asks Save, Don't Save or Cancel first.
    Quit,
    /// Close the window: the app's one window, so the same request as Quit.
    CloseWindow,
}

/// The PCB 3D view's standard views (the navigation cube's targets).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View3d {
    Top,
    Bottom,
    Front,
    Back,
    Left,
    Right,
    /// The isometric home view, fitted.
    Iso,
    /// Frame the selection, so orbiting turns about it.
    Selection,
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
    /// Cycle the layer focus: all layers, dim others, active only.
    LayerFocus,
}

/// What running a command does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    App(AppCommand),
    /// The same event as the workspace's ribbon button or tab.
    Ribbon(Workspace, RibbonEvent),
    Pcb(PcbView),
    /// Make layer `n` of the stackup (0: the top) the active copper layer;
    /// while routing, change to it through a via.
    PcbLayer(usize),
    /// Open one of the PCB's docked setup dialogs, as the Board card's
    /// button does.
    PcbDialog(PcbDialog),
    /// A key of a gesture: shown, never run from the palette.
    Key,
}

/// The PCB's docked setup dialogs the Board card opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PcbDialog {
    /// Board card › Rules…
    Rules,
    /// Board card › Copper › Stackup…
    Stackup,
    /// Board card › Net classes › Edit…
    NetClasses,
    /// Board card › High speed › Edit…
    HighSpeed,
}

/// A copper layer a command names: by its name, or by its place in the
/// stackup from the top (0).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StackupLayer {
    Named(CopperLayer),
    Place(usize),
}

impl StackupLayer {
    /// Whether a board with the copper layers `stack` (top to bottom) has it.
    pub fn on(self, stack: &[CopperLayer]) -> bool {
        match self {
            Self::Named(layer) => stack.contains(&layer),
            Self::Place(index) => index < stack.len(),
        }
    }
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
    fn also(mut self, chords: &[Chord]) -> Self {
        self.also = chords.to_vec();
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

    /// The copper layer of the stackup the command is about, by name or by
    /// place: a board without it does not offer the command.
    pub fn stackup_layer(&self) -> Option<StackupLayer> {
        match self.action {
            Action::Pcb(PcbView::Copper(layer)) => Some(StackupLayer::Named(layer)),
            Action::PcbLayer(index) => Some(StackupLayer::Place(index)),
            _ => None,
        }
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
    /// Further default keys of the same command (Backspace besides Delete).
    pub also: &'static [Chord],
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
        .with_keys(Context::SchematicWire, crate::schematic_editor::WIRE_KEYS)
        .with_keys(Context::PcbRoute, crate::pcb_editor::ROUTE_KEYS)
        .with_keys(Context::PcbFill, FILL_KEYS)
        .with_keys(Context::PcbTune, crate::pcb_editor::TUNE_KEYS)
        .with_gestures(Context::Pcb3d, crate::navigation::NAVIGATION_3D)
    }

    /// Add a view's pointer gestures ("Shift + two fingers", what it does)
    /// for display only: rows that name no key, so they take none and
    /// cannot be bound (navigation stays with `navigation.rs`).
    pub fn with_gestures(mut self, context: Context, gestures: &[(&str, &str)]) -> Self {
        let prefix = format!("gesture.{context:?}").to_lowercase();
        for (index, (input, what)) in gestures.iter().enumerate() {
            let mut command =
                Command::new(format!("{prefix}.{index}"), *what, context, Action::Key).in_place();
            command.key_text = Some((*input).to_owned());
            self.commands.push(command);
        }
        self
    }

    /// Add an editor's declared keys ([`KeyDecl`]) as in-place commands of
    /// `context`, under their own ids.
    pub fn with_keys(mut self, context: Context, keys: &[KeyDecl]) -> Self {
        for decl in keys {
            let mut command = Command::new(decl.id, decl.title, context, Action::Key).in_place();
            match key_cell(decl.keys) {
                Some((chords, shown)) => {
                    let mut chords = chords.into_iter();
                    command.chord = chords.next();
                    command.also = chords.collect();
                    if !command.also.is_empty() {
                        command.key_text = Some(shown);
                    }
                }
                None => command.key_text = Some(decl.keys.to_owned()),
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

    /// Every chord the command takes: the user's one, else its default and
    /// a key-table row's others.
    pub fn chords(&self, command: &Command) -> Vec<Chord> {
        match self.custom.get(&command.id) {
            Some(custom) => custom.iter().copied().collect(),
            None => command
                .chord
                .into_iter()
                .chain(command.also.iter().copied())
                .collect(),
        }
    }

    /// Whether `chord` is one of the command's effective keys, without
    /// building the list.
    pub fn takes(&self, command: &Command, chord: Chord) -> bool {
        match self.custom.get(&command.id) {
            Some(custom) => *custom == Some(chord),
            None => command.chord == Some(chord) || command.also.contains(&chord),
        }
    }

    /// Whether the user changed the command's key.
    pub fn is_custom(&self, id: &str) -> bool {
        self.custom.contains_key(id)
    }

    /// The user's changes: command id → chord (`None`: no key).
    pub fn overrides(&self) -> &BTreeMap<String, Option<Chord>> {
        &self.custom
    }

    /// Whether the command can take a key: a row that names no key
    /// ("Click") cannot.
    pub fn bindable(command: &Command) -> bool {
        command.chord.is_some() || command.key_text.is_none()
    }

    /// The command that already takes `chord` where `id` would see it, if
    /// any (the registry's context and [`SHADOWS`] rules).
    pub fn conflict_for(&self, id: &str, chord: Chord) -> Option<&Command> {
        let command = self.get(id)?;
        self.commands
            .iter()
            .find(|c| c.id != id && self.takes(c, chord) && clash(command, c, chord))
    }

    /// Give `id` the chord `chord`, and the command holding it `id`'s
    /// current key (or none), as one change. Refused, changing nothing,
    /// when either key would conflict with a third command.
    pub fn swap(&mut self, id: &str, chord: Chord) -> Result<(), String> {
        let Some(other) = self.conflict_for(id, chord).map(|c| c.id.clone()) else {
            return self.rebind(id, Some(chord));
        };
        let mine = self.get(id).and_then(|c| self.chord(c));
        let before = self.custom.clone();
        // Free the key first so neither step sees the other as a conflict.
        let result = self
            .rebind(&other, None)
            .and_then(|()| self.rebind(id, Some(chord)))
            .and_then(|()| self.rebind(&other, mine));
        if result.is_err() {
            self.custom = before;
        }
        result
    }

    /// Back to the default keymap.
    pub fn reset_all(&mut self) {
        self.custom.clear();
    }

    /// Back to the command's default key, even where it now conflicts
    /// ([`Self::conflicts`] reports that).
    pub fn reset(&mut self, id: &str) {
        self.custom.remove(id);
    }

    /// The command with id `prefix.name`, found without building the id.
    pub fn get_parts(&self, prefix: &str, name: &str) -> Option<&Command> {
        self.commands.iter().find(|c| {
            c.id.len() == prefix.len() + 1 + name.len()
                && c.id.starts_with(prefix)
                && c.id.ends_with(name)
                && c.id.as_bytes().get(prefix.len()) == Some(&b'.')
        })
    }

    /// The shortcut as shown: the user's key, else a several-key cell as
    /// its text, else the chord, else the text.
    pub fn shortcut(&self, command: &Command) -> Option<String> {
        if let Some(custom) = self.custom.get(&command.id) {
            return custom.map(Chord::shown);
        }
        if !command.also.is_empty() {
            return command.key_text.clone().or_else(|| {
                let shown: Vec<String> =
                    self.chords(command).into_iter().map(Chord::shown).collect();
                Some(shown.join(", "))
            });
        }
        self.chord(command)
            .map(Chord::shown)
            .or_else(|| command.key_text.clone())
    }

    /// The shortcut as a menu row shows it: one key, as a menu has room
    /// for and macOS shows it. A command with two keys (Delete and
    /// Backspace) shows its first; [`Self::shortcut`] lists both.
    pub fn menu_shortcut(&self, command: &Command) -> Option<String> {
        self.chord(command)
            .map(Chord::shown)
            .or_else(|| self.shortcut(command))
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
    /// unknown, the row names no key, or the chord conflicts with another
    /// command ([`Self::conflict_for`]).
    pub fn rebind(&mut self, id: &str, chord: Option<Chord>) -> Result<(), String> {
        let Some(command) = self.get(id) else {
            return Err(format!("No command {id}"));
        };
        if !Self::bindable(command) {
            return Err(format!("{} is not a key", command.title));
        }
        if let Some(chord) = chord
            && let Some(other) = self.conflict_for(id, chord)
        {
            return Err(format!("{} already uses {}", other.title, chord.shown()));
        }
        let default = command.chord;
        // A several-key default is only the default while untouched.
        if chord == default && command.also.is_empty() {
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
                    .find(|c| c.context == *context && self.takes(c, chord))
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
        // Letters in order are a fallback: once a command matches by a
        // word or a substring, scattered-letter hits ("drc" in "Draw
        // arc") would only bury it.
        if hits.iter().any(|(score, _)| *score >= WORD_MATCH) {
            hits.retain(|(score, _)| *score >= WORD_MATCH);
        }
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

/// The least [`match_score`] of a match by a word or a substring of the
/// title, keywords or id; lower scores are letters found in order.
const WORD_MATCH: u32 = 300;

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
    title.max(other).max(id).max(whole_words(q, command))
}

/// Every word of `q` is a whole word of the title: above a word that only
/// starts with it. A workspace is where its name leads ("schematic": Go to
/// Schematic before Export schematic PDF…). 0 otherwise.
fn whole_words(q: &str, command: &Command) -> u32 {
    let title = command.title.to_lowercase();
    let words = title.split(|c: char| !c.is_alphanumeric());
    let whole = q.split_whitespace().all(|w| words.clone().any(|t| t == w));
    match (whole, &command.action) {
        (false, _) => 0,
        (true, Action::App(AppCommand::Workspace(_))) => 900,
        (true, _) => 850,
    }
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
            .also(t.also)
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
    let dialog = |id: &str, title: &str, dialog: PcbDialog| {
        Command::new(id, title, C::Pcb, Action::PcbDialog(dialog))
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
        app("app.quit", "Quit Pcbcraft", A::Quit)
            .chord(Chord::cmd(K::Q))
            .icon("close")
            .keywords("exit close window"),
        app("window.close", "Close window", A::CloseWindow)
            .chord(Chord::cmd(K::W))
            .icon("close")
            .keywords("exit quit"),
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
        app(
            "file.export-schematic-pdf",
            "Export schematic PDF…",
            A::ExportSchematicPdf,
        )
        .icon("save")
        .keywords("print pdf sheets schematic title block"),
        app("edit.undo", "Undo", A::Undo)
            .chord(Chord::cmd(K::Z))
            .icon("undo"),
        app("edit.redo", "Redo", A::Redo)
            .chord(Chord::cmd(K::Z).with_shift())
            .icon("redo"),
        // The schematic sheet's; over a focused field the field keeps these
        // keys (`command_dispatch::dispatchable`).
        app("edit.cut", "Cut", A::Cut)
            .chord(Chord::cmd(K::X))
            .icon("cut")
            .keywords("clipboard text field schematic"),
        app("edit.copy", "Copy", A::Copy)
            .chord(Chord::cmd(K::C))
            .icon("copy")
            .keywords("clipboard text field schematic duplicate"),
        app("edit.paste", "Paste", A::Paste)
            .chord(Chord::cmd(K::V))
            .icon("paste")
            .keywords("clipboard text field schematic duplicate"),
        app("edit.select-all", "Select all", A::SelectAll)
            .chord(Chord::cmd(K::A))
            .icon("select")
            .keywords("text field sheet schematic everything group"),
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
        app("app.settings", "Settings…", A::Settings)
            .chord(Chord::cmd(K::Comma))
            .icon("settings")
            .keywords("preferences options keyboard keymap shortcuts rebind keys"),
        app(
            "app.connect-assistants",
            "Connect AI assistants…",
            A::ConnectAssistants,
        )
        .icon("assistant")
        .keywords("mcp agent llm claude assistant ai live remote control"),
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
        )
        .also(&[Chord::key(K::Backspace)]),
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
            "schematic.junction",
            "Junction point",
            C::Schematic,
            Workspace::Schematic,
            "junction",
        )
        .chord(Chord::key(K::J))
        .icon("point")
        .keywords("dot join connect crossing"),
        tool(
            "schematic.no-connect",
            "No connect",
            C::Schematic,
            Workspace::Schematic,
            "no-connect",
        )
        .chord(Chord::key(K::N))
        .icon("close")
        .keywords("nc no-connect unused unconnected not connected cross pin"),
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
        // Labels, ports, hierarchy and sheet furniture
        // (docs/schematic-capture.md).
        tool(
            "schematic.label",
            "Net label",
            C::Schematic,
            Workspace::Schematic,
            "label",
        )
        .chord(Chord::key(K::L))
        .icon("point")
        .keywords("net name local"),
        tool(
            "schematic.global-label",
            "Global label",
            C::Schematic,
            Workspace::Schematic,
            "global-label",
        )
        .chord(Chord::key(K::L).with_shift())
        .icon("point")
        .keywords("off-sheet connector port net"),
        tool(
            "schematic.hierarchical-label",
            "Hierarchical label",
            C::Schematic,
            Workspace::Schematic,
            "hierarchical-label",
        )
        .chord(Chord::key(K::H))
        .icon("point")
        .keywords("sheet port net"),
        tool(
            "schematic.power",
            "Power port",
            C::Schematic,
            Workspace::Schematic,
            "power",
        )
        .chord(Chord::key(K::P).with_shift())
        .icon("point")
        .keywords("ground gnd supply vcc rail"),
        tool(
            "schematic.sheet-symbol",
            "Sheet symbol",
            C::Schematic,
            Workspace::Schematic,
            "sheet-symbol",
        )
        .chord(Chord::key(K::S).with_shift())
        .icon("folder")
        .keywords("hierarchy child sheet"),
        tool(
            "schematic.note",
            "Note",
            C::Schematic,
            Workspace::Schematic,
            "note",
        )
        .chord(Chord::key(K::T))
        .icon("list")
        .keywords("text comment"),
        tool(
            "schematic.line",
            "Line",
            C::Schematic,
            Workspace::Schematic,
            "draw-line",
        )
        .icon("line")
        .keywords("graphic drawing"),
        tool(
            "schematic.rect",
            "Rectangle",
            C::Schematic,
            Workspace::Schematic,
            "draw-rect",
        )
        .icon("rect")
        .keywords("graphic drawing box"),
        tool(
            "schematic.add-sheet",
            "Add sheet",
            C::Schematic,
            Workspace::Schematic,
            "add-sheet",
        )
        .icon("new")
        .keywords("page new"),
        tool(
            "schematic.annotate",
            "Annotate designators",
            C::Schematic,
            Workspace::Schematic,
            "annotate",
        )
        .icon("list")
        .keywords("renumber reference designator"),
        key(
            "schematic.delete",
            "Delete the selected part, wire, wire corner, label, sheet symbol or drawing",
            C::Schematic,
            Chord::key(K::Delete),
        )
        .also(&[Chord::key(K::Backspace)]),
        key(
            "schematic.split-wire",
            "Split the selected wire at the segment under the pointer",
            C::Schematic,
            Chord::key(K::Delete).with_shift(),
        )
        .also(&[Chord::key(K::Backspace).with_shift()]),
        key(
            "schematic.arc-corner",
            "Round the picked wire corner, or make it sharp",
            C::Schematic,
            Chord::key(K::A),
        ),
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
    ];
    // PCB 3D standard views: the navigation cube's faces, in Solidcraft's
    // order (its Ctrl+1 to 7; plain digits here, as Cmd+1 to 3 change
    // workspace).
    for (id, title, view, key) in [
        ("view.front", "Front view", View3d::Front, K::Num1),
        ("view.back", "Back view", View3d::Back, K::Num2),
        ("view.left", "Left view", View3d::Left, K::Num3),
        ("view.right", "Right view", View3d::Right, K::Num4),
        ("view.top", "Top view", View3d::Top, K::Num5),
        ("view.bottom", "Bottom view", View3d::Bottom, K::Num6),
        ("view.iso", "Home view (isometric)", View3d::Iso, K::Num7),
    ] {
        v.push(
            Command::new(id, title, C::Pcb3d, Action::App(A::View3d(view)))
                .chord(Chord::key(key))
                .icon("plane")
                .keywords("orbit rotate turn camera standard view cube 3d"),
        );
    }
    // Solidcraft's `view.look` (its ⇧P, which here refills the fills):
    // ⇧R for Realistic.
    v.push(
        Command::new(
            "view.look",
            "Realistic look",
            C::Pcb3d,
            Action::App(A::ToggleRealistic),
        )
        .chord(Chord::key(K::R).with_shift())
        .icon("render_scene")
        .keywords("standard photoreal materials studio lighting render shading"),
    );
    v.push(
        Command::new(
            "view.selection",
            "Fit and orbit around selection",
            C::Pcb3d,
            Action::App(A::View3d(View3d::Selection)),
        )
        .chord(Chord::key(K::F).with_shift())
        .icon("fit")
        .keywords("zoom selected frame orbit pivot focus"),
    );
    v.extend([
        // Global, so ⇧⌘U runs it from the schematic too (it opens PCB).
        tool(
            "pcb.update",
            "Update PCB from schematic",
            C::Global,
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
            .chord(Chord::key(K::R))
            .icon("trace")
            .keywords("track trace copper wire"),
        tool("pcb.via", "Via", C::PcbLayout, Workspace::Pcb, "via")
            .chord(Chord::key(K::V))
            .icon("circle")
            .keywords("hole through"),
        tool(
            "pcb.tune",
            "Tune length",
            C::PcbLayout,
            Workspace::Pcb,
            "tune",
        )
        .chord(Chord::key(K::T))
        .icon("dimension")
        .keywords("meander length match skew serpentine trombone accordion"),
        tool("pcb.fill", "Fill", C::PcbLayout, Workspace::Pcb, "fill")
            .chord(Chord::key(K::P))
            .icon("polygon")
            .keywords("pour zone plane copper area"),
        // Layout and 3D share one Dimension tool and its key.
        tool(
            "pcb.dimension",
            "Dimension",
            C::Pcb,
            Workspace::Pcb,
            "dimension",
        )
        .chord(Chord::key(K::D).with_shift())
        .icon("dimension")
        .keywords("measure distance hold face edge hole board edge parametric"),
        key(
            "pcb.dimension-cancel",
            "Drop the dimension's picks, then end the tool",
            C::Pcb3d,
            Chord::key(K::Escape),
        ),
        key(
            "pcb.dimension-delete",
            "Delete the selected dimension",
            C::Pcb3d,
            Chord::key(K::Delete),
        )
        .also(&[Chord::key(K::Backspace)]),
        tool(
            "pcb.place-part",
            "Place 3D part…",
            C::PcbLayout,
            Workspace::Pcb,
            "place-part",
        )
        .chord(Chord::key(K::M))
        .icon("component")
        .keywords(
            "mechanical part file solidcraft nut standoff spacer heatsink bracket parametric",
        ),
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
        dialog("pcb.rules", "Design rules…", PcbDialog::Rules)
            .icon("params")
            .keywords("drc clearance checks track width annular ring profile"),
        dialog("pcb.stackup", "Stackup…", PcbDialog::Stackup)
            .icon("plane")
            .keywords("copper layers dielectric prepreg core thickness impedance"),
        dialog("pcb.net-classes", "Net classes…", PcbDialog::NetClasses)
            .icon("plane")
            .keywords("netclass track width clearance via drill"),
        dialog("pcb.high-speed", "High speed…", PcbDialog::HighSpeed)
            .icon("trace")
            .keywords("differential diff pair length group matching skew impedance"),
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
            "Cancel the tool or gesture, else deselect",
            C::PcbLayout,
            Chord::key(K::Escape),
        ),
        key(
            "pcb.delete",
            "Delete the selected track, via, fill or dimension",
            C::PcbLayout,
            Chord::key(K::Delete),
        )
        .also(&[Chord::key(K::Backspace)]),
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
        view(
            "pcb.layer-focus",
            "Cycle layer focus (all, dim others, active only)",
            C::PcbLayout,
            PcbView::LayerFocus,
        )
        .chord(Chord::key(K::L).with_shift())
        .keywords("active layer single only dim others solo focus"),
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
        )
        .also(&[Chord::key(K::Backspace)]),
        key(
            "shape.cancel",
            "Cancel the tool",
            C::BoardShape,
            Chord::key(K::Escape),
        ),
        key(
            "shape.finish",
            "Finish the spline",
            C::BoardShape,
            Chord::key(K::Enter),
        ),
    ]);
    // The active copper layer by its place in the stackup: 1 to 9, then 0
    // for the tenth (`crate::pcb_editor::LAYER_KEYS`).
    for (index, decl) in crate::pcb_editor::LAYER_KEYS.iter().enumerate() {
        v.push(
            Command::new(decl.id, decl.title, C::PcbLayout, Action::PcbLayer(index))
                .maybe(Chord::parse(decl.keys))
                .icon("display")
                .keywords("active copper layer switch via stackup"),
        );
    }
    // Layer and display toggles: in the palette; no default keys. One per
    // copper layer a stackup can have (`pcb.layer.copper-0` is the top, 1
    // the bottom, then the inner layers); the palette lists those of the
    // board's stackup.
    let inner = (1..pcbcraft_core::MAX_COPPER_LAYERS - 1)
        .filter_map(|n| u8::try_from(n).ok())
        .map(CopperLayer::Inner);
    for (n, layer) in [CopperLayer::Top, CopperLayer::Bottom]
        .into_iter()
        .chain(inner)
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
