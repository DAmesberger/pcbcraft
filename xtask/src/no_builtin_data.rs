//! `cargo xtask no-builtin-data` (alias `no-builtin-parts`): no library
//! data compiled into the program (`docs/shipped-library.md`,
//! `docs/generator-plugins.md`).
//!
//! Parts, profiles, standards and every other kind of library data ship as
//! files in `library/` (generator modules among them) and are read at run
//! time. This gate parses every Rust file of `apps/` and `crates/` with
//! `syn` and fails on an `include_str!` or `include_bytes!` in non-test
//! code that [`ALLOWED`] does not list. Test code is skipped: items under a
//! test-only `#[cfg]` (`test`, or the `test-fixtures` feature), `#[test]`
//! functions, files a test-only `mod` declares, and `tests/` and `benches/`
//! folders. A macro inside another macro's arguments counts too.
//!
//! The allowlist holds the program's own assets (fonts, shaders, icons) and
//! the MCP skill prompt, which belong to the program rather than to the
//! library, and the sites that are still being moved to `library/`. An
//! entry that no longer matches a site fails, so it is removed with the
//! data it allowed.
use crate::{Result, root};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use syn::visit::Visit;

/// Why an embedded file is allowed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Reason {
    /// The program's own interface assets: fonts, shaders, icons.
    UiAsset,
    /// The MCP prompt served from a skill.
    SkillPrompt,
    /// In-code templates the generator modules replace in slice GP5.
    UntilGp5,
}

/// `(source file, included path suffix, reason)`.
const ALLOWED: &[(&str, &str, Reason)] = &[
    // UI assets.
    (
        "crates/ui/src/font_setup.rs",
        "IBMPlexSans-Variable.ttf",
        Reason::UiAsset,
    ),
    (
        "crates/ui/src/font_setup.rs",
        "IBMPlexMono-Regular.ttf",
        Reason::UiAsset,
    ),
    (
        "crates/ui/src/kit/blur_native.rs",
        "blur.wgsl",
        Reason::UiAsset,
    ),
    // The MCP skill prompt.
    (
        "crates/automation/src/mcp/prompts.rs",
        "skills/sparksmith-datasheet-part/SKILL.md",
        Reason::SkillPrompt,
    ),
    // Until GP5: the starter package rows, which generator modules replace.
    (
        "crates/editor-core/src/library/templates/starter.rs",
        "starter/chip.json",
        Reason::UntilGp5,
    ),
    (
        "crates/editor-core/src/library/templates/starter.rs",
        "starter/gull_wing.json",
        Reason::UntilGp5,
    ),
    (
        "crates/editor-core/src/library/templates/starter.rs",
        "starter/no_lead.json",
        Reason::UntilGp5,
    ),
    (
        "crates/editor-core/src/library/templates/starter.rs",
        "starter/header.json",
        Reason::UntilGp5,
    ),
    (
        "crates/editor-core/src/library/templates/starter.rs",
        "starter/tabbed.json",
        Reason::UntilGp5,
    ),
    (
        "crates/editor-core/src/library/templates/starter.rs",
        "starter/molded.json",
        Reason::UntilGp5,
    ),
    (
        "crates/editor-core/src/library/templates/starter.rs",
        "starter/grid.json",
        Reason::UntilGp5,
    ),
    (
        "crates/editor-core/src/library/templates/starter.rs",
        "starter/tht.json",
        Reason::UntilGp5,
    ),
    (
        "crates/editor-core/src/library/templates/starter.rs",
        "starter/usb_c.json",
        Reason::UntilGp5,
    ),
];

/// The folders scanned: the program. Generator crates (`generators/`) are
/// the modules themselves and carry their own data; `xtask` is tooling.
const SCANNED: &[&str] = &["apps", "crates"];

/// One embedded file in non-test code.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Site {
    /// Relative to the scanned root, with `/`.
    file: String,
    line: usize,
    /// The string literals of the macro's arguments, joined.
    path: String,
}

pub fn check() -> Result<()> {
    let root = root()?;
    let sites = scan(&root, SCANNED)?;
    let report = review(&sites, ALLOWED);
    for line in &report.allowed {
        eprintln!("allowed: {line}");
    }
    if report.problems.is_empty() {
        eprintln!(
            "no-builtin-data: no library data compiled in ({} allowed embeds)",
            report.allowed.len()
        );
        return Ok(());
    }
    Err(format!(
        "library data is compiled into the program:\n  {}\nShip it as a file in library/ and read it through sparksmith_io::shipped (docs/shipped-library.md).",
        report.problems.join("\n  ")
    )
    .into())
}

struct Report {
    problems: Vec<String>,
    allowed: Vec<String>,
}

