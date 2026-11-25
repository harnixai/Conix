#!/bin/sh
# command 门禁 brief-stats 跑的脚本：在运行的输出目录里数简报的字数，把门禁结果打成一行 JSON（result: json）。
# 退出码不看，看打印出来的 verdict。
f=${1:-brief.md}
if [ ! -f "$f" ]; then
    printf '{"verdict":"error","summary":"%s 不存在"}\n' "$f"
    exit 0
fi
chars=$(LC_ALL=en_US.UTF-8 wc -m < "$f" | tr -d ' ')
lines=$(wc -l < "$f" | tr -d ' ')
if [ "$chars" -lt 300 ]; then
    printf '{"verdict":"fail","summary":"只有 %s 字，至少 300","findings":[{"rule":"length","severity":"error","path":"%s","message":"太短：%s 字"}],"metrics":{"chars":%s,"lines":%s}}\n' "$chars" "$f" "$chars" "$chars" "$lines"
elif [ "$chars" -gt 6000 ]; then
    printf '{"verdict":"fail","summary":"%s 字，至多 6000","findings":[{"rule":"length","severity":"error","path":"%s","message":"太长：%s 字"}],"metrics":{"chars":%s,"lines":%s}}\n' "$chars" "$f" "$chars" "$chars" "$lines"
else
    printf '{"verdict":"pass","summary":"%s 字，%s 行","metrics":{"chars":%s,"lines":%s}}\n' "$chars" "$lines" "$chars" "$lines"
fi
