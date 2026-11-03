//! Package definitions: physical terminals, the typed form, the envelope,
//! the 3D model source and its placement.
use super::package_form::{ChipForm, ChipKind, PackageForm};
use super::package_model::{MAX_ROLE_MATERIALS, ModelPlacement, PackageModel};
use super::publish::{issue, object};
use super::{MAX_LABEL, MAX_PACKAGE_TERMINALS, printable};
use crate::diagnostics::{Error, Finding, ObjectRef, Result};
use crate::geometry::{Point3, finite_within, size_valid};
use crate::ids::{DefinitionId, PackageTerminalId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PackageDefinition {
    pub next_local: u32,
    pub terminals: Vec<PackageTerminal>,
    /// Typed physical parameters, the only source of `pkg_*` values.
    /// `Generic` has only the authored envelope.
    pub form: PackageForm,
    /// Envelope length, width and height in mm, in the model frame from the
    /// seating plane up, centred in x and y. Derived from a typed `form`
    /// (`PackageForm::envelope`); authored for `Generic`.
    pub envelope: [f64; 3],
    /// Where the 3D model comes from and its body materials.
    pub model: PackageModel,
    /// The model in the datum frame (`model_frame`).
    pub placement: ModelPlacement,
    /// Set when the core generated this package's set from a part file
    /// (`layout.place_part`, docs/mechanical-parts.md): the variant record.
    /// Display, dedupe and assembly flags only; electrical rules never
    /// read it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mechanical: Option<Box<super::mechanical::MechanicalPart>>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PackageTerminal {
    pub id: PackageTerminalId,
    /// Printed or datasheet designation, e.g. `1` or `EP`. Never identity.
    pub designation: String,
}

impl PackageDefinition {
    pub fn terminal(&self, id: PackageTerminalId) -> Option<&PackageTerminal> {
        self.terminals.iter().find(|t| t.id == id)
    }

    /// A positive finite envelope with a finite placement: the condition
    /// for a native envelope model.
    pub fn envelope_valid(&self) -> bool {
        size_valid(&self.envelope) && self.placement.finite()
    }

    /// The chip construction of a typed chip form.
    pub fn chip_kind(&self) -> Option<ChipKind> {
        match &self.form {
            PackageForm::Chip(c) => Some(c.kind),
            _ => None,
        }
    }

    /// Set the envelope to `size`, keeping a typed form in step: a chip
    /// takes the size as its length, width and height (its termination
    /// kept, or clamped below half the length). Other typed forms derive
    /// the envelope from several parameters, so they refuse.
    pub fn resize_envelope(&mut self, size: [f64; 3]) -> Result<()> {
        match &mut self.form {
            PackageForm::Generic => {}
            PackageForm::Chip(c) => {
                let termination = c.termination.min(0.45 * size[0]);
                **c = ChipForm {
                    termination,
                    ..ChipForm::new(c.kind, size)
                };
            }
            other => {
                return Err(Error::Invalid(format!(
                    "package.envelope: a {} envelope follows its parameters",
                    other.label().to_lowercase()
                )));
            }
        }
        self.envelope = size;
        Ok(())
    }

    /// The envelope box's corners in the model frame (z = 0 up).
    pub fn envelope_corners(&self) -> [Point3; 8] {
        let [l, w, h] = self.envelope;
        std::array::from_fn(|i| {
            [
                if i & 1 == 0 { -l / 2.0 } else { l / 2.0 },
                if i & 2 == 0 { -w / 2.0 } else { w / 2.0 },
                if i & 4 == 0 { 0.0 } else { h },
            ]
        })
    }

    pub(super) fn within_bounds(&self) -> bool {
        self.terminals.len() <= MAX_PACKAGE_TERMINALS
            && self
                .terminals
                .iter()
                .all(|t| t.designation.len() <= MAX_LABEL)
            && self.form.within_bounds()
            && match &self.model {
                PackageModel::Generated { materials, .. } => {
                    materials.roles.len() <= MAX_ROLE_MATERIALS
                }
                PackageModel::PartFile(part) => {
                    part.values.len() <= super::package_model::MAX_PART_VALUES
                        && part.derived.surfaces.len() <= super::package_model::MAX_PART_SURFACES
                }
                _ => true,
            }
    }

    pub(super) fn geometry_finite(&self) -> bool {
        finite_within(&self.envelope)
            && finite_within(&self.form.lengths())
            && self.placement.finite()
    }

    pub(super) fn local_ids(&self) -> Vec<u32> {
        self.terminals.iter().map(|t| t.id.0).collect()
    }

    pub(super) fn findings(&self, id: DefinitionId, out: &mut Vec<Finding>) {
        let this = ObjectRef::Definition { id };
        // A part-file body may be purely mechanical (docs/mechanical-parts.md).
        let least = usize::from(!matches!(self.model, super::PackageModel::PartFile(_)));
        if !(least..=MAX_PACKAGE_TERMINALS).contains(&self.terminals.len()) {
            out.push(issue(
                "package.terminal_count",
                format!("A package needs {least}–{MAX_PACKAGE_TERMINALS} physical terminals."),
                vec![this],
            ));
        }
        let mut designations = BTreeSet::new();
        for terminal in &self.terminals {
            if !printable(&terminal.designation) || !designations.insert(&terminal.designation) {
                out.push(issue(
                    "package.designation",
                    "A package terminal needs a unique printable designation.".into(),
                    vec![object(id, terminal.id.0)],
                ));
            }
        }
        for problem in self.form.range_problems() {
            out.push(issue("package.form_range", problem, vec![this]));
        }
        let counts = self.form.terminal_counts();
        let terminals = u64::try_from(self.terminals.len()).unwrap_or(u64::MAX);
        if !counts.is_empty() && !counts.contains(&terminals) {
            let expected = match counts.as_slice() {
                [2] => "exactly two".to_owned(),
                _ => counts
                    .iter()
                    .map(u64::to_string)
                    .collect::<Vec<_>>()
                    .join(" or "),
            };
            out.push(issue(
                "package.form_terminals",
                format!(
                    "A {} package has {expected} terminals; this one has {terminals}.",
                    self.form.label().to_lowercase()
                ),
                vec![this],
            ));
        }
        if let Some(derived) = self.form.envelope()
            && derived != self.envelope
        {
            out.push(issue(
                "package.envelope_derived",
                format!(
                    "The envelope {} mm differs from the {} form's {} mm.",
                    size_text(&self.envelope),
                    self.form.label().to_lowercase(),
                    size_text(&derived)
                ),
                vec![this],
            ));
        }
        match &self.model {
            PackageModel::Library(model) => {
                super::model3d::package_findings(self, model, id, out);
            }
            PackageModel::Generated { materials, .. } => {
                if self.form == PackageForm::Generic {
                    out.push(issue(
                        "package.model_form",
                        "A generated model needs a typed form; a generic package has an envelope."
                            .into(),
                        vec![this],
                    ));
                }
                let mut roles = BTreeSet::new();
                if !materials.roles.iter().all(|r| roles.insert(r.role)) {
                    out.push(issue(
                        "package.model_materials",
                        "Each body role takes at most one material.".into(),
                        vec![this],
                    ));
                }
            }
            PackageModel::PartFile(part) => {
                for problem in part.problems() {
                    out.push(issue(
                        "part_file.values",
                        format!("The part file: {problem}."),
                        vec![this],
                    ));
                }
            }
            PackageModel::Envelope { .. } => {}
        }
        if !self.envelope_valid() {
            out.push(issue(
                "package.envelope",
                "Package envelope dimensions must be finite and positive.".into(),
                vec![this],
            ));
        }
    }
}

/// A size for a message: "1.6 × 0.8 × 0.45".
fn size_text(size: &[f64]) -> String {
    size.iter()
        .map(|v| format!("{v}"))
        .collect::<Vec<_>>()
        .join(" × ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::package_form::{LeadSides, NoLeadForm};
    use crate::library::package_model::{BodyRole, GeneratedMaterials, RoleMaterial};
    use crate::materials::MaterialId;

    fn codes(package: &PackageDefinition) -> Vec<String> {
        let mut out = Vec::new();
        package.findings(DefinitionId(1), &mut out);
        out.into_iter().map(|f| f.code).collect()
    }

    fn chip(kind: ChipKind) -> PackageDefinition {
        let form = ChipForm::new(kind, [1.6, 0.8, 0.45]);
        PackageDefinition {
            next_local: 3,
            terminals: (1..=2)
                .map(|i| PackageTerminal {
                    id: PackageTerminalId(i),
                    designation: i.to_string(),
                })
                .collect(),
            form: PackageForm::Chip(Box::new(form)),
            envelope: [1.6, 0.8, 0.45],
            model: PackageModel::Generated {
                family_rev: 1,
                materials: GeneratedMaterials::default(),
            },
            placement: ModelPlacement::default(),
            mechanical: None,
        }
    }

    #[test]
    fn a_consistent_chip_has_no_findings() {
        assert_eq!(codes(&chip(ChipKind::Resistor)), Vec::<String>::new());
    }

    #[test]
    fn form_terminal_and_envelope_findings() {
        let mut p = chip(ChipKind::Capacitor);
        p.terminals.push(PackageTerminal {
            id: PackageTerminalId(3),
            designation: "3".into(),
        });
        assert!(codes(&p).contains(&"package.form_terminals".to_owned()));
        let mut p = chip(ChipKind::Capacitor);
        p.envelope[2] = 1.0;
        assert_eq!(codes(&p), ["package.envelope_derived"]);
        let mut p = chip(ChipKind::Capacitor);
        if let PackageForm::Chip(c) = &mut p.form {
            c.termination = 1.0;
        }
        assert_eq!(codes(&p), ["package.form_range"]);
    }

    #[test]
    fn an_exposed_pad_may_be_a_terminal() {
        let form = NoLeadForm {
            sides: LeadSides::Two,
            terminals_per_side: [3, 0],
            pitch: 0.95,
            body_length: 3.0,
            body_width: 3.0,
            body_height: 0.9,
            terminal_length: 0.4,
            terminal_width: 0.35,
            exposed_pad: Some([2.4, 1.6]),
        };
        let mut p = PackageDefinition {
            terminals: (1..=6)
                .map(|i| PackageTerminal {
                    id: PackageTerminalId(i),
                    designation: i.to_string(),
                })
                .collect(),
            envelope: [3.0, 3.0, 0.9],
            form: PackageForm::NoLead(Box::new(form)),
            ..PackageDefinition::default()
        };
        assert_eq!(codes(&p), Vec::<String>::new(), "six terminals");
        p.terminals.push(PackageTerminal {
            id: PackageTerminalId(7),
            designation: "EP".into(),
        });
        assert_eq!(codes(&p), Vec::<String>::new(), "six plus the pad");
        p.terminals.push(PackageTerminal {
            id: PackageTerminalId(8),
            designation: "8".into(),
        });
        assert_eq!(codes(&p), ["package.form_terminals"]);
    }

    #[test]
    fn model_findings_and_bounds() {
        let mut p = chip(ChipKind::Resistor);
        p.form = PackageForm::Generic;
        assert_eq!(codes(&p), ["package.model_form"]);
        let mut p = chip(ChipKind::Resistor);
        p.model = PackageModel::Generated {
            family_rev: 1,
            materials: GeneratedMaterials {
                roles: vec![
                    RoleMaterial {
                        role: BodyRole::Body,
                        material: MaterialId::Alumina96,
                    },
                    RoleMaterial {
                        role: BodyRole::Body,
                        material: MaterialId::Ferrite,
                    },
                ],
            },
        };
        assert_eq!(codes(&p), ["package.model_materials"]);
        p.model = PackageModel::Generated {
            family_rev: 1,
            materials: GeneratedMaterials {
                roles: vec![
                    RoleMaterial {
                        role: BodyRole::Other,
                        material: MaterialId::Pbt,
                    };
                    MAX_ROLE_MATERIALS + 1
                ],
            },
        };
        assert!(!p.within_bounds(), "too many role entries is a refusal");
        let mut p = chip(ChipKind::Resistor);
        p.model = PackageModel::Library(Default::default());
        assert_eq!(codes(&p), Vec::<String>::new(), "an empty lead map");
        let entry = |lead, terminal| crate::library::model3d::LeadMapEntry {
            lead,
            terminal: PackageTerminalId(terminal),
        };
        p.model = PackageModel::Library(crate::library::model3d::LibraryModel {
            model: Default::default(),
            lead_map: vec![entry(0, 1), entry(0, 2), entry(1, 9), entry(2, 1)],
        });
        assert_eq!(
            codes(&p),
            ["model.leads", "model.leads", "model.leads"],
            "a repeated lead, a foreign terminal and a repeated terminal"
        );
        let mut p = chip(ChipKind::Resistor);
        p.placement.rotation_deg[0] = f64::INFINITY;
        assert!(!p.geometry_finite());
    }

    #[test]
    fn resizing_keeps_a_chip_in_step() -> Result<()> {
        let mut p = chip(ChipKind::Resistor);
        p.resize_envelope([0.6, 0.3, 0.25])?;
        assert_eq!(p.form.envelope(), Some([0.6, 0.3, 0.25]));
        assert!(codes(&p).is_empty(), "{:?}", codes(&p));
        let mut generic = PackageDefinition::default();
        generic.resize_envelope([5.0, 4.0, 1.0])?;
        assert_eq!(generic.envelope, [5.0, 4.0, 1.0]);
        let mut p = PackageDefinition {
            form: PackageForm::NoLead(Box::new(NoLeadForm {
                sides: LeadSides::Two,
                terminals_per_side: [1, 0],
                pitch: 1.0,
                body_length: 1.0,
                body_width: 1.0,
                body_height: 0.5,
                terminal_length: 0.3,
                terminal_width: 0.5,
                exposed_pad: None,
            })),
            envelope: [1.0, 1.0, 0.5],
            ..PackageDefinition::default()
        };
        assert!(p.resize_envelope([2.0, 2.0, 2.0]).is_err());
        assert_eq!(p.envelope, [1.0, 1.0, 0.5], "a refusal changes nothing");
        Ok(())
    }
}
