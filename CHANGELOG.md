# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.2] - 2026-04-23

### Fixed

- Map OpenAI-compatible JSON body-read timeouts to HTTP errors instead of parse failures
- Extract assistant text from OpenAI-compatible `content` arrays, `reasoning`, and `reasoning_details`

## [0.1.1] - 2026-02-07

### Fixed

- Rename lib target from `litellm_rs` to `litellm_rust` so import paths match the crate name

## [0.1.0] - 2026-02-07

### Added

- Unified `LiteLLM` client for multi-provider LLM access
- Chat completions with streaming support (SSE)
- Text embeddings
- Image generation
- Video generation (Gemini Veo long-running operations)
- Gemini image generation support (native + Imagen)
- Provider implementations:
  - OpenAI-compatible (OpenAI, OpenRouter, xAI, LiteLLM proxy)
  - Anthropic (Messages API)
  - Gemini (native API)
- Model routing with `provider/model` format
- Automatic retry with exponential backoff
- Embedded model pricing and context window registry
- Cost tracking via response headers
