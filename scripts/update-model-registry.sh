#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
destination=${MODEL_REGISTRY_DESTINATION:-"$repo_root/data/model_prices_and_context_window.json"}
baseline_file=${MODEL_REGISTRY_BASELINE:-"$repo_root/data/registry-baseline.json"}
source_url=${MODEL_REGISTRY_SOURCE_URL:-"https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json"}
core_providers='["openai", "anthropic", "gemini", "xai", "zai", "openrouter"]'

profile_of() {
  jq --argjson core "$core_providers" '
    {total: (length - 1),
     core_providers: ([to_entries[] | select(.key != "sample_spec") | (.value.litellm_provider? // "")] as $ps
       | reduce $core[] as $p ({}; .[$p] = ([$ps[] | select(. == $p)] | length)))}' "$1"
}
temporary_file=$(mktemp)
trap 'rm -f "$temporary_file"' EXIT

curl --fail --silent --show-error --location \
  --retry 3 --retry-all-errors \
  "$source_url" \
  --output "$temporary_file"

"$repo_root/scripts/validate-model-registry.sh" "$temporary_file"

new_count=$(jq 'length - 1' "$temporary_file")
new_bytes=$(wc -c < "$temporary_file")
if [[ -f "$destination" ]]; then
  old_count=$(jq 'length - 1' "$destination")
  old_bytes=$(wc -c < "$destination")

  # Per-run shrink guard.
  if (( new_count < old_count * 3 / 4 )); then
    echo "refusing registry shrink from $old_count to $new_count entries" >&2
    exit 1
  fi

  # Growth guard: a sudden multiple of the catalog is junk injection, not
  # organic model growth.
  if (( new_count > old_count * 4 )); then
    echo "refusing registry growth from $old_count to $new_count entries" >&2
    exit 1
  fi

  # Byte-size guards: entry counts alone cannot stop one giant injected field
  # from bloating the embedded JSON (and thus the crate) arbitrarily.
  if (( new_bytes > old_bytes * 4 )) || (( new_bytes < old_bytes / 4 )); then
    echo "refusing registry byte-size jump from $old_bytes to $new_bytes bytes" >&2
    exit 1
  fi

  # Capability-coverage delta guard: output-config support must not collapse
  # relative to the data we already trust.
  old_output_config=$(jq '[.[] | select(.supports_output_config? == true)] | length' "$destination")
  new_output_config=$(jq '[.[] | select(.supports_output_config? == true)] | length' "$temporary_file")
  if (( new_output_config < old_output_config / 2 )); then
    echo "refusing output-config coverage collapse from $old_output_config to $new_output_config entries" >&2
    exit 1
  fi

  # Provider-identity durability: entries that are core-provider in the
  # trusted file must never be quietly reassigned to tail providers — a
  # merged demotion becomes "trusted" for the next run, so any per-run
  # allowance compounds into a ratchet. Zero tolerance; a legitimate
  # upstream migration fails closed into the tracking issue for a human.
  reassigned=$(jq -n \
    --slurpfile old "$destination" --slurpfile new "$temporary_file" \
    --argjson core "$core_providers" '
    [$old[0] | to_entries[]
     | select(.key != "sample_spec")
     | select((.value.litellm_provider? // "") as $p | $core | index($p))
     | select($new[0][.key] != null)
     | select(($new[0][.key].litellm_provider? // "") != (.value.litellm_provider? // ""))
     | .key]')
  if [[ "$(jq 'length' <<<"$reassigned")" != "0" ]]; then
    echo "refusing core-provider reassignment: $(jq -rc '.[:10]' <<<"$reassigned")" >&2
    exit 1
  fi
fi

# Absolute size ceiling, independent of history.
if (( new_bytes > 20000000 )); then
  echo "refusing registry larger than 20MB: $new_bytes bytes" >&2
  exit 1
fi

# Non-compounding shrink guards: measure the total AND each core provider's
# entry count against the high-water profile ever accepted (seeded from the
# trusted committed file when absent). A slow daily shrink cannot ratchet the
# catalog down 25% at a time, and one core family cannot be hollowed out while
# the total stays within tolerance.
new_profile=$(profile_of "$temporary_file")
if [[ -f "$baseline_file" ]]; then
  baseline_profile=$(<"$baseline_file")
elif [[ -f "$destination" ]]; then
  baseline_profile=$(profile_of "$destination")
else
  baseline_profile=$new_profile
fi

shrink_violations=$(jq -n --argjson new "$new_profile" --argjson base "$baseline_profile" '
  [ (if $new.total * 4 < $base.total * 3 then "total \($new.total) vs high-water \($base.total)" else empty end),
    ($base.core_providers | to_entries[]
     | select((($new.core_providers[.key] // 0) * 4) < (.value * 3))
     | "\(.key) \($new.core_providers[.key] // 0) vs high-water \(.value)") ]')
if [[ "$(jq 'length' <<<"$shrink_violations")" != "0" ]]; then
  echo "refusing shrink below high-water baseline: $(jq -rc '.' <<<"$shrink_violations")" >&2
  exit 1
fi

if cmp --silent "$temporary_file" "$destination"; then
  echo "model registry is already current"
  exit 0
fi

mv "$temporary_file" "$destination"
jq -n --argjson new "$new_profile" --argjson base "$baseline_profile" '
  {total: ([$new.total, $base.total] | max),
   core_providers: ($new.core_providers
     | with_entries(.value = ([.value, ($base.core_providers[.key] // 0)] | max)))}' \
  > "$baseline_file"
echo "updated model registry to $new_count entries (high-water $(jq -c . "$baseline_file"))"
