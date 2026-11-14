//! The chip generator: two-terminal chip parts (resistors, capacitors,
//! inductors, ferrite beads and LEDs) as a symbol, package, footprint and
//! 3D model.
//!
//! Lands follow the host's estimate for chip packages (an original rule of
//! thumb, not a land-pattern standard): a land of termination + 0.6 mm by
//! width + 0.2 mm centred on each termination, its heel trimmed so the two
//! lands stay the project's least chip land gap apart. A preset may give the
//! manufacturer's recommended lands instead (`lands: explicit`).
//!
//! The presets are the chip starter table, in nm, with its citations.
#![cfg_attr(target_arch = "wasm32", no_std)]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use sparksmith_generator_sdk::args::{Args, checked, problem};
use sparksmith_generator_sdk::draw::{self, Drawing, LEFT, RIGHT};
use sparksmith_generator_sdk::land::{self, Box2};
use sparksmith_generator_sdk::units::{DEG, MM, div_round, mm_text};
use sparksmith_generator_sdk::v1::{
    self, BindingEntry, Capability, Choice, Component, Description, Direction, Effect, Fill,
    Footprint, Generation, Grammar, InstanceField, Lead, Opening, Package, PackageGeometry, Pad,
    PadShape, Param, ParamType, ParamValue, PartOffer, Preset, Primitive, Problem, Request, Rule,
    Scope, Side, Symbol, Terminal,
};
use sparksmith_generator_sdk::{Generator, export_generator};

/// The starter rows, in nm, with their citations.
const PRESETS: &str = include_str!("../presets.json");

/// Estimated land: termination plus this along the body…
const LAND_EXTRA_LENGTH_NM: i64 = 600_000;
/// …and the body width plus this across it.
const LAND_EXTRA_WIDTH_NM: i64 = 200_000;

/// The generator's version (`Description::version`): 1, the first with a
/// 3D model.
const VERSION: u32 = 1;

pub struct Chip;

export_generator!(Chip);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Part {
    Resistor,
    Capacitor,
    Inductor,
    Ferrite,
    Led,
}

impl Part {
    const ALL: [(Self, &'static str, &'static str); 5] = [
        (Self::Resistor, "resistor", "Resistor"),
        (Self::Capacitor, "capacitor", "Capacitor"),
        (Self::Inductor, "inductor", "Inductor"),
        (Self::Ferrite, "ferrite", "Ferrite bead"),
        (Self::Led, "led", "LED"),
    ];

    fn from_key(key: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .find(|(_, k, _)| *k == key)
            .map(|(p, _, _)| *p)
    }

    /// The part in a name, e.g. "Chip resistor 1.60 × 0.80 mm".
    fn noun(self) -> &'static str {
        match self {
            Self::Resistor => "resistor",
            Self::Capacitor => "capacitor",
            Self::Inductor => "inductor",
            Self::Ferrite => "ferrite bead",
            Self::Led => "LED",
        }
    }

    fn prefix(self) -> &'static str {
        match self {
            Self::Resistor => "R",
            Self::Capacitor => "C",
            Self::Inductor => "L",
            Self::Ferrite => "FB",
            Self::Led => "D",
        }
    }
}

fn length(key: &str, label: &str, min_nm: i64, max_nm: i64, default: i64, group: &str) -> Param {
    Param {
        key: key.into(),
        label: label.into(),
        help: None,
        ty: ParamType::Length {
            min_nm,
            max_nm,
            step_nm: None,
        },
        default: ParamValue::Int(default),
        group: Some(group.into()),
        context: None,
    }
}

fn choices(pairs: &[(&str, &str)]) -> Vec<Choice> {
    pairs
        .iter()
        .map(|(key, label)| Choice {
            key: (*key).into(),
            label: (*label).into(),
        })
        .collect()
}

fn enumeration(
    key: &str,
    label: &str,
    options: &[(&str, &str)],
    default: &str,
    group: &str,
) -> Param {
    Param {
        key: key.into(),
        label: label.into(),
        help: None,
        ty: ParamType::Enum {
            choices: choices(options),
        },
        default: ParamValue::Text(default.into()),
        group: Some(group.into()),
        context: None,
    }
}

