//! Identity after a format change. Revision content hashes are SHA-256 of
//! a record's serde encoding, so converting a revision changes its hash
//! while its identity — lineage, revision number and every pin to it —
//! stays. A project remembers the hash of each revision it imported from or
//! promoted to a library ([`Origin::content_hash`]), and the hash of the
//! model design each 3D dimension reference was traced on
//! ([`ModelTrace::design`]). Both are recomputed here from the converted
//! project, exactly as import and tracing compute them, so a converted
//! project still recognises its revisions in a converted library folder
//! (no "edited after import" report, imports dedupe, updates are offered)
//! and its dimension references stay exact.
//!
//! Lineages imported from another project file are named by a hash of
//! their first revision in that file (`import::project_lineage_id`), which
//! includes that file's own id; the copy cannot recompute it, so it keeps
//! its old name and a later import of the converted file brings the
//! lineage in again under its new name. The run lists those lineages.
use crate::Result;
use sparksmith_core::format::EditorFile;
use sparksmith_core::ids::{DefinitionKind, DefinitionRef, LibraryId};
use sparksmith_core::layout::dimensions::model::package_design_key;
use sparksmith_core::library::bundle::{OriginRecord, SourceRevision, content_hash, to_record};
use sparksmith_core::library::import::PROJECT_FILES;

/// What [`refresh`] changed, and what it could not.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Refreshed {
    /// Origin hashes recomputed.
    pub origins: usize,
    /// Model-trace design keys recomputed.
    pub design_keys: usize,
    /// Revisions from other project files whose lineage names cannot be
    /// recomputed, described.
    pub project_file_lineages: Vec<String>,
    /// Fill keys recomputed: the refill was the stored copper within
    /// [`FILL_MATCH_MM`].
    pub fill_keys: usize,
    /// The farthest a recomputed fill's copper moved, nanometres.
    pub fill_moved_nm: u64,
    /// Fills whose refill would change their copper: left stale.
    pub stale_fills: Vec<sparksmith_core::Id>,
}

impl Refreshed {
    pub fn changed(&self) -> usize {
        self.origins + self.design_keys + self.fill_keys
    }
}

/// Recompute the hashes a converted project remembers.
pub fn refresh(file: &mut EditorFile) -> Result<Refreshed> {
    let mut out = Refreshed::default();
    let origins: Vec<OriginRecord> = file.project.library.origins.to_vec();
    let mut next = origins.clone();
    for (i, record) in origins.iter().enumerate() {
        let hash = origin_hash(file, &origins, record)?;
        if hash != record.origin.content_hash {
            next[i].origin.content_hash = hash;
            out.origins += 1;
        }
        if record.origin.library == PROJECT_FILES {
            out.project_file_lineages.push(
                file.project
                    .library
                    .describe_lineage(record.local.kind, record.local.id),
            );
        }
    }
    if out.origins > 0 {
        *file.project.library.origins = next;
    }

    let traces = file.project.model_traces.to_vec();
    let mut next = traces.clone();
    for (i, trace) in traces.iter().enumerate() {
        let (Some(package), Some(design)) = (trace.package, trace.design) else {
            continue;
        };
        // A package the project no longer holds, or a design that cannot be
        // traced any more, keeps its key: the trace reports it as it did.
        let Ok(key) = package_design_key(&file.project, package) else {
            continue;
        };
        if key != design {
            next[i].design = Some(key);
            out.design_keys += 1;
        }
    }
    if out.design_keys > 0 {
        *file.project.model_traces = next;
    }
    fill_keys(file, &mut out)?;
    out.project_file_lineages.sort();
    out.project_file_lineages.dedup();
    Ok(out)
}

/// How far a refill may move a fill's outline and still count as the
/// stored copper, mm. Converting rounds every stored length by at most
/// half a nanometre; a fill flattens its arcs to chords within
/// `FILL_CHORD_MM` (5 µm), so a sub-nanometre change can still move a
/// chord end where a segment count tips over (0.27 µm on the ESP32
/// board). 1 µm is above that and a fifth of the chord tolerance: any real
/// edit of the board moves copper by more.
pub const FILL_MATCH_MM: f64 = 0.001;

/// Most vertex-to-edge distances one fill comparison measures.
const MAX_FILL_COMPARISONS: usize = 200_000_000;

