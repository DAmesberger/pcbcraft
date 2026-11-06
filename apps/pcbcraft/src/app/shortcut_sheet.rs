//! The Keyboard shortcuts sheet (⌘/ / Ctrl+/, the palette and the File
//! menu): every command with a key, grouped by where it applies, the
//! contexts in force now first. Reading only; keys are changed through the
//! registry (`Registry::rebind`) once a preferences page exists.
use crate::commands::{Context, Registry};
use crate::kit;
use solidcraft_ui_egui::ui_kit;

const MAX_FILTER: usize = 128;

/// One context's shortcuts: (what it does, key as shown).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Group {
    pub context: Context,
    pub here: bool,
    pub rows: Vec<(String, String)>,
}

/// The sheet's groups for `filter` (matched against the name, key and
/// context): contexts in `active` first, then the others in registry order;
/// empty groups are left out.
pub(crate) fn groups(registry: &Registry, filter: &str, active: &[Context]) -> Vec<Group> {
    let filter = filter.trim().to_lowercase();
    let mut contexts: Vec<Context> = active.iter().rev().copied().collect();
    contexts.extend(Context::ALL.into_iter().filter(|c| !active.contains(c)));
    contexts
        .into_iter()
        .filter_map(|context| {
            // One line per name, with all its keys (the palette's two).
            let mut named: Vec<(String, String)> = Vec::new();
            for command in registry.commands().iter().filter(|c| c.context == context) {
                let Some(key) = registry.shortcut(command) else {
                    continue;
                };
                match named.iter_mut().find(|(title, _)| *title == command.title) {
                    Some((_, keys)) if keys.split(", ").any(|k| k == key) => {}
                    Some((_, keys)) => {
                        keys.push_str(", ");
                        keys.push_str(&key);
                    }
                    None => named.push((command.title.clone(), key)),
                }
            }
            let rows: Vec<(String, String)> = named
                .into_iter()
                .filter(|(title, key)| {
                    filter.is_empty()
                        || title.to_lowercase().contains(&filter)
                        || key.to_lowercase().contains(&filter)
                        || context.label().to_lowercase().contains(&filter)
                })
                .collect();
            (!rows.is_empty()).then(|| Group {
                context,
                here: active.contains(&context),
                rows,
            })
        })
        .collect()
}

#[derive(Default)]
pub(crate) struct ShortcutSheet {
    open: bool,
    filter: String,
}

impl ShortcutSheet {
    pub fn close(&mut self) {
        self.open = false;
    }
    pub fn toggle(&mut self) {
        self.open = !self.open;
    }
    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn show(&mut self, ctx: &egui::Context, registry: &Registry, active: &[Context]) {
        if !self.open {
            return;
        }
        let groups = groups(registry, &self.filter, active);
        let filter = &mut self.filter;
        let closed = ui_kit::Window::new(
            egui::Id::unique("keyboard-shortcuts"),
            "list",
            "Keyboard shortcuts",
        )
        .show_body(ctx, |ui| {
            kit::note(ui, crate::reveal::hint_line());
            kit::filter_field(
                ui,
                egui::Id::unique("keyboard-shortcuts-filter"),
                filter,
                "Filter by name or key",
                false,
                MAX_FILTER,
            );
            if groups.is_empty() {
                kit::status(ui, "No shortcut matches", kit::Tone::Dim);
            }
            for group in &groups {
                let title = if group.here {
                    format!("{} · here", group.context.label())
                } else {
                    group.context.label().to_owned()
                };
                kit::section(
                    ui,
                    ("keyboard-shortcuts", group.context),
                    &title,
                    true,
                    Some(group.rows.len()),
                    |ui| {
                        for (name, key) in &group.rows {
                            ui.horizontal(|ui| {
                                ui.label(name);
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        crate::shell::key_cap(ui, key);
                                    },
                                );
                            });
                        }
                    },
                );
            }
        });
        if closed {
            self.open = false;
        }
    }
}
