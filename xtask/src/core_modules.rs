//! `cargo xtask core-modules`: the intra-crate module map of `pcbcraft-core`.
//!
//! `docs/component-model.md` ("Module map") gives every core module the complete set of
//! `crate::` modules it may use. This gate parses each module's non-test sources with `syn`,
//! collects every `crate::…` path and `use crate::…` tree, resolves crate-root re-exports to
//! the module that defines them, and fails on any edge outside the map. Keep the table below
//! and the document in step; the gate is the enforcement.
use crate::{Result, root};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use syn::visit::Visit;

/// (module, modules it may use). Order is the document's numbering, which is acyclic.
const MAP: &[(&str, &[&str])] = &[
    ("geometry", &[]),
    ("ids", &[]),
    ("diagnostics", &["ids"]),
    ("quantity", &["diagnostics"]),
    ("fields", &["ids", "diagnostics", "quantity"]),
    ("functions", &["ids", "diagnostics"]),
    (
        "library",
        &[
            "geometry",
            "ids",
            "diagnostics",
            "quantity",
            "fields",
            "functions",
        ],
    ),
    (
        "model",
        &[
            "geometry",
            "ids",
            "diagnostics",
            "library",
            "fields",
            "functions",
        ],
    ),
    (
        "instances",
        &[
            "geometry",
            "ids",
            "diagnostics",
            "library",
            "model",
            "fields",
            "functions",
        ],
    ),
    (
        "nets",
        &["ids", "diagnostics", "library", "model", "instances"],
    ),
    (
        "schematic",
        &[
            "geometry",
            "ids",
            "diagnostics",
            "library",
            "model",
            "instances",
            "nets",
        ],
    ),
    (
        "layout",
        &[
            "geometry",
            "ids",
            "diagnostics",
            "library",
            "model",
            "instances",
        ],
    ),
    (
        "erc",
        &[
            "ids",
            "diagnostics",
            "library",
            "model",
            "instances",
            "functions",
            "nets",
        ],
    ),
    (
        "completeness",
        &[
            "ids",
            "diagnostics",
            "library",
            "model",
            "instances",
            "fields",
            "functions",
            "nets",
        ],
    ),
    ("scripting", &["ids", "diagnostics", "quantity", "library"]),
    (
        "validate",
        &[
            "ids",
            "diagnostics",
            "library",
            "model",
            "instances",
            "nets",
            "schematic",
            "layout",
        ],
    ),
    ("updates", BELOW_UPDATES),
    ("commands", BELOW_COMMANDS),
    ("format", &["diagnostics", "model", "library", "validate"]),
];

const BELOW_UPDATES: &[&str] = &[
    "geometry",
    "ids",
    "diagnostics",
    "quantity",
    "fields",
    "functions",
    "library",
    "model",
    "instances",
    "nets",
    "schematic",
    "layout",
    "erc",
    "completeness",
    "scripting",
    "validate",
];

const BELOW_COMMANDS: &[&str] = &[
    "geometry",
    "ids",
    "diagnostics",
    "quantity",
    "fields",
    "functions",
    "library",
    "model",
    "instances",
    "nets",
    "schematic",
    "layout",
    "erc",
    "completeness",
    "scripting",
    "validate",
    "updates",
];

/// Source files a project module may span, and the module tree's size bound.
const MAX_FILES: usize = 512;
const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;

/// The crate root's own items (`Engine`, `Operation`, `Query`, …) belong to no module, so a
/// module that names them depends upward on the engine.
const ROOT: &str = "crate root";

