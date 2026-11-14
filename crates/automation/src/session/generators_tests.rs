//! GP1 acceptance through the session (the CLI's and MCP's request path):
//! a part made from the shipped chip module, saved, reopened with the
//! generator folder gone; a changed embedded module and a changed stored
//! part refused with both hashes, the open document untouched.
use super::super::Session;
use serde_json::{Value, json};
use sparksmith_core::Engine;
use sparksmith_core::library::generated::MODULE_MEDIA_TYPE;
use sparksmith_io::shipped::{MARKER, ShippedLibrary};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

type TestResult<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

/// The repository's chip generator bundle (`cargo xtask generators build`
/// keeps it equal to modules.lock and the crate's part files).
const BUNDLE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../library/generators/chip");
/// Its module.
#[allow(dead_code)]
const CHIP: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../library/generators/chip/chip.wasm"
);

/// A shipped library in `dir` holding only the chip generator's bundle.
fn library(dir: &Path) -> TestResult<PathBuf> {
    let root = dir.join("library");
    std::fs::create_dir_all(root.join("generators"))?;
    std::fs::write(root.join(MARKER), "{\"format\":1}\n")?;
    copy_dir(Path::new(BUNDLE), &root.join("generators/chip"))?;
    Ok(root)
}

fn copy_dir(from: &Path, to: &Path) -> TestResult {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let path = entry?.path();
        let target = to.join(path.file_name().ok_or("no file name")?);
        if path.is_dir() {
            copy_dir(&path, &target)?;
        } else {
            std::fs::copy(&path, &target)?;
        }
    }
    Ok(())
}

fn session(root: &Path) -> TestResult<Session> {
    let mut session = Session::new(Engine::default());
    session.set_shipped(ShippedLibrary::at(root)?);
    Ok(session)
}

fn call(session: &mut Session, command: &str, mut params: Value) -> Value {
    if let Some(map) = params.as_object_mut()
        && !map.contains_key("expected_revision")
        && !matches!(command, "library.generators" | "project.save")
    {
        map.insert(
            "expected_revision".into(),
            json!(session.engine().revision()),
        );
    }
    session.dispatch(json!({"command": command, "params": params}))
}

/// A project with a chip resistor (component) and a chip capacitor
/// package made from the shipped module, saved at `path`.
fn generated_project(dir: &Path) -> TestResult<(Session, PathBuf)> {
    let root = library(dir)?;
    let mut session = session(&root)?;
    let made = call(
        &mut session,
        "library.generate_part",
        json!({"generator": "chip", "scope": "component", "params": {"part": "resistor"}}),
    );
    assert_eq!(made["ok"], true, "{made}");
    let report = &made["result"]["outcome"]["report"];
    assert_eq!(report["created"], true, "{report}");
    assert_eq!(report["revisions"].as_array().map(Vec::len), Some(5));
    let package = call(
        &mut session,
        "library.generate_part",
        json!({"generator": "chip", "scope": "package", "params": {"part": "capacitor", "length": 2_000_000, "width": 1_250_000, "height": 1_250_000, "termination": 500_000}}),
    );
    assert_eq!(package["ok"], true, "{package}");
    let path = dir.join("generated.sparksmith");
    let saved = call(
        &mut session,
        "project.save",
        json!({"path": path.display().to_string()}),
    );
    assert_eq!(saved["ok"], true, "{saved}");
    Ok((session, path))
}

