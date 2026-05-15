use crate::config::ProviderConfig;
use crate::error::{LiteLLMError, Result};
use crate::http::send_json;
use crate::providers::{merge_extra_body, resolve_api_key};
use crate::types::{
    ChatContentPart, ChatContentPartImageUrl, ChatContentPartText, ChatImageUrl, ChatMessage,
    ChatMessageContent, ChatRequest, ChatResponse, Reasoning, Usage,
};
use base64::{engine::general_purpose, Engine as _};
use mime_guess::MimeGuess;
use reqwest::header::CONTENT_TYPE;
use reqwest::Client;
use serde_json::Value;

const DEFAULT_VERSION: &str = "2023-06-01";

pub async fn chat(client: &Client, cfg: &ProviderConfig, req: ChatRequest) -> Result<ChatResponse> {
    let base = cfg
        .base_url
        .clone()
        .ok_or_else(|| LiteLLMError::Config("base_url required".into()))?;
    let url = format!("{}/v1/messages", base.trim_end_matches('/'));
    let key = resolve_api_key(cfg)?;
    let key = key.ok_or_else(|| LiteLLMError::MissingApiKey("ANTHROPIC_API_KEY".into()))?;

    let mut messages = req.messages.clone();
    let system_blocks = extract_system_blocks(client, &mut messages).await?;
    let mut anthropic_messages = Vec::with_capacity(messages.len());
    for message in messages {
        anthropic_messages.push(anthropic_message_from_chat(client, message).await?);
    }

    let body = build_anthropic_body(&req, anthropic_messages, system_blocks, false)?;

    let mut builder = client
        .post(url)
        .header("x-api-key", key)
        .header("anthropic-version", DEFAULT_VERSION)
        .json(&body);
    for (k, v) in &cfg.extra_headers {
        builder = builder.header(k, v);
    }

    let (parsed, _headers) = send_json::<Value>(builder).await?;
    validate_stop_reason(&parsed)?;
    let (content, reasoning) = extract_text_and_reasoning(&parsed);
    let usage = parse_usage(&parsed);

    Ok(ChatResponse {
        content,
        reasoning,
        usage,
        response_id: parsed
            .get("id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        header_cost: None,
        raw: None,
    })
}

pub async fn chat_stream(
    client: &Client,
    cfg: &ProviderConfig,
    req: ChatRequest,
) -> Result<crate::stream::ChatStream> {
    let base = cfg
        .base_url
        .clone()
        .ok_or_else(|| LiteLLMError::Config("base_url required".into()))?;
    let url = format!("{}/v1/messages", base.trim_end_matches('/'));
    let key = resolve_api_key(cfg)?;
    let key = key.ok_or_else(|| LiteLLMError::MissingApiKey("ANTHROPIC_API_KEY".into()))?;

    let mut messages = req.messages.clone();
    let system_blocks = extract_system_blocks(client, &mut messages).await?;
    let mut anthropic_messages = Vec::with_capacity(messages.len());
    for message in messages {
        anthropic_messages.push(anthropic_message_from_chat(client, message).await?);
    }

    let body = build_anthropic_body(&req, anthropic_messages, system_blocks, true)?;

    let mut builder = client
        .post(url)
        .header("x-api-key", key)
        .header("anthropic-version", DEFAULT_VERSION)
        .header("accept", "text/event-stream")
        .json(&body);
    for (k, v) in &cfg.extra_headers {
        builder = builder.header(k, v);
    }

    let resp = builder.send().await.map_err(LiteLLMError::from)?;
    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.map_err(LiteLLMError::from)?;
        return Err(LiteLLMError::http(format!(
            "http {}: {}",
            status.as_u16(),
            text
        )));
    }

    Ok(crate::stream::parse_anthropic_sse_stream(
        resp.bytes_stream(),
    ))
}

