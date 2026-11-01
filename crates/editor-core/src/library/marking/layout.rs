//! The marking layout: where each item of a spec sits in the marking area.
//!
//! Text lines (templates and codes) stack top to bottom in item order,
//! centred vertically in the area minus its margin and minus the room
//! polarity marks take at the ends. Lines without an explicit height share
//! one auto-fitted height, at most [`MAX_AUTO_TEXT_HEIGHT_MM`]. A line that
//! does not fit at the minimum legible height is dropped from the end with
//! a `marking.does_not_fit` finding; text never shrinks below the minimum.
use super::codes;
use super::template::{self, Missing};
use super::{
    AREA_MARGIN, Align, Anchor, LINE_GAP, LaidElement, LaidText, MAX_RENDERED_LINE, MarkingInputs,
    MarkingItem, MarkingLayout, MarkingSpec, PolarityMark, TEXT_ADVANCE, positive,
};
use crate::diagnostics::{Error, Finding, Result};
use crate::quantity::format_number;

/// The tallest auto-fitted text, mm: typical package top marks are 0.5–1.2
/// mm tall (an estimate, data rather than a rule), so large bodies do not
/// get poster-sized marks.
pub const MAX_AUTO_TEXT_HEIGHT_MM: f64 = 1.2;

struct Line {
    text: String,
    height: Option<f64>,
    align: Align,
}

struct Context<'a> {
    inputs: &'a MarkingInputs,
    findings: Vec<Finding>,
    area: [f64; 2],
    min_height: f64,
}

impl Context<'_> {
    fn note(&mut self, code: &str, message: String) {
        let message = if self.inputs.designator.is_empty() {
            format!("{message}; nothing is drawn.")
        } else {
            format!("{}: {message}; nothing is drawn.", self.inputs.designator)
        };
        self.findings.push(Finding::new(code, message, Vec::new()));
    }
    fn unrepresentable(&mut self, message: String) {
        self.note("marking.unrepresentable", message);
    }
    fn does_not_fit(&mut self, what: &str) {
        let [w, h] = self.area;
        let message = format!(
            "{what} does not fit the {} × {} mm marking area at {} mm",
            format_number(w),
            format_number(h),
            format_number(self.min_height)
        );
        self.note("marking.does_not_fit", message);
    }
}

fn width(text: &str, height: f64) -> f64 {
    text.chars().count() as f64 * TEXT_ADVANCE * height
}

/// The flow rectangle: [x0, x1, y0, y1].
type Rect = [f64; 4];

/// A polarity mark's size and centre.
fn polarity(mark: PolarityMark, at: Anchor, area: [f64; 2], margin: f64) -> ([f64; 2], [f64; 2]) {
    let [w, h] = area;
    let size = match mark {
        PolarityMark::Dot => {
            let d = 0.2 * w.min(h);
            [d, d]
        }
        PolarityMark::Bar => [0.1 * w, h - 2.0 * margin],
        PolarityMark::Band => [0.15 * w, h],
    };
    // A band sits on the body edge; the others keep the margin.
    let inset = if mark == PolarityMark::Band {
        0.0
    } else {
        margin
    };
    let start = -w / 2.0 + inset + size[0] / 2.0;
    let centre = match (mark, at) {
        (_, Anchor::Centre) => [0.0, 0.0],
        (PolarityMark::Dot, Anchor::Pin1Corner) => [start, h / 2.0 - margin - size[1] / 2.0],
        (_, Anchor::Pin1Corner | Anchor::Start) => [start, 0.0],
        (_, Anchor::End) => [-start, 0.0],
    };
    (size, centre)
}

fn anchored(at: Anchor, size: [f64; 2], area: [f64; 2], margin: f64) -> [f64; 2] {
    let x = -area[0] / 2.0 + margin + size[0] / 2.0;
    match at {
        Anchor::Centre => [0.0, 0.0],
        Anchor::Pin1Corner => [x, area[1] / 2.0 - margin - size[1] / 2.0],
        Anchor::Start => [x, 0.0],
        Anchor::End => [-x, 0.0],
    }
}

/// The lines a text item renders, or nothing with a finding.
fn text_lines(
    cx: &mut Context<'_>,
    template: &str,
    height: Option<f64>,
    align: Align,
) -> Vec<Line> {
    let rendered = template::parse(template)
        .map_err(|problem| Missing::Value(format!("the template \"{template}\" has {problem}")))
        .and_then(|pieces| template::render(&pieces, cx.inputs));
    match rendered {
        Ok(text) => text
            .split('\n')
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(|line| Line {
                text: line.to_string(),
                height,
                align,
            })
            .collect(),
        Err(Missing::Value(message) | Missing::Code(message)) => {
            cx.unrepresentable(message);
            Vec::new()
        }
    }
}

