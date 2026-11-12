//! An Excellon drill-file reader for verification, from the public
//! Excellon format description: the `M48` header with `METRIC`/`INCH`,
//! `FMAT`, tool definitions `TnnC…`, then `%` (or `M95`) and the body with
//! `G90`, `G05`, tool selections, hits, `G85` slots and routed paths
//! (`G00`, `M15`, `G01`, `M16`, `M17`), ending with `M30`. Coordinates with
//! a decimal point are read as written; integers carry 3 implied decimals
//! in mm (4 in inches). Comments (`;`) are kept, so the X2-style
//! `; #@! TF.…` attribute comments can be checked.
use crate::Fallible;
use crate::Point;
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Hit {
    Round { tool: u32, at: Point },
    Slot { tool: u32, from: Point, to: Point },
}

impl Hit {
    pub fn tool(&self) -> u32 {
        match *self {
            Self::Round { tool, .. } | Self::Slot { tool, .. } => tool,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct DrillFile {
    pub metric: bool,
    /// Tool number to diameter, mm.
    pub tools: BTreeMap<u32, f64>,
    pub hits: Vec<Hit>,
    pub comments: Vec<String>,
    /// Tool number to the values of the X2-style aperture attribute
    /// comment `; #@! TA.AperFunction,…` written before its definition.
    pub tool_functions: BTreeMap<u32, Vec<String>>,
}

impl DrillFile {
    /// The values of an X2-style attribute comment `; #@! TF.<name>,…`.
    pub fn attribute(&self, name: &str) -> Option<Vec<String>> {
        self.comments.iter().find_map(|c| {
            let rest = c.trim().strip_prefix("#@!")?.trim();
            let rest = rest.strip_prefix("TF")?;
            let mut parts = rest.split(',');
            (parts.next()? == name).then(|| parts.map(str::to_string).collect())
        })
    }
}

pub fn parse(source: &str) -> Fallible<DrillFile> {
    let mut file = DrillFile::default();
    let mut unit: Option<f64> = None;
    let mut header = false;
    let mut seen_header = false;
    let mut ended = false;
    let mut tool: Option<u32> = None;
    let mut at: Point = [0.0, 0.0];
    let mut routing = false;
    let mut down = false;
    let mut function: Option<Vec<String>> = None;
    for raw in source.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(comment) = line.strip_prefix(';') {
            let comment = comment.trim();
            if let Some(rest) = comment
                .strip_prefix("#@!")
                .and_then(|r| r.trim().strip_prefix("TA.AperFunction,"))
            {
                function = Some(rest.split(',').map(str::to_string).collect());
            }
            file.comments.push(comment.to_string());
            continue;
        }
        if ended {
            return Err(format!("excellon: {line:?} after M30").into());
        }
        if line == "M48" {
            header = true;
            seen_header = true;
            continue;
        }
        if header {
            if line.starts_with("METRIC") {
                unit = Some(1.0);
                file.metric = true;
            } else if line.starts_with("INCH") {
                unit = Some(25.4);
                file.metric = false;
            } else if line == "%" || line == "M95" {
                header = false;
            } else if line.starts_with("FMAT") || line.starts_with("VER") {
            } else if let Some(t) = line.strip_prefix('T') {
                let (number, diameter) = tool_definition(t)?;
                let s = unit.ok_or("excellon: a tool before METRIC/INCH")?;
                file.tools.insert(number, diameter * s);
                if let Some(values) = function.take() {
                    file.tool_functions.insert(number, values);
                }
            } else {
                return Err(format!("excellon: unsupported header line {line:?}").into());
            }
            continue;
        }
        if !seen_header {
            return Err("excellon: no M48 header".into());
        }
        let s = unit.ok_or("excellon: no unit")?;
        match line {
            "G90" | "G05" => {
                routing = false;
                continue;
            }
            "M30" => {
                ended = true;
                continue;
            }
            "M15" => {
                down = true;
                continue;
            }
            "M16" | "M17" => {
                down = false;
                continue;
            }
            "G91" => return Err("excellon: incremental coordinates are not supported".into()),
            _ => {}
        }
        if let Some(t) = line.strip_prefix('T') {
            let number: u32 = t.parse()?;
            if number != 0 && !file.tools.contains_key(&number) {
                return Err(format!("excellon: T{number} is not defined").into());
            }
            tool = (number != 0).then_some(number);
            continue;
        }
        let selected = || tool.ok_or("excellon: a hit before a tool is selected");
        if let Some(rest) = line.strip_prefix("G00") {
            routing = true;
            at = point(rest, at, s)?;
            continue;
        }
        if let Some(rest) = line.strip_prefix("G01") {
            let to = point(rest, at, s)?;
            if !(routing && down) {
                return Err("excellon: G01 without G00 and M15".into());
            }
            file.hits.push(Hit::Slot {
                tool: selected()?,
                from: at,
                to,
            });
            at = to;
            continue;
        }
        if let Some((a, b)) = line.split_once("G85") {
            let from = point(a, at, s)?;
            let to = point(b, from, s)?;
            file.hits.push(Hit::Slot {
                tool: selected()?,
                from,
                to,
            });
            at = to;
            continue;
        }
        if line.starts_with('X') || line.starts_with('Y') {
            let p = point(line, at, s)?;
            file.hits.push(Hit::Round {
                tool: selected()?,
                at: p,
            });
            at = p;
            continue;
        }
        return Err(format!("excellon: unsupported line {line:?}").into());
    }
    if !ended {
        return Err("excellon: the file does not end with M30".into());
    }
    Ok(file)
}

/// `nnC0.300` (other tool parameters such as `F` and `S` ignored).
fn tool_definition(t: &str) -> Fallible<(u32, f64)> {
    let digits = t.find(|c: char| !c.is_ascii_digit()).unwrap_or(t.len());
    let number: u32 = t[..digits].parse()?;
    let rest = &t[digits..];
    let c = rest
        .find('C')
        .ok_or_else(|| format!("excellon: tool T{t} without a diameter"))?;
    let value = &rest[c + 1..];
    let end = value
        .find(|ch: char| !(ch.is_ascii_digit() || ch == '.'))
        .unwrap_or(value.len());
    Ok((number, value[..end].parse()?))
}

/// `X…Y…` (either may be left out: modal), scaled by `s`.
fn point(text: &str, previous: Point, s: f64) -> Fallible<Point> {
    let mut p = previous;
    let mut rest = text.trim();
    while let Some(letter) = rest.chars().next() {
        let body = &rest[1..];
        let end = body
            .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == '+'))
            .unwrap_or(body.len());
        let v = coordinate(&body[..end], s)?;
        match letter {
            'X' => p[0] = v,
            'Y' => p[1] = v,
            _ => return Err(format!("excellon: unexpected {letter:?} in {text:?}").into()),
        }
        rest = &body[end..];
    }
    Ok(p)
}

fn coordinate(digits: &str, s: f64) -> Fallible<f64> {
    if digits.contains('.') {
        return Ok(digits.parse::<f64>()? * s);
    }
    let n: i64 = digits.parse()?;
    // Implied decimals: 3 in mm, 4 in inches.
    let implied = if s == 1.0 { 1e3 } else { 1e4 };
    Ok(n as f64 / implied * s)
}
