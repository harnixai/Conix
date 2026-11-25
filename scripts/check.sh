#!/usr/bin/env bash
# 一批改动做完跑一次：格式、clippy、全部测试。
set -euo pipefail
cd "$(dirname "$0")/.."
# 不带 --all：--all 连 agix 子模块也格式化。流程库（crates/flow）自己的 rustfmt.toml 关掉了格式化。
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
