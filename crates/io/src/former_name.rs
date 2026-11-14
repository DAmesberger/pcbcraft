//! The one-time copy of a per-user folder from the product's former name
//! (Pcbcraft) to its current one; [`crate::settings::UserDirs::adopt_former`]
//! chooses the folders. The former folder is only read.
use crate::settings::Adopted;
use crate::{Error, Result};
use std::fs;
use std::path::Path;

/// Folders nested below the copied one.
const MAX_DEPTH: usize = 32;
/// Files, folders and links copied.
const MAX_ENTRIES: usize = 100_000;

/// Copy `from` to `to` when `to` does not exist and `from` is a folder:
/// into a temporary sibling first, renamed into place at the end. `None`
/// when there is nothing to do, or another process adopted it first.
pub(crate) fn adopt(from: &Path, to: &Path) -> Result<Option<Adopted>> {
    if fs::symlink_metadata(to).is_ok() || !fs::metadata(from).is_ok_and(|m| m.is_dir()) {
        return Ok(None);
    }
    let (Some(parent), Some(name)) = (to.parent(), to.file_name()) else {
        return Err(Error::Invalid(format!(
            "{} has no parent folder",
            to.display()
        )));
    };
    fs::create_dir_all(parent)?;
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let temporary = parent.join(format!(
        ".{}.adopting-{}-{nanos}",
        name.to_string_lossy(),
        std::process::id()
    ));
    let mut entries = 0;
    let copied = fs::create_dir(&temporary)
        .map_err(Error::from)
        .and_then(|()| copy_tree(from, &temporary, 0, &mut entries))
        .and_then(|()| fs::rename(&temporary, to).map_err(Error::from));
    match copied {
        Ok(()) => Ok(Some(Adopted {
            from: from.to_path_buf(),
            to: to.to_path_buf(),
        })),
        Err(error) => {
            let _ = fs::remove_dir_all(&temporary);
            if fs::symlink_metadata(to).is_ok() {
                // Another launch adopted the folder meanwhile.
                Ok(None)
            } else {
                Err(Error::Invalid(format!(
                    "could not copy {} to {}: {error}",
                    from.display(),
                    to.display()
                )))
            }
        }
    }
}

