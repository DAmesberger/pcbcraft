//! A fabrication profile named by id resolves through the user's profile
//! library: the CLI runs with a temporary config folder
//! (`SPARKSMITH_CONFIG_DIR`) holding the two sourced test fixture profiles,
//! imported through `fab-profile import`.
use serde_json::{Value, json};
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

type Fallible = Result<(), Box<dyn std::error::Error>>;

const FIXTURES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../crates/editor-core/src/rules/fixtures"
);

fn cli(config: &Path, cache: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_sparksmith"));
    command
        .env("SPARKSMITH_CONFIG_DIR", config)
        .env("SPARKSMITH_CACHE_DIR", cache);
    command
}

/// Run `args` and return its standard output; a failure is an error.
fn run(config: &Path, cache: &Path, args: &[&str]) -> Result<String, Box<dyn std::error::Error>> {
    let out = cli(config, cache).args(args).output()?;
    if !out.status.success() {
        return Err(format!("{args:?}: {}", String::from_utf8_lossy(&out.stderr)).into());
    }
    Ok(String::from_utf8(out.stdout)?)
}

/// One reply per request line of an operator stdio session.
fn session(
    config: &Path,
    cache: &Path,
    requests: &[Value],
) -> Result<Vec<Value>, Box<dyn std::error::Error>> {
    let mut child = cli(config, cache)
        .args(["stdio", "--operator"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;
    {
        let mut stdin = child.stdin.take().ok_or("stdin")?;
        for request in requests {
            writeln!(stdin, "{request}")?;
        }
    }
    let out = child.wait_with_output()?;
    String::from_utf8(out.stdout)?
        .lines()
        .map(|l| Ok(serde_json::from_str(l)?))
        .collect()
}

#[test]
fn the_production_check_resolves_a_profile_id_through_the_library() -> Fallible {
    let dir = tempfile::tempdir()?;
    let (config, cache) = (dir.path().join("config"), dir.path().join("cache"));
    for fixture in ["prototype", "fine"] {
        let file = format!("{FIXTURES}/{fixture}.json");
        run(&config, &cache, &["fab-profile", "import", &file])?;
    }
    let listed: Value = serde_json::from_str(&run(&config, &cache, &["fab-profile", "list"])?)?;
    let ids: Vec<&str> = listed["profiles"]
        .as_array()
        .ok_or("profiles")?
        .iter()
        .filter_map(|p| p["id"].as_str())
        .collect();
    assert_eq!(
        ids,
        [
            "test-fine",
            "test-prototype",
            "jlcpcb-2-layer-1oz",
            "jlcpcb-4-layer-1oz"
        ],
        "the user's profiles, then the built-ins"
    );

    // The showcase on the library's prototype profile, by id.
    let board = dir.path().join("board.sparksmith");
    let board_text = board.display().to_string();
    run(
        &config,
        &cache,
        &["showcase", &board_text, "--profile", "test-prototype"],
    )?;
    let fine: Value =
        serde_json::from_str(&std::fs::read_to_string(format!("{FIXTURES}/fine.json"))?)?;
    let preflight = |profile: Value| {
        json!({"command": "project.query", "params": {
            "query": "fabrication.preflight", "params": {"profile": profile}}})
    };
    let replies = session(
        &config,
        &cache,
        &[
            json!({"command": "project.open", "params": {"expected_revision": 0, "path": board_text}}),
            preflight(json!("test-fine")),
            preflight(fine["fab_profile"].clone()),
            preflight(json!("no-such-profile")),
        ],
    )?;
    let [opened, by_id, whole, unknown] = replies.as_slice() else {
        return Err(format!("four replies expected: {replies:?}").into());
    };
    assert_eq!(opened["ok"], true, "{opened}");
    assert_eq!(by_id["ok"], true, "{by_id}");
    // The id reads as the whole profile does.
    assert_eq!(by_id["result"], whole["result"]);
    assert_eq!(by_id["result"]["result"]["profile"], "Test fine");
    // Its fills were computed under the prototype: stale under the fine one.
    assert!(
        by_id["result"]["result"]["blocking"].as_u64() > Some(0),
        "{by_id}"
    );
    assert_eq!(unknown["ok"], false, "{unknown}");
    Ok(())
}

/// `project.new` never asks: without `profile` it takes the user's default
/// (Settings), else the built-in JLCPCB 2-layer 1 oz; `profile` names any
/// library or built-in id. `rules.fab_profiles` lists the built-ins.
#[test]
fn a_new_project_takes_the_builtin_jlcpcb_profile_unless_one_is_named() -> Fallible {
    let dir = tempfile::tempdir()?;
    let (config, cache) = (dir.path().join("config"), dir.path().join("cache"));
    let profile_id =
        json!({"command": "project.inspect", "params": {"at": "/project/rules/profile/id"}});
    let replies = session(
        &config,
        &cache,
        &[
            json!({"command": "project.new", "params": {"expected_revision": 0}}),
            profile_id.clone(),
            json!({"command": "project.query", "params": {"query": "rules.fab_profiles", "params": {}}}),
            json!({"command": "project.new", "params": {"expected_revision": 1, "profile": "jlcpcb-4-layer-1oz"}}),
            profile_id,
        ],
    )?;
    let [new, first, listed, named, second] = replies.as_slice() else {
        return Err(format!("five replies expected: {replies:?}").into());
    };
    assert_eq!(new["ok"], true, "{new}");
    assert_eq!(first["result"]["value"], "jlcpcb-2-layer-1oz", "{first}");
    let ids: Vec<&str> = listed["result"]["result"]
        .as_array()
        .ok_or("profiles")?
        .iter()
        .filter_map(|p| p["id"].as_str())
        .collect();
    assert_eq!(
        ids,
        ["jlcpcb-2-layer-1oz", "jlcpcb-4-layer-1oz"],
        "{listed}"
    );
    assert_eq!(named["ok"], true, "{named}");
    assert_eq!(second["result"]["value"], "jlcpcb-4-layer-1oz", "{second}");
    Ok(())
}