fn review(sites: &[Site], allowed: &[(&str, &str, Reason)]) -> Report {
    let mut used = BTreeSet::new();
    let mut problems = Vec::new();
    let mut ok = Vec::new();
    for site in sites {
        let entry = allowed
            .iter()
            .enumerate()
            .find(|(_, (file, suffix, _))| site.file == *file && site.path.ends_with(suffix));
        match entry {
            Some((index, (_, _, reason))) => {
                used.insert(index);
                ok.push(format!(
                    "{}:{} {} ({reason:?})",
                    site.file, site.line, site.path
                ));
            }
            None => problems.push(format!(
                "{}:{} embeds {:?} in non-test code",
                site.file, site.line, site.path
            )),
        }
    }
    for (index, (file, suffix, _)) in allowed.iter().enumerate() {
        if !used.contains(&index) {
            problems.push(format!(
                "the allowlist entry {file} → {suffix} matches nothing; remove it from xtask/src/no_builtin_data.rs"
            ));
        }
    }
    Report {
        problems,
        allowed: ok,
    }
}

/// Every embed in non-test code under `folders` of `root`.
fn scan(root: &Path, folders: &[&str]) -> Result<Vec<Site>> {
    let mut files = Vec::new();
    for folder in folders {
        collect(&root.join(folder), &mut files)?;
    }
    files.sort();
    let mut parsed = Vec::new();
    for path in &files {
        let text = std::fs::read_to_string(path)?;
        let file = syn::parse_file(&text)
            .map_err(|e| format!("{}: not Rust syn can read: {e}", path.display()))?;
        parsed.push((path.clone(), file));
    }
    // Files a test-only `mod` declares, and everything they declare, to a
    // fixed point.
    let mut test_files: BTreeSet<PathBuf> = files
        .iter()
        .filter(|p| in_test_folder(root, p))
        .cloned()
        .collect();
    loop {
        let before = test_files.len();
        for (path, file) in &parsed {
            let whole = test_files.contains(path);
            for (child, test) in declared_modules(path, file) {
                if (whole || test) && child.is_file() {
                    test_files.insert(child);
                }
            }
        }
        if test_files.len() == before {
            break;
        }
    }
    let mut sites = Vec::new();
    for (path, file) in &parsed {
        if test_files.contains(path) {
            continue;
        }
        let relative = path
            .strip_prefix(root)
            .unwrap_or(path)
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/");
        let mut finder = Finder::default();
        finder.visit_file(file);
        for (line, path) in finder.found {
            sites.push(Site {
                file: relative.clone(),
                line,
                path,
            });
        }
    }
    sites.sort();
    Ok(sites)
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if entry.file_type()?.is_dir() {
            if name == "target" || name.starts_with('.') {
                continue;
            }
            collect(&path, out)?;
        } else if name.ends_with(".rs") {
            out.push(path);
        }
    }
    Ok(())
}

/// Whether `path` lies in a crate's `tests/` or `benches/` folder.
fn in_test_folder(root: &Path, path: &Path) -> bool {
    path.strip_prefix(root).is_ok_and(|p| {
        p.components()
            .any(|c| matches!(c.as_os_str().to_str(), Some("tests" | "benches")))
    })
}

/// The files `file` (at `path`) declares with `mod name;`, each with
/// whether its declaration is test-only.
fn declared_modules(path: &Path, file: &syn::File) -> Vec<(PathBuf, bool)> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    // `mod.rs`, `lib.rs`, `main.rs` and `build.rs` own their folder; any
    // other file's children live in a folder of its name.
    let owns_dir = matches!(stem, "mod" | "lib" | "main" | "build");
    let base = if owns_dir {
        dir.to_path_buf()
    } else {
        dir.join(stem)
    };
    let mut out = Vec::new();
    for item in &file.items {
        let syn::Item::Mod(module) = item else {
            continue;
        };
        if module.content.is_some() {
            continue;
        }
        let test = test_only(&module.attrs);
        let explicit = module.attrs.iter().find_map(|a| {
            if !a.path().is_ident("path") {
                return None;
            }
            match &a.meta {
                syn::Meta::NameValue(nv) => match &nv.value {
                    syn::Expr::Lit(syn::ExprLit {
                        lit: syn::Lit::Str(s),
                        ..
                    }) => Some(s.value()),
                    _ => None,
                },
                _ => None,
            }
        });
        match explicit {
            Some(relative) => out.push((dir.join(relative), test)),
            None => {
                let name = module.ident.to_string();
                out.push((base.join(format!("{name}.rs")), test));
                out.push((base.join(&name).join("mod.rs"), test));
            }
        }
    }
    out
}

