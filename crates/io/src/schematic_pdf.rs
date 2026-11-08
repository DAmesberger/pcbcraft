//! Schematic printing: a vector PDF with one page per sheet at its paper
//! size, drawn only from `pcbcraft_core::schematic::scene::sheet_scene`
//! (page frame and title block, labels, power ports, sheet symbols,
//! furniture, wires, junction dots, no-connect crosses and units), so the
//! print matches the canvas. Black ink on white, through the shared PDF
//! canvas ([`crate::pdf`]). A sheet symbol links to its child sheet's page
//! when that page is printed.
use crate::pdf::{Align, Colour, Document, Font, Page, Stroke};
use crate::persistence::atomic_write;
use crate::{Error, Result};
use pcbcraft_core::geometry::{Point, Shape};
use pcbcraft_core::library::{Fill, HAlign, PlacedText};
use pcbcraft_core::schematic::scene::{self, Drawing, SheetScene};
use pcbcraft_core::{Project, SheetId};
use std::path::Path;

/// Pages one schematic PDF holds.
pub const MAX_PAGES: usize = 256;
/// Helvetica's cap height per unit of font size (Adobe core-font metrics).
const CAP_HEIGHT: f64 = 0.718;
/// How far curves may stray from their chords, mm.
const CHORD_MM: f64 = 0.02;
/// Thinnest printed stroke, mm.
const MIN_STROKE_MM: f64 = 0.1;
/// Junction dot radius, mm. The no-connect cross is core's
/// `schematic::NO_CONNECT_MM`, as on the canvas.
const DOT_MM: f64 = 0.4;
const WHITE: Colour = Colour::Grey(1.0);

/// Text the WinAnsi canvas can set: Ω (resistance values) is spelled
/// "Ohm"; anything else the encoding lacks prints as `?`.
fn printable(text: &str) -> String {
    text.replace(['\u{3a9}', '\u{2126}'], "Ohm")
}

/// Sheet millimetres (y up, page top-left corner at `top_left`) to page
/// millimetres (y up from the bottom-left corner).
struct Frame {
    height: f64,
    top_left: Point,
}

impl Frame {
    fn of(scene: &SheetScene) -> Self {
        Self {
            height: scene.size[1],
            top_left: scene.top_left,
        }
    }
    fn map(&self, p: Point) -> Point {
        [
            p[0] - self.top_left[0],
            p[1] - self.top_left[1] + self.height,
        ]
    }
}

fn ink(width: f64) -> Stroke {
    Stroke::new(width.max(MIN_STROKE_MM), Colour::BLACK)
}

/// Stroke a shape, filling it first with `fill` when given.
fn shape(page: &mut Page, f: &Frame, shape: &Shape, width: f64, fill: Option<Colour>) {
    let points: Vec<Point> = shape
        .flatten(CHORD_MM)
        .into_iter()
        .map(|p| f.map(p))
        .collect();
    let closed = shape.is_closed();
    if let Some(colour) = fill
        && closed
    {
        page.fill(std::slice::from_ref(&points), colour, false);
    }
    if fill.is_none() || width > 0.0 {
        page.polyline(&points, closed, &ink(width));
    }
}

fn line(page: &mut Page, f: &Frame, a: Point, b: Point) {
    page.line(f.map(a), f.map(b), &ink(scene::LINE_MM));
}

/// One placed text: each line's cap box is centred on its anchor, then
/// aligned horizontally by the text's justification.
fn text(page: &mut Page, f: &Frame, t: &PlacedText) {
    if t.text.trim().is_empty() || !(t.height.is_finite() && t.height > 0.0) {
        return;
    }
    let size = t.height / CAP_HEIGHT;
    let (rotation_deg, lines) = t.lines();
    let align = match t.line_justify().h {
        HAlign::Left => Align::Left,
        HAlign::Center => Align::Centre,
        HAlign::Right => Align::Right,
    };
    let (s, c) = rotation_deg.to_radians().sin_cos();
    let dy = -t.height / 2.0;
    for l in lines {
        if l.text.is_empty() {
            continue;
        }
        let [x, y] = f.map(l.at);
        let baseline = [x - dy * s, y + dy * c];
        page.text(
            baseline,
            size,
            &printable(l.text),
            Font::Regular,
            align,
            rotation_deg,
            Colour::BLACK,
        );
    }
}

