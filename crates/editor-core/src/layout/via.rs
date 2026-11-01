//! Board via commands and the document rules over vias. Manufacturing
//! checks (annular ring, drill-to-copper clearance) and connectivity through
//! vias build on these types.
use super::{EPS, Error, Result, invalid};
use crate::geometry::{Point, point_valid};
use crate::ids::Id;
use crate::model::{CopperLayer, Project, Via, ViaSpan};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Most vias on one board.
pub const MAX_VIAS: usize = 4_096;
/// Default through-via drill and pad, in mm (a common fabricator standard
/// via, not a qualification).
pub const DEFAULT_VIA_DRILL_MM: f64 = 0.3;
pub const DEFAULT_VIA_DIAMETER_MM: f64 = 0.6;
const MIN_DRILL_MM: f64 = 0.1;
const MAX_DIAMETER_MM: f64 = 6.0;

/// A via to add: everything but its identity.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ViaRequest {
    pub net: Id,
    pub position: Point,
    pub drill: f64,
    pub diameter: f64,
    pub span: ViaSpan,
    pub tented: bool,
}

impl ViaRequest {
    /// A default through via on `net` at `position`, tented.
    pub fn through(net: Id, position: Point) -> Self {
        Self {
            net,
            position,
            drill: DEFAULT_VIA_DRILL_MM,
            diameter: DEFAULT_VIA_DIAMETER_MM,
            span: ViaSpan::Through,
            tented: true,
        }
    }
}

/// A route that changes layer: a track on `request.layer` to
/// `request.end`, a via there, then a track on `to_layer` to `end`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LayerChangeRequest {
    pub request: super::RouteRequest,
    pub via: ViaRequest,
    pub to_layer: CopperLayer,
    pub end: Point,
}

/// Add a via; returns its identity.
pub(super) fn add(project: &mut Project, request: ViaRequest) -> Result<Id> {
    let id = project.allocate()?;
    project.vias.push(Via {
        id,
        net: request.net,
        position: request.position,
        drill: request.drill,
        diameter: request.diameter,
        span: request.span,
        tented: request.tented,
        locked: false,
    });
    Ok(id)
}

fn unlocked(project: &mut Project, id: Id) -> Result<&mut Via> {
    let via = project
        .vias
        .iter_mut()
        .find(|v| v.id == id)
        .ok_or_else(|| Error::NotFound("unknown via".into()))?;
    if via.locked {
        return Err(Error::Conflict("via is locked".into()));
    }
    Ok(via)
}

pub(super) fn move_to(project: &mut Project, id: Id, position: Point) -> Result<()> {
    unlocked(project, id)?.position = position;
    Ok(())
}

pub(super) fn delete(project: &mut Project, id: Id) -> Result<()> {
    unlocked(project, id)?;
    project.vias.retain(|v| v.id != id);
    Ok(())
}

pub(super) fn set_tented(project: &mut Project, id: Id, tented: bool) -> Result<()> {
    unlocked(project, id)?.tented = tented;
    Ok(())
}

pub(super) fn set_lock(project: &mut Project, id: Id, locked: bool) -> Result<()> {
    project
        .vias
        .iter_mut()
        .find(|v| v.id == id)
        .ok_or_else(|| Error::NotFound("unknown via".into()))?
        .locked = locked;
    Ok(())
}

