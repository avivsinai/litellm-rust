use crate::config::ProviderConfig;
use crate::error::{LiteLLMError, Result};
use serde_json::{Map, Value};
use std::env;

pub mod anthropic;
pub mod gemini;
pub mod openai_compat;

pub fn resolve_api_key(cfg: &ProviderConfig) -> Result<Option<String>> {
    if cfg.no_auth {
        return Ok(None);
    }
    if let Some(key) = cfg.api_key.clone() {
        return Ok(Some(key));
    }
    if let Some(env_key) = cfg.api_key_env.as_deref() {
        let key =
            env::var(env_key).map_err(|_| LiteLLMError::MissingApiKey(env_key.to_string()))?;
        return Ok(Some(key));
    }
    Err(LiteLLMError::MissingApiKey("<unset>".into()))
}

pub(crate) fn merge_extra_body(
    body: &mut Value,
    extra_body: Option<&Map<String, Value>>,
    protected_fields: &[&str],
) -> Result<()> {
    let Some(extra_body) = extra_body else {
        return Ok(());
    };
    let body_map = body
        .as_object_mut()
        .ok_or_else(|| LiteLLMError::Config("request body must be a JSON object".into()))?;

    for (key, value) in extra_body {
        if protected_fields.iter().any(|field| *field == key) {
            return Err(LiteLLMError::Config(format!(
                "extra_body cannot override protected request field `{key}`"
            )));
        }
        if is_denied_extra_body_key(key) {
            return Err(LiteLLMError::Config(format!(
                "extra_body contains unsupported internal field `{key}`"
            )));
        }
        body_map.insert(key.clone(), value.clone());
    }

    Ok(())
}

fn is_denied_extra_body_key(key: &str) -> bool {
    key.starts_with("litellm_")
        || matches!(
            key,
            "vector_store_id"
                | "vector_store_ids"
                | "extra_body"
                | "extra_headers"
                | "extra_query"
                | "reasoning"
                | "reasoning_content"
                | "reasoning_details"
        )
}
