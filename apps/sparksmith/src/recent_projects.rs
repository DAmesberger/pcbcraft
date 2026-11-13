//! File › Open Recent (night finding N7): the project files last opened or
//! saved, newest first, as the menus and the command palette list them.
//! The list itself is kept per user in the settings
//! (`UserSettings::projects`); here is how a row reads and which files are
//! gone. Whether a file is still there is checked off the UI thread, so a
//! slow or unmounted volume never stalls a frame.
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};

/// Seconds between two checks of which recent files are gone.
const RECHECK_SECONDS: f64 = 2.0;

/// One Open Recent row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecentItem {
    pub path: PathBuf,
    /// "board · ~/work" or "board · unavailable", in the recovery rows'
    /// "name · detail" form.
    pub label: String,
    pub available: bool,
}

/// The rows of `paths`, in order; a path in `missing` reads as unavailable.
pub fn items(paths: &[PathBuf], missing: &BTreeSet<PathBuf>) -> Vec<RecentItem> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    paths
        .iter()
        .map(|path| {
            let available = !missing.contains(path);
            RecentItem {
                path: path.clone(),
                label: label(path, available, home.as_deref()),
                available,
            }
        })
        .collect()
}

/// A row's label: the file's name and its folder (home as `~`), or that it
/// is unavailable.
fn label(path: &Path, available: bool, home: Option<&Path>) -> String {
    let name = path.file_stem().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    if !available {
        return format!("{name} · unavailable");
    }
    let folder = path.parent().unwrap_or(path);
    let folder = match home.and_then(|h| folder.strip_prefix(h).ok()) {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
        Some(rest) => format!("~/{}", rest.display()),
        None => folder.display().to_string(),
    };
    format!("{name} · {folder}")
}

/// Which recent files are gone, as last checked.
#[derive(Default)]
pub struct Availability {
    missing: BTreeSet<PathBuf>,
    pending: Option<Receiver<BTreeSet<PathBuf>>>,
    checked_at: Option<f64>,
}

impl Availability {
    /// The recent files found gone at the last check.
    pub fn missing(&self) -> &BTreeSet<PathBuf> {
        &self.missing
    }

    /// Take a finished check, and start the next one every few seconds;
    /// `ctx` repaints when a check changes what is gone.
    pub fn poll(&mut self, ctx: &egui::Context, paths: &[PathBuf], now: f64) {
        if let Some(pending) = &self.pending {
            match pending.try_recv() {
                Ok(missing) => {
                    self.missing = missing;
                    self.pending = None;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => return,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => self.pending = None,
            }
        }
        if self
            .checked_at
            .is_some_and(|at| now - at < RECHECK_SECONDS && now >= at)
        {
            return;
        }
        self.checked_at = Some(now);
        let (sender, receiver) = channel();
        let paths = paths.to_vec();
        let known = self.missing.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let missing: BTreeSet<PathBuf> = paths
                .into_iter()
                .filter(|p| !p.try_exists().unwrap_or(false))
                .collect();
            if missing != known {
                ctx.request_repaint();
            }
            let _ = sender.send(missing);
        });
        self.pending = Some(receiver);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Rows keep the list's order (newest first), name the file and its
    /// folder with home as `~`, and a missing file reads as unavailable.
    #[test]
    fn rows_name_the_file_and_mark_missing_ones() {
        let home = Path::new("/Users/someone");
        assert_eq!(
            label(
                Path::new("/Users/someone/work/board.sparksmith"),
                true,
                Some(home)
            ),
            "board · ~/work"
        );
        assert_eq!(
            label(
                Path::new("/Users/someone/board.sparksmith"),
                true,
                Some(home)
            ),
            "board · ~"
        );
        assert_eq!(
            label(Path::new("/srv/board.sparksmith"), true, Some(home)),
            "board · /srv"
        );
        assert_eq!(
            label(Path::new("/srv/old.sparksmith"), false, Some(home)),
            "old · unavailable"
        );
        let paths = vec![
            PathBuf::from("/a/new.sparksmith"),
            PathBuf::from("/a/old.sparksmith"),
        ];
        let missing = BTreeSet::from([PathBuf::from("/a/old.sparksmith")]);
        let rows = items(&paths, &missing);
        assert_eq!(
            rows.iter()
                .map(|r| (r.path.clone(), r.available))
                .collect::<Vec<_>>(),
            [(paths[0].clone(), true), (paths[1].clone(), false)]
        );
    }

    /// A check runs off the UI thread and finds the file that is gone.
    #[test]
    fn a_missing_file_is_found_off_the_ui_thread() -> std::io::Result<()> {
        let dir = tempfile::tempdir()?;
        let there = dir.path().join("there.sparksmith");
        std::fs::write(&there, "{}")?;
        let gone = dir.path().join("gone.sparksmith");
        let ctx = egui::Context::default();
        let mut availability = Availability::default();
        let paths = vec![there, gone.clone()];
        availability.poll(&ctx, &paths, 0.0);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while availability.pending.is_some() && std::time::Instant::now() < deadline {
            std::thread::yield_now();
            availability.poll(&ctx, &paths, 0.0);
        }
        assert_eq!(availability.missing(), &BTreeSet::from([gone]));
        Ok(())
    }
}
