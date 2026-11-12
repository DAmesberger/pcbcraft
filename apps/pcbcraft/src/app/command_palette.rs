//! The command palette (⌘P / Ctrl+P): find a command by name, see its
//! shortcut and context, and run it. Ranking comes from the registry
//! (`crate::commands::Registry::search`); whether a command can run now
//! and why not comes from the app. Running goes through the app's one
//! command path, the same as the command's ribbon button or key.
//!
//! Adapted from Solidcraft's ribbon command search (shortcut_box.rs at the
//! pinned revision: recent commands first, ↑↓ skip disabled rows, Enter
//! runs, Esc closes), drawn with the kit's search sheet.
use crate::kit;
use egui::pos2;
use pcbcraft_ui::theme::Tokens;

/// How many recent commands are kept (as Solidcraft's command search).
const RECENT: usize = 12;
/// The longest query, in bytes.
const MAX_QUERY: usize = 256;
/// Rows shown at once; the rest are counted.
const MAX_ROWS: usize = 60;
const KEYS: &str = "↑↓ move   Enter run   Esc close";

/// One row as the app ranks it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PaletteRow {
    pub id: String,
    pub title: String,
    pub icon: &'static str,
    /// The context, or why the command cannot run now.
    pub detail: String,
    pub shortcut: Option<String>,
    pub enabled: bool,
    /// One of the recent commands (an empty query shows them first).
    pub recent: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PaletteEvent {
    Run(String),
}

/// The ranked rows as the palette lists them: one row per name and
/// context (a command with two keys, such as the palette's own, shows
/// once), and in each group (Recent, Commands) the commands that run here
/// before those that do not, which show dimmed with why. The ranking is
/// otherwise kept.
pub(crate) fn arrange(rows: &mut Vec<PaletteRow>, empty_query: bool) {
    let mut seen = std::collections::BTreeSet::new();
    rows.retain(|r| seen.insert((r.title.clone(), r.detail.clone())));
    rows.sort_by_key(|r| (!(empty_query && r.recent), !r.enabled));
}

#[derive(Default)]
pub(crate) struct CommandPalette {
    open: bool,
    query: String,
    highlighted: usize,
    /// Ask the field for focus on the next frame only.
    focus: bool,
    scroll: bool,
    /// Most recent first.
    recent: Vec<String>,
}

impl CommandPalette {
    pub fn is_open(&self) -> bool {
        self.open
    }
    pub fn open(&mut self) {
        self.open = true;
        self.query.clear();
        self.highlighted = 0;
        self.focus = true;
        self.scroll = true;
    }
    pub fn close(&mut self) {
        self.open = false;
        self.query.clear();
    }
    pub fn toggle(&mut self) {
        if self.open {
            self.close();
        } else {
            self.open();
        }
    }
    pub fn recent(&self) -> &[String] {
        &self.recent
    }
    /// A command ran from the palette: it leads the recent list.
    pub fn used(&mut self, id: &str) {
        self.recent.retain(|r| r != id);
        self.recent.insert(0, id.to_owned());
        self.recent.truncate(RECENT);
    }

    /// Move the highlight one enabled row down or up, wrapping, and keep
    /// it on an enabled row when the rows change.
    fn navigate(&mut self, rows: &[PaletteRow], up: bool, down: bool) {
        let len = rows.len().min(MAX_ROWS);
        if len == 0 {
            self.highlighted = 0;
            return;
        }
        let enabled = |i: usize| rows.get(i).is_some_and(|r| r.enabled);
        self.highlighted = self.highlighted.min(len - 1);
        if up || down {
            let step = if down { 1 } else { len - 1 };
            for _ in 0..len {
                self.highlighted = (self.highlighted + step) % len;
                if enabled(self.highlighted) {
                    break;
                }
            }
            self.scroll = true;
        }
        if !enabled(self.highlighted) {
            self.highlighted = (0..len).find(|i| enabled(*i)).unwrap_or(0);
        }
    }

    /// Draw the palette while open. `rank` gives the rows for a query.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        mut rank: impl FnMut(&str) -> Vec<PaletteRow>,
    ) -> Option<PaletteEvent> {
        if !self.open {
            return None;
        }
        let keys = kit::search_keys(ctx);
        if keys.escape {
            self.close();
            return None;
        }
        let t = Tokens::get();
        let area = ctx.content_rect();
        let width = (t.form_card_width * 1.6).min(area.width() - t.space_16);
        let top = pos2(area.center().x - width / 2.0, area.top() + t.space_24 * 2.0);
        let list_height = (area.height() - t.space_24 * 8.0).max(kit::search_row_height() * 3.0);
        let focus = std::mem::take(&mut self.focus);
        let mut picked = None;
        let mut rows = Vec::new();
        let mut changed = false;
        kit::search_sheet(ctx, "command-palette", top, width, KEYS, |ui| {
            let field = kit::search_field(
                ui,
                egui::Id::unique("command-palette-query"),
                &mut self.query,
                "Find a command: route, save, layer, shortcuts",
                false,
                focus,
                MAX_QUERY,
            );
            changed = field.changed();
            if changed {
                self.highlighted = 0;
            }
            rows = rank(&self.query);
            arrange(&mut rows, self.query.trim().is_empty());
            self.navigate(&rows, keys.up, keys.down);
            if self.query.trim().is_empty() {
                kit::search_message(ui, crate::reveal::hint_line(), false);
            }
            if rows.is_empty() {
                kit::search_message(ui, "No command matches", false);
                return;
            }
            let scroll = std::mem::take(&mut self.scroll);
            egui::ScrollArea::vertical()
                .id_salt("command-palette-rows")
                .max_height(list_height)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    let empty = self.query.trim().is_empty();
                    let mut group = None;
                    for (index, row) in rows.iter().take(MAX_ROWS).enumerate() {
                        let recent = empty && row.recent;
                        if group != Some(recent) {
                            let count = rows
                                .iter()
                                .take(MAX_ROWS)
                                .filter(|r| (empty && r.recent) == recent)
                                .count();
                            kit::search_heading(
                                ui,
                                if recent { "Recent" } else { "Commands" },
                                count,
                            );
                            group = Some(recent);
                        }
                        let response = kit::command_row(
                            ui,
                            egui::Id::unique(("command-palette-row", &row.id)),
                            &kit::CommandRow {
                                icon: row.icon,
                                label: &row.title,
                                query: &self.query,
                                detail: &row.detail,
                                shortcut: row.shortcut.as_deref(),
                                enabled: row.enabled,
                                highlighted: index == self.highlighted,
                            },
                        );
                        if scroll && index == self.highlighted {
                            response.scroll_to_me(None);
                        }
                        if response.clicked() && row.enabled {
                            picked = Some(index);
                        }
                    }
                    if rows.len() > MAX_ROWS {
                        kit::search_message(
                            ui,
                            &format!(
                                "{} more: refine the search to see them",
                                rows.len() - MAX_ROWS
                            ),
                            false,
                        );
                    }
                });
        });
        if keys.enter {
            picked = picked.or(Some(self.highlighted));
        }
        let row = picked.and_then(|i| rows.get(i)).filter(|row| row.enabled)?;
        let id = row.id.clone();
        self.close();
        Some(PaletteEvent::Run(id))
    }
}

#[cfg(test)]
#[path = "command_palette_tests.rs"]
mod tests;
