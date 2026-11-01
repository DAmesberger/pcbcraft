//! Copper fills in the native model: each fill's computed copper
//! ([`pcbcraft_core::model::fill::FilledCopper`]) as saved copper on its
//! layer, one body per region in one component per fill, each extruded
//! once from its boundary and holes (inserted into the sketch in one
//! command), so pad clearances cost no Boolean and no command each. The
//! bodies are built last, after the rest of the board: every later
//! command would pay for re-evaluating them. An inner-layer
//! fill is also cut from the laminate, as inner tracks are, so the saved
//! bodies stay disjoint. Over an outer-layer fill the display-only mask
//! conforms as a cap, as over a track; it is never saved.
//!
//! The fill copper is the copper the document stores: the board view and
//! the 3D view show the same regions, stale or not, and a refill (a new
//! document revision) rebuilds them.
use super::{Builder, Source, copper_param, inner_base, next_pick};
use crate::appearance::{self, Board, Land, MASK_CAP_MM, Region, ViaMark};
use crate::material::BodyKind;
use crate::native::{BoardJoint, Expr, Mount};
use crate::scene::Geometry;
use crate::{Error, Result};
use pcbcraft_core::model::Fill;
use pcbcraft_core::model::fill::FilledRegion;
use pcbcraft_core::{CopperLayer, Point, Project};

/// Mask cap tiles over a fill, as the board mask's.
const CAP_TILE_MM: f64 = 4.0;

/// A fill as the display layers see it: its pick, layer and regions.
pub(super) struct FillMark<'a> {
    pub pick: u64,
    pub layer: CopperLayer,
    pub regions: &'a [FilledRegion],
}

/// The fills with copper to build, in document order.
fn built(project: &Project) -> impl Iterator<Item = (&Fill, &[FilledRegion])> {
    project.fills.iter().filter_map(|f| {
        let regions = f.computed.as_ref()?.regions.as_slice();
        (!regions.is_empty() && project.stackup.has(f.layer)).then_some((f, regions))
    })
}

/// A region's outer boundary and holes, or nothing for a region of
/// fewer than three points.
fn loops(region: &FilledRegion) -> Vec<&[Point]> {
    if region.outer.len() < 3 {
        return Vec::new();
    }
    std::iter::once(region.outer.as_slice())
        .chain(
            region
                .holes
                .iter()
                .filter(|h| h.len() >= 3)
                .map(Vec::as_slice),
        )
        .collect()
}

fn body_name(fill: &Fill) -> String {
    format!("fill_{}", fill.id)
}

/// Every fill's copper ([`bodies`]), after the inner-layer fills are cut
/// from the laminate ([`cuts`]).
pub(super) fn build<'a>(
    b: &mut Builder,
    project: &'a Project,
    board: u64,
    sources: &mut Vec<Source>,
) -> Result<Vec<FillMark<'a>>> {
    cuts(b, project)?;
    bodies(b, project, board, sources)
}

/// Cut every inner-layer fill's copper from the laminate, in the
/// substrate's component.
fn cuts(b: &mut Builder, project: &Project) -> Result<()> {
    if !built(project).any(|(f, _)| f.layer.side().is_none()) {
        return Ok(());
    }
    let substrate = b
        .native
        .find_component("PCB substrate")
        .ok_or_else(|| Error::Verification("the board has no substrate component".into()))?;
    b.native.activate(substrate.component)?;
    for (fill, regions) in built(project).filter(|(f, _)| f.layer.side().is_none()) {
        let loops: Vec<&[Point]> = regions.iter().flat_map(loops).collect();
        if loops.is_empty() {
            continue;
        }
        b.native.polyline_cut(
            &format!("{} cut", body_name(fill)),
            inner_base(&project.stackup, fill.layer)?.into(),
            &loops,
            copper_param(fill.layer).into(),
            &["Substrate"],
        )?;
    }
    Ok(())
}