fn copy_tree(from: &Path, to: &Path, depth: usize, entries: &mut usize) -> Result<()> {
    if depth > MAX_DEPTH {
        return Err(Error::TooLarge(format!(
            "{} is nested more than {MAX_DEPTH} folders deep",
            from.display()
        )));
    }
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        *entries += 1;
        if *entries > MAX_ENTRIES {
            return Err(Error::TooLarge(format!(
                "more than {MAX_ENTRIES} entries to copy"
            )));
        }
        let kind = entry.file_type()?;
        let target = to.join(entry.file_name());
        if kind.is_dir() {
            fs::create_dir(&target)?;
            copy_tree(&entry.path(), &target, depth + 1, entries)?;
        } else if kind.is_symlink() {
            link(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

/// A link is copied as the same link, never followed.
#[cfg(unix)]
fn link(from: &Path, to: &Path) -> Result<()> {
    Ok(std::os::unix::fs::symlink(fs::read_link(from)?, to)?)
}

/// Links are not copied where making one needs privileges; the former
/// folder still holds it.
#[cfg(not(unix))]
fn link(_: &Path, _: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::settings::UserDirs;
    use std::ffi::OsString;
    use std::fs;
    use std::path::{Path, PathBuf};

    fn write(path: &Path, text: &str) -> crate::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        Ok(fs::write(path, text)?)
    }

    /// The variables of a user whose home is `home`: `HOME`, and on Windows
    /// the roaming and local application data folders below it.
    fn home_vars(home: &Path) -> impl Fn(&str) -> Option<OsString> + Copy + '_ {
        move |name: &str| match name {
            "HOME" => Some(home.into()),
            "APPDATA" if cfg!(windows) => Some(home.join("AppData").join("Roaming").into()),
            "LOCALAPPDATA" if cfg!(windows) => Some(home.join("AppData").join("Local").into()),
            _ => None,
        }
    }

    /// The former and current config folders of a home directory, as the
    /// platform names them.
    fn folders(home: &Path) -> (PathBuf, PathBuf) {
        let base = if cfg!(target_os = "macos") {
            home.join("Library").join("Application Support")
        } else if cfg!(windows) {
            home.join("AppData").join("Roaming")
        } else {
            home.join(".config")
        };
        (base.join("pcbcraft"), base.join("sparksmith"))
    }

    /// The former recovery folder of a home directory.
    fn former_recovery(home: &Path) -> PathBuf {
        if cfg!(target_os = "macos") {
            // A case-sensitive volume keeps the two recovery folders apart.
            home.join("Library")
                .join("Application Support")
                .join("Pcbcraft")
                .join("Recovery")
        } else if cfg!(windows) {
            home.join("AppData")
                .join("Local")
                .join("Pcbcraft")
                .join("Recovery")
        } else {
            home.join(".local")
                .join("state")
                .join("pcbcraft")
                .join("recovery")
        }
    }

    #[test]
    fn the_former_config_folder_is_copied_once_and_kept() -> crate::Result<()> {
        let home = tempfile::tempdir()?;
        let vars = home_vars(home.path());
        let (former, current) = folders(home.path());
        write(&former.join("settings.json"), "{\"format\": 1}")?;
        write(&former.join("fab-profiles").join("house.json"), "profile")?;
        let state = former_recovery(home.path());
        write(&state.join("snapshot.pcbcraft"), "zip")?;

        let adopted = UserDirs::adopt_former_from_vars(vars);
        assert!(adopted.iter().all(Result::is_ok), "{adopted:?}");
        assert!(!adopted.is_empty());
        assert_eq!(
            fs::read_to_string(current.join("settings.json"))?,
            "{\"format\": 1}"
        );
        assert_eq!(
            fs::read_to_string(current.join("fab-profiles").join("house.json"))?,
            "profile"
        );
        let recovery = UserDirs::recovery_from_vars(vars)?;
        assert_eq!(
            fs::read_to_string(recovery.join("snapshot.pcbcraft"))?,
            "zip"
        );
        assert_eq!(
            fs::read_to_string(former.join("settings.json"))?,
            "{\"format\": 1}",
            "the former folder is kept"
        );
        assert!(state.join("snapshot.pcbcraft").exists());

        // Once: a later launch leaves the current folder as the user left it.
        write(&current.join("settings.json"), "edited")?;
        assert!(UserDirs::adopt_former_from_vars(vars).is_empty());
        assert_eq!(fs::read_to_string(current.join("settings.json"))?, "edited");
        let leftovers: Vec<_> = fs::read_dir(current.parent().unwrap_or(home.path()))?
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains("adopting"))
            .collect();
        assert!(leftovers.is_empty(), "no temporary folder is left");
        Ok(())
    }

    #[test]
    fn nothing_is_copied_without_a_former_folder_or_with_an_override() -> crate::Result<()> {
        let home = tempfile::tempdir()?;
        let (former, current) = folders(home.path());
        let plain = home_vars(home.path());
        assert!(UserDirs::adopt_former_from_vars(plain).is_empty());
        assert!(
            !current.exists(),
            "a fresh install gets no folder from this"
        );

        write(&former.join("settings.json"), "x")?;
        let elsewhere = home.path().join("elsewhere");
        let overridden = |name: &str| match name {
            "SPARKSMITH_CONFIG_DIR" | "SPARKSMITH_RECOVERY_DIR" => Some(elsewhere.clone().into()),
            _ => plain(name),
        };
        assert!(UserDirs::adopt_former_from_vars(overridden).is_empty());
        assert!(!current.exists() && !elsewhere.exists());
        Ok(())
    }
}
