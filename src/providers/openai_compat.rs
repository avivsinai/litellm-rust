use crate::config::ProviderConfig;
use crate::error::{LiteLLMError, Result};
use crate::http::send_json;
use crate::providers::{merge_extra_body, resolve_api_key};
use crate::stream::{parse_responses_sse_stream, parse_sse_stream, ChatStream};
use crate::types::{
    ChatRequest, ChatResponse, EmbeddingRequest, EmbeddingResponse, ImageData, ImageRequest,
    ImageResponse, Reasoning, Usage, VideoRequest, VideoResponse,
};
use base64::{engine::general_purpose, Engine as _};
use reqwest::multipart::Form;
use reqwest::Client;
use serde::Deserialize;
use serde_json::Value;
use std::time::Duration;
use tokio::time::sleep;

/// Default maximum polling attempts for video generation (120 * 5s = 10 minutes)
pub const DEFAULT_VIDEO_MAX_POLL_ATTEMPTS: u32 = 120;
/// Default polling interval for video generation status checks
pub const DEFAULT_VIDEO_POLL_INTERVAL_SECS: u64 = 5;

#[derive(Debug, Deserialize)]
struct OpenAIChatResponse {
    id: Option<String>,
    choices: Vec<OpenAIChoice>,
    usage: Option<OpenAIUsage>,
}

#[derive(Debug, Deserialize)]
struct OpenAIChoice {
    message: OpenAIMessage,
}

