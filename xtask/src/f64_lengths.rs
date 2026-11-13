//! `cargo xtask f64-lengths`: stored lengths are integer nanometres.
//!
//! The units migration moves every stored length from f64 millimetres to
//! `units::Nm` (and rotations to `units::Angle`). This gate parses the
//! document's data modules (`model`, `library`, and `geometry`, whose
//! `Point` and `Shape` they store) and finds every serialized field whose
//! type holds `f64`, `f32` or the f64 point aliases. Each must be in
//! [`ALLOWED`], the fields a later slice converts, or in [`NOT_LENGTHS`],
//! f64 values that are not lengths. An allow-list entry whose field is gone
//! fails too, so the list only shrinks: each slice deletes its entries.
use crate::core_modules::{module_files, read};
use crate::{Result, root};
use std::collections::BTreeSet;
use syn::visit::{self, Visit};

/// The slice of the units migration that converts a field.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Slice {
    /// Schematic: occurrences, wires, labels, sheets and drawings.
    U1,
    /// Library geometry: shapes, symbols, footprints, packages, markings.
    U2,
    /// Board and copper: placements, tracks, vias, board, fills, stackup.
    U3,
}

/// Modules whose serialized fields are checked.
const MODULES: &[&str] = &["model", "library", "geometry"];

/// Type names that are f64 lengths or hold them.
const F64_TYPES: &[&str] = &["f64", "f32", "Point", "Point3"];

#[derive(Default)]
struct Fields {
    /// `Type.field` or `Enum::Variant.field` (tuple fields by index).
    found: Vec<String>,
}

fn is_test_mod(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|a| {
        a.path().is_ident("cfg") && a.parse_args::<syn::Ident>().is_ok_and(|i| i == "test")
    })
}

fn derives_serialize(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|a| {
        a.path().is_ident("derive")
            && a.parse_args_with(
                syn::punctuated::Punctuated::<syn::Path, syn::Token![,]>::parse_terminated,
            )
            .is_ok_and(|paths| {
                paths
                    .iter()
                    .any(|p| p.segments.last().is_some_and(|s| s.ident == "Serialize"))
            })
    })
}

/// `#[serde(skip)]` or `#[serde(skip_serializing)]`: not written.
fn skipped(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|a| {
        let mut skip = false;
        if a.path().is_ident("serde") {
            let _ = a.parse_nested_meta(|meta| {
                if meta.path.is_ident("skip") || meta.path.is_ident("skip_serializing") {
                    skip = true;
                }
                // Consume a value such as `default = "…"`.
                if meta.input.peek(syn::Token![=]) {
                    let _: syn::Expr = meta.value()?.parse()?;
                } else if meta.input.peek(syn::token::Paren) {
                    let _ = meta.parse_nested_meta(|_| Ok(()));
                }
                Ok(())
            });
        }
        skip
    })
}

/// The type names an f64 length, anywhere inside it.
fn holds_f64(ty: &syn::Type) -> bool {
    struct Finder(bool);
    impl<'ast> Visit<'ast> for Finder {
        fn visit_path_segment(&mut self, s: &'ast syn::PathSegment) {
            if F64_TYPES.iter().any(|t| s.ident == t) {
                self.0 = true;
            }
            visit::visit_path_segment(self, s);
        }
    }
    let mut finder = Finder(false);
    finder.visit_type(ty);
    finder.0
}

impl Fields {
    fn fields(&mut self, owner: &str, fields: &syn::Fields) {
        for (i, field) in fields.iter().enumerate() {
            if skipped(&field.attrs) || !holds_f64(&field.ty) {
                continue;
            }
            let name = field
                .ident
                .as_ref()
                .map_or_else(|| i.to_string(), ToString::to_string);
            self.found.push(format!("{owner}.{name}"));
        }
    }
}

