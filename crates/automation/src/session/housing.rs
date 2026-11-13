//! The enclosure link's host requests (docs/architecture/board-interface.md,
//! "Link updates"; slice B4): `housing.preview`, `housing.update` (which
//! links, updates and relinks) and `housing.follow`, and the dry run
//! `interface.publish_preview` runs on a linked enclosure. The link's
//! status is the core's `housing.status` query.
//!
//! Every read copies the enclosure into a scratch folder first
//! (`pcbcraft_io::housing::copy`) and opens the copy in Sparksmith's own
//! engine (`pcbcraft_solidcraft::enclosure`): the user's enclosure file is
//! only ever read. What was read becomes one `housing.*` command, one undo
//! step; an update is only applied at the hash its preview showed.
use super::Session;
use crate::Result;
use pcbcraft_core::Operation;
use pcbcraft_core::diagnostics::Error as CoreError;
use pcbcraft_core::interface::housing::{
    self, BindingRead, FollowParams, HousingCommand, HousingRead, NewReference, PreviewParams,
    ReferenceRead, UpdateParams,
};
use pcbcraft_core::interface::{DryRun, Sidecar};
use pcbcraft_core::library::footprint_source::ParamName;
use pcbcraft_core::model::dimension::{BodyName, FaceName, ReferenceStatus};
use pcbcraft_core::model::housing::{Consumer, HousingBinding, HousingLink};
use pcbcraft_core::units::Nm;
use pcbcraft_io::housing::{EnclosureCopy, copy};
use pcbcraft_solidcraft::enclosure::{BoardLink, Enclosure};
use serde_json::Value;
use std::path::{Path, PathBuf};

/// An enclosure read from a scratch copy.
struct Opened {
    copy: EnclosureCopy,
    enclosure: Enclosure,
    board: BoardLink,
    /// The sidecar beside the published design the enclosure links.
    published: Option<Sidecar>,
}

fn refused(message: String) -> crate::Error {
    CoreError::Invalid(message).into()
}

impl Session {
    /// A path as the user gave it, resolved against the project's folder
    /// when it is relative.
    fn housing_path(&self, given: &str) -> PathBuf {
        let path = Path::new(given);
        match (
            path.is_relative(),
            self.path.as_deref().and_then(Path::parent),
        ) {
            (true, Some(folder)) if !folder.as_os_str().is_empty() => folder.join(path),
            _ => path.to_path_buf(),
        }
    }

    /// Copy and open the enclosure at `path`, and find its link to the
    /// board's published `design`.
    fn open_enclosure(&self, path: &str, design: &str) -> Result<Opened> {
        let file = self.housing_path(path);
        if !file.try_exists().map_err(pcbcraft_io::Error::from)? {
            return Err(refused(format!(
                "housing.missing: The enclosure file was not found at {}. The board uses the \
                 last known geometry; relink it",
                file.display()
            )));
        }
        let copy = copy(&file)?;
        let (enclosure, newer) = Enclosure::open(&copy.path.to_string_lossy())?;
        if newer {
            return Err(refused(
                "housing.newer: This enclosure was saved by a newer SolidCraft than this \
                 Sparksmith can read"
                    .into(),
            ));
        }
        let design = self.housing_path(design);
        let board = enclosure.board_link(&copy.folder, &design)?;
        let published = pcbcraft_io::interface::read_sidecar(&design)?;
        Ok(Opened {
            copy,
            enclosure,
            board,
            published,
        })
    }

