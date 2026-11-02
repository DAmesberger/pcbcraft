//! A Gerber X2 file writer, from Ucamco's public Gerber Layer Format
//! Specification: millimetres in the 4.6 coordinate format, absolute
//! coordinates, multi-quadrant arcs, standard and macro apertures defined
//! once each before the image, attributes (`%TF`, `%TA`, `%TO`) and regions.
//!
//! The writer is deterministic: apertures and macros are numbered in the
//! order they are first used, and numbers are written in one canonical
//! form (no exponent, trailing zeros trimmed).
use super::MAX_FILE_BYTES;
use crate::diagnostics::{Error, Result};
use crate::geometry::Point;
use std::collections::BTreeMap;
use std::fmt::Write as _;

/// Decimal places of coordinates: 6 (1 nm), the format's finest.
pub const DECIMALS: u32 = 6;
/// Coordinates are below this many mm in magnitude (4 integer digits).
pub const COORDINATE_LIMIT_MM: f64 = 9999.0;
const SCALE: f64 = 1e6;
/// The first aperture number the specification allows.
const FIRST_D: u32 = 10;

/// A coordinate in format units (nm), refusing values the format cannot
/// carry.
pub fn units(mm: f64) -> Result<i64> {
    if !(mm.is_finite() && mm.abs() < COORDINATE_LIMIT_MM) {
        return Err(Error::Invalid(format!(
            "fabrication.coordinate: {mm} mm is outside the Gerber coordinate range"
        )));
    }
    // Bounded above: |mm| < 10^4, so |mm·10^6| < 10^10.
    Ok((mm * SCALE).round() as i64)
}

/// A decimal number in canonical form: at most 6 decimals, trailing zeros
/// and a trailing point trimmed, `-0` written as `0`.
pub fn number(v: f64) -> String {
    let mut s = format!("{:.6}", v);
    if s.contains('.') {
        while s.ends_with('0') {
            s.pop();
        }
        if s.ends_with('.') {
            s.pop();
        }
    }
    if s == "-0" { "0".into() } else { s }
}

/// Escape an attribute field value: characters outside printable ASCII
/// and the reserved `%`, `*`, `,` and `\` become `\uXXXX`
/// (UTF-16 code units).
pub fn escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        if c.is_ascii_graphic() && !matches!(c, '%' | '*' | ',' | '\\') || c == ' ' {
            out.push(c);
        } else {
            let mut buf = [0u16; 2];
            for unit in c.encode_utf16(&mut buf) {
                let _ = write!(out, "\\u{unit:04X}");
            }
        }
    }
    out
}

/// An aperture shape. Standard sizes are kept in format units so equal
/// apertures share one number.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Aperture {
    /// Diameter.
    Circle(i64),
    /// Width and height.
    Rect(i64, i64),
    /// Width and height; the short axis is the round one.
    Obround(i64, i64),
    /// A macro: its primitive lines (without `%AM…` and the final `%`).
    Macro(String),
}

impl Aperture {
    pub fn circle(diameter_mm: f64) -> Result<Self> {
        Ok(Self::Circle(units(diameter_mm.max(0.0))?))
    }
    fn definition(&self, macros: &BTreeMap<String, String>) -> String {
        let mm = |u: i64| number(u as f64 / SCALE);
        match self {
            Self::Circle(d) => format!("C,{}", mm(*d)),
            Self::Rect(w, h) => format!("R,{}X{}", mm(*w), mm(*h)),
            Self::Obround(w, h) => format!("O,{}X{}", mm(*w), mm(*h)),
            Self::Macro(body) => macros.get(body).cloned().unwrap_or_default(),
        }
    }
}

/// One piece of a path or region contour, ending at `to`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Seg {
    Line(Point),
    /// A circular arc about `center`, counter-clockwise when `ccw`. An arc
    /// ending where it starts is a full circle.
    Arc {
        to: Point,
        center: Point,
        ccw: bool,
    },
}

impl Seg {
    pub fn to(&self) -> Point {
        match *self {
            Self::Line(to) | Self::Arc { to, .. } => to,
        }
    }
}

