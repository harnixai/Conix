#!/bin/sh
# 项目的验证：xforge 流程实施站的出口门禁 unit-tests 跑它，退出码 0 才过。按项目自己的需要写。
set -e
cd "${FLOW_WORKSPACE:-$(dirname "$0")/../..}"
# 门禁在只读的沙箱里跑（flows.md §7.14），项目里写不了：字节码缓存放进这次调用自己的临时目录。
export PYTHONPYCACHEPREFIX="${TMPDIR:-/tmp}/pycache"
python3 -m py_compile src/textkit/__init__.py
python3 -m unittest discover -s tests -q
