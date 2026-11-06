//! The app's side of the key reveal (`crate::reveal`): hold ⌘ for key
//! badges on the controls on screen, a tool mode's keys near the pointer
//! and the keys with no control in a small panel; tap ⌥ for ribbon key
//! tips, typed to run the command through the normal command path.
//!
//! Badges and tips come from the effective keymap and the active contexts,
//! so they show the key that would fire here (F is Fit view in Layout and
//! Flip the elbow while routing) and follow a rebinding.
use super::App;
use crate::commands::{Action, Command, Context, Registry};
use crate::reveal::{self, AltTap, Hold, Target, TipEntry};
use crate::shell::{Ribbon, RibbonEvent, Workspace};
use egui::{Rect, pos2, vec2};

/// Rows the "keys without a control" panel shows before pointing to ⌘/.
const MAX_FREE_ROWS: usize = 14;

/// One ribbon command in key-tip mode.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TipItem {
    pub target: Target,
    pub label: &'static str,
    pub tip: String,
    /// A split button: its tip lists the family's tools.
    pub family: bool,
}

/// Key-tip mode: the tips of the shown level and what was typed.
#[derive(Clone, Debug, Default)]
pub(crate) struct Tips {
    pub items: Vec<TipItem>,
    /// The family whose tools are listed (the second level).
    pub family: Option<&'static str>,
    pub typed: String,
}

#[derive(Default)]
pub(crate) struct KeyReveal {
    hold: Hold,
    tap: AltTap,
    /// The hold badges show this frame.
    badges: bool,
    pub tips: Option<Tips>,
    /// This frame's registered controls (kept, so it never grows).
    targets: Vec<(Target, Rect)>,
}

impl KeyReveal {
    /// One frame of the reveal: the hold badges, and the ⌥ tap opening or
    /// closing key tips. Neither works while a text field has the
    /// keyboard (a tap there leaves tips closed) or a key is being
    /// recorded. Returns whether the key tips take this frame's keys.
    pub fn step(
        &mut self,
        sample: reveal::Sample,
        settings: pcbcraft_io::settings::RevealSettings,
        typing: bool,
        recording: bool,
    ) -> bool {
        let delay = f64::from(settings.hold_ms) / 1000.0;
        self.badges = settings.hold_badges && !recording && self.hold.update(sample, delay);
        let tapped = self.tap.update(sample);
        if !settings.key_tips || typing || recording {
            self.tips = None;
            return false;
        }
        if tapped {
            self.tips = match self.tips {
                Some(_) => None,
                None => Some(Tips::default()),
            };
            return false;
        }
        self.tips.is_some()
    }

    #[cfg(test)]
    pub fn badges(&self) -> bool {
        self.badges
    }
}

/// The command a registered control runs in `workspace`.
pub(crate) fn command_of(
    registry: &Registry,
    workspace: Workspace,
    target: Target,
) -> Option<&Command> {
    match target {
        Target::Tool(id) => registry.for_tool(workspace, id),
        Target::Tab(id) => registry.commands().iter().find(|c| {
            matches!(c.action, Action::Ribbon(w, RibbonEvent::Tab(t)) if w == workspace && t == id)
        }),
        Target::App(app) => registry.for_app(app),
    }
}

/// The key `command` answers to here, as shown: its first chord that the
/// active contexts resolve to it (or to a command doing the same, as the
/// palette's two keys); `None` when another command shadows every one.
pub(crate) fn key_in_force(
    registry: &Registry,
    command: &Command,
    active: &[Context],
) -> Option<String> {
    let chord = registry.chords(command).into_iter().find(|chord| {
        registry
            .resolve(*chord, active)
            .is_some_and(|c| c.id == command.id || c.action == command.action)
    })?;
    Some(chord.shown())
}

/// The second level holding a ribbon's tabs, when its first level has no
/// room for them.
pub(crate) const VIEWS: &str = "#views";

