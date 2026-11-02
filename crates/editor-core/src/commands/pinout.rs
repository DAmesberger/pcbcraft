//! Slice C3: handlers of `library.pinout_preview` (query) and
//! `library.pinout_apply`. Preview stages the import on a scratch copy of
//! the document and reports its changes, review items and every publish
//! finding the resulting drafts would have. Apply is atomic and one undo
//! step (the engine's candidate), refused when the target changed since
//! the plan's `base` (the stale-plan rule of updates).
use super::Outcome;
use crate::completeness;
use crate::diagnostics::{Error, Finding, Result};
use crate::ids::DefinitionId;
use crate::library::drafts;
use crate::library::pinout::{
    self, PinoutAccept, PinoutApplied, PinoutApply, PinoutDocument, PinoutPlan, PinoutTarget,
    StagedImport,
};
use crate::model::Document;
use std::sync::Arc;

#[cfg(test)]
mod tests;

/// Stage the import on `document` and store its drafts.
fn stage(
    document: &mut Document,
    target: &PinoutTarget,
    pinout: &PinoutDocument,
    accept: &PinoutAccept,
) -> Result<StagedImport> {
    let project = Arc::make_mut(&mut document.project);
    let library = std::mem::take(&mut project.library);
    let staged = {
        let mut allocate = || project.allocate().map(DefinitionId);
        pinout::stage(
            &library,
            &document.drafts,
            target,
            pinout,
            accept,
            &mut allocate,
        )
    };
    project.library = library;
    let staged = staged?;
    let store = Arc::make_mut(&mut document.drafts);
    let mut edits = Vec::new();
    for draft in &staged.drafts {
        if draft.create {
            drafts::create(
                store,
                &document.project.library,
                draft.id,
                None,
                draft.meta.clone(),
                draft.definition.clone(),
            )?;
        } else {
            edits.push(crate::library::DraftEdit {
                id: draft.id,
                meta: draft.meta.clone(),
                definition: draft.definition.clone(),
            });
        }
    }
    if !edits.is_empty() {
        drafts::edit(store, edits)?;
    }
    Ok(staged)
}

/// `library.pinout_preview`.
pub(crate) fn preview(
    document: &Document,
    target: &PinoutTarget,
    pinout: &PinoutDocument,
) -> Result<PinoutPlan> {
    let base = pinout::base_token(&document.project.library, &document.drafts, target)?;
    let mut scratch = document.clone();
    let staged = stage(&mut scratch, target, pinout, &PinoutAccept::All)?;
    let mut findings = staged.findings;
    let reports = completeness::drafts(&scratch.project, &scratch.drafts)?;
    for draft in &staged.drafts {
        for report in &reports {
            if report.target == completeness::CheckTarget::Draft(draft.id) {
                findings.extend(report.findings.iter().cloned());
            }
        }
    }
    dedupe(&mut findings);
    let rules = staged
        .drafts
        .iter()
        .find_map(|d| match &d.definition {
            crate::library::Definition::Component(c) => {
                Some(crate::functions::rules::check_samples(
                    &c.declarations(),
                    &c.resources,
                    &c.pin_rules,
                ))
            }
            _ => None,
        })
        .unwrap_or_default();
    Ok(PinoutPlan {
        base,
        changes: staged.changes,
        findings,
        review: staged.review,
        rules,
    })
}

/// Drop exact repeats, keeping the first.
fn dedupe(findings: &mut Vec<Finding>) {
    let mut kept: Vec<Finding> = Vec::with_capacity(findings.len());
    for finding in findings.drain(..) {
        if !kept.contains(&finding) {
            kept.push(finding);
        }
    }
    *findings = kept;
}

/// `library.pinout_apply` (`Outcome::PinoutApplied`).
pub(crate) fn apply(document: &mut Document, request: PinoutApply) -> Result<Outcome> {
    let base = pinout::base_token(&document.project.library, &document.drafts, &request.target)?;
    if base != request.base {
        return Err(Error::Conflict(
            "pinout.stale_base: the target changed since this plan was previewed; preview the pinout again"
                .into(),
        ));
    }
    let staged = stage(
        document,
        &request.target,
        &request.document,
        &request.accept,
    )?;
    Ok(Outcome::PinoutApplied {
        report: PinoutApplied {
            drafts: staged.drafts.iter().map(|d| d.id).collect(),
            unreviewed: staged.written,
        },
    })
}