/// Whether `attrs` make an item test-only: `#[test]`, or a `#[cfg]` that
/// holds only under `test` or the `test-fixtures` feature.
fn test_only(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|a| {
        a.path().is_ident("test")
            || (a.path().is_ident("cfg")
                && a.parse_args::<syn::Meta>().is_ok_and(|m| cfg_test_only(&m)))
    })
}

fn cfg_test_only(meta: &syn::Meta) -> bool {
    match meta {
        syn::Meta::Path(p) => p.is_ident("test"),
        syn::Meta::NameValue(nv) => {
            nv.path.is_ident("feature")
                && matches!(&nv.value, syn::Expr::Lit(syn::ExprLit { lit: syn::Lit::Str(s), .. }) if s.value() == "test-fixtures")
        }
        syn::Meta::List(list) => {
            let parsed = list.parse_args_with(
                syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated,
            );
            let Ok(items) = parsed else {
                return false;
            };
            if list.path.is_ident("any") {
                !items.is_empty() && items.iter().all(cfg_test_only)
            } else if list.path.is_ident("all") {
                items.iter().any(cfg_test_only)
            } else {
                false
            }
        }
    }
}

/// Embeds in a file, skipping test-only items.
#[derive(Default)]
struct Finder {
    found: Vec<(usize, String)>,
}

impl Finder {
    fn tokens(&mut self, tokens: proc_macro2::TokenStream) {
        let trees: Vec<proc_macro2::TokenTree> = tokens.into_iter().collect();
        for (index, tree) in trees.iter().enumerate() {
            match tree {
                proc_macro2::TokenTree::Ident(ident)
                    if (ident == "include_str" || ident == "include_bytes")
                        && matches!(trees.get(index + 1), Some(proc_macro2::TokenTree::Punct(p)) if p.as_char() == '!') =>
                {
                    let arguments = match trees.get(index + 2) {
                        Some(proc_macro2::TokenTree::Group(g)) => g.stream(),
                        _ => proc_macro2::TokenStream::new(),
                    };
                    let mut literals = Vec::new();
                    strings(arguments, &mut literals);
                    self.found
                        .push((ident.span().start().line, literals.concat()));
                }
                proc_macro2::TokenTree::Group(group) => self.tokens(group.stream()),
                _ => {}
            }
        }
    }
}

/// The string literals in `tokens`, outside `env!` names.
fn strings(tokens: proc_macro2::TokenStream, out: &mut Vec<String>) {
    for tree in tokens {
        match tree {
            proc_macro2::TokenTree::Literal(literal) => {
                if let Ok(syn::Lit::Str(s)) = syn::parse_str::<syn::Lit>(&literal.to_string())
                    && s.value() != "CARGO_MANIFEST_DIR"
                {
                    out.push(s.value());
                }
            }
            proc_macro2::TokenTree::Group(group) => strings(group.stream(), out),
            _ => {}
        }
    }
}

impl<'ast> Visit<'ast> for Finder {
    fn visit_item(&mut self, item: &'ast syn::Item) {
        let attrs: &[syn::Attribute] = match item {
            syn::Item::Const(i) => &i.attrs,
            syn::Item::Enum(i) => &i.attrs,
            syn::Item::Fn(i) => &i.attrs,
            syn::Item::Impl(i) => &i.attrs,
            syn::Item::Macro(i) => &i.attrs,
            syn::Item::Mod(i) => &i.attrs,
            syn::Item::Static(i) => &i.attrs,
            syn::Item::Struct(i) => &i.attrs,
            syn::Item::Trait(i) => &i.attrs,
            syn::Item::Use(i) => &i.attrs,
            _ => &[],
        };
        if !test_only(attrs) {
            syn::visit::visit_item(self, item);
        }
    }

    fn visit_impl_item_fn(&mut self, item: &'ast syn::ImplItemFn) {
        if !test_only(&item.attrs) {
            syn::visit::visit_impl_item_fn(self, item);
        }
    }

    fn visit_impl_item_const(&mut self, item: &'ast syn::ImplItemConst) {
        if !test_only(&item.attrs) {
            syn::visit::visit_impl_item_const(self, item);
        }
    }

    fn visit_stmt(&mut self, stmt: &'ast syn::Stmt) {
        if let syn::Stmt::Local(local) = stmt
            && test_only(&local.attrs)
        {
            return;
        }
        syn::visit::visit_stmt(self, stmt);
    }

