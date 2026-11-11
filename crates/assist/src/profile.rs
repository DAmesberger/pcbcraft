//! Run profiles: how a run talks to one model. A profile is plain data:
//! the system prompt (the base prompt with the model's own additions, or a
//! replacement), the tools it leaves out and the few harness settings a
//! model's tuning showed it needs. Capabilities stay in
//! [`crate::model::ModelProfile`]; a run profile changes what the model is
//! told and how the loop reacts, never what the model is declared to do.
//!
//! Built-in profiles ship in `crates/assist/profiles/`. A user copies one
//! into their config folder (`assist-profiles/NAME.json`) and edits it;
//! a user file of the same name takes the built-in's place. Every file is
//! checked when it is read, and a broken one is refused with the reason.
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::path::{Path, PathBuf};

/// The one profile format until the first release.
pub const PROFILE_FORMAT: u32 = 1;
/// The folder under the user's config folder that holds their profiles.
pub const USER_FOLDER: &str = "assist-profiles";
/// The profile of a model that has none of its own.
pub const DEFAULT_PROFILE: &str = "default";
const MAX_FILE_BYTES: u64 = 256 * 1024;
const MAX_PROMPT_CHARS: usize = 64 * 1024;
const MAX_NAME: usize = 64;
const MAX_MODELS: usize = 32;

/// The built-in profiles, by name.
const BUILT_IN: &[(&str, &str)] = &[
    ("default", include_str!("../profiles/default.json")),
    ("claude", include_str!("../profiles/claude.json")),
];

/// The system prompt of a profile, as paragraphs (joined by blank lines).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum SystemPrompt {
    /// The base prompt unchanged.
    #[default]
    Base,
    /// The base prompt followed by these paragraphs.
    Add(Vec<String>),
    /// These paragraphs instead of the base prompt.
    Replace(Vec<String>),
}

/// How the loop reacts, where a model needs something other than the
/// default.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Harness {
    /// The same refused call (same tool, same input) this many times ends
    /// the run with the reason; the model is told from the second time.
    #[serde(default = "default_repeat_limit")]
    pub repeat_limit: u32,
}

fn default_repeat_limit() -> u32 {
    3
}

impl Default for Harness {
    fn default() -> Self {
        Self {
            repeat_limit: default_repeat_limit(),
        }
    }
}

/// One model's run profile, as stored.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunProfile {
    pub format: u32,
    pub name: String,
    /// Model ids this profile is chosen for when none is named (exact).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<String>,
    #[serde(default)]
    pub system: SystemPrompt,
    /// Tools the model is not offered.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub omit_tools: Vec<String>,
    #[serde(default)]
    pub harness: Harness,
}

impl Default for RunProfile {
    fn default() -> Self {
        Self {
            format: PROFILE_FORMAT,
            name: DEFAULT_PROFILE.into(),
            models: Vec::new(),
            system: SystemPrompt::Base,
            omit_tools: Vec::new(),
            harness: Harness::default(),
        }
    }
}

impl RunProfile {
    /// Read and check a profile's text; `origin` names it in errors.
    pub fn parse(text: &str, origin: &str) -> Result<Self> {
        let profile: Self = serde_json::from_str(text)
            .map_err(|e| Error::Config(format!("run profile {origin}: {e}")))?;
        profile
            .check()
            .map_err(|why| Error::Config(format!("run profile {origin}: {why}")))?;
        Ok(profile)
    }

    /// Every rule a profile must keep.
    pub fn check(&self) -> std::result::Result<(), String> {
        if self.format != PROFILE_FORMAT {
            return Err(format!(
                "format {} is not supported (this version reads format {PROFILE_FORMAT})",
                self.format
            ));
        }
        if !valid_name(&self.name) {
            return Err(format!(
                "name {:?}: use 1–{MAX_NAME} characters a–z, 0–9, '.', '-' or '_'",
                self.name
            ));
        }
        if self.models.len() > MAX_MODELS
            || self
                .models
                .iter()
                .any(|m| m.trim().is_empty() || m.len() > 200)
        {
            return Err(format!(
                "models: at most {MAX_MODELS} model ids, none empty or over 200 characters"
            ));
        }
        let paragraphs = match &self.system {
            SystemPrompt::Base => &[][..],
            SystemPrompt::Add(p) | SystemPrompt::Replace(p) => p.as_slice(),
        };
        let chars: usize = paragraphs.iter().map(String::len).sum();
        if chars > MAX_PROMPT_CHARS {
            return Err(format!(
                "system: {chars} characters, more than {MAX_PROMPT_CHARS}"
            ));
        }
        if matches!(self.system, SystemPrompt::Replace(_)) && chars == 0 {
            return Err("system: a replacement prompt can't be empty".into());
        }
        let known: Vec<String> = crate::tools::definitions()
            .into_iter()
            .map(|t| t.name)
            .chain([crate::documents::VIEW_PAGE_TOOL.to_owned()])
            .collect();
        for tool in &self.omit_tools {
            if !known.contains(tool) {
                return Err(format!("omit_tools: no tool {tool:?}"));
            }
            if tool == "submit_review" {
                return Err("omit_tools: submit_review ends the run and can't be left out".into());
            }
        }
        if !(2..=20).contains(&self.harness.repeat_limit) {
            return Err("harness.repeat_limit: from 2 to 20".into());
        }
        Ok(())
    }

