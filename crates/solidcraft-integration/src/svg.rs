//! SVG artwork → editable library graphics (logos and marks for symbols and
//! the silkscreen or assembly drawing of a footprint).
//!
//! Geometry comes from Solidcraft's SVG reader (`solidcraft_engine::io::
//! read_svg`, MIT OR Apache-2.0): paths with every command, lines, circles,
//! ellipses, polylines and polygons, with transforms, user units scaled to
//! millimetres from the root `width`/`height` and `viewBox` and y flipped.
//! That reader returns curves without their element, so this module walks
//! the XML itself for what Pcbcraft keeps per element (fill, stroke and
//! stroke width, inherited through groups and `style`) and hands the reader
//! one element at a time, inside its ancestors' transforms.
//!
//! What becomes what:
//! - circles stay circles, circular arcs stay arcs, and an axis-aligned
//!   rectangle stays a rectangle with its corner radius;
//! - straight runs become polylines or polygons; a circular arc tangent to
//!   the straight edges on both sides becomes that corner's fillet radius;
//! - Béziers and ellipses are flattened within the chord tolerance;
//! - a filled closed outline becomes a solid-filled polygon (or circle or
//!   rectangle); its stroke width is kept as the outline width.
//!
//! Filled regions with holes (one element whose loops nest) are imported as
//! outlines only: Pcbcraft graphics have no polygon with holes yet (FD15).
//! Text, images and `use` references are skipped and reported.
//!
//! The input is bounded: [`MAX_SVG_BYTES`], [`MAX_SVG_ELEMENTS`] and
//! [`MAX_SVG_SEGMENTS`], then the library's graphic limits (at most
//! [`MAX_GRAPHIC_EDITS`] graphics, [`MAX_VERTICES`] points and
//! [`MAX_SHAPE_POINTS`] points in one filled region). Anything beyond is a
//! refusal that names the limit, never a partial import.
use crate::{Error, Result};
use pcbcraft_core::geometry::{
    Bounds, Point, Shape, corner_radii_fit, fit_corner_radii, polygon_contains,
    polygon_self_intersects, signed_area2,
};
use pcbcraft_core::library::graphics::{
    DrawingLayer, Fill, GraphicEdit, GraphicTarget, Item, MAX_GRAPHIC_EDITS, MAX_SHAPE_POINTS,
    MAX_STROKE_MM, MAX_VERTICES,
};
use quick_xml::events::{BytesStart, Event};
use solidcraft_engine::io::Geom2;
use std::collections::BTreeMap;

/// Largest SVG file imported, in bytes.
pub const MAX_SVG_BYTES: usize = 1 << 20;
/// Drawn elements (`path`, `rect`, `circle`, …) one import reads.
pub const MAX_SVG_ELEMENTS: usize = 2048;
/// Curve segments the elements may produce together.
pub const MAX_SVG_SEGMENTS: usize = 32_768;
/// Default chord tolerance for Béziers and ellipses, in millimetres.
pub const DEFAULT_TOLERANCE_MM: f64 = 0.01;
/// Accepted chord tolerances, in millimetres.
pub const TOLERANCE_MM: std::ops::RangeInclusive<f64> = 0.001..=1.0;
/// Accepted user scale factors.
pub const SCALE: std::ops::RangeInclusive<f64> = 0.001..=1000.0;
const MAX_DEPTH: usize = 256;
/// Points closer than this join one outline, in millimetres.
const JOIN_MM: f64 = 1e-6;
/// Segments one curve flattens to at most.
const MAX_CURVE_SEGMENTS: usize = 256;

/// How an SVG is converted.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Options {
    /// Multiplies the SVG's own millimetre size.
    pub scale: f64,
    /// Chord tolerance for flattened curves, in millimetres (after scaling).
    pub tolerance_mm: f64,
    /// Outline width for shapes whose fill cannot be kept (regions with
    /// holes), in millimetres.
    pub fallback_stroke_mm: f64,
}

impl Options {
    /// Scale 1, the default tolerance and `fallback_stroke_mm`.
    pub fn new(fallback_stroke_mm: f64) -> Self {
        Self {
            scale: 1.0,
            tolerance_mm: DEFAULT_TOLERANCE_MM,
            fallback_stroke_mm,
        }
    }
}

/// Converted artwork, centred on the origin.
#[derive(Clone, Debug, PartialEq)]
pub struct Artwork {
    /// Graphics in drawing order, millimetres, the bounding-box centre at
    /// the origin.
    pub items: Vec<Item>,
    /// Width and height of the bounding box, in millimetres.
    pub size: Point,
    /// What was skipped or simplified, one sentence each.
    pub notes: Vec<String>,
}

impl Artwork {
    /// `library.edit_graphics` edits adding the artwork centred at `at`.
    pub fn edits(&self, target: GraphicTarget, at: Point) -> Result<Vec<GraphicEdit>> {
        if let GraphicTarget::Footprint {
            layer: DrawingLayer::Courtyard,
            ..
        } = target
        {
            return Err(invalid(
                "SVG artwork goes on the silkscreen or assembly drawing, not the courtyard",
            ));
        }
        self.items
            .iter()
            .map(|item| {
                let item = item.translated(at);
                match &item {
                    Item::Shape { shape, .. } if !shape.finite() => Err(invalid(
                        "the placed artwork reaches beyond ±10000 mm; place it nearer the origin or scale it down",
                    )),
                    _ => Ok(GraphicEdit::Add { target, item }),
                }
            })
            .collect()
    }
}

fn invalid(message: impl Into<String>) -> Error {
    Error::Invalid(message.into())
}

