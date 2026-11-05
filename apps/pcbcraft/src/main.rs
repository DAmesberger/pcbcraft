//! Pcbcraft desktop editor.
mod app;
mod commands;
mod file_ui;
mod fill_worker;
mod issues;
mod keymap;
mod keys;
mod kit;
mod layer_view;
mod library_sources;
mod lighting;
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

fn main() -> eframe::Result {
    let compact = std::env::args().any(|argument| argument == "--compact");
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
        "Pcbcraft",
        options,
        Box::new(|cc| Ok(Box::new(app::App::new(cc)?))),
    )
}