/// The full description. Unreadable presets are a build bug that the
/// golden and preset tests catch; describe still answers, without them.
fn description() -> Description {
    description_with(sparksmith_generator_sdk::presets(PRESETS).unwrap_or_default())
}

/// The description, with `presets` (generate checks parameters against it
/// without reading the preset table, which costs most of describe's work).
fn description_with(presets: Vec<Preset>) -> Description {
    let parts: Vec<(&str, &str)> = Part::ALL.iter().map(|(_, k, l)| (*k, *l)).collect();
    let mut gap = length("land_gap_min", "Least land gap", 0, MM, 200_000, "lands");
    gap.help =
        Some("The least gap between the two lands; the project's land rules fill it.".into());
    gap.context = Some("project.land.chip_gap_min".into());
    let mut style = enumeration(
        "resistor_style",
        "Resistor symbol",
        &[("iec", "IEC (rectangle)"), ("us", "US (zigzag)")],
        "iec",
        "symbol",
    );
    style.context = Some("project.symbols.resistor_style".into());
    let mut land_mode = enumeration(
        "lands",
        "Lands",
        &[
            ("estimated", "Estimated from the body"),
            ("explicit", "Given"),
        ],
        "estimated",
        "lands",
    );
    land_mode.help = Some("Given lands use land length, width and pitch as stated.".into());
    let when = |part: &str| {
        let mut m = v1::Params::new();
        m.insert("part".into(), ParamValue::Text(part.into()));
        m
    };
    let params = vec![
        enumeration("part", "Part", &parts, "resistor", "part"),
        length("length", "Body length", 200_000, 8 * MM, 1_600_000, "body"),
        length("width", "Body width", 100_000, 6 * MM, 800_000, "body"),
        length("height", "Body height", 50_000, 3 * MM, 450_000, "body"),
        length(
            "termination",
            "Termination length",
            50_000,
            2 * MM,
            300_000,
            "body",
        ),
        land_mode,
        length(
            "land_length",
            "Land length",
            50_000,
            5 * MM,
            900_000,
            "lands",
        ),
        length(
            "land_width",
            "Land width",
            50_000,
            6 * MM,
            1_000_000,
            "lands",
        ),
        length(
            "land_pitch",
            "Land pitch",
            100_000,
            10 * MM,
            1_300_000,
            "lands",
        ),
        gap,
        style,
    ];
    let rules = vec![
        Rule {
            text: "2 × termination < body length".into(),
            fields: vec!["termination".into(), "length".into()],
        },
        Rule {
            text: "Given lands: land length < land pitch, so the lands do not touch".into(),
            fields: vec!["lands".into(), "land_length".into(), "land_pitch".into()],
        },
    ];
    let offer = PartOffer {
        families: vec!["chip".into()],
        scopes: vec![Scope::Component, Scope::Package],
        grammar: vec![Grammar {
            scope: Scope::Component,
            patterns: vec!["{part} {size:code}".into(), "{value} {size:code}".into()],
            examples: vec!["resistor 0603".into(), "10k 0603".into(), "led 0805".into()],
        }],
        instance_fields: vec![
            InstanceField {
                standard_field: "value".into(),
                when: v1::Params::new(),
            },
            InstanceField {
                standard_field: "colour".into(),
                when: when("led"),
            },
        ],
    };
    Description {
        abi: v1::ABI,
        id: "chip".into(),
        version: VERSION,
        title: "Chip passives and LEDs".into(),
        licence: "MIT OR Apache-2.0".into(),
        deterministic: true,
        capabilities: vec![Capability {
            name: v1::GENERATE_PART.into(),
            effect: Effect::None,
            title: Some("Make a chip part".into()),
            params,
            rules,
            presets,
            part: Some(offer),
        }],
        needs: Vec::new(),
    }
}

/// A chip's dimensions, checked.
struct Dims {
    part: Part,
    length: i64,
    width: i64,
    height: i64,
    termination: i64,
}

fn dims(args: &Args<'_>) -> Result<Dims, Vec<Problem>> {
    let one = |p: Problem| vec![p];
    let part_key = args.text("part").map_err(one)?;
    let part = Part::from_key(part_key).ok_or_else(|| {
        vec![problem(
            "param.out_of_range",
            "part",
            format!("Part \"{part_key}\" is not one this generator makes."),
        )]
    })?;
    let d = Dims {
        part,
        length: args.int("length").map_err(one)?,
        width: args.int("width").map_err(one)?,
        height: args.int("height").map_err(one)?,
        termination: args.int("termination").map_err(one)?,
    };
    if 2 * d.termination >= d.length {
        return Err(vec![problem(
            "param.inconsistent",
            "termination",
            format!(
                "The two terminations (2 × {}) are not shorter than the body ({}).",
                mm_text(d.termination),
                mm_text(d.length)
            ),
        )]);
    }
    Ok(d)
}

