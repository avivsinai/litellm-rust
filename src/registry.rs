use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use crate::error::{LiteLLMError, Result};

#[derive(Debug, Clone)]
pub struct ModelPricing {
    pub input_cost_per_1k: Option<f64>,
    pub output_cost_per_1k: Option<f64>,
    /// Cost per 1k tokens read from the prompt cache, converted from the
    /// upstream `cache_read_input_token_cost` per-token field using the same
    /// per-token -> per-1k convention as `input_cost_per_1k`. When that field
    /// is absent, the equivalent `input_cost_per_token_cache_hit` field is
    /// read instead. No current registry entry needs this fallback (entries
    /// that publish the cache-hit field also publish the cache-read field);
    /// it guards against an upstream regression that drops the latter.
    pub cache_read_cost_per_1k: Option<f64>,
    /// Cost per 1k tokens written to the prompt cache, converted from the
    /// upstream `cache_creation_input_token_cost` per-token field.
    pub cache_creation_cost_per_1k: Option<f64>,
    pub max_input_tokens: Option<u32>,
    pub max_output_tokens: Option<u32>,
    pub mode: Option<String>,
    pub provider: Option<String>,
    pub supports_function_calling: Option<bool>,
    pub supports_tool_choice: Option<bool>,
    pub supports_vision: Option<bool>,
    pub supports_reasoning: Option<bool>,
    pub supports_prompt_caching: Option<bool>,
    pub supports_response_schema: Option<bool>,
    pub supports_parallel_function_calling: Option<bool>,
    pub supported_endpoints: Option<Vec<String>>,
    pub supported_modalities: Option<Vec<String>>,
}

#[derive(Debug, Clone)]
pub struct Registry {
    pub models: HashMap<String, ModelPricing>,
}

impl Registry {
    pub fn load_embedded() -> Result<Self> {
        Self::from_json_str(include_str!("../data/model_prices_and_context_window.json"))
    }

