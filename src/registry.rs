use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use crate::error::{LiteLLMError, Result};

#[derive(Debug, Clone)]
pub struct ModelPricing {
    pub input_cost_per_1k: Option<f64>,
    pub output_cost_per_1k: Option<f64>,
    pub max_input_tokens: Option<u32>,
    pub max_output_tokens: Option<u32>,
    pub mode: Option<String>,
    pub provider: Option<String>,
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
                models.insert(
                    name.to_string(),
                    ModelPricing {
                        input_cost_per_1k: input,
                        output_cost_per_1k: output,
                        max_input_tokens: max_input,
                        max_output_tokens: max_output,
                        mode,
                        provider,
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

    supported.contains(&model.to_ascii_lowercase().replace('.', "-"))
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
}
