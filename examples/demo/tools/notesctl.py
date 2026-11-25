#!/usr/bin/env python3
"""演示项目自己的命令行：command 门禁跑它，写简报的会话交付前也可以自己跑一次。只用标准库。

    python3 tools/notesctl.py stats <brief.md> [--min 300] [--max 6000]
    python3 tools/notesctl.py verify-reviews <reviews 目录> [--notes notes]

每个子命令往标准输出打一行门禁结果的 JSON（`{verdict, summary, findings, metrics}`），command 门禁写的是
`result: json`，看的是这一行，不看退出码。退出码照常：pass 是 0，fail 是 1，判不了是 2，方便人在终端里用。

为什么不在 script 门禁里做 verify-reviews：要按笔记逐条找原文，还要容忍模型抄引文时的空白与标点差别，
这是项目自己的工具该懂的规矩；门禁只管跑它、记下它的结论。
"""

import argparse
import json
import os
import re
import sys


def emit(verdict, summary, findings=(), metrics=None):
    out = {"verdict": verdict, "summary": summary, "findings": list(findings)}
    if metrics:
        out["metrics"] = metrics
    print(json.dumps(out, ensure_ascii=False))
    return {"pass": 0, "fail": 1}.get(verdict, 2)


def finding(rule, message, path=None, line=None, severity="error"):
    f = {"rule": rule, "severity": severity, "message": message}
    if path:
        f["path"] = path
    if line:
        f["line"] = line
    return f


def stats(args):
    if not os.path.isfile(args.file):
        return emit("error", f"{args.file} 不存在")
    with open(args.file, encoding="utf-8") as f:
        text = f.read()
    chars = len(text)
    lines = text.count("\n") + (0 if text.endswith("\n") or not text else 1)
    sections = len(re.findall(r"(?m)^## ", text))
    metrics = {"chars": chars, "lines": lines, "sections": sections}
    if chars < args.min:
        return emit("fail", f"只有 {chars} 字，至少 {args.min}",
                    [finding("length", f"太短：{chars} 字", args.file)], metrics)
    if chars > args.max:
        return emit("fail", f"{chars} 字，至多 {args.max}",
                    [finding("length", f"太长：{chars} 字", args.file)], metrics)
    return emit("pass", f"{chars} 字，{sections} 节", metrics=metrics)


def squash(s):
    """比对引文时不看空白、标点与 Markdown 记号：模型抄一句话常把全角半角、列表符号弄得不一样。"""
    return re.sub(r"[\s\W_]+", "", s, flags=re.UNICODE).lower()


def verify_reviews(args):
    facts = os.path.join(args.dir, "facts.jsonl")
    if not os.path.isfile(facts):
        return emit("error", f"{facts} 不存在")
    found = []
    claims = supported = 0
    with open(facts, encoding="utf-8") as f:
        rows = [(i, l) for i, l in enumerate(f, 1) if l.strip()]
    for n, raw in rows:
        try:
            row = json.loads(raw)
        except json.JSONDecodeError as e:
            found.append(finding("parse", f"不是一行 JSON：{e}", facts, n))
            continue
        claims += 1
        if row.get("verdict") != "supported":
            continue
        supported += 1
        src, quote = row.get("source", ""), row.get("quote", "")
        if not re.fullmatch(r"notes/[a-z0-9-]+\.md", src or ""):
            found.append(finding("source", f"来源写成了 {src!r}，要是 notes/<名>.md", facts, n))
            continue
        path = os.path.join(args.notes, os.path.basename(src))
        if not os.path.isfile(path):
            found.append(finding("source", f"{src} 不存在", facts, n))
            continue
        with open(path, encoding="utf-8") as f:
            note = squash(f.read())
        q = squash(quote)
        if len(q) < 6:
            found.append(finding("quote", "引文太短，至少要有一句原文", facts, n))
        elif q not in note:
            found.append(finding("quote", f"引文在 {src} 里找不到：{quote[:40]}", facts, n))
    metrics = {"claims": claims, "supported": supported, "checked": supported}
    if claims == 0:
        return emit("fail", "事实核对一条都没有", [finding("empty", "facts.jsonl 是空的", facts)], metrics)
    if found:
        return emit("fail", f"{len(found)} 条核对不上原文", found, metrics)
    return emit("pass", f"{supported} 条有依据的说法都在原文里找到了（共 {claims} 条）", metrics=metrics)


def main():
    p = argparse.ArgumentParser(prog="notesctl", description="演示项目的笔记与简报工具")
    sub = p.add_subparsers(dest="cmd", required=True)
    s = sub.add_parser("stats", help="数简报的字数与节数")
    s.add_argument("file")
    s.add_argument("--min", type=int, default=300)
    s.add_argument("--max", type=int, default=6000)
    v = sub.add_parser("verify-reviews", help="核对事实审读里每条引文都出自它说的那份笔记")
    v.add_argument("dir")
    v.add_argument("--notes", default="notes")
    args = p.parse_args()
    return stats(args) if args.cmd == "stats" else verify_reviews(args)


if __name__ == "__main__":
    sys.exit(main())