#[derive(Debug, Deserialize)]
struct OpenAIMessage {
    content: Option<Value>,
    reasoning_content: Option<String>,
    reasoning: Option<String>,
    reasoning_details: Option<Value>,
    /// Raw OpenAI-format tool_calls array, surfaced unchanged on `ChatResponse`.
    /// Optional because non-tool-using replies omit it entirely.
    #[serde(default)]
    tool_calls: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct OpenAIUsage {
    prompt_tokens: Option<u64>,
    completion_tokens: Option<u64>,
    total_tokens: Option<u64>,
    cost: Option<Value>,
    completion_tokens_details: Option<CompletionTokensDetails>,
}

#[derive(Debug, Deserialize)]
struct CompletionTokensDetails {
    reasoning_tokens: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct OpenAIEmbeddingResponse {
    data: Vec<OpenAIEmbeddingItem>,
    usage: Option<OpenAIUsage>,
}

#[derive(Debug, Deserialize)]
struct OpenAIEmbeddingItem {
    embedding: Vec<f32>,
}

/// Build the chat request body from a ChatRequest.
///
/// This is shared between streaming and non-streaming chat calls.
fn build_chat_body(req: &ChatRequest, stream: bool) -> Result<Value> {
    let mut body = serde_json::json!({
        "model": req.model,
        "messages": req.messages,
    });

    if stream {
        body["stream"] = serde_json::json!(true);
    }

    if let Some(temp) = req.temperature {
        body["temperature"] = serde_json::json!(temp);
    }
    if let Some(max_tokens) = req.max_tokens {
        body["max_tokens"] = serde_json::json!(max_tokens);
    }
    if let Some(ref fmt) = req.response_format {
        body["response_format"] = fmt.clone();
    }
    if let Some(max_completion_tokens) = req.max_completion_tokens {
        body["max_completion_tokens"] = serde_json::json!(max_completion_tokens);
    }
    if let Some(ref tools) = req.tools {
        body["tools"] = tools.clone();
    }
    if let Some(ref tool_choice) = req.tool_choice {
        body["tool_choice"] = tool_choice.clone();
    }
    if let Some(parallel) = req.parallel_tool_calls {
        body["parallel_tool_calls"] = serde_json::json!(parallel);
    }
    if let Some(ref stop) = req.stop {
        body["stop"] = stop.clone();
    }
    if let Some(top_p) = req.top_p {
        body["top_p"] = serde_json::json!(top_p);
    }
    if let Some(presence) = req.presence_penalty {
        body["presence_penalty"] = serde_json::json!(presence);
    }
    if let Some(frequency) = req.frequency_penalty {
        body["frequency_penalty"] = serde_json::json!(frequency);
    }
    if let Some(seed) = req.seed {
        body["seed"] = serde_json::json!(seed);
    }
    if let Some(ref user) = req.user {
        body["user"] = serde_json::json!(user);
    }
    if let Some(ref metadata) = req.metadata {
        body["metadata"] = metadata.clone();
    }
    if let Some(ref reasoning_effort) = req.reasoning_effort {
        body["reasoning_effort"] = reasoning_effort.clone();
    }
    if let Some(ref thinking) = req.thinking {
        body["thinking"] = thinking.clone();
    }

    merge_extra_body(
        &mut body,
        req.extra_body.as_ref(),
        &[
            "model",
            "messages",
            "stream",
            "temperature",
            "max_tokens",
            "response_format",
            "max_completion_tokens",
            "tools",
            "tool_choice",
            "parallel_tool_calls",
            "stop",
            "top_p",
            "presence_penalty",
            "frequency_penalty",
            "seed",
            "user",
            "metadata",
            "reasoning_effort",
            "thinking",
        ],
    )?;

    Ok(body)
}

fn is_astra(model: &str) -> bool {
    model == "gpt-6-astra" || model.starts_with("gpt-6-astra-")
}

fn responses_content(role: &str, content: &crate::types::ChatMessageContent) -> Result<Value> {
    match content {
        crate::types::ChatMessageContent::Text(text) => Ok(Value::String(text.clone())),
        crate::types::ChatMessageContent::Parts(parts) => {
            let mapped = parts
                .iter()
                .map(|part| match part {
                    crate::types::ChatContentPart::Text(text) => {
                        let kind = if role == "assistant" {
                            "output_text"
                        } else {
                            "input_text"
                        };
                        Ok(serde_json::json!({ "type": kind, "text": text.text }))
                    }
                    crate::types::ChatContentPart::ImageUrl(image) => {
                        let image_url = match &image.image_url {
                            crate::types::ChatImageUrl::Url(url) => url.clone(),
                            crate::types::ChatImageUrl::Object(object) => object.url.clone(),
                        };
                        Ok(serde_json::json!({ "type": "input_image", "image_url": image_url }))
                    }
                    other => Err(LiteLLMError::Unsupported(format!(
                        "Responses input does not support chat content part: {other:?}"
                    ))),
                })
                .collect::<Result<Vec<_>>>()?;
            Ok(Value::Array(mapped))
        }
    }
}

fn responses_input(req: &ChatRequest) -> Result<Vec<Value>> {
    let mut input = Vec::new();
    for message in &req.messages {
        if message.role == "tool" {
            let call_id = message.tool_call_id.as_deref().ok_or_else(|| {
                LiteLLMError::Config("Responses tool messages require tool_call_id".into())
            })?;
            let output = match &message.content {
                crate::types::ChatMessageContent::Text(text) => text.clone(),
                other => serde_json::to_string(other)
                    .map_err(|err| LiteLLMError::Parse(err.to_string()))?,
            };
            input.push(serde_json::json!({
                "type": "function_call_output",
                "call_id": call_id,
                "output": output,
            }));
            continue;
        }

        input.push(serde_json::json!({
            "role": message.role,
            "content": responses_content(&message.role, &message.content)?,
        }));

        if let Some(tool_calls) = message.tool_calls.as_ref().and_then(Value::as_array) {
            for call in tool_calls {
                let function =
                    call.get("function")
                        .and_then(Value::as_object)
                        .ok_or_else(|| {
                            LiteLLMError::Config("invalid Chat Completions tool call".into())
                        })?;
                input.push(serde_json::json!({
                    "type": "function_call",
                    "call_id": call.get("id").and_then(Value::as_str).ok_or_else(|| LiteLLMError::Config("tool call requires id".into()))?,
                    "name": function.get("name").and_then(Value::as_str).ok_or_else(|| LiteLLMError::Config("tool call requires function.name".into()))?,
                    "arguments": function.get("arguments").and_then(Value::as_str).ok_or_else(|| LiteLLMError::Config("tool call requires function.arguments".into()))?,
                }));
            }
        }
    }
    Ok(input)
}

fn responses_tools(tools: &Value) -> Result<Value> {
    let tools = tools
        .as_array()
        .ok_or_else(|| LiteLLMError::Config("tools must be an array".into()))?;
    let mapped = tools
        .iter()
        .map(|tool| {
            if tool.get("type").and_then(Value::as_str) != Some("function") {
                return Ok(tool.clone());
            }
            let function = tool
                .get("function")
                .and_then(Value::as_object)
                .ok_or_else(|| {
                    LiteLLMError::Config("function tool requires function object".into())
                })?;
            let mut mapped = serde_json::Map::new();
            mapped.insert("type".into(), Value::String("function".into()));
            for key in ["name", "description", "parameters", "strict"] {
                if let Some(value) = function.get(key) {
                    mapped.insert(key.into(), value.clone());
                }
            }
            if !mapped.contains_key("name") {
                return Err(LiteLLMError::Config("function tool requires name".into()));
            }
            Ok(Value::Object(mapped))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(Value::Array(mapped))
}

fn responses_text_format(format: &Value) -> Result<Value> {
    if format.get("type").and_then(Value::as_str) == Some("json_schema") {
        let schema = format
            .get("json_schema")
            .and_then(Value::as_object)
            .ok_or_else(|| {
                LiteLLMError::Config("json_schema response format requires json_schema".into())
            })?;
        let mut flattened = schema.clone();
        flattened.insert("type".into(), Value::String("json_schema".into()));
        Ok(Value::Object(flattened))
    } else {
        Ok(format.clone())
    }
}

fn build_responses_body(req: &ChatRequest, stream: bool) -> Result<Value> {
    let astra = is_astra(&req.model);
    if req.max_tokens.is_some() {
        return Err(LiteLLMError::Unsupported(
            "Responses API uses max_completion_tokens, not max_tokens".into(),
        ));
    }
    for (name, value) in [
        ("stop", req.stop.as_ref()),
        (
            "presence_penalty",
            req.presence_penalty.as_ref().map(|_| &Value::Null),
        ),
        (
            "frequency_penalty",
            req.frequency_penalty.as_ref().map(|_| &Value::Null),
        ),
        ("seed", req.seed.as_ref().map(|_| &Value::Null)),
        ("thinking", req.thinking.as_ref()),
    ] {
        if value.is_some() {
            return Err(LiteLLMError::Unsupported(format!(
                "{name} is not mapped by the Responses transport"
            )));
        }
    }

    let mut body = serde_json::json!({
        "model": req.model,
        "input": responses_input(req)?,
    });
    if stream {
        body["stream"] = Value::Bool(true);
    }
    if let Some(max_tokens) = req.max_completion_tokens {
        body["max_output_tokens"] = serde_json::json!(max_tokens);
    }
    if !astra {
        if let Some(temperature) = req.temperature {
            body["temperature"] = serde_json::json!(temperature);
        }
        if let Some(top_p) = req.top_p {
            body["top_p"] = serde_json::json!(top_p);
        }
    }
    if let Some(format) = &req.response_format {
        body["text"] = serde_json::json!({ "format": responses_text_format(format)? });
    }
    if let Some(tools) = &req.tools {
        body["tools"] = responses_tools(tools)?;
    }
    if let Some(tool_choice) = &req.tool_choice {
        body["tool_choice"] = match tool_choice {
            Value::Object(choice)
                if choice.get("type").and_then(Value::as_str) == Some("function")
                    && choice.get("function").is_some() =>
            {
                let function = choice
                    .get("function")
                    .and_then(Value::as_object)
                    .ok_or_else(|| {
                        LiteLLMError::Config("function tool_choice requires function object".into())
                    })?;
                serde_json::json!({
                    "type": "function",
                    "name": function.get("name").ok_or_else(|| LiteLLMError::Config("function tool_choice requires name".into()))?
                })
            }
            value => value.clone(),
        };
    }
    if let Some(parallel) = req.parallel_tool_calls {
        body["parallel_tool_calls"] = Value::Bool(parallel);
    }
    if let Some(metadata) = &req.metadata {
        body["metadata"] = metadata.clone();
    }
    if let Some(user) = &req.user {
        body["user"] = Value::String(user.clone());
    }
    if let Some(effort) = &req.reasoning_effort {
        let effort = if astra && matches!(effort.as_str(), Some("none" | "minimal")) {
            Value::String("low".into())
        } else {
            effort.clone()
        };
        body["reasoning"] = serde_json::json!({ "effort": effort });
    }
    merge_extra_body(
        &mut body,
        req.extra_body.as_ref(),
        &[
            "model",
            "input",
            "stream",
            "max_output_tokens",
            "text",
            "tools",
            "tool_choice",
            "parallel_tool_calls",
            "metadata",
            "user",
            "reasoning",
            "temperature",
            "top_p",
            "top_logprobs",
        ],
    )?;
    Ok(body)
}

pub async fn responses_chat(
    client: &Client,
    cfg: &ProviderConfig,
    req: ChatRequest,
) -> Result<ChatResponse> {
    let base = cfg
        .base_url
        .clone()
        .ok_or_else(|| LiteLLMError::Config("base_url required".into()))?;
    let key = resolve_api_key(cfg)?;
    let body = build_responses_body(&req, false)?;
    let mut builder = client
        .post(format!("{}/responses", base.trim_end_matches('/')))
        .json(&body);
    if let Some(key) = key {
        builder = builder.bearer_auth(key);
    }
    for (k, v) in &cfg.extra_headers {
        builder = builder.header(k, v);
    }
    let (raw, headers) = send_json::<Value>(builder).await?;
    let content = raw
        .get("output")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|item| item.get("type").and_then(Value::as_str) == Some("message"))
        .flat_map(|item| {
            item.get("content")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
        })
        .filter(|part| part.get("type").and_then(Value::as_str) == Some("output_text"))
        .filter_map(|part| part.get("text").and_then(Value::as_str))
        .collect::<String>();
    let usage = raw
        .get("usage")
        .map(|usage| Usage {
            prompt_tokens: usage.get("input_tokens").and_then(Value::as_u64),
            completion_tokens: usage.get("output_tokens").and_then(Value::as_u64),
            thoughts_tokens: usage
                .pointer("/output_tokens_details/reasoning_tokens")
                .and_then(Value::as_u64),
            total_tokens: usage.get("total_tokens").and_then(Value::as_u64),
            cost_usd: usage.get("cost").and_then(Value::as_f64),
        })
        .unwrap_or_default();
    let refusal = raw
        .get("output")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|item| item.get("type").and_then(Value::as_str) == Some("message"))
        .flat_map(|item| {
            item.get("content")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
        })
        .filter(|part| part.get("type").and_then(Value::as_str) == Some("refusal"))
        .filter_map(|part| part.get("refusal").and_then(Value::as_str))
        .collect::<String>();
    if !refusal.is_empty() {
        return Err(LiteLLMError::Refusal {
            text: refusal,
            usage,
        });
    }
    if matches!(raw.get("status").and_then(Value::as_str), Some("failed")) {
        let message = raw
            .pointer("/error/message")
            .and_then(Value::as_str)
            .unwrap_or("Responses API failed to complete the response");
        return Err(LiteLLMError::http(message));
    }
    if matches!(
        raw.get("status").and_then(Value::as_str),
        Some("incomplete")
    ) {
        let reason = raw
            .pointer("/incomplete_details/reason")
            .and_then(Value::as_str)
            .unwrap_or("Responses API returned an incomplete response");
        return Err(LiteLLMError::Truncated {
            text: if content.is_empty() {
                reason.to_string()
            } else {
                content.clone()
            },
            usage,
        });
    }
    let calls: Vec<Value> = raw.get("output").and_then(Value::as_array).into_iter().flatten()
        .filter(|item| item.get("type").and_then(Value::as_str) == Some("function_call"))
        .map(|item| serde_json::json!({
            "id": item.get("call_id").or_else(|| item.get("id")).cloned().unwrap_or(Value::Null),
            "type": "function",
            "function": {
                "name": item.get("name").cloned().unwrap_or(Value::Null),
                "arguments": item.get("arguments").cloned().unwrap_or_else(|| Value::String(String::new())),
            }
        })).collect();
    let response_id = raw.get("id").and_then(Value::as_str).map(str::to_owned);
    let header_cost = headers
        .get("x-litellm-response-cost")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok());
    Ok(ChatResponse {
        content,
        reasoning: None,
        tool_calls: (!calls.is_empty()).then_some(Value::Array(calls)),
        usage,
        response_id,
        header_cost,
        raw: Some(raw),
    })
}

pub async fn responses_chat_stream(
    client: &Client,
    cfg: &ProviderConfig,
    req: ChatRequest,
) -> Result<ChatStream> {
    let base = cfg
        .base_url
        .clone()
        .ok_or_else(|| LiteLLMError::Config("base_url required".into()))?;
    let key = resolve_api_key(cfg)?;
    let body = build_responses_body(&req, true)?;
    let mut builder = client
        .post(format!("{}/responses", base.trim_end_matches('/')))
        .json(&body);
    if let Some(key) = key {
        builder = builder.bearer_auth(key);
    }
    for (k, v) in &cfg.extra_headers {
        builder = builder.header(k, v);
    }
    let resp = builder.send().await.map_err(LiteLLMError::from)?;
    let status = resp.status();
    if !status.is_success() {
        return Err(LiteLLMError::http(format!(
            "http {}: {}",
            status.as_u16(),
            resp.text().await.map_err(LiteLLMError::from)?
        )));
    }
    Ok(parse_responses_sse_stream(resp.bytes_stream()))
}

pub async fn chat(client: &Client, cfg: &ProviderConfig, req: ChatRequest) -> Result<ChatResponse> {
    let base = cfg
        .base_url
        .clone()
        .ok_or_else(|| LiteLLMError::Config("base_url required".into()))?;
    let url = format!("{}/chat/completions", base.trim_end_matches('/'));
    let key = resolve_api_key(cfg)?;

    let body = build_chat_body(&req, false)?;

    let mut builder = client.post(url).json(&body);
    if let Some(key) = key {
        builder = builder.bearer_auth(key);
    }
    for (k, v) in &cfg.extra_headers {
        builder = builder.header(k, v);
    }

    let (parsed, headers) = send_json::<OpenAIChatResponse>(builder).await?;
    let first_message = parsed.choices.first().map(|c| &c.message);
    let content = first_message
        .and_then(|message| extract_text_value(message.content.as_ref()))
        .unwrap_or_default();
    let reasoning = first_message.and_then(extract_reasoning);
    let tool_calls = first_message
        .and_then(|m| m.tool_calls.clone())
        .filter(|v| !matches!(v, Value::Null) && !matches!(v, Value::Array(a) if a.is_empty()));
    let header_cost = headers
        .get("x-litellm-response-cost")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<f64>().ok());
    let mut usage = map_usage(parsed.usage);
    if usage.cost_usd.is_none() {
        usage.cost_usd = header_cost;
    }

