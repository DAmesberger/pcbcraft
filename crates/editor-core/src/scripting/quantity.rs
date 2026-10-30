//! Checked quantity arithmetic. Unit definitions are supplied as data, not
//! selected by component names. This proof supports linear SI units only.
use crate::diagnostics::{Error, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// SI exponents: length, mass, time, current, temperature, amount, luminosity.
pub type Dimension = [i8; 7];
pub const LENGTH: Dimension = [1, 0, 0, 0, 0, 0, 0];

fn failure(message: &str) -> Error {
    Error::Invalid(message.into())
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UnitDefinition {
    pub symbol: String,
    pub dimension: Dimension,
    /// Multiply by this positive factor to obtain an SI base-unit value.
    pub scale: f64,
}

/// A result independent of Rhai. Construction and every operation are checked.
#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct Quantity {
    pub(crate) value: f64,
    pub(crate) dimension: Dimension,
}

impl Quantity {
    pub fn value(&self) -> f64 {
        self.value
    }
    pub fn dimension(&self) -> Dimension {
        self.dimension
    }

    pub(super) fn new(value: f64, dimension: Dimension) -> Result<Self> {
        if !value.is_finite() || value.abs() > 1e100 {
            return Err(failure(
                "quantity must be finite and within ±1e100 SI units",
            ));
        }
        if dimension.iter().any(|d| !(-16..=16).contains(d)) {
            return Err(failure("dimension exponent exceeds ±16"));
        }
        Ok(Self { value, dimension })
    }

    pub(super) fn add(self, other: Self) -> Result<Self> {
        if self.dimension != other.dimension {
            return Err(failure("cannot add quantities with different dimensions"));
        }
        Self::new(self.value + other.value, self.dimension)
    }

    pub(super) fn sub(self, other: Self) -> Result<Self> {
        self.add(Self::new(-other.value, other.dimension)?)
    }

    pub(super) fn product(self, other: Self, divide: bool) -> Result<Self> {
        if divide && other.value == 0.0 {
            return Err(failure("quantity division by zero"));
        }
        let mut dimension = [0; 7];
        for ((out, a), b) in dimension
            .iter_mut()
            .zip(self.dimension)
            .zip(other.dimension)
        {
            *out = if divide {
                a.checked_sub(b)
            } else {
                a.checked_add(b)
            }
            .ok_or_else(|| failure("dimension exponent overflow"))?;
        }
        let value = if divide {
            self.value / other.value
        } else {
            self.value * other.value
        };
        if value == 0.0 && self.value != 0.0 && other.value != 0.0 {
            return Err(failure("quantity arithmetic underflow"));
        }
        Self::new(value, dimension)
    }

    pub(super) fn scale(self, factor: f64) -> Result<Self> {
        self.product(Self::new(factor, [0; 7])?, false)
    }

    pub(super) fn scale_integer(self, factor: i64) -> Result<Self> {
        if !(-9_007_199_254_740_992..=9_007_199_254_740_992).contains(&factor) {
            return Err(failure(
                "integer quantity multiplier exceeds exact floating-point range",
            ));
        }
        self.scale(factor as f64)
    }

    pub(super) fn millimetres(&self) -> Result<f64> {
        if self.dimension != LENGTH {
            return Err(failure("pad geometry requires a length quantity"));
        }
        let mm = self.value * 1000.0;
        if !mm.is_finite() || mm.abs() > 10000.0 {
            return Err(failure("pad geometry exceeds ±10000 mm"));
        }
        Ok(mm)
    }
}