/// One Gerber file being written.
pub struct Writer {
    header: String,
    body: String,
    /// Aperture (with its function) to number, in first-use order.
    apertures: Vec<(Aperture, Option<String>, u32)>,
    /// Macro primitive text to macro name.
    macros: BTreeMap<String, String>,
    macro_order: Vec<String>,
    next_d: u32,
    current: Option<u32>,
    at: Option<(i64, i64)>,
    dark: bool,
    object: Vec<(String, String)>,
    /// 1 linear, 2 clockwise, 3 counter-clockwise.
    mode: u8,
}

impl Writer {
    /// A file with these file attributes (`("FileFunction", "Copper,L1,Top")`),
    /// written in order after the format statements.
    pub fn new(attributes: &[(&str, String)]) -> Self {
        let mut header = String::new();
        for (name, value) in attributes {
            if value.is_empty() {
                let _ = writeln!(header, "%TF.{name}*%");
            } else {
                let _ = writeln!(header, "%TF.{name},{value}*%");
            }
        }
        Self {
            header,
            body: String::new(),
            apertures: Vec::new(),
            macros: BTreeMap::new(),
            macro_order: Vec::new(),
            next_d: FIRST_D,
            current: None,
            at: None,
            dark: true,
            object: Vec::new(),
            mode: 0,
        }
    }

    /// The number of `aperture` with aperture function `function` (an X2
    /// `.AperFunction` value such as `SMDPad,CuDef`), defining it on first
    /// use.
    pub fn aperture(&mut self, aperture: Aperture, function: Option<&str>) -> u32 {
        let function = function.map(str::to_string);
        if let Some((_, _, d)) = self
            .apertures
            .iter()
            .find(|(a, f, _)| *a == aperture && *f == function)
        {
            return *d;
        }
        if let Aperture::Macro(body) = &aperture
            && !self.macros.contains_key(body)
        {
            let name = format!("M{}", self.macros.len());
            self.macros.insert(body.clone(), name);
            self.macro_order.push(body.clone());
        }
        let d = self.next_d;
        self.next_d = self.next_d.saturating_add(1);
        self.apertures.push((aperture, function, d));
        d
    }

    /// Set the object attributes of the objects that follow (`("N", net)`,
    /// `("P", "U1,3")`, `("C", "U1")`), values escaped by the caller.
    pub fn object(&mut self, attributes: &[(&str, String)]) {
        let wanted: Vec<(String, String)> = attributes
            .iter()
            .map(|(k, v)| ((*k).to_string(), v.clone()))
            .collect();
        if wanted == self.object {
            return;
        }
        if !self.object.is_empty() {
            self.body.push_str("%TD*%\n");
        }
        for (k, v) in &wanted {
            let _ = writeln!(self.body, "%TO.{k},{v}*%");
        }
        self.object = wanted;
    }

    /// Dark (adds) or clear (erases what was drawn before) polarity.
    pub fn polarity(&mut self, dark: bool) {
        if dark != self.dark {
            self.body
                .push_str(if dark { "%LPD*%\n" } else { "%LPC*%\n" });
            self.dark = dark;
        }
    }

    fn select(&mut self, d: u32) {
        if self.current != Some(d) {
            let _ = writeln!(self.body, "D{d}*");
            self.current = Some(d);
        }
    }

    fn set_mode(&mut self, mode: u8) {
        if self.mode != mode {
            let _ = writeln!(self.body, "G0{mode}*");
            self.mode = mode;
        }
    }

    /// `X…Y…` for a move to `p`, leaving out an unchanged coordinate.
    fn xy(&mut self, p: Point) -> Result<String> {
        let (x, y) = (units(p[0])?, units(p[1])?);
        let mut s = String::new();
        match self.at {
            Some((ax, ay)) => {
                if ax != x {
                    let _ = write!(s, "X{x}");
                }
                if ay != y {
                    let _ = write!(s, "Y{y}");
                }
            }
            None => {
                let _ = write!(s, "X{x}Y{y}");
            }
        }
        self.at = Some((x, y));
        Ok(s)
    }

    fn check_size(&self) -> Result<()> {
        if self.body.len() > MAX_FILE_BYTES {
            return Err(Error::Limit(
                "fabrication.too_large: a Gerber layer exceeds the file bound".into(),
            ));
        }
        Ok(())
    }

