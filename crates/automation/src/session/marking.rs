//! `library.set_marking`: set (or clear) a component draft's generic
//! marking with SVG artwork given as text. The session packs each SVG as
//! an asset named by its SHA-256, which an agent cannot compute, and
//! applies `library.pack_artwork` (or `library.edit` without artwork): one
//! atomic undo step, refused whole on any problem. Markings stay
//! display-only (founder decision F7).
use super::{Session, hosted};
use crate::Result;
use pcbcraft_core::library::marking::{
    ARTWORK_MEDIA_TYPE, Align, Anchor, AreaChoice, Finish, MAX_ARTWORK_BYTES, MAX_MARKING_ITEMS,
    MarkingItem, MarkingSpec, PolarityMark, Template,
};
use pcbcraft_core::library::{Definition, DraftEdit, PackedAsset};
use pcbcraft_core::{DefinitionId, LibraryCommand, Operation};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

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

/// A marking whose artwork is SVG text.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MarkingInput {
    /// 1–8 items.
    pub items: Vec<ItemInput>,
    #[serde(default)]
    pub area: AreaChoice,
    #[serde(default)]
    pub finish: Finish,
    #[serde(default)]
    pub min_text_height_mm: Option<f64>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "item", rename_all = "snake_case", deny_unknown_fields)]
pub enum ItemInput {
    /// A text line from a template (`{mpn}`, `{designator}`, literal text).
    Text {
        template: String,
        #[serde(default)]
        height_mm: Option<f64>,
        #[serde(default)]
        align: Align,
    },
    /// SVG artwork as text (at most 1 MiB), with its licence (an SPDX id or
    /// the owner's terms). Text elements in the SVG are not drawn; use
    /// outlines or a `text` item.
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
}

impl Session {
    pub(super) fn set_marking(&mut self, params: SetMarkingParams) -> Result<Value> {
        let invalid = |m: String| crate::Error::Document(pcbcraft_core::Error::Invalid(m));
        let draft = self
            .engine
            .drafts()
            .get(params.draft)
            .ok_or_else(|| invalid(format!("marking.draft: draft {} does not exist", params.draft)))?;
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
                if input.items.is_empty() || input.items.len() > MAX_MARKING_ITEMS {
                    return Err(invalid(format!(
                        "marking.items: a marking has 1–{MAX_MARKING_ITEMS} items"
                    )));
                }
                let mut items = Vec::with_capacity(input.items.len());
                for item in input.items {
                    items.push(match item {
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
                            let data = pcbcraft_core::library::bundle::AssetBytes::new(
                                svg.into_bytes(),
                            );
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
                    });
                }
                Some(Box::new(MarkingSpec {
                    items,
                    area: input.area,
                    finish: input.finish,
                    min_text_height_mm: input.min_text_height_mm,
                }))
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
        let mut snapshot = self.snapshot();
        snapshot["outcome"] = serde_json::to_value(outcome)?;
        Ok(snapshot)
    }
}