pub fn check() -> Result<()> {
    let src = root()?.join("crates/editor-core/src");
    let reexports = reexports(&src)?;
    let modules: BTreeSet<&str> = MAP.iter().map(|(m, _)| *m).collect();
    let declared = declared_modules(&src)?;
    let mut violations = Vec::new();
    for module in &declared {
        if !modules.contains(module.as_str()) {
            violations.push(format!(
                "core module `{module}` is not in the module map (xtask/src/core_modules.rs and docs/component-model.md)"
            ));
        }
    }
    for (module, allowed) in MAP {
        let allowed: BTreeSet<&str> = allowed.iter().copied().collect();
        for file in module_files(&src, module)? {
            let text = read(&file)?;
            let ast = syn::parse_file(&text)
                .map_err(|e| format!("{}: cannot parse: {e}", file.display()))?;
            let mut refs = CrateRefs::default();
            refs.visit_file(&ast);
            for name in refs.names {
                let target = if modules.contains(name.as_str()) {
                    name.clone()
                } else if let Some(origin) = reexports.get(&name) {
                    origin.clone()
                } else {
                    violations.push(format!(
                        "{}: `crate::{name}` does not resolve to a mapped module or re-export",
                        rel(&file)
                    ));
                    continue;
                };
                if target != *module && !allowed.contains(target.as_str()) {
                    let via = if target == name {
                        String::new()
                    } else {
                        format!(" (via `crate::{name}`)")
                    };
                    violations.push(format!(
                        "{}: `{module}` may not use `{target}`{via}",
                        rel(&file)
                    ));
                }
            }
        }
    }
    if violations.is_empty() {
        println!("core-modules: {} modules follow the module map", MAP.len());
        return Ok(());
    }
    violations.sort();
    violations.dedup();
    Err(format!("core module map violations:\n  {}", violations.join("\n  ")).into())
}

/// First segments of every `crate::…` path and `use crate::…` tree in one file.
#[derive(Default)]
struct CrateRefs {
    names: BTreeSet<String>,
}

impl<'ast> Visit<'ast> for CrateRefs {
    fn visit_path(&mut self, path: &'ast syn::Path) {
        let mut segments = path.segments.iter();
        if segments.next().is_some_and(|s| s.ident == "crate")
            && let Some(next) = segments.next()
        {
            self.names.insert(next.ident.to_string());
        }
        syn::visit::visit_path(self, path);
    }

    fn visit_item_use(&mut self, item: &'ast syn::ItemUse) {
        if let syn::UseTree::Path(path) = &item.tree
            && path.ident == "crate"
        {
            first_segments(&path.tree, &mut self.names);
        }
    }

    /// Test modules declared inline are test code, like `tests.rs` files.
    fn visit_item_mod(&mut self, item: &'ast syn::ItemMod) {
        let is_test = item.attrs.iter().any(|a| {
            a.path().is_ident("cfg") && a.parse_args::<syn::Ident>().is_ok_and(|i| i == "test")
        });
        if !is_test {
            syn::visit::visit_item_mod(self, item);
        }
    }
}

fn first_segments(tree: &syn::UseTree, out: &mut BTreeSet<String>) {
    match tree {
        syn::UseTree::Path(p) => {
            out.insert(p.ident.to_string());
        }
        syn::UseTree::Name(n) => {
            out.insert(n.ident.to_string());
        }
        syn::UseTree::Rename(r) => {
            out.insert(r.ident.to_string());
        }
        syn::UseTree::Group(g) => {
            for item in &g.items {
                first_segments(item, out);
            }
        }
        syn::UseTree::Glob(_) => {
            out.insert("*".into());
        }
    }
}

/// Crate-root name → module defining it: explicit `pub use m::{…}` items, the public items of
/// glob-re-exported modules, and the root's own public items.
fn reexports(src: &Path) -> Result<BTreeMap<String, String>> {
    let lib = syn::parse_file(&read(&src.join("lib.rs"))?)
        .map_err(|e| format!("lib.rs: cannot parse: {e}"))?;
    let mut map = BTreeMap::new();
    for item in &lib.items {
        match item {
            syn::Item::Use(u) if matches!(u.vis, syn::Visibility::Public(_)) => {
                let syn::UseTree::Path(p) = &u.tree else {
                    continue;
                };
                let module = p.ident.to_string();
                let mut names = BTreeSet::new();
                first_segments(&p.tree, &mut names);
                for name in names {
                    if name == "*" {
                        for item in public_items(src, &module)? {
                            map.entry(item).or_insert_with(|| module.clone());
                        }
                    } else {
                        map.insert(name, module.clone());
                    }
                }
            }
            other => {
                if let Some(name) = public_item(other) {
                    map.insert(name, ROOT.into());
                }
            }
        }
    }
    Ok(map)
}

fn public_items(src: &Path, module: &str) -> Result<Vec<String>> {
    let mut items = Vec::new();
    for file in module_files(src, module)? {
        let ast = syn::parse_file(&read(&file)?)
            .map_err(|e| format!("{}: cannot parse: {e}", file.display()))?;
        items.extend(ast.items.iter().filter_map(public_item));
    }
    Ok(items)
}