    Ok(ChatResponse {
        content,
        reasoning,
        tool_calls,
        usage,
        response_id: parsed.id,
        header_cost,
        raw: None,
    })
}

fn extract_reasoning(message: &OpenAIMessage) -> Option<Reasoning> {
    message
        .reasoning_details
        .as_ref()
        .and_then(|details| details.as_array().cloned())
        .and_then(Reasoning::from_details)
        .or_else(|| {
            message
                .reasoning_content
                .as_ref()
                .or(message.reasoning.as_ref())
                .cloned()
                .and_then(Reasoning::from_text)
        })
}

fn extract_text_value(value: Option<&Value>) -> Option<String> {
    let mut out = String::new();
    collect_text_fragments(value?, &mut out);
    (!out.trim().is_empty()).then_some(out)
}

fn collect_text_fragments(value: &Value, out: &mut String) {
    match value {
        Value::String(text) => out.push_str(text),
        Value::Array(items) => {
            for item in items {
                collect_text_fragments(item, out);
            }
        }
        Value::Object(map) => {
            if let Some(text) = map.get("text") {
                collect_text_fragments(text, out);
            } else if let Some(content) = map.get("content") {
                collect_text_fragments(content, out);
            } else if let Some(value) = map.get("value") {
                collect_text_fragments(value, out);
            }
        }
        _ => {}
    }
}