/// Land centre x (of the +x land) and size.
fn lands(args: &Args<'_>, d: &Dims) -> Result<(i64, [i64; 2], bool), Vec<Problem>> {
    let one = |p: Problem| vec![p];
    let overflow = || {
        vec![problem(
            "param.out_of_range",
            "length",
            "The dimensions are too large to compute lands.".into(),
        )]
    };
    if args.text("lands").map_err(one)? == "explicit" {
        let size = [
            args.int("land_length").map_err(one)?,
            args.int("land_width").map_err(one)?,
        ];
        let pitch = args.int("land_pitch").map_err(one)?;
        if size[0] >= pitch {
            return Err(vec![problem(
                "param.inconsistent",
                "land_length",
                format!(
                    "Lands of {} at a pitch of {} would touch.",
                    mm_text(size[0]),
                    mm_text(pitch)
                ),
            )]);
        }
        let x = div_round(pitch, 2).ok_or_else(overflow)?;
        return Ok((x, size, false));
    }
    let gap = args.int("land_gap_min").map_err(one)?;
    let x = div_round(d.length - d.termination, 2).ok_or_else(overflow)?;
    let mut size = [
        d.termination + LAND_EXTRA_LENGTH_NM,
        d.width + LAND_EXTRA_WIDTH_NM,
    ];
    // Keep the inner edges at least the least gap apart: trim the heel,
    // never the toe.
    let half = div_round(size[0], 2).ok_or_else(overflow)?;
    let outer = x + half;
    let inner = (x - half).max(div_round(gap, 2).ok_or_else(overflow)?);
    let mut centre = x;
    if inner < outer {
        size[0] = outer - inner;
        centre = div_round(outer + inner, 2).ok_or_else(overflow)?;
    }
    Ok((centre, size, true))
}

fn pad(key: &str, at: [i64; 2], size: [i64; 2]) -> Pad {
    Pad {
        key: key.into(),
        terminal: key.into(),
        shape: PadShape::Rect { size },
        at,
        rot_mdeg: 0,
        side: Side::Top,
        hole: None,
        mask: Opening::Default,
        paste: Opening::Default,
    }
}

fn symbol(part: Part, resistor_style: &str) -> Symbol {
    let mut d = Drawing::default();
    let m = |v: i64| v * MM / 1000; // thousandths of a mm
    match part {
        Part::Resistor => {
            d.two_pins("1", "2", m(2500), m(2500));
            if resistor_style == "us" {
                let mut points = vec![[m(-2500), 0]];
                for k in 0..6i64 {
                    // x = −2.5 + 5 (2k + 1) / 12 mm
                    let x = m(-2500) + div_round(m(5000) * (2 * k + 1), 12).unwrap_or(0);
                    points.push([x, if k % 2 == 0 { m(1000) } else { m(-1000) }]);
                }
                points.push([m(2500), 0]);
                d.open(Primitive::Polyline { points });
            } else {
                d.shape(
                    Primitive::Rect {
                        min: [m(-2500), m(-1000)],
                        max: [m(2500), m(1000)],
                    },
                    draw::STROKE_NM,
                    Fill::Body,
                );
            }
            d.labels(m(1000), m(-1000));
        }
        Part::Capacitor => {
            d.two_pins("1", "2", m(500), m(500));
            d.plate([m(-500), m(-1750)], [m(-500), m(1750)]);
            d.plate([m(500), m(-1750)], [m(500), m(1750)]);
            d.labels(m(1750), m(-1750));
        }
        Part::Inductor => {
            d.two_pins("1", "2", m(2500), m(2500));
            for k in 0..4i64 {
                if let Some(arc) =
                    draw::arc([m(-1875) + k * m(1250), 0], m(625), 180 * DEG, -180 * DEG)
                {
                    d.open(arc);
                }
            }
            d.labels(m(625), m(-500));
        }
        Part::Ferrite => {
            d.two_pins("1", "2", m(2500), m(2500));
            d.line([m(-2500), 0], [m(2500), 0]);
            // A slanted solid block across the line.
            let corners = [[-600, -1600], [600, -1600], [600, 1600], [-600, 1600]];
            let points: Vec<[i64; 2]> = corners
                .iter()
                .filter_map(|[u, v]| draw::rotate_45([m(*u), m(*v)]))
                .collect();
            d.shape(Primitive::Polygon { points }, draw::STROKE_NM, Fill::Solid);
            d.labels(m(1600), m(-1600));
        }
        Part::Led => {
            // Anode (terminal 2) left, cathode (terminal 1) right.
            d.pin("2", [-draw::TWO_TERMINAL_PIN_X_NM, 0], RIGHT, m(3750));
            d.pin("1", [draw::TWO_TERMINAL_PIN_X_NM, 0], LEFT, m(3750));
            d.shape(
                Primitive::Polygon {
                    points: vec![[m(-1250), m(1250)], [m(-1250), m(-1250)], [m(1250), 0]],
                },
                draw::STROKE_NM,
                Fill::Body,
            );
            d.line([m(1250), m(-1250)], [m(1250), m(1250)]);
            // Two arrows leaving the junction, up and to the right.
            for dx in [-500, 500] {
                d.arrow([m(dx - 250), m(1250)], [m(dx + 750), m(2250)]);
            }
            d.labels(m(2500), m(-1250));
        }
    }
    Symbol {
        units: vec![d.unit("A")],
    }
}

