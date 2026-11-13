//! `cargo xtask licences --check`: license gate over the resolved dependency graph.
//!
//! Sparksmith is MIT OR Apache-2.0 and stays independent of GPL-family code. The gate reads
//! `cargo metadata --locked` and fails when:
//! - a workspace member does not declare exactly `MIT OR Apache-2.0`;
//! - a resolved package's SPDX expression cannot be satisfied without a GPL, LGPL or AGPL
//!   license (`X OR GPL-…` passes only when `X` is a reviewed permissive license);
//! - a license id is not in the reviewed list, or a package declares no license expression,
//!   unless that exact package version has a reviewed exception with a reason.
//!
//! Every resolved package is checked (all targets and dependency kinds), so a dev-only or
//! platform-specific dependency cannot slip through.

use crate::{Result, root};
use serde_json::Value;
use std::process::Command;

/// The license every workspace member must declare.
const PROJECT_LICENSE: &str = "MIT OR Apache-2.0";

/// Upper bounds on untrusted metadata, so a malformed manifest cannot exhaust the gate.
const MAX_METADATA_BYTES: usize = 64 * 1024 * 1024;
const MAX_PACKAGES: usize = 20_000;
const MAX_EXPRESSION_BYTES: usize = 512;
const MAX_TOKENS: usize = 128;
const MAX_DEPTH: usize = 16;

/// SPDX ids reviewed as compatible with distributing Sparksmith under MIT OR Apache-2.0.
/// Add an id only after reading its terms; unlisted ids fail the gate.
const PERMITTED: &[&str] = &[
    "0BSD",
    "Apache-2.0",
    "BSD-2-Clause",
    "BSD-3-Clause",
    "BSL-1.0",
    "CC0-1.0",
    // Data licence of the CA root certificates (webpki-roots, used by the
    // assistant's HTTPS client). Permissive; sharing the data needs the
    // agreement text alongside: licenses/webpki-roots-CDLA-Permissive-2.0.txt.
    "CDLA-Permissive-2.0",
    "ISC",
    "MIT",
    // File-level copyleft: unmodified library use is fine. Modified MPL files must stay MPL.
    "MPL-2.0",
    // Font licenses for bundled typefaces; they do not extend to the program code.
    "OFL-1.1",
    "Ubuntu-font-1.0",
    "Unicode-3.0",
    "Unlicense",
    "Zlib",
];

/// Strong copyleft families that Sparksmith never depends on.
const COPYLEFT_FAMILIES: &[&str] = &["GPL", "LGPL", "AGPL"];

/// A reviewed package whose metadata lacks a usable license expression.
struct Exception {
    name: &'static str,
    version: &'static str,
    reason: &'static str,
}

/// Exact package versions accepted after manual review. An entry that matches no resolved
/// package is itself an error, so the list cannot go stale. Currently none are needed.
const EXCEPTIONS: &[Exception] = &[];

pub fn check() -> Result<()> {
    let output = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
        .args(["metadata", "--locked", "--format-version", "1"])
        .current_dir(root()?)
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "cargo metadata failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    if output.stdout.len() > MAX_METADATA_BYTES {
        return Err(format!("cargo metadata exceeds {MAX_METADATA_BYTES} bytes").into());
    }
    let metadata: Value = serde_json::from_slice(&output.stdout)?;
    let report = review(&metadata, EXCEPTIONS)?;
    if !report.problems.is_empty() {
        return Err(format!("license gate failed:\n  {}", report.problems.join("\n  ")).into());
    }
    eprintln!("licences: {} resolved packages pass", report.checked);
    Ok(())
}

#[derive(Debug)]
struct Report {
    checked: usize,
    problems: Vec<String>,
}

