use crate::error::{LiteLLMError, Result};
use crate::http::MAX_SSE_BUFFER_SIZE;
use crate::types::{Reasoning, Usage};
use bytes::Bytes;
use futures_util::stream::{Stream, StreamExt, TryStreamExt};
use serde_json::Value;
use std::pin::Pin;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio_util::io::StreamReader;

#[derive(Debug, Clone)]
pub struct ChatStreamChunk {
    /// User-visible answer text for this chunk.
    pub content: String,
    /// Provider reasoning, kept separate from answer content.
    pub reasoning: Option<Reasoning>,
    pub raw: Option<Value>,
    pub usage: Option<Usage>,
}

pub type ChatStream = Pin<Box<dyn Stream<Item = Result<ChatStreamChunk>> + Send>>;

#[derive(Debug, Clone)]
struct SseEvent {
    event: Option<String>,
    data: String,
}

type SseEventStream = Pin<Box<dyn Stream<Item = Result<SseEvent>> + Send>>;

fn sse_event_stream<S>(stream: S) -> SseEventStream
where
    S: Stream<Item = std::result::Result<Bytes, reqwest::Error>> + Send + Unpin + 'static,
{
    let s = async_stream::try_stream! {
        let stream = stream.map_err(std::io::Error::other);
        let reader = StreamReader::new(stream);
        let mut lines = BufReader::new(reader).lines();

        let mut event_name: Option<String> = None;
        let mut data_buf = String::new();

        while let Some(line) = lines.next_line().await.map_err(LiteLLMError::from)? {
            if line.is_empty() {
                if !data_buf.is_empty() {
                    let data = std::mem::take(&mut data_buf);
                    let event = event_name.take();
                    yield SseEvent { event, data };
                } else {
                    event_name = None;
                }
                continue;
            }

            if line.starts_with(':') {
                continue;
            }

            let (field, value) = if let Some((field, value)) = line.split_once(':') {
                (field, value.strip_prefix(' ').unwrap_or(value))
            } else {
                (line.as_str(), "")
            };

            match field {
                "event" => {
                    event_name = Some(value.to_string());
                }
                "data" => {
                    if !data_buf.is_empty() {
                        data_buf.push('\n');
                    }
                    data_buf.push_str(value);
                    if data_buf.len() > MAX_SSE_BUFFER_SIZE {
                        Err(LiteLLMError::http(format!(
                            "SSE data buffer exceeded maximum size of {} bytes",
                            MAX_SSE_BUFFER_SIZE
                        )))?;
                    }
                }
                _ => {}
            }
        }

        if !data_buf.is_empty() {
            let data = std::mem::take(&mut data_buf);
            let event = event_name.take();
            yield SseEvent { event, data };
        }
    };
    Box::pin(s)
}

/// Parse an OpenAI-compatible SSE stream into chat chunks.
///
/// This function includes protection against unbounded memory growth by limiting
/// the internal buffer size to `MAX_SSE_BUFFER_SIZE`.
pub fn parse_sse_stream<S>(stream: S) -> ChatStream
where
    S: Stream<Item = std::result::Result<Bytes, reqwest::Error>> + Send + Unpin + 'static,
{
    let s = async_stream::try_stream! {
        let mut events = sse_event_stream(stream);
        while let Some(event) = events.next().await {
            let event = event?;
            let data = event.data.trim();
            if data == "[DONE]" {
                return;
            }
            let value: Value = serde_json::from_str(data)
                .map_err(|e| LiteLLMError::Parse(e.to_string()))?;
            let usage = parse_usage(&value);
            let content = value
                .pointer("/choices/0/delta/content")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let reasoning_text = value
                .pointer("/choices/0/delta/reasoning_content")
                .and_then(|v| v.as_str())
                .or_else(|| {
                    value
                        .pointer("/choices/0/delta/reasoning")
                        .and_then(|v| v.as_str())
                })
                .map(|s| s.to_string());
            let reasoning_details = value
                .pointer("/choices/0/delta/reasoning_details")
                .and_then(|v| v.as_array())
                .cloned();
            let reasoning = reasoning_details
                .and_then(Reasoning::from_details)
                .or_else(|| reasoning_text.and_then(Reasoning::from_text));
            let content = content.unwrap_or_default();
            yield ChatStreamChunk {
                content,
                reasoning,
                raw: Some(value),
                usage,
            };
        }
    };
    Box::pin(s)
}

