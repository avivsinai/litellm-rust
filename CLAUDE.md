# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

litellm-rs is a Rust port of LiteLLM — a unified SDK for chat completions, embeddings, images, and video across multiple LLM providers (OpenAI-compatible, Anthropic, Gemini, xAI). Library only, no proxy/server.

## Build & Development Commands

```bash
cargo build                          # Build
cargo test                           # Run all tests (no API keys needed — uses WireMock)
cargo test stream                    # Run tests matching "stream"
cargo test --test integration_providers  # Run only integration test suite
cargo clippy --all-targets -- -D warnings  # Lint (CI enforces zero warnings)
cargo fmt --all -- --check           # Check formatting
cargo doc --no-deps                  # Build docs (CI uses RUSTDOCFLAGS=-D warnings)
```

MSRV is Rust 1.88. CI also runs `cargo-deny` (dependency audit, configured in `deny.toml`) and `gitleaks` (secret scanning with LLM-specific rules in `.gitleaks.toml`). Doc builds use `RUSTDOCFLAGS=-D warnings`.

## Architecture

**Request flow**: `LiteLLM::completion(ChatRequest)` → `router::resolve_model()` parses `"provider/model"` format → `dispatch_chat()` matches on `ProviderKind` → provider-specific function (e.g., `openai_compat::chat()`) builds HTTP request, sends via `http::send_json()` with retry, parses response.

**Key modules:**
- `client.rs` — `LiteLLM` struct: entry point with builder pattern (`.with_provider()`, `.with_default_provider()`)
- `router.rs` — Parses `"provider/model"` strings, resolves provider config with built-in defaults for openai/anthropic/gemini/openrouter/xai
- `config.rs` — `Config`, `ProviderConfig`, `ProviderKind` enum (OpenAICompatible, Anthropic, Gemini)
- `providers/` — One file per provider family. Each exports free functions: `chat()`, `chat_stream()`, `embeddings()`, etc.
- `http.rs` — Retry logic with exponential backoff. Retries 5xx, 429, 408. 16MB SSE buffer limit.
- `stream.rs` — SSE parsers: `parse_sse_stream()` (OpenAI-style) and `parse_anthropic_sse_stream()`. Uses `async-stream` macros.
- `registry.rs` — Embedded model pricing/context data loaded via `include_str!()` from `data/model_prices_and_context_window.json`
- `types.rs` — All request/response DTOs. Multimodal content support (images, audio, files).
- `error.rs` — `LiteLLMError` enum with `thiserror`. Type alias `Result<T>`.

**Provider dispatch** is a `match` on `ProviderKind` in `client.rs` — not trait-based. Providers are plain modules with free functions, not trait impls.

## Adding a New Provider

1. Create `src/providers/your_provider.rs` with `chat()`, `chat_stream()`, etc.
2. Add the variant to `ProviderKind` in `config.rs`
3. Register default config in `router::default_provider_config()`
4. Add dispatch arms in `client.rs` methods
5. Add WireMock-based tests in `tests/integration_providers.rs`

## Conventions

- Commit messages: conventional format (`feat:`, `fix:`, `docs:`, `refactor:`, `test:`)
- Error handling: `thiserror` for typed errors, `anyhow` available but used sparingly
- API key resolution order: explicit key → env var → error (see `providers::resolve_api_key()`)
- Gemini debug mode: set `LITELLM_GEMINI_DEBUG=1` to dump raw responses
- Tests use WireMock for HTTP mocking — no real API calls in CI
