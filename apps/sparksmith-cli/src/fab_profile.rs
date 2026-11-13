//! `fab-profile list|import|export`: the user's fab profile
//! library (`sparksmith_io::rule_profiles`, JSON files in
//! `SPARKSMITH_CONFIG_DIR/fab-profiles/` or the platform config folder) and
//! the read-only built-in profiles. Switching a project's profile is the typed `rules.set_fab_profile` request
//! (`stdio`/`mcp`); this subcommand only moves profile files.
use super::{Result, open, print};
use serde_json::json;
use sparksmith_core::product::SHORT_NAME as CLI;
use sparksmith_core::rules::{RuleEntry, RuleProfile};
use sparksmith_io::rule_profiles::{self, ProfileLibrary};
use sparksmith_io::settings::UserDirs;
use std::path::Path;

/// The usage line(s).
pub const USAGE: &str = "fab-profile list        list the user's fab profiles and the shipped ones
  fab-profile import FILE.json
                                   add a fab profile file to the user's library
  fab-profile export ID|FILE.sparksmith NEW_FILE.json
                                   write a fab profile (or a project's effective rules)";

fn library() -> Result<ProfileLibrary> {
    Ok(ProfileLibrary::from_dirs(&UserDirs::from_env()?)?)
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
        if name.len() <= sparksmith_core::rules::MAX_PROFILE_TEXT {
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
                .chain(listing.builtins.iter().map(|p| summary(p, "built-in")))
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
                .is_some_and(|e| e == sparksmith_core::format::EXTENSION)
            {
                project_profile(source)?
            } else {
                library.get(source)?
            };
            rule_profiles::export(&profile, Path::new(target))?;
            Ok(json!({"exported": summary(&profile, "file"), "path": target}))
        }
        _ => Err(format!("usage: {CLI} {USAGE}").into()),
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
        let library = ProfileLibrary::new(dir.path().join("fab-profiles"));
        let kinds = |listed: &serde_json::Value| -> Vec<(String, String)> {
            listed["profiles"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|p| {
                    let text = |v: &serde_json::Value| v.as_str().unwrap_or("").to_owned();
                    (text(&p["id"]), text(&p["kind"]))
                })
                .collect()
        };
        let pair = |id: &str, kind: &str| (id.to_owned(), kind.to_owned());
        let listed = reply(&library, &args(&["list"]))?;
        assert_eq!(
            kinds(&listed),
            [
                pair("jlcpcb-2-layer-1oz", "built-in"),
                pair("jlcpcb-4-layer-1oz", "built-in"),
            ],
            "the built-ins, with an empty user library"
        );
        // A built-in exports like a user profile.
        let builtin = dir.path().join("builtin.json");
        reply(
            &library,
            &args(&["export", "jlcpcb-4-layer-1oz", &builtin.to_string_lossy()]),
        )?;
        assert_eq!(
            rule_profiles::read_profile_file(&builtin)?.name,
            "JLCPCB 4-layer 1 oz"
        );
        let imported = reply(&library, &args(&["import", EXAMPLE]))?;
        assert_eq!(imported["imported"]["id"], "jlcpcb-2-layer-1oz");
        assert_eq!(imported["imported"]["source"]["publisher"], "JLCPCB");
        let listed = reply(&library, &args(&["list"]))?;
        assert_eq!(
            kinds(&listed),
            [
                pair("jlcpcb-2-layer-1oz", "user"),
                pair("jlcpcb-4-layer-1oz", "built-in"),
            ],
            "the user's copy replaces the built-in of its id"
        );

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