    fn visit_macro(&mut self, mac: &'ast syn::Macro) {
        // The macro itself, then everything inside its arguments.
        let mut stream = proc_macro2::TokenStream::new();
        stream.extend(
            mac.path
                .segments
                .last()
                .map(|s| proc_macro2::TokenTree::Ident(s.ident.clone())),
        );
        stream.extend([proc_macro2::TokenTree::Punct(proc_macro2::Punct::new(
            '!',
            proc_macro2::Spacing::Alone,
        ))]);
        stream.extend([proc_macro2::TokenTree::Group(proc_macro2::Group::new(
            proc_macro2::Delimiter::Parenthesis,
            mac.tokens.clone(),
        ))]);
        self.tokens(stream);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(files: &[(&str, &str)]) -> Result<tempfile::TempDir> {
        let dir = tempfile::tempdir()?;
        for (path, text) in files {
            let path = dir.path().join(path);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(path, text)?;
        }
        Ok(dir)
    }

    fn found(files: &[(&str, &str)]) -> Result<Vec<Site>> {
        let dir = tree(files)?;
        scan(dir.path(), &["crates"])
    }

    #[test]
    fn a_planted_part_file_in_program_code_fails() -> Result<()> {
        let sites = found(&[
            (
                "crates/core/src/lib.rs",
                "pub mod parts;\npub const ROWS: &str = include_str!(\"rows/chip.json\");\n",
            ),
            (
                "crates/core/src/parts.rs",
                "pub fn part() -> &'static [u8] { include_bytes!(concat!(env!(\"CARGO_MANIFEST_DIR\"), \"/data/r0603.solidcraft\")) }\n",
            ),
        ])?;
        let report = review(&sites, &[]);
        assert_eq!(report.problems.len(), 2, "{:?}", report.problems);
        assert!(report.problems[0].contains("crates/core/src/lib.rs:2"));
        assert!(report.problems[0].contains("rows/chip.json"));
        assert!(report.problems[1].contains("/data/r0603.solidcraft"));
        Ok(())
    }

    #[test]
    fn an_embed_inside_another_macro_is_found() -> Result<()> {
        let sites = found(&[(
            "crates/core/src/lib.rs",
            "pub fn rows() -> Vec<&'static str> { vec![include_str!(\"a.json\"), include_str!(\"b.json\")] }\n",
        )])?;
        let paths: Vec<&str> = sites.iter().map(|s| s.path.as_str()).collect();
        assert_eq!(paths, ["a.json", "b.json"]);
        Ok(())
    }

    #[test]
    fn test_code_is_skipped() -> Result<()> {
        let sites = found(&[
            (
                "crates/core/src/lib.rs",
                concat!(
                    "#[cfg(test)]\nmod tests;\n",
                    "#[cfg(any(test, feature = \"test-fixtures\"))]\npub mod fixtures;\n",
                    "#[cfg(test)]\n#[path = \"shared_tests.rs\"]\nmod shared;\n",
                    "#[cfg(test)]\nconst A: &str = include_str!(\"a.json\");\n",
                    "#[test]\nfn t() { let _ = include_bytes!(\"b.bin\"); }\n",
                    "#[cfg(any(test, feature = \"other\"))]\nconst C: &str = include_str!(\"c.json\");\n",
                ),
            ),
            (
                "crates/core/src/tests.rs",
                "const D: &str = include_str!(\"d.json\");\nmod deeper;\n",
            ),
            (
                "crates/core/src/tests/deeper.rs",
                "const E: &str = include_str!(\"e.json\");\n",
            ),
            (
                "crates/core/src/fixtures.rs",
                "const F: &str = include_str!(\"f.json\");\n",
            ),
            (
                "crates/core/src/shared_tests.rs",
                "const G: &str = include_str!(\"g.json\");\n",
            ),
            (
                "crates/core/tests/it.rs",
                "const H: &str = include_str!(\"h.json\");\n",
            ),
        ])?;
        // Only `c.json` is in code a non-test build may compile.
        let paths: Vec<&str> = sites.iter().map(|s| s.path.as_str()).collect();
        assert_eq!(paths, ["c.json"]);
        Ok(())
    }

    #[test]
    fn allowed_embeds_pass_and_stale_entries_fail() -> Result<()> {
        let sites = found(&[(
            "crates/ui/src/fonts.rs",
            "const F: &[u8] = include_bytes!(\"fonts/Plex.ttf\");\n",
        )])?;
        let allowed = [
            ("crates/ui/src/fonts.rs", "fonts/Plex.ttf", Reason::UiAsset),
            ("crates/ui/src/gone.rs", "old.json", Reason::UntilGp5),
        ];
        let report = review(&sites, &allowed);
        assert_eq!(report.allowed.len(), 1);
        assert_eq!(report.problems.len(), 1, "{:?}", report.problems);
        assert!(report.problems[0].contains("matches nothing"));
        Ok(())
    }
}