/// A fill's stored copper is keyed by a hash of its serialized inputs,
/// pads included, so a format change alone makes every fill read stale.
/// Each fill whose key no longer matches is refilled; when the refill is
/// the stored copper within [`FILL_MATCH_MM`] (the converted pads are the
/// nanometre rounding of the old ones), the refill is kept under its new
/// key. A fill whose copper would move farther was stale before the
/// conversion or depends on one that was: it keeps its copper and old key,
/// and still reads stale.
fn fill_keys(file: &mut EditorFile, out: &mut Refreshed) -> Result<()> {
    use sparksmith_core::layout::fill::{RefillTarget, refill};
    let refilled = refill(&file.project, RefillTarget::Stale)?;
    if refilled.is_empty() {
        return Ok(());
    }
    let mut fills = file.project.fills.to_vec();
    for (id, copper) in refilled {
        let Some(fill) = fills.iter_mut().find(|f| f.id == id) else {
            continue;
        };
        let moved = fill.computed.as_ref().and_then(|stored| {
            (stored.removed_islands == copper.removed_islands)
                .then(|| copper_moved(&stored.regions, &copper.regions))
                .flatten()
        });
        match moved {
            Some(moved) if moved <= FILL_MATCH_MM => {
                fill.computed = Some(copper);
                out.fill_keys += 1;
                // Under 1 µm: the cast cannot overflow.
                let nm = (moved * 1e6).ceil() as u64;
                out.fill_moved_nm = out.fill_moved_nm.max(nm);
            }
            _ => out.stale_fills.push(id),
        }
    }
    if out.fill_keys > 0 {
        *file.project.fills = fills;
    }
    Ok(())
}

type Ring = [sparksmith_core::geometry::Point];

/// The farthest any outline of `a` lies from its counterpart in `b` (each
/// vertex from the other ring's edges, both ways), mm; `None` when the
/// regions or their holes do not pair up one to one, or the comparison
/// would be too large.
fn copper_moved(
    a: &[sparksmith_core::model::fill::FilledRegion],
    b: &[sparksmith_core::model::fill::FilledRegion],
) -> Option<f64> {
    if a.len() != b.len() {
        return None;
    }
    let mut pairs: Vec<(&Ring, &Ring)> = Vec::new();
    for (x, y) in a.iter().zip(b) {
        if x.holes.len() != y.holes.len() {
            return None;
        }
        pairs.push((&x.outer, &y.outer));
        pairs.extend(x.holes.iter().zip(&y.holes).map(|(h, k)| (&h[..], &k[..])));
    }
    let work: usize = pairs
        .iter()
        .map(|(x, y)| x.len().saturating_mul(y.len()))
        .fold(0usize, usize::saturating_add);
    if work > MAX_FILL_COMPARISONS / 2 {
        return None;
    }
    pairs.iter().try_fold(0.0f64, |worst, (x, y)| {
        Some(worst.max(ring_distance(x, y)?))
    })
}

/// The symmetric vertex-to-edge distance of two closed rings, mm.
fn ring_distance(x: &Ring, y: &Ring) -> Option<f64> {
    fn edge(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let len2 = dx * dx + dy * dy;
        let t = if len2 > 0.0 {
            (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / len2).clamp(0.0, 1.0)
        } else {
            0.0
        };
        (p[0] - a[0] - t * dx).hypot(p[1] - a[1] - t * dy)
    }
    fn one_way(x: &Ring, y: &Ring) -> Option<f64> {
        let mut worst = 0.0f64;
        for p in x {
            let near = (0..y.len())
                .map(|i| edge(*p, y[i], y[(i + 1) % y.len()]))
                .fold(f64::INFINITY, f64::min);
            worst = worst.max(near);
        }
        worst.is_finite().then_some(worst)
    }
    if x.is_empty() || y.is_empty() {
        return None;
    }
    Some(one_way(x, y)?.max(one_way(y, x)?))
}

