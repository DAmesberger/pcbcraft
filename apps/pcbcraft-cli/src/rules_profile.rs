//! `rules-profile list|import|export`: the user's design-rule profile
//! library (`pcbcraft_io::rule_profiles`, JSON files in
//! `PCBCRAFT_CONFIG_DIR/rule-profiles/` or the platform config folder).
//! Switching a project's profile is the typed `rules.set_profile` request
//! (`stdio`/`mcp`); this subcommand only moves profile files.
use super::{Result, open, print};
use pcbcraft_core::rules::{ProfileOrigin, RuleProfile, builtin};
use pcbcraft_io::rule_profiles::{self, ProfileLibrary};
use pcbcraft_io::settings::UserDirs;
use serde_json::json;
use std::path::Path;

/// The usage line(s).
pub const USAGE: &str = "rules-profile list      list built-in and user design-rule profiles
  rules-profile import FILE.json
                                   add a profile file to the user profile library
  rules-profile export ID|FILE.pcbcraft NEW_FILE.json
                                   write a profile (or a project's effective rules)";

fn library() -> Result<ProfileLibrary> {
    Ok(ProfileLibrary::from_dirs(&UserDirs::from_env()?))
}

fn summary(profile: &RuleProfile, kind: &str) -> serde_json::Value {
    json!({
        "id": profile.id,
        "name": profile.name,
        "kind": kind,
        "provenance": profile.provenance,
    })
}

/// A built-in or user profile by id.
fn find(library: &ProfileLibrary, id: &str) -> Result<RuleProfile> {
    if let Some(profile) = builtin::all().into_iter().find(|p| p.id == id) {
        return Ok(profile);
    }
    Ok(library.get(id)?)
}

/// The project's effective rules as a profile: its snapshot with every
/// project override applied. With overrides it is the user's own profile.
fn project_profile(path: &str) -> Result<RuleProfile> {
    let session = open(Some(&path.to_owned()))?;
    let rules = &session.engine().project().rules;
    let mut profile = rules.profile.clone();
    if !rules.overrides.is_empty() {
        profile.rules = rules.effective();
        profile.provenance.origin = ProfileOrigin::User;
        let name = format!("{} with project overrides", profile.name);
        if name.len() <= pcbcraft_core::rules::MAX_PROFILE_TEXT {
            profile.name = name;
        }
    }
    Ok(profile)
}

/// Run the subcommand with the arguments after its name.
pub fn run(args: &[String]) -> Result<()> {
    print(&reply(&library()?, args)?)
}

/// The subcommand's JSON reply over `library`.
fn reply(library: &ProfileLibrary, args: &[String]) -> Result<serde_json::Value> {
    match args {
        [command] if command == "list" => {
            let listing = library.list()?;
            let profiles: Vec<_> = builtin::all()
                .iter()
                .map(|p| summary(p, "built_in"))
                .chain(listing.profiles.iter().map(|p| summary(p, "user")))
                .collect();
            let problems: Vec<_> = listing
                .problems
                .iter()
                .map(|p| json!({"file": p.file, "message": p.message}))
                .collect();
            Ok(json!({
                "folder": library.dir(),
                "profiles": profiles,
                "problems": problems,
            }))
        }
        [command, file] if command == "import" => {
            let stored = library.import(Path::new(file))?;
            Ok(json!({"imported": summary(&stored, "user")}))
        }
        [command, source, target] if command == "export" => {
            let profile = if Path::new(source)
                .extension()
                .is_some_and(|e| e == pcbcraft_core::format::EXTENSION)
            {
                project_profile(source)?
            } else {
                find(library, source)?
            };
            rule_profiles::export(&profile, Path::new(target))?;
            Ok(json!({"exported": summary(&profile, "file"), "path": target}))
        }
        _ => Err(format!("usage: pcbcraft-cli {USAGE}").into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn list_export_import_through_the_subcommand() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let library = ProfileLibrary::new(dir.path().join("rule-profiles"));
        let listed = reply(&library, &args(&["list"]))?;
        let ids: Vec<_> = listed["profiles"]
            .as_array()
            .map(|a| a.iter().filter_map(|p| p["id"].as_str()).collect())
            .unwrap_or_default();
        assert_eq!(ids, ["pcbcraft-default", "std-6-6", "fine-4-4", "hdi-3-3"]);
        assert_eq!(listed["profiles"][0]["provenance"]["basis"], "estimated");

        let file = dir.path().join("fine.json");
        let file_text = file.to_string_lossy().into_owned();
        reply(&library, &args(&["export", "fine-4-4", &file_text]))?;
        assert!(
            reply(&library, &args(&["export", "fine-4-4", &file_text])).is_err(),
            "an export never replaces a file"
        );
        assert!(reply(&library, &args(&["export", "no-such", "x.json"])).is_err());
        let imported = reply(&library, &args(&["import", &file_text]))?;
        assert_eq!(imported["imported"]["id"], "fine-4-4-2");
        assert_eq!(imported["imported"]["provenance"]["origin"], "imported");
        let listed = reply(&library, &args(&["list"]))?;
        assert_eq!(listed["profiles"].as_array().map(Vec::len), Some(5));
        assert_eq!(listed["profiles"][4]["kind"], "user");
        // The user profile exports by its id too.
        let again = dir.path().join("again.json");
        reply(
            &library,
            &args(&["export", "fine-4-4-2", &again.to_string_lossy()]),
        )?;
        assert_eq!(
            rule_profiles::read_profile_file(&again)?.rules,
            builtin::fine().rules
        );
        assert!(reply(&library, &args(&["bogus"])).is_err());
        Ok(())
    }
}
