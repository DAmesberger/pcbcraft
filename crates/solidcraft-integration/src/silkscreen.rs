//! Display-only silkscreen films (graphics design G5): the placed footprint
//! legend (`layout::placed_graphics`, silkscreen layers) as thin matte-ink
//! films on the mask, in the board's silkscreen colour, clipped at mask
//! openings, top and bottom. Never saved in the `.solidcraft` file.
use crate::Result;
use crate::appearance::{Board, Land};
use crate::scene::Geometry;
use pcbcraft_core::Project;

/// Legend ink thickness over the mask (est.; screen-printed and inkjet
/// legend is 10–25 µm).
pub const INK_THICKNESS_MM: f64 = 0.015;

/// The silkscreen films of `project` on `board`, cut at the mask openings
/// of `lands`. Each film is a [`crate::material::BodyKind::Silkscreen`]
/// geometry whose `layer` names its side's outer copper layer.
pub fn films(project: &Project, board: &Board, lands: &[Land]) -> Result<Vec<Geometry>> {
    // Films come with the native slice; until then the board shows none.
    let _ = (project, board, lands);
    Ok(Vec::new())
}
