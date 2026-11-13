//! A small vector canvas that writes PDF through `pdf-writer` (MIT OR
//! Apache-2.0): pages in millimetres with y up, strokes, fills, circles and
//! text in the standard Helvetica fonts (WinAnsi encoding, no embedding).
//! Output is deterministic: no dates, no random file ids, uncompressed
//! content streams. Shared by the fabrication drawings and any other
//! printable output; it knows nothing about boards.
use crate::{Error, Result};
use pdf_writer::{Content, Name, Pdf, Rect, Ref, Str};

/// Page sizes in mm, width × height.
pub const A4_PORTRAIT: [f64; 2] = [210.0, 297.0];
pub const A4_LANDSCAPE: [f64; 2] = [297.0, 210.0];
pub const A3_LANDSCAPE: [f64; 2] = [420.0, 297.0];
/// Pages in one document.
pub const MAX_PAGES: usize = 1000;
/// Bytes of one page's content stream.
pub const MAX_PAGE_BYTES: usize = 64 * 1024 * 1024;
/// Characters of one text run.
pub const MAX_TEXT_CHARS: usize = 4096;

const PT_PER_MM: f64 = 72.0 / 25.4;

/// A paint colour.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Colour {
    /// 0 black, 1 white.
    Grey(f32),
    Rgb(f32, f32, f32),
}

impl Colour {
    pub const BLACK: Self = Self::Grey(0.0);
}

/// A stroke: width in mm and colour. Lines have round caps and joins.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stroke {
    pub width: f64,
    pub colour: Colour,
}

impl Stroke {
    pub fn new(width: f64, colour: Colour) -> Self {
        Self { width, colour }
    }
}

/// The two fonts the canvas offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Font {
    Regular,
    Bold,
}

/// Where a text's anchor point is along its baseline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Left,
    Centre,
    Right,
}

/// Helvetica advance widths (1/1000 em) of the printable ASCII characters
/// 32–126, from Adobe's freely distributable core-font metrics. Other
/// characters use 556 (the digit width); Helvetica-Bold is measured with
/// the same table, which runs a few percent narrow.
const HELVETICA: [u16; 95] = [
    278, 278, 355, 556, 556, 889, 667, 191, 333, 333, 389, 584, 278, 333, 278,
    278, // ' '..'/'
    556, 556, 556, 556, 556, 556, 556, 556, 556, 556, // '0'..'9'
    278, 278, 584, 584, 584, 556, 1015, // ':'..'@'
    667, 667, 722, 722, 667, 611, 778, 722, 278, 500, 667, 556, 833, // 'A'..'M'
    722, 778, 667, 778, 722, 667, 611, 722, 667, 944, 667, 667, 611, // 'N'..'Z'
    278, 278, 278, 469, 556, 333, // '['..'`'
    556, 556, 500, 556, 556, 278, 556, 556, 222, 222, 500, 222, 833, // 'a'..'m'
    556, 556, 556, 556, 333, 500, 278, 556, 500, 722, 500, 500, 500, // 'n'..'z'
    334, 260, 334, 584, // '{'..'~'
];

/// The width of `text` set at `size` mm (the em size), in mm.
pub fn text_width(text: &str, size: f64) -> f64 {
    let units: f64 = text
        .chars()
        .take(MAX_TEXT_CHARS)
        .map(|c| {
            let code = c as u32;
            if (32..127).contains(&code) {
                f64::from(HELVETICA[(code - 32) as usize])
            } else {
                556.0
            }
        })
        .sum();
    units / 1000.0 * size
}

/// The WinAnsi byte of `c`, or `?` when the encoding lacks it.
fn win_ansi(c: char) -> u8 {
    let code = c as u32;
    match code {
        32..=126 | 0xA0..=0xFF => code as u8,
        0x2013 => 0x96, // en dash
        0x2014 => 0x97, // em dash
        0x2026 => 0x85, // ellipsis
        0x2022 => 0x95, // bullet
        0x2018 => 0x91,
        0x2019 => 0x92,
        0x201C => 0x93,
        0x201D => 0x94,
        _ => b'?',
    }
}

fn pt(mm: f64) -> f32 {
    (mm * PT_PER_MM) as f32
}

fn finite(p: [f64; 2]) -> bool {
    p[0].is_finite() && p[1].is_finite()
}

