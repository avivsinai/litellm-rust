# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Surface model-requested tool calls on `ChatResponse.tool_calls` in OpenAI shape — an array of `{ id, type: "function", function: { name, arguments } }` where `arguments` is the raw JSON string the model produced. OpenAI-compatible providers pass the field through unchanged; Anthropic `tool_use` content blocks and Gemini `functionCall` parts are normalized to the same shape so consumers can use a single dispatch path. Empty / missing arrays become `None`.

## [0.2.0] - 2026-05-15

### Breaking Changes

- Added `extra_body` to `ChatRequest`, `EmbeddingRequest`, and `ImageRequest`; struct literals must now set this field or use the builders.
- Changed `LiteLLMError::Refusal` and `LiteLLMError::Truncated` from tuple variants to struct variants carrying `{ text, usage }`.

### Added

- Map Anthropic `response_format` to the GA `output_config.format` request shape for supported Claude structured-output models.
- Surface provider reasoning through `ChatResponse.reasoning` and stream chunks using the canonical `Reasoning { details }` model.
- Preserve OpenAI-compatible `reasoning_details` arrays raw, including encrypted and unknown detail types.
- Normalize OpenAI-compatible `reasoning_content` and `reasoning` strings into reasoning text details.
- Surface Anthropic thinking blocks on non-streaming responses and `thinking_delta` on streams.
- Add opt-in `extra_body` passthrough that flattens provider-specific fields into the top-level request body.

### Changed

- Reject Anthropic structured-output requests for unsupported models instead of falling back to legacy tool-call workarounds.
- Return structured refusal and truncation errors for Anthropic `stop_reason=refusal` and `stop_reason=max_tokens`.
- Reject `extra_body` collisions with library-managed fields and deny internal or response-side keys such as `litellm_*`, `vector_store_id(s)`, `extra_*`, and `reasoning_*`.
- Keep reasoning data out of request messages to avoid DeepSeek-style round-trip failures.

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
