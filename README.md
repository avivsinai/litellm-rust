# litellm-rs

[![CI](https://github.com/avivsinai/litellm-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/avivsinai/litellm-rs/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust: 1.88+](https://img.shields.io/badge/rust-1.88%2B-orange.svg)](https://www.rust-lang.org/)

Minimal Rust SDK port of [LiteLLM](https://github.com/BerriAI/litellm) (library only). Provides a unified interface for chat, embeddings, images, and video across multiple LLM providers.

> **Note**: This project is under active development. APIs may change.

## Features

- **Unified client** for OpenAI-compatible, Anthropic, Gemini, and xAI providers
- **Chat completions** with streaming (SSE) support
- **Text embeddings**
- **Image generation** (OpenAI DALL-E / GPT Image)
- **Video generation** (Gemini Veo, OpenAI Sora)
- **Model routing** with `provider/model` format
- **Automatic retry** with exponential backoff
- **Cost tracking** via response headers
- **Embedded model registry** with pricing and context window data

## Supported Providers

| Provider | Chat | Streaming | Embeddings | Images | Video |
|----------|------|-----------|------------|--------|-------|
| OpenAI-compatible | yes | yes | yes | yes | yes |
| Anthropic | yes | yes | - | - | - |
| Gemini | yes | - | - | yes | yes |
| xAI | yes | yes | - | - | - |

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
litellm-rs = { git = "https://github.com/avivsinai/litellm-rs" }
```

## Quick Start

```rust
use litellm_rs::{
    LiteLLM, ProviderConfig, ProviderKind,
    ChatRequest, EmbeddingRequest, ImageRequest, VideoRequest,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let client = LiteLLM::new()?
        .with_provider(
            "openai",
            ProviderConfig::default()
                .with_kind(ProviderKind::OpenAICompatible)
                .with_api_key_env("OPENAI_API_KEY"),
        )
        .with_provider(
            "gemini",
            ProviderConfig::default()
                .with_kind(ProviderKind::Gemini)
                .with_api_key_env("GEMINI_API_KEY"),
        );

    let resp = client
        .completion(ChatRequest::new("openai/gpt-4o").message("user", "hello"))
        .await?;
    println!("{}", resp.content);

    let embed = client
        .embedding(EmbeddingRequest {
            model: "openai/text-embedding-3-small".to_string(),
            input: serde_json::json!("hello"),
        })
        .await?;
    println!("{} vectors", embed.vectors.len());

    let images = client
        .image_generation(ImageRequest {
            model: "openai/gpt-image-1.5".to_string(),
            prompt: "A cozy cabin in snow".to_string(),
            n: Some(1),
            size: None,
            quality: None,
            background: None,
        })
        .await?;
    println!("{} images", images.images.len());

    let video = client
        .video_generation(VideoRequest {
            model: "openai/sora-2-pro".to_string(),
            prompt: "Drone flyover".to_string(),
            seconds: Some(5),
            size: None,
        })
        .await?;
    println!("video url: {:?}", video.video_url);

    Ok(())
}
```

## Streaming

```rust
use futures_util::StreamExt;
use litellm_rs::{LiteLLM, ChatRequest};

# async fn run() -> anyhow::Result<()> {
let client = LiteLLM::new()?;
let mut stream = client
    .stream_completion(ChatRequest::new("openai/gpt-4o").message("user", "hello"))
    .await?;
while let Some(chunk) = stream.next().await {
    let chunk = chunk?;
    print!("{}", chunk.content);
}
# Ok(())
# }
```

## Provider Configuration

Set API keys via environment variables:

| Variable | Provider |
|----------|----------|
| `OPENAI_API_KEY` | OpenAI |
| `ANTHROPIC_API_KEY` | Anthropic |
| `GEMINI_API_KEY` | Google Gemini |
| `OPENROUTER_API_KEY` | OpenRouter |
| `XAI_API_KEY` | xAI / Grok |

Model routing uses `provider/model` format (e.g., `openai/gpt-4o`, `openrouter/anthropic/claude-sonnet-4-5`).

## Minimum Supported Rust Version

The MSRV is **Rust 1.88**. This is verified in CI.

## Notes

- xAI uses OpenAI-compatible endpoints. Configure provider `xai` with base URL `https://api.x.ai/v1` and `XAI_API_KEY`.
- This crate intentionally excludes LiteLLM proxy/server features.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for development setup and guidelines.

## License

[MIT](LICENSE) - This project is a Rust port of [LiteLLM](https://github.com/BerriAI/litellm) by Berri AI (also MIT licensed).
