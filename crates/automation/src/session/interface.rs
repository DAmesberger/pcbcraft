//! `interface.publish_preview` and `interface.publish`, the board
//! interface's host requests (docs/architecture/board-interface.md,
//! "Commands"; slice B3): the preview reads the document and the published
//! sidecar; the publish builds the design from the core's candidate sidecar
//! with Solidcraft, puts the design and its sidecar in place atomically and
//! records the publish with `interface.record_publish`, one undo step.
use super::Session;
use crate::Result;
use pcbcraft_core::Operation;
use pcbcraft_core::interface::publish;
use pcbcraft_core::interface::{InterfaceCommand, PublishParams, PublishPreviewParams};
use pcbcraft_io::interface as files;
use serde_json::{Value, json};
use std::path::Path;

impl Session {
    /// The diff publishing would make, against the recorded publish and
    /// the sidecar beside `path` when it is there. Reads only.
    pub(super) fn interface_preview(&mut self, params: PublishPreviewParams) -> Result<Value> {
        let previous = match params.path.as_deref() {
            Some(path) => {
                let path = Path::new(path);
                files::check_design_path(path)?;
                files::read_sidecar(path)?
            }
            None => None,
        };
        let preview = publish::preview(
            self.engine.project(),
            params.tier,
            previous.as_ref(),
            self.engine.revision(),
        )?;
        Ok(serde_json::to_value(preview)?)
    }

    /// Write the interface design and its sidecar at `path`, then record
    /// the publish. The document's revision was checked before this runs
    /// (`Request::expected_revision`), so a stale request writes nothing.
    /// Refused, writing nothing, when the files there belong to another
    /// board's interface.
    pub(super) fn interface_publish(&mut self, params: PublishParams) -> Result<Value> {
        let design = Path::new(&params.path);
        files::check_design_path(design)?;
        let project = self.engine.project_arc().clone();
        let previous = files::read_sidecar(design)?;
        let recorded = project.interface.source_id;
        let source_id = match (recorded, &previous) {
            (Some(own), Some(there)) if there.source_id != own => None,
            (Some(own), _) => Some(own),
            (None, Some(_)) => None,
            (None, None) => Some(files::new_source_id()?),
        }
        .ok_or_else(|| {
            pcbcraft_io::Error::Invalid(
                "interface.source: the interface published there belongs to another board; \
                 choose another file"
                    .into(),
            )
        })?;
        let sidecar = publish::candidate(&project, params.tier, source_id)?;
        let exported = pcbcraft_solidcraft::interface::build(&project, sidecar)?;
        let temporary = files::temporary_design(design)?;
        let temporary_text = temporary.to_string_lossy().into_owned();
        if let Err(error) = exported.save_new(&temporary_text) {
            let _ = std::fs::remove_file(&temporary);
            return Err(error.into());
        }
        let text = exported.sidecar.text()?;
        files::put_in_place(design, &temporary, &text)?;
        let record = publish::record(&project, &exported.sidecar)?;
        let mut reply = self.apply(
            params.expected_revision,
            Operation::Interface(InterfaceCommand::RecordPublish { source_id, record }),
        )?;
        reply["published"] = json!({
            "path": params.path,
            "sidecar": files::sidecar_path(design).to_string_lossy(),
            "revision": exported.sidecar.revision,
            "hash": exported.sidecar.hash,
            "tier": exported.sidecar.tier,
            "datums": exported.sidecar.datums.len(),
        });
        Ok(reply)
    }
}