/// One page being drawn.
pub struct Page {
    size: [f64; 2],
    content: Content,
    texts: Vec<String>,
    stroke: Option<(f32, Colour)>,
    fill: Option<Colour>,
    /// Link areas (lower-left and upper-right corner, mm) and the index of
    /// the page in the document each jumps to.
    links: Vec<([f64; 2], [f64; 2], usize)>,
}

impl Page {
    pub fn new(size: [f64; 2]) -> Self {
        let mut content = Content::new();
        content.set_line_cap(pdf_writer::types::LineCapStyle::RoundCap);
        content.set_line_join(pdf_writer::types::LineJoinStyle::RoundJoin);
        Self {
            size,
            content,
            texts: Vec::new(),
            stroke: None,
            fill: None,
            links: Vec::new(),
        }
    }

    /// A clickable area that jumps to page `target` (index in the
    /// document); ignored when that page does not exist or a corner is not
    /// finite.
    pub fn link(&mut self, min: [f64; 2], max: [f64; 2], target: usize) {
        if finite(min) && finite(max) {
            self.links.push((min, max, target));
        }
    }

    /// The page size in mm.
    pub fn size(&self) -> [f64; 2] {
        self.size
    }

    /// Every text run drawn on the page, in order (for tests and search).
    pub fn texts(&self) -> &[String] {
        &self.texts
    }

    fn set_stroke(&mut self, s: &Stroke) {
        let w = pt(s.width.max(0.0));
        if self.stroke != Some((w, s.colour)) {
            self.content.set_line_width(w);
            match s.colour {
                Colour::Grey(g) => self.content.set_stroke_gray(g),
                Colour::Rgb(r, g, b) => self.content.set_stroke_rgb(r, g, b),
            };
            self.stroke = Some((w, s.colour));
        }
    }

    fn set_fill(&mut self, c: Colour) {
        if self.fill != Some(c) {
            match c {
                Colour::Grey(g) => self.content.set_fill_gray(g),
                Colour::Rgb(r, g, b) => self.content.set_fill_rgb(r, g, b),
            };
            self.fill = Some(c);
        }
    }

    /// Add `points` as a subpath; false when fewer than two are finite.
    fn path(&mut self, points: &[[f64; 2]], closed: bool) -> bool {
        let mut iter = points.iter().filter(|p| finite(**p));
        let Some(first) = iter.next() else {
            return false;
        };
        self.content.move_to(pt(first[0]), pt(first[1]));
        let mut any = false;
        for p in iter {
            self.content.line_to(pt(p[0]), pt(p[1]));
            any = true;
        }
        if closed && any {
            self.content.close_path();
        }
        any
    }

    pub fn line(&mut self, a: [f64; 2], b: [f64; 2], stroke: &Stroke) {
        self.polyline(&[a, b], false, stroke);
    }

    /// An open or closed polyline.
    pub fn polyline(&mut self, points: &[[f64; 2]], closed: bool, stroke: &Stroke) {
        self.set_stroke(stroke);
        if self.path(points, closed) {
            self.content.stroke();
        } else {
            self.content.end_path();
        }
    }

    /// Fill closed loops (holes by the even-odd rule when `even_odd`).
    pub fn fill(&mut self, loops: &[Vec<[f64; 2]>], colour: Colour, even_odd: bool) {
        self.set_fill(colour);
        let mut any = false;
        for l in loops {
            any |= self.path(l, true);
        }
        if !any {
            self.content.end_path();
        } else if even_odd {
            self.content.fill_even_odd();
        } else {
            self.content.fill_nonzero();
        }
    }

    /// Fill and outline closed loops.
    pub fn fill_stroke(&mut self, loops: &[Vec<[f64; 2]>], fill: Colour, stroke: &Stroke) {
        self.fill(loops, fill, true);
        for l in loops {
            self.polyline(l, true, stroke);
        }
    }

    /// A rectangle from its lower-left corner and size.
    pub fn rect(&mut self, at: [f64; 2], size: [f64; 2], stroke: &Stroke) {
        let [x, y] = at;
        let [w, h] = size;
        self.polyline(
            &[[x, y], [x + w, y], [x + w, y + h], [x, y + h]],
            true,
            stroke,
        );
    }

