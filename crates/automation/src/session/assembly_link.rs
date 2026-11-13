//! The assembly link's host requests (docs/architecture/board-interface.md,
//! "Link updates"; slice B4): `assembly.preview`, `assembly.update` (which
//! links, updates and relinks) and `assembly.follow`, and the dry run
//! `interface.publish_preview` runs on a linked design. The link's
//! status is the core's `assembly.status` query.
//!
//! Every read copies the linked design into a scratch folder first
//! (`pcbcraft_io::assembly_link::copy`) and opens the copy in Sparksmith's own
//! engine (`pcbcraft_solidcraft::linked_design`): the user's linked design file is
//! only ever read. What was read becomes one `assembly.*` command, one undo
//! step; an update is only applied at the hash its preview showed.
use super::Session;
use crate::Result;
use pcbcraft_core::Operation;
use pcbcraft_core::diagnostics::Error as CoreError;
use pcbcraft_core::interface::assembly_link::{
    self, AssemblyCommand, AssemblyRead, BindingRead, FollowParams, NewReference, PreviewParams,
    ReferenceRead, UpdateParams,
};
use pcbcraft_core::interface::{DryRun, Sidecar};
use pcbcraft_core::library::footprint_source::ParamName;
use pcbcraft_core::model::assembly_link::{AssemblyBinding, AssemblyLink, Consumer, UnexportedUse};
use pcbcraft_core::model::dimension::{BodyName, FaceName, ReferenceStatus};
use pcbcraft_core::units::Nm;
use pcbcraft_io::assembly_link::{LinkedDesignCopy, copy};
use pcbcraft_solidcraft::linked_design::{BoardLink, LinkedDesign, Use};
use serde_json::Value;
use std::path::{Path, PathBuf};

/// A linked design read from a scratch copy.
struct Opened {
    copy: LinkedDesignCopy,
    linked_design: LinkedDesign,
    board: BoardLink,
    /// The sidecar beside the published design the assembly links.
    published: Option<Sidecar>,
}

fn refused(message: String) -> crate::Error {
    CoreError::Invalid(message).into()
}

impl Session {
    /// A path as the user gave it, resolved against the project's folder
    /// when it is relative.
    fn assembly_path(&self, given: &str) -> PathBuf {
        let path = Path::new(given);
        match (
            path.is_relative(),
            self.path.as_deref().and_then(Path::parent),
        ) {
            (true, Some(folder)) if !folder.as_os_str().is_empty() => folder.join(path),
            _ => path.to_path_buf(),
        }
    }

    /// Copy and open the linked design at `path`, and find its link to the
    /// board's published `design`.
    fn open_linked_design(&self, path: &str, design: &str) -> Result<Opened> {
        let file = self.assembly_path(path);
        if !file.try_exists().map_err(pcbcraft_io::Error::from)? {
            return Err(refused(format!(
                "assembly.missing: The linked design file was not found at {}. The board uses the \
                 last known geometry; relink it",
                file.display()
            )));
        }
        let copy = copy(&file)?;
        let (linked_design, newer) = LinkedDesign::open(&copy.path.to_string_lossy())?;
        if newer {
            return Err(refused(
                "assembly.newer: This linked design was saved by a newer SolidCraft than this \
                 Sparksmith can read"
                    .into(),
            ));
        }
        let design = self.assembly_path(design);
        let board = linked_design.board_link(&copy.folder, &design)?;
        let published = pcbcraft_io::interface::read_sidecar(&design)?;
        Ok(Opened {
            copy,
            linked_design,
            board,
            published,
        })
    }

    /// What the board reads of an opened linked design, for `link` (its
    /// references and bindings; none for a new link).
    fn assembly_read(
        &self,
        opened: &Opened,
        path: &str,
        design: &str,
        link: Option<&AssemblyLink>,
    ) -> AssemblyRead {
        let mut references = Vec::new();
        let mut bindings = Vec::new();
        for r in link.map(|l| l.references.as_slice()).unwrap_or_default() {
            references.push(
                match opened.linked_design.trace(
                    &opened.board,
                    &r.body.0,
                    &r.face.0,
                    r.pick.map(Nm::to_mm_f64),
                ) {
                    Ok((traced, pick)) => ReferenceRead {
                        id: r.id,
                        status: traced.status,
                        pick: Some(pick),
                        geometry: Some(traced.geometry),
                        note: traced.note,
                        built_on: built_on(opened, &r.face.0),
                    },
                    Err(error) => ReferenceRead {
                        id: r.id,
                        status: ReferenceStatus::Lost,
                        pick: None,
                        geometry: None,
                        note: words(&error.to_string()),
                        built_on: Vec::new(),
                    },
                },
            );
        }
        for b in link.map(|l| l.bindings.as_slice()).unwrap_or_default() {
            bindings.push(BindingRead {
                board_param: b.board_param.clone(),
                value_mm: opened.linked_design.parameter_mm(&b.assembly_param),
            });
        }
        AssemblyRead {
            assembly_hint: path.to_owned(),
            design_hint: design.to_owned(),
            component: opened.board.name.clone(),
            file_sha256: opened.copy.hash,
            board_revision: opened.published.as_ref().map(|s| s.revision),
            references,
            bindings,
            consumers: consumers(opened),
            board_sha256: opened.board.file_sha256,
            unexported: unexported(opened),
        }
    }

