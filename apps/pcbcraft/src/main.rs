//! Pcbcraft desktop editor.
mod app;
mod file_ui;
mod kit;
mod lighting;
mod model_scene;
mod palette;
mod part_editor;
mod pcb_editor;
mod place_palette;
mod scene;
mod schematic_editor;
mod shell;
mod update_preview;
mod viewport;

fn main() -> eframe::Result {
    let compact = std::env::args().any(|argument| argument == "--compact");
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
