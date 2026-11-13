//! The lists' per-user settings between `UserSettings` and the lists:
//! density and reduced motion are published to every list once per frame
//! (`kit::list::tokens::publish`), as the keymap is; saved filters too
//! (`kit::list::filter::publish_saved`), and what a filter bar saved or
//! forgot is persisted at the frame's end.
//!
//! Reduced motion follows the system's Reduce motion until the user sets
//! it. On macOS the flag is read once, off the UI thread, from the
//! accessibility defaults (`defaults read com.apple.universalaccess
//! reduceMotion`), so no native API or unsafe code is needed; elsewhere
//! it is the setting alone.
use super::App;
use crate::kit::list::filter::{self, Saved, SavedChange};
use crate::kit::list::tokens::{self, Density, Prefs};
use sparksmith_io::settings::{ListDensity, ListSettings};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, Ordering};

/// 0 unknown yet, 1 off, 2 on.
static SYSTEM_REDUCE_MOTION: AtomicU8 = AtomicU8::new(0);
static ASKED: OnceLock<()> = OnceLock::new();

/// The system's Reduce motion, once known; asked on first use.
fn system_reduce_motion() -> bool {
    ASKED.get_or_init(|| {
        #[cfg(target_os = "macos")]
        std::thread::spawn(|| {
            let on = std::process::Command::new("/usr/bin/defaults")
                .args(["read", "com.apple.universalaccess", "reduceMotion"])
                .output()
                .ok()
                .is_some_and(|o| o.status.success() && o.stdout.trim_ascii() == b"1");
            SYSTEM_REDUCE_MOTION.store(if on { 2 } else { 1 }, Ordering::Relaxed);
        });
    });
    SYSTEM_REDUCE_MOTION.load(Ordering::Relaxed) == 2
}

/// What the lists read from the user's list settings.
pub(crate) fn prefs_of(lists: &ListSettings) -> Prefs {
    Prefs {
        density: match lists.density {
            ListDensity::Compact => Density::Compact,
            ListDensity::Comfortable => Density::Comfortable,
        },
        reduced_motion: lists.reduced_motion.unwrap_or_else(system_reduce_motion),
    }
}

impl App {
    /// Publish the list settings for this frame's lists.
    pub(super) fn list_prefs(&self, ctx: &egui::Context) {
        let lists = self.sources.list_settings();
        tokens::publish(ctx, prefs_of(lists));
        filter::publish_saved(
            ctx,
            lists
                .saved_filters
                .iter()
                .map(|f| Saved {
                    list: f.list.clone(),
                    query: f.query.clone(),
                })
                .collect(),
        );
    }

    /// Persist what the filter bars saved or forgot this frame.
    pub(super) fn list_changes(&mut self, ctx: &egui::Context) {
        let changes = filter::take_changes(ctx);
        if changes.is_empty() {
            return;
        }
        let mut lists = self.sources.list_settings().clone();
        for change in changes {
            match change {
                SavedChange::Add(s) => {
                    if let Err(error) = lists.save_filter(&s.list, &s.query) {
                        crate::kit::toast::post(
                            ctx,
                            crate::kit::severity::Level::Warning,
                            format!("Can't save the filter: {error}"),
                            vec![],
                        );
                    }
                }
                SavedChange::Remove(s) => lists.remove_filter(&s.list, &s.query),
            }
        }
        self.sources.set_list_settings(lists);
    }
}
