//! Parameters checked against a description: the one check both the host
//! (before it calls and hashes) and the module (before it generates) run.
use crate::v1::{Description, Param, ParamType, ParamValue, Params, Problem};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// A problem with `field`.
pub fn problem(code: &str, field: &str, message: String) -> Problem {
    Problem {
        code: code.into(),
        field: Some(field.into()),
        message,
    }
}

/// A length in mm for messages, e.g. `0.45 mm`, without floats.
pub fn mm_text(nm: i64) -> String {
    let sign = if nm < 0 { "-" } else { "" };
    let abs = nm.unsigned_abs();
    let whole = abs / 1_000_000;
    let frac = abs % 1_000_000;
    let mut digits = format!("{frac:06}");
    while digits.len() > 2 && digits.ends_with('0') {
        digits.pop();
    }
    format!("{sign}{whole}.{digits} mm")
}

/// Every declared parameter present, of its type and in its range, and
/// nothing else; otherwise every problem found, in plain words.
pub fn check(description: &Description, params: &Params) -> Result<(), Vec<Problem>> {
    let mut problems = Vec::new();
    for key in params.keys() {
        if !description.params.iter().any(|p| &p.key == key) {
            problems.push(problem(
                "param.unknown",
                key,
                format!("This generator has no parameter \"{key}\"."),
            ));
        }
    }
    for param in &description.params {
        match params.get(&param.key) {
            None => problems.push(problem(
                "param.missing",
                &param.key,
                format!(
                    "{} is missing; the request must give every parameter.",
                    param.label
                ),
            )),
            Some(value) => {
                if let Err(p) = check_value(param, &param.ty, value) {
                    problems.push(p);
                }
            }
        }
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems)
    }
}

/// Whether `value` is of type `ty` and in its range.
pub fn fits(ty: &ParamType, value: &ParamValue) -> bool {
    let param = Param {
        key: String::new(),
        label: String::new(),
        help: None,
        ty: ty.clone(),
        default: value.clone(),
        group: None,
        context: None,
    };
    check_value(&param, ty, value).is_ok()
}

fn check_value(param: &Param, ty: &ParamType, value: &ParamValue) -> Result<(), Problem> {
    let label = &param.label;
    let key = &param.key;
    let wrong = |kind: &str| problem("param.type", key, format!("{label} must be {kind}."));
    match (ty, value) {
        (
            ParamType::Length {
                min_nm,
                max_nm,
                step_nm,
            },
            ParamValue::Int(v),
        ) => {
            if v < min_nm || v > max_nm {
                return Err(problem(
                    "param.out_of_range",
                    key,
                    format!(
                        "{label} {} is outside this generator's range, {} to {}.",
                        mm_text(*v),
                        mm_text(*min_nm),
                        mm_text(*max_nm)
                    ),
                ));
            }
            if let Some(step) = step_nm
                && *step > 0
                && v.checked_rem(*step) != Some(0)
            {
                return Err(problem(
                    "param.out_of_range",
                    key,
                    format!("{label} must be a multiple of {}.", mm_text(*step)),
                ));
            }
            Ok(())
        }
        (
            ParamType::Angle {
                min_mdeg: min,
                max_mdeg: max,
            },
            ParamValue::Int(v),
        )
        | (ParamType::Count { min, max }, ParamValue::Int(v)) => {
            if v < min || v > max {
                Err(problem(
                    "param.out_of_range",
                    key,
                    format!("{label} {v} is outside this generator's range, {min} to {max}."),
                ))
            } else {
                Ok(())
            }
        }
        (ParamType::Enum { choices }, ParamValue::Text(v)) => {
            if choices.iter().any(|c| &c.key == v) {
                Ok(())
            } else {
                let keys: Vec<&str> = choices.iter().map(|c| c.key.as_str()).collect();
                Err(problem(
                    "param.out_of_range",
                    key,
                    format!("{label} \"{v}\" is not one of: {}.", keys.join(", ")),
                ))
            }
        }
        (ParamType::Bool, ParamValue::Bool(_)) => Ok(()),
        (ParamType::Text { max_chars }, ParamValue::Text(v)) => {
            if v.chars().count() > *max_chars as usize {
                Err(problem(
                    "param.out_of_range",
                    key,
                    format!("{label} is longer than {max_chars} characters."),
                ))
            } else {
                Ok(())
            }
        }
        (ParamType::Quantity { .. }, ParamValue::Text(_)) => Ok(()),
        (ParamType::List { of, max_len }, ParamValue::List(items)) => {
            if items.len() > *max_len as usize {
                return Err(problem(
                    "param.out_of_range",
                    key,
                    format!("{label} has more than {max_len} entries."),
                ));
            }
            items
                .iter()
                .try_for_each(|item| check_value(param, of, item))
        }
        (ParamType::Length { .. }, _) => Err(wrong("a length in nm")),
        (ParamType::Angle { .. }, _) => Err(wrong("an angle in millidegrees")),
        (ParamType::Count { .. }, _) => Err(wrong("a whole number")),
        (ParamType::Enum { .. }, _) => Err(wrong("one of its listed choices")),
        (ParamType::Bool, _) => Err(wrong("true or false")),
        (ParamType::Text { .. }, _) => Err(wrong("text")),
        (ParamType::Quantity { .. }, _) => Err(wrong("a decimal quantity as text")),
        (ParamType::List { .. }, _) => Err(wrong("a list")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_mm_without_floats() {
        assert_eq!(mm_text(450_000), "0.45 mm");
        assert_eq!(mm_text(1_600_000), "1.60 mm");
        assert_eq!(mm_text(-50_000), "-0.05 mm");
        assert_eq!(mm_text(1_234_567), "1.234567 mm");
    }
}
