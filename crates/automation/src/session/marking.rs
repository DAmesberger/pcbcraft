//! `library.set_marking`: set (or clear) a component draft's generic
//! marking with SVG artwork given as text. The session packs each SVG as
//! an asset named by its SHA-256, which an agent cannot compute, and
//! applies `library.pack_artwork` (or `library.edit` without artwork): one
//! atomic undo step, refused whole on any problem. Markings stay
//! display-only (founder decision F7).
//!
//! `library.marking_surfaces` lists where a package can be marked: its
//! model area, its six box faces and the named anchors its family offers
//! (a module's `can_top`, `pcb_top`, `antenna_area`; a radial can's `top`
//! and `sleeve`; an axial part's or a MELF's `body`), each with what it is
//! and its size, so an agent placing a datasheet's marking can choose.
use super::{Session, hosted};
use crate::Result;
use pcbcraft_core::library::marking::surface::{self, SurfaceGeometry};
use pcbcraft_core::library::marking::{
    ARTWORK_MEDIA_TYPE, Align, Anchor, AreaChoice, BoxFace, Ecc, Finish, MAX_ARTWORK_BYTES,
    MAX_DECALS, MAX_MARKING_ITEMS, MarkingItem, MarkingSpec, Placement, PolarityMark, SurfaceRef,
    Symbology, Template,
};
use pcbcraft_core::library::{Definition, DraftEdit, PackageDefinition, PackedAsset};
use pcbcraft_core::{DefinitionId, FieldRef, LibraryCommand, Operation};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetMarkingParams {
    /// The revision the edit was prepared against.
    pub expected_revision: u64,
    /// A component draft.
    pub draft: DefinitionId,
    /// The marking; `null` clears it.
    pub marking: Option<MarkingInput>,
}

/// A marking whose artwork is SVG text: items laid out in one surface
/// region (`placement`), plus up to four further `decals`, each its own
/// items, finish and surface (for example laser-etched text, logo and QR
/// code on a module's `can_top`, and a white silkscreen line on its
/// `pcb_top`). Text auto-fits the region unless given a height; items
/// anchored `start`, `end` or `pin1_corner` keep the text clear of them.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MarkingInput {
    /// 1–8 items.
    pub items: Vec<ItemInput>,
    /// With the `model` surface only: the model's marking area, or an
    /// explicit rectangle on its top face.
    #[serde(default)]
    pub area: AreaChoice,
    /// `laser_etch` (moulded bodies), `laser_etch_metal` (a shield can:
    /// darker, matte, still metal), `ink_white` (silkscreen, raised),
    /// `{"ink": {"colour": [r, g, b]}}`, `{"pad_print": {"colour": [r, g,
    /// b]}}` (sRGB 0–255) or `engraved` (the face's own look, recessed by
    /// shading only).
    #[serde(default)]
    pub finish: Finish,
    #[serde(default)]
    pub min_text_height_mm: Option<f64>,
    /// Where the marking goes: `on` is `{"surface": "model"}` (default:
    /// the top marking area), `{"surface": "face", "face": "top" |
    /// "bottom" | "pos_x" | "neg_x" | "pos_y" | "neg_y"}` (a box face of
    /// the outermost body; sides read upright from outside) or
    /// `{"surface": "anchor", "name": …}` (see `library.marking_surfaces`).
    /// Then `offset_mm` [u, v], `rotation_deg` (counter-clockwise seen
    /// from outside; a quarter turn swaps the region's sides for the
    /// layout), `scale`, `mirror`, `clip` (default true) and, on a
    /// cylinder, `wrap` {`seam_deg` (where the chart's start edge sits,
    /// from the anchor's zero direction; the chart centre is at seam +
    /// wrap/2), `wrap_deg` (1–360), `along_mm` [from, to] along the axis,
    /// `repeat` (1–8 copies round the wrap)}. Wrapping keeps arc length:
    /// a chart millimetre is a millimetre on the surface. Curved surfaces
    /// other than cylinders are not mapped.
    #[serde(default)]
    pub placement: Placement,
    /// Up to four further decals.
    #[serde(default)]
    pub decals: Vec<DecalInput>,
}

/// A further decal: its own items, finish and surface.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DecalInput {
    /// 1–8 items.
    pub items: Vec<ItemInput>,
    #[serde(default)]
    pub area: AreaChoice,
    #[serde(default)]
    pub finish: Finish,
    #[serde(default)]
    pub min_text_height_mm: Option<f64>,
    #[serde(default)]
    pub placement: Placement,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "item", rename_all = "snake_case", deny_unknown_fields)]
