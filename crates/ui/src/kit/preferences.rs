//! Shared preference compositions: the page scroller, rounded outlines and inline notes (the rest
//! of SolidCraft's preferences.rs is not vendored).
use super::metrics::dimension;
use crate::tokens::Tokens;
use egui::{Rect, Sense, Ui, pos2, vec2};

pub(super) const PAGE_SCROLL_GUTTER: &str = "sc_page_scroll_gutter";

/// Page scroller keeps content aligned while centring its bar in the window's right inset.
/// Nested pages (including Shortcuts) inherit the same gutter and height contract.
pub fn settings_scroll<R>(ui: &mut Ui, id: impl egui::AsIdSalt, add: impl FnOnce(&mut Ui) -> R) -> egui::scroll_area::ScrollAreaOutput<R> {
    let t = Tokens::get();
    let gutter = ui.stack().iter().find_map(|node| node.tags().get_downcast::<f32>(PAGE_SCROLL_GUTTER)).copied().unwrap_or(0.0);
    let width = dimension(ui.available_width());
    let height = if ui.available_height().is_finite() { dimension(ui.available_height()) } else { t.control_height * 16.0 };
    let bar = ui.spacing().scroll.floating_width;
    let extension = if gutter > 0.0 { gutter / 2.0 + bar / 2.0 } else { 0.0 };
    let host = Rect::from_min_size(ui.next_widget_position(), vec2(width + extension, height));
    ui.scope_builder(egui::UiBuilder::new().max_rect(host).layout(egui::Layout::top_down(egui::Align::Min)), |ui| {
        if gutter > 0.0 {
            let scroll = &mut ui.style_mut().spacing.scroll;
            scroll.floating = true;
            scroll.floating_allocated_width = 0.0;
            scroll.bar_outer_margin = 0.0;
        }
        let mut scroll = egui::ScrollArea::vertical().id_salt(id).max_height(height).auto_shrink([false, false]).show(ui, |ui| {
            if gutter > 0.0 {
                // The scrollbar host extends into the gutter. Its content must neither use
                // that extension nor grow the page/window when a child requests extra width.
                let bounds = Rect::from_min_size(ui.next_widget_position(), vec2(width, ui.available_height()));
                let mut content = ui.new_child(egui::UiBuilder::new().max_rect(bounds));
                let clip = content.clip_rect();
                content.set_clip_rect(clip.intersect(Rect::from_min_max(pos2(bounds.left(), clip.top()), pos2(bounds.right(), clip.bottom()))));
                let result = add(&mut content);
                ui.allocate_exact_size(vec2(width, content.min_size().y), Sense::hover());
                result
            } else {
                add(ui)
            }
        });
        // Consumers publish the page's content well, not the extended scrollbar host.
        if gutter > 0.0 {
            scroll.inner_rect.max.x = scroll.inner_rect.left() + width;
        }
        scroll
    })
    .inner
}
