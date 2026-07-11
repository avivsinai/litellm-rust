#!/usr/bin/env bash
set -euo pipefail

registry_path=${1:-data/model_prices_and_context_window.json}

if [[ ! -f "$registry_path" ]]; then
  echo "model registry not found: $registry_path" >&2
  exit 1
fi

jq -e '
  type == "object"
  and (.sample_spec | type == "object")
  and ([to_entries[] | select(.key != "sample_spec") | .value | type]
    | all(. == "object"))
  and ([.[] | select(.litellm_provider? == "openai")] | length > 0)
  and ([.[] | select(.litellm_provider? == "anthropic")] | length > 0)
  and ([.[] | select(.litellm_provider? == "xai")] | length > 0)
  and ([.[] | select(.litellm_provider? == "zai")] | length > 0)
' "$registry_path" >/dev/null

model_count=$(jq 'length - 1' "$registry_path")
if (( model_count < 1000 )); then
  echo "model registry unexpectedly small: $model_count entries" >&2
  exit 1
fi

echo "validated model registry: $model_count entries"