pub enum ItemInput {
    /// A text line from a template (`{mpn}`, `{designator}`,
    /// `{field:<id>}`, `{var:lot}`, literal text; a line break starts a
    /// new line).
    Text {
        template: String,
        #[serde(default)]
        height_mm: Option<f64>,
        #[serde(default)]
        align: Align,
    },
    /// SVG artwork as text (at most 1 MiB), with its licence (an SPDX id or
    /// the owner's terms). Paths, shapes, fills, strokes and transforms are
    /// drawn; draw text as outlines or use a `text` item. Never supply a
    /// manufacturer's logo you have no right to; a plain placeholder box
    /// marks where the user's own logo goes.
    Artwork {
        svg: String,
        size_mm: [f64; 2],
        #[serde(default)]
        anchor: Anchor,
        licence: String,
    },
    Polarity {
        mark: PolarityMark,
        #[serde(default)]
        at: Anchor,
    },
    /// A generated QR code of a template (`{designator}`, `{var:serial}`,
    /// `{mpn}`; at most 128 bytes rendered), `size_mm` the symbol's side
    /// without its 4-module quiet zone, which the region must also hold.
    Code2d {
        payload: String,
        size_mm: f64,
        #[serde(default)]
        anchor: Anchor,
        #[serde(default)]
        ecc: Ecc,
    },
    /// An aluminium electrolytic's marking from the instance's values:
    /// "<C>µF" and "<V>V" lines and the negative stripe with minus signs
    /// at the negative lead (angle 0 of a radial `sleeve`, the +x end
    /// elsewhere).
    Electrolytic {
        capacitance: FieldRef,
        #[serde(default)]
        voltage: Option<FieldRef>,
        #[serde(default = "yes")]
        stripe: bool,
    },
}

fn yes() -> bool {
    true
}

/// `library.marking_surfaces`: a package draft or published package
/// lineage.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MarkingSurfacesParams {
    pub package: DefinitionId,
}

fn invalid(m: String) -> crate::Error {
    crate::Error::Document(pcbcraft_core::Error::Invalid(m))
}

/// The items of one spec, artwork staged into `artwork`.
fn items(inputs: Vec<ItemInput>, artwork: &mut Vec<PackedAsset>) -> Result<Vec<MarkingItem>> {
    if inputs.is_empty() || inputs.len() > MAX_MARKING_ITEMS {
        return Err(invalid(format!(
            "marking.items: a marking has 1–{MAX_MARKING_ITEMS} items"
        )));
    }
    let mut out = Vec::with_capacity(inputs.len());
    for item in inputs {
        out.push(match item {
            ItemInput::Text {
                template,
                height_mm,
                align,
            } => MarkingItem::Text {
                template: Template(template),
                height_mm,
                align,
            },
            ItemInput::Artwork {
                svg,
                size_mm,
                anchor,
                licence,
            } => {
                if svg.len() > MAX_ARTWORK_BYTES || !svg.contains("<svg") {
                    return Err(invalid(format!(
                        "marking.artwork: artwork is SVG text of at most {} KiB",
                        MAX_ARTWORK_BYTES >> 10
                    )));
                }
                let data = pcbcraft_core::library::bundle::AssetBytes::new(svg.into_bytes());
                let hash = data.hash();
                if !artwork.iter().any(|a: &PackedAsset| a.hash == hash) {
                    artwork.push(PackedAsset {
                        hash,
                        media_type: ARTWORK_MEDIA_TYPE.into(),
                        data,
                    });
                }
                MarkingItem::Artwork {
                    svg: hash,
                    size_mm,
                    anchor,
                    licence: Some(licence),
                }
            }
            ItemInput::Polarity { mark, at } => MarkingItem::Polarity { mark, at },
            ItemInput::Code2d {
                payload,
                size_mm,
                anchor,
                ecc,
            } => MarkingItem::Code2d {
                symbology: Symbology::Qr,
                payload: Template(payload),
                size_mm,
                anchor,
                ecc,
            },
            ItemInput::Electrolytic {
                capacitance,
                voltage,
                stripe,
            } => MarkingItem::Electrolytic {
                capacitance,
                voltage,
                stripe,
            },
        });
    }
    Ok(out)
}

