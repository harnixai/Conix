#!/usr/bin/env python3
"""演示用的外部审批系统：一个 stdio 的 MCP server，加上给人用的命令行。

流程 `publish` 的审批站经 MCP 调它：`approval_create` 建一张单，`approval_get` 查单的状态。
单存在 `.conix/state/demo-approvals.json`（相对当前目录，也就是项目）。人在终端里批：

    python3 tools/approvals_mcp.py list
    python3 tools/approvals_mcp.py approve <单号>
    python3 tools/approvals_mcp.py reject <单号>

不带参数时作为 MCP server 运行（stdin / stdout 上一行一条 JSON-RPC）。只用标准库。
"""

import json
import os
import sys
import time
import uuid

STORE = os.path.join(".conix", "state", "demo-approvals.json")

TOOLS = [
    {
        "name": "approval_create",
        "description": "Create an approval request; returns its id, status and link.",
        "inputSchema": {
            "type": "object",
            "required": ["title"],
            "properties": {
                "title": {"type": "string"},
                "approver": {"type": "string"},
                "run": {"type": "string"},
                "brief": {"type": "string"},
            },
        },
    },
    {
        "name": "approval_get",
        "description": "Read an approval request: its status (pending, approved, rejected) and who decided.",
        "inputSchema": {"type": "object", "required": ["id"], "properties": {"id": {"type": "string"}}},
    },
]


def load():
    try:
        with open(STORE, encoding="utf-8") as f:
            return json.load(f)
    except (FileNotFoundError, json.JSONDecodeError):
        return {}


def save(db):
    os.makedirs(os.path.dirname(STORE), exist_ok=True)
    tmp = STORE + ".tmp"
    with open(tmp, "w", encoding="utf-8") as f:
        json.dump(db, f, ensure_ascii=False, indent=2)
    os.replace(tmp, STORE)


def create(args):
    db = load()
    rid = "A-" + uuid.uuid4().hex[:6]
    db[rid] = {
        "id": rid,
        "title": args.get("title", ""),
        "approver": args.get("approver"),
        "run": args.get("run"),
        "brief": args.get("brief"),
        "status": "pending",
        "created": int(time.time()),
        "url": f"file://{os.path.abspath(STORE)}#{rid}",
    }
    save(db)
    return db[rid]


def get(args):
    r = load().get(args.get("id", ""))
    if r is None:
        raise ValueError(f"no approval request {args.get('id')!r}")
    return r


def call(name, args):
    if name == "approval_create":
        return create(args)
    if name == "approval_get":
        return get(args)
    raise ValueError(f"no tool {name!r}")


def reply(id_, result=None, error=None):
    msg = {"jsonrpc": "2.0", "id": id_}
    if error is not None:
        msg["error"] = error
    else:
        msg["result"] = result
    sys.stdout.write(json.dumps(msg, ensure_ascii=False) + "\n")
    sys.stdout.flush()


def serve():
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            req = json.loads(line)
        except json.JSONDecodeError:
            continue
        method, id_ = req.get("method"), req.get("id")
        if id_ is None:  # a notification
            continue
        if method == "initialize":
            reply(id_, {
                "protocolVersion": req.get("params", {}).get("protocolVersion", "2025-06-18"),
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "demo-approvals", "version": "1.0.0"},
            })
        elif method == "tools/list":
            reply(id_, {"tools": TOOLS})
        elif method == "tools/call":
            p = req.get("params", {})
            try:
                out = call(p.get("name"), p.get("arguments") or {})
                reply(id_, {
                    "content": [{"type": "text", "text": json.dumps(out, ensure_ascii=False)}],
                    "structuredContent": out,
                })
            except ValueError as e:
                reply(id_, {"content": [{"type": "text", "text": str(e)}], "isError": True})
        elif method == "ping":
            reply(id_, {})
        else:
            reply(id_, error={"code": -32601, "message": f"no method {method}"})


def decide(rid, status):
    db = load()
    if rid not in db:
        sys.exit(f"没有单 {rid}")
    db[rid]["status"] = status
    db[rid]["approver"] = os.environ.get("USER", "someone")
    db[rid]["decided"] = int(time.time())
    save(db)
    print(f"{rid}: {status}")


def main(argv):
    if not argv:
        serve()
    elif argv[0] == "list":
        for r in load().values():
            print(f"{r['id']}  {r['status']:<8}  {r['title']}")
    elif argv[0] in ("approve", "reject") and len(argv) == 2:
        decide(argv[1], "approved" if argv[0] == "approve" else "rejected")
    else:
        sys.exit(__doc__)


if __name__ == "__main__":
    main(sys.argv[1:])
