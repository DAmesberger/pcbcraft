//! The chain and the golden files: one per format version, of the same
//! small original design (`Engine::demo`, two connected parts placed on
//! the board, with their library and open drafts), kept forever.
//!
//! Each old golden converts to the current golden byte for byte, and the
//! current golden opens and saves byte for byte. A golden file is never
//! regenerated once committed. When `FILE_VERSION` rises, the new
//! version's golden is the previous golden converted by the new step
//! (`write_missing_golden_files`, which writes only files that are
//! missing), checked by hand before it is committed.
use super::*;
use crate::{read_project, write_project};
use sparksmith_core::Engine;
use sparksmith_core::format::CONTAINER_DOCUMENT;
use std::io::{Read, Write};
use std::path::PathBuf;

type TestResult = std::result::Result<(), Box<dyn std::error::Error + Send + Sync>>;

fn golden(version: u32) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/format")
        .join(format!("v{version}"))
        .join("demo.sparksmith")
}

#[test]
fn the_chain_has_one_step_from_every_older_version() {
    for version in 1..FILE_VERSION {
        assert_eq!(
            STEPS.iter().filter(|s| s.from == version).count(),
            1,
            "one step from version {version}"
        );
    }
    assert!(STEPS.iter().all(|s| s.from < FILE_VERSION));
}

#[test]
fn every_version_has_a_golden_file() {
    for version in 1..=FILE_VERSION {
        assert!(golden(version).is_file(), "{}", golden(version).display());
    }
}

#[test]
fn the_current_golden_opens_and_saves_byte_for_byte() -> TestResult {
    let dir = tempfile::tempdir()?;
    let saved = dir.path().join("saved.sparksmith");
    let file = read_project(&golden(FILE_VERSION))?;
    Engine::from_file(file.clone())?;
    write_project(&saved, &file)?;
    assert_eq!(fs_read(&saved)?, fs_read(&golden(FILE_VERSION))?);
    Ok(())
}

#[test]
fn each_old_golden_converts_to_the_current_golden() -> TestResult {
    let dir = tempfile::tempdir()?;
    for version in 1..FILE_VERSION {
        let path = dir.path().join(format!("old-v{version}.sparksmith"));
        std::fs::copy(golden(version), &path)?;
        let refused = read_project(&path).err().ok_or("an old file is refused")?;
        assert!(
            refused.to_string().contains(&format!(
                "`{} convert FILE`",
                sparksmith_core::product::SHORT_NAME
            )),
            "{refused}"
        );
        let lines = convert_path(&path, &Options::default())?;
        assert!(
            lines[0].contains(&format!(
                "converted from version {version} to {FILE_VERSION}"
            )),
            "{lines:?}"
        );
        assert_eq!(
            fs_read(&path)?,
            fs_read(&golden(FILE_VERSION))?,
            "v{version}"
        );
        let backup = path.with_file_name(format!("old-v{version}.sparksmith.v{version}"));
        assert_eq!(fs_read(&backup)?, fs_read(&golden(version))?);
        Engine::from_file(read_project(&path)?)?;
        // A converted file is current: a second run changes nothing.
        let again = convert_path(&path, &Options::default())?;
        assert_eq!(
            again,
            [format!(
                "{}: already version {FILE_VERSION}",
                path.display()
            )]
        );
        assert_eq!(fs_read(&path)?, fs_read(&golden(FILE_VERSION))?);
    }
    Ok(())
}

#[test]
fn the_chain_refuses_what_it_cannot_convert() {
    let options = Options::default();
    let newer = upgrade(
        &mut serde_json::json!({"version": FILE_VERSION + 1}),
        &options,
    );
    assert!(newer.is_err_and(|e| e.to_string().contains("newer")));
    for bad in [
        serde_json::json!({}),
        serde_json::json!({"version": 0}),
        serde_json::json!({"version": "2"}),
    ] {
        assert!(upgrade(&mut bad.clone(), &options).is_err(), "{bad}");
    }
    let mut current = serde_json::json!({"version": FILE_VERSION});
    assert_eq!(
        upgrade(&mut current, &options).ok(),
        Some(Upgraded {
            from: FILE_VERSION,
            values: 0
        })
    );
}

fn fs_read(path: &std::path::Path) -> std::io::Result<Vec<u8>> {
    std::fs::read(path)
}

/// Writes the golden files that are missing (run with `--ignored` after
/// raising `FILE_VERSION`). The first golden of all was the demo design as
/// version 2 saved it; version 1 is that file as the version 1 writer
/// wrote it, which differed only in the version number.
#[test]
#[ignore = "writes golden files; run by hand when a version is added"]
fn write_missing_golden_files() -> TestResult {
    if !golden(2).exists() {
        std::fs::create_dir_all(golden(2).parent().ok_or("folder")?)?;
        write_project(&golden(2), &Engine::demo()?.to_file())?;
    }
    if !golden(1).exists() {
        std::fs::create_dir_all(golden(1).parent().ok_or("folder")?)?;
        let bytes = fs_read(&golden(2))?;
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(&bytes[..]))?;
        let mut out = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .last_modified_time(zip::DateTime::default())
            .unix_permissions(0o644);
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i)?;
            if entry.name()? == CONTAINER_DOCUMENT {
                let mut text = String::new();
                entry.read_to_string(&mut text)?;
                let old = text.replacen("\"version\": 2,", "\"version\": 1,", 1);
                assert_ne!(old, text);
                out.start_file(CONTAINER_DOCUMENT, options)?;
                out.write_all(old.as_bytes())?;
            } else {
                drop(entry);
                out.raw_copy_file(archive.by_index_raw(i)?)?;
            }
        }
        std::fs::write(golden(1), out.finish()?.into_inner())?;
    }
    let (current, previous) = (golden(FILE_VERSION), golden(FILE_VERSION - 1));
    if !current.exists() {
        std::fs::create_dir_all(current.parent().ok_or("folder")?)?;
        std::fs::copy(&previous, &current)?;
        convert_path(&current, &Options::default())?;
        let backup = format!("{}.v{}", current.display(), FILE_VERSION - 1);
        std::fs::remove_file(backup)?;
    }
    Ok(())
}
