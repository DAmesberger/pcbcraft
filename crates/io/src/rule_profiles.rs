//! The user's design-rule profile library: one versioned JSON
//! [`ProfileFile`] per profile in `UserDirs::config/rule-profiles/`, named
//! `<id>.json`. Writes are atomic; reads are bounded; an unsupported format
//! or a damaged file is reported, never rewritten or silently skipped.
//!
//! A project never reads this folder to evaluate its rules: it embeds a
//! snapshot of its profile (`ProjectRules::profile`). The library only
//! supplies profiles to switch to, and keeps the ones a user saved,
//! imported or entered from a fabricator's published capabilities.
use crate::library_folder::{pretty, read_bounded};
use crate::persistence::{atomic_create, atomic_write};
use crate::settings::UserDirs;
use crate::{Error, Result};
use pcbcraft_core::rules::{PROFILE_FILE_FORMAT, ProfileFile, ProfileOrigin, RuleProfile, builtin};
use std::path::{Path, PathBuf};

/// The library folder's name under the user config directory.
pub const PROFILES_DIR: &str = "rule-profiles";
/// The extension of a profile file.
pub const EXTENSION: &str = "json";
/// Bytes of one profile file.
pub const MAX_PROFILE_FILE_BYTES: usize = 256 * 1024;
/// Profiles the library holds.
pub const MAX_LIBRARY_PROFILES: usize = 256;
/// Bytes of a library profile's id (it names its file).
pub const MAX_ID: usize = 64;

/// A file in the library folder that could not be read as a profile.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    pub file: PathBuf,
    pub message: String,
}

/// What [`ProfileLibrary::list`] found: the readable profiles, sorted by
/// name, and every file it could not use.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Listing {
    pub profiles: Vec<RuleProfile>,
    pub problems: Vec<Problem>,
}

/// The user profile library folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProfileLibrary {
    dir: PathBuf,
}

/// Whether `id` names a built-in profile (reserved: a user profile never
/// takes one).
pub fn is_builtin_id(id: &str) -> bool {
    builtin::all().iter().any(|p| p.id == id)
}

/// `text` as a library id: lower-case ASCII letters, digits and single
/// dashes, at most [`MAX_ID`] bytes; "profile" when nothing is left.
pub fn slug(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
        if out.len() >= MAX_ID {
            break;
        }
    }
    let out = out.trim_end_matches('-').to_owned();
    if out.is_empty() {
        "profile".into()
    } else {
        out
    }
}

/// Whether `id` is a library id ([`slug`]'s alphabet, at most [`MAX_ID`]).
pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_ID
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !id.starts_with('-')
        && !id.ends_with('-')
}

/// A `YYYY-MM-DD` date, or empty.
fn valid_date(date: &str) -> bool {
    if date.is_empty() {
        return true;
    }
    let parts: Vec<&str> = date.split('-').collect();
    let number = |s: &str, len: usize| s.len() == len && s.bytes().all(|b| b.is_ascii_digit());
    let [y, m, d] = parts.as_slice() else {
        return false;
    };
    if !(number(y, 4) && number(m, 2) && number(d, 2)) {
        return false;
    }
    let (m, d) = (m.parse::<u32>().unwrap_or(0), d.parse::<u32>().unwrap_or(0));
    (1..=12).contains(&m) && (1..=31).contains(&d)
}

/// A profile the library may store: [`RuleProfile::validate`], a library
/// id that is not a built-in's, a user or imported origin and an ISO date.
pub fn check(profile: &RuleProfile) -> Result<()> {
    profile.validate()?;
    if !valid_id(&profile.id) {
        return Err(Error::Invalid(format!(
            "the profile id {:?} must be lower-case letters, digits and dashes, at most {MAX_ID} bytes",
            profile.id
        )));
    }
    if is_builtin_id(&profile.id) {
        return Err(Error::Invalid(format!(
            "{} is a built-in profile id; a user profile needs its own",
            profile.id
        )));
    }
    if profile.provenance.origin == ProfileOrigin::BuiltIn {
        return Err(Error::Invalid(
            "a library profile is a user or imported profile, not a built-in one".into(),
        ));
    }
    if !valid_date(&profile.provenance.date) {
        return Err(Error::Invalid(format!(
            "the profile date {:?} must be YYYY-MM-DD or empty",
            profile.provenance.date
        )));
    }
    Ok(())
}