/// Convert SVG text into graphics. Fails without a result when the input
/// is malformed, empty or beyond the limits.
pub fn convert(text: &str, options: &Options) -> Result<Artwork> {
    if text.len() > MAX_SVG_BYTES {
        return Err(invalid(format!(
            "the SVG is {} bytes; at most {} KiB is imported",
            text.len(),
            MAX_SVG_BYTES >> 10
        )));
    }
    if !(options.scale.is_finite() && SCALE.contains(&options.scale)) {
        return Err(invalid("the import scale must be between 0.001 and 1000"));
    }
    if !(options.tolerance_mm.is_finite() && TOLERANCE_MM.contains(&options.tolerance_mm)) {
        return Err(invalid(
            "the curve tolerance must be between 0.001 and 1 mm",
        ));
    }
    let mut walk = Walk::new(options);
    walk.run(text)?;
    walk.finish()
}

/// Presentation that inherits through groups.
#[derive(Clone, Copy, Debug)]
struct Style {
    fill: bool,
    stroke: bool,
    /// In user units.
    stroke_width: f64,
}

impl Default for Style {
    fn default() -> Self {
        // SVG's initial values: black fill, no stroke, width 1.
        Self {
            fill: true,
            stroke: false,
            stroke_width: 1.0,
        }
    }
}

#[derive(Clone, Debug, Default)]
struct Frame {
    /// The `transform` attributes from the root to here, outermost first.
    transforms: Vec<String>,
    style: Style,
}

/// One element's local frame in millimetres (before the user scale):
/// `origin + x·ux + y·uy`.
#[derive(Clone, Copy, Debug)]
struct Affine {
    origin: Point,
    ux: Point,
    uy: Point,
}

impl Affine {
    fn apply(&self, x: f64, y: f64) -> Point {
        [
            self.origin[0] + x * self.ux[0] + y * self.uy[0],
            self.origin[1] + x * self.ux[1] + y * self.uy[1],
        ]
    }
    /// The factor a length (stroke width) scales by.
    fn length_scale(&self) -> f64 {
        cross(self.ux, self.uy).abs().sqrt()
    }
    fn axis_aligned(&self) -> bool {
        let lx = norm(self.ux);
        let ly = norm(self.uy);
        lx > 0.0
            && ly > 0.0
            && ((self.ux[1].abs() <= 1e-9 * lx && self.uy[0].abs() <= 1e-9 * ly)
                || (self.ux[0].abs() <= 1e-9 * lx && self.uy[1].abs() <= 1e-9 * ly))
    }
    fn uniform(&self) -> bool {
        let (lx, ly) = (norm(self.ux), norm(self.uy));
        (lx - ly).abs() <= 1e-9 * lx.max(ly) && dot(self.ux, self.uy).abs() <= 1e-9 * lx * ly
    }
}

struct Walk<'a> {
    options: &'a Options,
    root: Option<String>,
    stack: Vec<Frame>,
    /// Depth inside a non-rendered subtree (`defs`, `clipPath`, hidden…).
    skip: usize,
    affines: BTreeMap<String, Option<Affine>>,
    elements: usize,
    segments: usize,
    items: Vec<Item>,
    skipped: BTreeMap<&'static str, usize>,
    outlined_holes: usize,
    flattened: usize,
}

const NOT_RENDERED: [&str; 14] = [
    "defs",
    "clipPath",
    "mask",
    "pattern",
    "marker",
    "symbol",
    "metadata",
    "title",
    "desc",
    "style",
    "script",
    "foreignObject",
    "linearGradient",
    "radialGradient",
];

impl<'a> Walk<'a> {
    fn new(options: &'a Options) -> Self {
        Self {
            options,
            root: None,
            stack: Vec::new(),
            skip: 0,
            affines: BTreeMap::new(),
            elements: 0,
            segments: 0,
            items: Vec::new(),
            skipped: BTreeMap::new(),
            outlined_holes: 0,
            flattened: 0,
        }
    }

    fn run(&mut self, text: &str) -> Result<()> {
        let mut reader = quick_xml::Reader::from_str(text);
        reader.config_mut().trim_text(true);
        loop {
            let event = reader
                .read_event()
                .map_err(|e| invalid(format!("the SVG is not well-formed XML: {e}")))?;
            match event {
                Event::Start(e) => self.open(&e, false)?,
                Event::Empty(e) => self.open(&e, true)?,
                Event::End(_) => {
                    if self.skip > 0 {
                        self.skip -= 1;
                    } else {
                        self.stack.pop();
                    }
                }
                Event::Eof => break,
                _ => {}
            }
        }
        if self.root.is_none() {
            return Err(invalid("the file has no <svg> root element"));
        }
        Ok(())
    }

    fn open(&mut self, e: &BytesStart, empty: bool) -> Result<()> {
        let name = String::from_utf8_lossy(e.local_name().as_ref()).to_string();
        if self.skip > 0 {
            if !empty {
                self.skip += 1;
            }
            return Ok(());
        }
        let attrs = attributes(e);
        if self.root.is_none() {
            if name != "svg" {
                return Err(invalid("the file's root element is not <svg>"));
            }
            let mut root = String::new();
            for key in ["width", "height", "viewBox"] {
                if let Some(value) = attr(&attrs, key) {
                    root.push_str(&format!(" {key}=\"{}\"", quick_xml::escape::escape(value)));
                }
            }
            self.root = Some(root);
        }
        let parent = self.stack.last().cloned().unwrap_or_default();
        let hidden = matches!(declared(&attrs, "display").as_deref(), Some("none"))
            || matches!(
                declared(&attrs, "visibility").as_deref(),
                Some("hidden" | "collapse")
            );
        if hidden || NOT_RENDERED.contains(&name.as_str()) {
            if !empty {
                self.skip = 1;
            }
            return Ok(());
        }
        let mut frame = parent;
        if let Some(t) = attr(&attrs, "transform") {
            frame.transforms.push(t.to_string());
        }
        frame.style = style(&attrs, frame.style);
        match name.as_str() {
            "path" | "line" | "rect" | "circle" | "ellipse" | "polyline" | "polygon" => {
                self.elements += 1;
                if self.elements > MAX_SVG_ELEMENTS {
                    return Err(invalid(format!(
                        "the SVG has more than {MAX_SVG_ELEMENTS} drawn elements"
                    )));
                }
                self.element(&name, &attrs, &frame)?;
            }
            "text" | "image" | "use" => {
                *self.skipped.entry(skipped_label(&name)).or_default() += 1;
                if !empty {
                    self.skip = 1;
                }
                return Ok(());
            }
            _ => {}
        }
        if !empty {
            self.stack.push(frame);
            if self.stack.len() > MAX_DEPTH {
                return Err(invalid("the SVG is nested too deeply"));
            }
        }
        Ok(())
    }

