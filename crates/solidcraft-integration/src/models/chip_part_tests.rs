//! The chip model the shipped `chip` generator gives (GP2): the part file
//! of its bundle it names, evaluated through the part-file path at the
//! values it emits, against today's in-code chip builder for the same
//! parameters: the same bodies (names, roles, materials) with the same
//! bounds and volumes within 1 µm.
use super::part_file::{self, PartValues};
use super::*;
use sparksmith_core::PackageTerminalId;
use sparksmith_core::library::PackageTerminal;
use sparksmith_core::library::package_form::{ChipForm, ChipKind};
use sparksmith_core::library::package_model::PartValue;
use sparksmith_core::units::Nm;
use sparksmith_generators::abi::v1::{Model, ParamValue, Scope};
use sparksmith_generators::abi::{Failure, GeneratorRuntime, hex, sha256};
use sparksmith_generators::params::complete;
use sparksmith_generators::{Limits, WasmiRuntime};

/// The shipped module (test code may read it).
const CHIP: &[u8] = include_bytes!("../../../../library/generators/chip/chip.wasm");
/// The bundle's part files, as shipped.
const PARTS: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../library/generators/chip/parts"
);

/// 1 µm, in mm.
const UM: f64 = 1e-3;

fn n(nm: i64) -> Result<Nm> {
    Nm::new(nm).map_err(|e| Error::Invalid(e.to_string()))
}

fn fail(f: Failure) -> Error {
    Error::Verification(f.to_string())
}

/// The bundle's part file of SHA-256 `asset`.
fn part_bytes(asset: &str) -> Result<Vec<u8>> {
    for entry in std::fs::read_dir(PARTS).map_err(|e| Error::Verification(e.to_string()))? {
        let path = entry
            .map_err(|e| Error::Verification(e.to_string()))?
            .path();
        let bytes = std::fs::read(&path).map_err(|e| Error::Verification(e.to_string()))?;
        if hex(&sha256(&bytes)) == asset {
            return Ok(bytes);
        }
    }
    Err(Error::Verification(format!("no part file sha256 {asset}")))
}

/// Preset `key` of the module: its part file, the values in the file's
/// units (nm to mm), and the chip's dimensions and part.
fn generated(key: &str) -> Result<(Vec<u8>, PartValues, [i64; 4], String)> {
    let runtime = WasmiRuntime::new(Limits::DEFAULT);
    let description = runtime.describe(CHIP).map_err(fail)?;
    let preset = description
        .presets()
        .iter()
        .find(|p| p.key == key)
        .ok_or_else(|| Error::Verification(format!("no preset {key}")))?;
    let request = complete(&description, Scope::Package, &preset.params).map_err(fail)?;
    let made = runtime.generate(CHIP, &request).map_err(fail)?;
    let Some(Model::PartFile { asset, values }) = made.generation.model else {
        return Err(Error::Verification("no model".into()));
    };
    let mut converted = PartValues::new();
    for (name, value) in &values {
        let ParamValue::Int(v) = value else {
            return Err(Error::Verification(format!("{name} is not an integer")));
        };
        converted.insert(name.clone(), PartValue::Nm(n(*v)?.get()));
    }
    let int = |k: &str| match request.params.get(k) {
        Some(ParamValue::Int(v)) => *v,
        _ => 0,
    };
    let part = match request.params.get("part") {
        Some(ParamValue::Text(p)) => p.clone(),
        _ => String::new(),
    };
    Ok((
        part_bytes(&asset)?,
        converted,
        [
            int("length"),
            int("width"),
            int("height"),
            int("termination"),
        ],
        part,
    ))
}

/// The in-code builder's package for the same chip.
fn in_code(dims: [i64; 4], part: &str) -> Result<PackageDefinition> {
    let kind = match part {
        "resistor" => ChipKind::Resistor,
        "capacitor" => ChipKind::Capacitor,
        "led" => ChipKind::Led,
        _ => ChipKind::Inductor,
    };
    let form = PackageForm::Chip(Box::new(ChipForm {
        kind,
        length_nm: n(dims[0])?,
        width_nm: n(dims[1])?,
        height_nm: n(dims[2])?,
        termination_nm: n(dims[3])?,
    }));
    Ok(PackageDefinition {
        next_local: 3,
        terminals: (1..=2)
            .map(|i| PackageTerminal {
                id: PackageTerminalId(i),
                designation: i.to_string(),
            })
            .collect(),
        model: PackageModel::Generated {
            family_rev: family_rev(&form).unwrap_or(0),
            materials: GeneratedMaterials::default(),
        },
        form,
        ..PackageDefinition::default()
    })
}