    fn linked(&self) -> Result<AssemblyLink> {
        self.engine
            .project()
            .assembly_link
            .as_deref()
            .cloned()
            .ok_or_else(|| refused("assembly.unlinked: no linked design is linked".into()))
    }

    /// Whether the files a request names are the link's: the linked design
    /// (unless relinking) and the board's published design.
    fn same_files(&self, link: &AssemblyLink, path: Option<&str>, design: &str) -> Result<()> {
        let same = |a: &str, b: &str| {
            let (a, b) = (self.assembly_path(a), self.assembly_path(b));
            match (a.canonicalize(), b.canonicalize()) {
                (Ok(a), Ok(b)) => a == b,
                _ => a == b,
            }
        };
        if let Some(path) = path
            && !same(path, &link.assembly_hint)
        {
            return Err(refused(format!(
                "assembly.relink: the board is linked to the linked design at {}; preview {path} and \
             update from it to relink",
                link.assembly_hint
            )));
        }
        if !same(design, &link.design_hint) {
            return Err(refused(format!(
                "assembly.design: the assembly link reads the board published at {}",
                link.design_hint
            )));
        }
        Ok(())
    }

    /// `assembly.update` with no assembly linked: the read that links the
    /// one at `path`.
    fn assembly_link(&self, params: &UpdateParams) -> Result<AssemblyCommand> {
        let opened = self.open_linked_design(&params.path, &params.design)?;
        if let Some(expect) = params.expect_hash.as_deref()
            && assembly_link::expected_hash(expect)? != opened.copy.hash
        {
            return Err(CoreError::Conflict(
                "assembly.changed: the linked design changed since the preview; preview it again"
                    .into(),
            )
            .into());
        }
        let read = self.assembly_read(&opened, &params.path, &params.design, None);
        Ok(AssemblyCommand::RecordLink { read })
    }

    /// `assembly.follow`: a face (a followed face) or a parameter (a
    /// binding).
    pub(super) fn assembly_follow(&mut self, params: FollowParams) -> Result<Value> {
        let link = self.linked()?;
        self.same_files(&link, Some(&params.path), &params.design)?;
        let opened = self.open_linked_design(&params.path, &params.design)?;
        if opened.copy.hash != link.file_sha256 {
            return Err(CoreError::Conflict(
                "assembly.changed: the linked design changed since the board read it; update from it \
                 first"
                    .into(),
            )
            .into());
        }
        let face = (params.body, params.face, params.pick);
        let parameter = (params.board_param, params.assembly_param);
        let command = match (face, parameter) {
            ((Some(body), Some(face), Some(pick)), (None, None)) => {
                let (traced, _) = opened
                    .linked_design
                    .trace(&opened.board, &body, &face, pick)?;
                if traced.status != ReferenceStatus::Resolved {
                    return Err(refused(format!(
                        "assembly.reference: the face is not the one picked: {}",
                        traced.note
                    )));
                }
                let name = params.name.unwrap_or_else(|| format!("{body} · {face}"));
                let built_on = built_on(&opened, &face);
                AssemblyCommand::RecordReference {
                    reference: NewReference {
                        body: BodyName(body),
                        face: FaceName(face),
                        pick,
                        name,
                        geometry: traced.geometry,
                        built_on,
                    },
                }
            }
            ((None, None, None), (Some(board_param), Some(assembly_param))) => {
                let value = opened.linked_design.parameter_mm(&assembly_param).ok_or_else(|| {
                    refused(format!(
                        "assembly.bind: the linked design has no length parameter `{assembly_param}`"
                    ))
                })?;
                AssemblyCommand::RecordBinding {
                    binding: AssemblyBinding {
                        board_param: ParamName(board_param),
                        assembly_param,
                        value: Nm::from_mm_f64_rounded(value)?,
                    },
                }
            }
            _ => {
                return Err(refused(
                    "assembly.follow: give body, face and pick, or board_param and assembly_param"
                        .into(),
                ));
            }
        };
        let mut reply = self.apply(params.expected_revision, Operation::Assembly(command))?;
        reply["assembly"] = serde_json::to_value(self.engine.project().assembly_link.as_deref())?;
        Ok(reply)
    }

    /// The read for an update from, or a relink to, the linked design at `path`.
    fn update_read(&self, path: &str, design: &str) -> Result<AssemblyRead> {
        let link = self.linked()?;
        self.same_files(&link, None, design)?;
        let opened = self.open_linked_design(path, design)?;
        Ok(self.assembly_read(&opened, path, design, Some(&link)))
    }

