//! What the user configures: a connection (endpoint, key), a model with its
//! declared capabilities, and a run's budget. Nothing here is detected, and
//! no behaviour above the backends depends on a vendor's name.
use super::wire::AuthStyle;
use super::{Endpoint, KeySource};
use crate::cost::{Price, PriceTable};
use crate::{Error, Result};
use std::time::Duration;

/// Endpoint of the Messages API.
pub const ANTHROPIC_BASE_URL: &str = "https://api.anthropic.com";

/// Which wire format an endpoint speaks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Provider {
    /// Anthropic Messages.
    Anthropic,
    /// OpenAI-compatible chat completions: Ollama, LM Studio, `llama-server`,
    /// OpenAI, OpenRouter or any such URL.
    OpenAiCompatible,
}

impl Provider {
    /// `anthropic` or `openai` (the CLI flag).
    pub fn parse(text: &str) -> Result<Self> {
        match text {
            "anthropic" => Ok(Self::Anthropic),
            "openai" => Ok(Self::OpenAiCompatible),
            other => Err(Error::Config(format!(
                "provider {other:?}: use anthropic or openai (any OpenAI-compatible endpoint)"
            ))),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Anthropic => "Anthropic Messages",
            Self::OpenAiCompatible => "OpenAI-compatible",
        }
    }

    pub(super) fn auth_style(self) -> AuthStyle {
        match self {
            Self::Anthropic => AuthStyle::ApiKeyHeader,
            Self::OpenAiCompatible => AuthStyle::Bearer,
        }
    }
}

/// What a configured model can do, as declared.
#[derive(Clone, Debug, PartialEq)]
pub struct Capabilities {
    pub tool_calling: bool,
    /// It accepts images.
    pub vision: bool,
    /// It reads PDF documents itself.
    pub native_pdf: bool,
    /// It takes prompt-cache breakpoints.
    pub caching: bool,
    /// It takes the adaptive thinking parameter.
    pub adaptive_thinking: bool,
    /// It takes the server-side refusal fallback.
    pub server_fallback: bool,
    /// Prompt plus output tokens one request may hold.
    pub context_tokens: u64,
    /// USD per million tokens; `None` when unknown or free.
    pub price: Option<Price>,
}

/// Output tokens per request. These come from the model's configuration and
/// stay near what it supports, because reasoning counts against them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OutputLimits {
    pub per_request: u32,
    /// For the request after one that was cut off at its limit.
    pub after_cut: u32,
}

impl OutputLimits {
    /// The defaults for a declared context: half of it, up to 32 000 tokens,
    /// and three quarters of it, up to 64 000, after a cut-off.
    pub fn for_context(context_tokens: u64) -> Self {
        let part = |n: u64, d: u64, top: u32| {
            u32::try_from(context_tokens.saturating_mul(n) / d)
                .unwrap_or(top)
                .min(top)
        };
        Self {
            per_request: part(1, 2, 32_000),
            after_cut: part(3, 4, 64_000),
        }
    }
}

/// A model as configured.
#[derive(Clone, Debug, PartialEq)]
pub struct ModelProfile {
    pub provider: Provider,
    pub model: String,
    pub capabilities: Capabilities,
    pub output: OutputLimits,
    /// The effort or reasoning level, when one is set.
    pub effort: Option<String>,
    /// Longest wait for one response. Requests are not streamed.
    pub request_timeout: Duration,
    /// Refuse to run without a price, because this endpoint bills and the
    /// cost cap could not be enforced.
    pub price_required: bool,
}

/// Model ids with a declared profile on the Messages API.
const ANTHROPIC_CONTEXT: u64 = 1_000_000;
const FALLBACK_MODELS: &[&str] = &[
    "claude-opus-5-5",
    "claude-sonnet-5-5",
    "claude-opus-5",
    "claude-fable-5-1",
];

impl ModelProfile {
    /// A model on the Messages API. Its capabilities are the ones that API
    /// declares for its current models; `price` comes from `prices` by
    /// exact id. The limits are today's: 32 000 output tokens a request and
    /// 64 000 after a cut-off, for a 30-minute non-streaming wait.
    pub fn anthropic(model: &str, prices: &PriceTable) -> Self {
        Self {
            provider: Provider::Anthropic,
            model: model.to_owned(),
            capabilities: Capabilities {
                tool_calling: true,
                vision: true,
                native_pdf: true,
                caching: true,
                adaptive_thinking: true,
                server_fallback: FALLBACK_MODELS.contains(&model),
                context_tokens: ANTHROPIC_CONTEXT,
                price: prices.of(model).ok(),
            },
            output: OutputLimits {
                per_request: 32_000,
                after_cut: 64_000,
            },
            effort: Some("high".into()),
            request_timeout: Duration::from_secs(30 * 60),
            price_required: true,
        }
    }

