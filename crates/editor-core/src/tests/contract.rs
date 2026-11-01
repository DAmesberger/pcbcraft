//! Contract C0 (docs/library-pins-3d-contract.md): the new kinds, commands
//! and queries exist on the wire, refuse explicitly until their slice
//! lands, and leave the document unchanged; existing files are unchanged
//! by the new optional fields.
use super::support::*;
use crate::diagnostics::Error;
use crate::ids::DefinitionKind;
use crate::{Engine, Operation, Query};

type Fallible<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn unsupported<T: std::fmt::Debug>(result: crate::Result<T>) -> bool {
    matches!(result, Err(Error::Unsupported(_)))
}

#[test]
fn new_queries_refuse_as_unsupported() -> Fallible<()> {
    let engine = Engine::demo()?;
    let queries = [Query::Templates {
        query: "soic pins:8".into(),
        limit: None,
    }];
    for query in queries {
        let shown = format!("{query:?}");
        assert!(unsupported(engine.query(query)), "{shown}");
    }
    Ok(())
}

#[test]
fn model_and_row_drafts_cannot_publish_yet() -> Fallible<()> {
    let mut engine = Engine::default();
    for kind in [DefinitionKind::Model, DefinitionKind::PackageRow] {
        let draft = new_draft(&mut engine, kind)?;
        let revision = engine.revision();
        let published = engine.apply(
            revision,
            Operation::Library(crate::LibraryCommand::Publish {
                drafts: vec![draft],
            }),
        );
        assert!(published.is_err(), "{kind:?}");
        assert_eq!(engine.revision(), revision);
    }
    Ok(())
}

#[test]
fn existing_documents_do_not_mention_the_new_fields() -> Fallible<()> {
    let text = serde_json::to_string(&Engine::showcase()?.to_file())?;
    for key in [
        "\"models\"",
        "\"package_rows\"",
        "\"pin_rules\"",
        "\"marking\"",
        "\"top_mark\"",
        "\"pin_config\"",
        "\"notes\"",
        "\"mux\"",
    ] {
        assert!(!text.contains(key), "{key}");
    }
    Ok(())
}

#[test]
fn schemas_include_the_pinout_document_and_the_firmware_ir() {
    let schemas = crate::schemas();
    assert!(schemas["pinout"].is_object());
    assert!(schemas["pin_config"].is_object());
}
