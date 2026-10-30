//! Feature panels build forms through the shared card kit
//! (`apps/pcbcraft/src/kit`), not raw egui widgets, so every panel has the
//! same label column, controls, sections and messages as Solidcraft's
//! dialogs. Shell, kit, platform-dialog and renderer modules are the
//! infrastructure that may use egui directly; every other module of the
//! desktop app is feature content and is checked. Rust is parsed; code
//! inside macro invocations is not.
use crate::{Result, root};
use std::path::Path;
use syn::visit::{self, Visit};

/// Desktop modules allowed to use egui widgets directly, relative to
/// `apps/pcbcraft/src`.
const INFRASTRUCTURE: &[&str] = &[
    "kit",
    "app",
    "app.rs",
    "shell.rs",
    "file_ui.rs",
    "viewport.rs",
    "scene.rs",
    "palette.rs",
    "main.rs",
];

/// Widget types with a kit equivalent.
const TYPES: &[&str] = &[
    "ComboBox",
    "DragValue",
    "CollapsingHeader",
    "Checkbox",
    "Slider",
];

/// `Ui` methods with a kit equivalent: sections, check rows, status lines,
/// text rows and rules.
const METHODS: &[&str] = &[
    "collapsing",
    "checkbox",
    "separator",
    "colored_label",
    "text_edit_singleline",
    "text_edit_multiline",
    "strong",
    "weak",
    "heading",
    "radio_value",
];

#[derive(Default)]
struct Guard {
    found: Vec<String>,
}

impl<'ast> Visit<'ast> for Guard {
    fn visit_path(&mut self, path: &'ast syn::Path) {
        let names: Vec<String> = path.segments.iter().map(|s| s.ident.to_string()).collect();
        if names.first().is_some_and(|n| n == "egui")
            && let Some(name) = names.iter().skip(1).find(|n| TYPES.contains(&n.as_str()))
        {
            let line = syn::spanned::Spanned::span(path).start().line;
            self.found.push(format!("line {line}: egui::{name}"));
        }
        visit::visit_path(self, path);
    }
    fn visit_expr_method_call(&mut self, call: &'ast syn::ExprMethodCall) {
        let name = call.method.to_string();
        if METHODS.contains(&name.as_str()) {
            let line = call.method.span().start().line;
            self.found.push(format!("line {line}: .{name}(…)"));
        }
        visit::visit_expr_method_call(self, call);
    }
}

fn feature_files(directory: &Path, base: &Path, out: &mut Vec<std::path::PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(directory)? {
        let path = entry?.path();
        let relative = path.strip_prefix(base).unwrap_or(&path);
        let first = relative
            .components()
            .next()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .unwrap_or_default();
        if INFRASTRUCTURE.contains(&first.as_str()) {
            continue;
        }
        if path.is_dir() {
            feature_files(&path, base, out)?;
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
    Ok(())
}

fn scan(source: &str) -> std::result::Result<Vec<String>, syn::Error> {
    let syntax = syn::parse_file(source)?;
    let mut guard = Guard::default();
    guard.visit_file(&syntax);
    Ok(guard.found)
}

pub fn check() -> Result<()> {
    let base = root()?.join("apps/pcbcraft/src");
    let mut files = Vec::new();
    feature_files(&base, &base, &mut files)?;
    let mut violations = Vec::new();
    for file in &files {
        let source = std::fs::read_to_string(file)?;
        let found = scan(&source).map_err(|e| format!("{}: cannot parse: {e}", file.display()))?;
        violations.extend(
            found
                .into_iter()
                .map(|f| format!("{}: {f}; build forms with crate::kit", file.display())),
        );
    }
    if violations.is_empty() {
        eprintln!("UI forms: {} feature modules use the card kit", files.len());
        Ok(())
    } else {
        Err(violations.join("\n").into())
    }
}

#[cfg(test)]
mod tests {
    use super::scan;

    #[test]
    fn raw_form_widgets_are_rejected_and_kit_calls_are_allowed() -> Result<(), syn::Error> {
        assert_eq!(
            scan("fn f(ui: &mut egui::Ui){ egui::ComboBox::from_id_salt(1); }")?.len(),
            1
        );
        assert_eq!(
            scan("fn f(ui: &mut egui::Ui){ ui.collapsing(\"x\", |_| {}); }")?.len(),
            1
        );
        assert_eq!(
            scan("fn f(ui: &mut egui::Ui){ ui.checkbox(&mut b, \"x\"); }")?.len(),
            1
        );
        assert_eq!(
            scan(
                "fn f(ui: &mut egui::Ui){ kit::check_row(ui, \"x\", &mut b); kit::section(ui, 1, \"T\", true, None, |_| {}); }"
            )?,
            Vec::<String>::new()
        );
        Ok(())
    }
}
