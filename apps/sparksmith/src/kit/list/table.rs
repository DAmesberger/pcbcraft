//! Tables on the list row: column captions with sort, per-column priority,
//! minimum width and alignment with at most one flex column
//! ([`super::columns`]). Columns that do not fit leave the row; their
//! values show in the row's hover, and the header says "+n". Numbers are
//! tabular mono, right-aligned and decimal-aligned per column.
use super::columns::{self, Column, Fit};
use super::measure::{self, Slots};
use super::model::RowKey;
use super::row::{self, RowResponse};
use super::state::Sort;
use super::tokens::ListTokens;
use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, FontId, Rect, Sense, Stroke, Ui, pos2, vec2};

/// One column of a table.
#[derive(Clone, Debug, PartialEq)]
pub struct ColumnSpec<'a> {
    pub caption: &'a str,
    pub column: Column,
    /// Numbers: right-aligned on the decimal point.
    pub numeric: bool,
}

impl<'a> ColumnSpec<'a> {
    pub fn text(caption: &'a str, min: f32, priority: u8) -> Self {
        Self {
            caption,
            column: Column::new(min, min, priority),
            numeric: false,
        }
    }

    pub fn number(caption: &'a str, min: f32, priority: u8) -> Self {
        Self {
            numeric: true,
            ..Self::text(caption, min, priority)
        }
    }

    pub fn flex(mut self) -> Self {
        self.column = self.column.flex();
        self
    }
}

