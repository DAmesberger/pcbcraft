//! Sparksmith desktop editor (formerly Pcbcraft).
mod app;
#[cfg(test)]
mod bundle_tests;
mod commands;
mod context_menu;
mod file_ui;
mod fill_worker;
mod issues;
mod keymap;
mod keys;
mod kit;
mod layer_view;
mod library_sources;
mod lighting;
mod menu_bar;
mod model_export;
mod model_library;
mod model_scene;
#[cfg(target_os = "macos")]
mod native_menu;
mod navigation;
mod open_documents;
mod palette;
mod part_chooser;
mod part_editor;
mod pcb_editor;
mod pin_mux;
mod place_palette;
mod recent_projects;
mod refusal;
mod resolve_parts;
mod reveal;
mod scene;
mod schematic_editor;
mod shell;
mod symbol_view;
mod template_gallery;
mod update_preview;
mod viewport;

/// Tests: the shipped material library, loaded once for the test process
/// as `main` loads it for the program.
#[cfg(test)]
pub(crate) fn test_materials() -> Result<&'static sparksmith_core::materials::MaterialLibrary, String> {
    sparksmith_solidcraft::looks::ensure(sparksmith_io::materials::load_shipped)
        .map_err(|e| e.to_string())
}

/// Tests: the stack of the default stackup with the material library
/// loaded.
#[cfg(test)]
pub(crate) fn test_stack() -> Result<sparksmith_solidcraft::stack::Stack, String> {
    test_materials()?;
    sparksmith_solidcraft::stack::Stack::standard().map_err(|e| e.to_string())
}

fn main() -> eframe::Result {
    let compact = std::env::args().any(|argument| argument == "--compact");
    // Before anything reads or creates the per-user folders.
    sparksmith_io::settings::adopt_former_folders();
    // The shipped material library, once: the 3D view and every export
    // read it, so no build reloads it.
    sparksmith_automation::load_materials();
    // Files opened from Finder arrive as AppKit events, not arguments.
    #[cfg(target_os = "macos")]
    open_documents::install();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size(if compact {
                [900.0, 680.0]
            } else {
                [1380.0, 900.0]
            })
            .with_min_inner_size([760.0, 560.0])
            .with_title_shown(false)
            .with_titlebar_shown(false)
            .with_fullsize_content_view(true),
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    eframe::run_native(
        sparksmith_core::product::NAME,
        options,
        Box::new(|cc| Ok(Box::new(app::App::new(cc)?))),
    )
}