    fn circle_path(&mut self, c: [f64; 2], r: f64) -> bool {
        if !(finite(c) && r.is_finite() && r > 0.0) {
            return false;
        }
        // Four cubic quarter arcs (control distance 0.5523 r).
        let k = 0.552_284_75 * r;
        let (x, y) = (c[0], c[1]);
        self.content.move_to(pt(x + r), pt(y));
        self.content
            .cubic_to(pt(x + r), pt(y + k), pt(x + k), pt(y + r), pt(x), pt(y + r));
        self.content
            .cubic_to(pt(x - k), pt(y + r), pt(x - r), pt(y + k), pt(x - r), pt(y));
        self.content
            .cubic_to(pt(x - r), pt(y - k), pt(x - k), pt(y - r), pt(x), pt(y - r));
        self.content
            .cubic_to(pt(x + k), pt(y - r), pt(x + r), pt(y - k), pt(x + r), pt(y));
        self.content.close_path();
        true
    }

    pub fn circle(&mut self, c: [f64; 2], r: f64, stroke: &Stroke) {
        self.set_stroke(stroke);
        if self.circle_path(c, r) {
            self.content.stroke();
        }
    }

    pub fn disc(&mut self, c: [f64; 2], r: f64, colour: Colour) {
        self.set_fill(colour);
        if self.circle_path(c, r) {
            self.content.fill_nonzero();
        }
    }

    /// Text with its anchor at `at` (baseline), `size` mm high (the em),
    /// turned `rotation_deg` counter-clockwise about the anchor.
    #[allow(clippy::too_many_arguments)]
    pub fn text(
        &mut self,
        at: [f64; 2],
        size: f64,
        text: &str,
        font: Font,
        align: Align,
        rotation_deg: f64,
        colour: Colour,
    ) {
        if !(finite(at) && size.is_finite() && size > 0.0 && rotation_deg.is_finite()) {
            return;
        }
        let text: String = text.chars().take(MAX_TEXT_CHARS).collect();
        let width = text_width(&text, size);
        let shift = match align {
            Align::Left => 0.0,
            Align::Centre => -width / 2.0,
            Align::Right => -width,
        };
        let (s, c) = rotation_deg.to_radians().sin_cos();
        let origin = [at[0] + c * shift, at[1] + s * shift];
        let bytes: Vec<u8> = text.chars().map(win_ansi).collect();
        self.set_fill(colour);
        self.content.begin_text();
        self.content.set_font(
            Name(match font {
                Font::Regular => b"F1",
                Font::Bold => b"F2",
            }),
            pt(size),
        );
        self.content.set_text_matrix([
            c as f32,
            s as f32,
            -s as f32,
            c as f32,
            pt(origin[0]),
            pt(origin[1]),
        ]);
        self.content.show(Str(&bytes));
        self.content.end_text();
        self.texts.push(text);
    }
}

/// A document of pages.
#[derive(Default)]
pub struct Document {
    pages: Vec<Page>,
    title: Option<String>,
}

impl Document {
    pub fn new() -> Self {
        Self::default()
    }

    /// The document title (Info dictionary).
    pub fn title(&mut self, title: &str) {
        self.title = Some(title.chars().take(MAX_TEXT_CHARS).collect());
    }

    pub fn push(&mut self, page: Page) -> Result<()> {
        if self.pages.len() >= MAX_PAGES {
            return Err(Error::TooLarge(format!(
                "a PDF holds at most {MAX_PAGES} pages"
            )));
        }
        self.pages.push(page);
        Ok(())
    }

    pub fn pages(&self) -> &[Page] {
        &self.pages
    }