#[test]
fn a_generated_part_reopens_offline_identically() -> TestResult {
    let temp = tempfile::tempdir()?;
    let (made, path) = generated_project(temp.path())?;
    let library = &made.engine().project().library;
    assert_eq!(library.generated.len(), 2);
    let modules = library
        .assets
        .iter()
        .filter(|a| a.media_type == MODULE_MEDIA_TYPE)
        .count();
    assert_eq!(modules, 1, "both parts share one embedded module");

    // The generator folder is gone: the project reopens from its own copy.
    std::fs::remove_dir_all(temp.path().join("library/generators"))?;
    let mut reopened = session(&temp.path().join("library"))?;
    let opened = call(
        &mut reopened,
        "project.open",
        json!({"path": path.display().to_string()}),
    );
    assert_eq!(opened["ok"], true, "{opened}");
    assert_eq!(reopened.engine().to_file(), made.engine().to_file());

    let listed = call(&mut reopened, "library.generators", json!({}));
    assert_eq!(listed["ok"], true, "{listed}");
    let parts = listed["result"]["parts"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(parts.len(), 2);
    assert!(
        parts.iter().all(|p| p["state"] == "not_shipped"),
        "{listed}"
    );
    assert_eq!(listed["result"]["shipped"], json!([]));
    Ok(())
}

#[test]
fn the_same_part_twice_is_found_not_made_again() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = library(temp.path())?;
    let mut session = session(&root)?;
    let params = json!({"generator": "chip", "scope": "component", "preset": "resistor-0603"});
    let first = call(&mut session, "library.generate_part", params.clone());
    assert_eq!(first["ok"], true, "{first}");
    let revision = session.engine().revision();
    let again = call(&mut session, "library.generate_part", params);
    assert_eq!(again["ok"], true, "{again}");
    let report = &again["result"]["outcome"]["report"];
    assert_eq!(report["created"], false, "{report}");
    assert_eq!(
        report["key"], first["result"]["outcome"]["report"]["key"],
        "the part key is module hash and params hash"
    );
    assert_eq!(session.engine().revision(), revision, "nothing changed");
    let listed = call(&mut session, "library.generators", json!({}));
    assert_eq!(listed["result"]["parts"][0]["state"], "current", "{listed}");
    assert_eq!(listed["result"]["shipped"][0]["name"], "chip", "{listed}");
    Ok(())
}

#[test]
fn refusals_change_nothing() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = library(temp.path())?;
    let mut session = session(&root)?;
    let before = session.engine().to_file();
    let revision = session.engine().revision();
    for params in [
        // The module's own refusal: terminations longer than the body.
        json!({"generator": "chip", "scope": "component", "params": {"length": 600_000, "termination": 400_000}}),
        // A parameter the module does not have.
        json!({"generator": "chip", "scope": "component", "params": {"colour": "red"}}),
        // No such generator.
        json!({"generator": "nonesuch", "scope": "component"}),
        // An unknown preset.
        json!({"generator": "chip", "scope": "package", "preset": "9999"}),
    ] {
        let reply = call(&mut session, "library.generate_part", params);
        assert_eq!(reply["ok"], false, "{reply}");
    }
    let bad = call(
        &mut session,
        "library.generate_part",
        json!({"generator": "chip", "scope": "component", "params": {"length": 600_000, "termination": 400_000}}),
    );
    assert!(bad["error"]["rule"] == "param.inconsistent", "{bad}");
    // Inline bytes that do not hash to the hash named with them.
    let chip = std::fs::read(CHIP)?;
    let forged = call(
        &mut session,
        "project.apply",
        json!({"operation": {"editor": "Library", "operation": {"command": "library.generate", "params": {
            "module": {"hash": "00".repeat(32), "data": base64(&chip)},
            "scope": "component",
        }}}}),
    );
    assert_eq!(forged["ok"], false, "{forged}");
    assert_eq!(session.engine().revision(), revision);
    assert_eq!(session.engine().to_file(), before);
    Ok(())
}

/// Rewrite the container at `path`, passing each entry's name and bytes
/// through `edit`.
fn rewrite(path: &Path, edit: impl Fn(&str, Vec<u8>) -> TestResult<Vec<u8>>) -> TestResult {
    let mut entries = Vec::new();
    {
        let mut archive = zip::ZipArchive::new(std::fs::File::open(path)?)?;
        for index in 0..archive.len() {
            let mut entry = archive.by_index(index)?;
            let name = entry.name()?.into_owned();
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes)?;
            entries.push((name.clone(), edit(&name, bytes)?));
        }
    }
    let mut writer = zip::ZipWriter::new(std::fs::File::create(path)?);
    for (name, bytes) in entries {
        writer.start_file(name, zip::write::SimpleFileOptions::default())?;
        writer.write_all(&bytes)?;
    }
    writer.finish()?;
    Ok(())
}