/// The code line of a `Code` item: the first candidate that represents the
/// value and fits `rect` at the minimum height.
fn code_line(
    cx: &mut Context<'_>,
    scheme: &codes::CodeScheme,
    field: crate::ids::DefinitionId,
    rect: Rect,
) -> Option<Line> {
    let Some((text, si)) = cx.inputs.fields.get(&field).cloned() else {
        cx.unrepresentable(format!(
            "field {field} has no value for its {} code",
            scheme.label()
        ));
        return None;
    };
    let Some(value) = si else {
        cx.unrepresentable(format!("{text} is not a number for a {}", scheme.label()));
        return None;
    };
    if let Some(problem) = scheme.problem() {
        cx.unrepresentable(problem);
        return None;
    }
    let (fit_w, fit_h) = (rect[1] - rect[0], rect[3] - rect[2]);
    let mut representable = None;
    for candidate in scheme.candidates() {
        let code = match codes::encode(candidate, value) {
            Ok(Some(code)) => code,
            Ok(None) => continue,
            Err(error) => {
                cx.unrepresentable(error.to_string());
                return None;
            }
        };
        if width(&code, cx.min_height) <= fit_w && cx.min_height <= fit_h {
            return Some(Line {
                text: code,
                height: None,
                align: Align::Centre,
            });
        }
        representable.get_or_insert(code);
    }
    match representable {
        Some(code) => cx.does_not_fit(&format!("The code \"{code}\"")),
        None => cx.unrepresentable(format!("No {} code for {text}", scheme.label())),
    }
    None
}

/// Drop lines that cannot fit, then return the shared auto height.
fn fit(cx: &mut Context<'_>, lines: &mut Vec<Line>, rect: Rect) -> f64 {
    let (w, h) = (rect[1] - rect[0], rect[3] - rect[2]);
    let gap = LINE_GAP * cx.min_height;
    loop {
        if let Some(index) = lines.iter().position(|l| {
            l.height
                .is_some_and(|lh| lh < cx.min_height || lh > h || width(&l.text, lh) > w)
                || l.text.len() > MAX_RENDERED_LINE
        }) {
            let line = lines.remove(index);
            cx.does_not_fit(&format!("\"{}\"", line.text));
            continue;
        }
        let Some(last) = lines.last() else {
            return 0.0;
        };
        let count = lines.len() as f64;
        let explicit: f64 = lines.iter().filter_map(|l| l.height).sum();
        let auto: Vec<&Line> = lines.iter().filter(|l| l.height.is_none()).collect();
        let free = h - explicit - (count - 1.0) * gap;
        let auto_height = if auto.is_empty() {
            (free >= 0.0).then_some(0.0)
        } else {
            let by_height = free / auto.len() as f64;
            let by_width = auto
                .iter()
                .map(|l| w / (l.text.chars().count().max(1) as f64 * TEXT_ADVANCE))
                .fold(f64::INFINITY, f64::min);
            let height = by_height
                .min(by_width)
                .min(MAX_AUTO_TEXT_HEIGHT_MM.max(cx.min_height));
            (height >= cx.min_height).then_some(height)
        };
        match auto_height {
            Some(height) => return height,
            None => {
                let text = last.text.clone();
                lines.pop();
                cx.does_not_fit(&format!("\"{text}\""));
            }
        }
    }
}