fn drawing(page: &mut Page, f: &Frame, d: &Drawing) {
    for s in &d.strokes {
        shape(
            page,
            f,
            &s.shape,
            s.width,
            s.filled.then_some(Colour::BLACK),
        );
    }
    for t in &d.texts {
        text(page, f, t);
    }
}

/// Draw one sheet onto a page of its paper size.
fn draw(scene: &SheetScene) -> Page {
    let f = Frame::of(scene);
    let mut page = Page::new(scene.size);
    drawing(&mut page, &f, &scene.frame);
    for wire in &scene.wires {
        shape(&mut page, &f, wire, scene::WIRE_MM, None);
    }
    for unit in &scene.units {
        for s in &unit.shapes {
            let fill = match s.fill {
                Fill::None => None,
                Fill::Body => Some(WHITE),
                Fill::Solid => Some(Colour::BLACK),
            };
            shape(&mut page, &f, &s.shape, s.width, fill);
        }
        for pin in &unit.pins {
            line(&mut page, &f, pin.end, pin.line_end);
            for d in &pin.decorations {
                shape(&mut page, &f, d, scene::LINE_MM, None);
            }
            for t in pin.name.iter().chain(&pin.number) {
                text(&mut page, &f, t);
            }
        }
        for t in unit.texts.iter().chain(&unit.fields) {
            text(&mut page, &f, t);
        }
    }
    drawing(&mut page, &f, &scene.items);
    for p in &scene.dots {
        page.disc(f.map(*p), DOT_MM, Colour::BLACK);
    }
    for p in &scene.no_connects {
        let h = pcbcraft_core::schematic::NO_CONNECT_MM;
        line(&mut page, &f, [p[0] - h, p[1] - h], [p[0] + h, p[1] + h]);
        line(&mut page, &f, [p[0] - h, p[1] + h], [p[0] + h, p[1] - h]);
    }
    page
}

/// The sheets to print, in page order without repeats; all for an empty
/// selection. An unknown sheet is refused.
fn selection(project: &Project, sheets: &[SheetId]) -> Result<Vec<SheetId>> {
    if let Some(missing) = sheets.iter().find(|s| project.sheet(**s).is_none()) {
        return Err(Error::Invalid(format!(
            "sheet {missing} is not in the project"
        )));
    }
    let pages: Vec<SheetId> = project
        .sheets
        .list
        .iter()
        .map(|s| s.id)
        .filter(|id| sheets.is_empty() || sheets.contains(id))
        .collect();
    if pages.len() > MAX_PAGES {
        return Err(Error::TooLarge(format!(
            "a schematic PDF holds at most {MAX_PAGES} pages"
        )));
    }
    Ok(pages)
}

/// A vector PDF of `sheets` (all for an empty slice), one page per sheet in
/// page order at its paper size.
pub fn schematic_pdf(project: &Project, sheets: &[SheetId]) -> Result<Vec<u8>> {
    let pages = selection(project, sheets)?;
    let mut doc = Document::new();
    doc.title(if project.properties.title.trim().is_empty() {
        &project.name
    } else {
        &project.properties.title
    });
    for sheet in &pages {
        let scene = scene::sheet_scene(project, *sheet);
        let f = Frame::of(&scene);
        let mut page = draw(&scene);
        // A link over each sheet symbol whose child page is printed.
        for symbol in project.sheet_symbols.iter().filter(|s| s.sheet == *sheet) {
            if let Some(target) = pages.iter().position(|s| *s == symbol.child) {
                let [x, y] = symbol.position;
                let [w, h] = symbol.size;
                page.link(f.map([x, y - h]), f.map([x + w, y]), target);
            }
        }
        doc.push(page)?;
    }
    doc.finish()
}

/// Write [`schematic_pdf`] to `path` atomically; returns the page count.
pub fn export_schematic_pdf(project: &Project, sheets: &[SheetId], path: &Path) -> Result<usize> {
    let pages = selection(project, sheets)?.len();
    let bytes = schematic_pdf(project, sheets)?;
    atomic_write(path, &bytes)?;
    Ok(pages)
}

#[cfg(test)]
mod tests;
