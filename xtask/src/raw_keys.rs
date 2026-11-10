//! Every keyboard shortcut goes through the command registry
//! (apps/pcbcraft/src/keys.rs): an editor asks for a command id, never for
//! a key, so a rebinding changes what the key does. This check parses the
//! desktop app and rejects raw key reads (`key_pressed`, `consume_key`,
//! `consume_shortcut`, `key_down`, `key_released`, `num_presses`) outside
//! the dispatch layer, except the counted, reasoned exceptions below.
//! Test code (`#[cfg(test)]` items and `*tests.rs` files) is not checked.
use crate::{Result, root};
use std::collections::BTreeMap;
use std::path::Path;
use syn::visit::{self, Visit};

const RAW: &[&str] = &[
    "key_pressed",
    "key_released",
    "key_down",
    "consume_key",
    "consume_shortcut",
    "num_presses",
];

/// The dispatch layer: it turns key events into commands.
const LAYER: &[&str] = &[
    "keys.rs",
    "commands.rs",
    "app/command_dispatch.rs",
    "app/settings_window.rs",
    "app/key_reveal.rs",
];

/// Raw key reads that are not command shortcuts: (file, how many, why).
/// A new site, one more in a listed file, or a stale count fails the check.
const EXCEPTIONS: &[(&str, usize, &str)] = &[
    (
        "viewport.rs",
        1,
        "navigation: Space held pans the 3D view (pointer gesture)",
    ),
    (
        "part_editor/symbol_canvas.rs",
        1,
        "navigation: Space held pans the symbol canvas",
    ),
    (
        "part_editor/footprint_canvas.rs",
        1,
        "navigation: Space held pans the footprint canvas",
    ),
    (
        "part_editor/footprint_dims_ui.rs",
        2,
        "text field: Enter commits and Esc reverts the dimension being typed",
    ),
    (
        "pcb_editor/dimensions/edit.rs",
        2,
        "text field: Enter commits and Esc drops the part dimension value being typed, in Layout or 3D (one helper for both fields)",
    ),
    (
        "part_editor/model_view.rs",
        1,
        "gesture: Esc ends the triad drag in progress",
    ),
    (
        "part_editor/pins_tab.rs",
        1,
        "text field: Esc reverts the pin field",
    ),
    (
        "schematic_editor.rs",
        2,
        "text field: Esc reverts the label and field being typed",
    ),
    (
        "pcb_editor/net_classes.rs",
        2,
        "text field: Enter commits and Esc reverts a class field",
    ),
    (
        "pcb_editor/rules_panel.rs",
        2,
        "text field: Esc reverts a rule value",
    ),
    (
        "pcb_editor/track_card.rs",
        1,
        "text field: Esc reverts the track field",
    ),
    (
        "pcb_editor/placement_card.rs",
        1,
        "text field: Esc reverts the placement field",
    ),
    (
        "template_gallery.rs",
        2,
        "dialog: Esc closes it and Tab moves between its fields",
    ),
    (
        "place_palette.rs",
        3,
        "list: arrows move the highlight; ⌘D marks the highlighted row a favourite",
    ),
    (
        "pin_mux.rs",
        3,
        "grid: Tab, Shift+Tab and Enter move between and commit cells",
    ),
    (
        "kit/search.rs",
        6,
        "search sheet: arrows, Enter and Esc of the palette lists",
    ),
    (
        "kit/dialog.rs",
        2,
        "dialog: Enter runs the primary and Esc cancels",
    ),
    ("kit/form.rs", 1, "text field: Esc reverts a form row"),
    (
        "context_menu.rs",
        1,
        "menu: Esc closes the open context menu",
    ),
    (
        "model_library.rs",
        1,
        "grid: arrows move the highlighted tile",
    ),
];

#[derive(Default)]
struct Finder {
    lines: Vec<usize>,
}

fn test_only(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|a| {
        a.path().is_ident("cfg")
            && a.parse_args::<syn::Meta>()
                .is_ok_and(|m| m.path().is_ident("test"))
    })
}

impl<'ast> Visit<'ast> for Finder {
    fn visit_item_mod(&mut self, item: &'ast syn::ItemMod) {
        if !test_only(&item.attrs) {
            visit::visit_item_mod(self, item);
        }
    }
    fn visit_item_fn(&mut self, item: &'ast syn::ItemFn) {
        if !test_only(&item.attrs) {
            visit::visit_item_fn(self, item);
        }
    }
    fn visit_impl_item_fn(&mut self, item: &'ast syn::ImplItemFn) {
        if !test_only(&item.attrs) {
            visit::visit_impl_item_fn(self, item);
        }
    }
    fn visit_expr_method_call(&mut self, call: &'ast syn::ExprMethodCall) {
        let name = call.method.to_string();
        if RAW.contains(&name.as_str()) {
            self.lines
                .push(syn::spanned::Spanned::span(&call.method).start().line);
        }
        visit::visit_expr_method_call(self, call);
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

/// Raw key reads of one source file, by line.
fn scan(source: &str) -> std::result::Result<Vec<usize>, String> {
    let syntax = syn::parse_file(source).map_err(|e| e.to_string())?;
    let mut finder = Finder::default();
    finder.visit_file(&syntax);
    Ok(finder.lines)
}

pub fn check() -> Result<()> {
    let base = root()?.join("apps/pcbcraft/src");
    let mut files = Vec::new();
    rust_files(&base, &mut files)?;
    let allowed: BTreeMap<&str, usize> = EXCEPTIONS.iter().map(|(f, n, _)| (*f, *n)).collect();
    let mut violations = Vec::new();
    for file in files {
        let relative = file
            .strip_prefix(&base)
            .map_err(|e| e.to_string())?
            .to_string_lossy()
            .replace('\\', "/");
        if relative.ends_with("tests.rs") || LAYER.contains(&relative.as_str()) {
            continue;
        }
        let source = std::fs::read_to_string(&file)?;
        let lines = scan(&source).map_err(|e| format!("{relative}: cannot parse: {e}"))?;
        let limit = allowed.get(relative.as_str()).copied().unwrap_or(0);
        if lines.len() > limit {
            violations.push(format!(
                "{relative}: {} raw key read(s) at line(s) {:?}, {limit} allowed; \
                 declare a command and read it with crate::keys::pressed",
                lines.len(),
                lines
            ));
        } else if lines.len() < limit {
            violations.push(format!(
                "{relative}: {limit} raw key reads allowed but {} found; lower the exception",
                lines.len()
            ));
        }
    }
    if violations.is_empty() {
        eprintln!("Raw key reads passed");
        Ok(())
    } else {
        Err(violations.join("\n").into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_reads_are_found_and_test_code_is_skipped() {
        let found = scan(
            "fn f(ui: &Ui) { ui.input(|i| i.key_pressed(Key::X)); i.consume_key(m, k); }\n\
             #[cfg(test)] mod tests { fn g() { i.key_pressed(Key::Y); } }",
        );
        assert_eq!(found.map(|l| l.len()), Ok(2));
        assert_eq!(
            scan("fn f() { keys::pressed(ctx, \"pcb.route\"); }"),
            Ok(vec![])
        );
    }

    #[test]
    fn every_exception_says_why() {
        for (file, count, why) in EXCEPTIONS {
            assert!(*count > 0 && !why.is_empty(), "{file}");
        }
    }
}
