#!/bin/sh
# 项目的验证：xforge 流程实施站的出口门禁 unit-tests 跑它，退出码 0 才过。按项目自己的需要写。
set -e
cd "${FLOW_WORKSPACE:-$(dirname "$0")/../..}"
python3 -m py_compile src/textkit/__init__.py
python3 -m unittest discover -s tests -q
