# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Breaking Changes

- Gemini thought parts (`"thought": true`) now come back in
  `ChatResponse.reasoning` and `ChatStreamChunk.reasoning`. Before, their text
  was part of `content`.
- Gemini requests now send `thinking` and `reasoning_effort`; before, Gemini
  ignored both. A `reasoning_effort` other than `none`, `disable`, `minimal`,
  `low`, `medium` or `high`, or a `thinking.type` other than `enabled` or
  `disabled`, now returns `LiteLLMError::Config`.
- A Gemini `thinking` object with a `type` key
  (`{"type": "enabled", "budget_tokens": N}`) is now mapped to Gemini fields
  (`includeThoughts`, plus `thinkingBudget` before Gemini 3). An object without
  `type` is sent as is.

### Added

- Stream Gemini chat completions with `stream_completion`. Requests go to
  `streamGenerateContent?alt=sse`. Only the chunk with the `finishReason`
  carries `usage`.
- Create Gemini embeddings with `embedding`. One input uses `embedContent`; an
  array of inputs uses `batchEmbedContents`, and `extra_body` fields go on each
  batch item. A response with a missing vector, or a vector count that does not
  match the input count, returns `LiteLLMError::Parse`.
- Send `thinking` and `reasoning_effort` to Gemini as
  `generationConfig.thinkingConfig`. On Gemini 3 and newer, `reasoning_effort`
  becomes `thinkingLevel`: `low`, `medium`, `high` become `LOW`, `MEDIUM`,
  `HIGH`; `minimal`, `none` and `disable` become `MINIMAL` on Flash models and
  `LOW` on other models. Older Gemini models, and models without the `gemini-`
  prefix such as Gemma, get a `thinkingBudget` and never a `thinkingLevel`.
  `includeThoughts` is `true`, or `false` for `none` and `disable`, unless
  `thinking` sets it.

## [0.4.1] - 2026-10-20

### Changed

- Refresh the embedded pricing, context-window and capability metadata from the
  current LiteLLM registry. This adds 184 models and updates 435 models. No
  models are removed.

## [0.4.0] - 2026-10-11

### Breaking Changes

- Added the `ProviderKind::OpenAIResponses` variant; downstream code that
  matches `ProviderKind` exhaustively must handle it.
- The embedded model registry no longer has OpenRouter entries, so
  `registry().get("openrouter/...")` now returns `None` (see Changed).

### Added

- Add the opt-in `ProviderKind::OpenAIResponses` transport, which sends chat
  requests through the OpenAI `/responses` wire API (non-streaming and
  streaming, including function-call items). Embeddings, image, and video
  requests on this kind return `LiteLLMError::Unsupported`.

### Fixed

- Recognize dated model snapshots when deriving Anthropic structured-output
  support from the embedded registry.

### Changed

- Refresh embedded pricing, context-window, and capability metadata from the
  current LiteLLM registry.
- The daily registry refresh accepts upstream removal of models that the
  committed registry already marks as past their deprecation date.
- The embedded model registry no longer has OpenRouter entries. Upstream LiteLLM removed them (BerriAI/litellm#45665–#45669) because OpenRouter reports request cost in `usage.cost`, which `Usage::cost_usd` already reads. `registry().get("openrouter/...")` now returns `None`; routing `openrouter/...` models is unchanged.
- Document crates.io (`litellm-rust` 0.3.0) as the preferred install path and keep git as the unreleased-`main` option.

## [0.3.0] - 2026-07-11

### Breaking Changes

- Added `ChatResponse.tool_calls`; downstream code constructing `ChatResponse`
  with struct literals must initialize the new field. The next release is
  therefore versioned as `0.3.0`.

### Added

- Surface model-requested tool calls on `ChatResponse.tool_calls` in OpenAI shape — an array of `{ id, type: "function", function: { name, arguments } }` where `arguments` is the raw JSON string the model produced. OpenAI-compatible providers pass the field through unchanged; Anthropic `tool_use` content blocks and Gemini `functionCall` parts are normalized to the same shape so consumers can use a single dispatch path. Empty / missing arrays become `None`.
- Add built-in OpenAI-compatible routing for Z.AI models through the `zai/`
  prefix and `ZAI_API_KEY`.
- Add a validated daily workflow that refreshes the embedded LiteLLM model
  registry through a reviewable pull request.

### Changed

- Refresh embedded pricing, context-window, and capability metadata from the
  current LiteLLM registry.
- Derive current Anthropic structured-output support from registry metadata,
  while retaining narrow fallbacks for documented models missing that flag.

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
