#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Slice S4: generated, envelope and STEP models built as named designs
//! give the same face names on every build, in this process and in a
//! fresh one, and the board inserts the generated design it is traced on.
use super::*;
use crate::models::family_rev;
use crate::models::imported::tests::{fixture_step, model};
#[allow(unused_imports)]
use crate::models::tests::{nm, pt};
use solidcraft_engine::doc::naming;
use sparksmith_core::PackageTerminalId;
use sparksmith_core::library::{
    ChipForm, ChipKind, GeneratedMaterials, PackageForm, PackageModel, PackageTerminal,
};

type Fallible<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

/// A 0603 chip resistor with a generated model.
pub(crate) fn resistor() -> PackageDefinition {
    let form = PackageForm::Chip(Box::new(ChipForm {
        kind: ChipKind::Resistor,
        length_nm: nm(1.6),
        width_nm: nm(0.8),
        height_nm: nm(0.45),
        termination_nm: nm(0.3),
    }));
    PackageDefinition {
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
        envelope_nm: [nm(1.6), nm(0.8), nm(0.45)],
        form,
        ..PackageDefinition::default()
    }
}

/// Every body of a design with its face names, in order.
/// A design's bodies, each with its face names.
type Names = Vec<(String, Vec<String>)>;

fn names(state: &ModelState) -> Names {
    state
        .bodies
        .iter()
        .map(|b| (b.name.clone(), naming::face_names(b).to_vec()))
        .collect()
}

/// The names of the resistor's generated design, its envelope design and
/// the STEP fixture's design, each built anew (not from the cache).
fn built_names() -> Fallible<Vec<Names>> {
    let package = resistor();
    let mut native = Native::new("Package model")?;
    super::super::build_standalone(&mut native, &package)?;
    let generated = names(&*native.world_state()?);
    let mut envelope_package = package.clone();
    envelope_package.model = PackageModel::default();
    let envelope = names(envelope(&envelope_package)?.state());
    let bytes = fixture_step()?;
    let seen = crate::models::inspect::inspect(&bytes, ModelFormat::Step)?;
    let step = names(step(&model(ModelFormat::Step, &bytes, &seen), &bytes)?.state());
    Ok(vec![generated, envelope, step])
}

/// The resistor's face names as this builder revision makes them. Solidcraft
/// writes a fillet's nested edge in parentheses (`F5:blend(<edge>)`, upstream
/// #11359); names saved in the older colon form (`F5:blend:<edge>`) still read
/// as the same names (`old_blend_names_read_as_the_current_ones`).
const RESISTOR_NAMES: [(&str, &[&str]); 4] = [
    (
        "body",
        &[
            "F1:+z",
            "F1:-x",
            "F1:-y",
            "F1:+x",
            "F5:blend(F1:+y|F1:+z)",
            "F1:+y",
            "F5:blend(F1:+y|F1:-z)",
            "F1:-z",
            "F5:blend(F1:-y|F1:-z)",
            "F5:blend(F1:+z|F1:-y)",
        ],
    ),
    (
        "overcoat",
        &["F2:+z", "F2:+y", "F2:-x", "F2:-y", "F2:+x", "F2:-z"],
    ),
    (
        "termination 1",
        &[
            "F6:blend(F3:-x|F3:-y)",
            "F6:blend(F3:+z|F3:-x)",
            "F3:-x",
            "F6:blend(F3:-x|F3:-z)",
            "F3:-z",
            "F3:-y",
            "F3:+z",
            "F3:+y",
            "F3:+x",
            "F6:blend(F3:+y|F3:-x)",
        ],
    ),
    (
        "termination 2",
        &[
            "F7:blend(F4:+x|F4:+y)",
            "F7:blend(F4:+x|F4:-y)",
            "F7:blend(F4:+x|F4:-z)",
            "F7:blend(F4:+x|F4:+z)",
            "F4:-y",
            "F4:-z",
            "F4:-x",
            "F4:+z",
            "F4:+y",
            "F4:+x",
        ],
    ),
];

/// The resistor's design: its bodies by their own names, each face named
/// after the feature that made it, and these exact names: a change here is
/// a change every reference to the model sees (a new Solidcraft naming or
/// another builder), so it needs a new builder revision (`family_rev`).
#[test]
fn a_generated_model_is_a_design_with_its_own_names() -> Fallible {
    let _context = crate::looks::test_context().map(|c| c.enter());
    let design = generated(&resistor())?;
    let got = names(design.state());
    let expected: Names = RESISTOR_NAMES
        .iter()
        .map(|(b, f)| ((*b).to_owned(), f.iter().map(|s| (*s).to_owned()).collect()))
        .collect();
    assert_eq!(got, expected);
    assert!(design.generated.iter().any(|b| b.terminal == Some(1)));
    Ok(())
}

