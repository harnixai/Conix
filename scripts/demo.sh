#!/usr/bin/env bash
# 演示环境 demo/（不入库）：独立的 CONIX_HOME、一个示例项目、凭据与模型配置，再用真实模型
# 灌一批数据，让会话树、todo、子 Agent、后台任务、goal、待答提问、作业都有东西可看。
#
#   scripts/demo.sh            建 demo/ 并灌数据（凭据取 ../Agix/.env，没有就取 ./.env）
#   demo/live.sh               打开真实实例（之后再开就是接回）
#   demo/mock.sh               打开 mock 演示（不起实例、不调模型）
#   demo/stop.sh               停掉演示实例
#   scripts/demo.sh reset      停掉并删掉 demo/，从头再来
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT=$PWD
D=$ROOT/demo
BIN=$ROOT/target/debug/conix

if [[ "${1:-}" == "reset" ]]; then
    [[ -x $D/stop.sh ]] && "$D/stop.sh" || true
    rm -rf "$D"
    shift
fi

ENV_SRC=""
for f in "$ROOT/../Agix/.env" "$ROOT/.env"; do
    [[ -f $f ]] && { ENV_SRC=$f; break; }
done
[[ -n $ENV_SRC ]] || { echo "找不到凭据：../Agix/.env 或 ./.env" >&2; exit 1; }

echo "== 编译"
cargo build -q -p conix --bin conix --example demo_seed

echo "== 建 demo/"
mkdir -p "$D/home" "$D/shop"
install -m 600 "$ENV_SRC" "$D/env"
set -a; source "$D/env"; set +a
# Agix 不读 ANTHROPIC_MODEL：模型名写进它的设置。提问的超时放长，留给人看的那个不会自己撤掉。
# 允许客户端放宽权限模式：演示里 Shift+Tab 才能切到"自动接受编辑"（缺省设置下只能收紧）。
cat > "$D/home/settings.json" <<JSON
{
  "model": { "model_name": "${ANTHROPIC_MODEL:-claude-sonnet-4-6}" },
  "allow_client_permission_override": true,
  "execution": { "permission_prompt_timeout_secs": 86400 }
}
JSON

if [[ ! -f $D/shop/shop.py ]]; then
    cat > "$D/shop/shop.py" <<'PY'
"""一个很小的购物清单命令行：add / list / done / total。数据存在 items.json。"""
import json, sys
from pathlib import Path

DB = Path(__file__).with_name("items.json")

def load():
    return json.loads(DB.read_text()) if DB.exists() else []

def save(items):
    DB.write_text(json.dumps(items, ensure_ascii=False, indent=2))

def add(name, price):
    items = load()
    items.append({"name": name, "price": float(price), "done": False})
    save(items)

def done(index):
    items = load()
    items[int(index)]["done"] = True   # bug：下标从 1 开始给人看，这里却按 0 开始
    save(items)

def total():
    return sum(i["price"] for i in load() if i["done"] == False) + 0.1   # bug：多加了 0.1

def main(argv):
    cmd, *rest = argv or ["list"]
    if cmd == "add":
        add(*rest)
    elif cmd == "done":
        done(*rest)
    elif cmd == "total":
        print(f"{total():.2f}")
    else:
        for n, i in enumerate(load(), 1):
            print(f"{n}. [{'x' if i['done'] else ' '}] {i['name']} {i['price']}")

if __name__ == "__main__":
    main(sys.argv[1:])
PY
    cat > "$D/shop/test_shop.py" <<'PY'
import shop

def test_total_counts_open_items(tmp_path, monkeypatch):
    monkeypatch.setattr(shop, "DB", tmp_path / "items.json")
    shop.add("milk", 3)
    shop.add("bread", 2)
    shop.done(1)
    assert shop.total() == 2
PY
    printf '[{"name": "苹果", "price": 6.5, "done": false}]\n' > "$D/shop/items.json"
    git -C "$D/shop" init -q 2>/dev/null || true
fi

cat > "$D/live.sh" <<SH
#!/usr/bin/env bash
# 打开演示的真实实例：在示例项目里运行 conix，就接入（或拉起）它的实例。
set -a; source "$D/env"; set +a
export CONIX_HOME="$D/home"
cd "$D/shop" && exec "$BIN" "\$@"
SH
cat > "$D/mock.sh" <<SH
#!/usr/bin/env bash
# mock 演示：脚本化的数据，不起实例、不调模型。
export CONIX_HOME="$D/home"
exec "$BIN" --mock "\$@"
SH
cat > "$D/stop.sh" <<SH
#!/usr/bin/env bash
export CONIX_HOME="$D/home"
for n in \$("$BIN" list --all 2>/dev/null | awk '{print \$1}'); do "$BIN" stop "\$n"; done
SH
chmod +x "$D"/*.sh

echo "== 灌数据（真实模型，约一两分钟）"
"$BIN" --version >/dev/null   # 新编出的二进制第一次执行 macOS 要扫描，先跑一次
CONIX_HOME=$D/home "$ROOT/target/debug/examples/demo_seed" "$BIN" "$D/shop"

cat <<TXT

演示准备好了：
  demo/live.sh    打开真实实例（会话、子 Agent、任务、goal、待答提问、作业、流程运行）
  demo/mock.sh    打开 mock 演示（脚本数据，不起实例、不调模型）
  demo/stop.sh    停掉演示实例
TXT