/// Review every resolved package in `cargo metadata` output.
fn review(metadata: &Value, exceptions: &[Exception]) -> Result<Report> {
    let packages = metadata
        .get("packages")
        .and_then(Value::as_array)
        .ok_or("metadata has no packages")?;
    if packages.len() > MAX_PACKAGES {
        return Err(format!("metadata lists more than {MAX_PACKAGES} packages").into());
    }
    let members = string_array(metadata.get("workspace_members"), "workspace_members")?;
    let nodes = metadata
        .get("resolve")
        .and_then(|resolve| resolve.get("nodes"))
        .and_then(Value::as_array)
        .ok_or("metadata has no resolved dependency graph")?;
    let mut resolved = Vec::with_capacity(nodes.len());
    for node in nodes {
        resolved.push(
            node.get("id")
                .and_then(Value::as_str)
                .ok_or("resolve node has no id")?,
        );
    }

    let mut problems = Vec::new();
    let mut used = vec![false; exceptions.len()];
    let mut checked = 0usize;
    for package in packages {
        let id = field(package, "id")?;
        if !resolved.contains(&id) {
            continue;
        }
        checked += 1;
        let name = field(package, "name")?;
        let version = field(package, "version")?;
        let license = package.get("license").and_then(Value::as_str);
        if members.contains(&id) {
            if license != Some(PROJECT_LICENSE) {
                problems.push(format!(
                    "workspace member {name} must declare license = \"{PROJECT_LICENSE}\" \
                     (use license.workspace = true); found {license:?}"
                ));
            }
            continue;
        }
        let verdict = match license {
            Some(expression) => evaluate(expression),
            None => Err(Rejection::Missing),
        };
        if let Err(rejection) = verdict {
            let exception = exceptions
                .iter()
                .position(|e| e.name == name && e.version == version);
            match exception {
                // A reviewed exception never waives a GPL-family requirement.
                Some(index) if !matches!(rejection, Rejection::Copyleft { .. }) => {
                    if let Some(flag) = used.get_mut(index) {
                        *flag = true;
                    }
                }
                _ => problems.push(format!("{name} {version}: {rejection}")),
            }
        }
    }
    for (exception, used) in exceptions.iter().zip(&used) {
        if !used {
            problems.push(format!(
                "unused license exception {} {} ({}); remove it",
                exception.name, exception.version, exception.reason
            ));
        }
    }
    Ok(Report { checked, problems })
}

fn field<'a>(package: &'a Value, key: &str) -> Result<&'a str> {
    package
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("package has no {key}").into())
}

fn string_array<'a>(value: Option<&'a Value>, what: &str) -> Result<Vec<&'a str>> {
    let items = value
        .and_then(Value::as_array)
        .ok_or_else(|| format!("metadata has no {what}"))?;
    items
        .iter()
        .map(|item| {
            item.as_str()
                .ok_or_else(|| format!("{what} entry is not a string").into())
        })
        .collect()
}

/// How one license (sub)expression may be satisfied.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Verdict {
    Permitted,
    /// Every way to satisfy it needs this GPL-family license.
    Copyleft(String),
    /// Satisfying it needs a license id nobody has reviewed.
    Unknown(String),
}

impl Verdict {
    /// Either alternative may be chosen: prefer a permitted one.
    fn either(self, other: Self) -> Self {
        match (self, other) {
            (Self::Permitted, _) | (_, Self::Permitted) => Self::Permitted,
            (unknown @ Self::Unknown(_), _) | (_, unknown @ Self::Unknown(_)) => unknown,
            (copyleft, _) => copyleft,
        }
    }

    /// Both terms apply: any copyleft term binds.
    fn both(self, other: Self) -> Self {
        match (self, other) {
            (copyleft @ Self::Copyleft(_), _) | (_, copyleft @ Self::Copyleft(_)) => copyleft,
            (unknown @ Self::Unknown(_), _) | (_, unknown @ Self::Unknown(_)) => unknown,
            _ => Self::Permitted,
        }
    }
}

/// Why a package's license metadata fails the gate.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Rejection {
    Missing,
    Malformed { expression: String, error: String },
    Copyleft { expression: String, id: String },
    Unreviewed { expression: String, id: String },
}

impl std::fmt::Display for Rejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing => write!(f, "declares no SPDX license expression"),
            Self::Malformed { expression, error } => {
                write!(f, "malformed license {expression:?}: {error}")
            }
            Self::Copyleft { expression, id } => write!(
                f,
                "license {expression:?} cannot be satisfied without {id} (GPL family)"
            ),
            Self::Unreviewed { expression, id } => write!(
                f,
                "license {expression:?} needs unreviewed id {id}; review its terms first"
            ),
        }
    }
}

/// Decide whether `expression` can be satisfied with reviewed non-GPL-family licenses.
fn evaluate(expression: &str) -> std::result::Result<(), Rejection> {
    let verdict = parse(expression).map_err(|error| Rejection::Malformed {
        expression: expression.to_owned(),
        error,
    })?;
    let expression = expression.to_owned();
    match verdict {
        Verdict::Permitted => Ok(()),
        Verdict::Copyleft(id) => Err(Rejection::Copyleft { expression, id }),
        Verdict::Unknown(id) => Err(Rejection::Unreviewed { expression, id }),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Token<'a> {
    Open,
    Close,
    Or,
    And,
    With,
    Id(&'a str),
}

fn tokenize(expression: &str) -> std::result::Result<Vec<Token<'_>>, String> {
    if expression.len() > MAX_EXPRESSION_BYTES {
        return Err(format!("longer than {MAX_EXPRESSION_BYTES} bytes"));
    }
    let mut tokens = Vec::new();
    let mut rest = expression;
    loop {
        rest = rest.trim_start();
        let Some(first) = rest.chars().next() else {
            break;
        };
        if tokens.len() >= MAX_TOKENS {
            return Err(format!("more than {MAX_TOKENS} tokens"));
        }
        let (token, length) = match first {
            '(' => (Token::Open, 1),
            ')' => (Token::Close, 1),
            // Legacy crates.io form `MIT/Apache-2.0` means a choice.
            '/' => (Token::Or, 1),
            _ => {
                let length = rest
                    .find(|c: char| c.is_whitespace() || matches!(c, '(' | ')' | '/'))
                    .unwrap_or(rest.len());
                let word = rest.get(..length).ok_or("invalid character boundary")?;
                let token = match word {
                    "OR" => Token::Or,
                    "AND" => Token::And,
                    "WITH" => Token::With,
                    _ if word
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || "-.+".contains(c)) =>
                    {
                        Token::Id(word)
                    }
                    _ => return Err(format!("invalid license id {word:?}")),
                };
                (token, length)
            }
        };
        tokens.push(token);
        rest = rest.get(length..).ok_or("invalid character boundary")?;
    }
    Ok(tokens)
}