impl<'ast> Visit<'ast> for Fields {
    fn visit_item_struct(&mut self, item: &'ast syn::ItemStruct) {
        if derives_serialize(&item.attrs) {
            self.fields(&item.ident.to_string(), &item.fields);
        }
    }

    fn visit_item_enum(&mut self, item: &'ast syn::ItemEnum) {
        if derives_serialize(&item.attrs) {
            for variant in &item.variants {
                if !skipped(&variant.attrs) {
                    self.fields(
                        &format!("{}::{}", item.ident, variant.ident),
                        &variant.fields,
                    );
                }
            }
        }
    }

    fn visit_item_mod(&mut self, item: &'ast syn::ItemMod) {
        if !is_test_mod(&item.attrs) {
            visit::visit_item_mod(self, item);
        }
    }
}

/// Every serialized f64 field, as `path/from/src.rs Type.field`.
fn scan() -> Result<BTreeSet<String>> {
    let src = root()?.join("crates/editor-core/src");
    let mut all = BTreeSet::new();
    for module in MODULES {
        for file in module_files(&src, module)? {
            let name = file.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.ends_with("tests.rs") {
                continue;
            }
            let ast = syn::parse_file(&read(&file)?)
                .map_err(|e| format!("{}: cannot parse: {e}", file.display()))?;
            let mut fields = Fields::default();
            fields.visit_file(&ast);
            let rel = file
                .strip_prefix(&src)
                .map_err(|e| format!("{}: {e}", file.display()))?
                .to_string_lossy()
                .replace('\\', "/");
            all.extend(fields.found.into_iter().map(|f| format!("{rel} {f}")));
        }
    }
    Ok(all)
}

pub fn check() -> Result<()> {
    let found = scan()?;
    let allowed: BTreeSet<&str> = ALLOWED.iter().map(|(f, _)| *f).collect();
    let not_lengths: BTreeSet<&str> = NOT_LENGTHS.iter().map(|(f, _)| *f).collect();
    let mut violations = Vec::new();
    for field in &found {
        if !allowed.contains(field.as_str()) && !not_lengths.contains(field.as_str()) {
            violations.push(format!(
                "{field}: a stored length is `units::Nm` (or `PointNm`, `Angle`), not f64; \
                 a value that is not a length goes in NOT_LENGTHS with its reason"
            ));
        }
    }
    for field in allowed.iter().chain(&not_lengths) {
        if !found.contains(*field) {
            violations.push(format!(
                "{field}: no longer a serialized f64 field; remove it from xtask/src/f64_lengths.rs"
            ));
        }
    }
    if violations.is_empty() {
        let count = |slice| ALLOWED.iter().filter(|(_, s)| *s == slice).count();
        println!(
            "f64-lengths: {} f64 length fields left (U1 {}, U2 {}, U3 {}); {} f64 fields are exempt (not lengths, or Solidcraft parameters)",
            ALLOWED.len(),
            count(Slice::U1),
            count(Slice::U2),
            count(Slice::U3),
            NOT_LENGTHS.len()
        );
        return Ok(());
    }
    Err(format!("f64 length fields:\n  {}", violations.join("\n  ")).into())
}

/// Why a Solidcraft parameter stays f64 millimetres (founder decision,
/// 10 November 2026).
const SOLIDCRAFT: &str = "Solidcraft parameter: Solidcraft computes in f64 mm; converted only at the sparksmith-solidcraft boundary";

/// Why a computed value may stay f64 millimetres.
const COMPUTED: &str =
    "a reply value, computed and never stored: replies carry millimetres (design §2.2)";

/// Why the computing shape stays f64 millimetres.
const COMPUTING_SHAPE: &str = "the computing shape (flattening, fillets, bounds, hit tests, transforms) and reply value; libraries store library::ShapeNm (design §2.1, §2.2)";

