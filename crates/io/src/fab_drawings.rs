//! The fabrication and assembly drawings as PDF, laid out from the core's
//! `fabrication::DrawingData` on the small canvas in [`crate::pdf`]. Both
//! are A3 landscape. Nothing here interprets the design: the core decides
//! what is drawn; this module only places it on paper.
//!
//! - **Fabrication drawing:** the board outline with cutouts and every hole
//!   (at scale, with its tool's symbol), overall dimensions, the drill
//!   table, the stackup table, the notes and a title block.
//! - **Assembly drawing:** one page per side that has parts. Pads in light
//!   grey, part bodies, designators and polarity (pin 1) dots in red. The
//!   bottom page is viewed from below, so it is mirrored left to right.
use crate::Result;
use crate::pdf::{A3_LANDSCAPE, Align, Colour, Document, Font, Page, Stroke, text_width};
use sparksmith_core::Layer;
use sparksmith_core::fabrication::drawing::{AssemblySide, DrawingData};
use sparksmith_core::fabrication::{DrillTool, HoleFeature};

type P = [f64; 2];
/// Draws the symbol of a table row at a paper point.
type SymbolFn = dyn Fn(&mut Page, P, usize);

const MARGIN: f64 = 10.0;
/// Rows of one drawn table before the rest is summarised.
const MAX_TABLE_ROWS: usize = 40;

const INK: Colour = Colour::BLACK;
const FAINT: Colour = Colour::Grey(0.55);
const PAD: Colour = Colour::Grey(0.82);
const POLARITY: Colour = Colour::Rgb(0.85, 0.1, 0.1);

fn thin() -> Stroke {
    Stroke::new(0.18, INK)
}
fn hair() -> Stroke {
    Stroke::new(0.1, INK)
}

/// Board millimetres to paper: scale about the board's lower-left corner,
/// mirrored left to right when `mirror`.
#[derive(Clone, Copy)]
struct View {
    scale: f64,
    origin: P,
    board_min: P,
    board_max: P,
    mirror: bool,
}

impl View {
    /// Fit `[min, max]` into the paper box `[lo, hi]`, at the largest of
    /// the usual drawing scales that fits.
    fn fit(min: P, max: P, lo: P, hi: P, mirror: bool) -> Self {
        let w = (max[0] - min[0]).max(1e-3);
        let h = (max[1] - min[1]).max(1e-3);
        let fit = ((hi[0] - lo[0]) / w).min((hi[1] - lo[1]) / h);
        let scales = [
            20.0, 10.0, 8.0, 5.0, 4.0, 3.0, 2.5, 2.0, 1.5, 1.0, 0.75, 0.5, 0.4, 0.25, 0.2, 0.1,
            0.05,
        ];
        let scale = scales
            .iter()
            .copied()
            .find(|s| *s <= fit)
            .unwrap_or(fit.max(1e-3));
        // Centred in the box.
        let origin = [
            lo[0] + ((hi[0] - lo[0]) - w * scale) / 2.0,
            lo[1] + ((hi[1] - lo[1]) - h * scale) / 2.0,
        ];
        Self {
            scale,
            origin,
            board_min: min,
            board_max: max,
            mirror,
        }
    }

    fn map(&self, p: P) -> P {
        let x = if self.mirror {
            self.board_max[0] - p[0]
        } else {
            p[0] - self.board_min[0]
        };
        [
            self.origin[0] + x * self.scale,
            self.origin[1] + (p[1] - self.board_min[1]) * self.scale,
        ]
    }

    fn map_all(&self, points: &[P]) -> Vec<P> {
        points.iter().map(|p| self.map(*p)).collect()
    }

    /// The paper box of the board.
    fn paper(&self) -> (P, P) {
        let a = self.map(self.board_min);
        let b = self.map(self.board_max);
        (
            [a[0].min(b[0]), a[1].min(b[1])],
            [a[0].max(b[0]), a[1].max(b[1])],
        )
    }

    fn label(&self) -> String {
        if self.scale >= 1.0 {
            format!("Scale {}:1", trim(self.scale))
        } else {
            format!("Scale 1:{}", trim(1.0 / self.scale))
        }
    }
}

