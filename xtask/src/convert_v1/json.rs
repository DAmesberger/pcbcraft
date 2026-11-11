//! JSON edits shared by every slice's conversion: millimetres to whole
//! nanometres and degrees to whole millidegrees, rounded half away from
//! zero exactly as the app's one crossing (`units::Nm::from_mm_f64_rounded`,
//! `units::Angle::from_degrees_f64_rounded`) does. Each edit reports how many
//! values it changed and leaves an already converted value alone, so a
//! second run changes nothing.
// The U2 domain conversions use these as they land; the last removes this.
#![allow(dead_code)]
use crate::Result;
use serde_json::{Map, Value};

/// `units::MAX_COORD_NM`.
const MAX_COORD_NM: f64 = 2_000_000_000.0;
/// `units::MILLIDEGREES_PER_TURN`.
const MILLIDEGREES_PER_TURN: i64 = 360_000;

/// Millimetres to whole nanometres.
pub fn nm(mm: &Value) -> Result<i64> {
    let mm = mm.as_f64().ok_or_else(|| format!("{mm} is not a number"))?;
    let nm = (mm * 1_000_000.0).round();
    if !nm.is_finite() || nm.abs() > MAX_COORD_NM {
        return Err(format!("{mm} mm is outside ±2 m").into());
    }
    // Integral and within ±2e9: the conversion is exact.
    Ok(nm as i64)
}

/// Degrees to whole millidegrees, reduced to one turn (`units::Angle`).
pub fn millidegrees(degrees: &Value) -> Result<i64> {
    let degrees = degrees
        .as_f64()
        .ok_or_else(|| format!("{degrees} is not a number"))?;
    if !degrees.is_finite() || degrees.abs() > 1e12 {
        return Err(format!("{degrees}° is not a finite angle").into());
    }
    // Within ±1e15: the cast is lossless.
    Ok(((degrees * 1_000.0).round() as i64).rem_euclid(MILLIDEGREES_PER_TURN))
}

/// `[x, y]` millimetres to `{"x_nm", "y_nm"}`.
pub fn point_value(value: &Value) -> Result<Option<Value>> {
    let Some([x, y]) = value.as_array().map(Vec::as_slice) else {
        return Ok(None);
    };
    Ok(Some(serde_json::json!({"x_nm": nm(x)?, "y_nm": nm(y)?})))
}

/// The point at `key`, in place; 1 if it changed.
pub fn point(object: &mut Map<String, Value>, key: &str) -> Result<usize> {
    let Some(value) = object.get_mut(key) else {
        return Ok(0);
    };
    match point_value(value)? {
        Some(converted) => {
            *value = converted;
            Ok(1)
        }
        None => Ok(0),
    }
}

/// The point at `from`, moved to `to`; 1 if it changed.
pub fn point_as(object: &mut Map<String, Value>, from: &str, to: &str) -> Result<usize> {
    let Some(value) = object.get(from) else {
        return Ok(0);
    };
    let Some(converted) = point_value(value)? else {
        return Ok(0);
    };
    object.remove(from);
    object.insert(to.into(), converted);
    Ok(1)
}

/// Every point of the array at `key`; the number changed.
pub fn points(object: &mut Map<String, Value>, key: &str) -> Result<usize> {
    let Some(items) = object.get_mut(key).and_then(Value::as_array_mut) else {
        return Ok(0);
    };
    let mut n = 0;
    for item in items {
        if let Some(converted) = point_value(item)? {
            *item = converted;
            n += 1;
        }
    }
    Ok(n)
}

/// A millimetre length at `from` to nanometres at `to`; 1 if it changed.
/// With `omit_zero`, a zero length is left out, as the app writes it. A
/// `null` (an absent optional) moves as `null`.
pub fn length(
    object: &mut Map<String, Value>,
    from: &str,
    to: &str,
    omit_zero: bool,
) -> Result<usize> {
    let Some(value) = object.remove(from) else {
        return Ok(0);
    };
    if value.is_null() {
        object.insert(to.into(), Value::Null);
        return Ok(1);
    }
    let nm = nm(&value)?;
    if !(omit_zero && nm == 0) {
        object.insert(to.into(), Value::from(nm));
    }
    Ok(1)
}

/// Every millimetre length of the array at `from`, to nanometres at `to`.
pub fn lengths(object: &mut Map<String, Value>, from: &str, to: &str) -> Result<usize> {
    let Some(Value::Array(items)) = object.remove(from) else {
        return Ok(0);
    };
    let converted = items.iter().map(nm).collect::<Result<Vec<_>>>()?;
    object.insert(to.into(), Value::from(converted));
    Ok(1)
}

