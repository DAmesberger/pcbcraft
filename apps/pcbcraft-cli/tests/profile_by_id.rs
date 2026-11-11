//! A fabrication profile named by id resolves through the user's profile
//! library: the CLI runs with a temporary config folder
//! (`PCBCRAFT_CONFIG_DIR`) holding the two sourced test fixture profiles,
//! imported through `rules-profile import`.
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
    let mut command = Command::new(env!("CARGO_BIN_EXE_pcbcraft-cli"));
    command
        .env("PCBCRAFT_CONFIG_DIR", config)
        .env("PCBCRAFT_CACHE_DIR", cache);
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
        run(&config, &cache, &["rules-profile", "import", &file])?;
    }
    let listed: Value = serde_json::from_str(&run(&config, &cache, &["rules-profile", "list"])?)?;
    let ids: Vec<&str> = listed["profiles"]
        .as_array()
        .ok_or("profiles")?
        .iter()
        .filter_map(|p| p["id"].as_str())
        .collect();
    assert_eq!(ids, ["test-fine", "test-prototype"]);

    // The showcase on the library's prototype profile, by id.
    let board = dir.path().join("board.pcbcraft");
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
            preflight(fine["profile"].clone()),
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
