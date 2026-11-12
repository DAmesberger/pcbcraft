//! Every outer UI host goes through Solidcraft's `ui_kit::Container` (or its
//! system `ui_kit::Window`), so panels share chrome, insets and input
//! shielding. Adapted from Solidcraft's xtask `ui_containers.rs` (pinned
//! revision; MIT option): Rust is parsed, so `use egui::Window as W` and
//! `egui::containers::Area` are caught as well as plain paths. Code inside
//! macro invocations is not parsed.
use crate::{Result, root};
use std::path::Path;
use syn::visit::{self, Visit};

fn host(name: &str) -> bool {
    matches!(
        name,
        "Area"
            | "Window"
            | "Modal"
            | "Panel"
            | "SidePanel"
            | "TopBottomPanel"
            | "CentralPanel"
            | "Popup"
            | "SubMenu"
    )
}

#[derive(Default)]
struct Guard {
    found: Vec<String>,
}

impl Guard {
    fn check(&mut self, names: &[String], line: usize, what: &str) {
        if names.first().is_some_and(|n| n == "egui") && names.iter().skip(1).any(|n| host(n)) {
            self.found
                .push(format!("line {line}: {what} {}", names.join("::")));
        }
    }
}

fn use_paths(tree: &syn::UseTree, prefix: &mut Vec<String>, out: &mut Vec<Vec<String>>) {
    match tree {
        syn::UseTree::Path(path) => {
            prefix.push(path.ident.to_string());
            use_paths(&path.tree, prefix, out);
            prefix.pop();
        }
        syn::UseTree::Name(name) => {
            let mut full = prefix.clone();
            full.push(name.ident.to_string());
            out.push(full);
        }
        syn::UseTree::Rename(rename) => {
            let mut full = prefix.clone();
            full.push(rename.ident.to_string());
            out.push(full);
        }
        syn::UseTree::Glob(_) => {
            let mut full = prefix.clone();
            full.push("*".into());
            out.push(full);
        }
        syn::UseTree::Group(group) => {
            for item in &group.items {
                use_paths(item, prefix, out);
            }
        }
    }
}

impl<'ast> Visit<'ast> for Guard {
    fn visit_item_use(&mut self, item: &'ast syn::ItemUse) {
        let mut paths = Vec::new();
        use_paths(&item.tree, &mut Vec::new(), &mut paths);
        let line = syn::spanned::Spanned::span(item).start().line;
        for path in paths {
            self.check(&path, line, "imports");
        }
    }
    fn visit_path(&mut self, path: &'ast syn::Path) {
        let names: Vec<String> = path.segments.iter().map(|s| s.ident.to_string()).collect();
        let line = syn::spanned::Spanned::span(path).start().line;
        self.check(&names, line, "uses");
        visit::visit_path(self, path);
    }
}

fn rust_files(directory: &Path, out: &mut Vec<std::path::PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(directory)? {
        let path = entry?.path();
        if path.is_dir() {
            rust_files(&path, out)?;
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
    Ok(())
}

pub fn check() -> Result<()> {
    let root = root()?;
    let mut files = Vec::new();
    for directory in ["apps", "crates"] {
        rust_files(&root.join(directory), &mut files)?;
    }
    let mut violations = Vec::new();
    // The vendored kit is where `Container` and `Window` themselves are built.
    let kit = root.join("crates/ui");
    for file in files.into_iter().filter(|f| !f.starts_with(&kit)) {
        let source = std::fs::read_to_string(&file)?;
        let syntax = syn::parse_file(&source)
            .map_err(|e| format!("{}: cannot parse: {e}", file.display()))?;
        let mut guard = Guard::default();
        guard.visit_file(&syntax);
        violations.extend(
            guard
                .found
                .into_iter()
                .map(|f| format!("{}: {f}; use ui_kit::Container", file.display())),
        );
    }
    if violations.is_empty() {
        eprintln!("UI container boundary passed");
        Ok(())
    } else {
        Err(violations.join("\n").into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan(source: &str) -> Vec<String> {
        let mut guard = Guard::default();
        if let Ok(file) = syn::parse_file(source) {
            guard.visit_file(&file);
        }
        guard.found
    }

    #[test]
    fn raw_hosts_and_aliases_are_rejected_and_the_kit_is_allowed() {
        assert_eq!(scan("fn f(){ egui::Window::new(\"x\"); }").len(), 1);
        assert_eq!(scan("use egui::Area as A;").len(), 1);
        assert_eq!(scan("use egui::{containers::{SidePanel}, Ui};").len(), 1);
        assert!(scan("fn f(){ ui_kit::Window::new(id, \"i\", \"t\"); }").is_empty());
        assert!(scan("fn f(){ egui::Frame::NONE; egui::Ui::new(); }").is_empty());
    }
}
