#!/usr/bin/env bash
# LIVE 测试（testing.md §8）：真实的实例进程、真实的 Agix 与模型，经 TUI 那条接入路径测 Conix
# 自己的功能。凭据取仓库根的 .env（CONIX_LIVE_ENV 可指定别的文件）。
#
#   scripts/live.sh                    全部 live 用例
#   scripts/live.sh live_jobs          只跑一个文件（live_instance / live_attach / live_sessions / live_turns / live_jobs / live_tools / live_flows / live_models / live_tui …）
#   scripts/live.sh live_jobs catch_up 再按用例名过滤
#   scripts/live.sh tty                只跑真终端那一组（live_tty*.rs：伪终端里起真的 conix，键盘进、屏幕出），可再跟用例名过滤
#   scripts/live.sh snaps [用例]       列出拍过的快照（§8.2）；LIVE_FROM=<用例>/<快照> 从那一处续跑，LIVE_SNAPS=0 不拍
#
# 同时起的实例数用 LIVE_THREADS 控制（缺省 4），太多会撞上模型服务的限流。
set -euo pipefail
cd "$(dirname "$0")/.."
threads="${LIVE_THREADS:-4}"
if [[ ${1:-} == snaps ]]; then
    dir=target/tmp/live-snap/${2:-}
    [[ -d $dir ]] || { echo "没有快照：$dir"; exit 0; }
    find "$dir" -name '*.json' | sort | while read -r m; do
        case=$(basename "$(dirname "$m")"); name=$(basename "$m" .json)
        printf '%s/%s  %s\n' "$case" "$name" "$(grep -oE '"(conix|elapsed_s)": [^,]*' "$m" | tr '\n' ' ')"
    done
    exit 0
fi
if [[ ${1:-} == tty ]]; then
    shift
    files=()
    for f in crates/conix/tests/live_tty*.rs; do
        files+=(--test "$(basename "$f" .rs)")
    done
    exec cargo test -p conix --no-fail-fast "${files[@]}" -- --ignored --test-threads="$threads" "$@"
fi
if [[ $# -ge 1 ]]; then
    file=$1; shift
    exec cargo test -p conix --test "$file" -- --ignored --test-threads="$threads" "$@"
fi
files=()
for f in crates/conix/tests/live_*.rs; do
    files+=(--test "$(basename "$f" .rs)")
done
# --no-fail-fast：一个文件挂了，后面的文件照样跑，一次看全。
cargo test -p conix --no-fail-fast "${files[@]}" -- --ignored --test-threads="$threads" --skip conformance || failed=1
# 流程的一致性套件自己就要跑十几轮模型，挤在并发里会撞上它的单轮超时：最后单独跑。
cargo test -p conix --test live_flows -- --ignored --test-threads=1 conformance || failed=1
exit "${failed:-0}"