/// One cell of a table row.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Cell<'a> {
    Empty,
    Text(&'a str),
    /// The row's name (a designator): full ink.
    Strong(&'a str),
    Dim(&'a str),
    /// A number with its unit, decimal-aligned.
    Number(&'a str),
    /// A board side or layer: its colour swatch and name (C's identity).
    Layer(Color32, &'a str),
    /// A net: its colour dot and name.
    Net(Color32, &'a str),
}

impl Cell<'_> {
    fn text(&self) -> &str {
        match self {
            Cell::Empty => "",
            Cell::Text(t)
            | Cell::Strong(t)
            | Cell::Dim(t)
            | Cell::Number(t)
            | Cell::Layer(_, t)
            | Cell::Net(_, t) => t,
        }
    }
}

/// A table's fitted layout for one width, measured over every row.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TableLayout {
    pub fit: Fit,
    pub lefts: Vec<Option<f32>>,
    /// Decimal slots per numeric column.
    pub slots: Vec<Slots>,
}

/// Fit the columns into `width`, with each column's ideal width measured
/// over `rows` (every row, not only the shown ones) so values are never cut.
pub fn layout<'r>(
    ui: &Ui,
    specs: &[ColumnSpec<'_>],
    width: f32,
    rows: impl Iterator<Item = &'r [Cell<'r>]>,
) -> TableLayout {
    let l = ListTokens::get(ui.ctx());
    let font = FontId::proportional(l.fs);
    let caption = FontId::proportional(l.fs_xs);
    let mut ideal: Vec<f32> = specs
        .iter()
        .map(|s| measure::widest(ui, &caption, [s.caption]) + 12.0)
        .collect();
    let mut numbers: Vec<Vec<&str>> = vec![Vec::new(); specs.len()];
    // Each distinct text of a column is laid out once: 10 000 vias share
    // a few dozen net names.
    let mut seen: std::collections::HashSet<(usize, bool, &str)> = Default::default();
    for cells in rows {
        for (i, cell) in cells.iter().enumerate() {
            let (Some(spec), Some(w)) = (specs.get(i), ideal.get_mut(i)) else {
                continue;
            };
            if spec.numeric {
                if let (Cell::Number(t), Some(n)) = (cell, numbers.get_mut(i)) {
                    n.push(t);
                    continue;
                }
                // Text in a number column ("not placed") is measured as text.
                if cell.text().is_empty() {
                    continue;
                }
            }
            let swatch = matches!(cell, Cell::Layer(..) | Cell::Net(..));
            if !seen.insert((i, swatch, cell.text())) {
                continue;
            }
            let text_w = measure::widest(ui, &font, [cell.text()]);
            *w = w.max(text_w + if swatch { l.swatch + l.gap } else { 0.0 });
        }
    }
    let slots: Vec<Slots> = numbers
        .iter()
        .map(|n| measure::slots(ui, &l, n.iter().copied()))
        .collect();
    for ((w, s), spec) in ideal.iter_mut().zip(&slots).zip(specs) {
        if spec.numeric {
            *w = w.max(s.width(l.gap * 0.5));
        }
    }
    let cols: Vec<Column> = specs
        .iter()
        .zip(&ideal)
        .map(|(s, w)| Column {
            ideal: w.ceil().max(s.column.min),
            ..s.column
        })
        .collect();
    let fit = columns::fit(&cols, (width - 2.0 * l.pad_x).max(0.0), l.column_gap);
    let lefts = fit.lefts(l.column_gap);
    TableLayout { fit, lefts, slots }
}

impl TableLayout {
    /// The rect of column `i` in a row `rect`, when shown.
    pub fn cell(&self, rect: Rect, i: usize, pad_x: f32) -> Option<Rect> {
        let left = (*self.lefts.get(i)?)?;
        let width = (*self.fit.widths.get(i)?)?;
        Some(Rect::from_min_size(
            pos2(rect.left() + pad_x + left, rect.top()),
            vec2(width, rect.height()),
        ))
    }
}

/// The caption row: captions (a click sorts: ascending, then descending),
/// the sort arrow and "+n" for columns that left the row. Returns the
/// column clicked.
pub fn header(
    ui: &mut Ui,
    id: egui::Id,
    specs: &[ColumnSpec<'_>],
    layout: &TableLayout,
    sort: Option<Sort>,
) -> Option<usize> {
    let l = ListTokens::get(ui.ctx());
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), l.header_h), Sense::hover());
    let painter = ui.painter_at(rect);
    painter.line_segment(
        [
            pos2(rect.left(), rect.bottom() - 0.5),
            pos2(rect.right(), rect.bottom() - 0.5),
        ],
        Stroke::new(1.0, l.line),
    );
    let mut clicked = None;
    let font = FontId::proportional(l.fs_xs);
    for (i, spec) in specs.iter().enumerate() {
        let Some(cell) = layout.cell(rect, i, l.pad_x) else {
            continue;
        };
        let response = ui.interact(cell, id.with(("caption", i)), Sense::click());
        let sorted = sort.filter(|s| s.column == i);
        let ink = if response.hovered() || sorted.is_some() {
            l.fg
        } else {
            l.fg_dim
        };
        let mut job = LayoutJob::default();
        job.append(spec.caption, 0.0, TextFormat::simple(font.clone(), ink));
        if let Some(s) = sorted {
            let arrow = if s.ascending {
                " \u{25b2}"
            } else {
                " \u{25bc}"
            };
            job.append(
                arrow,
                0.0,
                TextFormat::simple(FontId::proportional(8.0), l.focus),
            );
        }
        job.wrap.max_width = cell.width();
        job.wrap.max_rows = 1;
        job.wrap.overflow_character = Some(super::super::text::ELLIPSIS);
        let g = painter.layout_job(job);
        let x = if spec.numeric {
            cell.right() - g.size().x
        } else {
            cell.left()
        };
        painter.galley(pos2(x, cell.center().y - g.size().y / 2.0), g, ink);
        if response.has_focus() {
            painter.rect_stroke(
                cell,
                2.0,
                Stroke::new(1.0, l.focus),
                egui::StrokeKind::Inside,
            );
        }
        let label = match sorted {
            Some(s) if s.ascending => format!("{}, sorted ascending", spec.caption),
            Some(_) => format!("{}, sorted descending", spec.caption),
            None => spec.caption.to_owned(),
        };
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::ColumnHeader, true, &label));
        if response
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .clicked()
        {
            clicked = Some(i);
        }
    }
    if !layout.fit.hidden.is_empty() {
        let names: Vec<&str> = layout
            .fit
            .hidden
            .iter()
            .filter_map(|i| specs.get(*i).map(|s| s.caption))
            .collect();
        let text = format!("+{}", names.len());
        let g = painter.layout_no_wrap(text.clone(), FontId::monospace(l.fs_xs), l.fg_faint);
        let at = Rect::from_min_size(
            pos2(
                rect.right() - l.pad_x - g.size().x,
                rect.center().y - g.size().y / 2.0,
            ),
            g.size(),
        );
        painter.galley(at.min, g, l.fg_faint);
        let more = format!("{} more columns: {}", names.len(), names.join(", "));
        ui.interact(at, id.with("more-columns"), Sense::hover())
            .on_hover_text(&more)
            .widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, &more));
    }
    clicked
}

