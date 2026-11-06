//! Pictures of the schematic canvas without a GPU: one egui frame of the
//! canvas, tessellated by egui and filled on the CPU with its font atlas,
//! written as PNG under `target/schematic-pictures/` for a person to look
//! at (pin marks, junction dots, the Wire tool's snap marker).
use super::*;
use egui::epaint::{ClippedPrimitive, Primitive};
use egui::vec2;
use solidcraft_ui_egui::fonts;
use std::collections::HashMap;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const SIZE: f32 = 360.0;

/// Fill egui's tessellated meshes into an RGBA image, `ppp` pixels per
/// point, blending premultiplied colours over a white page.
fn rasterize(
    primitives: &[ClippedPrimitive],
    textures: &HashMap<egui::TextureId, egui::ColorImage>,
    w: usize,
    h: usize,
    ppp: f32,
) -> Vec<u8> {
    let mut px = vec![[0.0_f32; 4]; w * h];
    for p in primitives {
        let Primitive::Mesh(mesh) = &p.primitive else {
            continue;
        };
        let texture = textures.get(&mesh.texture_id);
        let sample = |uv: egui::Pos2| -> [f32; 4] {
            let Some(t) = texture else {
                return [1.0; 4];
            };
            let [tw, th] = t.size;
            let x = ((uv.x * tw as f32) as usize).min(tw.saturating_sub(1));
            let y = ((uv.y * th as f32) as usize).min(th.saturating_sub(1));
            let c = t.pixels[y * tw + x];
            [
                f32::from(c.r()) / 255.0,
                f32::from(c.g()) / 255.0,
                f32::from(c.b()) / 255.0,
                f32::from(c.a()) / 255.0,
            ]
        };
        for tri in mesh.indices.chunks_exact(3) {
            let v: Vec<&egui::epaint::Vertex> = tri
                .iter()
                .filter_map(|i| mesh.vertices.get(*i as usize))
                .collect();
            if v.len() != 3 {
                continue;
            }
            let q = |i: usize| (v[i].pos.x * ppp, v[i].pos.y * ppp);
            let (a, b, c) = (q(0), q(1), q(2));
            let area = (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0);
            if area.abs() < 1e-9 {
                continue;
            }
            let x0 = a.0.min(b.0).min(c.0).floor().max(0.0) as usize;
            let x1 = (a.0.max(b.0).max(c.0).ceil() as usize).min(w);
            let y0 = a.1.min(b.1).min(c.1).floor().max(0.0) as usize;
            let y1 = (a.1.max(b.1).max(c.1).ceil() as usize).min(h);
            for y in y0..y1 {
                for x in x0..x1 {
                    let (sx, sy) = (x as f32 + 0.5, y as f32 + 0.5);
                    let w0 = ((b.0 - sx) * (c.1 - sy) - (b.1 - sy) * (c.0 - sx)) / area;
                    let w1 = ((c.0 - sx) * (a.1 - sy) - (c.1 - sy) * (a.0 - sx)) / area;
                    let w2 = 1.0 - w0 - w1;
                    if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                        continue;
                    }
                    let col = |k: usize| -> [f32; 4] {
                        let c = v[k].color;
                        [
                            f32::from(c.r()) / 255.0,
                            f32::from(c.g()) / 255.0,
                            f32::from(c.b()) / 255.0,
                            f32::from(c.a()) / 255.0,
                        ]
                    };
                    let (c0, c1, c2) = (col(0), col(1), col(2));
                    let uv = egui::pos2(
                        v[0].uv.x * w0 + v[1].uv.x * w1 + v[2].uv.x * w2,
                        v[0].uv.y * w0 + v[1].uv.y * w1 + v[2].uv.y * w2,
                    );
                    let t = sample(uv);
                    let dst = &mut px[y * w + x];
                    let src: [f32; 4] =
                        std::array::from_fn(|i| (c0[i] * w0 + c1[i] * w1 + c2[i] * w2) * t[i]);
                    for i in 0..4 {
                        dst[i] = src[i] + dst[i] * (1.0 - src[3]);
                    }
                }
            }
        }
    }
    px.iter()
        .flat_map(|p| {
            // Over an opaque page in the canvas background already drawn.
            [p[0], p[1], p[2], 1.0].map(|c| (c.clamp(0.0, 1.0) * 255.0) as u8)
        })
        .collect()
}