/// Parse Responses API SSE events into the existing text/reasoning stream.
/// Function-call events remain available in `raw`; callers that need a final
/// normalized `tool_calls` array should use non-streaming completion.
pub fn parse_responses_sse_stream<S>(stream: S) -> ChatStream
where
    S: Stream<Item = std::result::Result<Bytes, reqwest::Error>> + Send + Unpin + 'static,
{
    let s = async_stream::try_stream! {
        let mut events = sse_event_stream(stream);
        let mut accumulated_content = String::new();
        let mut refusal = String::new();
        let mut usage = Usage::default();
        while let Some(event) = events.next().await {
            let event = event?;
            let data = event.data.trim();
            if data == "[DONE]" {
                if !refusal.is_empty() {
                    Err(LiteLLMError::Refusal { text: refusal.clone(), usage: usage.clone() })?;
                }
                return;
            }
            let value: Value = serde_json::from_str(data)
                .map_err(|e| LiteLLMError::Parse(e.to_string()))?;
            let kind = value.get("type").and_then(Value::as_str).or(event.event.as_deref());
            if kind == Some("error") || value.get("type").and_then(Value::as_str) == Some("error") {
                let message = value
                    .get("message")
                    .or_else(|| value.pointer("/error/message"))
                    .and_then(Value::as_str)
                    .unwrap_or("Responses API stream error");
                Err(LiteLLMError::http(message))?;
            }
            if matches!(kind, Some("response.failed" | "response.incomplete")) {
                if kind == Some("response.incomplete") {
                    if let Some(incomplete_usage) = value.get("response").and_then(parse_responses_usage) {
                        usage = incomplete_usage;
                    }
                }
                let message = value
                    .pointer("/response/error/message")
                    .and_then(Value::as_str)
                    .or_else(|| value.pointer("/response/incomplete_details/reason").and_then(Value::as_str))
                    .unwrap_or("Responses API did not complete the response");
                if kind == Some("response.incomplete") {
                    Err(LiteLLMError::Truncated { text: accumulated_content.clone(), usage: usage.clone() })?;
                }
                Err(LiteLLMError::http(message))?;
            }
            if kind == Some("response.refusal.delta") {
                refusal.push_str(value.get("delta").and_then(Value::as_str).unwrap_or_default());
                continue;
            }
            if kind == Some("response.refusal.done") {
                if let Some(text) = value.get("refusal").and_then(Value::as_str) {
                    refusal = text.to_owned();
                }
                if !refusal.is_empty() {
                    Err(LiteLLMError::Refusal { text: refusal.clone(), usage: usage.clone() })?;
                }
                continue;
            }
            let content = if kind == Some("response.output_text.delta") {
                value.get("delta").and_then(Value::as_str).unwrap_or_default().to_string()
            } else { String::new() };
            accumulated_content.push_str(&content);
            let reasoning = if kind == Some("response.reasoning_summary_text.delta") {
                value.get("delta").and_then(Value::as_str).map(str::to_owned).and_then(Reasoning::from_text)
            } else { None };
            let event_usage = if kind == Some("response.completed") {
                value.get("response").and_then(parse_responses_usage)
            } else { None };
            if let Some(completed_usage) = event_usage.clone() {
                usage = completed_usage;
            }
            if kind == Some("response.completed") {
                let response = value.get("response").unwrap_or(&Value::Null);
                let completed_refusal = response
                    .get("output").and_then(Value::as_array).into_iter().flatten()
                    .flat_map(|item| item.get("content").and_then(Value::as_array).into_iter().flatten())
                    .filter(|part| part.get("type").and_then(Value::as_str) == Some("refusal"))
                    .filter_map(|part| part.get("refusal").and_then(Value::as_str))
                    .collect::<String>();
                if !completed_refusal.is_empty() {
                    Err(LiteLLMError::Refusal { text: completed_refusal, usage: usage.clone() })?;
                }
                if !refusal.is_empty() {
                    Err(LiteLLMError::Refusal { text: refusal.clone(), usage: usage.clone() })?;
                }
            }
            if !content.is_empty() || reasoning.is_some() || event_usage.is_some()
                || kind.map(|k| k.starts_with("response.function_call_arguments.") || k.starts_with("response.output_item.")).unwrap_or(false) {
                yield ChatStreamChunk { content, reasoning, raw: Some(value), usage: event_usage };
            }
        }
        if !refusal.is_empty() {
            Err(LiteLLMError::Refusal { text: refusal, usage })?;
        }
    };
    Box::pin(s)
}

