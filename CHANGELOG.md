# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Breaking Changes

- Added `ChatContentPartText.cache_control`; downstream code constructing
  `ChatContentPartText` with struct literals must initialize the new field.
  The next release is therefore versioned as `0.5.0`.
- Anthropic requests now reject `thinking` in `extra_body`. Set
  `ChatRequest.thinking` instead.
- Added `Usage.cache_read_input_tokens` and
  `Usage.cache_creation_input_tokens`; downstream code constructing `Usage`
  with struct literals must initialize the new fields.
- `LiteLLMError::Refusal` and `LiteLLMError::Truncated` now carry
  `usage: Box<Usage>`, which keeps the error type small after `Usage` grew.
  Code that constructs these variants must box the value.
- Anthropic `Usage.prompt_tokens` is now the total prompt tokens:
  `input_tokens` plus cache-read and cache-creation tokens. Before, it was
  `input_tokens` only, which excludes cached tokens.
- Anthropic requests that set `ChatRequest.reasoning_effort` now send
  `output_config.effort`. Models without effort support return an upstream
  error; before, the field was silently dropped.
- Added 11 public fields to `ModelPricing`: `cache_read_cost_per_1k`,
  `cache_creation_cost_per_1k`, `supports_function_calling`,
  `supports_tool_choice`, `supports_vision`, `supports_reasoning`,
  `supports_prompt_caching`, `supports_response_schema`,
  `supports_parallel_function_calling`, `supported_endpoints`, and
  `supported_modalities`. Downstream code that constructs `ModelPricing` with
  a struct literal must set them.

### Added

- Forward `cache_control` on text content parts to Anthropic (system and chat
  messages), so you can set prompt-caching breakpoints. OpenAI-compatible
  providers receive the field as is; Gemini and the Responses transport
  ignore it.
- Report Anthropic prompt-cache usage in `Usage.cache_read_input_tokens` and
  `Usage.cache_creation_input_tokens` for non-streaming chat. Streaming chat
  does not report Anthropic usage yet.
- Read prompt-cache prices, capability flags, and supported endpoints and
  modalities from the embedded model registry into `ModelPricing`. When
  `cache_read_input_token_cost` is absent, the cache-read price falls back to
  `input_cost_per_token_cache_hit`. No current registry entry needs this
  fallback; it guards against upstream regressions.
- Add `Registry::estimate_cost_with_cache` and
  `LiteLLM::estimate_cost_with_cache`. They price cache-read and
  cache-creation tokens at the model's cache rates, and use the input rate
  when the model has no cache rate. Cache-creation tokens use the base
  (5-minute TTL) write rate; 1-hour TTL writes and long-context tier rates
  are not modeled. `input_tokens` must exclude cached tokens: pass
  Anthropic counts directly, and subtract the cached count from
  OpenAI-compatible and Gemini prompt counts first.

### Fixed

- Anthropic no longer drops `ChatRequest.thinking`; it is sent as `thinking`.
- Anthropic no longer drops `ChatRequest.reasoning_effort`; it is sent as
  `output_config.effort`, next to any structured-output `format`.

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