/// A table row: the band, then each shown cell; the cells of columns that
/// left the row show on the row's hover.
#[allow(clippy::too_many_arguments)]
pub fn row(
    ui: &mut Ui,
    id: egui::Id,
    rect: Rect,
    key: RowKey,
    specs: &[ColumnSpec<'_>],
    layout: &TableLayout,
    cells: &[Cell<'_>],
    selected: bool,
    highlighted: bool,
    cursor: bool,
    index: usize,
) -> RowResponse {
    let l = ListTokens::get(ui.ctx());
    let id = id.with(key);
    let response = ui.interact(rect, id, Sense::click());
    let hovered = ui.is_enabled() && ui.rect_contains_pointer(rect);
    row::band(ui, &l, rect, id, selected, highlighted, cursor, 0.0);
    let painter = ui.painter_at(rect);
    let cy = rect.center().y;
    let font = FontId::proportional(l.fs);
    // Shown cells cut short: their whole text goes in the row's hover.
    let mut cut: Vec<String> = Vec::new();
    for (i, cell) in cells.iter().enumerate() {
        let (Some(at), Some(spec)) = (layout.cell(rect, i, l.pad_x), specs.get(i)) else {
            continue;
        };
        let line = |text: &str, font: FontId, ink: Color32, width: f32| {
            let mut job = LayoutJob::simple_singleline(text.to_owned(), font, ink);
            job.wrap.max_width = width.max(1.0);
            job.wrap.max_rows = 1;
            job.wrap.break_anywhere = true;
            job.wrap.overflow_character = Some(super::super::text::ELLIPSIS);
            painter.layout_job(job)
        };
        match cell {
            Cell::Empty => {}
            Cell::Text(t) | Cell::Strong(t) | Cell::Dim(t) => {
                let ink = match cell {
                    Cell::Dim(_) => l.fg_dim,
                    _ => l.fg,
                };
                let g = line(t, font.clone(), ink, at.width());
                if g.elided {
                    cut.push(format!("{}: {t}", spec.caption));
                }
                let x = if spec.numeric {
                    at.right() - g.size().x
                } else {
                    at.left()
                };
                painter.galley(pos2(x, cy - g.size().y / 2.0), g, ink);
            }
            Cell::Number(t) => {
                let slots = layout.slots.get(i).copied().unwrap_or_default();
                number(&painter, &l, at, t, &slots);
            }
            Cell::Layer(color, t) | Cell::Net(color, t) => {
                let c = pos2(at.left() + l.swatch / 2.0, cy);
                if matches!(cell, Cell::Layer(..)) {
                    painter.rect_filled(
                        Rect::from_center_size(c, vec2(l.swatch - 2.0, l.swatch - 2.0)),
                        2.0,
                        *color,
                    );
                } else {
                    painter.circle_filled(c, l.swatch / 2.0, *color);
                }
                let left = at.left() + l.swatch + l.gap;
                let g = line(t, FontId::proportional(l.fs_s), l.fg_dim, at.right() - left);
                if g.elided {
                    cut.push(format!("{}: {t}", spec.caption));
                }
                painter.galley(pos2(left, cy - g.size().y / 2.0), g, l.fg_dim);
            }
        }
    }
    let label: Vec<String> = specs
        .iter()
        .zip(cells)
        .filter(|(_, c)| !c.text().is_empty())
        .map(|(s, c)| format!("{}: {}", s.caption, c.text()))
        .collect();
    let label = label.join(", ");
    response.widget_info(|| {
        let mut info = egui::WidgetInfo::selected(egui::Role::Row, true, selected, &label);
        info.value = Some(index as f64 + 1.0);
        info
    });
    let mut hidden: Vec<String> = layout
        .fit
        .hidden
        .iter()
        .filter_map(|i| {
            let (s, c) = (specs.get(*i)?, cells.get(*i)?);
            (!c.text().is_empty()).then(|| format!("{}: {}", s.caption, c.text()))
        })
        .collect();
    hidden.splice(0..0, cut);
    let response = if hidden.is_empty() {
        response
    } else {
        response.on_hover_text(hidden.join(" \u{b7} "))
    };
    RowResponse {
        clicked: response.clicked(),
        double_clicked: response.double_clicked(),
        hovered,
        fold: false,
        action: None,
        grip: None,
        value_rect: rect,
        response,
    }
}

/// A number in a cell, decimal-aligned in the column's slots.
fn number(painter: &egui::Painter, l: &ListTokens, at: Rect, text: &str, slots: &Slots) {
    let cy = at.center().y;
    let mono = measure::mono(l);
    let Some(n) = measure::split(text) else {
        let g = painter.layout_no_wrap(text.to_owned(), mono, l.fg_faint);
        painter.galley(
            pos2(at.right() - g.size().x, cy - g.size().y / 2.0),
            g,
            l.fg_faint,
        );
        return;
    };
    let unit_left = at.right() - slots.unit;
    let frac_left = unit_left - if slots.unit > 0.0 { l.gap * 0.5 } else { 0.0 } - slots.frac;
    if !n.unit.is_empty() {
        let g = painter.layout_no_wrap(n.unit.to_owned(), FontId::monospace(l.fs_s), l.fg_faint);
        painter.galley(pos2(unit_left, cy - g.size().y / 2.0), g, l.fg_faint);
    }
    if !n.frac.is_empty() {
        let g = painter.layout_no_wrap(n.frac.to_owned(), mono.clone(), l.fg);
        painter.galley(pos2(frac_left, cy - g.size().y / 2.0), g, l.fg);
    }
    let g = painter.layout_no_wrap(n.int.to_owned(), mono, l.fg);
    painter.galley(pos2(frac_left - g.size().x, cy - g.size().y / 2.0), g, l.fg);
}