/// Recursive-descent evaluation of an SPDX license expression. `WITH` exceptions only add
/// permissions, so a `license WITH exception` term is judged by its license.
fn parse(expression: &str) -> std::result::Result<Verdict, String> {
    let tokens = tokenize(expression)?;
    let mut parser = Parser {
        tokens: &tokens,
        next: 0,
    };
    let verdict = parser.or(0)?;
    match parser.peek() {
        None => Ok(verdict),
        Some(token) => Err(format!("unexpected {token:?}")),
    }
}

struct Parser<'t, 'a> {
    tokens: &'t [Token<'a>],
    next: usize,
}

impl<'a> Parser<'_, 'a> {
    fn peek(&self) -> Option<Token<'a>> {
        self.tokens.get(self.next).copied()
    }

    fn take(&mut self) -> Option<Token<'a>> {
        let token = self.peek();
        if token.is_some() {
            self.next += 1;
        }
        token
    }

    fn or(&mut self, depth: usize) -> std::result::Result<Verdict, String> {
        let mut verdict = self.and(depth)?;
        while self.peek() == Some(Token::Or) {
            self.take();
            verdict = verdict.either(self.and(depth)?);
        }
        Ok(verdict)
    }

    fn and(&mut self, depth: usize) -> std::result::Result<Verdict, String> {
        let mut verdict = self.term(depth)?;
        while self.peek() == Some(Token::And) {
            self.take();
            verdict = verdict.both(self.term(depth)?);
        }
        Ok(verdict)
    }

    fn term(&mut self, depth: usize) -> std::result::Result<Verdict, String> {
        let verdict = match self.take() {
            Some(Token::Open) => {
                if depth >= MAX_DEPTH {
                    return Err(format!("nested deeper than {MAX_DEPTH}"));
                }
                let inner = self.or(depth + 1)?;
                if self.take() != Some(Token::Close) {
                    return Err("missing ')'".to_owned());
                }
                inner
            }
            Some(Token::Id(id)) => classify(id),
            Some(token) => return Err(format!("expected a license, found {token:?}")),
            None => return Err("expected a license, found the end".to_owned()),
        };
        if self.peek() == Some(Token::With) {
            self.take();
            match self.take() {
                Some(Token::Id(_)) => {}
                _ => return Err("expected an exception id after WITH".to_owned()),
            }
        }
        Ok(verdict)
    }
}