/// The versioned JSON of one profile file.
pub fn encode(profile: &RuleProfile) -> Result<Vec<u8>> {
    profile.validate()?;
    let bytes = pretty(&ProfileFile {
        format: PROFILE_FILE_FORMAT,
        profile: profile.clone(),
    })?;
    if bytes.len() > MAX_PROFILE_FILE_BYTES {
        return Err(Error::TooLarge(format!(
            "a profile file exceeds {} KiB",
            MAX_PROFILE_FILE_BYTES >> 10
        )));
    }
    Ok(bytes)
}

/// Decode one profile file: its format must be [`PROFILE_FILE_FORMAT`]
/// and its profile valid.
pub fn decode(bytes: &[u8]) -> Result<RuleProfile> {
    if bytes.len() > MAX_PROFILE_FILE_BYTES {
        return Err(Error::TooLarge(format!(
            "a profile file exceeds {} KiB",
            MAX_PROFILE_FILE_BYTES >> 10
        )));
    }
    // Read the format first, so a newer file is refused as unsupported
    // rather than as malformed.
    let value: serde_json::Value = serde_json::from_slice(bytes)?;
    match value.get("format").and_then(serde_json::Value::as_u64) {
        Some(format) if format == u64::from(PROFILE_FILE_FORMAT) => {}
        Some(format) => {
            return Err(Error::Document(pcbcraft_core::Error::Unsupported(format!(
                "unsupported rule profile format {format}"
            ))));
        }
        None => {
            return Err(Error::Invalid(
                "not a rule profile file: it has no format".into(),
            ));
        }
    }
    let file: ProfileFile = serde_json::from_value(value)?;
    file.profile.validate()?;
    Ok(file.profile)
}

/// Read and decode a profile file anywhere (an import).
pub fn read_profile_file(path: &Path) -> Result<RuleProfile> {
    decode(&read_bounded(path, MAX_PROFILE_FILE_BYTES)?)
}

/// Write `profile` to a new file at `path` (an export). An existing file
/// is never replaced.
pub fn export(profile: &RuleProfile, path: &Path) -> Result<()> {
    if path.extension().is_none_or(|e| e != EXTENSION) {
        return Err(Error::Invalid(
            "use a .json filename for a rule profile".into(),
        ));
    }
    if path.as_os_str().len() > crate::settings::MAX_PATH {
        return Err(Error::Invalid("the profile path is too long".into()));
    }
    atomic_create(path, &encode(profile)?)
}

/// [`export`], replacing an existing file at `path` (the desktop's save
/// dialog has already asked).
pub fn export_replacing(profile: &RuleProfile, path: &Path) -> Result<()> {
    if path.extension().is_none_or(|e| e != EXTENSION) {
        return Err(Error::Invalid(
            "use a .json filename for a rule profile".into(),
        ));
    }
    atomic_write(path, &encode(profile)?)
}

fn not_found(id: &str) -> Error {
    Error::Document(pcbcraft_core::Error::NotFound(format!(
        "no user rule profile {id}"
    )))
}