/// A session holding another project, its file and revision, to show a
/// refused open leaves it as it was.
fn open_refused(root: &Path, path: &Path) -> TestResult<String> {
    let mut other = session(root)?;
    let made = call(
        &mut other,
        "library.generate_part",
        json!({"generator": "chip", "scope": "package", "params": {"part": "inductor"}}),
    );
    assert_eq!(made["ok"], true, "{made}");
    let before = other.engine().to_file();
    let revision = other.engine().revision();
    let opened = call(
        &mut other,
        "project.open",
        json!({"path": path.display().to_string()}),
    );
    assert_eq!(opened["ok"], false, "{opened}");
    assert_eq!(
        other.engine().to_file(),
        before,
        "the open document is unchanged"
    );
    assert_eq!(other.engine().revision(), revision);
    Ok(format!(
        "{}: {}",
        opened["error"]["rule"].as_str().unwrap_or(""),
        opened["error"]["message"].as_str().unwrap_or("")
    ))
}

/// The two 64-digit hex hashes a message names.
fn hashes(message: &str) -> Vec<String> {
    message
        .split("sha256 ")
        .skip(1)
        .map(|rest| rest.chars().take_while(char::is_ascii_hexdigit).collect())
        .filter(|h: &String| h.len() == 64)
        .collect()
}

#[test]
fn a_changed_embedded_module_is_refused_naming_both_hashes() -> TestResult {
    let temp = tempfile::tempdir()?;
    let (made, path) = generated_project(temp.path())?;
    let module = made
        .engine()
        .project()
        .library
        .generated
        .first()
        .map(|g| g.from.module)
        .ok_or("no generated part")?;
    let entry = format!("assets/sha256/{module}");
    rewrite(&path, |name, mut bytes| {
        if name == entry
            && let Some(last) = bytes.last_mut()
        {
            *last ^= 0x01;
        }
        Ok(bytes)
    })?;
    let changed = sparksmith_core::library::bundle::asset_hash(&{
        let mut chip = std::fs::read(CHIP)?;
        if let Some(last) = chip.last_mut() {
            *last ^= 0x01;
        }
        chip
    });
    let message = open_refused(&temp.path().join("library"), &path)?;
    assert!(message.starts_with("generated.module_changed"), "{message}");
    assert!(message.contains("generator module"), "{message}");
    assert_eq!(
        hashes(&message),
        vec![module.to_string(), changed.to_string()],
        "{message}"
    );
    Ok(())
}

#[test]
fn a_changed_stored_part_is_refused_naming_both_hashes() -> TestResult {
    let temp = tempfile::tempdir()?;
    let (made, path) = generated_project(temp.path())?;
    let footprint = made
        .engine()
        .project()
        .library
        .generated
        .first()
        .and_then(|g| g.expansion.get(3).copied())
        .ok_or("no generated footprint")?;
    rewrite(&path, |name, bytes| {
        if name != "project.json" {
            return Ok(bytes);
        }
        let mut document: Value = serde_json::from_slice(&bytes)?;
        let footprints = document["project"]["library"]["footprints"]
            .as_array_mut()
            .ok_or("no footprints")?;
        let stored = footprints
            .iter_mut()
            .find(|f| f["id"] == json!(footprint.id) && f["revision"] == json!(footprint.revision))
            .ok_or("generated footprint not stored")?;
        // Move pad 1 by 10 µm: still a valid footprint, no longer the
        // generator's.
        let x = &mut stored["definition"]["pads"][0]["position"]["x_nm"];
        *x = json!(x.as_i64().ok_or("no pad position")? - 10_000);
        Ok(serde_json::to_vec(&document)?)
    })?;
    let message = open_refused(&temp.path().join("library"), &path)?;
    assert!(
        message.starts_with("generated.expansion_changed"),
        "{message}"
    );
    let named = hashes(&message);
    assert_eq!(named.len(), 2, "{message}");
    assert_ne!(named[0], named[1]);
    Ok(())
}

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for k in 0..4 {
            if k <= chunk.len() {
                out.push(char::from(ALPHABET[((n >> (18 - 6 * k)) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}