    /// A one-element document for Solidcraft's reader: the root's size,
    /// the ancestors' transforms, then `body`.
    fn document(&self, transforms: &[String], body: &str) -> String {
        let mut doc = format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\"{}>",
            self.root.as_deref().unwrap_or("")
        );
        for t in transforms {
            doc.push_str(&format!(
                "<g transform=\"{}\">",
                quick_xml::escape::escape(t.as_str())
            ));
        }
        doc.push_str(body);
        for _ in transforms {
            doc.push_str("</g>");
        }
        doc.push_str("</svg>");
        doc
    }

    fn read(&mut self, transforms: &[String], body: &str) -> Result<Vec<Geom2>> {
        let geometry = solidcraft_engine::io::read_svg(&self.document(transforms, body))
            .map_err(|e| invalid(format!("SVG: {e}")))?;
        self.segments = self.segments.saturating_add(geometry.len());
        if self.segments > MAX_SVG_SEGMENTS {
            return Err(invalid(format!(
                "the SVG has more than {MAX_SVG_SEGMENTS} curve segments"
            )));
        }
        Ok(geometry)
    }

    /// The element frame's map to millimetres, probed through the reader so
    /// it matches the curves exactly; `None` when it collapses a dimension.
    fn affine(&mut self, transforms: &[String]) -> Result<Option<Affine>> {
        let key = transforms.join("\u{1}");
        if let Some(found) = self.affines.get(&key) {
            return Ok(*found);
        }
        let x = self.read(transforms, "<path d=\"M0 0 L1 0\"/>")?;
        let y = self.read(transforms, "<path d=\"M0 0 L0 1\"/>")?;
        let found = match (x.first(), y.first()) {
            (Some(Geom2::Line(o, x)), Some(Geom2::Line(_, y))) => {
                let origin = [o.x, o.y];
                let affine = Affine {
                    origin,
                    ux: sub([x.x, x.y], origin),
                    uy: sub([y.x, y.y], origin),
                };
                (affine.length_scale() > 1e-12).then_some(affine)
            }
            _ => None,
        };
        self.affines.insert(key, found);
        Ok(found)
    }

    fn element(&mut self, name: &str, attrs: &[(String, String)], frame: &Frame) -> Result<()> {
        let Some(affine) = self.affine(&frame.transforms)? else {
            *self
                .skipped
                .entry("collapsed by its transform")
                .or_default() += 1;
            return Ok(());
        };
        let style = frame.style;
        let scale = self.options.scale;
        let width = if style.stroke && style.stroke_width > 0.0 {
            let mm = style.stroke_width * affine.length_scale() * scale;
            if !(mm.is_finite() && mm <= MAX_STROKE_MM) {
                return Err(invalid(format!(
                    "a stroke is {mm:.3} mm wide at this scale; graphics allow at most {MAX_STROKE_MM} mm, so scale the artwork down"
                )));
            }
            Some(mm)
        } else {
            None
        };
        if width.is_none() && !style.fill {
            *self.skipped.entry("without fill or stroke").or_default() += 1;
            return Ok(());
        }
        let body = match name {
            "rect" => {
                let Some(rect) = Rect::of(attrs) else {
                    return Ok(());
                };
                if let Some(shape) = rect.exact(&affine, scale) {
                    let fill = if style.fill { Fill::Solid } else { Fill::None };
                    self.items.push(Item::Shape {
                        shape,
                        width: width.unwrap_or(0.0),
                        fill,
                    });
                    return Ok(());
                }
                format!("<path d=\"{}\"/>", rect.path())
            }
            _ => {
                let mut body = format!("<{name}");
                for (key, value) in attrs {
                    if GEOMETRY_ATTRIBUTES.contains(&key.as_str()) {
                        body.push_str(&format!(
                            " {key}=\"{}\"",
                            quick_xml::escape::escape(value.as_str())
                        ));
                    }
                }
                body.push_str("/>");
                body
            }
        };
        let geometry = self.read(&frame.transforms, &body)?;
        let segments = segments(&geometry, scale, self.options.tolerance_mm);
        let chains = chains(segments);
        self.emit(chains, style.fill && name != "line", width)
    }

    fn emit(&mut self, chains: Vec<Chain>, fill: bool, width: Option<f64>) -> Result<()> {
        let tol = self.options.tolerance_mm;
        if !fill {
            for chain in &chains {
                self.outline(chain, width.unwrap_or(self.options.fallback_stroke_mm));
            }
            return Ok(());
        }
        // A filled element: closed loops (open ones close, as SVG fills
        // them); open runs of a single segment have no area.
        let loops: Vec<&Chain> = chains.iter().filter(|c| c.can_fill()).collect();
        let rings: Vec<Vec<Point>> = loops.iter().map(|c| c.ring(tol)).collect();
        let nested = rings.iter().enumerate().any(|(i, outer)| {
            rings.iter().enumerate().any(|(j, inner)| {
                i != j && inner.first().is_some_and(|p| polygon_contains(outer, *p))
            })
        });
        if nested {
            self.outlined_holes += 1;
            let stroke = width.unwrap_or(self.options.fallback_stroke_mm);
            for chain in &chains {
                self.outline(chain, stroke);
            }
            return Ok(());
        }
        for chain in &chains {
            let shape = if chain.can_fill() {
                self.filled(chain)?
            } else {
                None
            };
            if let Some(shape) = shape {
                self.items.push(Item::Shape {
                    shape,
                    width: width.unwrap_or(0.0),
                    fill: Fill::Solid,
                });
            } else if let Some(width) = width {
                self.outline(chain, width);
            }
        }
        Ok(())
    }

    /// The single filled shape of a closed chain; `None` when it encloses
    /// no area or crosses itself, so it can only be outlined.
    fn filled(&mut self, chain: &Chain) -> Result<Option<Shape>> {
        if let Some((center, radius)) = as_circle(chain) {
            return Ok(Some(Shape::Circle { center, radius }));
        }
        if chain.closed
            && let Some((points, radii)) = fillets(&chain.segs, true)
            && points.len() >= 3
            && points.len() <= MAX_SHAPE_POINTS
            && signed_area2(&points).abs() > 1e-12
            && !polygon_self_intersects(&points)
        {
            return Ok(Some(Shape::Polygon { points, radii }));
        }
        let points = chain.ring(self.options.tolerance_mm);
        if points.len() < 3
            || signed_area2(&points).abs() <= 1e-12
            || polygon_self_intersects(&points)
        {
            return Ok(None);
        }
        if points.len() > MAX_SHAPE_POINTS {
            return Err(invalid(format!(
                "a filled region has {} points; one region holds at most {MAX_SHAPE_POINTS} (raise the curve tolerance or simplify the artwork)",
                points.len()
            )));
        }
        if chain.segs.iter().any(|s| !matches!(s, Seg::Line(..))) {
            self.flattened += 1;
        }
        Ok(Some(Shape::Polygon {
            points,
            radii: vec![],
        }))
    }

    /// Stroked pieces of one chain: one polyline or polygon with fillet
    /// radii when every arc rounds a corner between straight edges, else
    /// straight runs and exact arcs.
    fn outline(&mut self, chain: &Chain, width: f64) {
        let tol = self.options.tolerance_mm;
        let (pieces, flattened) = outline_shapes(chain, tol);
        if flattened {
            self.flattened += 1;
        }
        self.items
            .extend(pieces.into_iter().map(|shape| Item::Shape {
                shape,
                width,
                fill: Fill::None,
            }));
    }

    fn finish(self) -> Result<Artwork> {
        if self.items.is_empty() {
            return Err(invalid("the SVG has no drawable shapes"));
        }
        if self.items.len() > MAX_GRAPHIC_EDITS {
            return Err(invalid(format!(
                "the SVG makes {} graphics; at most {MAX_GRAPHIC_EDITS} import at once (join paths or simplify the artwork)",
                self.items.len()
            )));
        }
        let vertices: usize = self.items.iter().map(Item::vertices).sum();
        if vertices > MAX_VERTICES {
            return Err(invalid(format!(
                "the SVG makes {vertices} points; a drawing holds at most {MAX_VERTICES} (raise the curve tolerance or simplify the artwork)"
            )));
        }
        let mut bounds: Option<Bounds> = None;
        for item in &self.items {
            if let Item::Shape { shape, width, .. } = item {
                if !shape.finite() {
                    return Err(invalid(
                        "the artwork is larger than ±10000 mm at this scale",
                    ));
                }
                let b = shape.bounds(*width);
                bounds = Some(bounds.map_or(b, |a| a.union(b)));
            }
        }
        let bounds = bounds.ok_or_else(|| invalid("the SVG has no drawable shapes"))?;
        let centre = bounds.center();
        let items = self
            .items
            .iter()
            .map(|item| item.translated([-centre[0], -centre[1]]))
            .collect();
        let mut notes = Vec::new();
        for (what, n) in &self.skipped {
            notes.push(format!("{n} element(s) skipped: {what}."));
        }
        if self.outlined_holes > 0 {
            notes.push(format!(
                "{} filled region(s) with holes imported as outlines; filled regions with holes are not supported yet.",
                self.outlined_holes
            ));
        }
        if self.flattened > 0 {
            notes.push(format!(
                "{} curved outline(s) flattened within {} mm.",
                self.flattened, self.options.tolerance_mm
            ));
        }
        Ok(Artwork {
            items,
            size: bounds.size(),
            notes,
        })
    }
}

