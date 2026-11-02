//! Schematic printing: a vector PDF with one page per sheet at its paper
//! size, drawn only from `pcbcraft_core::schematic::scene::sheet_scene`
//! (page frame and title block, labels, power ports, sheet symbols,
//! furniture, wires, junction dots, no-connect crosses and units), so the
//! print matches the canvas. Black ink on white; text in the standard
//! Helvetica font (WinAnsi encoding). A sheet symbol links to its child
//! sheet's page when that page is printed. Written with `pdf-writer`
//! (MIT OR Apache-2.0); content streams are left uncompressed.
use crate::persistence::atomic_write;
use crate::{Error, Result};
use pcbcraft_core::geometry::{Point, Shape};
use pcbcraft_core::library::{Fill, HAlign, PlacedText};
use pcbcraft_core::schematic::scene::{self, Drawing, SheetScene};
use pcbcraft_core::{Project, SheetId};
use pdf_writer::types::{ActionType, AnnotationType, LineCapStyle, LineJoinStyle};
use pdf_writer::{Content, Finish, Name, Pdf, Rect, Ref, Str, TextStr};
use std::path::Path;

/// Pages one PDF holds.
pub const MAX_PAGES: usize = 256;
/// Points per millimetre.
const PT_PER_MM: f64 = 72.0 / 25.4;
/// Helvetica's cap height per unit of font size (Adobe core-font metrics).
const CAP_HEIGHT: f64 = 0.718;
/// How far curves may stray from their chords, mm.
const CHORD_MM: f64 = 0.02;
/// Thinnest printed stroke, mm.
const MIN_STROKE_MM: f64 = 0.1;
/// Junction dot radius and no-connect cross half-size, mm.
const DOT_MM: f64 = 0.4;
const CROSS_MM: f64 = 0.8;
const FONT: Name<'static> = Name(b"F1");

/// Helvetica advance widths of the printable ASCII characters (32–126), in
/// thousandths of the font size, from Adobe's published core-font metrics.
const HELVETICA: [u16; 95] = [
    278, 278, 355, 556, 556, 889, 667, 191, 333, 333, 389, 584, 278, 333, 278, 278, 556, 556, 556,
    556, 556, 556, 556, 556, 556, 556, 278, 278, 584, 584, 584, 556, 1015, 667, 667, 722, 722, 667,
    611, 778, 722, 278, 500, 667, 556, 833, 722, 778, 667, 778, 722, 667, 611, 722, 667, 944, 667,
    667, 611, 278, 278, 278, 469, 556, 333, 556, 556, 500, 556, 556, 278, 556, 556, 222, 222, 500,
    222, 833, 556, 556, 556, 556, 333, 500, 278, 556, 500, 722, 500, 500, 500, 334, 260, 334, 584,
];

/// Text as WinAnsi bytes: ASCII and Latin-1 as they are, typographic
/// punctuation at its WinAnsi code, Ω spelled "Ohm", anything else `?`.
fn win_ansi(text: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len());
    for c in text.chars() {
        let code = u32::from(c);
        match c {
            ' '..='~' => out.push(code as u8),
            '\u{a0}'..='\u{ff}' => out.push(code as u8),
            '€' => out.push(0x80),
            '…' => out.push(0x85),
            '‘' => out.push(0x91),
            '’' => out.push(0x92),
            '“' => out.push(0x93),
            '”' => out.push(0x94),
            '•' => out.push(0x95),
            '–' => out.push(0x96),
            '—' => out.push(0x97),
            '\u{3a9}' | '\u{2126}' => out.extend_from_slice(b"Ohm"),
            'μ' => out.push(0xb5),
            _ => out.push(b'?'),
        }
    }
    out
}

/// Width of WinAnsi bytes at `size` points.
fn width(bytes: &[u8], size: f64) -> f64 {
    let units: u32 = bytes
        .iter()
        .map(|b| {
            b.checked_sub(32)
                .and_then(|i| HELVETICA.get(usize::from(i)))
                .map_or(556, |w| u32::from(*w))
        })
        .sum();
    f64::from(units) / 1000.0 * size
}

