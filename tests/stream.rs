use bytes::Bytes;
use futures_util::StreamExt;
use litellm_rust::stream::{parse_anthropic_sse_stream, parse_sse_stream};

#[tokio::test]
async fn parse_sse_basic() {
    let data = Bytes::from("data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\n");
    let stream = futures_util::stream::iter(vec![Ok(data)]);
    let mut parsed = parse_sse_stream(stream);
    let chunk = parsed.next().await.unwrap().unwrap();
    assert_eq!(chunk.content, "hi");
    assert!(chunk.reasoning.is_none());
}

#[tokio::test]
async fn parse_sse_uses_reasoning_delta_when_content_is_absent() {
    let data = Bytes::from("data: {\"choices\":[{\"delta\":{\"reasoning\":\"thinking\"}}]}\n\n");
    let stream = futures_util::stream::iter(vec![Ok(data)]);
    let mut parsed = parse_sse_stream(stream);
    let chunk = parsed.next().await.unwrap().unwrap();
    assert_eq!(chunk.content, "");
    assert_eq!(
        chunk.reasoning.and_then(|reasoning| reasoning.text()),
        Some("thinking".to_string())
    );
}

#[tokio::test]
async fn parse_sse_uses_reasoning_content_delta_when_content_is_absent() {
    let data =
        Bytes::from("data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"hidden\"}}]}\n\n");
    let stream = futures_util::stream::iter(vec![Ok(data)]);
    let mut parsed = parse_sse_stream(stream);
    let chunk = parsed.next().await.unwrap().unwrap();
    assert_eq!(chunk.content, "");
    assert_eq!(
        chunk.reasoning.and_then(|reasoning| reasoning.text()),
        Some("hidden".to_string())
    );
}

#[tokio::test]
async fn parse_sse_preserves_reasoning_details_as_raw_values() {
    let data = Bytes::from(concat!(
        "data: {\"choices\":[{\"delta\":{\"reasoning_details\":[",
        "{\"type\":\"provider.unknown\",\"value\":{\"nested\":true}},",
        "{\"type\":\"reasoning.encrypted\",\"data\":\"[REDACTED]\",\"index\":0}",
        "]}}]}\n\n"
    ));
    let stream = futures_util::stream::iter(vec![Ok(data)]);
    let mut parsed = parse_sse_stream(stream);
    let chunk = parsed.next().await.unwrap().unwrap();
    assert_eq!(chunk.content, "");
    let details = chunk.reasoning.unwrap().details;
    assert_eq!(details[0]["type"], "provider.unknown");
    assert_eq!(details[0]["value"]["nested"], true);
    assert_eq!(details[1]["type"], "reasoning.encrypted");
    assert_eq!(details[1]["data"], "[REDACTED]");
}

#[tokio::test]
async fn parse_sse_preserves_tool_call_reasoning_interleave_raw() {
    let data = Bytes::from(concat!(
        "data: {\"choices\":[{\"delta\":{",
        "\"reasoning_content\":\"thinking\",",
        "\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"type\":\"function\"}]",
        "}}]}\n\n"
    ));
    let stream = futures_util::stream::iter(vec![Ok(data)]);
    let mut parsed = parse_sse_stream(stream);
    let chunk = parsed.next().await.unwrap().unwrap();
    assert_eq!(chunk.content, "");
    assert_eq!(
        chunk.reasoning.and_then(|reasoning| reasoning.text()),
        Some("thinking".to_string())
    );
    assert_eq!(
        chunk.raw.unwrap()["choices"][0]["delta"]["tool_calls"][0]["id"],
        "call_1"
    );
}

#[tokio::test]
async fn parse_sse_preserves_final_finish_reason_chunk() {
    let data = Bytes::from(
        "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n",
    );
    let stream = futures_util::stream::iter(vec![Ok(data)]);
    let mut parsed = parse_sse_stream(stream);
    let chunk = parsed.next().await.unwrap().unwrap();
    assert_eq!(chunk.content, "");
    assert!(chunk.reasoning.is_none());
    assert_eq!(chunk.raw.unwrap()["choices"][0]["finish_reason"], "stop");
    assert!(parsed.next().await.is_none());
}

#[tokio::test]
async fn parse_anthropic_sse_basic() {
    let payload = concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\"}\n\n",
        "event: content_block_delta\n",
        "data: {\"delta\":{\"type\":\"text_delta\",\"text\":\"hello\"}}\n\n",
    );
    let stream = futures_util::stream::iter(vec![Ok(Bytes::from(payload))]);
    let mut parsed = parse_anthropic_sse_stream(stream);
    let chunk = parsed.next().await.unwrap().unwrap();
    assert_eq!(chunk.content, "hello");
    assert!(chunk.reasoning.is_none());
}

#[tokio::test]
async fn parse_anthropic_sse_uses_thinking_delta_when_text_is_absent() {
    let payload = concat!(
        "event: content_block_delta\n",
        "data: {\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"think\"}}\n\n",
    );
    let stream = futures_util::stream::iter(vec![Ok(Bytes::from(payload))]);
    let mut parsed = parse_anthropic_sse_stream(stream);
    let chunk = parsed.next().await.unwrap().unwrap();
    assert_eq!(chunk.content, "");
    assert_eq!(
        chunk.reasoning.and_then(|reasoning| reasoning.text()),
        Some("think".to_string())
    );
}