/// A circle, or a closed chain of arcs on one circle (as paths draw them).
fn as_circle(chain: &Chain) -> Option<(Point, f64)> {
    if let [Seg::Circle { center, radius }] = chain.segs.as_slice() {
        return Some((*center, *radius));
    }
    if !chain.closed {
        return None;
    }
    let mut arcs = chain.segs.iter().map(|s| match s {
        Seg::Arc(arc) => Some(arc),
        _ => None,
    });
    let first = arcs.next()??;
    let near = 1e-6 * first.radius.max(1.0);
    let mut sweep = first.sweep_deg;
    for arc in arcs {
        let arc = arc?;
        if distance(arc.center, first.center) > near || (arc.radius - first.radius).abs() > near {
            return None;
        }
        sweep += arc.sweep_deg;
    }
    ((sweep.abs() - 360.0).abs() <= 1e-6).then_some((first.center, first.radius))
}

/// The stroked shapes of one chain, and whether a curve was flattened.
fn outline_shapes(chain: &Chain, tol: f64) -> (Vec<Shape>, bool) {
    if let Some((center, radius)) = as_circle(chain) {
        return (vec![Shape::Circle { center, radius }], false);
    }
    if let [Seg::Arc(arc)] = chain.segs.as_slice() {
        return (vec![arc.shape()], false);
    }
    if let Some((points, radii)) = fillets(&chain.segs, chain.closed) {
        if !chain.closed {
            return (polylines(points, radii), false);
        }
        if points.len() >= 3
            && points.len() <= MAX_SHAPE_POINTS
            && signed_area2(&points).abs() > 1e-12
            && !polygon_self_intersects(&points)
        {
            return (vec![Shape::Polygon { points, radii }], false);
        }
    }
    let flattened = chain.segs.iter().any(|s| matches!(s, Seg::Curve(_)));
    if chain.closed
        && !chain
            .segs
            .iter()
            .any(|s| matches!(s, Seg::Arc(_) | Seg::Circle { .. }))
    {
        let ring = chain.ring(tol);
        if ring.len() >= 3
            && ring.len() <= MAX_SHAPE_POINTS
            && signed_area2(&ring).abs() > 1e-12
            && !polygon_self_intersects(&ring)
        {
            return (
                vec![Shape::Polygon {
                    points: ring,
                    radii: vec![],
                }],
                flattened,
            );
        }
    }
    // Straight runs (curves flattened) and exact arcs, in path order.
    let mut run: Vec<Point> = Vec::new();
    let mut pieces = Vec::new();
    for seg in &chain.segs {
        match seg {
            Seg::Arc(arc) => {
                pieces.extend(polylines(std::mem::take(&mut run), vec![]));
                pieces.push(arc.shape());
            }
            Seg::Circle { center, radius } => {
                pieces.extend(polylines(std::mem::take(&mut run), vec![]));
                pieces.push(Shape::Circle {
                    center: *center,
                    radius: *radius,
                });
            }
            other => {
                for p in other.points(tol) {
                    if run.last().is_none_or(|q| distance(*q, p) > JOIN_MM) {
                        run.push(p);
                    }
                }
            }
        }
    }
    pieces.extend(polylines(run, vec![]));
    (pieces, flattened)
}