/// A reference saved before Solidcraft wrote nested names in parentheses
/// names a fillet face `F5:blend:F1:+y|F1:+z`: it reads as the same face
/// name as the current text, so it still finds its face.
#[test]
fn old_blend_names_read_as_the_current_ones() -> Fallible {
    use solidcraft_engine::doc::persistent_name::FaceName;
    let mut blends = 0;
    for name in RESISTOR_NAMES.iter().flat_map(|(_, faces)| faces.iter()) {
        let Some((head, edge)) = name.split_once(":blend(") else {
            continue;
        };
        let edge = edge.strip_suffix(')').ok_or("a closed blend name")?;
        let old = format!("{head}:blend:{edge}");
        assert_eq!(FaceName::parse(&old)?, FaceName::parse(name)?, "{old}");
        assert_eq!(FaceName::parse(&old)?.to_string(), *name);
        blends += 1;
    }
    assert_eq!(blends, 12);
    Ok(())
}

/// Built twice, the designs have the same bodies and face names; the
/// cache gives the same design back without building it.
#[test]
fn a_named_design_has_the_same_names_on_every_build() -> Fallible {
    let _context = crate::looks::test_context().map(|c| c.enter());
    assert_eq!(built_names()?, built_names()?);
    let package = resistor();
    let a = generated(&package)?;
    let b = generated(&package)?;
    assert!(Arc::ptr_eq(&a, &b));
    Ok(())
}

const DUMP: &str = "SPARKSMITH_NAMED_DESIGN_DUMP";

/// Writes [`built_names`] to the file `SPARKSMITH_NAMED_DESIGN_DUMP` names;
/// run in a fresh process by the next test.
#[test]
#[ignore = "run in a fresh process by a_named_design_has_the_same_names_in_a_fresh_process"]
fn dump_named_design_names() -> Fallible {
    let _context = crate::looks::test_context().map(|c| c.enter());
    let path = std::env::var(DUMP)?;
    std::fs::write(path, serde_json::to_vec(&built_names()?)?)?;
    Ok(())
}

/// Another process builds the same designs with the same names.
#[test]
#[cfg(not(target_arch = "wasm32"))]
fn a_named_design_has_the_same_names_in_a_fresh_process() -> Fallible {
    let _context = crate::looks::test_context().map(|c| c.enter());
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("names.json");
    let status = std::process::Command::new(std::env::current_exe()?)
        .args([
            "--exact",
            "models::named::tests::dump_named_design_names",
            "--ignored",
            "--test-threads",
            "1",
        ])
        .env(DUMP, &path)
        .status()?;
    assert!(status.success());
    let theirs: Vec<Names> = serde_json::from_slice(&std::fs::read(&path)?)?;
    assert_eq!(theirs, built_names()?);
    Ok(())
}

/// The envelope design is one box: six faces of one feature.
#[test]
fn an_envelope_is_a_one_box_design() -> Fallible {
    let mut package = resistor();
    package.model = PackageModel::default();
    let design = super::package(&package)?;
    let got = names(design.state());
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].0, ENVELOPE_BODY);
    assert_eq!(got[0].1.len(), 6, "{got:?}");
    let b = design.state().bodies[0].outline_bounds();
    assert!((b.min.x + 0.8).abs() < 1e-9 && (b.max.z - 0.45).abs() < 1e-9);
    Ok(())
}

/// The STEP design names its bodies by their keys in the file, and the
/// file is parsed once while the board's source cache holds it.
#[test]
fn a_step_design_names_bodies_by_their_keys() -> Fallible {
    let bytes = fixture_step()?;
    let seen = crate::models::inspect::inspect(&bytes, ModelFormat::Step)?;
    let definition = model(ModelFormat::Step, &bytes, &seen);
    let design = step(&definition, &bytes)?;
    let bodies: Vec<&str> = design
        .state()
        .bodies
        .iter()
        .map(|b| b.name.as_str())
        .collect();
    let keys: Vec<&str> = seen.bodies.iter().map(|b| b.key.as_str()).collect();
    assert_eq!(bodies, keys);
    Ok(())
}
