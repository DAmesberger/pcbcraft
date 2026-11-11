//! Canonical JSON: the one encoder and decoder the host and every module use.
//!
//! Canonical form:
//! - object keys sorted by their UTF-8 bytes, each key once;
//! - no whitespace outside strings;
//! - numbers are integers (`i64` or `u64`) in shortest decimal form; no
//!   floats, exponents, `-0` or leading zeros;
//! - strings escaped as `serde_json` writes them (`\"`, `\\`, control
//!   characters), everything else literal UTF-8; with the `nfc` feature,
//!   strings must also be in Unicode NFC;
//! - absent optional fields are omitted, never `null`.
//!
//! [`decode`] accepts only bytes that [`encode`] would write for the decoded
//! value, so two different byte strings never decode to the same value.
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use sha2::{Digest, Sha256};

/// Deepest nesting of arrays and objects the codec accepts.
pub const MAX_DEPTH: usize = 64;

/// Why bytes were not canonical JSON of the expected shape, in plain words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CodecError {
    /// The input is longer than the caller's bound.
    TooLarge { bytes: usize, limit: usize },
    /// The bytes are not JSON.
    Syntax(String),
    /// A number is not an integer.
    Float,
    /// Nesting deeper than [`MAX_DEPTH`].
    TooDeep,
    /// A string is not in Unicode NFC (with the `nfc` feature).
    NotNfc,
    /// Valid JSON, but not in canonical form (key order, whitespace,
    /// duplicate keys, `null` for an absent field, number spelling).
    NotCanonical,
    /// Canonical JSON whose shape does not match the expected type.
    Shape(String),
}

impl fmt::Display for CodecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge { bytes, limit } => {
                write!(f, "the data is {bytes} bytes, more than the {limit} allowed")
            }
            Self::Syntax(detail) => write!(f, "the data is not valid JSON ({detail})"),
            Self::Float => f.write_str("a number is not a whole number; only integers are allowed"),
            Self::TooDeep => write!(f, "the data nests deeper than {MAX_DEPTH} levels"),
            Self::NotNfc => f.write_str("a text value is not in Unicode normal form C"),
            Self::NotCanonical => f.write_str(
                "the data is not in canonical form (sorted keys, no spaces, no nulls, each key once)",
            ),
            Self::Shape(detail) => write!(f, "the data has the wrong shape ({detail})"),
        }
    }
}

/// The canonical bytes of `value`.
pub fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, CodecError> {
    let value = serde_json::to_value(value).map_err(|e| CodecError::Shape(e.to_string()))?;
    let mut out = Vec::new();
    write_value(&value, &mut out, 0)?;
    Ok(out)
}

/// Decode canonical bytes of at most `limit` bytes into `T`.
pub fn decode<T: DeserializeOwned + Serialize>(
    bytes: &[u8],
    limit: usize,
) -> Result<T, CodecError> {
    if bytes.len() > limit {
        return Err(CodecError::TooLarge {
            bytes: bytes.len(),
            limit,
        });
    }
    let value: Value =
        serde_json::from_slice(bytes).map_err(|e| CodecError::Syntax(e.to_string()))?;
    let mut again = Vec::with_capacity(bytes.len());
    write_value(&value, &mut again, 0)?;
    if again != bytes {
        return Err(CodecError::NotCanonical);
    }
    let typed: T = serde_json::from_value(value).map_err(|e| CodecError::Shape(e.to_string()))?;
    // A field the type fills by default, or a `null` it reads as absent,
    // would encode differently: refuse it as not canonical.
    if encode(&typed)? != bytes {
        return Err(CodecError::NotCanonical);
    }
    Ok(typed)
}

fn write_value(value: &Value, out: &mut Vec<u8>, depth: usize) -> Result<(), CodecError> {
    if depth > MAX_DEPTH {
        return Err(CodecError::TooDeep);
    }
    match value {
        Value::Null => out.extend_from_slice(b"null"),
        Value::Bool(true) => out.extend_from_slice(b"true"),
        Value::Bool(false) => out.extend_from_slice(b"false"),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                out.extend_from_slice(i.to_string().as_bytes());
            } else if let Some(u) = n.as_u64() {
                out.extend_from_slice(u.to_string().as_bytes());
            } else {
                return Err(CodecError::Float);
            }
        }
        Value::String(s) => write_string(s, out)?,
        Value::Array(items) => {
            out.push(b'[');
            for (k, item) in items.iter().enumerate() {
                if k > 0 {
                    out.push(b',');
                }
                write_value(item, out, depth + 1)?;
            }
            out.push(b']');
        }
        Value::Object(map) => {
            let mut entries: Vec<(&String, &Value)> = map.iter().collect();
            entries.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
            out.push(b'{');
            for (k, (key, item)) in entries.into_iter().enumerate() {
                if k > 0 {
                    out.push(b',');
                }
                write_string(key, out)?;
                out.push(b':');
                write_value(item, out, depth + 1)?;
            }
            out.push(b'}');
        }
    }
    Ok(())
}