fn skipped_label(name: &str) -> &'static str {
    match name {
        "text" => "text (convert text to paths)",
        "image" => "embedded images",
        _ => "use references (expand them first)",
    }
}

/// The attributes that carry an element's geometry (style and transform
/// are applied by the walk).
const GEOMETRY_ATTRIBUTES: [&str; 12] = [
    "d",
    "x1",
    "y1",
    "x2",
    "y2",
    "cx",
    "cy",
    "r",
    "rx",
    "ry",
    "points",
    "pathLength",
];

fn attributes(e: &BytesStart) -> Vec<(String, String)> {
    e.attributes()
        .flatten()
        .map(|a| {
            let key = String::from_utf8_lossy(a.key.local_name().as_ref()).to_string();
            let value = a
                .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                .map(|v| v.to_string())
                .unwrap_or_else(|_| String::from_utf8_lossy(&a.value).to_string());
            (key, value)
        })
        .collect()
}

fn attr<'a>(attrs: &'a [(String, String)], key: &str) -> Option<&'a str> {
    attrs
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

/// A presentation property: the `style` declaration over the attribute.
fn declared(attrs: &[(String, String)], key: &str) -> Option<String> {
    let from_style = attr(attrs, "style").and_then(|style| {
        style.split(';').rev().find_map(|decl| {
            let (k, v) = decl.split_once(':')?;
            (k.trim() == key).then(|| v.trim().to_string())
        })
    });
    from_style.or_else(|| attr(attrs, key).map(|v| v.trim().to_string()))
}

fn paints(value: &str) -> bool {
    !matches!(value, "none" | "transparent")
}

fn style(attrs: &[(String, String)], inherited: Style) -> Style {
    let mut style = inherited;
    if let Some(fill) = declared(attrs, "fill")
        && fill != "inherit"
    {
        style.fill = paints(&fill);
    }
    if let Some(stroke) = declared(attrs, "stroke")
        && stroke != "inherit"
    {
        style.stroke = paints(&stroke);
    }
    if let Some(width) = declared(attrs, "stroke-width").and_then(|w| number(&w)) {
        style.stroke_width = width.max(0.0);
    }
    style
}

/// A user-unit number (`px` allowed).
fn number(s: &str) -> Option<f64> {
    let s = s.trim();
    let s = s.strip_suffix("px").unwrap_or(s).trim();
    s.parse::<f64>().ok().filter(|v| v.is_finite())
}

/// An SVG `rect` with its corner radii resolved (SVG 1.1 §9.2).
struct Rect {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    rx: f64,
    ry: f64,
}

impl Rect {
    fn of(attrs: &[(String, String)]) -> Option<Self> {
        let get = |k: &str| attr(attrs, k).and_then(number);
        let (w, h) = (get("width")?, get("height")?);
        if !(w > 0.0 && h > 0.0) {
            return None;
        }
        let (rx, ry) = match (
            get("rx").filter(|v| *v >= 0.0),
            get("ry").filter(|v| *v >= 0.0),
        ) {
            (Some(rx), Some(ry)) => (rx, ry),
            (Some(r), None) | (None, Some(r)) => (r, r),
            (None, None) => (0.0, 0.0),
        };
        Some(Self {
            x: get("x").unwrap_or(0.0),
            y: get("y").unwrap_or(0.0),
            w,
            h,
            rx: rx.min(w / 2.0),
            ry: ry.min(h / 2.0),
        })
    }