pub(super) fn run(spec: &MarkingSpec, inputs: &MarkingInputs) -> Result<MarkingLayout> {
    let min_height = spec
        .min_text_height_mm
        .unwrap_or(super::DEFAULT_MIN_TEXT_HEIGHT_MM);
    let explicit_heights = spec.items.iter().all(|item| match item {
        MarkingItem::Text { height_mm, .. } => height_mm.is_none_or(f64::is_finite),
        MarkingItem::Artwork { size_mm, .. } => size_mm.iter().all(|s| s.is_finite()),
        _ => true,
    });
    if !min_height.is_finite() || !explicit_heights || !inputs.area_mm.iter().all(|a| a.is_finite())
    {
        return Err(Error::Invalid(
            "marking.template: marking sizes must be finite".into(),
        ));
    }
    let mut cx = Context {
        inputs,
        findings: Vec::new(),
        area: inputs.area_mm,
        min_height,
    };
    let mut out = MarkingLayout {
        elements: Vec::new(),
        finish: spec.finish,
        findings: Vec::new(),
    };
    if spec.items.is_empty() {
        return Ok(out);
    }
    if !inputs.area_mm.iter().all(|a| positive(*a)) || !positive(min_height) {
        cx.unrepresentable("the package has no marking area".into());
        out.findings = cx.findings;
        return Ok(out);
    }
    let area = inputs.area_mm;
    let [w, h] = area;
    let margin = AREA_MARGIN * w.min(h);
    let mut rect: Rect = [
        -w / 2.0 + margin,
        w / 2.0 - margin,
        -h / 2.0 + margin,
        h / 2.0 - margin,
    ];

    // Marks that take room first: polarity at the ends.
    for item in &spec.items {
        if let MarkingItem::Polarity { mark, at } = *item {
            let (size, origin) = polarity(mark, at, area, margin);
            let shrinks =
                at != Anchor::Centre && !(mark == PolarityMark::Dot && at == Anchor::Pin1Corner);
            if shrinks {
                if at == Anchor::End {
                    rect[1] = rect[1].min(origin[0] - size[0] / 2.0 - margin);
                } else {
                    rect[0] = rect[0].max(origin[0] + size[0] / 2.0 + margin);
                }
            }
            out.elements.push(LaidElement::Polarity {
                mark,
                origin,
                size_mm: size,
            });
        }
    }

    let mut lines = Vec::new();
    for item in &spec.items {
        match item {
            MarkingItem::Polarity { .. } => {}
            MarkingItem::Artwork {
                svg,
                size_mm,
                anchor,
                ..
            } => {
                if size_mm[0] > w - 2.0 * margin || size_mm[1] > h - 2.0 * margin {
                    cx.does_not_fit("The artwork");
                } else {
                    out.elements.push(LaidElement::Artwork {
                        svg: *svg,
                        origin: anchored(*anchor, *size_mm, area, margin),
                        size_mm: *size_mm,
                    });
                }
            }
            MarkingItem::Bands { value, tolerance } => {
                let value_si = inputs.fields.get(&value.id).and_then(|(_, si)| *si);
                let tolerance_si =
                    tolerance.map(|t| inputs.fields.get(&t.id).and_then(|(_, si)| *si));
                let shown = inputs
                    .fields
                    .get(&value.id)
                    .map_or_else(|| "the value".to_string(), |(text, _)| text.clone());
                let colours = match (value_si, tolerance_si) {
                    (None, _) => None,
                    (Some(_), Some(None)) => None,
                    (Some(v), t) => codes::bands(v, t.flatten())?,
                };
                match colours {
                    Some(colours) => {
                        let pitch = (w - 2.0 * margin) / (colours.len() as f64 + 1.0);
                        out.elements.push(LaidElement::Bands {
                            colours,
                            pitch_mm: pitch,
                        });
                    }
                    None => cx.unrepresentable(format!("No colour bands for {shown}")),
                }
            }
            MarkingItem::Text {
                template,
                height_mm,
                align,
            } => lines.extend(text_lines(&mut cx, &template.0, *height_mm, *align)),
            MarkingItem::Code { scheme, field } => {
                if let Some(line) = code_line(&mut cx, scheme, field.id, rect) {
                    lines.push(line);
                }
            }
        }
    }

    let auto_height = fit(&mut cx, &mut lines, rect);
    let gap = LINE_GAP * min_height;
    let heights: Vec<f64> = lines
        .iter()
        .map(|l| l.height.unwrap_or(auto_height))
        .collect();
    let stack = heights.iter().sum::<f64>() + gap * (heights.len().saturating_sub(1) as f64);
    let mut top = (rect[2] + rect[3]) / 2.0 + stack / 2.0;
    for (line, height) in lines.into_iter().zip(heights) {
        let line_width = width(&line.text, height);
        let x = match line.align {
            Align::Centre => (rect[0] + rect[1]) / 2.0 - line_width / 2.0,
            Align::Start => rect[0],
            Align::End => rect[1] - line_width,
        };
        let y = top - height;
        out.elements.push(LaidElement::Text(LaidText {
            text: line.text,
            height_mm: height,
            origin: [x, y],
        }));
        top = y - gap;
    }
    out.findings = cx.findings;
    Ok(out)
}