    /// A model on an OpenAI-compatible endpoint, with what the user declared.
    /// `context_tokens` is what the server gives this model, not what the
    /// weights could take. Tool calling is assumed until declared otherwise;
    /// everything else stays off until declared.
    pub fn openai_compatible(model: &str, context_tokens: u64) -> Self {
        Self {
            provider: Provider::OpenAiCompatible,
            model: model.to_owned(),
            capabilities: Capabilities {
                tool_calling: true,
                vision: false,
                native_pdf: false,
                caching: false,
                adaptive_thinking: false,
                server_fallback: false,
                context_tokens,
                price: None,
            },
            output: OutputLimits::for_context(context_tokens),
            effort: None,
            // A local model at 17 tokens a second takes 31 minutes for 32 000.
            request_timeout: Duration::from_secs(90 * 60),
            price_required: false,
        }
    }

    /// Check the configuration before any request.
    pub fn check(&self) -> Result<()> {
        if self.model.trim().is_empty() {
            return Err(Error::Config("give a model name".into()));
        }
        if self.capabilities.context_tokens == 0 {
            return Err(Error::Config(format!(
                "declare the context size of {} (the tokens the server gives it)",
                self.model
            )));
        }
        if self.output.per_request == 0 {
            return Err(Error::Config("the output limit must be above 0".into()));
        }
        if self.price_required && self.capabilities.price.is_none() {
            return Err(Error::Config(format!(
                "no price for model {}; add it to the price table (--prices)",
                self.model
            )));
        }
        Ok(())
    }

    /// One line for the review report.
    pub fn declared(&self) -> String {
        let c = &self.capabilities;
        let yes_no = |b: bool| if b { "yes" } else { "no" };
        format!(
            "tools {}, vision {}, native PDF {}, caching {}, context {} tokens, output {} tokens a request",
            yes_no(c.tool_calling),
            yes_no(c.vision),
            yes_no(c.native_pdf),
            yes_no(c.caching),
            c.context_tokens,
            self.output.per_request,
        )
    }
}

/// An endpoint and where its key comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Connection {
    pub endpoint: Endpoint,
    pub key: KeySource,
}

/// What one run may use. The USD cap applies when a price is known; the
/// token, time and request caps apply to every model, local ones included.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Budget {
    pub max_usd: f64,
    pub max_requests: u32,
    /// Output tokens of the whole run.
    pub max_output_tokens: u64,
    /// Prompt plus output tokens of the whole run, every request counted.
    pub max_total_tokens: u64,
    pub max_duration: Duration,
}

impl Default for Budget {
    fn default() -> Self {
        Self {
            max_usd: crate::DEFAULT_MAX_USD,
            max_requests: 60,
            max_output_tokens: 400_000,
            max_total_tokens: 40_000_000,
            max_duration: Duration::from_secs(6 * 60 * 60),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_limits_scale_with_the_context_and_stay_generous() {
        // A small context: half for the output, three quarters after a cut.
        let small = OutputLimits::for_context(16_384);
        assert_eq!((small.per_request, small.after_cut), (8_192, 12_288));
        let large = OutputLimits::for_context(262_144);
        assert_eq!((large.per_request, large.after_cut), (32_000, 64_000));
        // Far above the 806 tokens a local tool call took with room, and the
        // 800 at which the same call was cut off.
        assert!(OutputLimits::for_context(8_192).per_request > 4 * 800);
    }

    #[test]
    fn capabilities_are_declared_never_inferred_from_a_name() {
        // A Claude id on an OpenAI-compatible proxy gets what was declared.
        let p = ModelProfile::openai_compatible("claude-opus-5-5", 100_000);
        assert!(!p.capabilities.native_pdf && !p.capabilities.caching);
        assert!(p.capabilities.price.is_none());
    }

    #[test]
    fn an_unpriced_billed_model_and_an_undeclared_context_are_refused() {
        let prices = PriceTable::default();
        assert!(
            ModelProfile::anthropic("claude-opus-5-5", &prices)
                .check()
                .is_ok()
        );
        assert!(
            ModelProfile::anthropic("unpriced", &prices)
                .check()
                .is_err()
        );
        assert!(ModelProfile::openai_compatible("local", 0).check().is_err());
        assert!(ModelProfile::openai_compatible(" ", 1000).check().is_err());
        assert!(
            ModelProfile::openai_compatible("local", 1000)
                .check()
                .is_ok()
        );
    }
}
