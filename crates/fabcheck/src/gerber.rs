//! A Gerber X2 reader for verification, written from Ucamco's public Gerber
//! Layer Format Specification: format and unit, standard and macro
//! apertures (macro primitives 0, 1, 4, 5, 20 and 21 with `$n` variables
//! and arithmetic), polarity, linear and multi-quadrant circular
//! interpolation, regions with several contours, and file, aperture and
//! object attributes. Coordinates come out in millimetres. Deprecated or
//! unused commands (`LM`, `LR`, `LS`, `SR`, `IP`, `G74`, …) are refused,
//! since the writer must not emit them.
use crate::Fallible;
use crate::Point;
use std::collections::BTreeMap;

/// Attribute name to values (`.FileFunction` → `["Copper", "L1", "Top"]`).
pub type Attrs = BTreeMap<String, Vec<String>>;

/// One macro primitive after its parameters were evaluated, in mm and
/// degrees.
#[derive(Clone, Debug, PartialEq)]
pub enum Prim {
    Circle {
        on: bool,
        diameter: f64,
        center: Point,
        rotation: f64,
    },
    VectorLine {
        on: bool,
        width: f64,
        start: Point,
        end: Point,
        rotation: f64,
    },
    CenterLine {
        on: bool,
        width: f64,
        height: f64,
        center: Point,
        rotation: f64,
    },
    Outline {
        on: bool,
        points: Vec<Point>,
        rotation: f64,
    },
    Polygon {
        on: bool,
        vertices: usize,
        center: Point,
        diameter: f64,
        rotation: f64,
    },
}

