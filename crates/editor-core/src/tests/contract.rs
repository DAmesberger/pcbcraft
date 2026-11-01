//! What the library/pins/3D contract (C0) still pins now that every slice
//! is integrated: empty drafts of the new kinds refuse to publish, existing
//! documents are unchanged by the new optional fields, and the new schemas
//! are served. Behaviour of each command and query is tested by its slice.
use super::support::*;
use crate::ids::DefinitionKind;
use crate::{Engine, Operation};

type Fallible<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[test]
fn model_and_empty_row_drafts_cannot_publish() -> Fallible<()> {
    let mut engine = Engine::default();
    // An empty model has no asset (`model.asset`); an empty row has no
    // family (`package_row.family`). Valid ones publish (model3d and
    // catalog/templates tests).
    for kind in [DefinitionKind::Model, DefinitionKind::PackageRow] {
        let draft = new_draft(&mut engine, kind)?;
        let revision = engine.revision();
        let before = engine.to_file();
        let published = engine.apply(
            revision,
            Operation::Library(crate::LibraryCommand::Publish {
                drafts: vec![draft],
            }),
        );
        assert!(published.is_err(), "{kind:?}");
        assert_eq!(engine.revision(), revision);
        assert_eq!(engine.to_file(), before);
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