fn parse_responses_usage(value: &Value) -> Option<Usage> {
    let usage = value.get("usage")?.as_object()?;
    Some(Usage {
        prompt_tokens: usage.get("input_tokens").and_then(Value::as_u64),
        completion_tokens: usage.get("output_tokens").and_then(Value::as_u64),
        thoughts_tokens: usage
            .get("output_tokens_details")
            .and_then(|v| v.get("reasoning_tokens"))
            .and_then(Value::as_u64),
        total_tokens: usage.get("total_tokens").and_then(Value::as_u64),
        cost_usd: None,
    })
}

/// Parse an Anthropic SSE stream into chat chunks.
///
/// This function includes protection against unbounded memory growth by limiting
/// the internal buffer size to `MAX_SSE_BUFFER_SIZE`.
pub fn parse_anthropic_sse_stream<S>(stream: S) -> ChatStream
where
    S: Stream<Item = std::result::Result<Bytes, reqwest::Error>> + Send + Unpin + 'static,
{
    let s = async_stream::try_stream! {
        let mut events = sse_event_stream(stream);
        while let Some(event) = events.next().await {
            let event = event?;
            let data = event.data.trim();
            if data == "[DONE]" {
                return;
            }
            let value: Value = serde_json::from_str(data)
                .map_err(|e| LiteLLMError::Parse(e.to_string()))?;
            let usage = parse_usage(&value);
            if event.event.as_deref() == Some("content_block_delta") {
                let text = value
                    .pointer("/delta/text")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let reasoning = value
                    .pointer("/delta/thinking")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .and_then(Reasoning::from_text);
                let content = text.unwrap_or_default();
                if !content.is_empty() || reasoning.is_some() {
                    yield ChatStreamChunk {
                        content,
                        reasoning,
                        raw: Some(value),
                        usage,
                    };
                }
            }
        }
    };
    Box::pin(s)
}