fn write_string(s: &str, out: &mut Vec<u8>) -> Result<(), CodecError> {
    #[cfg(feature = "nfc")]
    if !unicode_normalization::is_nfc(s) {
        return Err(CodecError::NotNfc);
    }
    let quoted = serde_json::to_vec(s).map_err(|e| CodecError::Shape(e.to_string()))?;
    out.extend_from_slice(&quoted);
    Ok(())
}

/// SHA-256 of `bytes`.
pub fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

/// The result hash: SHA-256 of a generation's canonical bytes.
pub fn result_hash(canonical_generation: &[u8]) -> [u8; 32] {
    sha256(canonical_generation)
}

/// Lower-case hexadecimal of `bytes`.
pub fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(char::from(DIGITS[usize::from(b >> 4)]));
        s.push(char::from(DIGITS[usize::from(b & 0x0f)]));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::collections::BTreeMap;
    use alloc::vec;

    #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Sample {
        zeta: i64,
        alpha: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note: Option<String>,
        map: BTreeMap<String, i64>,
    }

    fn sample() -> Sample {
        let mut map = BTreeMap::new();
        map.insert("b".into(), -2);
        map.insert("a".into(), 1);
        Sample {
            zeta: 7,
            alpha: "Ω \"q\"\n".into(),
            note: None,
            map,
        }
    }

    #[test]
    fn encodes_sorted_compact_and_round_trips() -> Result<(), CodecError> {
        let bytes = encode(&sample())?;
        assert_eq!(
            core::str::from_utf8(&bytes).unwrap_or(""),
            r#"{"alpha":"Ω \"q\"\n","map":{"a":1,"b":-2},"zeta":7}"#
        );
        assert_eq!(decode::<Sample>(&bytes, 1024)?, sample());
        Ok(())
    }

    #[test]
    fn refuses_every_non_canonical_spelling() {
        let refused = [
            (
                &br#"{"zeta":7,"alpha":"x","map":{}}"#[..],
                CodecError::NotCanonical,
            ),
            (
                &br#"{"alpha":"x", "map":{},"zeta":7}"#[..],
                CodecError::NotCanonical,
            ),
            (
                &br#"{"alpha":"x","map":{},"note":null,"zeta":7}"#[..],
                CodecError::NotCanonical,
            ),
            (
                &br#"{"alpha":"x","map":{},"zeta":7.0}"#[..],
                CodecError::Float,
            ),
            (
                &br#"{"alpha":"x","map":{},"zeta":7e0}"#[..],
                CodecError::Float,
            ),
            // serde_json reads `-0` as a float.
            (
                &br#"{"alpha":"x","map":{},"zeta":-0}"#[..],
                CodecError::Float,
            ),
            (
                &br#"{"alpha":"x","alpha":"x","map":{},"zeta":7}"#[..],
                CodecError::NotCanonical,
            ),
            // An escape where the character itself is canonical.
            (
                &br#"{"alpha":"\/","map":{},"zeta":7}"#[..],
                CodecError::NotCanonical,
            ),
        ];
        for (bytes, expected) in refused {
            assert_eq!(
                decode::<Sample>(bytes, 1024).err(),
                Some(expected),
                "{}",
                core::str::from_utf8(bytes).unwrap_or("")
            );
        }
        assert!(matches!(
            decode::<Sample>(br#"{"alpha":"x","map":{},"other":1,"zeta":7}"#, 1024),
            Err(CodecError::Shape(_))
        ));
        assert!(matches!(
            decode::<Sample>(b"{", 1024),
            Err(CodecError::Syntax(_))
        ));
    }

    #[test]
    fn bounds_size_and_depth() {
        let long = vec![b' '; 2048];
        assert_eq!(
            decode::<Value>(&long, 1024).err(),
            Some(CodecError::TooLarge {
                bytes: 2048,
                limit: 1024
            })
        );
        let mut deep = Value::Bool(true);
        for _ in 0..=MAX_DEPTH {
            deep = Value::Array(vec![deep]);
        }
        assert_eq!(encode(&deep).err(), Some(CodecError::TooDeep));
    }

    #[cfg(feature = "nfc")]
    #[test]
    fn nfc_refuses_decomposed_text() {
        // "é" as e + combining acute: not NFC.
        assert_eq!(
            decode::<String>("\"e\u{301}\"".as_bytes(), 64).err(),
            Some(CodecError::NotNfc)
        );
        assert!(decode::<String>("\"\u{e9}\"".as_bytes(), 64).is_ok());
    }

    #[test]
    fn hashes_are_sha256_in_hex() {
        assert_eq!(
            hex(&sha256(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