/// Key tips for a ribbon level, in ribbon order. The first level is the
/// tabs and the tools; when a crowded ribbon leaves a tab without a tip,
/// its tabs move behind one "Views" entry (a second level, [`VIEWS`]).
/// A split button opens a second level of its tools (`family`). A tool's
/// single-letter key in force is its tip.
pub(crate) fn ribbon_tips(
    registry: &Registry,
    workspace: Workspace,
    ribbon: &Ribbon,
    active: &[Context],
    family: Option<&'static str>,
) -> Vec<TipItem> {
    let mut items: Vec<(Target, &'static str, bool)> = Vec::new();
    let tabs = || {
        ribbon
            .tabs
            .iter()
            .filter(|_| ribbon.notice.is_none())
            .map(|tab| (Target::Tab(tab.id), tab.label, false))
    };
    let tools = || {
        ribbon
            .groups
            .iter()
            .flat_map(|g| g.tools.iter())
            .map(|tool| (Target::Tool(tool.id), tool.label, !tool.menu.is_empty()))
    };
    match family {
        Some(VIEWS) => items.extend(tabs()),
        Some(id) => {
            let shown = ribbon.groups.iter().flat_map(|g| g.tools.iter());
            if let Some(shown) = shown.into_iter().find(|t| t.id == id) {
                for variant in &shown.menu {
                    items.push((Target::Tool(variant.id), variant.label, false));
                }
            }
        }
        None => {
            items.extend(tabs());
            items.extend(tools());
            let tipped = tip(registry, workspace, active, &items);
            if tipped.iter().all(|i| !i.tip.is_empty()) || ribbon.tabs.is_empty() {
                return tipped;
            }
            items.clear();
            items.push((Target::Tab(VIEWS), "Views", true));
            items.extend(tools());
        }
    }
    tip(registry, workspace, active, &items)
}

/// [`reveal::assign_tips`] for one level's items.
fn tip(
    registry: &Registry,
    workspace: Workspace,
    active: &[Context],
    items: &[(Target, &'static str, bool)],
) -> Vec<TipItem> {
    let entries: Vec<TipEntry<'_>> = items
        .iter()
        .map(|(target, label, family)| TipEntry {
            title: label,
            late: matches!(target, Target::Tab(_)),
            key: (!family)
                .then(|| command_of(registry, workspace, *target))
                .flatten()
                .and_then(|c| {
                    registry.chords(c).into_iter().find(|chord| {
                        registry
                            .resolve(*chord, active)
                            .is_some_and(|r| r.id == c.id)
                    })
                })
                .and_then(reveal::single_letter),
        })
        .collect();
    let tips = reveal::assign_tips(&entries);
    items
        .iter()
        .zip(tips)
        .map(|((target, label, family), tip)| TipItem {
            target: *target,
            label,
            tip,
            family: *family,
        })
        .collect()
}

/// What typing in key-tip mode does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TipKey {
    /// Wait for more letters.
    More,
    /// Run or open this item.
    Pick(TipItem),
    Exit,
}

/// Add `letter` to what was typed: a whole tip picks it, a tip's start
/// waits for more, anything else starts over.
pub(crate) fn type_letter(tips: &mut Tips, letter: char) -> TipKey {
    tips.typed.push(letter.to_ascii_uppercase());
    if let Some(item) = tips.items.iter().find(|i| i.tip == tips.typed) {
        return TipKey::Pick(item.clone());
    }
    if !tips.items.iter().any(|i| i.tip.starts_with(&tips.typed)) {
        tips.typed.clear();
    }
    TipKey::More
}

impl App {
    /// Before anything reads input: watch ⌘ and ⌥, and while key tips
    /// show, take the frame's keys for them.
    pub(super) fn reveal_input(&mut self, ctx: &egui::Context) {
        reveal::begin_frame(ctx);
        let settings = self.sources.keyboard().reveal;
        let sample = reveal::sample(ctx);
        let typing = ctx.egui_wants_keyboard_input();
        let recording = self.settings_window.is_recording();
        if self.reveal.step(sample, settings, typing, recording) {
            self.tip_keys(ctx);
        }
        let delay = f64::from(settings.hold_ms) / 1000.0;
        if let Some(wait) = self.reveal.hold.wake_in(sample.time, delay) {
            // A timed wake can land a little early; close to the delay, ask
            // for the next frame straight away.
            if wait < 0.05 {
                ctx.request_repaint();
            } else {
                ctx.request_repaint_after(std::time::Duration::from_secs_f64(wait));
            }
        }
    }

    /// Key-tip mode's keys: letters type, Backspace steps back, Esc, a
    /// click or a ⌘ chord leaves (the chord still runs).
    fn tip_keys(&mut self, ctx: &egui::Context) {
        let clicked = ctx.input(|i| {
            i.events
                .iter()
                .any(|e| matches!(e, egui::Event::PointerButton { pressed: true, .. }))
        });
        if clicked {
            self.reveal.tips = None;
            return;
        }
        let presses: Vec<(egui::Key, egui::Modifiers)> = ctx.input(|i| {
            i.events
                .iter()
                .filter_map(|e| match e {
                    egui::Event::Key {
                        key,
                        pressed: true,
                        modifiers,
                        ..
                    } => Some((*key, *modifiers)),
                    _ => None,
                })
                .collect()
        });
        if presses.iter().any(|(_, m)| m.command) {
            self.reveal.tips = None;
            return;
        }
        // Nothing below sees a key while tips show.
        ctx.input_mut(|i| {
            i.events
                .retain(|e| !matches!(e, egui::Event::Key { .. } | egui::Event::Text(_)));
        });
        for (key, _) in presses {
            let Some(tips) = self.reveal.tips.as_mut() else {
                return;
            };
            let outcome = match key {
                egui::Key::Escape => TipKey::Exit,
                egui::Key::Backspace => {
                    if tips.typed.pop().is_none() && tips.family.take().is_none() {
                        TipKey::Exit
                    } else {
                        TipKey::More
                    }
                }
                other => match reveal::single_letter(crate::commands::Chord::key(other)) {
                    Some(letter) => type_letter(tips, letter),
                    None => TipKey::More,
                },
            };
            match outcome {
                TipKey::More => {}
                TipKey::Exit => self.reveal.tips = None,
                TipKey::Pick(item) => self.pick_tip(ctx, item),
            }
        }
    }

    /// Run a typed tip's command through the command path (a disabled one
    /// says why), or open a split button's tools.
    pub(super) fn pick_tip(&mut self, ctx: &egui::Context, item: TipItem) {
        if item.family
            && let (Target::Tool(id) | Target::Tab(id), Some(tips)) =
                (item.target, self.reveal.tips.as_mut())
        {
            tips.family = Some(id);
            tips.typed.clear();
            return;
        }
        self.reveal.tips = None;
        let command = command_of(&self.registry, self.workspace, item.target).map(|c| c.id.clone());
        if let Some(id) = command {
            self.run_command(ctx, &id);
            return;
        }
        match item.target {
            Target::Tab(tab) => self.ribbon_event(ctx, RibbonEvent::Tab(tab)),
            Target::Tool(tool) => {
                let ribbon = self.current_ribbon();
                let state = ribbon
                    .groups
                    .iter()
                    .flat_map(|g| {
                        g.tools
                            .iter()
                            .chain(g.tools.iter().flat_map(|t| t.menu.iter()))
                    })
                    .find(|t| t.id == tool)
                    .map(|t| (t.enabled, t.tip, t.label));
                match state {
                    Some((true, _, _)) => self.ribbon_event(ctx, RibbonEvent::Tool(tool)),
                    Some((false, "", label)) => self.say(
                        crate::kit::severity::Level::Info,
                        format!("{label} is unavailable now"),
                    ),
                    Some((false, tip, _)) => self.say(crate::kit::severity::Level::Info, tip),
                    None => {}
                }
            }
            Target::App(app) => {
                if let Some(id) = self.registry.for_app(app).map(|c| c.id.clone()) {
                    self.run_command(ctx, &id);
                }
            }
        }
    }

    /// The shown workspace's ribbon, as the shell draws it.
    pub(super) fn current_ribbon(&mut self) -> Ribbon {
        let mut ribbon = match self.workspace {
            Workspace::Part => self.part.ribbon(&self.engine),
            Workspace::Schematic => self.schematic.ribbon(&self.engine),
            Workspace::Pcb => self.pcb.ribbon(&self.engine),
        };
        self.annotate_ribbon(&mut ribbon);
        ribbon
    }

    /// The tips of the ribbon being drawn, while key-tip mode is on.
    pub(super) fn refresh_tips(&mut self, ribbon: &Ribbon) {
        let active = self.active_contexts();
        if let Some(tips) = self.reveal.tips.as_mut() {
            tips.items = ribbon_tips(&self.registry, self.workspace, ribbon, &active, tips.family);
        }
    }

    /// Paint the badges, panels and tips over everything, in one layer.
    pub(super) fn reveal_overlay(&mut self, ctx: &egui::Context) {
        if !self.reveal.badges && self.reveal.tips.is_none() {
            return;
        }
        let mut targets = std::mem::take(&mut self.reveal.targets);
        reveal::copy_targets(ctx, &mut targets);
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Tooltip,
            egui::Id::new("key-reveal"),
        ));
        let screen = ctx.content_rect();
        if self.reveal.badges {
            self.paint_badges(ctx, &painter, screen, &targets);
        }
        if let Some(tips) = &self.reveal.tips {
            paint_tips(&painter, screen, tips, &targets);
        }
        self.reveal.targets = targets;
    }

    fn paint_badges(
        &self,
        ctx: &egui::Context,
        painter: &egui::Painter,
        screen: Rect,
        targets: &[(Target, Rect)],
    ) {
        let active = self.active_contexts();
        let mut covered: Vec<&str> = Vec::with_capacity(targets.len());
        for (target, rect) in targets {
            let Some(command) = command_of(&self.registry, self.workspace, *target) else {
                continue;
            };
            covered.push(&command.id);
            if let Some(key) = key_in_force(&self.registry, command, &active) {
                // Below a small button (the app bar's), else at the corner.
                let at = if rect.height() < 40.0 {
                    pos2(rect.center().x, rect.bottom() + 8.0)
                } else {
                    pos2(rect.right() - 4.0, rect.top() + 6.0)
                };
                reveal::paint_cap(painter, at, &key, false);
            }
        }
        let inner = active.first().copied().unwrap_or(Context::Global);
        let table = match inner {
            Context::SchematicWire => Some(crate::schematic_editor::WIRE_KEYS),
            Context::PcbRoute => Some(crate::pcb_editor::ROUTE_KEYS),
            Context::PcbFill => Some(crate::commands::FILL_KEYS),
            Context::PcbTune => Some(crate::pcb_editor::TUNE_KEYS),
            _ => None,
        };
        if let Some(table) = table {
            let rows = crate::keys::rows(ctx, table);
            let at = ctx
                .pointer_hover_pos()
                .map_or(pos2(screen.center().x, screen.top() + 160.0), |p| {
                    p + vec2(24.0, 24.0)
                });
            reveal::paint_panel(painter, screen, at, inner.label(), &rows, None);
        }
        let rows = free_rows(&self.registry, &active, table.is_some(), &covered);
        let shown: Vec<(String, String)> = rows.iter().take(MAX_FREE_ROWS).cloned().collect();
        let all = self
            .registry
            .for_app(crate::commands::AppCommand::Shortcuts)
            .and_then(|c| self.registry.shortcut(c))
            .unwrap_or_default();
        let more = rows.len().saturating_sub(MAX_FREE_ROWS);
        let navigation = if self.model_key().is_some() {
            crate::navigation::hint_3d().to_owned()
        } else {
            "Two fingers pan · pinch zooms".to_owned()
        };
        let footer = if more > 0 {
            format!("{navigation}\n{more} more · every key: {all}")
        } else {
            format!("{navigation}\nEvery key: {all}")
        };
        reveal::paint_panel(
            painter,
            screen,
            pos2(screen.left() + 16.0, screen.bottom()),
            "Keys without a button",
            &shown,
            Some(&footer),
        );
    }
}