/// An aperture template with its parameters, in mm.
#[derive(Clone, Debug, PartialEq)]
pub enum Template {
    Circle {
        diameter: f64,
        hole: Option<f64>,
    },
    Rect {
        w: f64,
        h: f64,
        hole: Option<f64>,
    },
    Obround {
        w: f64,
        h: f64,
        hole: Option<f64>,
    },
    Polygon {
        diameter: f64,
        vertices: usize,
        rotation: f64,
        hole: Option<f64>,
    },
    Macro {
        name: String,
        primitives: Vec<Prim>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Aperture {
    pub template: Template,
    /// The aperture attribute dictionary when it was defined.
    pub attributes: Attrs,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Path {
    Line {
        to: Point,
    },
    Arc {
        to: Point,
        center: Point,
        clockwise: bool,
    },
}

/// A closed region contour from `start`.
#[derive(Clone, Debug, PartialEq)]
pub struct Contour {
    pub start: Point,
    pub segments: Vec<Path>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    Flash {
        at: Point,
    },
    Draw {
        from: Point,
        to: Point,
    },
    Arc {
        from: Point,
        to: Point,
        center: Point,
        clockwise: bool,
    },
    Region {
        contour: Contour,
    },
}

/// One graphics object.
#[derive(Clone, Debug, PartialEq)]
pub struct Object {
    pub kind: Kind,
    /// The aperture of a flash or draw; `None` for a region.
    pub aperture: Option<u32>,
    pub dark: bool,
    /// The aperture's attributes (for a region, the aperture attribute
    /// dictionary in force).
    pub aperture_attributes: Attrs,
    pub object_attributes: Attrs,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct GerberFile {
    pub metric: bool,
    pub integer_digits: u32,
    pub decimals: u32,
    pub file_attributes: Attrs,
    pub apertures: BTreeMap<u32, Aperture>,
    pub macros: BTreeMap<String, Vec<String>>,
    pub objects: Vec<Object>,
    pub comments: Vec<String>,
}

impl GerberFile {
    /// The values of a file attribute (`.FileFunction`).
    pub fn file_attribute(&self, name: &str) -> Option<&[String]> {
        self.file_attributes.get(name).map(Vec::as_slice)
    }
    pub fn flashes(&self) -> impl Iterator<Item = &Object> {
        self.objects
            .iter()
            .filter(|o| matches!(o.kind, Kind::Flash { .. }))
    }
    pub fn regions(&self) -> impl Iterator<Item = &Object> {
        self.objects
            .iter()
            .filter(|o| matches!(o.kind, Kind::Region { .. }))
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Linear,
    Clockwise,
    CounterClockwise,
}

struct State {
    file: GerberFile,
    format: Option<(u32, u32)>,
    /// mm per file unit.
    scale: Option<f64>,
    mode: Option<Mode>,
    multi_quadrant: bool,
    aperture: Option<u32>,
    dark: bool,
    at: Point,
    region: Option<Vec<Contour>>,
    contour: Option<Contour>,
    aperture_dict: Attrs,
    object_dict: Attrs,
    ended: bool,
}

/// Parse one Gerber file.
pub fn parse(source: &str) -> Fallible<GerberFile> {
    let text: String = source
        .chars()
        .filter(|c| *c != '\r' && *c != '\n')
        .collect();
    let mut state = State {
        file: GerberFile::default(),
        format: None,
        scale: None,
        mode: None,
        multi_quadrant: false,
        aperture: None,
        dark: true,
        at: [0.0, 0.0],
        region: None,
        contour: None,
        aperture_dict: Attrs::new(),
        object_dict: Attrs::new(),
        ended: false,
    };
    let mut rest = text.as_str();
    loop {
        rest = rest.trim_start();
        if rest.is_empty() {
            break;
        }
        if state.ended {
            return Err("gerber: content after M02".into());
        }
        if let Some(r) = rest.strip_prefix('%') {
            let end = r
                .find('%')
                .ok_or("gerber: an unterminated extended command")?;
            state.extended(&r[..end])?;
            rest = &r[end + 1..];
        } else {
            let end = rest.find('*').ok_or("gerber: a command without its *")?;
            state.word(rest[..end].trim())?;
            rest = &rest[end + 1..];
        }
    }
    if !state.ended {
        return Err("gerber: the file does not end with M02".into());
    }
    if state.region.is_some() {
        return Err("gerber: a region is not closed with G37".into());
    }
    Ok(state.file)
}

fn attribute(text: &str) -> (String, Vec<String>) {
    let mut parts = text.split(',');
    let name = parts.next().unwrap_or_default().to_string();
    (name, parts.map(unescape).collect())
}

/// Undo `\uXXXX` escapes (UTF-16 code units) in an attribute field.
pub fn unescape(field: &str) -> String {
    let mut units: Vec<u16> = Vec::new();
    let mut out = String::new();
    let mut rest = field;
    while !rest.is_empty() {
        if let Some(hex) = rest.strip_prefix("\\u")
            && hex.len() >= 4
            && let Ok(unit) = u16::from_str_radix(&hex[..4], 16)
        {
            units.push(unit);
            rest = &hex[4..];
            continue;
        }
        if !units.is_empty() {
            out.push_str(&String::from_utf16_lossy(&units));
            units.clear();
        }
        let mut chars = rest.chars();
        if let Some(c) = chars.next() {
            out.push(c);
        }
        rest = chars.as_str();
    }
    if !units.is_empty() {
        out.push_str(&String::from_utf16_lossy(&units));
    }
    out
}

impl State {
    fn scale(&self) -> Fallible<f64> {
        self.scale
            .ok_or_else(|| "gerber: no unit (MO) before use".into())
    }

    fn extended(&mut self, block: &str) -> Fallible<()> {
        let pieces: Vec<&str> = block.split('*').map(str::trim).collect();
        if pieces.last().is_some_and(|p| !p.is_empty()) {
            return Err(format!("gerber: extended command {block:?} lacks its final *").into());
        }
        if let Some(name) = pieces.first().and_then(|p| p.strip_prefix("AM")) {
            let body = pieces[1..]
                .iter()
                .filter(|p| !p.is_empty())
                .map(|p| p.to_string())
                .collect();
            self.file.macros.insert(name.to_string(), body);
            return Ok(());
        }
        for piece in pieces.into_iter().filter(|p| !p.is_empty()) {
            self.extended_command(piece)?;
        }
        Ok(())
    }

    fn extended_command(&mut self, c: &str) -> Fallible<()> {
        if let Some(fs) = c.strip_prefix("FSLA") {
            let digits = |s: &str| -> Fallible<(u32, u32)> {
                let b = s.as_bytes();
                if b.len() != 2 || !b.iter().all(u8::is_ascii_digit) {
                    return Err(format!("gerber: format {c:?}").into());
                }
                Ok((u32::from(b[0] - b'0'), u32::from(b[1] - b'0')))
            };
            let x = fs.strip_prefix('X').ok_or("gerber: FS without X")?;
            let (xs, ys) = x.split_at(x.find('Y').ok_or("gerber: FS without Y")?);
            let (fx, fy) = (digits(xs)?, digits(&ys[1..])?);
            if fx != fy {
                return Err("gerber: X and Y formats differ".into());
            }
            self.format = Some(fx);
            self.file.integer_digits = fx.0;
            self.file.decimals = fx.1;
        } else if c == "MOMM" {
            self.scale = Some(1.0);
            self.file.metric = true;
        } else if c == "MOIN" {
            self.scale = Some(25.4);
            self.file.metric = false;
        } else if let Some(ad) = c.strip_prefix("ADD") {
            self.aperture_definition(ad)?;
        } else if c == "LPD" {
            self.dark = true;
        } else if c == "LPC" {
            self.dark = false;
        } else if let Some(tf) = c.strip_prefix("TF") {
            let (name, values) = attribute(tf);
            self.file.file_attributes.insert(name, values);
        } else if let Some(ta) = c.strip_prefix("TA") {
            let (name, values) = attribute(ta);
            self.aperture_dict.insert(name, values);
        } else if let Some(to) = c.strip_prefix("TO") {
            let (name, values) = attribute(to);
            self.object_dict.insert(name, values);
        } else if let Some(td) = c.strip_prefix("TD") {
            if td.is_empty() {
                self.aperture_dict.clear();
                self.object_dict.clear();
            } else {
                self.aperture_dict.remove(td);
                self.object_dict.remove(td);
            }
        } else {
            return Err(format!("gerber: unsupported extended command {c:?}").into());
        }
        Ok(())
    }

    fn aperture_definition(&mut self, ad: &str) -> Fallible<()> {
        let digits = ad.find(|c: char| !c.is_ascii_digit()).unwrap_or(ad.len());
        let number: u32 = ad[..digits].parse()?;
        if number < 10 {
            return Err(format!("gerber: aperture number D{number} is reserved").into());
        }
        let rest = &ad[digits..];
        let (name, params) = match rest.split_once(',') {
            Some((n, p)) => (n, Some(p)),
            None => (rest, None),
        };
        let values: Vec<f64> = match params {
            Some(p) => p
                .split('X')
                .map(|v| v.trim().parse::<f64>())
                .collect::<Result<_, _>>()?,
            None => Vec::new(),
        };
        let s = self.scale()?;
        let get = |i: usize| -> Fallible<f64> {
            values
                .get(i)
                .copied()
                .ok_or_else(|| format!("gerber: D{number} lacks a parameter").into())
        };
        let hole = |i: usize| values.get(i).map(|v| v * s);
        let template = match name {
            "C" => Template::Circle {
                diameter: get(0)? * s,
                hole: hole(1),
            },
            "R" => Template::Rect {
                w: get(0)? * s,
                h: get(1)? * s,
                hole: hole(2),
            },
            "O" => Template::Obround {
                w: get(0)? * s,
                h: get(1)? * s,
                hole: hole(2),
            },
            "P" => Template::Polygon {
                diameter: get(0)? * s,
                vertices: get(1)? as usize,
                rotation: values.get(2).copied().unwrap_or(0.0),
                hole: hole(3),
            },
            other => {
                let body = self
                    .file
                    .macros
                    .get(other)
                    .ok_or_else(|| format!("gerber: macro {other:?} is not defined"))?;
                Template::Macro {
                    name: other.to_string(),
                    primitives: evaluate_macro(body, &values, s)?,
                }
            }
        };
        if self.file.apertures.contains_key(&number) {
            return Err(format!("gerber: D{number} is defined twice").into());
        }
        self.file.apertures.insert(
            number,
            Aperture {
                template,
                attributes: self.aperture_dict.clone(),
            },
        );
        Ok(())
    }

    fn word(&mut self, w: &str) -> Fallible<()> {
        if w.is_empty() {
            return Ok(());
        }
        if let Some(comment) = w.strip_prefix("G04") {
            self.file.comments.push(comment.trim().to_string());
            return Ok(());
        }
        for (code, mode) in [
            ("G01", Mode::Linear),
            ("G02", Mode::Clockwise),
            ("G03", Mode::CounterClockwise),
        ] {
            if let Some(rest) = w.strip_prefix(code) {
                self.mode = Some(mode);
                return self.word(rest);
            }
        }
        match w {
            "G75" => {
                self.multi_quadrant = true;
                return Ok(());
            }
            "G36" => {
                if self.region.is_some() {
                    return Err("gerber: G36 inside a region".into());
                }
                self.region = Some(Vec::new());
                return Ok(());
            }
            "G37" => {
                self.close_contour()?;
                let contours = self.region.take().ok_or("gerber: G37 without G36")?;
                for contour in contours {
                    self.file.objects.push(Object {
                        kind: Kind::Region { contour },
                        aperture: None,
                        dark: self.dark,
                        aperture_attributes: self.aperture_dict.clone(),
                        object_attributes: self.object_dict.clone(),
                    });
                }
                return Ok(());
            }
            "M02" => {
                self.ended = true;
                return Ok(());
            }
            _ => {}
        }
        if let Some(d) = w.strip_prefix('D')
            && let Ok(n) = d.parse::<u32>()
            && n >= 10
        {
            if !self.file.apertures.contains_key(&n) {
                return Err(format!("gerber: D{n} selected before it is defined").into());
            }
            if self.region.is_some() {
                return Err("gerber: aperture selected inside a region".into());
            }
            self.aperture = Some(n);
            return Ok(());
        }
        if w.starts_with(['X', 'Y', 'I', 'J', 'D']) {
            return self.operation(w);
        }
        Err(format!("gerber: unsupported command {w:?}").into())
    }

    fn coordinate(&self, digits: &str) -> Fallible<f64> {
        let (_, decimals) = self.format.ok_or("gerber: coordinates before FS")?;
        if digits.contains('.') {
            return Err(format!("gerber: a coordinate with a decimal point {digits:?}").into());
        }
        let n: i64 = digits.parse()?;
        Ok(n as f64 / 10f64.powi(decimals as i32) * self.scale()?)
    }

    fn operation(&mut self, w: &str) -> Fallible<()> {
        let mut values: BTreeMap<char, &str> = BTreeMap::new();
        let mut code = None;
        let mut rest = w;
        while let Some(letter) = rest.chars().next() {
            let body = &rest[1..];
            let end = body
                .find(|c: char| !(c.is_ascii_digit() || c == '-' || c == '+'))
                .unwrap_or(body.len());
            let value = &body[..end];
            match letter {
                'X' | 'Y' | 'I' | 'J' => {
                    values.insert(letter, value);
                }
                'D' => code = Some(value.parse::<u32>()?),
                _ => return Err(format!("gerber: unexpected {letter:?} in {w:?}").into()),
            }
            rest = &body[end..];
        }
        let code = code.ok_or_else(|| format!("gerber: {w:?} has no D01/D02/D03"))?;
        let x = match values.get(&'X') {
            Some(v) => self.coordinate(v)?,
            None => self.at[0],
        };
        let y = match values.get(&'Y') {
            Some(v) => self.coordinate(v)?,
            None => self.at[1],
        };
        let i = values.get(&'I').map(|v| self.coordinate(v)).transpose()?;
        let j = values.get(&'J').map(|v| self.coordinate(v)).transpose()?;
        let to = [x, y];
        let from = self.at;
        match code {
            2 => {
                if self.region.is_some() {
                    self.close_contour()?;
                    self.contour = Some(Contour {
                        start: to,
                        segments: Vec::new(),
                    });
                }
            }
            1 => {
                let mode = self.mode.ok_or("gerber: D01 before G01/G02/G03")?;
                let path = match mode {
                    Mode::Linear => Path::Line { to },
                    Mode::Clockwise | Mode::CounterClockwise => {
                        if !self.multi_quadrant {
                            return Err("gerber: circular interpolation without G75".into());
                        }
                        let center = [from[0] + i.unwrap_or(0.0), from[1] + j.unwrap_or(0.0)];
                        Path::Arc {
                            to,
                            center,
                            clockwise: mode == Mode::Clockwise,
                        }
                    }
                };
                if self.region.is_some() {
                    self.contour
                        .get_or_insert(Contour {
                            start: from,
                            segments: Vec::new(),
                        })
                        .segments
                        .push(path);
                } else {
                    let aperture = self.aperture.ok_or("gerber: D01 without an aperture")?;
                    let kind = match path {
                        Path::Line { to } => Kind::Draw { from, to },
                        Path::Arc {
                            to,
                            center,
                            clockwise,
                        } => Kind::Arc {
                            from,
                            to,
                            center,
                            clockwise,
                        },
                    };
                    self.push(kind, aperture);
                }
            }
            3 => {
                if self.region.is_some() {
                    return Err("gerber: D03 inside a region".into());
                }
                let aperture = self.aperture.ok_or("gerber: D03 without an aperture")?;
                self.push(Kind::Flash { at: to }, aperture);
            }
            other => return Err(format!("gerber: D{other:02} is not an operation").into()),
        }
        self.at = to;
        Ok(())
    }

    fn push(&mut self, kind: Kind, aperture: u32) {
        let aperture_attributes = self
            .file
            .apertures
            .get(&aperture)
            .map(|a| a.attributes.clone())
            .unwrap_or_default();
        self.file.objects.push(Object {
            kind,
            aperture: Some(aperture),
            dark: self.dark,
            aperture_attributes,
            object_attributes: self.object_dict.clone(),
        });
    }

    /// End the contour being drawn in a region: it must be closed.
    fn close_contour(&mut self) -> Fallible<()> {
        let Some(contour) = self.contour.take() else {
            return Ok(());
        };
        if contour.segments.is_empty() {
            return Ok(());
        }
        let end = match contour.segments.last() {
            Some(Path::Line { to } | Path::Arc { to, .. }) => *to,
            None => contour.start,
        };
        if (end[0] - contour.start[0]).hypot(end[1] - contour.start[1]) > 1e-9 {
            return Err(format!(
                "gerber: a region contour is not closed (from {:?} to {end:?})",
                contour.start
            )
            .into());
        }
        if let Some(region) = self.region.as_mut() {
            region.push(contour);
        }
        Ok(())
    }
}

/// Evaluate a macro body with its parameters `$1…`; lengths are scaled
/// by `s` (mm per file unit).
fn evaluate_macro(body: &[String], parameters: &[f64], s: f64) -> Fallible<Vec<Prim>> {
    let mut vars: BTreeMap<u32, f64> = parameters
        .iter()
        .enumerate()
        .map(|(i, v)| (i as u32 + 1, *v))
        .collect();
    let mut out = Vec::new();
    for statement in body {
        let statement = statement.trim();
        if let Some(def) = statement.strip_prefix('$') {
            let (name, expr) = def
                .split_once('=')
                .ok_or_else(|| format!("gerber: macro statement {statement:?}"))?;
            let value = Expr::eval(expr, &vars)?;
            vars.insert(name.trim().parse()?, value);
            continue;
        }
        // Primitive 0 is a comment: its text runs to the `*`.
        if statement == "0" || statement.starts_with("0 ") {
            continue;
        }
        let fields: Vec<&str> = statement.split(',').collect();
        let code: u32 = fields
            .first()
            .ok_or("gerber: an empty macro primitive")?
            .trim()
            .parse()?;
        if code == 0 {
            continue;
        }
        let v = fields[1..]
            .iter()
            .map(|f| Expr::eval(f, &vars))
            .collect::<Fallible<Vec<f64>>>()?;
        let get = |i: usize| -> Fallible<f64> {
            v.get(i)
                .copied()
                .ok_or_else(|| format!("gerber: primitive {code} lacks modifier {}", i + 1).into())
        };
        let on = get(0)? != 0.0;
        out.push(match code {
            1 => Prim::Circle {
                on,
                diameter: get(1)? * s,
                center: [get(2)? * s, get(3)? * s],
                rotation: v.get(4).copied().unwrap_or(0.0),
            },
            20 => Prim::VectorLine {
                on,
                width: get(1)? * s,
                start: [get(2)? * s, get(3)? * s],
                end: [get(4)? * s, get(5)? * s],
                rotation: get(6)?,
            },
            21 => Prim::CenterLine {
                on,
                width: get(1)? * s,
                height: get(2)? * s,
                center: [get(3)? * s, get(4)? * s],
                rotation: get(5)?,
            },
            4 => {
                let n = get(1)? as usize;
                let points = (0..=n)
                    .map(|k| Ok([get(2 + 2 * k)? * s, get(3 + 2 * k)? * s]))
                    .collect::<Fallible<Vec<Point>>>()?;
                let (first, last) = (points[0], points[n]);
                if (first[0] - last[0]).hypot(first[1] - last[1]) > 1e-9 {
                    return Err("gerber: an outline primitive must end at its start".into());
                }
                Prim::Outline {
                    on,
                    points: points[..n].to_vec(),
                    rotation: get(2 + 2 * (n + 1))?,
                }
            }
            5 => Prim::Polygon {
                on,
                vertices: get(1)? as usize,
                center: [get(2)? * s, get(3)? * s],
                diameter: get(4)? * s,
                rotation: get(5)?,
            },
            other => return Err(format!("gerber: macro primitive {other} is not supported").into()),
        });
    }
    Ok(out)
}

/// Macro arithmetic: `+ - x /`, unary signs, parentheses, numbers, `$n`.
struct Expr<'a> {
    chars: std::iter::Peekable<std::str::Chars<'a>>,
    vars: &'a BTreeMap<u32, f64>,
}

impl<'a> Expr<'a> {
    fn eval(text: &'a str, vars: &'a BTreeMap<u32, f64>) -> Fallible<f64> {
        let mut e = Expr {
            chars: text.trim().chars().peekable(),
            vars,
        };
        let v = e.sum()?;
        if e.chars.next().is_some() {
            return Err(format!("gerber: trailing text in expression {text:?}").into());
        }
        Ok(v)
    }
    fn skip(&mut self) {
        while self.chars.peek().is_some_and(|c| c.is_whitespace()) {
            self.chars.next();
        }
    }
    fn sum(&mut self) -> Fallible<f64> {
        let mut v = self.product()?;
        loop {
            self.skip();
            match self.chars.peek() {
                Some('+') => {
                    self.chars.next();
                    v += self.product()?;
                }
                Some('-') => {
                    self.chars.next();
                    v -= self.product()?;
                }
                _ => return Ok(v),
            }
        }
    }
    fn product(&mut self) -> Fallible<f64> {
        let mut v = self.factor()?;
        loop {
            self.skip();
            match self.chars.peek() {
                Some('x' | 'X') => {
                    self.chars.next();
                    v *= self.factor()?;
                }
                Some('/') => {
                    self.chars.next();
                    v /= self.factor()?;
                }
                _ => return Ok(v),
            }
        }
    }
    fn factor(&mut self) -> Fallible<f64> {
        self.skip();
        match self.chars.peek().copied() {
            Some('-') => {
                self.chars.next();
                Ok(-self.factor()?)
            }
            Some('+') => {
                self.chars.next();
                self.factor()
            }
            Some('(') => {
                self.chars.next();
                let v = self.sum()?;
                self.skip();
                if self.chars.next() != Some(')') {
                    return Err("gerber: unbalanced parenthesis".into());
                }
                Ok(v)
            }
            Some('$') => {
                self.chars.next();
                let mut n = String::new();
                while let Some(c) = self.chars.peek().copied().filter(char::is_ascii_digit) {
                    n.push(c);
                    self.chars.next();
                }
                let k: u32 = n.parse()?;
                self.vars
                    .get(&k)
                    .copied()
                    .ok_or_else(|| format!("gerber: macro variable ${k} is undefined").into())
            }
            Some(c) if c.is_ascii_digit() || c == '.' => {
                let mut n = String::new();
                while let Some(c) = self
                    .chars
                    .peek()
                    .copied()
                    .filter(|c| c.is_ascii_digit() || *c == '.')
                {
                    n.push(c);
                    self.chars.next();
                }
                Ok(n.parse()?)
            }
            other => Err(format!("gerber: unexpected {other:?} in an expression").into()),
        }
    }
}