/// Serialized f64 fields that are not stored lengths, permanently, with
/// why: values that are not lengths or rotations, and Solidcraft
/// parameters.
const NOT_LENGTHS: &[(&str, &str)] = &[
    (
        "library/marking/codes.rs BandColour.tolerance",
        "a tolerance in percent, not a length",
    ),
    (
        "library/marking/codes.rs ColourCode.unbanded_tolerance",
        "a tolerance in percent, not a length",
    ),
    (
        "library/marking/codes.rs VoltageLetter.volts",
        "a rated voltage in volts, not a length",
    ),
    (
        "library/standards.rs LengthUnit.scale",
        "a unit's scale factor to millimetres, not a length",
    ),
    ("geometry.rs Bounds.max", COMPUTED),
    ("geometry.rs Bounds.min", COMPUTED),
    ("geometry.rs Shape::Arc.center", COMPUTING_SHAPE),
    ("geometry.rs Shape::Arc.radius", COMPUTING_SHAPE),
    ("geometry.rs Shape::Arc.start_deg", COMPUTING_SHAPE),
    ("geometry.rs Shape::Arc.sweep_deg", COMPUTING_SHAPE),
    ("geometry.rs Shape::Circle.center", COMPUTING_SHAPE),
    ("geometry.rs Shape::Circle.radius", COMPUTING_SHAPE),
    ("geometry.rs Shape::Line.a", COMPUTING_SHAPE),
    ("geometry.rs Shape::Line.b", COMPUTING_SHAPE),
    ("geometry.rs Shape::Polygon.points", COMPUTING_SHAPE),
    ("geometry.rs Shape::Polygon.radii", COMPUTING_SHAPE),
    ("geometry.rs Shape::Polyline.points", COMPUTING_SHAPE),
    ("geometry.rs Shape::Polyline.radii", COMPUTING_SHAPE),
    ("geometry.rs Shape::Rect.a", COMPUTING_SHAPE),
    ("geometry.rs Shape::Rect.b", COMPUTING_SHAPE),
    ("geometry.rs Shape::Rect.radius", COMPUTING_SHAPE),
    ("geometry.rs Transform2.rotation_deg", COMPUTED),
    ("geometry.rs Transform2.translate", COMPUTED),
    (
        "library/footprint.rs PadShape::RoundRect.radius_ratio",
        "a ratio",
    ),
    ("library/footprint_source.rs DimValue::Fixed.0", SOLIDCRAFT),
    (
        "library/footprint_source.rs FootprintEdit::AddPad.at",
        SOLIDCRAFT,
    ),
    (
        "library/footprint_source.rs ParamValue::Angle.0",
        SOLIDCRAFT,
    ),
    (
        "library/footprint_source.rs ParamValue::Length.0",
        SOLIDCRAFT,
    ),
    (
        "library/footprint_source.rs SketchEdit::AddArc.center",
        SOLIDCRAFT,
    ),
    (
        "library/footprint_source.rs SketchEdit::AddArc.end",
        SOLIDCRAFT,
    ),
    (
        "library/footprint_source.rs SketchEdit::AddArc.start",
        SOLIDCRAFT,
    ),
    (
        "library/footprint_source.rs SketchEdit::AddCircle.center",
        SOLIDCRAFT,
    ),
    (
        "library/footprint_source.rs SketchEdit::AddCircle.radius",
        SOLIDCRAFT,
    ),
    (
        "library/footprint_source.rs SketchEdit::AddLine.a",
        SOLIDCRAFT,
    ),
    (
        "library/footprint_source.rs SketchEdit::AddLine.b",
        SOLIDCRAFT,
    ),
    (
        "library/footprint_source.rs SketchEdit::AddPoint.at",
        SOLIDCRAFT,
    ),
    (
        "library/footprint_source.rs SketchEdit::AddPolyline.points",
        SOLIDCRAFT,
    ),
    (
        "library/footprint_source.rs SketchEdit::AddRect.a",
        SOLIDCRAFT,
    ),
    (
        "library/footprint_source.rs SketchEdit::AddRect.b",
        SOLIDCRAFT,
    ),
    (
        "library/footprint_source.rs SketchEdit::Drag.to",
        SOLIDCRAFT,
    ),
    (
        "library/footprint_source.rs SketchEdit::Move.delta",
        SOLIDCRAFT,
    ),
    ("library/graphics.rs PlacedText.at", COMPUTED),
    ("library/graphics.rs PlacedText.height", COMPUTED),
    (
        "library/graphics.rs PlacedText.line_spacing",
        "a ratio (cap heights)",
    ),
    ("library/graphics.rs PlacedText.rotation_deg", COMPUTED),
    (
        "library/graphics.rs Text.line_spacing",
        "a ratio (cap heights)",
    ),
    ("library/marking.rs LaidElement::Artwork.origin", COMPUTED),
    ("library/marking.rs LaidElement::Artwork.size_mm", COMPUTED),
    ("library/marking.rs LaidElement::Bands.pitch_mm", COMPUTED),
    ("library/marking.rs LaidElement::Matrix.module_mm", COMPUTED),
    ("library/marking.rs LaidElement::Matrix.origin", COMPUTED),
    ("library/marking.rs LaidElement::Polarity.origin", COMPUTED),
    ("library/marking.rs LaidElement::Polarity.size_mm", COMPUTED),
    ("library/marking.rs LaidText.height_mm", COMPUTED),
    ("library/marking.rs LaidText.origin", COMPUTED),
    ("library/marking/params.rs PlacementParams.scale", "a ratio"),
    (
        "library/marking/params.rs WrapParams.wrap_deg",
        "the wire form of Wrap.wrap_mdeg: a sweep of up to a full turn in degrees, which units::Degrees cannot carry (it reduces 360° to 0)",
    ),
    (
        "library/marking/surface.rs CylinderArea.axis",
        "a unit direction",
    ),
    (
        "library/marking/surface.rs CylinderArea.zero",
        "a unit direction",
    ),
    ("library/marking/surface.rs Placement.scale", "a ratio"),
    ("library/model3d.rs CustomMaterial.base_colour", "a colour"),
    (
        "library/model3d.rs CustomMaterial.clearcoat",
        "an appearance factor",
    ),
    ("library/model3d.rs CustomMaterial.emissive", "a colour"),
    (
        "library/model3d.rs CustomMaterial.ior",
        "an index of refraction",
    ),
    (
        "library/model3d.rs CustomMaterial.metallic",
        "an appearance factor",
    ),
    (
        "library/model3d.rs CustomMaterial.roughness",
        "an appearance factor",
    ),
    (
        "library/model3d.rs CustomMaterial.transmission",
        "an appearance factor",
    ),
    ("library/datum.rs DatumFrame.x", "a unit direction"),
    ("library/datum.rs DatumFrame.y", "a unit direction"),
    (
        "library/datum.rs PackageDatumGeometry::Axis.direction",
        "a unit direction",
    ),
    (
        "library/datum.rs PackageDatumGeometry::Plane.normal",
        "a unit direction",
    ),
    ("library/model3d.rs MarkingArea.normal", "a unit direction"),
    ("library/model3d.rs MarkingArea.u_axis", "a unit direction"),
    (
        "library/model3d.rs SourceFrame.unit_mm",
        "the scale of a foreign file's unit, not a document length",
    ),
    (
        "library/model3d.rs SourceFrameParams.unit_mm",
        "the scale of a foreign file's unit, not a document length",
    ),
    ("library/package_fit.rs LeadFit.rms_mm", COMPUTED),
    ("library/package_fit.rs LeadResidual.error_mm", COMPUTED),
    (
        "library/pad_table.rs PadTableReport.envelope_moved_to",
        COMPUTED,
    ),
    ("library/package_model.rs PartValue::Deg.0", SOLIDCRAFT),
    ("library/package_model.rs PartValue::Mm.0", SOLIDCRAFT),
    (
        "library/package_model.rs PartValue::Number.0",
        "a dimensionless parameter value",
    ),
    ("model/stackup.rs Dielectric.dk", "a relative permittivity"),
];