fn classify(id: &str) -> Verdict {
    // `MPL-2.0+` and `GPL-2.0+` are the deprecated "or later" spellings.
    let base = id.strip_suffix('+').unwrap_or(id);
    let family = base.split('-').next().unwrap_or(base);
    if COPYLEFT_FAMILIES.contains(&family) {
        Verdict::Copyleft(id.to_owned())
    } else if PERMITTED.contains(&base) {
        Verdict::Permitted
    } else {
        Verdict::Unknown(id.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn permissive_expressions_pass() {
        for expression in [
            "MIT",
            "MIT OR Apache-2.0",
            "MIT/Apache-2.0",
            "Apache-2.0 / MIT",
            "Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT",
            "(MIT OR Apache-2.0) AND Unicode-3.0",
            "(MIT OR Apache-2.0) AND OFL-1.1 AND Ubuntu-font-1.0",
            "MPL-2.0+",
        ] {
            assert_eq!(evaluate(expression), Ok(()), "{expression}");
        }
    }

    #[test]
    fn gpl_family_passes_only_with_a_permissive_alternative() {
        for expression in [
            "Apache-2.0 OR GPL-2.0-only",
            "MIT OR Apache-2.0 OR LGPL-2.1-or-later",
            "(GPL-3.0-only OR MIT) AND Apache-2.0",
        ] {
            assert_eq!(evaluate(expression), Ok(()), "{expression}");
        }
        for expression in [
            "GPL-3.0-only",
            "GPL-2.0+",
            "LGPL-2.1-or-later",
            "AGPL-3.0-or-later",
            "MIT AND GPL-2.0-only",
            "(MIT OR Apache-2.0) AND LGPL-3.0-only",
            "GPL-2.0-only WITH Classpath-exception-2.0",
        ] {
            assert!(
                matches!(evaluate(expression), Err(Rejection::Copyleft { .. })),
                "{expression}"
            );
        }
    }

    #[test]
    fn unreviewed_and_malformed_expressions_fail() {
        for expression in [
            "LicenseRef-Proprietary",
            "CC-BY-SA-4.0",
            "SomethingNew OR GPL-3.0-only",
            "mit",
        ] {
            assert!(
                matches!(evaluate(expression), Err(Rejection::Unreviewed { .. })),
                "{expression}"
            );
        }
        let deep = format!("{}MIT{}", "(".repeat(40), ")".repeat(40));
        let long = vec!["MIT"; 200].join(" OR ");
        for expression in [
            "",
            "MIT OR",
            "AND MIT",
            "(MIT",
            "MIT)",
            "MIT WITH",
            "MIT Apache-2.0",
            "MIT, Apache-2.0",
            deep.as_str(),
            long.as_str(),
        ] {
            assert!(
                matches!(evaluate(expression), Err(Rejection::Malformed { .. })),
                "{expression}"
            );
        }
    }

    fn package(id: &str, name: &str, license: Option<&str>) -> Value {
        json!({ "id": id, "name": name, "version": "1.0.0", "license": license })
    }

    fn metadata(packages: Vec<Value>, members: &[&str], resolved: &[&str]) -> Value {
        let nodes: Vec<Value> = resolved.iter().map(|id| json!({ "id": id })).collect();
        json!({ "packages": packages, "workspace_members": members, "resolve": { "nodes": nodes } })
    }

    #[test]
    fn workspace_members_must_use_the_project_license() -> Result<()> {
        let data = metadata(
            vec![
                package("a", "sparksmith-a", Some("MIT OR Apache-2.0")),
                package("b", "sparksmith-b", None),
                package("c", "sparksmith-c", Some("MIT")),
            ],
            &["a", "b", "c"],
            &["a", "b", "c"],
        );
        let report = review(&data, &[])?;
        assert_eq!(report.checked, 3);
        assert_eq!(report.problems.len(), 2, "{:?}", report.problems);
        assert!(
            report
                .problems
                .iter()
                .all(|p| p.contains("workspace member"))
        );
        Ok(())
    }

    #[test]
    fn dependencies_are_checked_only_when_resolved() -> Result<()> {
        let data = metadata(
            vec![
                package("ok", "fine", Some("MIT")),
                package("gpl", "copyleft", Some("GPL-3.0-only")),
                package("none", "unlabelled", None),
                package("unused", "not-resolved", Some("AGPL-3.0-only")),
            ],
            &[],
            &["ok", "gpl", "none"],
        );
        let report = review(&data, &[])?;
        assert_eq!(report.checked, 3);
        assert_eq!(report.problems.len(), 2, "{:?}", report.problems);
        assert!(
            report
                .problems
                .iter()
                .any(|p| p.starts_with("copyleft 1.0.0"))
        );
        assert!(
            report
                .problems
                .iter()
                .any(|p| p.starts_with("unlabelled 1.0.0"))
        );
        Ok(())
    }

    #[test]
    fn exceptions_cover_unlabelled_packages_but_never_gpl_or_stale_entries() -> Result<()> {
        let data = metadata(
            vec![
                package("none", "unlabelled", None),
                package("gpl", "copyleft", Some("GPL-3.0-only")),
            ],
            &[],
            &["none", "gpl"],
        );
        let exceptions = [
            Exception {
                name: "unlabelled",
                version: "1.0.0",
                reason: "license file reviewed: MIT",
            },
            Exception {
                name: "copyleft",
                version: "1.0.0",
                reason: "attempted waiver",
            },
            Exception {
                name: "gone",
                version: "0.1.0",
                reason: "no longer resolved",
            },
        ];
        let report = review(&data, &exceptions)?;
        assert_eq!(report.problems.len(), 3, "{:?}", report.problems);
        assert!(
            report
                .problems
                .iter()
                .any(|p| p.starts_with("copyleft 1.0.0"))
        );
        assert!(
            report
                .problems
                .iter()
                .any(|p| p.contains("copyleft 1.0.0 (attempted"))
        );
        assert!(report.problems.iter().any(|p| p.contains("gone 0.1.0")));
        assert!(!report.problems.iter().any(|p| p.starts_with("unlabelled")));
        Ok(())
    }

    #[test]
    fn malformed_metadata_is_an_error() {
        assert!(review(&json!({}), &[]).is_err());
        assert!(review(&json!({ "packages": [], "workspace_members": [] }), &[]).is_err());
    }
}
