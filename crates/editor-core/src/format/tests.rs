use super::*;
use crate::{
    Endpoint, Engine, Layer, Operation, layout::LayoutCommand, layout::RouteRequest, parts,
    schematic::SchematicCommand,
};
use serde_json::json;

fn incomplete() -> Part {
    let mut part = parts::passive_template("");
    part.binding.mappings.clear();
    part.symbol.pins[0].terminal = 999;
    part.package.body_size = [0.0, -1.0, 0.5];
    part
}

fn routed_demo() -> Result<Engine> {
    let mut engine = Engine::demo()?;
    let shared = engine
        .project()
        .net_of(Endpoint {
            instance: 2,
            terminal: 2,
        })
        .map(|n| n.id)
        .ok_or_else(|| Error::NotFound("shared net".into()))?;
    engine.apply(
        engine.revision(),
        Operation::Layout(LayoutCommand::Route {
            request: RouteRequest {
                net: shared,
                start: [9.2, 8.0],
                end: [14.8, 8.0],
                width: 0.3,
                layer: Layer::Top,
            },
        }),
    )?;
    Ok(engine)
}

#[test]
fn routed_project_and_incomplete_draft_roundtrip_through_the_document() -> Result<()> {
    let mut engine = routed_demo()?;
    let draft = incomplete();
    assert!(
        parts::validate_part(&draft).is_err(),
        "fixture is deliberately unfinished"
    );
    engine.apply(
        engine.revision(),
        Operation::Part(parts::PartCommand::SetDraft {
            draft: draft.clone(),
        }),
    )?;
    let bytes = encode(&engine.to_file())?;
    let opened = Engine::from_file(decode(&bytes)?)?;
    assert_eq!(opened.draft(), Some(&draft));
    assert_eq!(opened.project(), engine.project());
    assert_eq!(opened.project().tracks.len(), 1);
    assert!(!opened.can_undo());
    Ok(())
}

#[test]
fn corrupt_newer_and_invalid_documents_leave_engine_and_history_unchanged() -> Result<()> {
    let mut engine = Engine::demo()?;
    engine.apply(
        engine.revision(),
        Operation::Part(parts::PartCommand::SetDraft {
            draft: incomplete(),
        }),
    )?;
    engine.apply(
        engine.revision(),
        Operation::Schematic(SchematicCommand::Edit {
            instance: 2,
            reference: "R99".into(),
            value: "22k".into(),
        }),
    )?;
    let before = engine.to_file();
    let revision = engine.revision();
    let mut newer = serde_json::to_value(&before)?;
    newer["version"] = json!(999);
    let mut feature = serde_json::to_value(&before)?;
    feature["required_features"] = json!(["future-editor"]);
    let mut missing = serde_json::to_value(&before)?;
    missing["project"]["parts"] = json!([]);
    let mut net = serde_json::to_value(&before)?;
    net["project"]["nets"][0]["terminals"] = json!([]);
    let mut draft = serde_json::to_value(&before)?;
    draft["authoring_draft"]["symbol"]["body_size"] = json!([null, 2]);
    assert!(matches!(
        decode(&serde_json::to_vec(&newer)?),
        Err(Error::Unsupported(_))
    ));
    for bytes in [
        b"broken".to_vec(),
        serde_json::to_vec(&newer)?,
        serde_json::to_vec(&feature)?,
        serde_json::to_vec(&missing)?,
        serde_json::to_vec(&net)?,
        serde_json::to_vec(&draft)?,
    ] {
        assert!(
            decode(&bytes)
                .and_then(|file| engine.replace_document(file))
                .is_err()
        );
        assert_eq!(engine.to_file(), before);
        assert_eq!(engine.revision(), revision);
        assert!(engine.can_undo());
    }
    let mut unsafe_draft = before.clone();
    if let Some(draft) = &mut unsafe_draft.authoring_draft {
        draft.footprint.pads[0].position[0] = f64::INFINITY;
    }
    assert!(engine.replace_document(unsafe_draft).is_err());
    assert_eq!(engine.to_file(), before);
    engine.undo()?;
    assert_eq!(engine.project().instances[0].reference, "R1");
    assert!(engine.draft().is_some());
    Ok(())
}

#[test]
fn encoding_is_bounded_and_validated() -> Result<()> {
    let engine = Engine::demo()?;
    let mut invalid = engine.to_file();
    let mut draft = incomplete();
    draft.provenance = "x".repeat(4097);
    invalid.authoring_draft = Some(draft);
    assert!(encode(&invalid).is_err());
    assert!(
        encode_bounded(&"x".repeat(MAX_FILE_BYTES)).is_err(),
        "serialization is bounded before writing"
    );
    assert!(decode(&vec![b' '; MAX_FILE_BYTES + 1]).is_err());
    let mut draft = incomplete();
    draft.footprint.pads = vec![draft.footprint.pads[0].clone(); 1025];
    assert!(validate_draft(&draft).is_err());
    Ok(())
}