/// Each body: name, role, material, bounds and volume (mm).
type Measured = Vec<(String, BodyRole, Option<MaterialId>, [f64; 6], f64)>;

fn measure(state: &solidcraft_engine::doc::ModelState, name: &str) -> Result<([f64; 6], f64)> {
    let body = state
        .body(name)
        .ok_or_else(|| Error::Verification(format!("no body {name}")))?;
    let m = body
        .measure()
        .map_err(|e| Error::Verification(e.to_string()))?;
    let (lo, hi) = (m.bbox.min, m.bbox.max);
    Ok(([lo.x, lo.y, lo.z, hi.x, hi.y, hi.z], m.volume))
}

fn from_part_file(bytes: &[u8], values: &PartValues) -> Result<Measured> {
    let part = part_file::evaluate(bytes, values)?;
    if let Some(refusal) = part.refusal() {
        return Err(refusal);
    }
    let mut out = Vec::new();
    for b in &part.bodies {
        let (bounds, volume) = measure(part.state(), &b.name)?;
        out.push((
            b.name.clone(),
            part_file::role_of(b.material),
            b.material,
            bounds,
            volume,
        ));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

fn from_builder(package: &PackageDefinition) -> Result<Measured> {
    let mut native = Native::new("parity")?;
    let bodies = build_standalone(&mut native, package)?;
    let state = native.session().model.state();
    let mut out = Vec::new();
    for b in bodies {
        let (bounds, volume) = measure(&state, &b.name)?;
        out.push((b.name, b.role, Some(b.material), bounds, volume));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

#[test]
fn the_generators_part_file_equals_the_in_code_chip_builder() -> Result<()> {
    let keys = [
        "resistor-0402",
        "resistor-0603",
        "resistor-0805",
        "resistor-1206",
        "capacitor-0402",
        "capacitor-0603",
        "capacitor-0805",
        "capacitor-1206",
        "ferrite-0603",
        "led-0603",
    ];
    let mut worst = (0.0f64, 0.0f64);
    for key in keys {
        let (bytes, values, dims, part) = generated(key)?;
        let file = from_part_file(&bytes, &values)?;
        let code = from_builder(&in_code(dims, &part)?)?;
        let ids = |v: &Measured| -> Vec<(String, BodyRole, Option<MaterialId>)> {
            v.iter()
                .map(|(n, r, m, _, _)| (n.clone(), *r, *m))
                .collect()
        };
        assert_eq!(ids(&file), ids(&code), "{key}: bodies");
        for ((name, _, _, a, va), (_, _, _, b, vb)) in file.iter().zip(&code) {
            let off = a
                .iter()
                .zip(b)
                .map(|(x, y)| (x - y).abs())
                .fold(0.0, f64::max);
            // A volume within 1 µm: at most the body's surface (from its
            // box) times 1 µm.
            let [x, y, z] = [b[3] - b[0], b[4] - b[1], b[5] - b[2]];
            let surface = 2.0 * (x * y + y * z + z * x);
            let dv = (va - vb).abs();
            assert!(off < UM, "{key} {name}: bounds differ by {off} mm");
            assert!(
                dv < surface * UM,
                "{key} {name}: volume {va} mm³ against {vb} mm³"
            );
            worst = (worst.0.max(off), worst.1.max(dv / surface));
        }
    }
    println!(
        "chip parity: bounds within {:.3e} mm, volumes within {:.3e} mm × surface",
        worst.0, worst.1
    );
    Ok(())
}

#[test]
fn the_bundles_part_files_pass_the_part_file_check() -> Result<()> {
    for entry in std::fs::read_dir(PARTS).map_err(|e| Error::Verification(e.to_string()))? {
        let path = entry
            .map_err(|e| Error::Verification(e.to_string()))?
            .path();
        let bytes = std::fs::read(&path).map_err(|e| Error::Verification(e.to_string()))?;
        let report = part_file::evaluate(&bytes, &PartValues::new())?.report();
        assert!(report.ok, "{}: {:?}", path.display(), report.findings);
    }
    Ok(())
}
