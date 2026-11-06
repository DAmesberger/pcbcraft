//! Right-click menus, one look everywhere, as Solidcraft's
//! (`crates/ui-egui/src/context_menu.rs`): a list of the commands that act
//! on the thing under the pointer, each with its key from the shortcut
//! registry, a disabled row dimmed rather than hidden, thin separators
//! between groups. The rows are Solidcraft's own (`context_menu::list_row`
//! in `menu_frame`, in a popup `Container`), placed as its `place` places
//! them.
//!
//! An owner (a canvas, the Browser, Issues) keeps a [`State`] with what the
//! menu is about, builds its rows each frame from that target, and runs the
//! row chosen: a registry command ([`Run::Command`], run by the app exactly
//! as from the palette) or one of its own typed commands ([`Run::Act`],
//! the same operation its key or card button sends). Nothing here changes
//! the document.
//!
//! An open menu owns the keyboard: a canvas runs before its menu in a frame
//! and would take Esc for itself, and ⌘Z would act underneath, so the app
//! calls [`hold_keys`] before any of them.
use egui::{Pos2, Vec2, vec2};
use solidcraft_ui_egui::context_menu::{Item, LIST_W, list_row, menu_frame, place};
use solidcraft_ui_egui::ui_kit::{Container, Surface};

/// What a chosen row does.
#[derive(Clone, Debug, PartialEq)]
pub enum Run<A> {
    /// Run this registry command, as the palette would.
    Command(&'static str),
    /// The owner's own action: the typed command its key or button sends.
    Act(A),
}

/// One menu row.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry<A> {
    pub label: String,
    pub icon: &'static str,
    /// The key shown at the right, from the effective keymap.
    pub key: Option<String>,
    pub enabled: bool,
    pub run: Run<A>,
}

impl<A> Entry<A> {
    /// A registry command's row: its icon and key from the registry.
    pub fn command(ctx: &egui::Context, id: &'static str, label: &str, enabled: bool) -> Self {
        let registry = crate::keys::registry(ctx);
        let command = registry.get(id);
        Self {
            label: label.to_owned(),
            icon: command.map_or("", |c| c.icon),
            key: command.and_then(|c| registry.menu_shortcut(c)),
            enabled: enabled && command.is_some(),
            run: Run::Command(id),
        }
    }

    /// The owner's action `act`, without a key.
    pub fn act(label: impl Into<String>, icon: &'static str, act: A, enabled: bool) -> Self {
        Self {
            label: label.into(),
            icon,
            key: None,
            enabled,
            run: Run::Act(act),
        }
    }

    /// Show the key of the registry command `id`, the key that runs the
    /// same action.
    pub fn key_of(mut self, ctx: &egui::Context, id: &str) -> Self {
        self.key = crate::keys::menu_key(ctx, id);
        self
    }
}

/// A menu's rows; `None` is a separator.
pub type Rows<A> = Vec<Option<Entry<A>>>;

/// An open menu: what it is about and where it opened.
#[derive(Clone, Debug)]
struct Open<T> {
    target: T,
    at: Pos2,
    /// The pass that last drew it. Until it was on screen a press is the
    /// opening click's own; a menu its owner stopped drawing (another
    /// workspace, a folded card) is closed, not shown again later.
    drawn: Option<u64>,
}

/// One owner's menu, closed or open.
#[derive(Clone, Debug)]
pub struct State<T>(Option<Open<T>>);

impl<T> Default for State<T> {
    fn default() -> Self {
        Self(None)
    }
}

/// Height of `rows` as Solidcraft's list draws them (its `list_height`):
/// margins and border, 30 points a row, 11 a separator.
fn height<A>(rows: &[Option<Entry<A>>]) -> f32 {
    14.0 + rows
        .iter()
        .map(|r| if r.is_some() { 30.0 } else { 11.0 })
        .sum::<f32>()
}

/// Temp memory: the pass that last drew a menu.
fn drawn_id() -> egui::Id {
    egui::Id::new("pcbcraft-context-menu-drawn")
}

/// Temp memory: the pass that asked every menu to close.
fn close_id() -> egui::Id {
    egui::Id::new("pcbcraft-context-menu-close")
}