/// The content hash of the source record a project revision was copied
/// from (or promoted as): the project's copy written back as a record of
/// its library, every reference mapped through that library's origins, as
/// `library::import::project_bundle` and promotion write records.
fn origin_hash(
    file: &EditorFile,
    origins: &[OriginRecord],
    record: &OriginRecord,
) -> Result<sparksmith_core::ids::ContentHash> {
    let library = &file.project.library;
    let source = record.origin.library;
    let (meta, body) = library
        .definition(record.local)
        .ok_or_else(|| format!("{} is not in the library", library.describe(record.local)))?;
    let of = |source: LibraryId| origins.iter().filter(move |o| o.origin.library == source);
    let converted = to_record(
        record.origin.lineage,
        record.origin.revision,
        meta,
        body,
        &mut |r: DefinitionRef| {
            of(source)
                .find(|o| o.local == r)
                .map(|o| SourceRevision {
                    lineage: o.origin.lineage,
                    revision: o.origin.revision,
                })
                .ok_or_else(|| {
                    sparksmith_core::Error::NotFound(format!(
                        "{} names {}, which came from no revision of its library",
                        library.describe(record.local),
                        library.describe(r)
                    ))
                })
        },
        &mut |kind: DefinitionKind, id| {
            of(source)
                .find(|o| o.local.kind == kind && o.local.id == id)
                .map(|o| o.origin.lineage)
                .ok_or_else(|| {
                    sparksmith_core::Error::NotFound(format!(
                        "{} names {}, which came from no lineage of its library",
                        library.describe(record.local),
                        library.describe_lineage(kind, id)
                    ))
                })
        },
    )?;
    Ok(content_hash(&converted)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sparksmith_core::ids::{ContentHash, DefinitionId};
    use sparksmith_core::library::import;

    /// Every Part of the synthetic catalog imported into an empty project,
    /// with the origins import recorded.
    fn imported() -> Result<EditorFile> {
        let board = sparksmith_io::new_project::shipped()?.board;
        let source = sparksmith_core::catalog::synthetic::project_with_parts(4, &board)?.library;
        let mut file = EditorFile {
            version: sparksmith_core::format::FILE_VERSION,
            project: Default::default(),
            drafts: Default::default(),
        };
        let mut next = 1000;
        for part in source.latest_parts() {
            let root = DefinitionRef {
                kind: DefinitionKind::Part,
                id: part.id,
                revision: part.revision,
            };
            let bundle = import::project_bundle(&source, root, "source")?;
            import::import(&mut file.project.library, &bundle, &mut || {
                next += 1;
                Ok(DefinitionId(next))
            })?;
        }
        Ok(file)
    }

    fn region(outer: Vec<[f64; 2]>) -> sparksmith_core::model::fill::FilledRegion {
        sparksmith_core::model::fill::FilledRegion {
            outer,
            holes: Vec::new(),
        }
    }

    /// A refill that re-flattens a fill (another vertex count, a chord end
    /// 0.3 µm off) is the stored copper; one moved by 2 µm, or with
    /// another region count, is not.
    #[test]
    fn a_refill_within_a_micrometre_is_the_stored_copper() {
        let square = vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        let mut reflattened = square.clone();
        reflattened.insert(1, [0.5, -0.000_3]);
        let moved = copper_moved(&[region(square.clone())], &[region(reflattened)]);
        assert!(
            moved.is_some_and(|m| (m - 0.000_3).abs() < 1e-12),
            "{moved:?}"
        );
        let shifted: Vec<[f64; 2]> = square.iter().map(|p| [p[0] + 0.002, p[1]]).collect();
        let moved = copper_moved(&[region(square.clone())], &[region(shifted)]);
        assert!(moved.is_some_and(|m| m > FILL_MATCH_MM), "{moved:?}");
        assert_eq!(
            copper_moved(
                &[region(square.clone())],
                &[region(square.clone()), region(square)]
            ),
            None
        );
    }

    #[test]
    fn origin_hashes_are_recomputed_as_import_records_them() -> Result<()> {
        let mut file = imported()?;
        let recorded = file.project.library.origins.to_vec();
        assert!(!recorded.is_empty());
        // Recomputing an up-to-date project changes nothing.
        assert_eq!(refresh(&mut file)?.changed(), 0);
        // Stale hashes, as a format change leaves them, come back exactly.
        for origin in file.project.library.origins.iter_mut() {
            origin.origin.content_hash = ContentHash([7; 32]);
        }
        let refreshed = refresh(&mut file)?;
        assert_eq!(refreshed.origins, recorded.len());
        assert!(!refreshed.project_file_lineages.is_empty());
        assert_eq!(*file.project.library.origins, recorded);
        Ok(())
    }
}
