use super::*;

fn box_body(
    s: &mut Session,
    name: &str,
    corner: Value,
    length: Value,
    width: Value,
    height: Value,
) -> Result<()> {
    call(
        s,
        "solid.box",
        json!({"name":name,"body_name":name,"corner":corner,"length":length,"width":width,"height":height}),
    )?;
    Ok(())
}
fn hole(s: &mut Session, name: &str, base: Value, radius: f64, height: Value) -> Result<()> {
    call(
        s,
        "solid.cylinder",
        json!({"base":base,"radius":radius,"height":height,"operation":"cut","targets":[name]}),
    )?;
    Ok(())
}
pub(super) fn fixture() -> Result<Session> {
    let mut s = Session::default();
    call(
        &mut s,
        "file.new",
        json!({"name":"Pcbcraft copper acceptance fixture"}),
    )?;
    for (name, expression) in [
        ("board_thickness", "1.6 mm"),
        ("copper_thickness", "0.035 mm"),
        ("trace_width", "0.5 mm"),
    ] {
        call(
            &mut s,
            "parameters.add",
            json!({"name":name,"expression":expression}),
        )?;
    }
    call(
        &mut s,
        "component.create",
        json!({"name":"Substrate","grounded":true}),
    )?;
    box_body(
        &mut s,
        "Substrate",
        json!([0, 0, 0]),
        json!(40),
        json!(30),
        json!("board_thickness"),
    )?;
    hole(
        &mut s,
        "Substrate",
        json!([4, 5, 0]),
        1.5,
        json!("board_thickness"),
    )?;
    hole(
        &mut s,
        "Substrate",
        json!([18.25, 20, 0]),
        0.5,
        json!("board_thickness"),
    )?;
    call(
        &mut s,
        "appearance.assign",
        json!({"bodies":["Substrate"],"color":"#236447"}),
    )?;
    let mut items = vec![];
    for (id, label, net, layer, kind) in [
        (1, "Signal trace A", "SIGNAL", "Top", "Trace"),
        (2, "Signal trace B", "SIGNAL", "Top", "Trace"),
        (3, "Input pad", "SIGNAL", "Top", "Pad"),
        (4, "Through via", "SIGNAL", "Through", "Via"),
        (5, "Disconnected island", "SIGNAL", "Top", "Trace"),
        (6, "Ground zone", "GND", "Top", "Zone"),
        (7, "Bottom signal", "SIGNAL", "Bottom", "Trace"),
    ] {
        call(&mut s, "component.activate", json!({"component":"root"}))?;
        let reply = call(
            &mut s,
            "component.create",
            json!({"name":format!("pcb-item-{id}"),"grounded":true}),
        )?;
        items.push(Item {
            id,
            label: label.into(),
            net: net.into(),
            layer: layer.into(),
            kind: kind.into(),
            component: crate::id(&reply, "component")?,
        });
        let name = format!("copper-{id}");
        match id {
            1 => box_body(
                &mut s,
                &name,
                json!([8, "8 mm - trace_width / 2", "board_thickness"]),
                json!(10),
                json!("trace_width"),
                json!("copper_thickness"),
            )?,
            2 => box_body(
                &mut s,
                &name,
                json!([18, 7.75, "board_thickness"]),
                json!(0.5),
                json!(11.45),
                json!("copper_thickness"),
            )?,
            3 => box_body(
                &mut s,
                &name,
                json!([6, 7, "board_thickness"]),
                json!(2),
                json!(2),
                json!("copper_thickness"),
            )?,
            4 => {
                // Lands and barrel only meet at their end faces: no overlapping volumes.
                for (suffix, z) in [("top", "board_thickness"), ("bottom", "-copper_thickness")] {
                    let land = format!("{name}-{suffix}");
                    box_body(
                        &mut s,
                        &land,
                        json!([17.45, 19.2, z]),
                        json!(1.6),
                        json!(1.6),
                        json!("copper_thickness"),
                    )?;
                    hole(
                        &mut s,
                        &land,
                        json!([18.25, 20, z]),
                        0.3,
                        json!("copper_thickness"),
                    )?;
                }
                call(
                    &mut s,
                    "solid.cylinder",
                    json!({"body_name":name,"base":[18.25,20,0],"radius":0.5,"height":"board_thickness"}),
                )?;
                hole(
                    &mut s,
                    &name,
                    json!([18.25, 20, 0]),
                    0.3,
                    json!("board_thickness"),
                )?;
            }
            5 => box_body(
                &mut s,
                &name,
                json!([7, 25, "board_thickness"]),
                json!(5),
                json!(0.5),
                json!("copper_thickness"),
            )?,
            6 => {
                box_body(
                    &mut s,
                    &name,
                    json!([26, 6, "board_thickness"]),
                    json!(10),
                    json!(10),
                    json!("copper_thickness"),
                )?;
                hole(
                    &mut s,
                    &name,
                    json!([31, 11, "board_thickness"]),
                    2.0,
                    json!("copper_thickness"),
                )?;
            }
            7 => box_body(
                &mut s,
                &name,
                json!([19.05, 19.75, "-copper_thickness"]),
                json!(10.95),
                json!(0.5),
                json!("copper_thickness"),
            )?,
            _ => return Err(Error::Invalid("unknown fixture item".into())),
        }
        let mut bodies = vec![name.clone()];
        if id == 4 {
            bodies.extend([format!("{name}-top"), format!("{name}-bottom")]);
        }
        call(
            &mut s,
            "appearance.assign",
            json!({"bodies":bodies,"color":if layer=="Bottom" {"#56a6cf"} else {"#c88c43"}}),
        )?;
    }
    call(&mut s, "component.activate", json!({"component":"root"}))?;
    call(
        &mut s,
        "document.metadata.set",
        json!({"values":{META:Manifest { version:1,items }}}),
    )?;
    Ok(s)
}