    /// The exact rectangle, when the frame keeps it axis-aligned and any
    /// rounding circular.
    fn exact(&self, affine: &Affine, scale: f64) -> Option<Shape> {
        if !affine.axis_aligned() {
            return None;
        }
        let rounded = self.rx > 0.0 || self.ry > 0.0;
        if rounded && !(affine.uniform() && (self.rx - self.ry).abs() <= 1e-9 * self.rx.max(1.0)) {
            return None;
        }
        let a = affine.apply(self.x, self.y);
        let b = affine.apply(self.x + self.w, self.y + self.h);
        Some(Shape::Rect {
            a: [a[0] * scale, a[1] * scale],
            b: [b[0] * scale, b[1] * scale],
            radius: self.rx * affine.length_scale() * scale,
        })
    }

    /// The same outline as path data (for frames that rotate or shear it).
    fn path(&self) -> String {
        let Self { x, y, w, h, rx, ry } = *self;
        if rx <= 0.0 || ry <= 0.0 {
            return format!("M{x} {y} h{w} v{h} h{} Z", -w);
        }
        format!(
            "M{} {y} H{} A{rx} {ry} 0 0 1 {} {} V{} A{rx} {ry} 0 0 1 {} {} H{} A{rx} {ry} 0 0 1 {x} {} V{} A{rx} {ry} 0 0 1 {} {y} Z",
            x + rx,
            x + w - rx,
            x + w,
            y + ry,
            y + h - ry,
            x + w - rx,
            y + h,
            x + rx,
            y + h - ry,
            y + ry,
            x + rx,
        )
    }
}

/// A circular arc in path direction.
#[derive(Clone, Copy, Debug)]
struct Arc {
    center: Point,
    radius: f64,
    start_deg: f64,
    /// Signed: positive counter-clockwise.
    sweep_deg: f64,
    from: Point,
    to: Point,
}

impl Arc {
    fn reversed(self) -> Self {
        Self {
            start_deg: self.start_deg + self.sweep_deg,
            sweep_deg: -self.sweep_deg,
            from: self.to,
            to: self.from,
            ..self
        }
    }
    fn shape(&self) -> Shape {
        Shape::Arc {
            center: self.center,
            radius: self.radius,
            start_deg: self.start_deg.rem_euclid(360.0),
            sweep_deg: self.sweep_deg,
        }
    }
    /// Unit tangent in path direction at `deg`.
    fn tangent(&self, deg: f64) -> Point {
        let (s, c) = deg.to_radians().sin_cos();
        let sign = self.sweep_deg.signum();
        [-s * sign, c * sign]
    }
}

#[derive(Clone, Debug)]
enum Seg {
    Line(Point, Point),
    Arc(Arc),
    Circle {
        center: Point,
        radius: f64,
    },
    /// A flattened Bézier or ellipse, in path direction.
    Curve(Vec<Point>),
}

impl Seg {
    fn start(&self) -> Option<Point> {
        match self {
            Self::Line(a, _) => Some(*a),
            Self::Arc(arc) => Some(arc.from),
            Self::Circle { .. } => None,
            Self::Curve(points) => points.first().copied(),
        }
    }
    fn end(&self) -> Option<Point> {
        match self {
            Self::Line(_, b) => Some(*b),
            Self::Arc(arc) => Some(arc.to),
            Self::Circle { .. } => None,
            Self::Curve(points) => points.last().copied(),
        }
    }
    fn reversed(&self) -> Self {
        match self {
            Self::Line(a, b) => Self::Line(*b, *a),
            Self::Arc(arc) => Self::Arc(arc.reversed()),
            Self::Circle { .. } => self.clone(),
            Self::Curve(points) => Self::Curve(points.iter().rev().copied().collect()),
        }
    }
    /// Points in path direction, ends included.
    fn points(&self, tol: f64) -> Vec<Point> {
        match self {
            Self::Line(a, b) => vec![*a, *b],
            Self::Arc(arc) => arc.shape().flatten(tol),
            Self::Circle { center, radius } => Shape::Circle {
                center: *center,
                radius: *radius,
            }
            .flatten(tol),
            Self::Curve(points) => points.clone(),
        }
    }
}

/// Reader output in millimetres times `scale`, as directed segments.
fn segments(geometry: &[Geom2], scale: f64, tol: f64) -> Vec<Seg> {
    let p = |v: &solidcraft_engine::geom::Vec2| [v.x * scale, v.y * scale];
    let mut out = Vec::new();
    for g in geometry {
        match g {
            Geom2::Line(a, b) => out.push(Seg::Line(p(a), p(b))),
            Geom2::Circle(c, r) => out.push(Seg::Circle {
                center: p(c),
                radius: r * scale,
            }),
            Geom2::Arc { c, a, b } => {
                let (c, a, b) = (p(c), p(a), p(b));
                let radius = distance(c, a);
                if radius <= 0.0 || distance(a, b) <= JOIN_MM {
                    continue;
                }
                let start = angle_deg(sub(a, c));
                let sweep = (angle_deg(sub(b, c)) - start).rem_euclid(360.0);
                out.push(Seg::Arc(Arc {
                    center: c,
                    radius,
                    start_deg: start,
                    sweep_deg: sweep,
                    from: a,
                    to: b,
                }));
            }
            Geom2::Ellipse { c, major, minor } => {
                let (c, m) = (p(c), sub(p(major), p(c)));
                let a = norm(m);
                let b = minor * scale;
                if a <= 0.0 || b <= 0.0 {
                    continue;
                }
                let u = [m[0] / a, m[1] / a];
                let v = [-u[1], u[0]];
                // Chord error of an ellipse is at most that of a circle of
                // its major radius with the same step.
                let n = arc_steps(a, 360.0, tol).max(8);
                let mut points: Vec<Point> = (0..=n)
                    .map(|i| {
                        let t = std::f64::consts::TAU * i as f64 / n as f64;
                        let (s, co) = t.sin_cos();
                        [
                            c[0] + u[0] * a * co + v[0] * b * s,
                            c[1] + u[1] * a * co + v[1] * b * s,
                        ]
                    })
                    .collect();
                if let Some(first) = points.first().copied() {
                    points.pop();
                    points.push(first);
                }
                out.push(Seg::Curve(points));
            }
            Geom2::Spline { pts, .. } if pts.len() == 4 => {
                let q: Vec<Point> = pts.iter().map(p).collect();
                if let [p0, p1, p2, p3] = q.as_slice() {
                    out.push(Seg::Curve(cubic(*p0, *p1, *p2, *p3, tol)));
                }
            }
            Geom2::Spline { pts, .. } => {
                out.push(Seg::Curve(pts.iter().map(p).collect()));
            }
            Geom2::Conic { a, apex, b, .. } => {
                out.push(Seg::Curve(quadratic(p(a), p(apex), p(b), tol)));
            }
            Geom2::Point(_) | Geom2::Text { .. } => {}
        }
    }
    out
}