/// Document rules over vias: bounded count, unique identities, known nets,
/// sane geometry inside the board, through vias only for now.
pub(super) fn validate(project: &Project, nets: &BTreeSet<Id>) -> Result<()> {
    if project.vias.len() > MAX_VIAS {
        return invalid("prototype supports at most 4096 vias");
    }
    let mut ids = BTreeSet::new();
    for via in &project.vias {
        if via.id == 0 || via.id >= project.next_id || !ids.insert(via.id) {
            return invalid("invalid or duplicate via identity");
        }
        if !nets.contains(&via.net) {
            return invalid("via references an unknown net");
        }
        if !point_valid(via.position)
            || !via.drill.is_finite()
            || !via.diameter.is_finite()
            || via.drill < MIN_DRILL_MM
            || via.diameter > MAX_DIAMETER_MM
            || via.diameter <= via.drill + EPS
        {
            return invalid("via drill must be at least 0.1 mm and smaller than its pad");
        }
        let r = via.diameter / 2.0;
        let [w, h] = project.board_size;
        let [x, y] = via.position;
        if x - r < -EPS || y - r < -EPS || x + r > w + EPS || y + r > h + EPS {
            return invalid("via copper must lie inside the board outline");
        }
        if let ViaSpan::Between { .. } = via.span {
            return Err(Error::Unsupported(
                "blind and buried vias are not supported yet; use a through via".into(),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::LayoutCommand;
    use crate::model::Stackup;
    use crate::{Engine, Operation, Outcome};

    fn layout(engine: &mut Engine, command: LayoutCommand) -> Result<Outcome> {
        engine.apply(engine.revision(), Operation::Layout(command))
    }

    #[test]
    fn a_via_is_added_moved_tented_and_deleted_as_a_document_object() -> Result<()> {
        let mut engine = Engine::demo()?;
        let net = engine.project().nets[0].id;
        let Outcome::ViaAdded { via } = layout(
            &mut engine,
            LayoutCommand::AddVia {
                via: ViaRequest::through(net, [20.0, 20.0]),
            },
        )?
        else {
            return Err(Error::Invalid("no via outcome".into()));
        };
        assert_eq!(engine.project().via(via).map(|v| v.tented), Some(true));
        layout(
            &mut engine,
            LayoutCommand::MoveVia {
                id: via,
                position: [21.0, 20.0],
            },
        )?;
        layout(
            &mut engine,
            LayoutCommand::SetViaTented {
                id: via,
                tented: false,
            },
        )?;
        let moved = engine.project().via(via).cloned();
        assert_eq!(
            moved.map(|v| (v.position, v.tented)),
            Some(([21.0, 20.0], false))
        );
        layout(&mut engine, LayoutCommand::DeleteVia { id: via })?;
        assert!(engine.project().vias.is_empty());
        Ok(())
    }

    #[test]
    fn invalid_vias_leave_the_document_unchanged() -> Result<()> {
        let mut engine = Engine::demo()?;
        let net = engine.project().nets[0].id;
        let before = engine.project().clone();
        let outside = ViaRequest::through(net, [0.1, 0.1]);
        let mut drill = ViaRequest::through(net, [20.0, 20.0]);
        drill.drill = drill.diameter;
        let mut blind = ViaRequest::through(net, [20.0, 20.0]);
        blind.span = ViaSpan::Between {
            from: CopperLayer::Top,
            to: CopperLayer::Inner(1),
        };
        for via in [
            outside,
            drill,
            blind,
            ViaRequest::through(9_999, [20.0, 20.0]),
        ] {
            assert!(layout(&mut engine, LayoutCommand::AddVia { via }).is_err());
        }
        assert_eq!(engine.project(), &before);
        Ok(())
    }

    #[test]
    fn the_stackup_is_set_and_checked_and_names_its_layers() -> Result<()> {
        let mut engine = Engine::demo()?;
        assert_eq!(engine.project().stackup, Stackup::default());
        let four = Stackup::preset(4)?;
        assert_eq!(
            four.layers(),
            [
                CopperLayer::Top,
                CopperLayer::Inner(1),
                CopperLayer::Inner(2),
                CopperLayer::Bottom
            ]
        );
        layout(&mut engine, LayoutCommand::SetStackup(four.clone()))?;
        assert_eq!(engine.project().stackup, four);
        let mut three = four.clone();
        three.copper.pop();
        assert!(layout(&mut engine, LayoutCommand::SetStackup(three)).is_err());
        assert_eq!(engine.project().stackup, four);
        Ok(())
    }

    #[test]
    fn stackup_heights_stack_from_the_bottom_dielectric_face() -> Result<()> {
        let missing = || Error::Invalid("missing layer".into());
        let two = Stackup::default();
        assert_eq!(two.copper_span(CopperLayer::Bottom), Some((-0.035, 0.0)));
        let top = two.copper_span(CopperLayer::Top).ok_or_else(missing)?;
        assert!((top.0 - 1.6).abs() < 1e-12 && (top.1 - 1.635).abs() < 1e-12);
        let four = Stackup::preset(4)?;
        let inner = four
            .copper_span(CopperLayer::Inner(2))
            .ok_or_else(missing)?;
        assert!((inner.1 - 0.2).abs() < 1e-12, "{inner:?}");
        assert_eq!(four.copper_span(CopperLayer::Inner(3)), None);
        assert!((four.thickness_mm() - 1.74).abs() < 1e-12);
        Ok(())
    }
}
