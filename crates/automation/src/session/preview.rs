//! `library.open_preview` (slice A2): read a folder bundle through io and
//! return `catalog::preview::inspect_bundle`, the same report
//! `library.inspect` gives a project item, without importing. Not a
//! document change: the document and its revision stay as they are.
use super::{BundleParams, Session};
use crate::Result;
use serde_json::Value;

impl Session {
    pub(super) fn open_preview(&mut self, params: BundleParams) -> Result<Value> {
        let bundle = self
            .sources
            .bundle(params.library, params.lineage, params.revision)?;
        let report = pcbcraft_core::catalog::preview::inspect_bundle(&bundle, None, None)?;
        Ok(serde_json::to_value(report)?)
    }
}

#[cfg(test)]
mod tests {
    use super::super::sources::Sources;
    use super::super::{Session, publish_library};
    use pcbcraft_core::Engine;
    use pcbcraft_core::format::{EditorFile, FILE_VERSION};
    use pcbcraft_core::library::Drafts;
    use pcbcraft_io::settings::UserDirs;
    use serde_json::{Value, json};

    type Fallible<T> = std::result::Result<T, Box<dyn std::error::Error>>;

    fn reply(session: &mut Session, command: &str, params: Value) -> Value {
        session.dispatch(json!({"command": command, "params": params}))
    }

    fn ok(session: &mut Session, command: &str, params: Value) -> Fallible<Value> {
        let reply = reply(session, command, params);
        assert_eq!(reply["ok"], true, "{command}: {reply}");
        Ok(reply["result"].clone())
    }

    /// An author publishes the synthetic catalog to a folder; a designer
    /// adds it read-only and previews a component and a Part from it
    /// without importing: the report matches the author's
    /// `library.inspect` and the designer's document is unchanged.
    #[test]
    fn open_preview_inspects_a_folder_item_without_importing() -> Fallible<()> {
        let temp = tempfile::tempdir()?;
        let folder = temp.path().join("team-library");
        let folder_text = folder.display().to_string();
        let mut author = Session::new(Engine::from_file(EditorFile {
            version: FILE_VERSION,
            project: pcbcraft_core::catalog::synthetic::project_with_parts(4)?,
            drafts: Drafts::default(),
        })?);
        let published = publish_library(&mut author, &folder_text, "Team library")?;
        let library = published["library"].clone();

        let dirs = UserDirs {
            config: temp.path().join("designer/config"),
            cache: temp.path().join("designer/cache"),
        };
        let mut designer = Session::with_sources(Engine::default(), Sources::with_dirs(dirs)?);
        ok(
            &mut designer,
            "library.source.add",
            json!({"path": folder_text}),
        )?;
        let search = ok(
            &mut designer,
            "project.query",
            json!({"query": "library.search", "params": {"query": "", "limit": 500}}),
        )?;
        let entries: Vec<Value> = search["result"]["results"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|hit| hit["entry"].clone())
            .filter(|e| e["source"]["library"] == library)
            .collect();
        let component = entries
            .iter()
            .find(|e| e["kind"] == "component")
            .ok_or("no folder component")?;
        let part = entries
            .iter()
            .find(|e| e["kind"] == "part")
            .ok_or("no folder part")?;

        let revision = designer.engine().revision();
        let document = designer.engine().document().clone();
        let preview = ok(
            &mut designer,
            "library.open_preview",
            json!({"library": library, "lineage": component["source_lineage"]}),
        )?;
        assert_eq!(designer.engine().revision(), revision);
        assert_eq!(designer.engine().document(), &document);
        assert_eq!(preview["entry"]["name"], component["name"], "{preview}");
        assert_eq!(preview["entry"]["source"]["library"], library);
        assert_eq!(
            preview["entry"]["source_lineage"],
            component["source_lineage"]
        );
        assert_eq!(preview["entry"]["revision"], component["revision"]);

        // The same content as the author's project copy reports.
        let name = component["name"].as_str().ok_or("name")?;
        let lineage = author
            .engine()
            .project()
            .library
            .latest_components()
            .iter()
            .find(|c| c.meta.name == name)
            .map(|c| c.id.0)
            .ok_or("author component")?;
        let inspected = ok(
            &mut author,
            "project.query",
            json!({"query": "library.inspect", "params": {"lineage": lineage}}),
        )?;
        let inspected = &inspected["result"];
        for key in ["terminals", "provenance"] {
            assert_eq!(preview[key], inspected[key], "{key}");
        }
        let packages = |r: &Value| {
            r["implementations"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|i| (i["package_name"].clone(), i["envelope"].clone()))
                .collect::<Vec<_>>()
        };
        assert_eq!(packages(&preview), packages(inspected));

        // A Part previews as the component it implements.
        let of_part = ok(
            &mut designer,
            "library.open_preview",
            json!({"library": library, "lineage": part["source_lineage"]}),
        )?;
        assert_eq!(of_part["entry"]["kind"], "component", "{of_part}");
        assert_eq!(of_part["entry"]["badges"]["mpn"], true);

        // An unknown lineage is refused and changes nothing.
        let missing = reply(
            &mut designer,
            "library.open_preview",
            json!({"library": library, "lineage": "ffffffffffffffffffffffffffffffff"}),
        );
        assert_eq!(missing["ok"], false, "{missing}");
        assert_eq!(missing["error"]["code"], "not_found", "{missing}");
        assert_eq!(designer.engine().revision(), revision);
        Ok(())
    }
}