/// Serialized f64 length and rotation fields left for the units
/// migration, by the slice that converts them (to `Nm`, `PointNm`, `Angle`
/// or, for values that feed expressions, an exact decimal). Only shrinks.
const ALLOWED: &[(&str, Slice)] = &[
    // Shipped symbol drawings (library/standards/symbols), mm until U2.
    ("library/symbol_style.rs DrawOp::Arrow.from", Slice::U2),
    ("library/symbol_style.rs DrawOp::Arrow.tip", Slice::U2),
    ("library/symbol_style.rs DrawOp::ArrowHead.from", Slice::U2),
    ("library/symbol_style.rs DrawOp::ArrowHead.tip", Slice::U2),
    ("library/symbol_style.rs DrawOp::Contact.center", Slice::U2),
    ("library/symbol_style.rs DrawOp::Labels.bottom", Slice::U2),
    ("library/symbol_style.rs DrawOp::Labels.top", Slice::U2),
    ("library/symbol_style.rs DrawOp::Line.a", Slice::U2),
    ("library/symbol_style.rs DrawOp::Line.b", Slice::U2),
    ("library/symbol_style.rs DrawOp::Pin.at", Slice::U2),
    ("library/symbol_style.rs DrawOp::Pin.length", Slice::U2),
    ("library/symbol_style.rs DrawOp::Plate.a", Slice::U2),
    ("library/symbol_style.rs DrawOp::Plate.b", Slice::U2),
    ("library/symbol_style.rs DrawOp::RailLabels.y", Slice::U2),
    ("library/symbol_style.rs DrawOp::Shape.width", Slice::U2),
    ("library/symbol_style.rs DrawOp::SideLabels.at", Slice::U2),
    ("library/symbol_style.rs DrawOp::TwoPins.left", Slice::U2),
    ("library/symbol_style.rs DrawOp::TwoPins.right", Slice::U2),
    ("library/symbol_style.rs SymbolStyle.arrow_mm", Slice::U2),
    (
        "library/symbol_style.rs SymbolStyle.arrow_stroke_mm",
        Slice::U2,
    ),
    (
        "library/symbol_style.rs SymbolStyle.contact_radius_mm",
        Slice::U2,
    ),
    (
        "library/symbol_style.rs SymbolStyle.label_gap_mm",
        Slice::U2,
    ),
    (
        "library/symbol_style.rs SymbolStyle.plate_stroke_mm",
        Slice::U2,
    ),
    (
        "library/symbol_style.rs SymbolStyle.rail_designator_mm",
        Slice::U2,
    ),
    (
        "library/symbol_style.rs SymbolStyle.side_label_gap_mm",
        Slice::U2,
    ),
    ("library/footprint.rs Outline.start", Slice::U3),
    ("library/footprint.rs Segment::Arc.bulge", Slice::U3),
    ("library/footprint.rs Segment::Arc.to", Slice::U3),
    ("library/footprint.rs Segment::Line.to", Slice::U3),
    ("model.rs Placement.position", Slice::U3),
    ("model.rs Placement.rotation_deg", Slice::U3),
    ("model.rs Project.board_size", Slice::U3),
    ("model.rs TextPlacement.offset", Slice::U3),
    ("model.rs Track.end", Slice::U3),
    ("model.rs Track.start", Slice::U3),
    ("model.rs Track.width", Slice::U3),
    ("model/board.rs BoardSketch.outline_fit", Slice::U3),
    ("model/board.rs ShapeFault.at", Slice::U3),
    ("model/board.rs SolvedEdge.a", Slice::U3),
    ("model/board.rs SolvedEdge.b", Slice::U3),
    ("model/board.rs SolvedEdge.bulge", Slice::U3),
    ("model/board.rs SolvedMountingHole.at", Slice::U3),
    ("model/board.rs SolvedMountingHole.drill", Slice::U3),
    ("model/board.rs SolvedMountingHole.pad_diameter", Slice::U3),
    ("model/dimension.rs FeatureGeometry::Arc.a", Slice::U3),
    ("model/dimension.rs FeatureGeometry::Arc.b", Slice::U3),
    ("model/dimension.rs FeatureGeometry::Arc.bulge", Slice::U3),
    (
        "model/dimension.rs FeatureGeometry::Circle.center",
        Slice::U3,
    ),
    (
        "model/dimension.rs FeatureGeometry::Circle.radius",
        Slice::U3,
    ),
    ("model/dimension.rs FeatureGeometry::Line.a", Slice::U3),
    ("model/dimension.rs FeatureGeometry::Line.b", Slice::U3),
    ("model/dimension.rs FeatureGeometry::Point.at", Slice::U3),
    ("model/dimension.rs ModelEdgeRef.pick", Slice::U3),
    ("model/dimension.rs ModelFaceRef.pick", Slice::U3),
    ("model/dimension.rs PartRelation::PointAlong.at", Slice::U3),
    ("model/fill.rs FillSettings.clearance", Slice::U3),
    ("model/fill.rs FillSettings.width_min", Slice::U3),
    ("model/fill.rs FilledRegion.holes", Slice::U3),
    ("model/fill.rs FilledRegion.outer", Slice::U3),
    ("model/fill.rs Hatch.angle_deg", Slice::U3),
    ("model/fill.rs Hatch.gap", Slice::U3),
    ("model/fill.rs Hatch.width", Slice::U3),
    ("model/fill.rs IslandRemoval::BelowArea.0", Slice::U3),
    ("model/stackup.rs CopperFoil.thickness_um", Slice::U3),
    ("model/stackup.rs Dielectric.thickness_mm", Slice::U3),
    ("model/stackup.rs Stackup.mask_um", Slice::U3),
    ("model/via.rs Via.diameter", Slice::U3),
    ("model/via.rs Via.drill", Slice::U3),
    ("model/via.rs Via.position", Slice::U3),
];