fn parse_usage(value: &Value) -> Option<Usage> {
    let usage = value.get("usage")?.as_object()?;
    let prompt_tokens = usage.get("prompt_tokens").and_then(|v| v.as_u64());
    let completion_tokens = usage.get("completion_tokens").and_then(|v| v.as_u64());
    let total_tokens = usage.get("total_tokens").and_then(|v| v.as_u64());
    let cost_usd = usage
        .get("cost")
        .and_then(|v| v.as_f64())
        .or_else(|| usage.get("cost").and_then(|v| v.as_str())?.parse().ok())
        .or_else(|| usage.get("cost_usd").and_then(|v| v.as_f64()))
        .or_else(|| usage.get("total_cost").and_then(|v| v.as_f64()));
    Some(Usage {
        prompt_tokens,
        completion_tokens,
        thoughts_tokens: None,
        total_tokens,
        cost_usd,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use futures_util::stream;

    #[tokio::test]
    async fn parse_sse_basic() {
        let data = "data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"}}]}\n\n\
                    data: {\"choices\":[{\"delta\":{\"content\":\" World\"}}]}\n\n\
                    data: [DONE]\n\n";
        let bytes_stream = stream::iter(vec![Ok(Bytes::from(data))]);
        let mut chat_stream = parse_sse_stream(bytes_stream);

        let chunk1 = chat_stream.next().await.unwrap().unwrap();
        assert_eq!(chunk1.content, "Hello");

        let chunk2 = chat_stream.next().await.unwrap().unwrap();
        assert_eq!(chunk2.content, " World");

        assert!(chat_stream.next().await.is_none());
    }

    #[tokio::test]
    async fn parse_responses_sse_text_function_and_usage() {
        let data = "event: response.output_text.delta\n\
                    data: {\"type\":\"response.output_text.delta\",\"delta\":\"Hello\"}\n\n\
                    event: response.function_call_arguments.delta\n\
                    data: {\"type\":\"response.function_call_arguments.delta\",\"delta\":\"{}\"}\n\n\
                    event: response.completed\n\
                    data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":3,\"output_tokens\":5,\"total_tokens\":8,\"output_tokens_details\":{\"reasoning_tokens\":2}}}}\n\n";
        let mut stream = parse_responses_sse_stream(stream::iter(vec![Ok(Bytes::from(data))]));
        assert_eq!(stream.next().await.unwrap().unwrap().content, "Hello");
        assert_eq!(
            stream.next().await.unwrap().unwrap().raw.unwrap()["delta"],
            "{}"
        );
        let usage = stream.next().await.unwrap().unwrap().usage.unwrap();
        assert_eq!(usage.prompt_tokens, Some(3));
        assert_eq!(usage.thoughts_tokens, Some(2));
    }

    #[tokio::test]
    async fn parse_responses_sse_preserves_function_items_and_fails_status_events() {
        let data = "data: {\"type\":\"response.output_item.added\",\"item\":{\"type\":\"function_call\",\"call_id\":\"call_1\",\"name\":\"weather\"}}\n\n\
                    data: {\"type\":\"response.function_call_arguments.done\",\"call_id\":\"call_1\",\"arguments\":\"{}\"}\n\n";
        let mut stream = parse_responses_sse_stream(stream::iter(vec![Ok(Bytes::from(data))]));
        assert_eq!(
            stream.next().await.unwrap().unwrap().raw.unwrap()["item"]["call_id"],
            "call_1"
        );
        assert_eq!(
            stream.next().await.unwrap().unwrap().raw.unwrap()["arguments"],
            "{}"
        );

        let failed = "data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"message\":\"blocked\"}}}\n\n";
        let mut stream = parse_responses_sse_stream(stream::iter(vec![Ok(Bytes::from(failed))]));
        assert!(stream
            .next()
            .await
            .unwrap()
            .unwrap_err()
            .to_string()
            .contains("blocked"));

        let event_error =
            "event: error\ndata: {\"type\":\"error\",\"message\":\"transport failed\"}\n\n";
        let mut stream =
            parse_responses_sse_stream(stream::iter(vec![Ok(Bytes::from(event_error))]));
        assert!(stream
            .next()
            .await
            .unwrap()
            .unwrap_err()
            .to_string()
            .contains("transport failed"));

        let refusal = "data: {\"type\":\"response.refusal.delta\",\"delta\":\"no\"}\n\n\
                       data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"output\":[{\"type\":\"message\",\"content\":[{\"type\":\"refusal\",\"refusal\":\"no\"}]}],\"usage\":{\"input_tokens\":1,\"output_tokens\":1,\"total_tokens\":2}}}\n\n";
        let mut stream = parse_responses_sse_stream(stream::iter(vec![Ok(Bytes::from(refusal))]));
        match stream.next().await.unwrap().unwrap_err() {
            LiteLLMError::Refusal { text, usage } => {
                assert_eq!(text, "no");
                assert_eq!(usage.total_tokens, Some(2));
            }
            other => panic!("unexpected error: {other:?}"),
        }

        let refusal_done = concat!(
            "data: {\"type\":\"response.refusal.delta\",\"delta\":\"no\"}\n\n",
            "data: {\"type\":\"response.refusal.done\",\"refusal\":\"no\"}\n\n",
        );
        let mut stream =
            parse_responses_sse_stream(stream::iter(vec![Ok(Bytes::from(refusal_done))]));
        match stream.next().await.unwrap().unwrap_err() {
            LiteLLMError::Refusal { text, .. } => assert_eq!(text, "no"),
            other => panic!("unexpected error: {other:?}"),
        }

        let refusal_completed = concat!(
            "data: {\"type\":\"response.refusal.delta\",\"delta\":\"no\"}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"usage\":{\"input_tokens\":1,\"output_tokens\":1,\"total_tokens\":2}}}\n\n",
        );
        let mut stream =
            parse_responses_sse_stream(stream::iter(vec![Ok(Bytes::from(refusal_completed))]));
        match stream.next().await.unwrap().unwrap_err() {
            LiteLLMError::Refusal { text, usage } => {
                assert_eq!(text, "no");
                assert_eq!(usage.total_tokens, Some(2));
            }
            other => panic!("unexpected error: {other:?}"),
        }

        let refusal_eof = "data: {\"type\":\"response.refusal.delta\",\"delta\":\"no\"}\n\n";
        let mut stream =
            parse_responses_sse_stream(stream::iter(vec![Ok(Bytes::from(refusal_eof))]));
        match stream.next().await.unwrap().unwrap_err() {
            LiteLLMError::Refusal { text, .. } => assert_eq!(text, "no"),
            other => panic!("unexpected error: {other:?}"),
        }

        let incomplete = concat!(
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"partial\"}\n\n",
            "data: {\"type\":\"response.incomplete\",\"response\":{\"status\":\"incomplete\",\"incomplete_details\":{\"reason\":\"max_output_tokens\"},\"usage\":{\"input_tokens\":4,\"output_tokens\":6,\"total_tokens\":10}}}\n\n",
        );
        let mut stream =
            parse_responses_sse_stream(stream::iter(vec![Ok(Bytes::from(incomplete))]));
        assert_eq!(stream.next().await.unwrap().unwrap().content, "partial");
        match stream.next().await.unwrap().unwrap_err() {
            LiteLLMError::Truncated { text, usage } => {
                assert_eq!(text, "partial");
                assert_eq!(usage.prompt_tokens, Some(4));
                assert_eq!(usage.total_tokens, Some(10));
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[tokio::test]
    async fn parse_anthropic_sse_basic() {
        let data = "event: content_block_delta\n\
                    data: {\"delta\":{\"text\":\"Hello\"}}\n\n\
                    event: content_block_delta\n\
                    data: {\"delta\":{\"text\":\" World\"}}\n\n";
        let bytes_stream = stream::iter(vec![Ok(Bytes::from(data))]);
        let mut chat_stream = parse_anthropic_sse_stream(bytes_stream);

        let chunk1 = chat_stream.next().await.unwrap().unwrap();
        assert_eq!(chunk1.content, "Hello");

        let chunk2 = chat_stream.next().await.unwrap().unwrap();
        assert_eq!(chunk2.content, " World");
    }

    #[tokio::test]
    async fn parse_sse_handles_split_chunks() {
        // Simulate data coming in multiple network chunks
        let chunk1 = "data: {\"choices\":[{\"delta\":{\"con";
        let chunk2 = "tent\":\"Split\"}}]}\n\ndata: [DONE]\n\n";
        let bytes_stream = stream::iter(vec![Ok(Bytes::from(chunk1)), Ok(Bytes::from(chunk2))]);
        let mut chat_stream = parse_sse_stream(bytes_stream);

        let chunk = chat_stream.next().await.unwrap().unwrap();
        assert_eq!(chunk.content, "Split");

        assert!(chat_stream.next().await.is_none());
    }
}
