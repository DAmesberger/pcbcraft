use super::*;
use sparksmith_core::Class;
use std::path::PathBuf;

type Fallible = std::result::Result<(), Box<dyn std::error::Error>>;

fn lock_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(LOCK_FILE)
}

/// `cargo xtask surface --check` runs this; `--update` sets
/// `SPARKSMITH_SURFACE_UPDATE=1`, which rewrites the lock once the
/// ratchet passes.
#[test]
fn the_lock_holds() -> Fallible {
    let current = current();
    let path = lock_path();
    let locked: Option<Lock> = match std::fs::read(&path) {
        Ok(bytes) => Some(serde_json::from_slice(&bytes)?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    let problems = locked
        .as_ref()
        .map(|locked| ratchet(locked, &current, MODULES))
        .unwrap_or_default();
    if std::env::var_os("SPARKSMITH_SURFACE_UPDATE").is_some() {
        if !problems.is_empty() {
            return Err(format!("surface.lock not updated:\n{}", problems.join("\n")).into());
        }
        std::fs::write(&path, text(&current)?)?;
        return Ok(());
    }
    let Some(locked) = locked else {
        return Err("surface.lock is missing: run cargo xtask surface --update".into());
    };
    if !problems.is_empty() {
        return Err(format!(
            "the catalog changed without the version it needs (crates/automation/surface.lock):\n{}",
            problems.join("\n")
        )
        .into());
    }
    if locked != current {
        return Err(
            "surface.lock is out of date: the versions are right, so run cargo xtask surface --update"
                .into(),
        );
    }
    Ok(())
}

#[test]
fn every_module_has_entries_and_every_entry_one_module() {
    let lock = current();
    let mut seen = BTreeSet::new();
    for module in &lock.modules {
        assert!(!module.entries.is_empty(), "module {} is empty", module.id);
        for entry in &module.entries {
            assert!(seen.insert(entry.name.clone()), "{} twice", entry.name);
        }
    }
    assert_eq!(seen.len(), metadata::entries().len());
}

#[test]
fn the_lock_text_reads_back() -> Fallible {
    let lock = current();
    let read: Lock = serde_json::from_str(&text(&lock)?)?;
    assert_eq!(read, lock);
    Ok(())
}

/// A renamed type or an edited doc comment changes no digest; a retyped
/// field does.
#[test]
fn the_canonical_form_ignores_names_and_words() {
    let a = json!({"properties": {"x": {"$ref": "#/$defs/A", "description": "one"}},
        "$defs": {"A": {"type": "integer", "title": "A"}}});
    let b = json!({"properties": {"x": {"$ref": "#/$defs/Renamed", "description": "two"}},
        "$defs": {"Renamed": {"type": "integer"}}});
    let c = json!({"properties": {"x": {"$ref": "#/$defs/A"}},
        "$defs": {"A": {"type": "string"}}});
    assert_eq!(digest(&canonical(&a, &a)), digest(&canonical(&b, &b)));
    assert_ne!(digest(&canonical(&a, &a)), digest(&canonical(&c, &c)));
}

fn module(version: &str, digest: &str, entries: Vec<LockedEntry>) -> Lock {
    Lock {
        format: LOCK_FORMAT,
        modules: vec![LockedModule {
            id: "layout".into(),
            version: version.into(),
            digest: digest.into(),
            entries,
        }],
    }
}

fn entry(name: &str, class: Class, fields: &[(&str, &str)], required: &[&str]) -> LockedEntry {
    LockedEntry {
        name: name.into(),
        class,
        schema: "0".into(),
        fields: fields
            .iter()
            .map(|(n, d)| ((*n).to_owned(), (*d).to_owned()))
            .collect(),
        required: required.iter().map(|r| (*r).to_owned()).collect(),
    }
}

fn base(version: &str) -> Lock {
    module(
        version,
        "sha256:a",
        vec![
            entry(
                "layout.a",
                Class::Edit,
                &[("at", "1"), ("net", "2")],
                &["at"],
            ),
            entry("layout.b", Class::Read, &[], &[]),
        ],
    )
}

/// The planted changes: each fails without a major bump on a 1.x module
/// and passes with one.
#[test]
fn a_breaking_change_needs_a_major_version() {
    let planted = [
        (
            "a removed entry",
            vec![entry(
                "layout.a",
                Class::Edit,
                &[("at", "1"), ("net", "2")],
                &["at"],
            )],
        ),
        (
            "a retyped field",
            vec![
                entry(
                    "layout.a",
                    Class::Edit,
                    &[("at", "9"), ("net", "2")],
                    &["at"],
                ),
                entry("layout.b", Class::Read, &[], &[]),
            ],
        ),
        (
            "a stricter class",
            vec![
                entry(
                    "layout.a",
                    Class::Ask,
                    &[("at", "1"), ("net", "2")],
                    &["at"],
                ),
                entry("layout.b", Class::Read, &[], &[]),
            ],
        ),
        (
            "a removed field",
            vec![
                entry("layout.a", Class::Edit, &[("at", "1")], &["at"]),
                entry("layout.b", Class::Read, &[], &[]),
            ],
        ),
        (
            "a new required field",
            vec![
                entry(
                    "layout.a",
                    Class::Edit,
                    &[("at", "1"), ("net", "2")],
                    &["at", "net"],
                ),
                entry("layout.b", Class::Read, &[], &[]),
            ],
        ),
    ];
    for (what, entries) in planted {
        let unchanged = module("1.2", "sha256:b", entries.clone());
        assert!(
            !ratchet(&base("1.2"), &unchanged, &[]).is_empty(),
            "{what} passed without a version change"
        );
        let minor = module("1.3", "sha256:b", entries.clone());
        assert!(
            !ratchet(&base("1.2"), &minor, &[]).is_empty(),
            "{what} passed with a minor bump"
        );
        let major = module("2.0", "sha256:b", entries);
        assert_eq!(
            ratchet(&base("1.2"), &major, &[]),
            Vec::<String>::new(),
            "{what}"
        );
    }
}

#[test]
fn a_compatible_change_needs_a_minor_version() {
    let added = vec![
        entry(
            "layout.a",
            Class::Edit,
            &[("at", "1"), ("net", "2"), ("pad", "3")],
            &["at"],
        ),
        entry("layout.b", Class::Read, &[], &[]),
    ];
    let unchanged = module("1.2", "sha256:b", added.clone());
    assert!(!ratchet(&base("1.2"), &unchanged, &[]).is_empty());
    let minor = module("1.3", "sha256:b", added);
    assert_eq!(ratchet(&base("1.2"), &minor, &[]), Vec::<String>::new());
    let back = module("1.1", "sha256:a", base("1.2").modules[0].entries.clone());
    assert!(!ratchet(&base("1.2"), &back, &[]).is_empty());
}

/// Before 1.0 the minor numbers every break, as in semver.
#[test]
fn before_one_a_break_raises_the_minor() {
    let removed = vec![entry(
        "layout.a",
        Class::Edit,
        &[("at", "1"), ("net", "2")],
        &["at"],
    )];
    let unchanged = module("0.1", "sha256:b", removed.clone());
    assert!(!ratchet(&base("0.1"), &unchanged, &[]).is_empty());
    let minor = module("0.2", "sha256:b", removed);
    assert_eq!(ratchet(&base("0.1"), &minor, &[]), Vec::<String>::new());
}

#[test]
fn a_removed_module_fails() {
    let empty = Lock {
        format: LOCK_FORMAT,
        modules: Vec::new(),
    };
    assert!(!ratchet(&base("0.1"), &empty, &[]).is_empty());
}