/// Build the Anthropic request body from a ChatRequest.
fn build_anthropic_body(
    req: &ChatRequest,
    messages: Vec<AnthropicMessage>,
    system_blocks: Vec<AnthropicContentBlock>,
    stream: bool,
) -> Result<Value> {
    let mut body = serde_json::json!({
        "model": req.model,
        "messages": messages,
        "max_tokens": req
            .max_tokens
            .or(req.max_completion_tokens)
            .unwrap_or(1024),
    });

    if stream {
        body["stream"] = serde_json::json!(true);
    }

    if !system_blocks.is_empty() {
        body["system"] = serde_json::json!(system_blocks);
    }
    if let Some(temp) = req.temperature {
        body["temperature"] = serde_json::json!(temp);
    }
    if let Some(top_p) = req.top_p {
        body["top_p"] = serde_json::json!(top_p);
    }
    if let Some(stop_sequences) = map_stop_sequences(req.stop.clone()) {
        body["stop_sequences"] = serde_json::json!(stop_sequences);
    }

    if let Some(output_config) =
        map_response_format_to_output_config(&req.model, req.response_format.as_ref())?
    {
        body["output_config"] = output_config;
    }

    if let Some(tools_value) = req.tools.clone() {
        body["tools"] = tools_value;
    }
    if let Some(tool_choice_value) = req.tool_choice.clone() {
        body["tool_choice"] = tool_choice_value;
    }

    if let Some(ref metadata) = req.metadata {
        body["metadata"] = metadata.clone();
    } else if let Some(ref user) = req.user {
        body["metadata"] = serde_json::json!({ "user_id": user });
    }

    merge_extra_body(
        &mut body,
        req.extra_body.as_ref(),
        &[
            "model",
            "messages",
            "max_tokens",
            "stream",
            "system",
            "temperature",
            "top_p",
            "stop_sequences",
            "output_config",
            "tools",
            "tool_choice",
            "metadata",
        ],
    )?;

    Ok(body)
}

fn map_stop_sequences(value: Option<Value>) -> Option<Vec<String>> {
    let value = value?;
    if let Some(s) = value.as_str() {
        let trimmed = s.trim();
        if trimmed.is_empty() {
            return None;
        }
        return Some(vec![trimmed.to_string()]);
    }
    if let Some(arr) = value.as_array() {
        let mut out = Vec::new();
        for item in arr {
            if let Some(s) = item.as_str() {
                let trimmed = s.trim();
                if !trimmed.is_empty() {
                    out.push(trimmed.to_string());
                }
            }
        }
        if out.is_empty() {
            None
        } else {
            Some(out)
        }
    } else {
        None
    }
}

fn map_response_format_to_output_config(
    model: &str,
    value: Option<&Value>,
) -> Result<Option<Value>> {
    let Some(value) = value else {
        return Ok(None);
    };
    let Some(type_value) = value.get("type").and_then(|v| v.as_str()) else {
        return Ok(None);
    };
    if type_value == "text" {
        return Ok(None);
    }
    if !model_supports_output_config(model) {
        return Err(LiteLLMError::Unsupported(format!(
            "anthropic response_format requires a Claude structured-output model; unsupported model `{model}`"
        )));
    }
    let schema = extract_json_schema(value)?;
    Ok(Some(serde_json::json!({
        "format": {
            "type": "json_schema",
            "schema": schema,
        }
    })))
}

fn extract_json_schema(value: &Value) -> Result<Value> {
    match value.get("type").and_then(|v| v.as_str()) {
        Some("json_schema") => value
            .get("json_schema")
            .and_then(|v| v.get("schema"))
            .cloned()
            .ok_or_else(|| {
                LiteLLMError::Config(
                    "anthropic response_format json_schema requires json_schema.schema".into(),
                )
            }),
        Some("json_object") => Ok(serde_json::json!({
            "type": "object",
            "properties": {},
            "additionalProperties": true,
        })),
        Some(other) => Err(LiteLLMError::Config(format!(
            "unsupported anthropic response_format type `{other}`"
        ))),
        None => Err(LiteLLMError::Config(
            "anthropic response_format requires a type".into(),
        )),
    }
}