impl ProfileLibrary {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// `rule-profiles/` in the user config directory.
    pub fn from_dirs(dirs: &UserDirs) -> Self {
        Self::new(dirs.config.join(PROFILES_DIR))
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn path(&self, id: &str) -> Result<PathBuf> {
        if !valid_id(id) {
            return Err(not_found(id));
        }
        Ok(self.dir.join(format!("{id}.{EXTENSION}")))
    }

    /// The profile files, sorted by file name; at most
    /// [`MAX_LIBRARY_PROFILES`] are read and the rest are reported.
    fn files(&self) -> Result<(Vec<PathBuf>, Vec<Problem>)> {
        let entries = match std::fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok((Vec::new(), Vec::new()));
            }
            Err(error) => return Err(error.into()),
        };
        let mut files = Vec::new();
        for entry in entries {
            let path = entry?.path();
            if path.extension().is_some_and(|e| e == EXTENSION) && path.is_file() {
                files.push(path);
            }
            // A runaway folder is still bounded: stop collecting well past
            // the limit; the excess is reported below.
            if files.len() > MAX_LIBRARY_PROFILES * 4 {
                break;
            }
        }
        files.sort();
        let mut problems = Vec::new();
        if files.len() > MAX_LIBRARY_PROFILES {
            for file in files.split_off(MAX_LIBRARY_PROFILES) {
                problems.push(Problem {
                    file,
                    message: format!(
                        "not read: the library holds at most {MAX_LIBRARY_PROFILES} profiles"
                    ),
                });
            }
        }
        Ok((files, problems))
    }

    /// Every profile in the folder, sorted by name; a missing folder is an
    /// empty library. A damaged, unsupported or misnamed file is a
    /// [`Problem`], never dropped silently.
    pub fn list(&self) -> Result<Listing> {
        let (files, mut problems) = self.files()?;
        let mut profiles = Vec::new();
        for file in files {
            match read_profile_file(&file) {
                Ok(profile)
                    if file.file_stem().and_then(|s| s.to_str()) == Some(profile.id.as_str()) =>
                {
                    profiles.push(profile);
                }
                Ok(profile) => problems.push(Problem {
                    message: format!("the file name does not match its profile id {}", profile.id),
                    file,
                }),
                Err(error) => problems.push(Problem {
                    file,
                    message: error.to_string(),
                }),
            }
        }
        profiles.sort_by(|a, b| {
            a.name
                .to_lowercase()
                .cmp(&b.name.to_lowercase())
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok(Listing { profiles, problems })
    }

    /// One profile by id.
    pub fn get(&self, id: &str) -> Result<RuleProfile> {
        let path = self.path(id)?;
        if !path.is_file() {
            return Err(not_found(id));
        }
        let profile = read_profile_file(&path)?;
        if profile.id != id {
            return Err(Error::Invalid(format!(
                "{} holds the profile {}, not {id}",
                path.display(),
                profile.id
            )));
        }
        Ok(profile)
    }

    /// Refuse a name another library or built-in profile already shows
    /// (ignoring case), except the profile `id` itself.
    fn check_name(&self, listing: &Listing, name: &str, id: &str) -> Result<()> {
        let taken = |p: &RuleProfile| p.id != id && p.name.trim().eq_ignore_ascii_case(name.trim());
        if builtin::all().iter().any(taken) || listing.profiles.iter().any(taken) {
            return Err(Error::Document(pcbcraft_core::Error::Conflict(format!(
                "a rule profile named {} exists; choose another name",
                name.trim()
            ))));
        }
        Ok(())
    }

    /// Add a new profile; its id must be free. Returns what was stored.
    pub fn create(&self, profile: &RuleProfile) -> Result<RuleProfile> {
        check(profile)?;
        let listing = self.list()?;
        if listing.profiles.len() + listing.problems.len() >= MAX_LIBRARY_PROFILES {
            return Err(Error::TooLarge(format!(
                "the profile library holds at most {MAX_LIBRARY_PROFILES} profiles"
            )));
        }
        self.check_name(&listing, &profile.name, &profile.id)?;
        let path = self.path(&profile.id)?;
        std::fs::create_dir_all(&self.dir)?;
        match atomic_create(&path, &encode(profile)?) {
            Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                Err(Error::Document(pcbcraft_core::Error::Conflict(format!(
                    "a user rule profile {} exists",
                    profile.id
                ))))
            }
            other => other.map(|()| profile.clone()),
        }
    }

    /// Replace an existing profile's content (same id).
    pub fn replace(&self, profile: &RuleProfile) -> Result<RuleProfile> {
        check(profile)?;
        self.get(&profile.id)?;
        let listing = self.list()?;
        self.check_name(&listing, &profile.name, &profile.id)?;
        atomic_write(&self.path(&profile.id)?, &encode(profile)?)?;
        Ok(profile.clone())
    }

    /// Give a profile a new shown name; its id (and file) stays, so
    /// projects that snapshotted it keep naming the same lineage.
    pub fn rename(&self, id: &str, name: &str) -> Result<RuleProfile> {
        let mut profile = self.get(id)?;
        profile.name = name.trim().to_owned();
        self.replace(&profile)
    }

    /// Remove a profile file. Projects keep their snapshots.
    pub fn delete(&self, id: &str) -> Result<()> {
        let path = self.path(id)?;
        match std::fs::remove_file(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Err(not_found(id)),
            other => Ok(other?),
        }
    }

    /// An id for `name` that neither the library nor the built-ins use.
    pub fn free_id(&self, name: &str) -> Result<String> {
        let listing = self.list()?;
        let base = slug(name);
        let taken = |id: &str| {
            is_builtin_id(id)
                || listing.profiles.iter().any(|p| p.id == id)
                || self.path(id).is_ok_and(|p| p.exists())
        };
        if !taken(&base) {
            return Ok(base);
        }
        for n in 2..=MAX_LIBRARY_PROFILES + 1 {
            let suffix = format!("-{n}");
            let mut stem = base.clone();
            stem.truncate(MAX_ID - suffix.len());
            let id = format!("{}{suffix}", stem.trim_end_matches('-'));
            if !taken(&id) {
                return Ok(id);
            }
        }
        Err(Error::TooLarge(
            "no free profile id; delete unused profiles".into(),
        ))
    }

    /// A free name derived from `name` ("Fab 6 mil (2)").
    fn free_name(&self, name: &str) -> Result<String> {
        let listing = self.list()?;
        let taken = |n: &str| {
            builtin::all()
                .iter()
                .chain(&listing.profiles)
                .any(|p| p.name.trim().eq_ignore_ascii_case(n.trim()))
        };
        if !taken(name) {
            return Ok(name.trim().to_owned());
        }
        for n in 2..=MAX_LIBRARY_PROFILES + 1 {
            let candidate = format!("{} ({n})", name.trim());
            if !taken(&candidate) {
                return Ok(candidate);
            }
        }
        Err(Error::TooLarge(
            "no free profile name; delete unused profiles".into(),
        ))
    }

    /// Save `profile` (from a project or the panel) as a new user profile
    /// named `name`: a fresh id, origin `User`. Provenance (basis, source,
    /// URL, date, notes) is kept as given.
    pub fn save_as(&self, profile: &RuleProfile, name: &str) -> Result<RuleProfile> {
        let mut stored = profile.clone();
        stored.name = name.trim().to_owned();
        stored.id = self.free_id(name)?;
        stored.provenance.origin = ProfileOrigin::User;
        self.create(&stored)
    }

    /// Import a profile file: decoded and checked, stored with origin
    /// `Imported`, under its own id when free (else a fresh one) and a free
    /// name. The source file is not changed.
    pub fn import(&self, path: &Path) -> Result<RuleProfile> {
        let mut profile = read_profile_file(path)?;
        profile.provenance.origin = ProfileOrigin::Imported;
        let id_free = valid_id(&profile.id)
            && !is_builtin_id(&profile.id)
            && !self.path(&profile.id).is_ok_and(|p| p.exists());
        if !id_free {
            profile.id = self.free_id(&profile.name)?;
        }
        profile.name = self.free_name(&profile.name)?;
        self.create(&profile)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pcbcraft_core::rules::{ProfileBasis, RuleKey, RuleValue};

    fn user(name: &str) -> RuleProfile {
        let mut p = builtin::standard();
        p.id = slug(name);
        p.name = name.into();
        p.provenance.origin = ProfileOrigin::User;
        p.provenance.basis = ProfileBasis::FabricatorEntered;
        p.provenance.source = "Example Fab capabilities page".into();
        p.provenance.url = "https://example.invalid/capabilities".into();
        p.provenance.date = "2026-10-30".into();
        p.provenance.notes = "Entered by hand from the published table.".into();
        p
    }

    #[test]
    fn save_list_rename_delete_round_trip() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let library = ProfileLibrary::new(dir.path().join(PROFILES_DIR));
        assert_eq!(library.list()?, Listing::default(), "missing folder");
        let mut fab = user("Example Fab 5 mil");
        RuleKey::MaskWebMin.set(&mut fab.rules, RuleValue::Length(0.08))?;
        let stored = library.save_as(&fab, "Example Fab 5 mil")?;
        assert_eq!(stored.id, "example-fab-5-mil");
        assert_eq!(stored.provenance.origin, ProfileOrigin::User);
        assert_eq!(stored.provenance.basis, ProfileBasis::FabricatorEntered);
        assert_eq!(library.get(&stored.id)?, stored);
        // A second save of the same name is refused: names stay distinct.
        assert!(library.save_as(&fab, "example fab 5 MIL").is_err());
        let other = library.save_as(&fab, "Second")?;
        assert_eq!(
            library
                .list()?
                .profiles
                .iter()
                .map(|p| p.name.as_str())
                .collect::<Vec<_>>(),
            ["Example Fab 5 mil", "Second"]
        );
        let renamed = library.rename(&other.id, "Alpha house")?;
        assert_eq!(renamed.id, other.id, "a rename keeps the id");
        assert_eq!(library.list()?.profiles[0].name, "Alpha house");
        assert!(
            library.rename(&other.id, "Standard 6/6").is_err(),
            "built-in name"
        );
        library.delete(&other.id)?;
        assert_eq!(
            library.delete(&other.id).map_err(|e| e.code()),
            Err("not_found")
        );
        assert_eq!(library.list()?.profiles, vec![stored]);
        Ok(())
    }

    #[test]
    fn builtin_ids_bad_ids_and_bad_dates_are_refused() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let library = ProfileLibrary::new(dir.path().to_path_buf());
        let mut p = user("x");
        p.id = builtin::DEFAULT_PROFILE_ID.into();
        assert!(library.create(&p).is_err(), "built-in id");
        p.id = "../escape".into();
        assert!(library.create(&p).is_err(), "path in id");
        assert!(library.get("../escape").is_err());
        p.id = "ok".into();
        p.provenance.date = "30.10.2026".into();
        assert!(library.create(&p).is_err(), "date");
        p.provenance.date = String::new();
        p.provenance.origin = ProfileOrigin::BuiltIn;
        assert!(library.create(&p).is_err(), "built-in origin");
        p.provenance.origin = ProfileOrigin::User;
        p.rules.copper.track_width_min = f64::NAN;
        assert!(library.create(&p).is_err(), "rule bounds");
        assert_eq!(std::fs::read_dir(dir.path())?.count(), 0, "nothing written");
        Ok(())
    }

    #[test]
    fn export_then_import_keeps_rules_and_provenance() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let library = ProfileLibrary::new(dir.path().join(PROFILES_DIR));
        let fab = library.save_as(&user("Fab"), "Fab")?;
        let file = dir.path().join("fab.json");
        export(&fab, &file)?;
        assert!(export(&fab, &file).is_err(), "never overwrites");
        assert!(
            export(&fab, &dir.path().join("fab.txt")).is_err(),
            "extension"
        );
        export_replacing(&builtin::hdi(), &file)?;
        assert_eq!(read_profile_file(&file)?, builtin::hdi());
        export_replacing(&fab, &file)?;
        // Importing it next to itself gets a fresh id and a free name.
        let imported = library.import(&file)?;
        assert_eq!(imported.provenance.origin, ProfileOrigin::Imported);
        assert_eq!(imported.rules, fab.rules);
        assert_eq!(imported.provenance.url, fab.provenance.url);
        assert_eq!(imported.id, "fab-2");
        assert_eq!(imported.name, "Fab (2)");
        // A built-in exported and imported becomes a user copy.
        let builtin_file = dir.path().join("fine.json");
        export(&builtin::fine(), &builtin_file)?;
        let copy = library.import(&builtin_file)?;
        assert_eq!(copy.id, "fine-4-4-2");
        assert_eq!(copy.name, "Fine 4/4 (2)");
        assert_eq!(copy.provenance.basis, ProfileBasis::Estimated);
        assert_eq!(library.list()?.profiles.len(), 3);
        Ok(())
    }

    #[test]
    fn damaged_unsupported_and_misnamed_files_are_reported_not_dropped() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let library = ProfileLibrary::new(dir.path().to_path_buf());
        let good = library.save_as(&user("Good"), "Good")?;
        std::fs::write(dir.path().join("broken.json"), "{\"format\":1")?;
        let mut newer =
            String::from_utf8(encode(&user("New"))?).map_err(|e| Error::Invalid(e.to_string()))?;
        newer = newer.replacen("\"format\": 1", "\"format\": 2", 1);
        std::fs::write(dir.path().join("new.json"), &newer)?;
        std::fs::write(dir.path().join("other.json"), encode(&user("Elsewhere"))?)?;
        std::fs::write(dir.path().join("notes.txt"), "not a profile")?;
        let listing = library.list()?;
        assert_eq!(listing.profiles, vec![good]);
        let mut problems: Vec<_> = listing
            .problems
            .iter()
            .filter_map(|p| p.file.file_name()?.to_str().map(str::to_owned))
            .collect();
        problems.sort();
        assert_eq!(problems, ["broken.json", "new.json", "other.json"]);
        assert_eq!(
            read_profile_file(&dir.path().join("new.json")).map_err(|e| e.code()),
            Err("unsupported")
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("new.json"))?,
            newer,
            "never rewritten"
        );
        // Oversized input is refused before parsing.
        let huge = vec![b' '; MAX_PROFILE_FILE_BYTES + 1];
        assert_eq!(decode(&huge).map_err(|e| e.code()), Err("limit_exceeded"));
        Ok(())
    }

    #[test]
    fn slugs_and_ids() {
        assert_eq!(slug("Fine 4/4 — Fab X"), "fine-4-4-fab-x");
        assert_eq!(slug("///"), "profile");
        assert!(slug(&"a".repeat(200)).len() <= MAX_ID);
        assert!(valid_id("std-6-6"));
        assert!(!valid_id("Std"));
        assert!(!valid_id("-a"));
        assert!(!valid_id(""));
        assert!(valid_date("2026-02-28"));
        assert!(!valid_date("2026-13-01"));
        assert!(!valid_date("26-01-01"));
    }
}
