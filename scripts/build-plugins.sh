#!/usr/bin/env bash
# 把 plugins/<名>/ 各打成 target/plugins/<名>-<版本>.zip（plugin.toml 在 zip 根上），打印路径。
# 版本取自各自 plugin.toml 的 [plugin] version。每次先删旧包再按名字排序打，不带 .DS_Store，重复运行结果一致。
set -euo pipefail
cd "$(dirname "$0")/.."
out=target/plugins
mkdir -p "$out"
for dir in plugins/*/; do
  name=$(basename "$dir")
  [ -f "$dir/plugin.toml" ] || continue
  version=$(awk '/^\[plugin\]/{p=1;next} /^\[/{p=0} p && /^version[[:space:]]*=/{gsub(/.*=[[:space:]]*"|".*/,"");print;exit}' "$dir/plugin.toml")
  [ -n "$version" ] || { echo "$dir/plugin.toml: no [plugin] version" >&2; exit 1; }
  zip="$PWD/$out/$name-$version.zip"
  rm -f "$zip"
  (cd "$dir" && find . -type f ! -name .DS_Store | sed 's|^\./||' | LC_ALL=C sort | zip -qX "$zip" -@)
  echo "$zip"
done