    pub fn from_json_str(raw: &str) -> Result<Self> {
        let json: Value = serde_json::from_str(raw)
            .map_err(|e| LiteLLMError::Parse(format!("model registry: {e}")))?;
        let mut models = HashMap::new();
        let map = json
            .as_object()
            .ok_or_else(|| LiteLLMError::Parse("model registry not an object".into()))?;
        for (name, entry) in map {
            if name == "sample_spec" {
                continue;
            }
            if let Some(obj) = entry.as_object() {
                let input = obj
                    .get("input_cost_per_token")
                    .and_then(|v| v.as_f64())
                    .map(|v| v * 1000.0);
                let output = obj
                    .get("output_cost_per_token")
                    .and_then(|v| v.as_f64())
                    .map(|v| v * 1000.0);
                let cache_read = obj
                    .get("cache_read_input_token_cost")
                    .and_then(|v| v.as_f64())
                    .or_else(|| {
                        obj.get("input_cost_per_token_cache_hit")
                            .and_then(|v| v.as_f64())
                    })
                    .map(|v| v * 1000.0);
                let cache_creation = obj
                    .get("cache_creation_input_token_cost")
                    .and_then(|v| v.as_f64())
                    .map(|v| v * 1000.0);
                let max_input = obj
                    .get("max_input_tokens")
                    .or_else(|| obj.get("max_tokens"))
                    .and_then(|v| v.as_u64())
                    .and_then(|v| u32::try_from(v).ok());
                let max_output = obj
                    .get("max_output_tokens")
                    .and_then(|v| v.as_u64())
                    .and_then(|v| u32::try_from(v).ok());
                let mode = obj
                    .get("mode")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let provider = obj
                    .get("litellm_provider")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let bool_field = |field: &str| obj.get(field).and_then(Value::as_bool);
                let string_list_field = |field: &str| {
                    obj.get(field).and_then(Value::as_array).map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(|s| s.to_string()))
                            .collect::<Vec<_>>()
                    })
                };
                models.insert(
                    name.to_string(),
                    ModelPricing {
                        input_cost_per_1k: input,
                        output_cost_per_1k: output,
                        cache_read_cost_per_1k: cache_read,
                        cache_creation_cost_per_1k: cache_creation,
                        max_input_tokens: max_input,
                        max_output_tokens: max_output,
                        mode,
                        provider,
                        supports_function_calling: bool_field("supports_function_calling"),
                        supports_tool_choice: bool_field("supports_tool_choice"),
                        supports_vision: bool_field("supports_vision"),
                        supports_reasoning: bool_field("supports_reasoning"),
                        supports_prompt_caching: bool_field("supports_prompt_caching"),
                        supports_response_schema: bool_field("supports_response_schema"),
                        supports_parallel_function_calling: bool_field(
                            "supports_parallel_function_calling",
                        ),
                        supported_endpoints: string_list_field("supported_endpoints"),
                        supported_modalities: string_list_field("supported_modalities"),
                    },
                );
            }
        }
        Ok(Self { models })
    }

    pub fn get(&self, model: &str) -> Option<&ModelPricing> {
        self.models.get(model)
    }

    pub fn estimate_cost(&self, model: &str, input_tokens: u32, output_tokens: u32) -> Option<f64> {
        let pricing = self.models.get(model)?;
        let input = pricing
            .input_cost_per_1k
            .map(|v| v * input_tokens as f64 / 1000.0)?;
        let output = pricing
            .output_cost_per_1k
            .map(|v| v * output_tokens as f64 / 1000.0)?;
        Some(input + output)
    }

    /// Estimates cost including prompt-cache read and cache-creation tokens.
    ///
    /// Cache read tokens are priced at `cache_read_cost_per_1k` and cache
    /// creation tokens at `cache_creation_cost_per_1k` when the model
    /// publishes that pricing. When a model has no cache-specific pricing,
    /// cache tokens fall back to being billed at the model's normal input
    /// rate (`input_cost_per_1k`) rather than being treated as free, since
    /// providers bill uncached-equivalent tokens at the input rate whenever
    /// they don't publish a discounted cache rate.
    ///
    /// Cache-creation tokens are priced at the base cache-write rate, which
    /// is the 5-minute TTL rate. Writes with a 1-hour TTL and long-context
    /// tier rates (for example `*_above_200k_tokens` prices) are not modeled,
    /// so the estimate is low for those requests.
    ///
    /// # Token accounting contract
    ///
    /// `input_tokens`, `cache_read_tokens`, and `cache_creation_tokens` must
    /// be disjoint counts that sum to the total prompt tokens billed —
    /// `input_tokens` must **exclude** any tokens already counted in
    /// `cache_read_tokens` or `cache_creation_tokens`.
    ///
    /// Providers report prompt tokens in different shapes, and the crate's
    /// [`Usage`](crate::types::Usage) `prompt_tokens` keeps each provider's
    /// shape as reported:
    ///
    /// - **Anthropic**: `input_tokens` already excludes
    ///   `cache_read_input_tokens` and `cache_creation_input_tokens`, which
    ///   are separate fields. Pass these counts directly.
    /// - **OpenAI-compatible**: `prompt_tokens` (or `input_tokens` on the
    ///   Responses API) includes cached tokens; `cached_tokens` in the
    ///   details object is a subset of it, not an additive count.
    /// - **Gemini**: `promptTokenCount` includes `cachedContentTokenCount`.
    ///
    /// For OpenAI-compatible and Gemini usage, subtract the cached count from
    /// the prompt count before you pass it as `input_tokens`. If you do not,
    /// the cached tokens are billed twice: once at the input rate and once
    /// at the cache rate.
    pub fn estimate_cost_with_cache(
        &self,
        model: &str,
        input_tokens: u32,
        output_tokens: u32,
        cache_read_tokens: u32,
        cache_creation_tokens: u32,
    ) -> Option<f64> {
        let pricing = self.models.get(model)?;
        let input = pricing
            .input_cost_per_1k
            .map(|v| v * input_tokens as f64 / 1000.0)?;
        let output = pricing
            .output_cost_per_1k
            .map(|v| v * output_tokens as f64 / 1000.0)?;
        let cache_read_rate = pricing
            .cache_read_cost_per_1k
            .or(pricing.input_cost_per_1k)?;
        let cache_creation_rate = pricing
            .cache_creation_cost_per_1k
            .or(pricing.input_cost_per_1k)?;
        let cache_read = cache_read_rate * cache_read_tokens as f64 / 1000.0;
        let cache_creation = cache_creation_rate * cache_creation_tokens as f64 / 1000.0;
        Some(input + output + cache_read + cache_creation)
    }
}