/// Edge rounds (est.: tumbled ceramic chips and dipped terminations show
/// 20–80 µm radii), capped by the chip's size in [`model`].
const BODY_EDGE_NM: i64 = 60_000;
const END_EDGE_NM: i64 = 80_000;
/// The overcoat's thickness (est.: thick-film overcoats are 10–40 µm), at
/// most a tenth of the chip height.
const OVERCOAT_NM: i64 = 30_000;
/// The overcoat stops this far in from the long edges (docs/materials.md).
const OVERCOAT_INSET_NM: i64 = 40_000;
/// A chip LED's substrate thickness (Kingbright's APT1608 and APT2012
/// drawings both give 0.25 mm), at most half the height.
const LED_SUBSTRATE_NM: i64 = 250_000;
/// Round of the lens's top edges (est.), capped by the lens's size.
const LENS_EDGE_NM: i64 = 100_000;

/// `n / d` to the nearest nanometre (the divisors here are small and
/// positive, so it never fails).
fn part_of(n: i64, d: i64) -> i64 {
    div_round(n, d).unwrap_or(0)
}

/// The SHA-256 of each part file in the bundle (`parts/`), written by the
/// build script.
mod parts {
    include!(concat!(env!("OUT_DIR"), "/parts.rs"));
}

/// The chip's 3D model: the bundle's part file for the part (a ceramic or
/// ferrite body between two wrapped end terminations; on a thick-film
/// resistor a black overcoat raised on the alumina's top; on a chip LED a
/// thin white substrate and terminations under a clear lens), with every
/// parameter of the file given: the body, the termination length, and the
/// overcoat, substrate and edge rounds the part's size decides. Each
/// termination is centred on its lead's contact ((length − termination) / 2
/// from the middle), lead 1 (pin 1) at −x.
fn model(d: &Dims) -> v1::Model {
    let overcoat = part_of(d.height, 10).min(OVERCOAT_NM);
    let inset = OVERCOAT_INSET_NM.min(part_of(d.width, 4));
    let substrate = LED_SUBSTRATE_NM.min(part_of(d.height, 2));
    let (asset, resistor, led) = match d.part {
        Part::Resistor => (parts::CHIP_RESISTOR, true, false),
        Part::Capacitor => (parts::CHIP_CAPACITOR, false, false),
        Part::Inductor | Part::Ferrite => (parts::CHIP_FERRITE, false, false),
        Part::Led => (parts::CHIP_LED, false, true),
    };
    let mut values = v1::Params::new();
    let mut set = |name: &str, nm: i64| {
        values.insert(name.into(), ParamValue::Int(nm));
    };
    set("length", d.length);
    set("width", d.width);
    set("height", d.height);
    set("termination", d.termination);
    if resistor {
        set("overcoat", overcoat);
    }
    if led {
        set("substrate", substrate);
        let radius = LENS_EDGE_NM
            .min(part_of(part_of(d.width, 2), 2))
            .min(part_of(d.height - substrate, 2));
        set("lens_round", radius);
    }
    // The body's long edges stay below the overcoat's inset, so the
    // overcoat sits on the flat; an LED's terminations wrap its substrate.
    let end_top = if led { substrate } else { d.height };
    let small = d.width.min(end_top);
    let mut body_edge = BODY_EDGE_NM.min(part_of(small, 10));
    if resistor {
        body_edge = body_edge.min(inset);
    }
    set("body_round", body_edge);
    let end_edge = END_EDGE_NM
        .min(part_of(small * 3, 20))
        .min(part_of(d.termination, 3));
    set("end_round", end_edge);
    v1::Model::PartFile {
        asset: asset.into(),
        values,
    }
}

