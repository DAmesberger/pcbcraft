//! File/start presentation; callers retain commands, caches and recovery ownership.
use crate::tokens::Tokens;
use egui::{InnerResponse, Rect, TextureId, Ui, Vec2, pos2};

pub struct RecoveryMessage<'a> {
    pub title: &'a str,
    pub before_filename: &'a str,
    pub filename: &'a str,
    pub after_filename: &'a str,
}

pub struct RecentRow<'a> {
    pub title: &'a str,
    pub path: &'a str,
    pub when: &'a str,
    pub workspace: &'a str,
    pub thumbnail: Option<(TextureId, Vec2)>,
    /// The design's picture could not be made (empty or unreadable). Draws a dimmed placeholder,
    /// so it reads as "no preview" rather than as one still loading.
    pub thumbnail_failed: bool,
}

/// A picture the host gives a kit surface: a name in the kit's icon set, a texture the host made
/// (its own app icon, say), or the host's own painter, called with the icon's rect and opacity.
#[derive(Clone, Copy)]
pub enum Icon<'a> {
    Kit(&'a str),
    Texture(TextureId),
    Paint(&'a dyn Fn(&egui::Painter, Rect, f32)),
}

impl<'a> From<&'a str> for Icon<'a> {
    fn from(name: &'a str) -> Self {
        Icon::Kit(name)
    }
}

impl Icon<'_> {
    pub fn paint(&self, painter: &egui::Painter, rect: Rect, opacity: f32) {
        match self {
            Icon::Kit(name) => crate::kit::icons::paint_accented(painter, rect, name, None, opacity),
            Icon::Texture(texture) => {
                painter.image(
                    *texture,
                    rect,
                    Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                    Tokens::get().file_image_tint.gamma_multiply(opacity),
                );
            }
            Icon::Paint(paint) => paint(painter, rect, opacity),
        }
    }
}

#[derive(Clone, Copy)]
pub struct SampleCard<'a> {
    pub title: &'a str,
    pub description: &'a str,
    pub icon: Icon<'a>,
    pub thumbnail: Option<(TextureId, Vec2)>,
}

/// Shared start-page surfaces. Every allocation is bounded by the caller's width.
pub struct FileView;

impl FileView {
    pub fn content<R>(ui: &mut Ui, narrow: bool, add: impl FnOnce(&mut Ui) -> R) -> InnerResponse<R> {
        let t = Tokens::get();
        let inset = if narrow { t.space_16 } else { t.file_content_inset_x };
        egui::Frame::NONE.inner_margin(egui::Margin::symmetric(inset as i8, t.file_content_inset_y as i8)).show(ui, add)
    }
}
