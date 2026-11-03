//! Slice D1: update-plan lines of a package's library model. A new model
//! revision reaches instances through a new package revision (packages pin
//! exact model revisions); these lines say what changes physically:
//! format, asset, bounds, lead tags and contacts, body mapping, marking
//! area and redistribution.
use crate::library::model3d::ModelDefinition;
use crate::library::{PackageDefinition, PackageModel, Published};
use crate::model::Project;

/// The library model a package revision uses, if it resolves.
pub(super) fn of<'a>(
    project: &'a Project,
    package: Option<&PackageDefinition>,
) -> Option<&'a Published<ModelDefinition>> {
    match &package?.model {
        PackageModel::Library(model) => project.library.model(model.model),
        PackageModel::PartFile(part) => project.library.model(part.model),
        PackageModel::Envelope { .. } | PackageModel::Generated { .. } => None,
    }
}

fn size(m: &ModelDefinition) -> String {
    let s = m.size_mm();
    format!("{:.3}×{:.3}×{:.3}", s[0], s[1], s[2])
}

/// Differences between two library model revisions. Empty when both are
/// absent or equal; a model added or removed is the package line's.
pub fn model_details(
    a: Option<&Published<ModelDefinition>>,
    b: Option<&Published<ModelDefinition>>,
) -> Vec<String> {
    let (Some(a), Some(b)) = (a, b) else {
        return Vec::new();
    };
    let (x, y) = (&a.definition, &b.definition);
    let mut out = Vec::new();
    if x == y {
        return out;
    }
    if x.format != y.format {
        out.push(format!(
            "3D model format {} → {}.",
            x.format.label(),
            y.format.label()
        ));
    }
    if x.asset.hash != y.asset.hash {
        out.push(format!(
            "3D model file ‘{}’ → ‘{}’.",
            x.asset.file_name, y.asset.file_name
        ));
    }
    if x.pair.as_ref().map(|p| p.hash) != y.pair.as_ref().map(|p| p.hash) {
        out.push("3D model appearance pair changes.".into());
    }
    if x.bounds_mm != y.bounds_mm || x.frame != y.frame {
        out.push(format!(
            "3D model bounds {} → {} mm (L×W×H).",
            size(x),
            size(y)
        ));
    }
    if x.leads.len() != y.leads.len() {
        out.push(format!(
            "3D model lead tags {} → {}.",
            x.leads.len(),
            y.leads.len()
        ));
    }
    // Lead tags are compared by index, the key a package's lead map uses.
    let mut moved: Option<(f64, &str)> = None;
    for (p, q) in x.leads.iter().zip(&y.leads) {
        if p.designation != q.designation {
            out.push(format!(
                "3D model lead {} designation ‘{}’ → ‘{}’.",
                q.key, p.designation, q.designation
            ));
        }
        let d = (0..3)
            .map(|i| (p.contact[i] - q.contact[i]).powi(2))
            .sum::<f64>()
            .sqrt();
        if d > 0.0 && moved.is_none_or(|(m, _)| d > m) {
            moved = Some((d, q.designation.as_str()));
        }
    }
    if let Some((d, lead)) = moved {
        out.push(format!(
            "3D model lead contacts move by up to {d:.3} mm (lead ‘{lead}’)."
        ));
    }
    if x.bodies != y.bodies || x.custom_materials != y.custom_materials {
        out.push(format!(
            "3D model body roles or materials change ({} → {} bodies).",
            x.bodies.len(),
            y.bodies.len()
        ));
    }
    if x.marking_area != y.marking_area {
        out.push(
            match (x.marking_area.is_some(), y.marking_area.is_some()) {
                (false, true) => "3D model marking area added.",
                (true, false) => "3D model marking area removed.",
                _ => "3D model marking area moves.",
            }
            .into(),
        );
    }
    if x.provenance.redistribution != y.provenance.redistribution {
        out.push(format!(
            "3D model redistribution {} → {}.",
            x.provenance.redistribution.label(),
            y.provenance.redistribution.label()
        ));
    }
    out
}