    /// The system prompt this profile sends.
    pub fn system_prompt(&self) -> Cow<'static, str> {
        match &self.system {
            SystemPrompt::Base => Cow::Borrowed(crate::SYSTEM),
            SystemPrompt::Add(more) => {
                Cow::Owned(format!("{}\n\n{}", crate::SYSTEM, more.join("\n\n")))
            }
            SystemPrompt::Replace(text) => Cow::Owned(text.join("\n\n")),
        }
    }

    /// The profile as a file a user can edit.
    pub fn to_text(&self) -> Result<String> {
        Ok(serde_json::to_string_pretty(self)? + "\n")
    }
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NAME
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b))
}

/// The built-in profile `name`.
pub fn built_in(name: &str) -> Option<Result<RunProfile>> {
    BUILT_IN
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(n, text)| RunProfile::parse(text, &format!("built-in {n}")))
}

/// The names of the built-in profiles.
pub fn built_in_names() -> Vec<&'static str> {
    BUILT_IN.iter().map(|(n, _)| *n).collect()
}

/// The user's profile folder under their config folder.
pub fn user_folder(config: &Path) -> PathBuf {
    config.join(USER_FOLDER)
}

/// Read and check the user's profile file `path`.
pub fn read_file(path: &Path) -> Result<RunProfile> {
    use std::io::Read as _;
    let origin = path.display().to_string();
    let mut text = String::new();
    std::fs::File::open(path)?
        .take(MAX_FILE_BYTES + 1)
        .read_to_string(&mut text)?;
    if text.len() as u64 > MAX_FILE_BYTES {
        return Err(Error::Config(format!(
            "run profile {origin}: larger than {MAX_FILE_BYTES} bytes"
        )));
    }
    let profile = RunProfile::parse(&text, &origin)?;
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned());
    if stem.as_deref() != Some(profile.name.as_str()) {
        return Err(Error::Config(format!(
            "run profile {origin}: its name is {:?}; the file must be {}.json",
            profile.name, profile.name
        )));
    }
    Ok(profile)
}

/// The user's profile files, each read and checked.
fn user_profiles(folder: &Path) -> Result<Vec<RunProfile>> {
    let entries = match std::fs::read_dir(folder) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    paths.sort();
    paths.iter().map(|p| read_file(p)).collect()
}

/// The profile a run uses. `name` given: the user's file of that name, else
/// the built-in. Otherwise the first user profile, then built-in, that lists
/// `model`; then [`DEFAULT_PROFILE`]. `folder` is the user's profile folder,
/// when there is one.
pub fn select(name: Option<&str>, model: &str, folder: Option<&Path>) -> Result<RunProfile> {
    let users = match folder {
        Some(f) => user_profiles(f)?,
        None => Vec::new(),
    };
    let named = |wanted: &str| -> Result<RunProfile> {
        if let Some(p) = users.iter().find(|p| p.name == wanted) {
            return Ok(p.clone());
        }
        built_in(wanted).unwrap_or_else(|| {
            Err(Error::Config(format!(
                "no run profile {wanted:?}; built in: {}",
                built_in_names().join(", ")
            )))
        })
    };
    if let Some(wanted) = name {
        return named(wanted);
    }
    if let Some(p) = users.iter().find(|p| p.models.iter().any(|m| m == model)) {
        return Ok(p.clone());
    }
    for (n, _) in BUILT_IN {
        let p = named(n)?;
        if p.models.iter().any(|m| m == model) {
            return Ok(p);
        }
    }
    named(DEFAULT_PROFILE)
}