fn terminals(part: Part) -> Vec<Terminal> {
    let names = if part == Part::Led {
        ["K", "A"]
    } else {
        ["1", "2"]
    };
    ["1", "2"]
        .iter()
        .zip(names)
        .map(|(key, name)| Terminal {
            key: (*key).into(),
            name: name.into(),
            direction: Direction::Passive,
        })
        .collect()
}

fn generate(request: &Request) -> Result<Generation, Vec<Problem>> {
    let description = description_with(Vec::new());
    let args = checked(&description, &request.params)?;
    let d = dims(&args)?;
    let (x, size, estimated) = lands(&args, &d)?;
    let pads = vec![pad("1", [-x, 0], size), pad("2", [x, 0], size)];
    let body = Box2::centred([0, 0], [d.length, d.width]);
    let courtyard = land::courtyard(&pads, Some(body)).unwrap_or_default();
    let top = courtyard.iter().map(|p| p[1]).max().unwrap_or(body.max[1]);
    let mut silk = land::silk_outline(&pads, body, land::DEFAULT_MASK_NM);
    silk.push(land::silk_designator(body, top));
    let lead_x = div_round(d.length - d.termination, 2).unwrap_or(0);
    let package = Package {
        terminals: vec!["1".into(), "2".into()],
        geometry: PackageGeometry {
            envelope_nm: [d.length, d.width, d.height],
            seating_nm: 0,
            leads: vec![
                Lead {
                    key: "1".into(),
                    contact: [-lead_x, 0, 0],
                },
                Lead {
                    key: "2".into(),
                    contact: [lead_x, 0, 0],
                },
            ],
            pin1: "1".into(),
        },
    };
    let footprint = Footprint {
        pads,
        courtyard,
        silk,
        assembly: land::assembly(body),
    };
    let mut notes: Vec<String> = Vec::new();
    if estimated {
        notes.push(
            "Lands are estimated from the body (termination + 0.6 mm by width + 0.2 mm); \
             confirm them against the part's recommended land pattern."
                .into(),
        );
    }
    let component_part = request.scope == Scope::Component;
    let name = format!(
        "Chip {} {} × {} mm",
        d.part.noun(),
        mm_text(d.length).trim_end_matches(" mm"),
        mm_text(d.width).trim_end_matches(" mm"),
    );
    let mut fields = vec![String::from("value")];
    if d.part == Part::Led {
        fields.push("colour".into());
    }
    Ok(Generation {
        component: component_part.then(|| Component {
            terminals: terminals(d.part),
            prefix: d.part.prefix().into(),
            name,
            fields,
        }),
        symbol: if component_part {
            Some(symbol(
                d.part,
                args.text("resistor_style").map_err(|p| vec![p])?,
            ))
        } else {
            None
        },
        package,
        footprint,
        binding: if component_part {
            ["1", "2"]
                .iter()
                .map(|k| BindingEntry {
                    package_terminal: (*k).into(),
                    terminal: (*k).into(),
                })
                .collect()
        } else {
            Vec::new()
        },
        model: Some(model(&d)),
        fields: Vec::new(),
        notes,
    })
}

impl Generator for Chip {
    fn describe() -> Description {
        description()
    }

    fn generate(request: &Request) -> Result<Generation, Vec<Problem>> {
        generate(request)
    }
}

#[cfg(test)]
mod tests;