/// Each fill's copper as one body in a component of its own, held upright
/// on the board at the origin; its source names the fill, its net and
/// layer. Returns what the display layers draw over each.
fn bodies<'a>(
    b: &mut Builder,
    project: &'a Project,
    board: u64,
    sources: &mut Vec<Source>,
) -> Result<Vec<FillMark<'a>>> {
    let mut marks = Vec::new();
    for (fill, regions) in built(project) {
        if regions.iter().all(|r| loops(r).is_empty()) {
            continue;
        }
        let name = body_name(fill);
        let height = copper_param(fill.layer);
        let offset: Expr = match fill.layer {
            CopperLayer::Top => "board_thickness".into(),
            CopperLayer::Bottom => format!("- {height}").into(),
            CopperLayer::Inner(_) => inner_base(&project.stackup, fill.layer)?.into(),
        };
        let created = b.native.component(&name, false)?;
        // One body per region: the regions are disjoint, so each sketch
        // makes exactly one body of a known name.
        for (k, region) in regions.iter().enumerate() {
            let loops = loops(region);
            if loops.is_empty() {
                continue;
            }
            let body = format!("{name}_{k}");
            b.native
                .polyline_body(&body, offset.clone(), &loops, height.as_str().into())?;
            b.record(&body, BodyKind::FillCopper)?;
        }
        b.native.board_joint(BoardJoint {
            board,
            occurrence: created.occurrence,
            x: 0.into(),
            y: 0.into(),
            angle: 0.into(),
            mount: Mount::Height(0.into()),
        })?;
        let pick = next_pick(sources);
        let label = if fill.name.is_empty() {
            format!(
                "Fill · {}",
                fill.net.map_or("No net", |n| project.net_name(n))
            )
        } else {
            format!("Fill · {}", fill.name)
        };
        sources.push(Source {
            pick_id: pick,
            instance: None,
            pad: None,
            track: None,
            via: None,
            fill: Some(fill.id),
            net: fill.net,
            layer: Some(fill.layer),
            label,
            component: created.component,
            occurrence: None,
        });
        marks.push(FillMark {
            pick,
            layer: fill.layer,
            regions,
        });
    }
    Ok(marks)
}

/// The display-only mask cap over each outer-layer fill region: from the
/// laminate to [`MASK_CAP_MM`] over the copper, opened where the pads' and
/// open vias' mask openings are and where a tented via's dome is the mask.
pub(super) fn films(
    board: &Board,
    lands: &[Land],
    vias: &[ViaMark],
    fills: &[FillMark<'_>],
) -> Result<Vec<Geometry>> {
    let mut out = Vec::new();
    for mark in fills {
        let Some(side) = mark.layer.side() else {
            continue;
        };
        if !board.mask(side) {
            continue;
        }
        let openings: Vec<Vec<Point>> = lands
            .iter()
            .filter(|land| land.layer == side)
            .flat_map(|land| land.openings.iter().cloned())
            .chain(
                vias.iter()
                    .filter(|v| v.covered(board, side))
                    .map(|v| appearance::circle(v.at, v.diameter / 2.0)),
            )
            .collect();
        let (z0, z1) = board.span(side, 0.0, board.copper(side) + MASK_CAP_MM);
        for region in mark.regions.iter().filter(|r| r.outer.len() >= 3) {
            let bounds = bounds(&region.outer);
            let region = Region {
                fills: vec![region.outer.clone()],
                cuts: region
                    .holes
                    .iter()
                    .cloned()
                    .chain(
                        openings
                            .iter()
                            .filter(|o| overlaps(bounds, self::bounds(o)))
                            .cloned(),
                    )
                    .collect(),
            };
            appearance::push(
                &mut out,
                &board.fabrication,
                BodyKind::MaskCap,
                Some(mark.pick),
                Some(side.into()),
                appearance::slab(&region, z0, z1, CAP_TILE_MM),
            )?;
        }
    }
    Ok(out)
}

/// The box of a loop.
fn bounds(points: &[Point]) -> [Point; 2] {
    points.iter().fold(
        [[f64::INFINITY; 2], [f64::NEG_INFINITY; 2]],
        |[lo, hi], p| {
            [
                [lo[0].min(p[0]), lo[1].min(p[1])],
                [hi[0].max(p[0]), hi[1].max(p[1])],
            ]
        },
    )
}

fn overlaps(a: [Point; 2], b: [Point; 2]) -> bool {
    a[0][0] <= b[1][0] && b[0][0] <= a[1][0] && a[0][1] <= b[1][1] && b[0][1] <= a[1][1]
}

#[cfg(test)]
mod tests;