/// Whether a menu was on screen in the last pass.
pub fn is_open(ctx: &egui::Context) -> bool {
    let drawn = ctx.data(|d| d.get_temp::<u64>(drawn_id()));
    drawn.is_some_and(|pass| pass.saturating_add(1) >= ctx.cumulative_pass_nr())
}

/// Ask the open menu to close when it next draws.
pub fn close_all(ctx: &egui::Context) {
    let pass = ctx.cumulative_pass_nr();
    ctx.data_mut(|d| d.insert_temp(close_id(), pass));
}

/// While a menu is open, take this frame's key presses and clipboard events
/// so nothing underneath acts on them; Esc closes the menu. Call before
/// anything else reads keys.
pub fn hold_keys(ctx: &egui::Context) {
    if !is_open(ctx) {
        return;
    }
    let escape = ctx.input_mut(|i| {
        let escape = i.key_pressed(egui::Key::Escape);
        i.events.retain(|event| {
            !matches!(
                event,
                egui::Event::Key { pressed: true, .. }
                    | egui::Event::Copy
                    | egui::Event::Cut
                    | egui::Event::Paste(_)
            )
        });
        escape
    });
    if escape {
        close_all(ctx);
    }
}

impl<T> State<T> {
    /// Open the menu about `target` at `at` (a menu already open closes).
    pub fn open(&mut self, at: Pos2, target: T) {
        self.0 = Some(Open {
            target,
            at,
            drawn: None,
        });
    }

    pub fn close(&mut self) {
        self.0 = None;
    }

    pub fn target(&self) -> Option<&T> {
        self.0.as_ref().map(|o| &o.target)
    }

    #[cfg(test)]
    pub fn is_open(&self) -> bool {
        self.0.is_some()
    }

    /// Draw the open menu with `rows` (built from [`Self::target`] this
    /// frame). Returns the enabled row clicked. The menu closes when a row
    /// is chosen, on Esc ([`hold_keys`]), on a press outside it and when it has no rows.
    pub fn show<A: Clone>(
        &mut self,
        ctx: &egui::Context,
        rows: &[Option<Entry<A>>],
    ) -> Option<Run<A>> {
        let open = self.0.as_mut()?;
        let pass = ctx.cumulative_pass_nr();
        let stale = open.drawn.is_some_and(|last| last.saturating_add(1) < pass);
        let asked = ctx
            .data_mut(|d| d.remove_temp::<u64>(close_id()))
            .is_some_and(|at| at.saturating_add(1) >= pass);
        if rows.is_empty() || stale || asked {
            self.0 = None;
            ctx.data_mut(|d| d.remove::<u64>(drawn_id()));
            return None;
        }
        let screen = ctx.content_rect();
        let at = place(open.at, Vec2::ZERO, vec2(LIST_W, height(rows)), screen);
        let mut picked = None;
        let response = Container::new(ctx, Surface::Popup)
            .area(egui::Id::new("pcbcraft-context-menu"))
            .fixed_pos(at)
            .constrain(true)
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                menu_frame(ui, |ui| {
                    for (k, row) in rows.iter().enumerate() {
                        let item = match row {
                            None => Item::sep(),
                            Some(entry) => Item::action(
                                &format!("pcbcraft.menu.{k}"),
                                &entry.label,
                                entry.icon,
                            )
                            .key(entry.key.as_deref().unwrap_or_default())
                            .on_if(entry.enabled),
                        };
                        if list_row(ui, &item).is_some()
                            && let Some(entry) = row
                        {
                            picked = Some(entry.run.clone());
                        }
                    }
                });
            });
        let shown = open.drawn.replace(pass).is_some();
        let pointer = ctx.input(|i| i.pointer.latest_pos());
        let inside = pointer.is_some_and(|p| response.response.rect.contains(p));
        let press =
            ctx.input(|i| shown && (i.pointer.primary_pressed() || i.pointer.secondary_pressed()));
        if picked.is_some() || (press && !inside) {
            self.0 = None;
            ctx.data_mut(|d| d.remove::<u64>(drawn_id()));
        } else {
            ctx.data_mut(|d| d.insert_temp(drawn_id(), pass));
        }
        picked
    }
}

#[cfg(test)]
#[path = "context_menu_tests.rs"]
mod tests;
