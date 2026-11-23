#!/usr/bin/env bash
# 按 3rds/INDEX.md 把第三方代码拷到 3rds/：原样整目录拷入，本地不改。从来源仓库的发布标签取
# （git archive），与本地检出停在哪里无关。
set -euo pipefail
cd "$(dirname "$0")/.."
HARNIX_DIR="${HARNIX_DIR:-../Harnix}"
HARNIX_TAG="Harnix-v0.4.7-2026111901"

if ! git -C "$HARNIX_DIR" rev-parse -q --verify "refs/tags/$HARNIX_TAG" >/dev/null; then
  git -C "$HARNIX_DIR" fetch -q origin tag "$HARNIX_TAG" || {
    echo "$HARNIX_DIR 里没有标签 ${HARNIX_TAG}，也取不到" >&2
    exit 1
  }
fi
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
git -C "$HARNIX_DIR" archive "$HARNIX_TAG" crates/flow | tar -x -C "$tmp"
rm -rf 3rds/harnix-flow
mv "$tmp/crates/flow" 3rds/harnix-flow
echo "3rds/harnix-flow ← ${HARNIX_TAG}:crates/flow"
