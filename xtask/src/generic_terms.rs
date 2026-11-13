//! `cargo xtask generic-terms`: the code names generic concepts, never the
//! product a founder gave as an example.
//!
//! Another design references a board only through its published interface
//! (docs/architecture/board-interface.md): a linked design, through an
//! assembly link. The code, commands, findings, tests and file names under
//! `crates/`, `apps/` and `xtask/` never name one particular kind of linked
//! design or its walls. The words are listed below, in two parts so this
//! file does not match itself, with no allow-list: an example use case may
//! appear in prose under `docs/` only. The gate lists tracked and
//! untracked-but-not-ignored files, and checks each path and each text
//! file's contents case-insensitively.
use crate::{Result, root};
use std::process::Command;

/// Product-specific words, each spelled in two parts.
const WORDS: &[(&str, &str)] = &[("hous", "ing"), ("enclo", "sure")];

/// The folders the check covers; `docs/` is not one of them.
const FOLDERS: &[&str] = &["crates/", "apps/", "xtask/"];

/// Files larger than this are not text the check reads.
const MAX_BYTES: u64 = 16 * 1024 * 1024;
const MAX_FILES: usize = 100_000;
/// Violations listed before the rest are counted.
const MAX_LISTED: usize = 200;

pub fn check() -> Result<()> {
    let root = root()?;
    let listed = Command::new("git")
        .args([
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
            "--",
        ])
        .args(FOLDERS)
        .current_dir(&root)
        .output()?;
    if !listed.status.success() {
        return Err(String::from_utf8_lossy(&listed.stderr).into_owned().into());
    }
    let words = words();
    let mut violations = Vec::new();
    let mut count = 0usize;
    for raw in listed.stdout.split(|b| *b == 0).filter(|p| !p.is_empty()) {
        count += 1;
        if count > MAX_FILES {
            return Err("generic-terms: too many files to check".into());
        }
        let path = String::from_utf8_lossy(raw).into_owned();
        if let Some(word) = found(&path, &words) {
            violations.push(format!("{path}: the path names `{word}`"));
        }
        let full = root.join(&path);
        let Ok(meta) = std::fs::metadata(&full) else {
            continue;
        };
        if !meta.is_file() || meta.len() > MAX_BYTES {
            continue;
        }
        let bytes = std::fs::read(&full)?;
        let Ok(text) = std::str::from_utf8(&bytes) else {
            continue;
        };
        for (line, word) in lines(text, &words) {
            violations.push(format!("{path}:{line}: names `{word}`"));
        }
    }
    if violations.is_empty() {
        println!("generic-terms: {count} files name no product-specific concept");
        return Ok(());
    }
    let total = violations.len();
    violations.truncate(MAX_LISTED);
    if total > MAX_LISTED {
        violations.push(format!("… and {} more", total - MAX_LISTED));
    }
    Err(format!(
        "product-specific words in code (xtask/src/generic_terms.rs); name the generic concept, a linked design or an assembly link:\n  {}",
        violations.join("\n  ")
    )
    .into())
}

fn words() -> Vec<String> {
    WORDS.iter().map(|(a, b)| format!("{a}{b}")).collect()
}

/// The first word `text` contains, case-insensitively.
fn found<'a>(text: &str, words: &'a [String]) -> Option<&'a str> {
    let lower = text.to_lowercase();
    words
        .iter()
        .find(|w| lower.contains(w.as_str()))
        .map(String::as_str)
}

/// Each line (counted from 1) of `text` that contains a word, with it.
fn lines<'a>(text: &str, words: &'a [String]) -> Vec<(usize, &'a str)> {
    text.lines()
        .enumerate()
        .filter_map(|(i, line)| found(line, words).map(|w| (i + 1, w)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_product_word_in_code_or_a_path_is_found_in_any_case() {
        let words = words();
        let first = format!("{}{}", WORDS[0].0, WORDS[0].1);
        let second = format!("{}{}", WORDS[1].0, WORDS[1].1).to_uppercase();
        let text =
            format!("pub struct AssemblyLink;\nfn {first}_update() {{}}\n// the {second} wall\n");
        assert_eq!(
            lines(&text, &words),
            vec![(2, first.as_str()), (3, words[1].as_str())]
        );
        assert_eq!(
            found(&format!("crates/io/src/{first}.rs"), &words),
            Some(first.as_str())
        );
        assert_eq!(found("crates/io/src/assembly_link.rs", &words), None);
    }

    #[test]
    fn the_generic_names_pass() {
        let words = words();
        let text = "pub mod assembly_link;\n/// A linked design follows the board.\n\
                    const ASSEMBLY_STALE: &str = \"assembly.stale\";\n";
        assert!(lines(text, &words).is_empty());
    }
}