/// Degrees at `from` to millidegrees at `to`; 1 if it changed. With
/// `omit_zero`, a zero angle is left out.
pub fn angle(
    object: &mut Map<String, Value>,
    from: &str,
    to: &str,
    omit_zero: bool,
) -> Result<usize> {
    let Some(value) = object.remove(from) else {
        return Ok(0);
    };
    let mdeg = millidegrees(&value)?;
    if !(omit_zero && mdeg == 0) {
        object.insert(to.into(), Value::from(mdeg));
    }
    Ok(1)
}

/// Degrees at `from` to whole millidegrees at `to`, NOT reduced to one turn:
/// a sweep (up to a full turn, which a rotation would reduce to 0); 1 if it
/// changed.
pub fn sweep(object: &mut Map<String, Value>, from: &str, to: &str) -> Result<usize> {
    let Some(value) = object.remove(from) else {
        return Ok(0);
    };
    let degrees = value
        .as_f64()
        .ok_or_else(|| format!("{value} is not a number"))?;
    let mdeg = (degrees * 1_000.0).round();
    if !mdeg.is_finite() || mdeg.abs() > f64::from(i32::MAX) {
        return Err(format!("{degrees}° is not a finite sweep").into());
    }
    // Integral and within i32: the cast is exact.
    object.insert(to.into(), Value::from(mdeg as i64));
    Ok(1)
}

/// Every angle of the array at `from`, degrees to millidegrees reduced to
/// one turn, at `to`; 1 if it changed.
pub fn angles(object: &mut Map<String, Value>, from: &str, to: &str) -> Result<usize> {
    let Some(Value::Array(items)) = object.remove(from) else {
        return Ok(0);
    };
    let converted = items.iter().map(millidegrees).collect::<Result<Vec<_>>>()?;
    object.insert(to.into(), Value::from(converted));
    Ok(1)
}

/// Bounds `[[x, y, z], [x, y, z]]` in millimetres at `from`, to nanometre
/// arrays at `to`; 1 if it changed.
pub fn bounds(object: &mut Map<String, Value>, from: &str, to: &str) -> Result<usize> {
    let Some(Value::Array(corners)) = object.remove(from) else {
        return Ok(0);
    };
    let converted = corners
        .iter()
        .map(|corner| {
            corner
                .as_array()
                .ok_or_else(|| format!("{corner} is not a corner").into())
                .and_then(|c| c.iter().map(nm).collect::<Result<Vec<_>>>())
        })
        .collect::<Result<Vec<_>>>()?;
    object.insert(to.into(), Value::from(converted));
    Ok(1)
}

/// The objects of the array at `key`.
pub fn items<'a>(
    object: &'a mut Map<String, Value>,
    key: &str,
) -> impl Iterator<Item = &'a mut Map<String, Value>> {
    object
        .get_mut(key)
        .and_then(Value::as_array_mut)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object_mut)
}

/// The object at `key`.
pub fn object<'a>(
    object: &'a mut Map<String, Value>,
    key: &str,
) -> Option<&'a mut Map<String, Value>> {
    object.get_mut(key).and_then(Value::as_object_mut)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn rounds_as_the_app_does() -> Result<()> {
        assert_eq!(nm(&json!(0.1))?, 100_000);
        assert_eq!(nm(&json!(-1.2345675))?, -1_234_568);
        assert_eq!(nm(&json!(1e-7))?, 0);
        assert!(nm(&json!(2000.5)).is_err());
        assert_eq!(millidegrees(&json!(-90.0))?, 270_000);
        assert_eq!(millidegrees(&json!(360.0))?, 0);
        assert_eq!(millidegrees(&json!(12.3456))?, 12_346);
        Ok(())
    }

    #[test]
    fn edits_once() -> Result<()> {
        let mut value = json!({"at": [1.0, 2.0], "w": 0.25, "r": 90.0, "ps": [[0.0, 0.5]]});
        let o = value.as_object_mut().ok_or("object")?;
        assert_eq!(
            point(o, "at")?
                + length(o, "w", "w_nm", false)?
                + angle(o, "r", "r_mdeg", true)?
                + points(o, "ps")?,
            4
        );
        assert_eq!(
            point(o, "at")?
                + length(o, "w", "w_nm", false)?
                + angle(o, "r", "r_mdeg", true)?
                + points(o, "ps")?,
            0
        );
        assert_eq!(
            value,
            json!({"at": {"x_nm": 1_000_000, "y_nm": 2_000_000}, "w_nm": 250_000, "r_mdeg": 90_000, "ps": [{"x_nm": 0, "y_nm": 500_000}]})
        );
        Ok(())
    }
}
