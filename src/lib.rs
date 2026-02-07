//! Unified Rust SDK for chat completions, embeddings, images, and video across
//! multiple LLM providers.
//!
//! litellm-rust is a Rust port of [LiteLLM](https://github.com/BerriAI/litellm).
//! It provides a single [`LiteLLM`] client that routes requests to OpenAI-compatible,
//! Anthropic, Gemini, and xAI backends using a `"provider/model"` format.
//!
//! # Quick Start
//!
//! ```rust,no_run
//! use litellm_rs::{LiteLLM, ChatRequest};
//!
//! # async fn run() -> litellm_rs::Result<()> {
//! let client = LiteLLM::new()?;
//! let resp = client
//!     .completion(ChatRequest::new("openai/gpt-4o").message("user", "hello"))
//!     .await?;
//! println!("{}", resp.content);
//! # Ok(())
//! # }
//! ```
//!
//! # Streaming
//!
//! ```rust,no_run
//! use futures_util::StreamExt;
//! use litellm_rs::{LiteLLM, ChatRequest};
//!
//! # async fn run() -> litellm_rs::Result<()> {
//! let client = LiteLLM::new()?;
//! let mut stream = client
//!     .stream_completion(ChatRequest::new("openai/gpt-4o").message("user", "hello"))
//!     .await?;
//! while let Some(chunk) = stream.next().await {
//!     print!("{}", chunk?.content);
//! }
//! # Ok(())
//! # }
//! ```
//!
//! # Supported Providers
//!
//! | Provider | Chat | Streaming | Embeddings | Images | Video |
//! |----------|------|-----------|------------|--------|-------|
//! | OpenAI-compatible | yes | yes | yes | yes | yes |
//! | Anthropic | yes | yes | - | - | - |
//! | Gemini | yes | - | - | yes | yes |
//! | xAI | yes | yes | - | - | - |

pub mod client;
pub mod config;
pub mod error;
pub mod http;
pub mod providers;
pub mod registry;
pub mod router;
pub mod stream;
pub mod types;

pub use client::LiteLLM;
pub use config::{Config, ProviderConfig, ProviderKind};
pub use error::{LiteLLMError, Result};
pub use stream::{ChatStream, ChatStreamChunk};
pub use types::*;