    /// Flash aperture `d` at `at`.
    pub fn flash(&mut self, d: u32, at: Point) -> Result<()> {
        self.select(d);
        let xy = self.xy(at)?;
        let _ = writeln!(self.body, "{xy}D03*");
        self.check_size()
    }

    fn move_to(&mut self, p: Point) -> Result<()> {
        let (x, y) = (units(p[0])?, units(p[1])?);
        if self.at != Some((x, y)) {
            let xy = self.xy(p)?;
            let _ = writeln!(self.body, "{xy}D02*");
        }
        Ok(())
    }

    fn segments(&mut self, segs: &[Seg]) -> Result<()> {
        for seg in segs {
            match *seg {
                Seg::Line(to) => {
                    self.set_mode(1);
                    let xy = self.xy(to)?;
                    // A zero-length draw still marks the aperture there.
                    let xy = if xy.is_empty() { self.here()? } else { xy };
                    let _ = writeln!(self.body, "{xy}D01*");
                }
                Seg::Arc { to, center, ccw } => {
                    let Some((sx, sy)) = self.at else {
                        return Err(Error::Invalid(
                            "fabrication.arc: an arc needs a start point".into(),
                        ));
                    };
                    self.set_mode(if ccw { 3 } else { 2 });
                    let (cx, cy) = (units(center[0])?, units(center[1])?);
                    let xy = self.xy(to)?;
                    let xy = if xy.is_empty() { self.here()? } else { xy };
                    let _ = writeln!(self.body, "{xy}I{}J{}D01*", cx - sx, cy - sy);
                }
            }
        }
        self.check_size()
    }

    /// The full `X…Y…` of the current point.
    fn here(&self) -> Result<String> {
        let Some((x, y)) = self.at else {
            return Err(Error::Invalid("fabrication.draw: no current point".into()));
        };
        Ok(format!("X{x}Y{y}"))
    }

    /// Stroke a path from `start` with aperture `d` (a circle for round
    /// ends and joins).
    pub fn stroke(&mut self, d: u32, start: Point, segs: &[Seg]) -> Result<()> {
        self.select(d);
        self.move_to(start)?;
        if segs.is_empty() {
            self.set_mode(1);
            let xy = self.here()?;
            let _ = writeln!(self.body, "{xy}D01*");
            return self.check_size();
        }
        self.segments(segs)
    }

    /// A region of one closed contour from `start` (the last segment must
    /// end at `start`), carrying aperture function `function`.
    pub fn region(&mut self, function: Option<&str>, start: Point, segs: &[Seg]) -> Result<()> {
        if let Some(f) = function {
            let _ = writeln!(self.body, "%TA.AperFunction,{f}*%");
        }
        self.body.push_str("G36*\n");
        // A contour starts with a move inside the region statement.
        let (x, y) = (units(start[0])?, units(start[1])?);
        self.at = Some((x, y));
        let _ = writeln!(self.body, "X{x}Y{y}D02*");
        self.segments(segs)?;
        self.body.push_str("G37*\n");
        if function.is_some() {
            self.body.push_str("%TD.AperFunction*%\n");
        }
        Ok(())
    }

    /// The complete file.
    pub fn finish(self) -> Result<Vec<u8>> {
        let mut out = String::with_capacity(self.body.len() + 4096);
        out.push_str("%FSLAX46Y46*%\n%MOMM*%\n");
        out.push_str(&self.header);
        out.push_str("G75*\n");
        for body in &self.macro_order {
            if let Some(name) = self.macros.get(body) {
                let _ = write!(out, "%AM{name}*\n{body}%\n");
            }
        }
        let mut function: Option<&String> = None;
        for (aperture, f, d) in &self.apertures {
            if f.as_ref() != function {
                match f {
                    Some(f) => {
                        let _ = writeln!(out, "%TA.AperFunction,{f}*%");
                    }
                    None => out.push_str("%TD.AperFunction*%\n"),
                }
                function = f.as_ref();
            }
            let _ = writeln!(out, "%ADD{d}{}*%", aperture.definition(&self.macros));
        }
        if function.is_some() {
            out.push_str("%TD.AperFunction*%\n");
        }
        out.push_str(&self.body);
        if !self.object.is_empty() {
            out.push_str("%TD*%\n");
        }
        out.push_str("M02*\n");
        if out.len() > MAX_FILE_BYTES {
            return Err(Error::Limit(
                "fabrication.too_large: a Gerber layer exceeds the file bound".into(),
            ));
        }
        Ok(out.into_bytes())
    }
}