    /// The finished PDF bytes.
    pub fn finish(self) -> Result<Vec<u8>> {
        let mut next = 1i32;
        let mut alloc = || {
            let r = Ref::new(next);
            next += 1;
            r
        };
        let catalog = alloc();
        let tree = alloc();
        let regular = alloc();
        let bold = alloc();
        let info = alloc();
        let mut pdf = Pdf::new();
        pdf.type1_font(regular)
            .base_font(Name(b"Helvetica"))
            .encoding_predefined(Name(b"WinAnsiEncoding"));
        pdf.type1_font(bold)
            .base_font(Name(b"Helvetica-Bold"))
            .encoding_predefined(Name(b"WinAnsiEncoding"));
        let refs: Vec<(Ref, Ref)> = self.pages.iter().map(|_| (alloc(), alloc())).collect();
        let ids: Vec<Ref> = refs.iter().map(|(page, _)| *page).collect();
        for (page, (page_id, content_id)) in self.pages.into_iter().zip(refs) {
            let bytes = page.content.finish();
            if bytes.len() > MAX_PAGE_BYTES {
                return Err(Error::TooLarge(format!(
                    "a PDF page's drawing exceeds {MAX_PAGE_BYTES} bytes"
                )));
            }
            let links: Vec<(Ref, Rect, Ref)> = page
                .links
                .iter()
                .filter_map(|(min, max, target)| {
                    let target = *ids.get(*target)?;
                    Some((
                        alloc(),
                        Rect::new(pt(min[0]), pt(min[1]), pt(max[0]), pt(max[1])),
                        target,
                    ))
                })
                .collect();
            let mut p = pdf.page(page_id);
            p.parent(tree)
                .media_box(Rect::new(0.0, 0.0, pt(page.size[0]), pt(page.size[1])))
                .contents(content_id);
            if !links.is_empty() {
                p.annotations(links.iter().map(|(r, _, _)| *r));
            }
            let mut resources = p.resources();
            let mut fonts = resources.fonts();
            fonts.pair(Name(b"F1"), regular);
            fonts.pair(Name(b"F2"), bold);
            drop(fonts);
            drop(resources);
            drop(p);
            for (r, rect, target) in links {
                let mut annotation = pdf.annotation(r);
                annotation
                    .subtype(pdf_writer::types::AnnotationType::Link)
                    .rect(rect)
                    .border(0.0, 0.0, 0.0, None);
                annotation
                    .action()
                    .action_type(pdf_writer::types::ActionType::GoTo)
                    .destination()
                    .page(target)
                    .fit();
            }
            pdf.stream(content_id, &bytes);
        }
        let count = i32::try_from(ids.len()).unwrap_or(i32::MAX);
        pdf.pages(tree).kids(ids).count(count);
        pdf.catalog(catalog).pages(tree);
        let mut doc_info = pdf.document_info(info);
        if let Some(title) = &self.title {
            doc_info.title(pdf_writer::TextStr(title));
        }
        doc_info.creator(pdf_writer::TextStr(sparksmith_core::product::NAME));
        drop(doc_info);
        Ok(pdf.finish())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widths_follow_the_helvetica_metrics() {
        // "1" is 556/1000 em; "W" 944; unknown characters count as 556.
        assert!((text_width("1", 10.0) - 5.56).abs() < 1e-9);
        assert!((text_width("W", 1.0) - 0.944).abs() < 1e-9);
        assert!((text_width("Ω", 1.0) - 0.556).abs() < 1e-9);
        assert_eq!(win_ansi('µ'), 0xB5);
        assert_eq!(win_ansi('–'), 0x96);
        assert_eq!(win_ansi('Ω'), b'?');
    }

    #[test]
    fn documents_are_deterministic_and_bounded() -> Result<()> {
        let build = || -> Result<Vec<u8>> {
            let mut doc = Document::new();
            doc.title("Test");
            let mut page = Page::new(A4_LANDSCAPE);
            page.rect([10.0, 10.0], [50.0, 20.0], &Stroke::new(0.3, Colour::BLACK));
            page.circle(
                [30.0, 30.0],
                5.0,
                &Stroke::new(0.2, Colour::Rgb(1.0, 0.0, 0.0)),
            );
            page.text(
                [20.0, 20.0],
                3.5,
                "R1 (x)",
                Font::Bold,
                Align::Centre,
                90.0,
                Colour::BLACK,
            );
            // Non-finite input draws nothing and does not fail.
            page.line(
                [f64::NAN, 0.0],
                [1.0, 1.0],
                &Stroke::new(0.1, Colour::BLACK),
            );
            page.text(
                [f64::INFINITY, 0.0],
                3.0,
                "x",
                Font::Regular,
                Align::Left,
                0.0,
                Colour::BLACK,
            );
            assert_eq!(page.texts(), ["R1 (x)"]);
            doc.push(page)?;
            doc.push(Page::new(A3_LANDSCAPE))?;
            doc.finish()
        };
        let a = build()?;
        assert_eq!(a, build()?);
        assert!(a.starts_with(b"%PDF-"));
        let tail = String::from_utf8_lossy(&a[a.len().saturating_sub(16)..]).to_string();
        assert!(tail.trim_end().ends_with("%%EOF"), "{tail}");
        let text = String::from_utf8_lossy(&a);
        assert_eq!(text.matches("/Parent").count(), 2, "{text}");
        assert!(text.contains("R1 ") && text.contains(") Tj"), "{text}");
        Ok(())
    }
}