/// Uniform steps keeping the chord error of a cubic within `tol`: the
/// error of an n-step polyline is at most max|B''| / (8 n²), and
/// max|B''| ≤ 6 · max(|p0 − 2p1 + p2|, |p1 − 2p2 + p3|).
fn cubic(p0: Point, p1: Point, p2: Point, p3: Point, tol: f64) -> Vec<Point> {
    let d1 = norm(add(sub(p0, scale_by(p1, 2.0)), p2));
    let d2 = norm(add(sub(p1, scale_by(p2, 2.0)), p3));
    let n = steps(0.75 * d1.max(d2), tol);
    (0..=n)
        .map(|i| {
            let t = i as f64 / n as f64;
            let u = 1.0 - t;
            let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
            [
                a * p0[0] + b * p1[0] + c * p2[0] + d * p3[0],
                a * p0[1] + b * p1[1] + c * p2[1] + d * p3[1],
            ]
        })
        .collect()
}

/// As [`cubic`]: |B''| = 2 |p0 − 2p1 + p2| for a quadratic.
fn quadratic(p0: Point, p1: Point, p2: Point, tol: f64) -> Vec<Point> {
    let d = norm(add(sub(p0, scale_by(p1, 2.0)), p2));
    let n = steps(0.25 * d, tol);
    (0..=n)
        .map(|i| {
            let t = i as f64 / n as f64;
            let u = 1.0 - t;
            let (a, b, c) = (u * u, 2.0 * u * t, t * t);
            [
                a * p0[0] + b * p1[0] + c * p2[0],
                a * p0[1] + b * p1[1] + c * p2[1],
            ]
        })
        .collect()
}

/// `ceil(sqrt(k / tol))`, within 1–[`MAX_CURVE_SEGMENTS`].
fn steps(k: f64, tol: f64) -> usize {
    let n = (k / tol).sqrt().ceil();
    if n.is_finite() && n >= 1.0 {
        (n as usize).min(MAX_CURVE_SEGMENTS)
    } else {
        1
    }
}

/// Steps keeping an arc's sagitta within `tol`.
fn arc_steps(radius: f64, sweep_deg: f64, tol: f64) -> usize {
    if radius <= tol {
        return 4;
    }
    let step = 2.0 * (1.0 - tol / radius).acos();
    let n = (sweep_deg.abs().to_radians() / step).ceil();
    if n.is_finite() && n >= 1.0 {
        (n as usize).min(MAX_CURVE_SEGMENTS)
    } else {
        1
    }
}

/// Segments joined end to start.
#[derive(Clone, Debug, Default)]
struct Chain {
    segs: Vec<Seg>,
    closed: bool,
}

impl Chain {
    fn can_fill(&self) -> bool {
        self.closed
            || self.segs.len() >= 2
            || matches!(self.segs.as_slice(), [Seg::Curve(p)] if p.len() >= 3)
    }
    /// The chain as one ring of points (curves and arcs flattened), the
    /// first point not repeated.
    fn ring(&self, tol: f64) -> Vec<Point> {
        let mut ring: Vec<Point> = Vec::new();
        for seg in &self.segs {
            for p in seg.points(tol) {
                if ring.last().is_none_or(|q| distance(*q, p) > JOIN_MM) {
                    ring.push(p);
                }
            }
        }
        if ring.len() > 1
            && ring
                .first()
                .zip(ring.last())
                .is_some_and(|(a, b)| distance(*a, *b) <= JOIN_MM)
        {
            ring.pop();
        }
        ring
    }
}

/// Join an element's segments into chains, in path order. Arcs come
/// counter-clockwise from the reader, so either direction may continue a
/// chain.
fn chains(segments: Vec<Seg>) -> Vec<Chain> {
    let mut out: Vec<Chain> = Vec::new();
    let mut current = Chain::default();
    let close = |chain: &mut Chain, out: &mut Vec<Chain>| {
        if !chain.segs.is_empty() {
            chain.closed = chain.closed
                || chain
                    .segs
                    .first()
                    .and_then(Seg::start)
                    .zip(chain.segs.last().and_then(Seg::end))
                    .is_some_and(|(a, b)| distance(a, b) <= JOIN_MM);
            out.push(std::mem::take(chain));
        }
    };
    for seg in segments {
        if matches!(seg, Seg::Circle { .. }) {
            close(&mut current, &mut out);
            out.push(Chain {
                segs: vec![seg],
                closed: true,
            });
            continue;
        }
        let joins = |end: Option<Point>, start: Option<Point>| {
            end.zip(start)
                .is_some_and(|(a, b)| distance(a, b) <= JOIN_MM)
        };
        let end = current.segs.last().and_then(Seg::end);
        let closed_already =
            current.segs.len() > 1 && joins(end, current.segs.first().and_then(Seg::start));
        if !current.segs.is_empty() && !closed_already {
            if joins(end, seg.start()) {
                current.segs.push(seg);
                continue;
            }
            if matches!(seg, Seg::Arc(_)) && joins(end, seg.end()) {
                current.segs.push(seg.reversed());
                continue;
            }
            // A lone first arc may run the other way.
            if let [first @ Seg::Arc(_)] = current.segs.as_mut_slice() {
                let flipped = first.reversed();
                if joins(flipped.end(), seg.start()) {
                    *first = flipped;
                    current.segs.push(seg);
                    continue;
                }
            }
        }
        close(&mut current, &mut out);
        current.segs.push(seg);
    }
    close(&mut current, &mut out);
    out
}

