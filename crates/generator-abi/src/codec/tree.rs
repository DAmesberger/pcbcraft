//! The JSON tree the codec works on, and its reader and writer. Numbers are
//! integers only: the reader refuses a fraction or an exponent before any
//! arithmetic, so no floating-point code is ever reached, in the host or
//! in a module.
use super::{CodecError, MAX_DEPTH};
use alloc::string::String;
use alloc::vec::Vec;

/// A JSON value with integer numbers. Objects keep their members in the
/// order read or built; the writer sorts them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Canon {
    Null,
    Bool(bool),
    /// A negative integer (`i64`), or any integer built from an `i64`.
    Int(i64),
    /// A non-negative integer beyond `i64`.
    UInt(u64),
    Str(String),
    Arr(Vec<Canon>),
    Obj(Vec<(String, Canon)>),
}

/// Write `value` canonically: keys sorted by their UTF-8 bytes, no
/// whitespace, integers in shortest form.
pub fn write(value: &Canon, out: &mut Vec<u8>, depth: usize) -> Result<(), CodecError> {
    if depth > MAX_DEPTH {
        return Err(CodecError::TooDeep);
    }
    match value {
        Canon::Null => out.extend_from_slice(b"null"),
        Canon::Bool(true) => out.extend_from_slice(b"true"),
        Canon::Bool(false) => out.extend_from_slice(b"false"),
        Canon::Int(i) => write_int(i128::from(*i), out),
        Canon::UInt(u) => write_int(i128::from(*u), out),
        Canon::Str(s) => write_string(s, out)?,
        Canon::Arr(items) => {
            out.push(b'[');
            for (k, item) in items.iter().enumerate() {
                if k > 0 {
                    out.push(b',');
                }
                write(item, out, depth + 1)?;
            }
            out.push(b']');
        }
        Canon::Obj(members) => {
            let mut sorted: Vec<&(String, Canon)> = members.iter().collect();
            sorted.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
            if sorted.windows(2).any(|w| w[0].0 == w[1].0) {
                return Err(CodecError::NotCanonical);
            }
            out.push(b'{');
            for (k, (key, item)) in sorted.into_iter().enumerate() {
                if k > 0 {
                    out.push(b',');
                }
                write_string(key, out)?;
                out.push(b':');
                write(item, out, depth + 1)?;
            }
            out.push(b'}');
        }
    }
    Ok(())
}