/// Maps sheet millimetres (y up, page y ∈ [−h, 0]) to page points.
struct Page {
    height_mm: f64,
}

impl Page {
    fn pt(&self, p: Point) -> (f32, f32) {
        (
            (p[0] * PT_PER_MM) as f32,
            ((p[1] + self.height_mm) * PT_PER_MM) as f32,
        )
    }
}

fn stroke_shape(
    c: &mut Content,
    page: &Page,
    shape: &Shape,
    width_mm: f64,
    fill: Option<f32>,
    stroke: bool,
) {
    let points = shape.flatten(CHORD_MM);
    let Some(first) = points.first() else {
        return;
    };
    let closed = shape.is_closed();
    let (x, y) = page.pt(*first);
    c.set_line_width((width_mm.max(MIN_STROKE_MM) * PT_PER_MM) as f32);
    c.move_to(x, y);
    for p in &points[1..] {
        let (x, y) = page.pt(*p);
        c.line_to(x, y);
    }
    if closed {
        c.close_path();
    }
    match (fill, stroke && width_mm > 0.0 || fill.is_none()) {
        (Some(gray), true) => {
            c.set_fill_gray(gray);
            c.fill_nonzero_and_stroke();
        }
        (Some(gray), false) => {
            c.set_fill_gray(gray);
            c.fill_nonzero();
        }
        (None, _) => {
            c.stroke();
        }
    }
}

fn line(c: &mut Content, page: &Page, a: Point, b: Point, width_mm: f64) {
    stroke_shape(c, page, &Shape::Line { a, b }, width_mm, None, true);
}

fn text(c: &mut Content, page: &Page, t: &PlacedText) {
    if t.text.trim().is_empty() || !(t.height.is_finite() && t.height > 0.0) {
        return;
    }
    let size = t.height / CAP_HEIGHT * PT_PER_MM;
    let (rotation_deg, lines) = t.lines();
    let justify = t.line_justify();
    let (s, co) = rotation_deg.to_radians().sin_cos();
    c.set_fill_gray(0.0);
    for l in lines {
        if l.text.is_empty() {
            continue;
        }
        let bytes = win_ansi(l.text);
        let w = width(&bytes, size);
        let dx = match justify.h {
            HAlign::Left => 0.0,
            HAlign::Center => -w / 2.0,
            HAlign::Right => -w,
        };
        // Each line centres its cap box on its anchor.
        let dy = -t.height * PT_PER_MM / 2.0;
        let (ax, ay) = page.pt(l.at);
        let x = f64::from(ax) + dx * co - dy * s;
        let y = f64::from(ay) + dx * s + dy * co;
        c.begin_text();
        c.set_font(FONT, size as f32);
        c.set_text_matrix([
            co as f32, s as f32, -s as f32, co as f32, x as f32, y as f32,
        ]);
        c.show(Str(&bytes));
        c.end_text();
    }
}

fn drawing(c: &mut Content, page: &Page, d: &Drawing) {
    for s in &d.strokes {
        stroke_shape(c, page, &s.shape, s.width, s.filled.then_some(0.0), true);
    }
    for t in &d.texts {
        text(c, page, t);
    }
}