/// One canvas frame at `ppp` pixels per point, as PNG bytes.
fn picture(
    editor: &mut SchematicEditor,
    engine: &Engine,
    ctx: &egui::Context,
    textures: &mut HashMap<egui::TextureId, egui::ColorImage>,
    events: Vec<egui::Event>,
    ppp: f32,
    extra_dots: &[Point],
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let rect = egui::Rect::from_min_size(Pos2::ZERO, vec2(SIZE, SIZE));
    ctx.set_pixels_per_point(ppp);
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(rect),
            events,
            ..Default::default()
        },
        |ui| {
            ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
                editor.canvas(ui, engine);
                // A trial look: dots on plain bends too.
                let view = symbol_view::View {
                    origin: rect.center() + editor.pan,
                    scale: editor.scale,
                };
                for p in extra_dots {
                    ui.painter().circle_filled(
                        view.screen(*p),
                        dot_radius(editor.scale),
                        Tokens::get().form_success,
                    );
                }
            });
        },
    );
    for (id, deltas) in &output.textures_delta.set {
        for delta in deltas {
            let egui::ImageData::Color(image) = &delta.image;
            let image: &egui::ColorImage = image;
            match delta.pos {
                None => {
                    textures.insert(*id, image.clone());
                }
                Some([x0, y0]) => {
                    if let Some(t) = textures.get_mut(id) {
                        for y in 0..image.size[1] {
                            for x in 0..image.size[0] {
                                if let Some(d) = t.pixels.get_mut((y0 + y) * t.size[0] + x0 + x) {
                                    *d = image.pixels[y * image.size[0] + x];
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    output.textures_delta.clear();
    let primitives = ctx.tessellate(output.shapes, ppp);
    let n = (SIZE * ppp) as usize;
    let rgba = rasterize(&primitives, textures, n, n, ppp);
    Ok(pcbcraft_solidcraft::render::png(n, n, rgba).ok_or("png encoding failed")?)
}

#[test]
#[ignore = "writes schematic canvas pictures"]
fn write_canvas_pictures() -> TestResult {
    let dir =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/schematic-pictures");
    std::fs::create_dir_all(&dir)?;
    let mut engine = Engine::demo()?;
    let first = engine.project().instances[0].clone();
    let mut id = std::collections::BTreeMap::new();
    let put = |engine: &mut Engine,
               id: &mut std::collections::BTreeMap<String, Id>,
               name: &str,
               at: Point|
     -> TestResult {
        engine.apply(
            engine.revision(),
            Operation::Instance(InstanceCommand::Place(NewInstance {
                sheet: None,
                component: first.component,
                designator: Some(name.into()),
                properties: vec![],
                representation: first.representation,
                unit: pcbcraft_core::UnitId(1),
                position: at,
                physical: None,
            })),
        )?;
        let placed = engine
            .project()
            .instance_by_designator(name)
            .map(|i| i.id)
            .ok_or("placed")?;
        id.insert(name.to_owned(), placed);
        Ok(())
    };
    let ep = |instance: Id, terminal| Endpoint {
        instance,
        terminal: pcbcraft_core::TerminalId(terminal),
    };
    let wire = |engine: &mut Engine, a: Endpoint, b: Endpoint| -> TestResult {
        engine.apply(
            engine.revision(),
            Operation::Schematic(SchematicCommand::Connect {
                from: a.into(),
                to: b.into(),
            }),
        )?;
        Ok(())
    };
    // The founder's case: a wire down the ends of a column of pins.
    put(&mut engine, &mut id, "R10", [0.0, -400.0])?;
    put(&mut engine, &mut id, "R11", [0.0, -430.0])?;
    wire(&mut engine, ep(id["R10"], 1), ep(id["R11"], 1))?;
    for (k, name) in ["R12", "R13", "R14"].into_iter().enumerate() {
        put(&mut engine, &mut id, name, [0.0, -407.5 - 7.5 * k as f64])?;
    }
    // A wire that bends around two pins in its way.
    put(&mut engine, &mut id, "R20", [30.0, -400.0])?;
    put(&mut engine, &mut id, "R21", [60.0, -420.0])?;
    put(&mut engine, &mut id, "R22", [60.0, -400.0])?;
    put(&mut engine, &mut id, "R23", [40.0, -420.0])?;
    wire(&mut engine, ep(id["R20"], 2), ep(id["R21"], 1))?;
    // A plain crossing, and one joined by a junction point.
    put(&mut engine, &mut id, "R30", [30.0, -440.0])?;
    put(&mut engine, &mut id, "R31", [60.0, -440.0])?;
    put(&mut engine, &mut id, "R32", [45.0, -432.0])?;
    put(&mut engine, &mut id, "R33", [45.0, -448.0])?;
    wire(&mut engine, ep(id["R30"], 2), ep(id["R31"], 1))?;
    wire(&mut engine, ep(id["R32"], 1), ep(id["R33"], 1))?;
    wire(&mut engine, ep(id["R32"], 2), ep(id["R33"], 2))?;
    engine.apply(
        engine.revision(),
        Operation::Schematic(SchematicCommand::PlaceJunction {
            sheet: None,
            position: [50.0, -440.0],
        }),
    )?;
    engine.apply(
        engine.revision(),
        Operation::Instance(InstanceCommand::SetNoConnect {
            instance: id["R12"],
            terminals: vec![pcbcraft_core::TerminalId(2)],
            no_connect: true,
        }),
    )?;
    // Plain bends: junctions with two wires.
    let project = engine.project();
    let bends: Vec<Point> = project
        .junctions
        .iter()
        .filter(|j| project.wires_at(WireEnd::junction(j.id)).count() == 2 && !j.placed)
        .map(|j| j.position)
        .collect();
    let centre = [30.0, -424.0];
    let ctx = egui::Context::default();
    fonts::setup_bundled_fonts(&ctx);
    let mut textures = HashMap::new();
    let hover_at = schematic::pin_position(engine.project(), ep(id["R13"], 2)).ok_or("pin")?;
    for (scale, name) in [(5.0, "overview"), (12.0, "normal")] {
        let mut editor = SchematicEditor {
            scale,
            pan: vec2(-(centre[0] as f32) * scale, centre[1] as f32 * scale),
            ..Default::default()
        };
        editor.on_tool("wire", &engine);
        let view = symbol_view::View {
            origin: Pos2::new(SIZE / 2.0, SIZE / 2.0) + editor.pan,
            scale,
        };
        let hover = vec![egui::Event::PointerMoved(
            view.screen(hover_at) + vec2(6.0, -5.0),
        )];
        let _ = picture(&mut editor, &engine, &ctx, &mut textures, vec![], 2.0, &[])?;
        let png = picture(
            &mut editor,
            &engine,
            &ctx,
            &mut textures,
            hover.clone(),
            2.0,
            &[],
        )?;
        std::fs::write(dir.join(format!("canvas-{name}.png")), png)?;
        let png = picture(
            &mut editor,
            &engine,
            &ctx,
            &mut textures,
            hover,
            2.0,
            &bends,
        )?;
        std::fs::write(dir.join(format!("canvas-{name}-bend-dots.png")), png)?;
    }
    let pdf = pcbcraft_io::schematic_pdf::schematic_pdf(engine.project(), &[])?;
    std::fs::write(dir.join("sheet.pdf"), pdf)?;
    Ok(())
}

/// The readability fixes of the e2e UX run (findings 4, 5, 10, 11) as one
/// sheet: the SOIC-14 MCU from a pin table, a resistor turned beside it
/// with its value, a GND port facing a label across a short gap, and a
/// junction point placed on a wire. Written to
/// `target/schematic-pictures/readability-*.png` and `readability.pdf`.
#[test]
#[ignore = "writes schematic canvas pictures"]
fn write_readability_pictures() -> TestResult {
    use pcbcraft_core::library::pinout::{PinoutAccept, PinoutApply, PinoutDocument, PinoutTarget};
    let dir =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/schematic-pictures");
    std::fs::create_dir_all(&dir)?;
    let mut engine = Engine::demo()?;
    // The MCU of finding 5, from a pin table as the e2e run pasted it.
    let names = [
        "PA05", "PA08", "PA09", "PA14", "PA15", "RESET", "SWCLK", "SWDIO", "USB_DM", "USB_DP",
        "GND", "VDD", "SDA", "SCL",
    ];
    let pins: Vec<serde_json::Value> = names
        .iter()
        .enumerate()
        .map(|(k, name)| {
            serde_json::json!({
                "row": format!("r{}", k + 1),
                "number": {"SOIC-14": (k + 1).to_string()},
                "name": name,
                "class": "electrical",
                "functions": [{"label": name.to_lowercase(), "direction": "passive", "signal": "analog"}],
            })
        })
        .collect();
    let document: PinoutDocument = serde_json::from_value(serde_json::json!({
        "format": 1,
        "device": {"name": "MCU ATSAMD11C14A", "reference": {"title": "fixture"}},
        "variants": [{"name": "SOIC-14", "terminal_count": 14}],
        "pins": pins,
    }))?;
    let target = PinoutTarget::New {
        name: "MCU ATSAMD11C14A".into(),
        row: None,
    };
    let base = match engine.query(pcbcraft_core::Query::PinoutPreview {
        target: target.clone(),
        document: Box::new(document.clone()),
    })? {
        pcbcraft_core::QueryResult::PinoutPlan(plan) => plan.base,
        other => return Err(format!("unexpected {other:?}").into()),
    };
    engine.apply(
        engine.revision(),
        Operation::Library(pcbcraft_core::LibraryCommand::PinoutApply(Box::new(
            PinoutApply {
                target,
                document,
                base,
                accept: PinoutAccept::All,
            },
        ))),
    )?;
    // The component and its symbol: the schematic needs no package.
    let edits: Vec<_> = engine
        .drafts()
        .items
        .iter()
        .filter_map(|d| match &d.definition {
            pcbcraft_core::library::Definition::Component(c) => {
                let mut c = c.clone();
                c.implementations.clear();
                Some(pcbcraft_core::library::DraftEdit {
                    id: d.id,
                    meta: d.meta.clone(),
                    definition: pcbcraft_core::library::Definition::Component(c),
                })
            }
            _ => None,
        })
        .collect();
    engine.apply(
        engine.revision(),
        Operation::Library(pcbcraft_core::LibraryCommand::Edit { edits }),
    )?;
    let drafts: Vec<_> = engine
        .drafts()
        .items
        .iter()
        .filter(|d| {
            matches!(
                d.definition,
                pcbcraft_core::library::Definition::Component(_)
                    | pcbcraft_core::library::Definition::Symbol(_)
            )
        })
        .map(|d| d.id)
        .collect();
    engine.apply(
        engine.revision(),
        Operation::Library(pcbcraft_core::LibraryCommand::Publish {
            drafts,
            close: false,
        }),
    )?;
    let mcu = engine
        .project()
        .library
        .latest_components()
        .into_iter()
        .find(|c| c.meta.name == "MCU ATSAMD11C14A")
        .ok_or("mcu")?;
    let mcu_ref = pcbcraft_core::ComponentRef::new(mcu.id, mcu.revision);
    let symbol = *mcu.definition.representations.first().ok_or("symbol")?;
    let terminal = |name: &str| {
        mcu.definition
            .terminals
            .iter()
            .find(|t| t.name == name)
            .map(|t| t.id)
            .unwrap_or(pcbcraft_core::TerminalId(0))
    };
    let (swclk, swdio) = (terminal("SWCLK"), terminal("SWDIO"));
    let resistor = engine.project().instances[0].clone();
    let place = |engine: &mut Engine,
                 component,
                 representation,
                 properties: Vec<pcbcraft_core::fields::PropertyEdit>,
                 name: &str,
                 at: Point|
     -> Result<Id, Box<dyn std::error::Error>> {
        engine.apply(
            engine.revision(),
            Operation::Instance(InstanceCommand::Place(NewInstance {
                sheet: None,
                component,
                designator: Some(name.into()),
                properties,
                representation,
                unit: pcbcraft_core::UnitId(1),
                position: at,
                physical: None,
            })),
        )?;
        Ok(engine
            .project()
            .instance_by_designator(name)
            .map(|i| i.id)
            .ok_or("placed")?)
    };
    let u2 = place(&mut engine, mcu_ref, symbol.id, vec![], "U2", [40.0, -40.0])?;
    let value =
        resistor
            .properties
            .iter()
            .map(|(field, value)| pcbcraft_core::fields::PropertyEdit {
                field: *field,
                input: match value {
                    pcbcraft_core::fields::FieldOverride::Assigned(v) => {
                        pcbcraft_core::fields::PropertyInput::Value(v.clone())
                    }
                    _ => pcbcraft_core::fields::PropertyInput::Inherit,
                },
            });
    let properties: Vec<_> = value.collect();
    // Finding 4: a resistor turned a quarter beside the MCU's left pins.
    let r3 = place(
        &mut engine,
        resistor.component,
        resistor.representation,
        properties.clone(),
        "R3",
        [20.0, -35.0],
    )?;
    let occurrence = engine
        .project()
        .occurrences_of(r3)
        .next()
        .map(|o| o.id)
        .ok_or("R3")?;
    engine.apply(
        engine.revision(),
        Operation::Schematic(SchematicCommand::OrientUnit {
            occurrence,
            rotation: pcbcraft_core::library::Rotation::R90,
            mirror: false,
        }),
    )?;
    // Finding 11: a resistor whose right pin ends 5 mm from U2 pin 7.
    let pin7 = schematic::pin_position(
        engine.project(),
        Endpoint {
            instance: u2,
            terminal: swclk,
        },
    )
    .ok_or("pin 7")?;
    let r4 = place(
        &mut engine,
        resistor.component,
        resistor.representation,
        properties,
        "R4",
        [pin7[0] - 10.0, pin7[1]],
    )?;
    for (name, kind, instance, terminal) in [
        (
            "GND",
            pcbcraft_core::model::LabelKind::Power {
                style: pcbcraft_core::model::PowerStyle::Ground,
            },
            r4,
            pcbcraft_core::TerminalId(2),
        ),
        ("SWCLK", pcbcraft_core::model::LabelKind::Local, u2, swclk),
        ("SWDIO", pcbcraft_core::model::LabelKind::Local, u2, swdio),
    ] {
        let at = schematic::pin_position(engine.project(), Endpoint { instance, terminal })
            .ok_or("pin")?;
        engine.apply(
            engine.revision(),
            Operation::Schematic(SchematicCommand::PlaceLabel {
                sheet: None,
                name: name.into(),
                kind,
                position: at,
                rotation: Default::default(),
                attach: Some(WireTarget::Terminal(Endpoint { instance, terminal })),
            }),
        )?;
    }
    // Finding 10: a junction point placed on the demo's wire.
    let wire = engine.project().wires.first().cloned().ok_or("wire")?;
    let a = schematic::end_position(engine.project(), wire.from).ok_or("end")?;
    let b = schematic::end_position(engine.project(), wire.to).ok_or("end")?;
    let mid = [((a[0] + b[0]) / 2.0 / 2.5).round() * 2.5, a[1]];
    engine.apply(
        engine.revision(),
        Operation::Schematic(SchematicCommand::PlaceJunction {
            sheet: None,
            position: mid,
        }),
    )?;
    let ctx = egui::Context::default();
    fonts::setup_bundled_fonts(&ctx);
    let mut textures = HashMap::new();
    for (centre, scale, name) in [([35.0, -40.0], 7.0, "mcu"), ([0.0, 0.0], 12.0, "junction")] {
        let mut editor = SchematicEditor {
            scale,
            pan: vec2(-(centre[0] as f32) * scale, centre[1] as f32 * scale),
            ..Default::default()
        };
        let _ = picture(&mut editor, &engine, &ctx, &mut textures, vec![], 2.0, &[])?;
        let png = picture(&mut editor, &engine, &ctx, &mut textures, vec![], 2.0, &[])?;
        std::fs::write(dir.join(format!("readability-{name}.png")), png)?;
    }
    let pdf = pcbcraft_io::schematic_pdf::schematic_pdf(engine.project(), &[])?;
    std::fs::write(dir.join("readability.pdf"), pdf)?;
    Ok(())
}