fn write_int(mut v: i128, out: &mut Vec<u8>) {
    if v < 0 {
        out.push(b'-');
        v = -v;
    }
    let mut digits = [0u8; 40];
    let mut n = 0;
    loop {
        // `v % 10` is 0–9.
        digits[n] = b'0' + (v % 10) as u8;
        n += 1;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    out.extend(digits[..n].iter().rev());
}

/// A string as `serde_json` writes one: `\"`, `\\`, the five short control
/// escapes, `\u00xx` (lower-case hex) for the other control characters,
/// everything else literal. With the `nfc` feature, the string must be in
/// Unicode NFC.
pub fn write_string(s: &str, out: &mut Vec<u8>) -> Result<(), CodecError> {
    #[cfg(feature = "nfc")]
    if !unicode_normalization::is_nfc(s) {
        return Err(CodecError::NotNfc);
    }
    const HEX: &[u8; 16] = b"0123456789abcdef";
    out.push(b'"');
    for &b in s.as_bytes() {
        match b {
            b'"' => out.extend_from_slice(b"\\\""),
            b'\\' => out.extend_from_slice(b"\\\\"),
            0x08 => out.extend_from_slice(b"\\b"),
            0x09 => out.extend_from_slice(b"\\t"),
            0x0a => out.extend_from_slice(b"\\n"),
            0x0c => out.extend_from_slice(b"\\f"),
            0x0d => out.extend_from_slice(b"\\r"),
            0x00..=0x1f => {
                out.extend_from_slice(b"\\u00");
                out.push(HEX[usize::from(b >> 4)]);
                out.push(HEX[usize::from(b & 0x0f)]);
            }
            _ => out.push(b),
        }
    }
    out.push(b'"');
    Ok(())
}

/// Read one JSON value filling `bytes`, whitespace allowed around tokens.
/// A fraction or exponent is [`CodecError::Float`], `-0` too (it is not
/// an integer's shortest form); a key twice in one object is
/// [`CodecError::NotCanonical`].
pub fn read(bytes: &[u8]) -> Result<Canon, CodecError> {
    let mut reader = Reader { bytes, at: 0 };
    let value = reader.value(0)?;
    reader.space();
    if reader.at != bytes.len() {
        return Err(reader.syntax("trailing characters"));
    }
    Ok(value)
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn syntax(&self, what: &str) -> CodecError {
        CodecError::Syntax(alloc::format!("{what} at byte {}", self.at))
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.at).copied()
    }

    fn space(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.at += 1;
        }
    }

    fn literal(&mut self, word: &[u8], value: Canon) -> Result<Canon, CodecError> {
        if self.bytes.get(self.at..self.at + word.len()) == Some(word) {
            self.at += word.len();
            Ok(value)
        } else {
            Err(self.syntax("an unknown word"))
        }
    }

    fn value(&mut self, depth: usize) -> Result<Canon, CodecError> {
        if depth > MAX_DEPTH {
            return Err(CodecError::TooDeep);
        }
        self.space();
        match self.peek() {
            None => Err(self.syntax("the data ends early")),
            Some(b'n') => self.literal(b"null", Canon::Null),
            Some(b't') => self.literal(b"true", Canon::Bool(true)),
            Some(b'f') => self.literal(b"false", Canon::Bool(false)),
            Some(b'"') => Ok(Canon::Str(self.string()?)),
            Some(b'[') => {
                self.at += 1;
                let mut items = Vec::new();
                self.space();
                if self.peek() == Some(b']') {
                    self.at += 1;
                    return Ok(Canon::Arr(items));
                }
                loop {
                    items.push(self.value(depth + 1)?);
                    self.space();
                    match self.peek() {
                        Some(b',') => self.at += 1,
                        Some(b']') => {
                            self.at += 1;
                            return Ok(Canon::Arr(items));
                        }
                        _ => return Err(self.syntax("expected , or ]")),
                    }
                }
            }
            Some(b'{') => {
                self.at += 1;
                let mut members: Vec<(String, Canon)> = Vec::new();
                self.space();
                if self.peek() == Some(b'}') {
                    self.at += 1;
                    return Ok(Canon::Obj(members));
                }
                loop {
                    self.space();
                    if self.peek() != Some(b'"') {
                        return Err(self.syntax("expected a key"));
                    }
                    let key = self.string()?;
                    self.space();
                    if self.peek() != Some(b':') {
                        return Err(self.syntax("expected :"));
                    }
                    self.at += 1;
                    let value = self.value(depth + 1)?;
                    if members.iter().any(|(k, _)| *k == key) {
                        return Err(CodecError::NotCanonical);
                    }
                    members.push((key, value));
                    self.space();
                    match self.peek() {
                        Some(b',') => self.at += 1,
                        Some(b'}') => {
                            self.at += 1;
                            return Ok(Canon::Obj(members));
                        }
                        _ => return Err(self.syntax("expected , or }")),
                    }
                }
            }
            Some(b'-' | b'0'..=b'9') => self.number(),
            Some(_) => Err(self.syntax("an unexpected character")),
        }
    }

    fn number(&mut self) -> Result<Canon, CodecError> {
        let negative = self.peek() == Some(b'-');
        if negative {
            self.at += 1;
        }
        let start = self.at;
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.at += 1;
        }
        let digits = &self.bytes[start..self.at];
        if digits.is_empty() {
            return Err(self.syntax("a number without digits"));
        }
        if matches!(self.peek(), Some(b'.' | b'e' | b'E')) {
            return Err(CodecError::Float);
        }
        if digits.len() > 1 && digits[0] == b'0' {
            return Err(self.syntax("a number with a leading zero"));
        }
        let mut magnitude: u128 = 0;
        for &d in digits {
            magnitude = magnitude
                .checked_mul(10)
                .and_then(|m| m.checked_add(u128::from(d - b'0')))
                .ok_or_else(|| self.syntax("a number out of range"))?;
        }
        if negative {
            if magnitude == 0 {
                return Err(CodecError::Float);
            }
            let value = 0i128
                .checked_sub_unsigned(magnitude)
                .and_then(|v| i64::try_from(v).ok())
                .ok_or_else(|| self.syntax("a number out of range"))?;
            Ok(Canon::Int(value))
        } else if let Ok(v) = i64::try_from(magnitude) {
            Ok(Canon::Int(v))
        } else {
            u64::try_from(magnitude)
                .map(Canon::UInt)
                .map_err(|_| self.syntax("a number out of range"))
        }
    }

    fn hex4(&mut self) -> Result<u32, CodecError> {
        let mut v = 0u32;
        for _ in 0..4 {
            let d = match self.peek() {
                Some(c @ b'0'..=b'9') => c - b'0',
                Some(c @ b'a'..=b'f') => c - b'a' + 10,
                Some(c @ b'A'..=b'F') => c - b'A' + 10,
                _ => return Err(self.syntax("a bad \\u escape")),
            };
            v = v * 16 + u32::from(d);
            self.at += 1;
        }
        Ok(v)
    }

    fn string(&mut self) -> Result<String, CodecError> {
        // At the opening quote.
        self.at += 1;
        let mut out: Vec<u8> = Vec::new();
        loop {
            let Some(b) = self.peek() else {
                return Err(self.syntax("a string without its closing quote"));
            };
            self.at += 1;
            match b {
                b'"' => break,
                b'\\' => {
                    let Some(e) = self.peek() else {
                        return Err(self.syntax("a string ends in an escape"));
                    };
                    self.at += 1;
                    match e {
                        b'"' => out.push(b'"'),
                        b'\\' => out.push(b'\\'),
                        b'/' => out.push(b'/'),
                        b'b' => out.push(0x08),
                        b'f' => out.push(0x0c),
                        b'n' => out.push(b'\n'),
                        b'r' => out.push(b'\r'),
                        b't' => out.push(b'\t'),
                        b'u' => {
                            let first = self.hex4()?;
                            let code = if (0xd800..0xdc00).contains(&first) {
                                if self.bytes.get(self.at..self.at + 2) != Some(b"\\u") {
                                    return Err(self.syntax("a lone surrogate"));
                                }
                                self.at += 2;
                                let second = self.hex4()?;
                                if !(0xdc00..0xe000).contains(&second) {
                                    return Err(self.syntax("a lone surrogate"));
                                }
                                0x10000 + ((first - 0xd800) << 10) + (second - 0xdc00)
                            } else {
                                first
                            };
                            let c = char::from_u32(code)
                                .ok_or_else(|| self.syntax("a lone surrogate"))?;
                            let mut buf = [0u8; 4];
                            out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                        }
                        _ => return Err(self.syntax("an unknown escape")),
                    }
                }
                0x00..=0x1f => return Err(self.syntax("a control character in a string")),
                _ => out.push(b),
            }
        }
        String::from_utf8(out).map_err(|_| self.syntax("a string that is not UTF-8"))
    }
}