pub async fn chat_stream(
    client: &Client,
    cfg: &ProviderConfig,
    req: ChatRequest,
) -> Result<ChatStream> {
    let base = cfg
        .base_url
        .clone()
        .ok_or_else(|| LiteLLMError::Config("base_url required".into()))?;
    let url = format!("{}/chat/completions", base.trim_end_matches('/'));
    let key = resolve_api_key(cfg)?;

    let body = build_chat_body(&req, true)?;

    let mut builder = client.post(url).json(&body);
    if let Some(key) = key {
        builder = builder.bearer_auth(key);
    }
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

    Ok(parse_sse_stream(resp.bytes_stream()))
}

pub async fn embeddings(
    client: &Client,
    cfg: &ProviderConfig,
    req: EmbeddingRequest,
) -> Result<EmbeddingResponse> {
    let base = cfg
        .base_url
        .clone()
        .ok_or_else(|| LiteLLMError::Config("base_url required".into()))?;
    let url = format!("{}/embeddings", base.trim_end_matches('/'));
    let key = resolve_api_key(cfg)?;

    let mut body = serde_json::json!({
        "model": req.model,
        "input": req.input,
    });
    merge_extra_body(&mut body, req.extra_body.as_ref(), &["model", "input"])?;

    let mut builder = client.post(url).json(&body);
    if let Some(key) = key {
        builder = builder.bearer_auth(key);
    }
    for (k, v) in &cfg.extra_headers {
        builder = builder.header(k, v);
    }

    let (parsed, _headers) = send_json::<OpenAIEmbeddingResponse>(builder).await?;
    let vectors = parsed.data.into_iter().map(|d| d.embedding).collect();

    Ok(EmbeddingResponse {
        vectors,
        usage: map_usage(parsed.usage),
        raw: None,
    })
}