/// Copy the built-in profile `name` into the user's folder to edit,
/// refusing to replace a file. Returns its path.
pub fn copy_to_user(name: &str, folder: &Path) -> Result<PathBuf> {
    let profile = built_in(name).unwrap_or_else(|| {
        Err(Error::Config(format!(
            "no built-in run profile {name:?}; built in: {}",
            built_in_names().join(", ")
        )))
    })?;
    std::fs::create_dir_all(folder)?;
    let path = folder.join(format!("{name}.json"));
    crate::write_new(&path, profile.to_text()?.as_bytes())?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    type Fallible = std::result::Result<(), Box<dyn std::error::Error>>;

    #[test]
    fn every_built_in_profile_loads() -> Fallible {
        for name in built_in_names() {
            let p = built_in(name).ok_or("listed")??;
            assert_eq!(p.name, name);
        }
        // Claude's prompt and tools are the base ones, unchanged.
        let claude = select(None, "claude-opus-5-5", None)?;
        assert_eq!(claude.name, "claude");
        assert_eq!(claude.system_prompt(), crate::SYSTEM);
        assert!(claude.omit_tools.is_empty());
        // A model without a profile gets the default.
        assert_eq!(select(None, "some-local-model", None)?.name, "default");
        // Tuning for one third-party model is the user's file, not built in.
        assert_eq!(built_in_names(), ["default", "claude"]);
        let qwen = "RadixArk/Qwen3.8-Flash-Next-NVFP4";
        assert_eq!(select(None, qwen, None)?.name, "default");
        Ok(())
    }

    /// Reports and profile copies are created atomically: complete or not
    /// at all, never over an existing file, with no temporary left behind,
    /// so a failed write cannot leave a partial file that blocks a rerun.
    #[test]
    fn new_files_are_written_whole_or_not_at_all() -> Fallible {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("review.json");
        crate::write_new(&path, b"{\"report\": 1}")?;
        let again = crate::write_new(&path, b"other");
        assert!(
            matches!(&again, Err(Error::Io(e)) if e.kind() == std::io::ErrorKind::AlreadyExists),
            "{again:?}"
        );
        assert_eq!(std::fs::read(&path)?, b"{\"report\": 1}");
        let big = dir.path().join("big.md");
        let refused = crate::write_new(&big, &vec![b'x'; 16 * 1024 * 1024 + 1]);
        assert!(refused.is_err());
        assert!(!big.exists(), "a failed write leaves no partial file");
        assert_eq!(
            std::fs::read_dir(dir.path())?.count(),
            1,
            "no temporary left"
        );
        Ok(())
    }

    #[test]
    fn a_user_copy_is_edited_loaded_and_a_broken_one_refused() -> Fallible {
        let dir = tempfile::tempdir()?;
        let folder = user_folder(dir.path());
        let path = copy_to_user("default", &folder)?;
        // A second copy never replaces the first.
        assert!(copy_to_user("default", &folder).is_err());
        let edited = std::fs::read_to_string(&path)?.replace(
            "\"system\": \"base\"",
            "\"models\": [\"my-model\"],\n  \"system\": {\"add\": [\"Be brief.\"]}",
        );
        std::fs::write(&path, edited)?;
        let p = select(None, "my-model", Some(&folder))?;
        assert_eq!(p.name, "default");
        assert!(p.system_prompt().ends_with("\n\nBe brief."));
        // Named, the user's file takes the built-in's place.
        assert_eq!(select(Some("default"), "x", Some(&folder))?, p);

        let refused = [
            (r#"{"format": 2, "name": "default"}"#, "format 2"),
            (
                r#"{"format": 1, "name": "default", "sytem": "base"}"#,
                "unknown field",
            ),
            (
                r#"{"format": 1, "name": "default", "omit_tools": ["nope"]}"#,
                "no tool",
            ),
            (
                r#"{"format": 1, "name": "default", "omit_tools": ["submit_review"]}"#,
                "submit_review",
            ),
            (
                r#"{"format": 1, "name": "default", "harness": {"repeat_limit": 1}}"#,
                "repeat_limit",
            ),
            (
                r#"{"format": 1, "name": "default", "system": {"replace": []}}"#,
                "empty",
            ),
            (r#"{"format": 1, "name": "other"}"#, "must be other.json"),
        ];
        for (text, why) in refused {
            std::fs::write(&path, text)?;
            let e = select(None, "my-model", Some(&folder))
                .err()
                .ok_or(text)?
                .to_string();
            assert!(e.contains(why) && e.contains("default.json"), "{text}: {e}");
        }
        Ok(())
    }
}