/// What one surface is, for `library.marking_surfaces`.
fn described(surface: &SurfaceRef, what: &str, geometry: Option<&SurfaceGeometry>) -> Value {
    let (kind, size) = match geometry {
        Some(SurfaceGeometry::Plane(a)) => ("plane", json!(a.size_mm)),
        Some(SurfaceGeometry::Cylinder(c)) => (
            "cylinder",
            json!({"radius_mm": c.radius, "length_mm": c.length,
                   "circumference_mm": std::f64::consts::TAU * c.radius,
                   "u_along_axis": c.along_u}),
        ),
        None => ("plane", Value::Null),
    };
    json!({"on": surface, "description": what, "map": kind, "size": size})
}

/// The surfaces of `package`, model area and box faces first.
pub fn surfaces(package: &PackageDefinition) -> Vec<Value> {
    let envelope = package.form.envelope().unwrap_or(package.envelope());
    let mut out = vec![described(
        &SurfaceRef::Model,
        "the model's marking area: the top face of its outermost body",
        None,
    )];
    for face in BoxFace::ALL {
        let area = face.of_box(envelope);
        out.push(described(
            &SurfaceRef::Face { face },
            "a box face of the outermost body (size from the envelope; the built mesh decides)",
            Some(&SurfaceGeometry::Plane(area)),
        ));
    }
    for anchor in surface::package_anchors(package) {
        out.push(described(
            &SurfaceRef::Anchor {
                name: anchor.name.clone(),
            },
            &anchor.description,
            Some(&anchor.geometry),
        ));
    }
    out
}

impl Session {
    pub(super) fn set_marking(&mut self, params: SetMarkingParams) -> Result<Value> {
        let draft = self.engine.drafts().get(params.draft).ok_or_else(|| {
            invalid(format!(
                "marking.draft: draft {} does not exist",
                params.draft
            ))
        })?;
        let Definition::Component(component) = &draft.definition else {
            return Err(invalid(format!(
                "marking.draft: ‘{}’ is not a component draft",
                draft.meta.name
            )));
        };
        let mut component = component.clone();
        let mut artwork = Vec::new();
        component.marking = match params.marking {
            None => None,
            Some(input) => {
                if input.decals.len() > MAX_DECALS {
                    return Err(invalid(format!(
                        "marking.decals: a marking has at most {MAX_DECALS} further decals"
                    )));
                }
                let mut decals = Vec::with_capacity(input.decals.len());
                for decal in input.decals {
                    decals.push(MarkingSpec {
                        items: items(decal.items, &mut artwork)?,
                        area: decal.area,
                        finish: decal.finish,
                        min_text_height_mm: decal.min_text_height_mm,
                        placement: decal.placement,
                        decals: Vec::new(),
                    });
                }
                let spec = MarkingSpec {
                    items: items(input.items, &mut artwork)?,
                    area: input.area,
                    finish: input.finish,
                    min_text_height_mm: input.min_text_height_mm,
                    placement: input.placement,
                    decals,
                };
                if let Some(finding) =
                    pcbcraft_core::library::marking::spec_findings(params.draft, &spec)
                        .into_iter()
                        .next()
                {
                    return Err(invalid(format!("marking.template: {}", finding.message)));
                }
                Some(Box::new(spec))
            }
        };
        let edits = vec![DraftEdit {
            id: params.draft,
            meta: draft.meta.clone(),
            definition: Definition::Component(component),
        }];
        let command = if artwork.is_empty() {
            LibraryCommand::Edit { edits }
        } else {
            LibraryCommand::PackArtwork { edits, artwork }
        };
        let outcome = self.engine.apply_hosted(
            params.expected_revision,
            Operation::Library(command),
            hosted(),
        )?;
        let mut reply = self.document_reply();
        reply["outcome"] = serde_json::to_value(outcome)?;
        Ok(reply)
    }

    pub(super) fn marking_surfaces(&self, params: MarkingSurfacesParams) -> Result<Value> {
        let drafted = self
            .engine
            .drafts()
            .get(params.package)
            .and_then(|d| match &d.definition {
                Definition::Package(p) => Some(p.clone()),
                _ => None,
            });
        let package = match drafted {
            Some(p) => p,
            None => self
                .engine
                .project()
                .library
                .packages
                .iter()
                .filter(|p| p.id == params.package)
                .max_by_key(|p| p.revision)
                .map(|p| p.definition.clone())
                .ok_or_else(|| {
                    invalid(format!(
                        "marking.surfaces: package {} is neither a package draft nor published",
                        params.package
                    ))
                })?,
        };
        Ok(json!({"package": params.package, "surfaces": surfaces(&package)}))
    }
}

#[cfg(test)]
#[path = "marking_tests.rs"]
mod tests;
