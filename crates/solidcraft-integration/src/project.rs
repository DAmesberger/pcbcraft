//! Electronics → native Solidcraft. Parametric pads, traces and envelopes are
//! ordinary native features and planar joints, editable without Pcbcraft.
use crate::copper::Geometry;
use crate::{Error, Result, call, id};
use pcbcraft_core::{Layer, Part, Project};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use solidcraft_engine::Session;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Source {
    pub pick_id: u64,
    pub instance: Option<u64>,
    pub pad: Option<u64>,
    pub track: Option<u64>,
    pub net: Option<String>,
    pub layer: Option<Layer>,
    pub label: String,
    pub component: u64,
}
pub struct NativeModel {
    session: Session,
    pub geometry: Vec<Geometry>,
    pub sources: Vec<Source>,
}
impl NativeModel {
    pub fn save(&self, path: &str) -> Result<()> {
        crate::validate_path(path)?;
        if std::path::Path::new(path).try_exists()? {
            return Err(Error::Invalid(
                "output exists; choose a new .solidcraft filename".into(),
            ));
        }
        let mut copy = Session::new((*self.session.doc).clone());
        call(
            &mut copy,
            "file.save_as",
            json!({"path":path,"format":"container","cache":false,"preview":false}),
        )?;
        Ok(())
    }
}
fn param(s: &mut Session, name: &str, value: f64, unit: &str) -> Result<()> {
    call(
        s,
        "parameters.add",
        json!({"name":name,"expression":format!("{value} {unit}")}),
    )?;
    Ok(())
}
fn component(s: &mut Session, name: &str) -> Result<(u64, u64)> {
    call(s, "component.activate", json!({"component":"root"}))?;
    let value = call(s, "component.create", json!({"name":name,"grounded":false}))?;
    Ok((id(&value, "component")?, id(&value, "occurrence")?))
}
fn box_body(s: &mut Session, name: &str, size: [Value; 3], corner: Value) -> Result<()> {
    call(
        s,
        "solid.box",
        json!({"body_name":name,"name":name,"length":size[0],"width":size[1],"height":size[2],"corner":corner}),
    )?;
    Ok(())
}
fn joint(
    s: &mut Session,
    board: u64,
    occurrence: u64,
    x: Value,
    y: Value,
    angle: Value,
    layer: Layer,
) -> Result<()> {
    call(
        s,
        "joint.create",
        json!({"type":"planar","name":format!("placement-{occurrence}"),
        "a":{"occurrence":board,"point":[0,0,0],"z":[0,0,1],"x":[1,0,0]},
        "b":{"occurrence":occurrence,"point":[0,0,0],"z":[0,0,1],"x":[1,0,0]},
        "flip":layer==Layer::Top,"angle":0,"offset":if layer==Layer::Top{"board_thickness"}else{"0 mm"},"values":[x,y,angle]}),
    )?;
    Ok(())
}
fn finish(mut session: Session, sources: Vec<Source>) -> Result<NativeModel> {
    call(
        &mut session,
        "document.metadata.set",
        json!({"values":{"org.pcbcraft.electronics":{"version":1,"sources":sources}}}),
    )?;
    if let Some(error) = session.model.results.iter().find(|f| f.error.is_some()) {
        return Err(Error::Verification(format!(
            "{}: {:?}",
            error.name, error.error
        )));
    }
    let mut geometry = vec![];
    for body in &session.world_state().bodies {
        let m = body
            .measure()
            .map_err(|e| Error::Verification(e.to_string()))?;
        geometry.push(Geometry {
            item: sources
                .iter()
                .find(|s| s.component == body.component)
                .map(|s| s.pick_id),
            mesh: body.mesh(),
            volume: m.volume,
            min: [m.bbox.min.x, m.bbox.min.y, m.bbox.min.z],
            max: [m.bbox.max.x, m.bbox.max.y, m.bbox.max.z],
        });
    }
    Ok(NativeModel {
        session,
        geometry,
        sources,
    })
}