#[cfg(test)]
mod tests {
    use super::*;

    fn found(source: &str) -> Result<Vec<String>> {
        let mut fields = Fields::default();
        fields.visit_file(&syn::parse_file(source)?);
        Ok(fields.found)
    }

    #[test]
    fn serialized_f64_fields_are_found_through_wrappers_and_aliases() -> Result<()> {
        let source = r#"
            #[derive(Clone, serde::Serialize)]
            struct A { width: f64, at: Point, ring: Vec<[f64; 2]>, maybe: Option<f32>, n: u32, x_nm: Nm }
            #[derive(Serialize, Deserialize)]
            enum E { Arc { radius: f64, turns: u8 }, Mm(f64), Count(u32) }
            #[derive(Deserialize)]
            struct ReadOnly { width: f64 }
            struct Plain { width: f64 }
        "#;
        assert_eq!(
            found(source)?,
            [
                "A.width",
                "A.at",
                "A.ring",
                "A.maybe",
                "E::Arc.radius",
                "E::Mm.0"
            ]
        );
        Ok(())
    }

    #[test]
    fn skipped_fields_and_test_modules_are_not_stored() -> Result<()> {
        let source = r#"
            #[derive(Serialize)]
            struct A {
                #[serde(skip)]
                cache: f64,
                #[serde(default, skip_serializing_if = "Option::is_none")]
                kept: Option<f64>,
                #[serde(rename = "w", skip_serializing)]
                hidden: f64,
            }
            #[cfg(test)]
            mod tests { #[derive(Serialize)] struct T { x: f64 } }
        "#;
        assert_eq!(found(source)?, ["A.kept"]);
        Ok(())
    }
}