/// The keys in force here that no control on screen shows: the innermost
/// contexts first, one row per name and key. A tool mode's own keys are
/// left out when its panel lists them.
pub(crate) fn free_rows(
    registry: &Registry,
    active: &[Context],
    skip_mode: bool,
    covered: &[&str],
) -> Vec<(String, String)> {
    let mut rows: Vec<(String, String)> = Vec::new();
    for context in active {
        if skip_mode && context.modal() && !context.exclusive() {
            continue;
        }
        for command in registry.commands().iter().filter(|c| c.context == *context) {
            // A command with a button, or another way to run the same
            // thing (the palette's second key), is shown on that button.
            let shown = covered.iter().any(|id| {
                *id == command.id
                    || registry
                        .get(id)
                        .is_some_and(|c| c.action == command.action && c.action != Action::Key)
            });
            if shown {
                continue;
            }
            let Some(key) = key_in_force(registry, command, active) else {
                continue;
            };
            // The ten layer keys on their digits read as one row.
            let row = if crate::keys::default_layer_key(registry, command) {
                (
                    crate::keys::LAYER_ROW.0.to_owned(),
                    crate::keys::LAYER_ROW.1.to_owned(),
                )
            } else {
                (key, command.title.clone())
            };
            if !rows.contains(&row) {
                rows.push(row);
            }
        }
    }
    rows
}

