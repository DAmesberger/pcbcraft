//! Shared aspect choices and camera composition guides; no document or command authority.

pub struct AspectTile<'a> {
    pub id: &'a str,
    pub label: &'a str,
    pub aspect: f64,
    pub selected: bool,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct FrameGuides {
    pub thirds: bool,
    pub safe_area: bool,
    pub centre_mark: bool,
}
