#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: ./scripts/release.sh [--ci] [--no-push] <version>

Examples:
  ./scripts/release.sh 0.1.5
  ./scripts/release.sh --ci 0.1.5
  ./scripts/release.sh --no-push --allow-empty --skip-verify 99.9.9

This script:
1. Verifies you are on a clean, up-to-date main branch
2. Creates release/vX.Y.Z
3. Moves CHANGELOG.md's Unreleased section into a versioned release entry
4. Bumps the package version in Cargo.toml
5. Runs cargo check/test/clippy/fmt release gates
6. Commits the release bump as chore(release): vX.Y.Z
7. Pushes the branch
8. Creates a GitHub PR with gh and enables squash auto-merge

Flags:
  --ci          GitHub Actions mode: allow a detached HEAD at origin/main,
                require GH_TOKEN, fail if squash auto-merge cannot be enabled
  --no-push     Prepare and commit locally; skip origin/main preflight, push, and PR
  --allow-empty Allow an empty CHANGELOG Unreleased section
  --skip-verify Skip cargo check/test/clippy/fmt
  --no-auto-merge  Create the PR without enabling auto-merge

After the PR merges, the release workflow creates the v* tag with the
release App, publishes the crate to crates.io, and creates the GitHub
release.
EOF
}

version=""
allow_empty=0
auto_merge=1
skip_verify=0
ci=0
no_push=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    -h|--help)
      usage
      exit 0
      ;;
    --allow-empty)
      allow_empty=1
      shift
      ;;
    --skip-verify)
      skip_verify=1
      shift
      ;;
    --no-auto-merge)
      auto_merge=0
      shift
      ;;
    --ci)
      ci=1
      shift
      ;;
    --no-push)
      no_push=1
      shift
      ;;
    --*)
      echo "error: unknown option: $1" >&2
      usage >&2
      exit 1
      ;;
    *)
      if [ -n "$version" ]; then
        echo "error: version already set to $version; unexpected extra argument: $1" >&2
        usage >&2
        exit 1
      fi
      version="$1"
      shift
      ;;
  esac
done

[ -n "$version" ] || { usage >&2; exit 1; }

version="${version#v}"
if [ "$ci" -eq 1 ]; then
  printf '%s' "$version" | grep -Eq '^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$' || {
    echo "error: version must be X.Y.Z, got: $version" >&2
    exit 1
  }
else
  printf '%s' "$version" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+([-.][0-9A-Za-z.]+)?$' || {
    echo "error: version must be X.Y.Z or X.Y.Z-rc.1, got: $version" >&2
    exit 1
  }
fi

if [ "$ci" -eq 1 ]; then
  export GIT_TERMINAL_PROMPT=0
  if [ "$no_push" -eq 0 ] && [ -z "${GH_TOKEN:-}" ]; then
    echo "error: GH_TOKEN is required with --ci" >&2
    exit 1
  fi
  if [ -z "$(git config --get user.email || true)" ]; then
    git config user.name "github-actions[bot]"
    git config user.email "41898282+github-actions[bot]@users.noreply.github.com"
  fi
fi

release_date="$(date -u +%Y-%m-%d)"
tag="v${version}"
branch="release/${tag}"

if [[ -n "$(git status --porcelain)" ]]; then
  echo "error: working tree has uncommitted changes" >&2
  exit 1
fi

git fetch --quiet origin main --tags

local_head="$(git rev-parse HEAD)"
remote_head="$(git rev-parse origin/main)"

if [ "$no_push" -eq 0 ]; then
  [ "$local_head" = "$remote_head" ] || {
    echo "error: local main is not at origin/main; pull or reset before releasing" >&2
    exit 1
  }
fi

current_branch="$(git branch --show-current)"
if [ "$current_branch" != "main" ]; then
  if [ "$ci" -eq 1 ] && [ "$no_push" -eq 0 ] && [ "$local_head" = "$remote_head" ]; then
    git switch -C main origin/main
  elif [ "$no_push" -eq 0 ]; then
    echo "error: releases must be prepared from main; current branch is ${current_branch:-detached}" >&2
    exit 1
  fi
fi

if git show-ref --verify --quiet "refs/heads/${branch}"; then
  echo "error: branch already exists locally: ${branch}" >&2
  exit 1
fi
if [ "$no_push" -eq 0 ]; then
  if git ls-remote --exit-code --heads origin "${branch}" >/dev/null 2>&1; then
    echo "error: branch already exists on origin: ${branch}" >&2
    exit 1
  fi