/// Key tips at the bottom centre of their controls; tips without a control
/// on screen (tools in the » menu, a family's tools) in a panel.
fn paint_tips(painter: &egui::Painter, screen: Rect, tips: &Tips, targets: &[(Target, Rect)]) {
    let mut unplaced: Vec<(String, String)> = Vec::new();
    let mut anchor = None;
    for item in &tips.items {
        if item.tip.is_empty() {
            continue;
        }
        let dim = !item.tip.starts_with(&tips.typed);
        let rect = targets
            .iter()
            .find(|(t, _)| match item.target {
                // The Views entry sits on the first tab.
                Target::Tab(VIEWS) => matches!(t, Target::Tab(_)),
                other => *t == other,
            })
            .map(|(_, r)| *r)
            .filter(|_| matches!(tips.family, None | Some(VIEWS)));
        match rect {
            Some(rect) if !dim => {
                // A tab's tip at its corner, a tool's under its label.
                let at = match item.target {
                    Target::Tab(_) => pos2(rect.right() - 2.0, rect.top() + 4.0),
                    _ => pos2(rect.center().x, rect.bottom() - 4.0),
                };
                reveal::paint_cap(painter, at, &item.tip, true);
            }
            Some(_) => {}
            None if !dim => unplaced.push((item.tip.clone(), item.label.to_owned())),
            None => {}
        }
    }
    if let Some(family) = tips.family {
        anchor = targets
            .iter()
            .find(|(t, _)| *t == Target::Tool(family))
            .map(|(_, r)| r.left_bottom() + vec2(0.0, 4.0));
    }
    if !unplaced.is_empty() {
        let at = anchor.unwrap_or(pos2(screen.right() - 260.0, screen.top() + 140.0));
        let caption = if tips.family.is_some() {
            "Type a key tip · Backspace back · Esc leaves"
        } else {
            "More tools"
        };
        reveal::paint_panel(painter, screen, at, caption, &unplaced, None);
    }
    if !tips.typed.is_empty() {
        reveal::paint_cap(
            painter,
            pos2(screen.center().x, screen.bottom() - 40.0),
            &format!("{}…", tips.typed),
            true,
        );
    }
}

#[cfg(test)]
#[path = "key_reveal_tests.rs"]
mod tests;