fn model_supports_output_config(model: &str) -> bool {
    let lower = model.to_lowercase();
    lower.contains("sonnet-4.5")
        || lower.contains("sonnet-4-5")
        || lower.contains("sonnet-4.6")
        || lower.contains("sonnet-4-6")
        || lower.contains("haiku-4.5")
        || lower.contains("haiku-4-5")
        || lower.contains("opus-4.5")
        || lower.contains("opus-4-5")
        || lower.contains("opus-4.6")
        || lower.contains("opus-4-6")
        || lower.contains("opus-4.7")
        || lower.contains("opus-4-7")
        || lower.contains("mythos")
}

fn extract_text_and_reasoning(resp: &Value) -> (String, Option<Reasoning>) {
    if let Some(content) = resp.get("content").and_then(|v| v.as_array()) {
        let mut out = String::new();
        let mut reasoning_details = Vec::new();
        for part in content {
            if part.get("type").and_then(|v| v.as_str()) == Some("text") {
                if let Some(text) = part.get("text").and_then(|v| v.as_str()) {
                    out.push_str(text);
                }
            } else if part.get("type").and_then(|v| v.as_str()) == Some("thinking") {
                reasoning_details.push(part.clone());
            }
        }
        return (out, Reasoning::from_details(reasoning_details));
    }
    (
        resp.get("completion")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        None,
    )
}

fn extract_text(resp: &Value) -> String {
    extract_text_and_reasoning(resp).0
}

fn validate_stop_reason(resp: &Value) -> Result<()> {
    match resp.get("stop_reason").and_then(|v| v.as_str()) {
        Some("refusal") => Err(LiteLLMError::Refusal {
            text: extract_text(resp),
            usage: parse_usage(resp),
        }),
        Some("max_tokens") => Err(LiteLLMError::Truncated {
            text: extract_text(resp),
            usage: parse_usage(resp),
        }),
        _ => Ok(()),
    }
}

fn parse_usage(resp: &Value) -> Usage {
    let usage = resp.get("usage").and_then(|v| v.as_object());
    if let Some(u) = usage {
        return Usage {
            prompt_tokens: u.get("input_tokens").and_then(|v| v.as_u64()),
            completion_tokens: u.get("output_tokens").and_then(|v| v.as_u64()),
            thoughts_tokens: None,
            total_tokens: None,
            cost_usd: None,
        };
    }
    Usage::default()
}

#[derive(Debug, serde::Serialize)]
struct AnthropicMessage {
    role: String,
    content: Vec<AnthropicContentBlock>,
}

#[derive(Debug, serde::Serialize)]
#[serde(tag = "type")]
enum AnthropicContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image")]
    Image { source: AnthropicImageSource },
}

#[derive(Debug, serde::Serialize)]
#[serde(tag = "type")]
enum AnthropicImageSource {
    #[serde(rename = "base64")]
    Base64 { media_type: String, data: String },
    #[serde(rename = "url")]
    Url { url: String },
}

async fn extract_system_blocks(
    client: &Client,
    messages: &mut Vec<ChatMessage>,
) -> Result<Vec<AnthropicContentBlock>> {
    let mut blocks = Vec::new();
    let mut indices = Vec::new();
    for (idx, msg) in messages.iter().enumerate() {
        if msg.role == "system" {
            let mut msg_blocks = anthropic_blocks_from_content(client, &msg.content).await?;
            if msg_blocks.is_empty() {
                continue;
            }
            blocks.append(&mut msg_blocks);
            indices.push(idx);
        }
    }
    for idx in indices.into_iter().rev() {
        messages.remove(idx);
    }
    Ok(blocks)
}