fn public_item(item: &syn::Item) -> Option<String> {
    let (vis, ident) = match item {
        syn::Item::Struct(i) => (&i.vis, &i.ident),
        syn::Item::Enum(i) => (&i.vis, &i.ident),
        syn::Item::Type(i) => (&i.vis, &i.ident),
        syn::Item::Const(i) => (&i.vis, &i.ident),
        syn::Item::Static(i) => (&i.vis, &i.ident),
        syn::Item::Fn(i) => (&i.vis, &i.sig.ident),
        syn::Item::Trait(i) => (&i.vis, &i.ident),
        _ => return None,
    };
    matches!(vis, syn::Visibility::Public(_)).then(|| ident.to_string())
}

/// `mod` items declared by `lib.rs`, except the test module.
fn declared_modules(src: &Path) -> Result<Vec<String>> {
    let lib = syn::parse_file(&read(&src.join("lib.rs"))?)
        .map_err(|e| format!("lib.rs: cannot parse: {e}"))?;
    Ok(lib
        .items
        .iter()
        .filter_map(|item| match item {
            syn::Item::Mod(m) if m.ident != "tests" => Some(m.ident.to_string()),
            _ => None,
        })
        .collect())
}

/// `src/<module>.rs` and every non-test file under `src/<module>/`.
fn module_files(src: &Path, module: &str) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let single = src.join(format!("{module}.rs"));
    if single.is_file() {
        files.push(single);
    }
    let dir = src.join(module);
    if dir.is_dir() {
        collect(&dir, &mut files)?;
    }
    if files.is_empty() {
        return Err(format!("core module `{module}` has no source file").into());
    }
    Ok(files)
}

fn collect(dir: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    let mut entries: Vec<_> = fs::read_dir(dir)?.collect::<std::io::Result<_>>()?;
    entries.sort_by_key(|e| e.path());
    for entry in entries {
        let path = entry.path();
        let name = entry.file_name();
        if name == "tests" || name == "tests.rs" {
            continue;
        }
        if path.is_dir() {
            collect(&path, files)?;
        } else if path.extension().is_some_and(|e| e == "rs") {
            if files.len() >= MAX_FILES {
                return Err("core module tree exceeds the file limit".into());
            }
            files.push(path);
        }
    }
    Ok(())
}

fn read(path: &Path) -> Result<String> {
    if fs::metadata(path)?.len() > MAX_FILE_BYTES {
        return Err(format!("{} exceeds the size limit", path.display()).into());
    }
    Ok(fs::read_to_string(path)?)
}

fn rel(path: &Path) -> String {
    root()
        .ok()
        .and_then(|r| path.strip_prefix(r).ok().map(Path::to_path_buf))
        .unwrap_or_else(|| path.to_path_buf())
        .display()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refs(source: &str) -> Result<Vec<String>> {
        let mut refs = CrateRefs::default();
        refs.visit_file(&syn::parse_file(source)?);
        Ok(refs.names.into_iter().collect())
    }

    #[test]
    fn use_trees_paths_and_root_names_are_collected() -> Result<()> {
        let source = r#"
            use crate::{ids::Id, model::{Project, Wire}, nets};
            use crate::library as lib;
            fn f(_: crate::Result<()>) -> crate::instances::Resolved { todo() }
        "#;
        assert_eq!(
            refs(source)?,
            ["Result", "ids", "instances", "library", "model", "nets"]
        );
        Ok(())
    }

    #[test]
    fn inline_test_modules_are_not_module_dependencies() -> Result<()> {
        let source = r#"
            use crate::ids::Id;
            #[cfg(test)]
            mod tests { use crate::updates::Change; }
        "#;
        assert_eq!(refs(source)?, ["ids"]);
        Ok(())
    }

    #[test]
    fn the_map_is_acyclic_in_document_order() {
        for (i, (module, allowed)) in MAP.iter().enumerate() {
            for used in *allowed {
                let position = MAP.iter().position(|(m, _)| m == used);
                assert!(
                    position.is_some_and(|p| p < i),
                    "{module} may use {used}, which is not listed before it"
                );
            }
        }
    }
}