pub async fn image_generation(
    client: &Client,
    cfg: &ProviderConfig,
    req: ImageRequest,
) -> Result<ImageResponse> {
    let base = cfg
        .base_url
        .clone()
        .ok_or_else(|| LiteLLMError::Config("base_url required".into()))?;
    let url = format!("{}/images/generations", base.trim_end_matches('/'));
    let key = resolve_api_key(cfg)?;

    let mut body = serde_json::json!({
        "model": req.model,
        "prompt": req.prompt,
    });
    if let Some(n) = req.n {
        body["n"] = serde_json::json!(n);
    }
    if let Some(ref size) = req.size {
        body["size"] = serde_json::json!(size);
    }
    if let Some(ref quality) = req.quality {
        body["quality"] = serde_json::json!(quality);
    }
    if let Some(ref background) = req.background {
        body["background"] = serde_json::json!(background);
    }
    merge_extra_body(&mut body, req.extra_body.as_ref(), &["model", "prompt"])?;

    let mut builder = client.post(url).json(&body);
    if let Some(key) = key {
        builder = builder.bearer_auth(key);
    }
    for (k, v) in &cfg.extra_headers {
        builder = builder.header(k, v);
    }

    let (parsed, _headers) = send_json::<Value>(builder).await?;
    let images = parsed
        .get("data")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .map(|item| ImageData {
                    b64_json: item
                        .get("b64_json")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                    url: item
                        .get("url")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                    revised_prompt: item
                        .get("revised_prompt")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                    mime_type: None,
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    Ok(ImageResponse {
        images,
        usage: Usage::default(),
        raw: None,
    })
}

/// Video generation options for configurable timeouts.
#[derive(Debug, Clone)]
pub struct VideoGenerationOptions {
    /// Maximum number of polling attempts
    pub max_poll_attempts: u32,
    /// Interval between polling attempts in seconds
    pub poll_interval_secs: u64,
}

impl Default for VideoGenerationOptions {
    fn default() -> Self {
        Self {
            max_poll_attempts: DEFAULT_VIDEO_MAX_POLL_ATTEMPTS,
            poll_interval_secs: DEFAULT_VIDEO_POLL_INTERVAL_SECS,
        }
    }
}

pub async fn video_generation(
    client: &Client,
    cfg: &ProviderConfig,
    req: VideoRequest,
) -> Result<VideoResponse> {
    video_generation_with_options(client, cfg, req, VideoGenerationOptions::default()).await
}

pub async fn video_generation_with_options(
    client: &Client,
    cfg: &ProviderConfig,
    req: VideoRequest,
    options: VideoGenerationOptions,
) -> Result<VideoResponse> {
    let base = cfg
        .base_url
        .clone()
        .ok_or_else(|| LiteLLMError::Config("base_url required".into()))?;
    let url = format!("{}/videos", base.trim_end_matches('/'));
    let key = resolve_api_key(cfg)?;

    let mut form = Form::new()
        .text("model", req.model)
        .text("prompt", req.prompt);
    if let Some(seconds) = req.seconds {
        form = form.text("seconds", seconds.to_string());
    }
    if let Some(size) = req.size {
        form = form.text("size", size);
    }

    let mut builder = client.post(url).multipart(form);
    if let Some(ref key) = key {
        builder = builder.bearer_auth(key.clone());
    }
    for (k, v) in &cfg.extra_headers {
        builder = builder.header(k, v);
    }

    let (parsed, _headers) = send_json::<Value>(builder).await?;
    let video_id = parsed
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| LiteLLMError::Parse("missing video id".into()))?;

    let status_url = format!("{}/videos/{}", base.trim_end_matches('/'), video_id);
    let poll_interval = Duration::from_secs(options.poll_interval_secs);

    for attempt in 0..options.max_poll_attempts {
        let mut status_builder = client.get(&status_url);
        if let Some(ref key) = key {
            status_builder = status_builder.bearer_auth(key.clone());
        }
        let (status_resp, _headers) = send_json::<Value>(status_builder).await?;
        let status = status_resp
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");

        match status {
            "completed" => {
                return fetch_video_content(client, &base, video_id, key.as_deref()).await;
            }
            "failed" => {
                let msg = status_resp
                    .get("error")
                    .and_then(|v| v.as_str())
                    .unwrap_or("video generation failed");
                return Err(LiteLLMError::http(msg.to_string()));
            }
            _ => {
                if attempt + 1 >= options.max_poll_attempts {
                    return Err(LiteLLMError::http(format!(
                        "video generation timed out after {} attempts",
                        options.max_poll_attempts
                    )));
                }
                sleep(poll_interval).await;
            }
        }
    }

    Err(LiteLLMError::http("video generation timed out"))
}

async fn fetch_video_content(
    client: &Client,
    base: &str,
    video_id: &str,
    key: Option<&str>,
) -> Result<VideoResponse> {
    let content_url = format!("{}/videos/{}/content", base.trim_end_matches('/'), video_id);
    let mut content_builder = client.get(&content_url);
    if let Some(key) = key {
        content_builder = content_builder.bearer_auth(key);
    }

    let bytes = content_builder
        .send()
        .await
        .map_err(LiteLLMError::from)?
        .bytes()
        .await
        .map_err(LiteLLMError::from)?;
    let b64 = general_purpose::STANDARD.encode(bytes);

    Ok(VideoResponse {
        video_url: Some(format!("data:video/mp4;base64,{b64}")),
        raw: None,
    })
}

fn map_usage(usage: Option<OpenAIUsage>) -> Usage {
    usage.map_or_else(Usage::default, |u| Usage {
        prompt_tokens: u.prompt_tokens,
        completion_tokens: u.completion_tokens,
        thoughts_tokens: u.completion_tokens_details.and_then(|d| d.reasoning_tokens),
        total_tokens: u.total_tokens,
        cost_usd: parse_cost(u.cost.as_ref()),
    })
}

fn parse_cost(value: Option<&Value>) -> Option<f64> {
    let v = value?;
    if let Some(n) = v.as_f64() {
        return Some(n);
    }
    if let Some(s) = v.as_str() {
        return s.parse::<f64>().ok();
    }
    None
}
