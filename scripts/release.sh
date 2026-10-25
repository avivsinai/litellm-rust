#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: ./scripts/release.sh <version>

Examples:
  ./scripts/release.sh 0.1.5

This script:
1. Verifies you are on a clean, up-to-date main branch
2. Creates release/vX.Y.Z
3. Moves CHANGELOG.md's Unreleased section into a versioned release entry
4. Bumps [workspace.package].version in Cargo.toml
5. Runs cargo check/test/clippy/fmt release gates
6. Commits the release bump as chore(release): vX.Y.Z
7. Pushes the branch
8. Creates a GitHub PR with gh and enables squash auto-merge

After the PR merges, the release workflow creates the v* tag with the
release App, publishes the crate to crates.io, and creates the GitHub
release.
EOF
}

version=""
allow_empty=0
auto_merge=1
skip_verify=0

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
printf '%s' "$version" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+([-.][0-9A-Za-z.]+)?$' || {
  echo "error: version must be X.Y.Z or X.Y.Z-rc.1, got: $version" >&2
  exit 1
}

release_date="$(date -u +%Y-%m-%d)"
tag="v${version}"
branch="release/${tag}"

if [[ -n "$(git status --porcelain)" ]]; then
  echo "error: working tree has uncommitted changes" >&2
  exit 1
fi

current_branch="$(git branch --show-current)"
[ "$current_branch" = "main" ] || {
  echo "error: releases must be prepared from main; current branch is $current_branch" >&2
  exit 1
}

git fetch --quiet origin main --tags

local_head="$(git rev-parse HEAD)"
remote_head="$(git rev-parse origin/main)"
[ "$local_head" = "$remote_head" ] || {
  echo "error: local main is not at origin/main; pull or reset before releasing" >&2
  exit 1
}

if git show-ref --verify --quiet "refs/heads/${branch}"; then
  echo "error: branch already exists locally: ${branch}" >&2
  exit 1
fi
if git ls-remote --exit-code --heads origin "${branch}" >/dev/null 2>&1; then
  echo "error: branch already exists on origin: ${branch}" >&2
  exit 1
fi
if git rev-parse -q --verify "refs/tags/${tag}" >/dev/null 2>&1; then
  echo "error: tag already exists locally: ${tag}" >&2
  exit 1
fi
if git ls-remote --exit-code --tags origin "refs/tags/${tag}" >/dev/null 2>&1; then
  echo "error: tag already exists on origin: ${tag}" >&2
  exit 1
fi

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
git push -u origin "$branch"

pr_url="$(gh pr create \
  --base main \
  --head "$branch" \
  --title "chore(release): ${tag}" \
  --body "Automated release PR for ${tag}. Merging triggers the release workflow: the release App creates the tag, the crate publishes to crates.io, and the GitHub release is created.")"
echo "Pull request: ${pr_url}"

if [ "$auto_merge" -eq 1 ]; then
  gh pr merge --auto --squash --delete-branch "${pr_url}" 2>/dev/null \
    && echo "Auto-merge: enabled (squash)" \
    || echo "warning: failed to enable auto-merge; merge manually or rerun with --no-auto-merge" >&2
else
  echo "Auto-merge: not enabled — merge manually"
fi