fn trim(v: f64) -> String {
    let s = format!("{v:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

fn mm2(v: f64) -> String {
    format!("{v:.2}")
}
fn mm3(v: f64) -> String {
    format!("{v:.3}")
}

/// The page frame and inner border.
fn frame(page: &mut Page) {
    let [w, h] = page.size();
    page.rect(
        [MARGIN, MARGIN],
        [w - 2.0 * MARGIN, h - 2.0 * MARGIN],
        &Stroke::new(0.5, INK),
    );
}

/// A title block in the lower-right corner: rows of (label, value).
fn title_block(page: &mut Page, title: &str, rows: &[(&str, String)]) {
    let [w, _] = page.size();
    let width = 150.0;
    let row = 6.0;
    let height = row * (rows.len() as f64 + 1.5);
    let x0 = w - MARGIN - width;
    let y0 = MARGIN;
    page.rect([x0, y0], [width, height], &Stroke::new(0.4, INK));
    let top = y0 + height;
    page.line(
        [x0, top - row * 1.5],
        [x0 + width, top - row * 1.5],
        &thin(),
    );
    page.text(
        [x0 + 3.0, top - row * 1.5 + 2.2],
        4.5,
        title,
        Font::Bold,
        Align::Left,
        0.0,
        INK,
    );
    for (k, (label, value)) in rows.iter().enumerate() {
        let y = top - row * 1.5 - row * (k as f64 + 1.0);
        page.line([x0, y], [x0 + width, y], &hair());
        page.text(
            [x0 + 3.0, y + 1.8],
            2.6,
            label,
            Font::Regular,
            Align::Left,
            0.0,
            FAINT,
        );
        page.text(
            [x0 + 38.0, y + 1.8],
            3.0,
            &fit_text(value, 3.0, width - 41.0),
            Font::Regular,
            Align::Left,
            0.0,
            INK,
        );
    }
    page.line([x0 + 35.0, y0], [x0 + 35.0, top - row * 1.5], &hair());
}

/// `text` cut with an ellipsis to fit `width` mm at `size`.
fn fit_text(text: &str, size: f64, width: f64) -> String {
    if text_width(text, size) <= width {
        return text.to_string();
    }
    let mut out: String = String::new();
    for c in text.chars() {
        let candidate = format!("{out}{c}…");
        if text_width(&candidate, size) > width {
            break;
        }
        out.push(c);
    }
    format!("{out}…")
}

/// Words of `text` wrapped to `width` mm at `size`.
fn wrap(text: &str, size: f64, width: f64) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        let candidate = if line.is_empty() {
            word.to_string()
        } else {
            format!("{line} {word}")
        };
        if text_width(&candidate, size) > width && !line.is_empty() {
            lines.push(std::mem::take(&mut line));
            line = word.to_string();
        } else {
            line = candidate;
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// A table with a heading: columns (title, width, align), rows of cells.
/// Returns the y below it.
fn table(
    page: &mut Page,
    at: P,
    heading: &str,
    columns: &[(&str, f64, Align)],
    rows: &[Vec<String>],
    symbols: Option<&SymbolFn>,
) -> f64 {
    let size = 2.8;
    let row = 5.0;
    let width: f64 = columns.iter().map(|c| c.1).sum();
    let [x0, mut y] = at;
    page.text(
        [x0, y - 4.0],
        3.8,
        heading,
        Font::Bold,
        Align::Left,
        0.0,
        INK,
    );
    y -= 6.5;
    let shown = rows.len().min(MAX_TABLE_ROWS);
    let lines = shown + 1 + usize::from(rows.len() > shown);
    let height = row * lines as f64;
    page.rect([x0, y - height], [width, height], &thin());
    let cell = |page: &mut Page, x: f64, w: f64, y: f64, text: &str, align: Align, font: Font| {
        let tx = match align {
            Align::Left => x + 1.5,
            Align::Centre => x + w / 2.0,
            Align::Right => x + w - 1.5,
        };
        page.text(
            [tx, y - row + 1.5],
            size,
            &fit_text(text, size, w - 2.0),
            font,
            align,
            0.0,
            INK,
        );
    };
    let mut x = x0;
    for (title, w, align) in columns {
        cell(page, x, *w, y, title, *align, Font::Bold);
        x += w;
    }
    page.line([x0, y - row], [x0 + width, y - row], &thin());
    for (k, cells) in rows.iter().take(shown).enumerate() {
        let ry = y - row * (k as f64 + 1.0);
        let mut x = x0;
        for (i, (_, w, align)) in columns.iter().enumerate() {
            if let Some(text) = cells.get(i) {
                cell(page, x, *w, ry, text, *align, Font::Regular);
            }
            x += w;
        }
        if let Some(draw) = symbols {
            draw(page, [x0 + columns[0].1 / 2.0, ry - row / 2.0], k);
        }
        if k + 1 < shown {
            page.line([x0, ry - row], [x0 + width, ry - row], &hair());
        }
    }
    if rows.len() > shown {
        let ry = y - row * (shown as f64 + 1.0);
        page.line([x0, ry], [x0 + width, ry], &hair());
        cell(
            page,
            x0,
            width,
            ry,
            &format!("… {} more rows", rows.len() - shown),
            Align::Left,
            Font::Regular,
        );
    }
    let mut x = x0;
    for (_, w, _) in &columns[..columns.len().saturating_sub(1)] {
        x += w;
        page.line([x, y], [x, y - row * (shown as f64 + 1.0)], &hair());
    }
    y - height
}

/// The symbol of tool `index` (in drill-table order) at `c`, `r` mm.
fn symbol(page: &mut Page, c: P, r: f64, index: usize) {
    let s = Stroke::new(0.15, INK);
    let [x, y] = c;
    let plus = |page: &mut Page| {
        page.line([x - r, y], [x + r, y], &s);
        page.line([x, y - r], [x, y + r], &s);
    };
    let cross = |page: &mut Page| {
        let k = r * 0.75;
        page.line([x - k, y - k], [x + k, y + k], &s);
        page.line([x - k, y + k], [x + k, y - k], &s);
    };
    let square = |page: &mut Page| {
        let k = r * 0.8;
        page.rect([x - k, y - k], [2.0 * k, 2.0 * k], &s);
    };
    // The drill map's order (`fabrication::excellon::SYMBOLS`): circle,
    // cross, square, triangle, x, diamond, circle-cross, square-x.
    match index % 8 {
        0 => page.circle(c, r, &s),
        1 => plus(page),
        2 => square(page),
        3 => page.polyline(
            &[[x - r, y - r * 0.7], [x + r, y - r * 0.7], [x, y + r]],
            true,
            &s,
        ),
        4 => cross(page),
        5 => page.polyline(&[[x - r, y], [x, y - r], [x + r, y], [x, y + r]], true, &s),
        6 => {
            page.circle(c, r, &s);
            plus(page);
        }
        _ => {
            square(page);
            cross(page);
        }
    }
}

/// A dimension: extension lines from `a` and `b` (paper) to a line `off`
/// mm away along `normal`, arrows and the length `value` written there.
fn dimension(page: &mut Page, a: P, b: P, normal: P, off: f64, value: f64) {
    let s = Stroke::new(0.15, INK);
    let a2 = [a[0] + normal[0] * off, a[1] + normal[1] * off];
    let b2 = [b[0] + normal[0] * off, b[1] + normal[1] * off];
    let over = 1.5;
    page.line(a, [a2[0] + normal[0] * over, a2[1] + normal[1] * over], &s);
    page.line(b, [b2[0] + normal[0] * over, b2[1] + normal[1] * over], &s);
    page.line(a2, b2, &s);
    let d = [b2[0] - a2[0], b2[1] - a2[1]];
    let len = d[0].hypot(d[1]).max(1e-9);
    let u = [d[0] / len, d[1] / len];
    let n = [-u[1], u[0]];
    let arrow = |page: &mut Page, tip: P, dir: P| {
        let (l, w) = (2.5, 0.8);
        page.fill(
            &[vec![
                tip,
                [
                    tip[0] + dir[0] * l + n[0] * w,
                    tip[1] + dir[1] * l + n[1] * w,
                ],
                [
                    tip[0] + dir[0] * l - n[0] * w,
                    tip[1] + dir[1] * l - n[1] * w,
                ],
            ]],
            INK,
            false,
        );
    };
    arrow(page, a2, u);
    arrow(page, b2, [-u[0], -u[1]]);
    let mid = [(a2[0] + b2[0]) / 2.0, (a2[1] + b2[1]) / 2.0];
    let angle = u[1].atan2(u[0]).to_degrees();
    let text_at = [
        mid[0] + normal[0] * 1.2 - n[0] * 0.0,
        mid[1] + normal[1] * 1.2,
    ];
    // Text sits on the side of the line away from the board.
    let shift = if normal[1] < -0.5 || normal[0] < -0.5 {
        3.2
    } else {
        0.0
    };
    page.text(
        [
            text_at[0] + normal[0] * shift,
            text_at[1] + normal[1] * shift,
        ],
        3.2,
        &format!("{} mm", mm2(value)),
        Font::Regular,
        Align::Centre,
        angle,
        INK,
    );
}

fn tool_label(tool: &DrillTool) -> String {
    format!("T{}", tool.number)
}

/// The fabrication drawing: outline with dimensions, drill table, stackup
/// table and notes.
pub fn fabrication_pdf(data: &DrawingData) -> Result<Vec<u8>> {
    let mut page = Page::new(A3_LANDSCAPE);
    frame(&mut page);
    let [w, h] = page.size();
    // Right column for the tables; the board view takes the rest.
    let column = 150.0;
    let col_x = w - MARGIN - column - 4.0;
    let view = View::fit(
        data.bounds.min,
        data.bounds.max,
        [MARGIN + 28.0, MARGIN + 30.0],
        [col_x - 12.0, h - MARGIN - 22.0],
        false,
    );
    page.text(
        [MARGIN + 6.0, h - MARGIN - 9.0],
        5.0,
        &format!("{} — fabrication drawing", data.title),
        Font::Bold,
        Align::Left,
        0.0,
        INK,
    );
    page.text(
        [MARGIN + 6.0, h - MARGIN - 15.0],
        3.0,
        &format!(
            "Viewed from the top. {}. All dimensions in mm.",
            view.label()
        ),
        Font::Regular,
        Align::Left,
        0.0,
        FAINT,
    );
    // Outline and cutouts.
    page.polyline(&view.map_all(&data.outline), true, &Stroke::new(0.35, INK));
    for cutout in &data.cutouts {
        page.polyline(&view.map_all(cutout), true, &Stroke::new(0.3, INK));
    }
    // Holes at scale, each with its tool's symbol.
    let index_of = |hole: &sparksmith_core::fabrication::DrillHole| {
        data.drills
            .tool_of(hole)
            .and_then(|t| data.drills.tools.iter().position(|u| u == t))
            .unwrap_or(0)
    };
    let sym_r = (0.9f64).min(view.scale * 0.5).max(0.35);
    for hole in &data.holes {
        let k = index_of(hole);
        match hole.feature {
            HoleFeature::Round { at, diameter } => {
                let c = view.map(at);
                page.circle(c, diameter / 2.0 * view.scale, &Stroke::new(0.08, FAINT));
                symbol(&mut page, c, sym_r, k);
            }
            HoleFeature::Slot { from, to, width } => {
                let (a, b) = (view.map(from), view.map(to));
                page.line(a, b, &Stroke::new(width * view.scale, PAD));
                page.line(a, b, &Stroke::new(0.08, FAINT));
                symbol(
                    &mut page,
                    [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0],
                    sym_r,
                    k,
                );
            }
        }
    }
    // Overall dimensions.
    let (lo, hi) = view.paper();
    let size = [
        data.bounds.max[0] - data.bounds.min[0],
        data.bounds.max[1] - data.bounds.min[1],
    ];
    dimension(&mut page, lo, [hi[0], lo[1]], [0.0, -1.0], 10.0, size[0]);
    dimension(&mut page, lo, [lo[0], hi[1]], [-1.0, 0.0], 10.0, size[1]);
    // Drill table.
    let rows: Vec<Vec<String>> = data
        .drills
        .tools
        .iter()
        .map(|t| {
            vec![
                String::new(),
                tool_label(t),
                mm3(t.diameter),
                if t.plated { "yes" } else { "no" }.into(),
                if t.slot { "slot" } else { "hole" }.into(),
                t.function.label().into(),
                t.count.to_string(),
            ]
        })
        .collect();
    let draw_symbol = |page: &mut Page, c: P, k: usize| symbol(page, c, 1.4, k);
    let total: usize = data.drills.tools.iter().map(|t| t.count).sum();
    let mut y = table(
        &mut page,
        [col_x, h - MARGIN - 4.0],
        &format!("Drill table ({total} holes, finished sizes)"),
        &[
            ("Sym", 12.0, Align::Centre),
            ("Tool", 14.0, Align::Left),
            ("Ø mm", 20.0, Align::Right),
            ("Plated", 18.0, Align::Centre),
            ("Kind", 16.0, Align::Centre),
            ("Use", 30.0, Align::Left),
            ("Count", 20.0, Align::Right),
        ],
        &rows,
        Some(&draw_symbol),
    );
    // Stackup table.
    let stack: Vec<Vec<String>> = data
        .stackup
        .iter()
        .map(|r| {
            vec![
                r.name.clone(),
                r.material.clone(),
                if r.thickness_mm > 0.0 {
                    mm3(r.thickness_mm)
                } else {
                    "—".into()
                },
            ]
        })
        .collect();
    y = table(
        &mut page,
        [col_x, y - 4.0],
        &format!(
            "Stackup ({} copper layers, {} mm over copper)",
            data.copper_layers,
            mm2(data.thickness_mm)
        ),
        &[
            ("Layer", 45.0, Align::Left),
            ("Material", 65.0, Align::Left),
            ("mm", 20.0, Align::Right),
        ],
        &stack,
        None,
    );
    // Notes, above the title block.
    page.text(
        [col_x, y - 8.0],
        3.8,
        "Notes",
        Font::Bold,
        Align::Left,
        0.0,
        INK,
    );
    let mut ny = y - 13.5;
    let floor = MARGIN + 6.0 * 7.5 + 4.0;
    'notes: for (k, note) in data.notes.iter().enumerate() {
        let prefix = format!("{}.", k + 1);
        for (i, line) in wrap(note, 2.8, column - 8.0).iter().enumerate() {
            if ny < floor {
                break 'notes;
            }
            if i == 0 {
                page.text(
                    [col_x, ny],
                    2.8,
                    &prefix,
                    Font::Regular,
                    Align::Left,
                    0.0,
                    INK,
                );
            }
            page.text(
                [col_x + 6.0, ny],
                2.8,
                line,
                Font::Regular,
                Align::Left,
                0.0,
                INK,
            );
            ny -= 4.0;
        }
    }
    title_block(
        &mut page,
        &data.title,
        &[
            ("Drawing", "Fabrication drawing, sheet 1 of 1".into()),
            ("Rules", data.profile.clone()),
            (
                "Board",
                format!(
                    "{} × {} mm, {} layers, {} mm",
                    mm2(size[0]),
                    mm2(size[1]),
                    data.copper_layers,
                    mm2(data.thickness_mm)
                ),
            ),
            ("Finish", data.finish.clone()),
            ("Mask / legend", format!("{} / {}", data.mask, data.legend)),
            ("Generator", sparksmith_core::product::NAME.into()),
        ],
    );
    let mut doc = Document::new();
    doc.title(&format!("{} fabrication drawing", data.title));
    doc.push(page)?;
    doc.finish()
}

fn bounds_of(loops: &[Vec<P>]) -> Option<(P, P)> {
    let mut it = loops
        .iter()
        .flatten()
        .filter(|p| p[0].is_finite() && p[1].is_finite());
    let first = *it.next()?;
    Some(it.fold((first, first), |(lo, hi), p| {
        (
            [lo[0].min(p[0]), lo[1].min(p[1])],
            [hi[0].max(p[0]), hi[1].max(p[1])],
        )
    }))
}

fn assembly_page(data: &DrawingData, side: &AssemblySide) -> Page {
    let mut page = Page::new(A3_LANDSCAPE);
    frame(&mut page);
    let [w, h] = page.size();
    let bottom = side.side == Layer::Bottom;
    let view = View::fit(
        data.bounds.min,
        data.bounds.max,
        [MARGIN + 10.0, MARGIN + 60.0],
        [w - MARGIN - 10.0, h - MARGIN - 22.0],
        bottom,
    );
    let heading = if bottom {
        "bottom side, viewed from below (mirrored)"
    } else {
        "top side, viewed from above"
    };
    page.text(
        [MARGIN + 6.0, h - MARGIN - 9.0],
        5.0,
        &format!("{} — assembly, {heading}", data.title),
        Font::Bold,
        Align::Left,
        0.0,
        INK,
    );
    page.text(
        [MARGIN + 6.0, h - MARGIN - 15.0],
        3.0,
        &format!(
            "{} parts. {}. Red dots mark pin 1 / polarity.",
            side.parts.len(),
            view.label()
        ),
        Font::Regular,
        Align::Left,
        0.0,
        FAINT,
    );
    page.polyline(
        &view.map_all(&data.outline),
        true,
        &Stroke::new(0.35, FAINT),
    );
    for cutout in &data.cutouts {
        page.polyline(&view.map_all(cutout), true, &Stroke::new(0.3, FAINT));
    }
    for part in &side.parts {
        let pads: Vec<Vec<P>> = part.pads.iter().map(|l| view.map_all(l)).collect();
        page.fill(&pads, PAD, false);
    }
    for part in &side.parts {
        for body in &part.body {
            page.polyline(&view.map_all(body), false, &Stroke::new(0.2, INK));
        }
    }
    for part in &side.parts {
        let body: Vec<Vec<P>> = part.body.iter().map(|l| view.map_all(l)).collect();
        let pads: Vec<Vec<P>> = part.pads.iter().map(|l| view.map_all(l)).collect();
        let (lo, hi) = bounds_of(&body)
            .or_else(|| bounds_of(&pads))
            .unwrap_or_else(|| {
                let c = view.map(part.at);
                (c, c)
            });
        let (bw, bh) = (hi[0] - lo[0], hi[1] - lo[1]);
        let tall = bh > bw * 1.2;
        let (long, short) = if tall { (bh, bw) } else { (bw, bh) };
        let unit = text_width(&part.designator, 1.0).max(0.3);
        let size = (long * 0.8 / unit).min(short * 0.6).clamp(1.2, 5.0);
        let c = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0];
        let angle = if tall { 90.0 } else { 0.0 };
        let off = size * 0.35;
        let at = if tall {
            [c[0] + off, c[1]]
        } else {
            [c[0], c[1] - off]
        };
        page.text(
            at,
            size,
            &part.designator,
            Font::Bold,
            Align::Centre,
            angle,
            INK,
        );
        if let Some(p) = part.polarity {
            page.disc(view.map(p), (0.25 * view.scale).clamp(0.4, 1.0), POLARITY);
        }
    }
    title_block(
        &mut page,
        &data.title,
        &[
            (
                "Drawing",
                format!(
                    "Assembly drawing, {} side",
                    if bottom { "bottom" } else { "top" }
                ),
            ),
            ("View", heading.into()),
            ("Parts", side.parts.len().to_string()),
            ("Generator", sparksmith_core::product::NAME.into()),
        ],
    );
    page
}

/// The assembly drawing: one page per populated side with designators and
/// polarity marks. A board without parts gives one page with its outline.
pub fn assembly_pdf(data: &DrawingData) -> Result<Vec<u8>> {
    let mut doc = Document::new();
    doc.title(&format!("{} assembly drawing", data.title));
    if data.sides.is_empty() {
        doc.push(assembly_page(
            data,
            &AssemblySide {
                side: Layer::Top,
                parts: Vec::new(),
            },
        ))?;
    }
    for side in &data.sides {
        doc.push(assembly_page(data, side))?;
    }
    doc.finish()
}

/// The pages of both drawings, for tests that read their text.
#[cfg(test)]
fn assembly_pages(data: &DrawingData) -> Vec<Page> {
    data.sides.iter().map(|s| assembly_page(data, s)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use sparksmith_core::Engine;
    use sparksmith_core::fabrication::{FabricationOptions, drawing};

    fn showcase() -> Result<DrawingData> {
        let engine = Engine::showcase()?;
        Ok(drawing::data(
            engine.project(),
            &FabricationOptions::default(),
            engine.check_settings(),
        )?)
    }

    /// The ESP32 dev board's BOM keeps every rating its parts were
    /// specified with: C1 is "10 µF 16 V", not "10 µF".
    #[test]
    fn the_esp32_bom_value_keeps_the_voltage_rating() -> Result<()> {
        let path = std::path::Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../examples/esp32-s31-devboard/esp32-s31-devboard.sparksmith"
        ));
        let project = crate::read_project(path)?.project;
        let bom = sparksmith_core::fabrication::assembly::bom_parts_csv(&project)?;
        let row = |designator: &str| {
            bom.lines()
                .find(|l| l.starts_with(&format!("{designator},")))
                .map(str::to_owned)
        };
        assert!(
            row("C1").is_some_and(|l| l.starts_with("C1,10 µF 16 V,")),
            "{bom}"
        );
        assert!(
            row("C3").is_some_and(|l| l.starts_with("C3,100 nF 16 V,")),
            "{bom}"
        );
        Ok(())
    }

    #[test]
    fn the_showcase_drawings_are_deterministic_pdfs() -> Result<()> {
        let data = showcase()?;
        let fab = fabrication_pdf(&data)?;
        let asm = assembly_pdf(&data)?;
        assert_eq!(fab, fabrication_pdf(&data)?);
        assert_eq!(asm, assembly_pdf(&data)?);
        for pdf in [&fab, &asm] {
            assert!(pdf.starts_with(b"%PDF-"));
            let tail = String::from_utf8_lossy(&pdf[pdf.len().saturating_sub(16)..]).to_string();
            assert!(tail.trim_end().ends_with("%%EOF"));
            assert!(pdf.len() < 4 * 1024 * 1024, "{}", pdf.len());
        }
        let fab_text = String::from_utf8_lossy(&fab);
        assert_eq!(fab_text.matches("/Parent").count(), 1);
        assert!(fab_text.contains("Drill table"));
        assert!(fab_text.contains("Stackup"));
        // Two sides: the showcase has a bottom resistor.
        let asm_text = String::from_utf8_lossy(&asm);
        assert_eq!(asm_text.matches("/Parent").count(), 2);
        for part in data.sides.iter().flat_map(|s| &s.parts) {
            assert!(
                asm_text.contains(&format!("({}) Tj", part.designator)),
                "{}",
                part.designator
            );
        }
        let pages = assembly_pages(&data);
        assert!(pages[1].texts().iter().any(|t| t.contains("mirrored")));
        Ok(())
    }

    #[test]
    fn an_empty_board_still_draws() -> Result<()> {
        let engine = Engine::default();
        let data = drawing::data(
            engine.project(),
            &FabricationOptions::default(),
            engine.check_settings(),
        )?;
        assert!(fabrication_pdf(&data)?.starts_with(b"%PDF-"));
        let asm = assembly_pdf(&data)?;
        assert_eq!(String::from_utf8_lossy(&asm).matches("/Parent").count(), 1);
        Ok(())
    }

    /// Writes the showcase drawings for visual review when
    /// `SPARKSMITH_DRAWINGS_DIR` is set.
    #[test]
    fn write_samples_when_asked() -> Result<()> {
        let Ok(dir) = std::env::var("SPARKSMITH_DRAWINGS_DIR") else {
            return Ok(());
        };
        let data = showcase()?;
        std::fs::create_dir_all(&dir)?;
        std::fs::write(format!("{dir}/fabrication.pdf"), fabrication_pdf(&data)?)?;
        std::fs::write(format!("{dir}/assembly.pdf"), assembly_pdf(&data)?)?;
        Ok(())
    }
}
