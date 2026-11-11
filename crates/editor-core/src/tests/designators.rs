//! Designators the core assigns from the component's designator prefix, and
//! the prefix and package-body checks at publication.
use super::support::*;
use crate::catalog::synthetic;
use crate::completeness::CheckTarget;
use crate::format::{EditorFile, FILE_VERSION};
use crate::instances::{self, NewInstance};
use crate::library::{ChipForm, ChipKind, Drafts, PackageForm, PackageTerminal};
use crate::{
    ComponentRef, Engine, Error, InstanceCommand, Operation, Outcome, PackageTerminalId,
    PhysicalChoice, Result, Stage, UnitId,
};

/// An engine over the synthetic catalog: resistors, capacitors and headers
/// in turn, with prefixes R, C and J.
fn catalog() -> Result<Engine> {
    Engine::from_file(EditorFile {
        version: FILE_VERSION,
        project: synthetic::project(6)?,
        drafts: Drafts::default(),
    })
}

/// The first published component with `prefix`.
fn with_prefix(engine: &Engine, prefix: &str) -> Result<ComponentRef> {
    engine
        .project()
        .library
        .components
        .iter()
        .find(|c| c.definition.designator_prefix == prefix)
        .map(|c| ComponentRef::new(c.id, c.revision))
        .ok_or_else(|| Error::NotFound(format!("no {prefix} component")))
}

/// `component.place` of `component`'s first symbol and implementation.
fn request(
    engine: &Engine,
    component: ComponentRef,
    designator: Option<&str>,
) -> Result<Operation> {
    let library = &engine.project().library;
    let definition = &library
        .component(component)
        .ok_or_else(|| Error::NotFound("component".into()))?
        .definition;
    let binding = definition
        .implementations
        .first()
        .and_then(|b| library.binding(*b))
        .ok_or_else(|| Error::NotFound("binding".into()))?;
    let footprint = binding
        .definition
        .footprints
        .first()
        .ok_or_else(|| Error::NotFound("footprint".into()))?;
    Ok(Operation::Instance(InstanceCommand::Place(NewInstance {
        sheet: None,
        component,
        designator: designator.map(Into::into),
        properties: vec![],
        representation: definition
            .representations
            .first()
            .ok_or_else(|| Error::NotFound("symbol".into()))?
            .id,
        unit: UnitId(1),
        position: pt([0.0, 0.0]),
        physical: Some(PhysicalChoice {
            binding: binding.id,
            footprint: footprint.id,
        }),
    })))
}

/// Place and return the designator the outcome reports, checking it is
/// the one the document holds.
fn place_as(
    engine: &mut Engine,
    component: ComponentRef,
    designator: Option<&str>,
) -> Result<String> {
    let operation = request(engine, component, designator)?;
    match apply(engine, operation)? {
        Outcome::InstancePlaced {
            instance,
            designator,
            ..
        } => {
            let stored = engine
                .project()
                .instance(instance)
                .map(|i| i.designator.clone());
            assert_eq!(stored.as_deref(), Some(designator.as_str()));
            Ok(designator)
        }
        other => Err(Error::Invalid(format!("unexpected outcome {other:?}"))),
    }
}

#[test]
fn placing_without_a_designator_numbers_each_prefix() -> Result<()> {
    let mut engine = catalog()?;
    let (r, c) = (with_prefix(&engine, "R")?, with_prefix(&engine, "C")?);
    let placed = [
        place_as(&mut engine, r, None)?,
        place_as(&mut engine, r, None)?,
        place_as(&mut engine, c, None)?,
    ];
    assert_eq!(placed, ["R1", "R2", "C1"]);
    // Undo removes the last one; its number is free again.
    engine.undo()?;
    assert_eq!(place_as(&mut engine, c, None)?, "C1");
    Ok(())
}

#[test]
fn an_explicit_designator_is_honoured_and_numbering_skips_taken_ones() -> Result<()> {
    let mut engine = catalog()?;
    let r = with_prefix(&engine, "R")?;
    assert_eq!(place_as(&mut engine, r, Some("R1"))?, "R1");
    // Taken case-insensitively; `R03` and `R2A` are other designators.
    assert_eq!(place_as(&mut engine, r, Some("r2"))?, "r2");
    assert_eq!(place_as(&mut engine, r, Some("R4"))?, "R4");
    assert_eq!(place_as(&mut engine, r, Some("R03"))?, "R03");
    assert_eq!(place_as(&mut engine, r, Some("R2A"))?, "R2A");
    assert_eq!(place_as(&mut engine, r, None)?, "R3");
    assert_eq!(place_as(&mut engine, r, None)?, "R5");
    // An explicit designator is still checked: duplicates and malformed
    // text change nothing.
    for bad in ["R5", "r5", "R 6", ""] {
        let kept = Snapshot::of(&engine);
        let operation = request(&engine, r, Some(bad))?;
        let error = engine
            .apply(engine.revision(), operation)
            .err()
            .ok_or_else(|| Error::Invalid(format!("{bad:?} was accepted")))?;
        assert!(
            matches!(error.code(), "conflict" | "invalid_input"),
            "{bad:?}: {error}"
        );
        kept.assert_kept(&engine, bad);
    }
    Ok(())
}