async fn anthropic_message_from_chat(
    client: &Client,
    message: ChatMessage,
) -> Result<AnthropicMessage> {
    let role = match message.role.as_str() {
        "user" | "assistant" => message.role,
        other => {
            return Err(LiteLLMError::Config(format!(
                "unsupported anthropic role: {}",
                other
            )))
        }
    };
    let content = anthropic_blocks_from_content(client, &message.content).await?;
    Ok(AnthropicMessage { role, content })
}

async fn anthropic_blocks_from_content(
    client: &Client,
    content: &ChatMessageContent,
) -> Result<Vec<AnthropicContentBlock>> {
    match content {
        ChatMessageContent::Text(text) => Ok(vec![AnthropicContentBlock::Text {
            text: text.to_string(),
        }]),
        ChatMessageContent::Parts(parts) => {
            let mut out = Vec::new();
            for part in parts {
                match part {
                    ChatContentPart::Text(ChatContentPartText { text, .. }) => {
                        if text.is_empty() {
                            continue;
                        }
                        out.push(AnthropicContentBlock::Text { text: text.clone() });
                    }
                    ChatContentPart::ImageUrl(ChatContentPartImageUrl { image_url, .. }) => {
                        let source = anthropic_image_source(client, image_url).await?;
                        out.push(AnthropicContentBlock::Image { source });
                    }
                    ChatContentPart::InputAudio(_) | ChatContentPart::File(_) => {
                        return Err(LiteLLMError::Config(
                            "anthropic does not support input_audio/file parts".into(),
                        ));
                    }
                    ChatContentPart::Other(value) => {
                        return Err(LiteLLMError::Config(format!(
                            "unsupported anthropic content part: {}",
                            value
                        )));
                    }
                }
            }
            Ok(out)
        }
    }
}

async fn anthropic_image_source(
    client: &Client,
    image_url: &ChatImageUrl,
) -> Result<AnthropicImageSource> {
    let (url, format) = match image_url {
        ChatImageUrl::Url(url) => (url.as_str(), None),
        ChatImageUrl::Object(obj) => (obj.url.as_str(), obj.format.as_deref()),
    };

    if url.starts_with("https://") {
        return Ok(AnthropicImageSource::Url {
            url: url.to_string(),
        });
    }
    if url.starts_with("http://") {
        let resp = client.get(url).send().await.map_err(LiteLLMError::from)?;
        let headers = resp.headers().clone();
        let bytes = resp.bytes().await.map_err(LiteLLMError::from)?;
        let header_mime = headers
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|v| v.split(';').next().unwrap_or(v).trim().to_string());
        let media_type = format
            .map(|v| v.to_string())
            .or(header_mime)
            .or_else(|| infer_mime_from_url(url))
            .unwrap_or_else(|| "application/octet-stream".to_string());
        let data = general_purpose::STANDARD.encode(bytes);
        return Ok(AnthropicImageSource::Base64 { media_type, data });
    }

    let (media_type, data) = parse_data_url(url, format)?;
    Ok(AnthropicImageSource::Base64 { media_type, data })
}

fn infer_mime_from_url(url: &str) -> Option<String> {
    let path = url.split('?').next().unwrap_or(url);
    MimeGuess::from_path(path)
        .first_raw()
        .map(|m| m.to_string())
}

fn parse_data_url(url: &str, override_format: Option<&str>) -> Result<(String, String)> {
    if url.starts_with("data:") {
        let stripped = url.strip_prefix("data:").unwrap_or(url);
        if let Some((meta, data)) = stripped.split_once(',') {
            let mut media_type = meta.split(';').next().unwrap_or("application/octet-stream");
            if let Some(fmt) = override_format {
                media_type = fmt;
            }
            let data = data.to_string();
            return Ok((media_type.to_string(), data));
        }
    }

    if let Some(fmt) = override_format {
        return Ok((fmt.to_string(), url.to_string()));
    }

    Err(LiteLLMError::Config(
        "expected data URL for anthropic image; provide data:...;base64,... or format".into(),
    ))
}