pub(crate) fn embedded_model_supports_output_config(model: &str) -> bool {
    static SUPPORTED_MODELS: OnceLock<HashSet<String>> = OnceLock::new();

    let supported = SUPPORTED_MODELS.get_or_init(|| {
        let raw = include_str!("../data/model_prices_and_context_window.json");
        let Ok(Value::Object(models)) = serde_json::from_str::<Value>(raw) else {
            return HashSet::new();
        };
        models
            .into_iter()
            .filter_map(|(name, metadata)| {
                metadata
                    .get("supports_output_config")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                    .then(|| name.to_ascii_lowercase().replace('.', "-"))
            })
            .collect()
    });

    let normalized = model.to_ascii_lowercase().replace('.', "-");
    if supported.contains(&normalized) {
        return true;
    }

    let Some((base, suffix)) = normalized.rsplit_once('-') else {
        return false;
    };
    suffix.len() == 8
        && suffix.bytes().all(|byte| byte.is_ascii_digit())
        && supported.contains(base)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_registry() {
        let registry = Registry::load_embedded().unwrap();
        assert!(!registry.models.is_empty());
    }

    #[test]
    fn parses_registry_from_json_string() {
        let registry = Registry::from_json_str(
            r#"{
                "sample_spec": {},
                "test/model": {
                    "input_cost_per_token": 0.000001,
                    "output_cost_per_token": 0.000002,
                    "max_input_tokens": 4096,
                    "max_output_tokens": 1024,
                    "mode": "chat",
                    "litellm_provider": "test"
                }
            }"#,
        )
        .unwrap();

        let model = registry.get("test/model").unwrap();
        assert_eq!(model.input_cost_per_1k, Some(0.001));
        assert_eq!(model.output_cost_per_1k, Some(0.002));
        assert_eq!(model.max_input_tokens, Some(4096));
        assert_eq!(model.max_output_tokens, Some(1024));
    }

    #[test]
    fn parses_cache_costs_and_capability_flags() {
        let registry = Registry::from_json_str(
            r#"{
                "sample_spec": {},
                "test/cached": {
                    "input_cost_per_token": 0.000001,
                    "output_cost_per_token": 0.000005,
                    "cache_read_input_token_cost": 0.0000001,
                    "cache_creation_input_token_cost": 0.00000125,
                    "mode": "chat",
                    "litellm_provider": "test",
                    "supports_function_calling": true,
                    "supports_tool_choice": true,
                    "supports_vision": false,
                    "supports_reasoning": true,
                    "supports_prompt_caching": true,
                    "supports_response_schema": true,
                    "supports_parallel_function_calling": false,
                    "supported_endpoints": ["/v1/chat/completions", "/v1/responses"],
                    "supported_modalities": ["text", "image"],
                    "some_unknown_future_field": {"nested": true}
                }
            }"#,
        )
        .unwrap();

        let model = registry.get("test/cached").unwrap();
        assert!((model.cache_read_cost_per_1k.unwrap() - 0.0001).abs() < 1e-12);
        assert!((model.cache_creation_cost_per_1k.unwrap() - 0.00125).abs() < 1e-12);
        assert_eq!(model.supports_function_calling, Some(true));
        assert_eq!(model.supports_tool_choice, Some(true));
        assert_eq!(model.supports_vision, Some(false));
        assert_eq!(model.supports_reasoning, Some(true));
        assert_eq!(model.supports_prompt_caching, Some(true));
        assert_eq!(model.supports_response_schema, Some(true));
        assert_eq!(model.supports_parallel_function_calling, Some(false));
        assert_eq!(
            model.supported_endpoints,
            Some(vec![
                "/v1/chat/completions".to_string(),
                "/v1/responses".to_string()
            ])
        );
        assert_eq!(
            model.supported_modalities,
            Some(vec!["text".to_string(), "image".to_string()])
        );
    }

    #[test]
    fn falls_back_to_cache_hit_field_when_cache_read_cost_is_absent() {
        // No current registry entry publishes only
        // `input_cost_per_token_cache_hit`; this guards the fallback against an
        // upstream regression that drops `cache_read_input_token_cost`.
        let registry = Registry::from_json_str(
            r#"{
                "sample_spec": {},
                "test/cache-hit-only": {
                    "input_cost_per_token": 0.0000005,
                    "output_cost_per_token": 0.000002,
                    "input_cost_per_token_cache_hit": 0.00000014,
                    "mode": "chat",
                    "litellm_provider": "test"
                }
            }"#,
        )
        .unwrap();

        let model = registry.get("test/cache-hit-only").unwrap();
        assert!((model.cache_read_cost_per_1k.unwrap() - 0.00014).abs() < 1e-12);
    }

    #[test]
    fn estimate_cost_with_cache_uses_published_cache_rates() {
        let registry = Registry::from_json_str(
            r#"{
                "test/cached": {
                    "input_cost_per_token": 0.000001,
                    "output_cost_per_token": 0.000005,
                    "cache_read_input_token_cost": 0.0000001,
                    "cache_creation_input_token_cost": 0.00000125
                }
            }"#,
        )
        .unwrap();

        let cost = registry
            .estimate_cost_with_cache("test/cached", 1000, 1000, 1000, 1000)
            .unwrap();
        // input: 1000/1000 * 0.001 = 0.001, output: 1000/1000 * 0.005 = 0.005
        // cache_read: 1000/1000 * 0.0001 = 0.0001, cache_creation: 1000/1000 * 0.00125 = 0.00125
        let expected = 0.001 + 0.005 + 0.0001 + 0.00125;
        assert!((cost - expected).abs() < 1e-12);

        // Without any cache tokens, the cache-aware estimate must match the
        // plain estimate exactly.
        let plain = registry.estimate_cost("test/cached", 1000, 1000).unwrap();
        let with_zero_cache = registry
            .estimate_cost_with_cache("test/cached", 1000, 1000, 0, 0)
            .unwrap();
        assert!((plain - with_zero_cache).abs() < 1e-12);
    }

    #[test]
    fn estimate_cost_with_cache_falls_back_to_input_rate_when_uncached() {
        let registry = Registry::from_json_str(
            r#"{
                "test/uncached": {
                    "input_cost_per_token": 0.000002,
                    "output_cost_per_token": 0.000004
                }
            }"#,
        )
        .unwrap();

        // No cache-specific pricing published: cache read/creation tokens
        // must be billed at the model's normal input rate.
        let cost = registry
            .estimate_cost_with_cache("test/uncached", 0, 0, 500, 500)
            .unwrap();
        let input_rate = registry
            .get("test/uncached")
            .unwrap()
            .input_cost_per_1k
            .unwrap();
        let expected = input_rate * 500.0 / 1000.0 * 2.0;
        assert!((cost - expected).abs() < 1e-12);
    }

    /// Structural check that cache-pricing and capability-flag parsing wires
    /// through to real entries in the embedded registry, without pinning to
    /// specific model names that may rotate on refresh.
    #[test]
    fn embedded_registry_capability_flags_are_populated() {
        let registry = Registry::load_embedded().unwrap();

        let function_calling_models = registry
            .models
            .values()
            .filter(|m| m.supports_function_calling == Some(true))
            .count();
        assert!(
            function_calling_models > 100,
            "expected broad supports_function_calling coverage, found {function_calling_models}"
        );

        let cache_priced_models = registry
            .models
            .values()
            .filter(|m| m.cache_read_cost_per_1k.is_some())
            .count();
        assert!(
            cache_priced_models > 100,
            "expected broad cache-read pricing coverage, found {cache_priced_models}"
        );

        let vision_models = registry
            .models
            .values()
            .filter(|m| m.supports_vision == Some(true))
            .count();
        assert!(
            vision_models > 100,
            "expected broad supports_vision coverage, found {vision_models}"
        );

        let with_modalities = registry
            .models
            .values()
            .filter(|m| {
                m.supported_modalities
                    .as_ref()
                    .is_some_and(|v| !v.is_empty())
            })
            .count();
        assert!(
            with_modalities > 0,
            "expected at least one model with supported_modalities populated"
        );
    }

    #[test]
    fn token_counts_beyond_u32_are_dropped_not_truncated() {
        let registry = Registry::from_json_str(
            r#"{
                "sample_spec": {},
                "test/overflow": {
                    "max_input_tokens": 5000000000,
                    "max_output_tokens": 4294967296,
                    "mode": "chat"
                }
            }"#,
        )
        .unwrap();

        let model = registry.get("test/overflow").unwrap();
        assert_eq!(model.max_input_tokens, None);
        assert_eq!(model.max_output_tokens, None);
    }

    #[test]
    fn embedded_registry_retains_metadata_coverage() {
        let registry = Registry::load_embedded().unwrap();
        let priced_models = registry
            .models
            .values()
            .filter(|model| model.output_cost_per_1k.is_some())
            .count();
        let models_with_context = registry
            .models
            .values()
            .filter(|model| model.max_input_tokens.is_some())
            .count();

        assert!(
            priced_models > 1000,
            "expected broad completion-pricing coverage, found {priced_models} models"
        );
        assert!(
            models_with_context > 1000,
            "expected broad context-window coverage, found {models_with_context} models"
        );
    }

    #[test]
    fn embedded_capabilities_drive_anthropic_output_config_support() {
        assert!(embedded_model_supports_output_config("claude-sonnet-5"));
        assert!(embedded_model_supports_output_config("claude-opus-4.8"));
        assert!(!embedded_model_supports_output_config(
            "claude-3-7-sonnet-20250219"
        ));
        assert!(!embedded_model_supports_output_config(
            "claude-3-5-sonnet-20241022"
        ));
    }

    #[test]
    fn embedded_capabilities_retry_without_one_trailing_date_token() {
        assert!(embedded_model_supports_output_config(
            "claude-opus-4.8-20260315"
        ));
    }

    #[test]
    fn embedded_capabilities_do_not_promote_unflagged_dated_models() {
        assert!(!embedded_model_supports_output_config(
            "claude-haiku-4-5-20260711"
        ));
    }

    #[test]
    fn embedded_capabilities_leave_non_dated_misses_unsupported() {
        assert!(!embedded_model_supports_output_config(
            "claude-sonnet-5-latest"
        ));
    }

    #[test]
    fn embedded_capabilities_strip_at_most_one_trailing_date_token() {
        assert!(!embedded_model_supports_output_config(
            "claude-sonnet-5-20260710-20260711"
        ));
    }
}