#[test]
fn numbering_needs_a_valid_prefix() -> Result<()> {
    let engine = catalog()?;
    assert_eq!(instances::next_designator(engine.project(), "TP")?, "TP1");
    for bad in ["", "r", "R1", "ÄB", "ABCDEFGHI", "R-"] {
        let error = instances::next_designator(engine.project(), bad)
            .err()
            .ok_or_else(|| Error::Invalid(format!("{bad:?} was accepted")))?;
        assert_eq!(error.code(), "invalid_input", "{bad:?}");
    }
    Ok(())
}

/// The blocking codes `component.check` reports for one draft.
fn blocking(engine: &Engine, id: crate::DefinitionId) -> Result<Vec<String>> {
    Ok(check(engine, CheckTarget::Draft(id))?
        .findings
        .into_iter()
        .filter(|f| f.blocks_stage(Stage::Publish))
        .map(|f| f.code)
        .collect())
}

#[test]
fn prefix_and_chip_body_are_checked_at_publication() -> Result<()> {
    let mut engine = Engine::default();
    let f = draft(
        &mut engine,
        &Spec {
            name: "Pair",
            terminals: vec![(1, "A", passive()), (2, "B", passive())],
            units: vec![(1, "A", vec![(1, [-5.0, 0.0]), (2, [5.0, 0.0])])],
            package: vec![(1, "1"), (2, "2")],
            mapping: vec![(1, 1), (2, 2)],
            physical: true,
        },
    )?;
    assert!(blocking(&engine, f.component)?.is_empty());
    for (prefix, valid) in [
        ("R", true),
        ("ABCDEFGH", true),
        ("", false),
        ("r", false),
        ("R1", false),
        ("ABCDEFGHI", false),
        ("Ω", false),
    ] {
        with(&mut engine, f.component, component_body, |c| {
            c.designator_prefix = prefix.into()
        })?;
        let codes = blocking(&engine, f.component)?;
        assert_eq!(
            codes.iter().any(|c| c == "component.designator_prefix"),
            !valid,
            "{prefix:?}: {codes:?}"
        );
    }
    with(&mut engine, f.component, component_body, |c| {
        c.designator_prefix = "C".into()
    })?;
    // A chip form needs exactly two package terminals.
    let chip_finding = |engine: &Engine| -> Result<bool> {
        Ok(blocking(engine, f.package)?
            .iter()
            .any(|c| c == "package.form_terminals"))
    };
    with(&mut engine, f.package, package_body, |p| {
        p.form = PackageForm::Chip(Box::new(ChipForm::new(
            ChipKind::Capacitor,
            (p.envelope()).map(nm),
        )))
    })?;
    assert!(!chip_finding(&engine)?);
    with(&mut engine, f.package, package_body, |p| {
        p.terminals.push(PackageTerminal {
            id: PackageTerminalId(p.next_local),
            designation: "X".into(),
        });
        p.next_local += 1;
    })?;
    assert!(chip_finding(&engine)?);
    with(&mut engine, f.package, package_body, |p| {
        p.form = PackageForm::Generic
    })?;
    assert!(!chip_finding(&engine)?);
    Ok(())
}

/// Each synthetic family carries its own prefix and package body: a
/// resistor and a capacitor of one size code are different packages.
#[test]
fn synthetic_families_carry_prefix_and_body() -> Result<()> {
    let engine = catalog()?;
    let library = &engine.project().library;
    for (prefix, body) in [
        ("R", Some(ChipKind::Resistor)),
        ("C", Some(ChipKind::Capacitor)),
        ("J", None),
    ] {
        let component = with_prefix(&engine, prefix)?;
        let binding = library
            .component(component)
            .and_then(|c| c.definition.implementations.first())
            .and_then(|b| library.binding(*b))
            .ok_or_else(|| Error::NotFound("binding".into()))?;
        let package = library
            .package(binding.definition.package)
            .ok_or_else(|| Error::NotFound("package".into()))?;
        assert_eq!(package.definition.chip_kind(), body, "{prefix}");
    }
    Ok(())
}