pub fn build(project: &Project) -> Result<NativeModel> {
    pcbcraft_core::validate(project).map_err(|e| Error::Invalid(e.to_string()))?;
    let mut s = Session::default();
    call(&mut s, "file.new", json!({"name":project.name}))?;
    for (name, value) in [
        ("board_width", project.board_size[0]),
        ("board_depth", project.board_size[1]),
        ("board_thickness", 1.6),
        ("copper_thickness", 0.035),
    ] {
        param(&mut s, name, value, "mm")?;
    }
    let board = call(
        &mut s,
        "component.create",
        json!({"name":"PCB substrate","grounded":true}),
    )?;
    let board_occurrence = id(&board, "occurrence")?;
    box_body(
        &mut s,
        "Substrate",
        [
            json!("board_width"),
            json!("board_depth"),
            json!("board_thickness"),
        ],
        json!([0, 0, 0]),
    )?;
    call(
        &mut s,
        "appearance.assign",
        json!({"bodies":["Substrate"],"color":"#236447"}),
    )?;
    let mut sources = vec![];
    for placement in &project.placements {
        let instance = project
            .instance(placement.instance)
            .ok_or_else(|| Error::Invalid("missing instance".into()))?;
        let part = project
            .part(instance.part)
            .ok_or_else(|| Error::Invalid("missing part snapshot".into()))?;
        let prefix = format!("instance_{}", instance.id);
        for (suffix, value, unit) in [
            ("x", placement.position[0], "mm"),
            ("y", placement.position[1], "mm"),
            ("angle", placement.rotation_deg, "deg"),
        ] {
            param(&mut s, &format!("{prefix}_{suffix}"), value, unit)?;
        }
        let (component, occurrence) =
            component(&mut s, &format!("{} envelope", instance.reference))?;
        let size = part.package.body_size;
        for (axis, value) in ["length", "width", "height"].into_iter().zip(size) {
            param(&mut s, &format!("{prefix}_{axis}"), value, "mm")?;
        }
        box_body(
            &mut s,
            &format!("{} envelope", instance.reference),
            [
                json!(format!("{prefix}_length")),
                json!(format!("{prefix}_width")),
                json!(format!("{prefix}_height")),
            ],
            json!([
                format!(
                    "- {prefix}_length / 2 + {} mm",
                    part.package.model_offset[0]
                ),
                format!("- {prefix}_width / 2 + {} mm", part.package.model_offset[1]),
                format!("copper_thickness + {} mm", part.package.model_offset[2])
            ]),
        )?;
        joint(
            &mut s,
            board_occurrence,
            occurrence,
            json!(format!("{prefix}_x")),
            json!(format!("{prefix}_y")),
            json!(format!("{prefix}_angle")),
            placement.layer,
        )?;
        sources.push(Source {
            pick_id: sources.len() as u64 + 1,
            instance: Some(instance.id),
            pad: None,
            track: None,
            net: None,
            layer: Some(placement.layer),
            label: format!("{} envelope", instance.reference),
            component,
        });
    }
    for pad in pcbcraft_core::layout::pads(project) {
        let instance = project
            .instance(pad.instance)
            .ok_or_else(|| Error::Invalid("missing pad instance".into()))?;
        let part = project
            .part(instance.part)
            .ok_or_else(|| Error::Invalid("missing pad definition".into()))?;
        let local = part
            .footprint
            .pads
            .iter()
            .find(|p| p.id == pad.pad)
            .ok_or_else(|| Error::Invalid("missing pad".into()))?;
        let prefix = format!("instance_{}", pad.instance);
        let pp = format!("pad_{}_{}", pad.instance, pad.pad);
        let dx = local.position[0] - part.footprint.datum_offset[0];
        let dy = (local.position[1] - part.footprint.datum_offset[1])
            * if pad.layer == Layer::Top { 1.0 } else { -1.0 };
        param(&mut s, &format!("{pp}_length"), pad.size[0], "mm")?;
        param(&mut s, &format!("{pp}_width"), pad.size[1], "mm")?;
        let (component, occurrence) = component(&mut s, &pp)?;
        box_body(
            &mut s,
            &pp,
            [
                json!(format!("{pp}_length")),
                json!(format!("{pp}_width")),
                json!("copper_thickness"),
            ],
            json!([format!("-{pp}_length / 2"), format!("-{pp}_width / 2"), 0]),
        )?;
        let x =
            format!("{prefix}_x + cos({prefix}_angle) * {dx} mm - sin({prefix}_angle) * {dy} mm");
        let y =
            format!("{prefix}_y + sin({prefix}_angle) * {dx} mm + cos({prefix}_angle) * {dy} mm");
        joint(
            &mut s,
            board_occurrence,
            occurrence,
            json!(x),
            json!(y),
            json!(format!("{prefix}_angle")),
            pad.layer,
        )?;
        sources.push(Source {
            pick_id: sources.len() as u64 + 1,
            instance: Some(pad.instance),
            pad: Some(pad.pad),
            track: None,
            net: Some(pad.net),
            layer: Some(pad.layer),
            label: format!("{} pad {}", instance.reference, pad.pad),
            component,
        });
    }
    for track in &project.tracks {
        let prefix = format!("trace_{}", track.id);
        for (suffix, value) in [
            ("x0", track.start[0]),
            ("y0", track.start[1]),
            ("x1", track.end[0]),
            ("y1", track.end[1]),
            ("width", track.width),
        ] {
            param(&mut s, &format!("{prefix}_{suffix}"), value, "mm")?;
        }
        let (component, occurrence) = component(&mut s, &prefix)?;
        let length = format!("sqrt(({prefix}_x1-{prefix}_x0)^2 + ({prefix}_y1-{prefix}_y0)^2)");
        let angle = format!("atan2({prefix}_y1-{prefix}_y0; {prefix}_x1-{prefix}_x0)");
        box_body(
            &mut s,
            &prefix,
            [
                json!(length),
                json!(format!("{prefix}_width")),
                json!("copper_thickness"),
            ],
            json!([0, format!("-{prefix}_width / 2"), 0]),
        )?;
        joint(
            &mut s,
            board_occurrence,
            occurrence,
            json!(format!("{prefix}_x0")),
            json!(format!("{prefix}_y0")),
            json!(angle),
            track.layer,
        )?;
        sources.push(Source {
            pick_id: sources.len() as u64 + 1,
            instance: None,
            pad: None,
            track: Some(track.id),
            net: Some(track.net.clone()),
            layer: Some(track.layer),
            label: format!("Trace {}", track.id),
            component,
        });
    }
    finish(s, sources)
}

pub fn preview(part: &Part) -> Result<NativeModel> {
    pcbcraft_core::parts::validate_part(part).map_err(|e| Error::Invalid(e.to_string()))?;
    let mut s = Session::default();
    call(
        &mut s,
        "file.new",
        json!({"name":format!("{} — package envelope",part.name)}),
    )?;
    for (name, value) in ["length", "width", "height"]
        .into_iter()
        .zip(part.package.body_size)
    {
        param(&mut s, name, value, "mm")?;
    }
    box_body(
        &mut s,
        "Package envelope",
        [json!("length"), json!("width"), json!("height")],
        json!([
            format!("-length/2 + {} mm", part.package.model_offset[0]),
            format!("-width/2 + {} mm", part.package.model_offset[1]),
            part.package.model_offset[2]
        ]),
    )?;
    finish(s, vec![])
}

#[cfg(test)]
mod tests;
