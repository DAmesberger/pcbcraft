//! `rules-profile list|import|export`: the user's design-rule profile
//! library (`pcbcraft_io::rule_profiles`, JSON files in
//! `PCBCRAFT_CONFIG_DIR/rule-profiles/` or the platform config folder).
//! Switching a project's profile is the typed `rules.set_profile` request
//! (`stdio`/`mcp`); this subcommand only moves profile files.
use super::{Result, open, print};
use pcbcraft_core::rules::{RuleEntry, RuleProfile};
use pcbcraft_io::rule_profiles::{self, ProfileLibrary};
use pcbcraft_io::settings::UserDirs;
use serde_json::json;
use std::path::Path;

/// The usage line(s).
pub const USAGE: &str =
    "rules-profile list      list the user's rule profiles (Pcbcraft ships none)
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
        "source": profile.source,
        "drill_files": profile.drill_files,
    })
}

/// The project's rules as a profile: its snapshot with every project
/// override written in as a value whose source says so.
fn project_profile(path: &str) -> Result<RuleProfile> {
    let session = open(Some(&path.to_owned()))?;
    let rules = &session.engine().project().rules;
    let mut profile = rules.require()?.clone();
    if !rules.overrides.is_empty() {
        for o in &rules.overrides {
            profile.set(o.key, RuleEntry::value(o.value, "Project override"))?;
        }
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
            let profiles: Vec<_> = listing
                .profiles
                .iter()
                .map(|p| summary(p, "user"))
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
                library.get(source)?
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

    /// The sourced example profile file in `examples/profiles/`.
    const EXAMPLE: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../examples/profiles/jlcpcb-2-layer-1oz.json"
    );

    #[test]
    fn list_import_export_through_the_subcommand() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let library = ProfileLibrary::new(dir.path().join("rule-profiles"));
        let listed = reply(&library, &args(&["list"]))?;
        assert_eq!(
            listed["profiles"].as_array().map(Vec::len),
            Some(0),
            "Pcbcraft ships no profiles"
        );
        let imported = reply(&library, &args(&["import", EXAMPLE]))?;
        assert_eq!(imported["imported"]["id"], "jlcpcb-2-layer-1oz");
        assert_eq!(imported["imported"]["source"]["publisher"], "JLCPCB");
        let listed = reply(&library, &args(&["list"]))?;
        assert_eq!(listed["profiles"].as_array().map(Vec::len), Some(1));
        assert_eq!(listed["profiles"][0]["kind"], "user");

        let file = dir.path().join("again.json");
        let file_text = file.to_string_lossy().into_owned();
        reply(
            &library,
            &args(&["export", "jlcpcb-2-layer-1oz", &file_text]),
        )?;
        assert!(
            reply(
                &library,
                &args(&["export", "jlcpcb-2-layer-1oz", &file_text])
            )
            .is_err(),
            "an export never replaces a file"
        );
        assert!(reply(&library, &args(&["export", "no-such", "x.json"])).is_err());
        assert_eq!(
            rule_profiles::read_profile_file(&file)?,
            rule_profiles::read_profile_file(Path::new(EXAMPLE))?
        );
        assert!(reply(&library, &args(&["bogus"])).is_err());
        Ok(())
    }
}
