//! Spend accounting: a configurable price table and the usage a model
//! reports, so a run on a priced model has a hard cost cap computed from the
//! responses themselves, not from an estimate after the fact.
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// USD per million tokens of one model.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Price {
    pub input: f64,
    pub output: f64,
    /// A 5-minute cache write (1.25 × input).
    pub cache_write: f64,
    pub cache_read: f64,
}

impl Price {
    /// A model with no price: nothing is booked, and the review says so.
    pub const FREE: Self = Self {
        input: 0.0,
        output: 0.0,
        cache_write: 0.0,
        cache_read: 0.0,
    };

    /// The cost of one response's usage, USD.
    pub fn cost(&self, usage: &Usage) -> f64 {
        let m = 1e-6;
        usage.input_tokens as f64 * self.input * m
            + usage.output_tokens as f64 * self.output * m
            + usage.cache_write_tokens as f64 * self.cache_write * m
            + usage.cache_read_tokens as f64 * self.cache_read * m
    }

    /// The dearest rate a prompt token can cost (an uncached write).
    pub fn worst_input(&self) -> f64 {
        self.input.max(self.cache_write)
    }
}

/// Prices by model id. The defaults are Anthropic's first-party list
/// prices as of October 2026 (cache writes at the 5-minute rate);
/// `--prices` replaces them with a JSON object of the same shape.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(transparent)]
pub struct PriceTable(pub BTreeMap<String, Price>);

impl Default for PriceTable {
    fn default() -> Self {
        let p = |input: f64, output: f64, cache_read: f64| Price {
            input,
            output,
            cache_write: input * 1.25,
            cache_read,
        };
        Self(BTreeMap::from([
            ("claude-opus-5-5".into(), p(4.0, 20.0, 0.20)),
            ("claude-sonnet-5-5".into(), p(2.0, 10.0, 0.20)),
            ("claude-haiku-5-5".into(), p(0.10, 0.50, 0.01)),
            ("claude-opus-5".into(), p(5.0, 25.0, 0.50)),
            ("claude-opus-4-8".into(), p(5.0, 25.0, 0.50)),
            ("claude-sonnet-5".into(), p(2.0, 10.0, 0.20)),
        ]))
    }
}

impl PriceTable {
    /// The price of `model`; a model without a price is refused, because
    /// the cost cap could not be enforced.
    pub fn of(&self, model: &str) -> Result<Price> {
        self.0.get(model).copied().ok_or_else(|| {
            Error::Config(format!(
                "no price for model {model}; add it to the price table (--prices)"
            ))
        })
    }

    /// Parse a JSON price table, checking every rate is finite and not
    /// negative.
    pub fn parse(text: &str) -> Result<Self> {
        let table: Self = serde_json::from_str(text)?;
        for (model, p) in &table.0 {
            let ok = [p.input, p.output, p.cache_write, p.cache_read]
                .iter()
                .all(|v| v.is_finite() && *v >= 0.0);
            if !ok {
                return Err(Error::Config(format!("invalid price for {model}")));
            }
        }
        Ok(table)
    }
}

/// Token counts of one response, in neutral names.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Usage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub cache_write_tokens: u64,
    #[serde(default)]
    pub cache_read_tokens: u64,
}

impl Usage {
    /// The whole prompt: uncached, written and read.
    pub fn prompt(&self) -> u64 {
        self.input_tokens
            .saturating_add(self.cache_write_tokens)
            .saturating_add(self.cache_read_tokens)
    }

    pub fn add(&mut self, other: &Self) {
        self.input_tokens = self.input_tokens.saturating_add(other.input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(other.output_tokens);
        self.cache_write_tokens = self
            .cache_write_tokens
            .saturating_add(other.cache_write_tokens);
        self.cache_read_tokens = self
            .cache_read_tokens
            .saturating_add(other.cache_read_tokens);
    }
}

/// Spend so far and the caps it is checked against.
#[derive(Clone, Debug, Default, Serialize, PartialEq)]
pub struct Ledger {
    pub spent_usd: f64,
    pub usage: Usage,
    pub requests: u32,
}

impl Ledger {
    /// Book one response at `price` (the dearer of the requested model's
    /// and a fallback model's price, when a fallback served it).
    pub fn book(&mut self, usage: &Usage, price: &Price) -> f64 {
        let cost = price.cost(usage);
        self.spent_usd += cost;
        self.usage.add(usage);
        self.requests = self.requests.saturating_add(1);
        cost
    }

    /// Whether a request of at most `prompt_tokens` prompt tokens and
    /// `max_tokens` output tokens could still be paid within `cap`.
    pub fn affords(&self, price: &Price, prompt_tokens: u64, max_tokens: u64, cap: f64) -> bool {
        let worst =
            (prompt_tokens as f64 * price.worst_input() + max_tokens as f64 * price.output) * 1e-6;
        self.spent_usd + worst <= cap
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_response_costs_its_four_token_kinds() -> Result<()> {
        let price = PriceTable::default().of("claude-opus-5-5")?;
        let usage = Usage {
            input_tokens: 1_000,
            output_tokens: 2_000,
            cache_write_tokens: 100_000,
            cache_read_tokens: 1_000_000,
        };
        // 0.004 + 0.04 + 0.5 + 0.2
        assert!((price.cost(&usage) - 0.744).abs() < 1e-9);
        let mut ledger = Ledger::default();
        ledger.book(&usage, &price);
        assert!(ledger.affords(&price, 0, 0, 0.75));
        // 10k output at $20/M is $0.20: over a $0.80 cap.
        assert!(!ledger.affords(&price, 0, 10_000, 0.80));
        Ok(())
    }

    #[test]
    fn unknown_models_and_bad_prices_are_refused() {
        assert!(PriceTable::default().of("some-model").is_err());
        assert!(
            PriceTable::parse(
                r#"{"m": {"input": -1, "output": 1, "cache_write": 1, "cache_read": 1}}"#
            )
            .is_err()
        );
        assert!(
            PriceTable::parse(
                r#"{"m": {"input": 1, "output": 1, "cache_write": 1, "cache_read": 1}}"#
            )
            .is_ok()
        );
    }
}
