#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
destination=${MODEL_REGISTRY_DESTINATION:-"$repo_root/data/model_prices_and_context_window.json"}
source_url=${MODEL_REGISTRY_SOURCE_URL:-"https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json"}
temporary_file=$(mktemp)
trap 'rm -f "$temporary_file"' EXIT

curl --fail --silent --show-error --location \
  --retry 3 --retry-all-errors \
  "$source_url" \
  --output "$temporary_file"

"$repo_root/scripts/validate-model-registry.sh" "$temporary_file"

new_count=$(jq 'length - 1' "$temporary_file")
if [[ -f "$destination" ]]; then
  old_count=$(jq 'length - 1' "$destination")
  minimum_count=$((old_count * 3 / 4))
  if (( new_count < minimum_count )); then
    echo "refusing registry shrink from $old_count to $new_count entries" >&2
    exit 1
  fi
fi

if cmp --silent "$temporary_file" "$destination"; then
  echo "model registry is already current"
  exit 0
fi

mv "$temporary_file" "$destination"
echo "updated model registry to $new_count entries"
