#!/usr/bin/env bash
set -euo pipefail

registry_path=${1:-data/model_prices_and_context_window.json}

if [[ ! -f "$registry_path" ]]; then
  echo "model registry not found: $registry_path" >&2
  exit 1
fi

# Structural shape and provider coverage.
jq -e '
  type == "object"
  and (.sample_spec | type == "object")
  and ([to_entries[] | select(.key != "sample_spec") | .value | type]
    | all(. == "object"))
  and ([.[] | select(.litellm_provider? == "openai")] | length > 0)
  and ([.[] | select(.litellm_provider? == "anthropic")] | length > 0)
  and ([.[] | select(.litellm_provider? == "gemini")] | length > 0)
  and ([.[] | select(.litellm_provider? == "xai")] | length > 0)
  and ([.[] | select(.litellm_provider? == "zai")] | length > 0)
' "$registry_path" >/dev/null || {
  echo "model registry failed structural/provider validation" >&2
  exit 1
}

# Field types and ranges for what the crate consumes (src/registry.rs).
# A wrong-typed price silently becomes None through as_f64, and token counts
# above u32::MAX would truncate without the try_from guard. Ceilings are
# generous (~80x the priciest known model, ~5x the largest context window).
# The live upstream carries a handful of long-tail vendor typos (e.g. wandb/*
# prices entered per-1M-tokens), so enforcement is tiered: zero tolerance for
# the providers this crate routes to by default, and a 1% global budget for
# tail noise so mass corruption still fails while known upstream warts do not
# wedge the pipeline shut.
# Presence-explicit checks: `field? // default` must not be used here because
# jq's // treats false as absent, letting a wrong-typed `false` price bypass a
# numeric check. Absent fields are fine; present fields must be exactly typed
# and in range. max_tokens is validated too — registry.rs consumes it as the
# max_input_tokens fallback.
core_providers='["openai", "anthropic", "gemini", "xai", "zai", "openrouter"]'
violations=$(jq --argjson core "$core_providers" '
  def ok_price($f):  (has($f) | not) or (.[$f] | type == "number" and . >= 0 and . <= 0.05);
  def ok_tokens($f): (has($f) | not) or (.[$f] | type == "number" and . == floor and . >= 0 and . <= 50000000);
  def ok_bool($f):   (has($f) | not) or (.[$f] | type == "boolean");
  def ok_str($f):    (has($f) | not) or (.[$f] | type == "string");
  [to_entries[]
   | select(.key != "sample_spec")
   | select(.value |
       ok_price("input_cost_per_token")
       and ok_price("output_cost_per_token")
       and ok_tokens("max_input_tokens")
       and ok_tokens("max_output_tokens")
       and ok_tokens("max_tokens")
       and ok_bool("supports_output_config")
       and ok_bool("supports_response_schema")
       and ok_str("litellm_provider")
       and ok_str("mode")
       | not)
   | {key, provider: (.value.litellm_provider? // "")}]
  | {total: length, core: [.[] | select(.provider as $p | $core | index($p))] | length,
     sample: [.[:10][].key]}
' "$registry_path")

# Identity fields are globally zero-tolerance: a wrong-typed litellm_provider
# or mode would otherwise demote a core entry into the tail budget, bypassing
# core protection entirely (proven with litellm_provider=false on a core key).
identity_violations=$(jq '
  [to_entries[]
   | select(.key != "sample_spec")
   | select(.value | (has("litellm_provider") and (.litellm_provider | type != "string"))
                     or (has("mode") and (.mode | type != "string")))
   | .key]
' "$registry_path")
if [[ "$(jq 'length' <<<"$identity_violations")" != "0" ]]; then
  echo "model registry has wrong-typed identity fields (litellm_provider/mode): $(jq -rc '.[:10]' <<<"$identity_violations")" >&2
  exit 1
fi

violation_total=$(jq -r '.total' <<<"$violations")
violation_core=$(jq -r '.core' <<<"$violations")
model_total=$(jq 'length - 1' "$registry_path")
if (( violation_core > 0 )); then
  echo "model registry has $violation_core type/range violations in core providers: $(jq -rc '.sample' <<<"$violations")" >&2
  exit 1
fi
if (( violation_total * 100 > model_total )); then
  echo "model registry has $violation_total type/range violations (>1% of $model_total): $(jq -rc '.sample' <<<"$violations")" >&2
  exit 1
fi
if (( violation_total > 0 )); then
  echo "warning: tolerating $violation_total long-tail type/range violations: $(jq -rc '.sample' <<<"$violations")" >&2
fi

model_count=$(jq 'length - 1' "$registry_path")
if (( model_count < 1000 )); then
  echo "model registry unexpectedly small: $model_count entries" >&2
  exit 1
fi

# Anthropic structured-output gating derives from this flag; a mass flip to
# false would silently disable output_config for every current Claude model.
output_config_count=$(jq '[.[] | select(.supports_output_config? == true)] | length' "$registry_path")
if (( output_config_count < 30 )); then
  echo "supports_output_config coverage unexpectedly low: $output_config_count entries" >&2
  exit 1
fi

echo "validated model registry: $model_count entries, $output_config_count with output-config support"