/// Corner points with fillet radii when the chain is straight edges whose
/// arcs each round the corner between two of them, tangent to both.
fn fillets(segs: &[Seg], closed: bool) -> Option<(Vec<Point>, Vec<f64>)> {
    let mut segs: Vec<Seg> = segs.to_vec();
    if segs
        .iter()
        .any(|s| matches!(s, Seg::Curve(_) | Seg::Circle { .. }))
    {
        return None;
    }
    if closed {
        let first_line = segs.iter().position(|s| matches!(s, Seg::Line(..)))?;
        segs.rotate_left(first_line);
    } else if !matches!(segs.first(), Some(Seg::Line(..)))
        || !matches!(segs.last(), Some(Seg::Line(..)))
    {
        return None;
    }
    let n = segs.len();
    let mut points = Vec::new();
    let mut radii = Vec::new();
    if !closed {
        points.push(segs.first()?.start()?);
        radii.push(0.0);
    }
    let mut i = 0;
    while i < n {
        let Seg::Line(a, b) = segs.get(i)? else {
            return None;
        };
        let last = i + 1 == n;
        match segs.get((i + 1) % n) {
            _ if last && !closed => {
                points.push(*b);
                radii.push(0.0);
                i += 1;
            }
            Some(Seg::Line(..)) => {
                points.push(*b);
                radii.push(0.0);
                i += 1;
            }
            Some(Seg::Arc(arc)) => {
                let next = segs.get((i + 2) % n)?;
                let Seg::Line(c, d) = next else {
                    return None;
                };
                if !closed && i + 2 >= n {
                    return None;
                }
                let d1 = unit(sub(*b, *a))?;
                let d2 = unit(sub(*d, *c))?;
                let t0 = arc.tangent(arc.start_deg);
                let t1 = arc.tangent(arc.start_deg + arc.sweep_deg);
                let sweep = arc.sweep_deg.abs();
                if dot(t0, d1) < 1.0 - 1e-6 || dot(t1, d2) < 1.0 - 1e-6 || sweep >= 179.0 {
                    return None;
                }
                let reach = arc.radius * (sweep.to_radians() / 2.0).tan();
                points.push(add(*b, scale_by(d1, reach)));
                radii.push(arc.radius);
                i += 2;
            }
            _ => return None,
        }
    }
    if radii.iter().all(|r| *r <= 0.0) {
        return Some((points, vec![]));
    }
    if corner_radii_fit(&points, &radii, closed) {
        return Some((points, radii));
    }
    // Tangent fillets meeting on an edge fit up to rounding.
    let fitted = fit_corner_radii(&points, &radii, closed);
    let close = fitted
        .iter()
        .zip(&radii)
        .all(|(f, r)| (f - r).abs() <= 1e-6 * r.max(1.0));
    (close && corner_radii_fit(&points, &fitted, closed)).then_some((points, fitted))
}

/// An open path as polylines of at most [`MAX_SHAPE_POINTS`] points (a
/// single line stays a line).
fn polylines(points: Vec<Point>, radii: Vec<f64>) -> Vec<Shape> {
    match points.as_slice() {
        [] | [_] => vec![],
        [a, b] => vec![Shape::Line { a: *a, b: *b }],
        _ if points.len() <= MAX_SHAPE_POINTS => vec![Shape::Polyline { points, radii }],
        _ => {
            // Split with one shared point; rounding is dropped at the cuts.
            let mut out = Vec::new();
            let mut start = 0;
            while start + 1 < points.len() {
                let end = (start + MAX_SHAPE_POINTS).min(points.len());
                let chunk = points
                    .get(start..end)
                    .map(<[Point]>::to_vec)
                    .unwrap_or_default();
                let mut r = radii
                    .get(start..end)
                    .map(<[f64]>::to_vec)
                    .unwrap_or_default();
                if let Some(first) = r.first_mut() {
                    *first = 0.0;
                }
                if let Some(last) = r.last_mut() {
                    *last = 0.0;
                }
                out.extend(polylines(chunk, r));
                start = end - 1;
            }
            out
        }
    }
}

fn sub(a: Point, b: Point) -> Point {
    [a[0] - b[0], a[1] - b[1]]
}
fn add(a: Point, b: Point) -> Point {
    [a[0] + b[0], a[1] + b[1]]
}
fn scale_by(a: Point, k: f64) -> Point {
    [a[0] * k, a[1] * k]
}
fn dot(a: Point, b: Point) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}
fn cross(a: Point, b: Point) -> f64 {
    a[0] * b[1] - a[1] * b[0]
}
fn norm(a: Point) -> f64 {
    a[0].hypot(a[1])
}
fn distance(a: Point, b: Point) -> f64 {
    norm(sub(a, b))
}
fn unit(a: Point) -> Option<Point> {
    let n = norm(a);
    (n > 1e-12).then(|| [a[0] / n, a[1] / n])
}
fn angle_deg(v: Point) -> f64 {
    v[1].atan2(v[0]).to_degrees()
}

#[cfg(test)]
mod tests;