/// The macro text of a primitive list: each primitive `code,params…*` on
/// its own line.
pub fn macro_body(primitives: &[Vec<f64>]) -> String {
    let mut s = String::new();
    for p in primitives {
        let fields: Vec<String> = p.iter().map(|v| number(*v)).collect();
        let _ = writeln!(s, "{}*", fields.join(","));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_and_coordinates_are_canonical() -> Result<()> {
        assert_eq!(number(1.25), "1.25");
        assert_eq!(number(2.0), "2");
        assert_eq!(number(-0.0000001), "0");
        assert_eq!(number(0.1234567), "0.123457");
        assert_eq!(units(12.3456789)?, 12_345_679);
        assert_eq!(units(-0.5)?, -500_000);
        assert!(units(10_000.0).is_err());
        assert!(units(f64::NAN).is_err());
        Ok(())
    }

    #[test]
    fn attribute_values_escape_reserved_characters() {
        assert_eq!(escape("GND"), "GND");
        assert_eq!(escape("A,B*C%"), "A\\u002CB\\u002AC\\u0025");
        assert_eq!(escape("Ω"), "\\u03A9");
        assert_eq!(escape("N 1"), "N 1");
    }

    #[test]
    fn apertures_are_defined_once_in_first_use_order_with_their_function() -> Result<()> {
        let mut w = Writer::new(&[("FileFunction", "Copper,L1,Top".into())]);
        let a = w.aperture(Aperture::circle(0.6)?, Some("ViaPad"));
        let b = w.aperture(Aperture::Rect(1_000_000, 500_000), Some("SMDPad,CuDef"));
        let again = w.aperture(Aperture::circle(0.6)?, Some("ViaPad"));
        let other = w.aperture(Aperture::circle(0.6)?, Some("Conductor"));
        assert_eq!((a, b, again, other), (10, 11, 10, 12));
        w.object(&[("N", "GND".into())]);
        w.flash(a, [1.0, 2.0])?;
        w.flash(a, [1.0, 3.0])?;
        w.stroke(other, [0.0, 0.0], &[Seg::Line([5.0, 0.0])])?;
        let text = String::from_utf8(w.finish()?).map_err(|e| Error::Invalid(e.to_string()))?;
        let expected = "%FSLAX46Y46*%\n%MOMM*%\n%TF.FileFunction,Copper,L1,Top*%\nG75*\n\
%TA.AperFunction,ViaPad*%\n%ADD10C,0.6*%\n%TA.AperFunction,SMDPad,CuDef*%\n%ADD11R,1X0.5*%\n\
%TA.AperFunction,Conductor*%\n%ADD12C,0.6*%\n%TD.AperFunction*%\n\
%TO.N,GND*%\nD10*\nX1000000Y2000000D03*\nY3000000D03*\nD12*\nX0Y0D02*\nG01*\nX5000000D01*\n%TD*%\nM02*\n";
        assert_eq!(text, expected);
        Ok(())
    }

    #[test]
    fn regions_and_arcs_carry_offsets_from_the_start() -> Result<()> {
        let mut w = Writer::new(&[]);
        w.region(
            Some("Conductor"),
            [0.0, 0.0],
            &[
                Seg::Line([2.0, 0.0]),
                Seg::Arc {
                    to: [2.0, 2.0],
                    center: [2.0, 1.0],
                    ccw: true,
                },
                Seg::Line([0.0, 0.0]),
            ],
        )?;
        let text = String::from_utf8(w.finish()?).map_err(|e| Error::Invalid(e.to_string()))?;
        assert!(text.contains(
            "%TA.AperFunction,Conductor*%\nG36*\nX0Y0D02*\nG01*\nX2000000D01*\nG03*\nY2000000I0J1000000D01*\nG01*\nX0Y0D01*\nG37*\n%TD.AperFunction*%\n"
        ), "{text}");
        Ok(())
    }
}