    /// What the board reads of an opened enclosure, for `link` (its
    /// references and bindings; none for a new link).
    fn housing_read(
        &self,
        opened: &Opened,
        path: &str,
        design: &str,
        link: Option<&HousingLink>,
    ) -> HousingRead {
        let mut references = Vec::new();
        let mut bindings = Vec::new();
        for r in link.map(|l| l.references.as_slice()).unwrap_or_default() {
            references.push(
                match opened.enclosure.trace(
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
                    },
                    Err(error) => ReferenceRead {
                        id: r.id,
                        status: ReferenceStatus::Lost,
                        pick: None,
                        geometry: None,
                        note: words(&error.to_string()),
                    },
                },
            );
        }
        for b in link.map(|l| l.bindings.as_slice()).unwrap_or_default() {
            bindings.push(BindingRead {
                board_param: b.board_param.clone(),
                value_mm: opened.enclosure.parameter_mm(&b.housing_param),
            });
        }
        HousingRead {
            enclosure_hint: path.to_owned(),
            design_hint: design.to_owned(),
            component: opened.board.name.clone(),
            file_sha256: opened.copy.hash,
            board_revision: opened.published.as_ref().map(|s| s.revision),
            references,
            bindings,
            consumers: consumers(opened),
        }
    }

    fn linked(&self) -> Result<HousingLink> {
        self.engine
            .project()
            .housing
            .as_deref()
            .cloned()
            .ok_or_else(|| refused("housing.unlinked: no enclosure is linked".into()))
    }

    /// Whether the files a request names are the link's: the enclosure
    /// (unless relinking) and the board's published design.
    fn same_files(&self, link: &HousingLink, path: Option<&str>, design: &str) -> Result<()> {
        let same = |a: &str, b: &str| {
            let (a, b) = (self.housing_path(a), self.housing_path(b));
            match (a.canonicalize(), b.canonicalize()) {
                (Ok(a), Ok(b)) => a == b,
                _ => a == b,
            }
        };
        if let Some(path) = path
            && !same(path, &link.enclosure_hint)
        {
            return Err(refused(format!(
                "housing.relink: the board is linked to the enclosure at {}; preview {path} and \
             update from it to relink",
                link.enclosure_hint
            )));
        }
        if !same(design, &link.design_hint) {
            return Err(refused(format!(
                "housing.design: the enclosure link reads the board published at {}",
                link.design_hint
            )));
        }
        Ok(())
    }

    /// `housing.update` with no enclosure linked: link the one at `path`.
    fn housing_link(&mut self, params: UpdateParams) -> Result<Value> {
        let opened = self.open_enclosure(&params.path, &params.design)?;
        if let Some(expect) = params.expect_hash.as_deref()
            && housing::expected_hash(expect)? != opened.copy.hash
        {
            return Err(CoreError::Conflict(
                "housing.changed: the enclosure changed since the preview; preview it again".into(),
            )
            .into());
        }
        let read = self.housing_read(&opened, &params.path, &params.design, None);
        let mut reply = self.apply(
            params.expected_revision,
            Operation::Housing(HousingCommand::RecordLink { read }),
        )?;
        reply["housing"] = serde_json::to_value(self.engine.project().housing.as_deref())?;
        Ok(reply)
    }

    /// `housing.follow`: a face (a housing reference) or a parameter (a
    /// binding).
    pub(super) fn housing_follow(&mut self, params: FollowParams) -> Result<Value> {
        let link = self.linked()?;
        self.same_files(&link, Some(&params.path), &params.design)?;
        let opened = self.open_enclosure(&params.path, &params.design)?;
        if opened.copy.hash != link.file_sha256 {
            return Err(CoreError::Conflict(
                "housing.changed: the enclosure changed since the board read it; update from it \
                 first"
                    .into(),
            )
            .into());
        }
        let face = (params.body, params.face, params.pick);
        let parameter = (params.board_param, params.housing_param);
        let command =
            match (face, parameter) {
                ((Some(body), Some(face), Some(pick)), (None, None)) => {
                    let (traced, _) = opened.enclosure.trace(&opened.board, &body, &face, pick)?;
                    if traced.status != ReferenceStatus::Resolved {
                        return Err(refused(format!(
                            "housing.reference: the face is not the one picked: {}",
                            traced.note
                        )));
                    }
                    let name = params.name.unwrap_or_else(|| format!("{body} · {face}"));
                    HousingCommand::RecordReference {
                        reference: NewReference {
                            body: BodyName(body),
                            face: FaceName(face),
                            pick,
                            name,
                            geometry: traced.geometry,
                        },
                    }
                }
                ((None, None, None), (Some(board_param), Some(housing_param))) => {
                    let value = opened.enclosure.parameter_mm(&housing_param).ok_or_else(|| {
                    refused(format!(
                        "housing.bind: the enclosure has no length parameter `{housing_param}`"
                    ))
                })?;
                    HousingCommand::RecordBinding {
                        binding: HousingBinding {
                            board_param: ParamName(board_param),
                            housing_param,
                            value: Nm::from_mm_f64_rounded(value)?,
                        },
                    }
                }
                _ => {
                    return Err(refused(
                    "housing.follow: give body, face and pick, or board_param and housing_param"
                        .into(),
                ));
                }
            };
        let mut reply = self.apply(params.expected_revision, Operation::Housing(command))?;
        reply["housing"] = serde_json::to_value(self.engine.project().housing.as_deref())?;
        Ok(reply)
    }

    /// The read for an update from, or a relink to, the enclosure at `path`.
    fn update_read(&self, path: &str, design: &str) -> Result<HousingRead> {
        let link = self.linked()?;
        self.same_files(&link, None, design)?;
        let opened = self.open_enclosure(path, design)?;
        Ok(self.housing_read(&opened, path, design, Some(&link)))
    }

    /// `housing.preview`: an update when `path` is the linked file, else
    /// a relink.
    pub(super) fn housing_preview(&mut self, params: PreviewParams) -> Result<Value> {
        let read = self.update_read(&params.path, &params.design)?;
        let hash = read.file_sha256;
        let mut fork = self.engine.fork();
        let revision = fork.revision();
        let preview = match fork.apply_hosted(
            revision,
            Operation::Housing(HousingCommand::RecordUpdate { read }),
            super::hosted(),
        ) {
            Ok(_) => housing::diff(self.engine.project(), fork.project(), revision, hash),
            Err(error) => {
                housing::refused(self.engine.project(), revision, hash, error.to_string())
            }
        };
        Ok(serde_json::to_value(preview)?)
    }

    /// `housing.update`: a link when none is linked; else the linked file,
    /// or another (a relink), applied at the hash its preview showed.
    pub(super) fn housing_update(&mut self, params: UpdateParams) -> Result<Value> {
        if self.engine.project().housing.is_none() {
            return self.housing_link(params);
        }
        let expect = params.expect_hash.as_deref().ok_or_else(|| {
            refused(
                "housing.expect_hash: an enclosure is linked; preview it and update at the \
                 expect_hash the preview gave"
                    .into(),
            )
        })?;
        let read = self.update_read(&params.path, &params.design)?;
        if read.file_sha256 != housing::expected_hash(expect)? {
            return Err(CoreError::Conflict(
                "housing.changed: the enclosure changed since the preview; preview it again".into(),
            )
            .into());
        }
        let mut reply = self.apply(
            params.expected_revision,
            Operation::Housing(HousingCommand::RecordUpdate { read }),
        )?;
        reply["housing"] = serde_json::to_value(self.engine.project().housing.as_deref())?;
        Ok(reply)
    }

    /// The dry run of a publish on the linked enclosure at `enclosure`: the
    /// candidate design built into the copy's scratch folder, the copy's
    /// board link pointed at it. `design` is the published design the
    /// enclosure links. `None` when the board was not published there yet.
    pub(super) fn dry_run(
        &self,
        enclosure: &str,
        design: &str,
        candidate: Sidecar,
    ) -> Result<Option<DryRun>> {
        let link = self.linked()?;
        self.same_files(&link, Some(enclosure), design)?;
        let mut opened = self.open_enclosure(enclosure, design)?;
        let Some(published) = opened.published.take() else {
            return Ok(None);
        };
        let project = self.engine.project_arc().clone();
        let exported = pcbcraft_solidcraft::interface::build(&project, candidate)?;
        let built = opened.copy.scratch().join("candidate.solidcraft");
        let built_text = built.to_string_lossy().into_owned();
        exported.save_new(&built_text)?;
        let effects =
            opened
                .enclosure
                .dry_run(&opened.board, &built_text, &published, &exported.sidecar)?;
        Ok(Some(DryRun {
            enclosure: enclosure.to_owned(),
            effects,
        }))
    }
}

/// The enclosure features that use the published datums, with the datum's
/// id and name as published.
fn consumers(opened: &Opened) -> Vec<Consumer> {
    let Some(sidecar) = opened.published.as_ref() else {
        return Vec::new();
    };
    opened
        .enclosure
        .uses(&opened.board, sidecar)
        .into_iter()
        .filter_map(|u| {
            let d = sidecar.datums.iter().find(|d| d.token == u.token)?;
            Some(Consumer {
                feature: u.feature,
                datum: d.id,
                token: u.token,
                name: d.name.clone(),
            })
        })
        .collect()
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