/// The content stream of one sheet.
fn content(scene: &SheetScene) -> Vec<u8> {
    let page = Page {
        height_mm: scene.size[1],
    };
    let mut c = Content::new();
    c.set_line_cap(LineCapStyle::RoundCap);
    c.set_line_join(LineJoinStyle::RoundJoin);
    c.set_stroke_gray(0.0);
    c.set_fill_gray(0.0);
    drawing(&mut c, &page, &scene.frame);
    for wire in &scene.wires {
        stroke_shape(
            &mut c,
            &page,
            &Shape::polyline(wire.to_vec()),
            scene::WIRE_MM,
            None,
            true,
        );
    }
    for unit in &scene.units {
        for s in &unit.shapes {
            let fill = match s.fill {
                Fill::None => None,
                Fill::Body => Some(1.0),
                Fill::Solid => Some(0.0),
            };
            stroke_shape(&mut c, &page, &s.shape, s.width, fill, true);
        }
        for pin in &unit.pins {
            line(&mut c, &page, pin.end, pin.line_end, scene::LINE_MM);
            for d in &pin.decorations {
                stroke_shape(&mut c, &page, d, scene::LINE_MM, None, true);
            }
            for t in pin.name.iter().chain(&pin.number) {
                text(&mut c, &page, t);
            }
        }
        for t in unit.texts.iter().chain(&unit.fields) {
            text(&mut c, &page, t);
        }
    }
    drawing(&mut c, &page, &scene.items);
    for p in &scene.dots {
        stroke_shape(
            &mut c,
            &page,
            &Shape::Circle {
                center: *p,
                radius: DOT_MM,
            },
            0.0,
            Some(0.0),
            false,
        );
    }
    for p in &scene.no_connects {
        let h = CROSS_MM;
        line(
            &mut c,
            &page,
            [p[0] - h, p[1] - h],
            [p[0] + h, p[1] + h],
            scene::LINE_MM,
        );
        line(
            &mut c,
            &page,
            [p[0] - h, p[1] + h],
            [p[0] + h, p[1] - h],
            scene::LINE_MM,
        );
    }
    c.finish().into_vec()
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
    let mut next = 1;
    let mut alloc = || {
        let r = Ref::new(next);
        next += 1;
        r
    };
    let catalog = alloc();
    let tree = alloc();
    let font = alloc();
    let info = alloc();
    let page_refs: Vec<(SheetId, Ref, Ref)> =
        pages.iter().map(|s| (*s, alloc(), alloc())).collect();
    let page_of = |sheet: SheetId| {
        page_refs
            .iter()
            .find(|(s, _, _)| *s == sheet)
            .map(|(_, page, _)| *page)
    };
    let mut pdf = Pdf::new();
    pdf.catalog(catalog).pages(tree);
    pdf.pages(tree)
        .kids(page_refs.iter().map(|(_, page, _)| *page))
        .count(i32::try_from(page_refs.len()).unwrap_or(i32::MAX));
    pdf.type1_font(font)
        .base_font(Name(b"Helvetica"))
        .encoding_predefined(Name(b"WinAnsiEncoding"));
    let title = if project.properties.title.trim().is_empty() {
        project.name.as_str()
    } else {
        project.properties.title.as_str()
    };
    pdf.document_info(info)
        .title(TextStr(title))
        .producer(TextStr("Pcbcraft"));
    for (sheet, page_ref, content_ref) in &page_refs {
        let scene = scene::sheet_scene(project, *sheet);
        let [w, h] = scene.size;
        let k = PT_PER_MM;
        let geometry = Page { height_mm: h };
        // A link over each sheet symbol whose child page is printed.
        let mut links = Vec::new();
        for symbol in project.sheet_symbols.iter().filter(|s| s.sheet == *sheet) {
            if let Some(target) = page_of(symbol.child) {
                let [x, y] = symbol.position;
                let [sw, sh] = symbol.size;
                let (x0, y0) = geometry.pt([x, y - sh]);
                let (x1, y1) = geometry.pt([x + sw, y]);
                links.push((alloc(), Rect::new(x0, y0, x1, y1), target));
            }
        }
        let mut page = pdf.page(*page_ref);
        page.parent(tree)
            .media_box(Rect::new(0.0, 0.0, (w * k) as f32, (h * k) as f32))
            .contents(*content_ref);
        page.resources().fonts().pair(FONT, font);
        if !links.is_empty() {
            page.annotations(links.iter().map(|(r, _, _)| *r));
        }
        page.finish();
        for (r, rect, target) in links {
            let mut annotation = pdf.annotation(r);
            annotation
                .subtype(AnnotationType::Link)
                .rect(rect)
                .border(0.0, 0.0, 0.0, None);
            annotation
                .action()
                .action_type(ActionType::GoTo)
                .destination()
                .page(target)
                .fit();
        }
        let bytes = content(&scene);
        pdf.stream(*content_ref, &bytes);
    }
    Ok(pdf.finish())
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