fi
if git rev-parse -q --verify "refs/tags/${tag}" >/dev/null 2>&1; then
  echo "error: tag already exists locally: ${tag}" >&2
  exit 1
fi
if [ "$no_push" -eq 0 ]; then
  if git ls-remote --exit-code --tags origin "refs/tags/${tag}" >/dev/null 2>&1; then
    echo "error: tag already exists on origin: ${tag}" >&2
    exit 1
  fi
fi

python3 - "$version" <<'PY'
import pathlib
import re
import sys

requested = sys.argv[1]
text = pathlib.Path("Cargo.toml").read_text(encoding="utf-8")
match = re.search(r'(?m)^version = "([^"]+)"', text)
if not match:
    raise SystemExit("error: could not read version from Cargo.toml")
current = match.group(1)


def core(value: str) -> tuple[int, int, int]:
    numeric = value.split("+", maxsplit=1)[0].split("-", maxsplit=1)[0]
    parts = numeric.split(".")
    if len(parts) != 3 or not all(part.isdigit() for part in parts):
        raise SystemExit(f"error: cannot compare versions: {value}")
    return int(parts[0]), int(parts[1]), int(parts[2])


if core(requested) <= core(current):
    raise SystemExit(
        f"error: version {requested} must be newer than current {current}"
    )
print(f"current version {current} -> {requested}")
PY

git switch -c "$branch"

python3 - "$version" "$release_date" "$allow_empty" <<'PY'
import pathlib
import sys

version, release_date, allow_empty = sys.argv[1], sys.argv[2], sys.argv[3] == "1"

path = pathlib.Path("CHANGELOG.md")
text = path.read_text(encoding="utf-8")
marker = "## [Unreleased]"
start = text.find(marker)
if start < 0:
    raise SystemExit("error: CHANGELOG.md is missing the Unreleased section")

rest = text[start + len(marker):]
import re
match = re.search(r"(?m)^## \[", rest)
unreleased_body = rest[:match.start()] if match else rest

if not unreleased_body.strip() and not allow_empty:
    raise SystemExit("error: CHANGELOG.md Unreleased section is empty; add release notes first or pass --allow-empty")

release_header = f"\n\n## [{version}] - {release_date}\n"
new_text = text[:start] + marker + release_header + unreleased_body.lstrip("\n")
path.write_text(new_text)
print("prepared CHANGELOG.md")
PY

python3 - "$version" <<'PY'
import pathlib, re, sys

version = sys.argv[1]
path = pathlib.Path("Cargo.toml")
text = path.read_text(encoding="utf-8")
new_text, count = re.subn(
    r'(?m)^version = "[^"]+"$',
    f'version = "{version}"',
    text,
    count=1,
)
if count != 1:
    raise SystemExit("error: could not bump version in Cargo.toml")
path.write_text(new_text)
print("bumped Cargo.toml to", version)
PY

if [ "$skip_verify" -eq 0 ]; then
  echo "Running release gates (cargo check/test/clippy/fmt)..."
  cargo check --workspace --all-targets --quiet
  cargo test --workspace --quiet
  cargo clippy --workspace --all-targets --quiet -- -D warnings
  cargo fmt --all -- --check
else
  echo "Skipping release gates (--skip-verify)"
fi

git add CHANGELOG.md Cargo.toml
git diff --cached --quiet && {
  echo "error: release prep produced no staged changes" >&2
  exit 1
}

git commit -m "chore(release): ${tag}"

if [ "$no_push" -eq 1 ]; then
  echo "Prepared ${tag} on ${branch} (--no-push: not pushing or opening a PR)"
  exit 0
fi

git push -u origin "$branch"

pr_url="$(gh pr create \
  --base main \
  --head "$branch" \
  --title "chore(release): ${tag}" \
  --body "Automated release PR for ${tag}. Merging triggers the release workflow: the release App creates the tag, the crate publishes to crates.io, and the GitHub release is created.")"
echo "Pull request: ${pr_url}"

if [ "$auto_merge" -eq 1 ]; then
  if gh pr merge --auto --squash --delete-branch "${pr_url}"; then
    echo "Auto-merge: enabled (squash)"
  else
    if [ "$ci" -eq 1 ]; then
      echo "error: failed to enable squash auto-merge" >&2
      exit 1
    fi
    echo "warning: failed to enable auto-merge; merge manually or rerun with --no-auto-merge" >&2
  fi
else
  echo "Auto-merge: not enabled — merge manually"
fi