    /// `assembly.preview`: an update when `path` is the linked file, else
    /// a relink.
    pub(super) fn assembly_preview(&mut self, params: PreviewParams) -> Result<Value> {
        let read = self.update_read(&params.path, &params.design)?;
        let preview = self.engine.assembly_preview(read, super::hosted());
        Ok(serde_json::to_value(preview)?)
    }

    /// `assembly.update`: a link when none is linked; else the linked file,
    /// or another (a relink), applied at the hash its preview showed.
    pub(super) fn assembly_update(&mut self, params: UpdateParams) -> Result<Value> {
        let command = self.assembly_update_command(&params)?;
        let mut reply = self.apply(params.expected_revision, Operation::Assembly(command))?;
        reply["assembly"] = serde_json::to_value(self.engine.project().assembly_link.as_deref())?;
        Ok(reply)
    }

    /// What `assembly.update` would apply, read from the linked design without
    /// applying it: `assembly.record_link` when none is linked, else
    /// `assembly.record_update` at the hash the preview showed. The desktop
    /// reads it off the UI thread, on a session over a fork of the window's
    /// engine, and applies it on its own engine as one undo step.
    pub fn assembly_update_command(&self, params: &UpdateParams) -> Result<AssemblyCommand> {
        if self.engine.project().assembly_link.is_none() {
            return self.assembly_link(params);
        }
        let expect = params.expect_hash.as_deref().ok_or_else(|| {
            refused(
                "assembly.expect_hash: a linked design is linked; preview it and update at the \
                 expect_hash the preview gave"
                    .into(),
            )
        })?;
        let read = self.update_read(&params.path, &params.design)?;
        if read.file_sha256 != assembly_link::expected_hash(expect)? {
            return Err(CoreError::Conflict(
                "assembly.changed: the linked design changed since the preview; preview it again"
                    .into(),
            )
            .into());
        }
        Ok(AssemblyCommand::RecordUpdate {
            read,
            resolve: params.resolve.clone(),
        })
    }

    /// The dry run of a publish on the linked design at `linked_design`: the
    /// candidate design built into the copy's scratch folder, the copy's
    /// board link pointed at it. `design` is the published design the
    /// assembly links. `None` when the board was not published there yet.
    pub(super) fn dry_run(
        &self,
        linked_design: &str,
        design: &str,
        candidate: Sidecar,
    ) -> Result<Option<DryRun>> {
        let link = self.linked()?;
        self.same_files(&link, Some(linked_design), design)?;
        let mut opened = self.open_linked_design(linked_design, design)?;
        let Some(published) = opened.published.take() else {
            return Ok(None);
        };
        let project = self.engine.project_arc().clone();
        let exported = pcbcraft_solidcraft::interface::build(&project, candidate)?;
        let built = opened.copy.scratch().join("candidate.solidcraft");
        let built_text = built.to_string_lossy().into_owned();
        exported.save_new(&built_text)?;
        let effects = opened.linked_design.dry_run(
            &opened.board,
            &built_text,
            &published,
            &exported.sidecar,
        )?;
        Ok(Some(DryRun {
            linked_design: linked_design.to_owned(),
            effects,
        }))
    }
}

/// Linked design uses as consumers: the datum's id, slot and name as
/// published.
fn as_consumers(sidecar: &Sidecar, uses: Vec<Use>) -> Vec<Consumer> {
    uses.into_iter()
        .filter_map(|u| {
            let d = sidecar.datums.iter().find(|d| d.token == u.token)?;
            Some(Consumer {
                feature: u.feature,
                datum: d.id,
                slot: d.slot,
                via: u.via,
                token: u.token,
                name: d.name.clone(),
            })
        })
        .collect()
}

/// The linked design features (and joints) that use the published datums, by
/// the slot that carries each.
fn consumers(opened: &Opened) -> Vec<Consumer> {
    let Some(sidecar) = opened.published.as_ref() else {
        return Vec::new();
    };
    as_consumers(sidecar, opened.linked_design.uses(&opened.board, sidecar))
}

/// The linked design features attached to faces of the board no slot carries.
fn unexported(opened: &Opened) -> Vec<UnexportedUse> {
    let Some(sidecar) = opened.published.as_ref() else {
        return Vec::new();
    };
    opened
        .linked_design
        .unexported(&opened.board, sidecar)
        .into_iter()
        .map(|u| UnexportedUse {
            feature: u.feature,
            body: u.body,
        })
        .collect()
}

/// The board's datums the linked design geometry under `face` is built on.
fn built_on(opened: &Opened, face: &str) -> Vec<Consumer> {
    let Some(sidecar) = opened.published.as_ref() else {
        return Vec::new();
    };
    as_consumers(
        sidecar,
        opened.linked_design.built_on(&opened.board, sidecar, face),
    )
}

/// An error's message without its code prefix, as a note.
fn words(message: &str) -> String {
    let text = message
        .split_once(": ")
        .map_or(
            message,
            |(head, rest)| {
                if head.contains(' ') { message } else { rest }
            },
        )
        .trim();
    text.chars().take(1000).collect()
}
