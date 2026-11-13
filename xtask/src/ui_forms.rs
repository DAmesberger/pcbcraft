//! Feature panels build forms through the shared card kit
//! (`apps/sparksmith/src/kit`), not raw egui widgets, so every panel has the
//! same label column, controls, sections and messages as Solidcraft's
//! dialogs. Shell, kit, platform-dialog and renderer modules are the
//! infrastructure that may use egui directly; every other module of the
//! desktop app is feature content and is checked. Rust is parsed; code
//! inside macro invocations is not.
//!
//! Feature modules also show error and warning text only through
//! `kit::field_error` and `kit::refusal` (docs/sidebar-ia.md, "Enforcement"):
//! `Tone::Error` and `Tone::Warning` messages are counted per file and may
//! not exceed [`LEGACY_MESSAGES`], the sites the sidebar slices still migrate.
use crate::{Result, root};
use std::path::Path;
use syn::visit::{self, Visit};

/// Desktop modules allowed to use egui widgets directly, relative to
/// `apps/sparksmith/src`.
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

/// Error and warning text sites that predate the sidebar redesign, per file
/// relative to `apps/sparksmith/src`. Each slice that migrates a file lowers or
/// removes its entry; no file may gain one.
const LEGACY_MESSAGES: &[(&str, usize)] = &[
    ("library_sources.rs", 5),
    ("part_editor/footprint_template_picker.rs", 2),
    ("part_editor/model_props.rs", 2),
    ("schematic_editor/instance_card.rs", 1),
];

/// Message tones that only `kit::field_error` and `kit::refusal` may show.
const PROBLEM_TONES: &[&str] = &["Error", "Warning"];

#[derive(Default)]
struct Guard {
    found: Vec<String>,
    /// Lines of `Tone::Error` and `Tone::Warning` paths.
    problems: Vec<usize>,
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
        if let [.., tone, level] = names.as_slice()
            && tone == "Tone"
            && PROBLEM_TONES.contains(&level.as_str())
        {
            self.problems
                .push(syn::spanned::Spanned::span(path).start().line);
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

#[cfg(test)]
fn scan(source: &str) -> std::result::Result<Vec<String>, syn::Error> {
    Ok(guard(source)?.found)
}

fn guard(source: &str) -> std::result::Result<Guard, syn::Error> {
    let syntax = syn::parse_file(source)?;
    let mut guard = Guard::default();
    guard.visit_file(&syntax);
    Ok(guard)
}

/// The message-rule violation for `file` with problem-tone sites on
/// `lines`, given its `allowed` legacy count.
fn message_violation(file: &str, lines: &[usize], allowed: usize) -> Option<String> {
    (lines.len() > allowed).then(|| {
        let at: Vec<String> = lines.iter().map(usize::to_string).collect();
        format!(
            "{file}: {} error or warning messages (lines {}), {allowed} allowed; show them with kit::field_error or kit::refusal",
            lines.len(),
            at.join(", ")
        )
    })
}

pub fn check() -> Result<()> {
    let base = root()?.join("apps/sparksmith/src");
    let mut files = Vec::new();
    feature_files(&base, &base, &mut files)?;
    let mut violations = Vec::new();
    let mut legacy = 0;
    for file in &files {
        let source = std::fs::read_to_string(file)?;
        let found = guard(&source).map_err(|e| format!("{}: cannot parse: {e}", file.display()))?;
        violations.extend(
            found
                .found
                .into_iter()
                .map(|f| format!("{}: {f}; build forms with crate::kit", file.display())),
        );
        let relative = file
            .strip_prefix(&base)
            .unwrap_or(file)
            .to_string_lossy()
            .replace('\\', "/");
        let allowed = LEGACY_MESSAGES
            .iter()
            .find(|(name, _)| *name == relative)
            .map_or(0, |(_, n)| *n);
        legacy += found.problems.len().min(allowed);
        violations.extend(message_violation(&relative, &found.problems, allowed));
    }
    if violations.is_empty() {
        eprintln!(
            "UI forms: {} feature modules use the card kit; {legacy} legacy error or warning messages remain",
            files.len()
        );
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

    #[test]
    fn error_and_warning_text_is_counted_against_the_legacy_sites() -> Result<(), syn::Error> {
        let source = "fn f(ui: &mut egui::Ui){ kit::status(ui, \"x\", kit::Tone::Error); kit::status(ui, \"y\", Tone::Warning); kit::status(ui, \"z\", Tone::Dim); }";
        let found = super::guard(source)?;
        assert_eq!(found.problems, vec![1, 1]);
        assert!(super::message_violation("a.rs", &found.problems, 2).is_none());
        let over = super::message_violation("a.rs", &found.problems, 1);
        assert!(over.is_some_and(|m| m.contains("kit::field_error")));
        let kit = super::guard(
            "fn f(ui: &mut egui::Ui){ kit::field_error(ui, &r, \"m\", true); kit::refusal(ui, \"m\"); }",
        )?;
        assert!(kit.problems.is_empty());
        Ok(())
    }
}
