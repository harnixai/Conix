# Conix 演示项目：笔记整理成简报

一个很小的项目，用来看 Conix 的**流程**和**作业**都能做什么。`notes/` 里是三份关于服务可靠性的笔记；
流程把它们整理成一份简报，每一站都有把关，最后由人签发；作业每天、每周自动跑一次。

## 怎么跑

```sh
cp -r examples/demo /tmp/conix-demo && cd /tmp/conix-demo && git init -q && git add -A && git commit -qm demo
conix            # 在这个目录里起实例并接入
/trust           # 信任这个项目：读它自己的流程、作业与设置
```

然后在流程标签里看流程，在对话里说"用 brief 流程把笔记整理成简报，主题是服务可靠性"，或者直接
`flow_start`。作业在作业标签里，克隆下来是停用的，看过再启用。

流程里写的模型（`claude-haiku-5-5`、`claude-sonnet-5-5`）换成你的 provider 有的模型。

## 流程

| 流程 | 演示什么 |
|---|---|
| `brief` | 多站、`foreach`、子 Agent 站与独立会话站各用一个模型；三类门禁（`check`、`script`、`command`）；不过自动打回；最后一站 `sealed` 加人的确认（`who: editor`，3 天时限） |
| `digest` / `digest-note` | 子流程：每份笔记开一个子运行，子流程声明 `result`；`children` 出口 `require: any`，个别失败也能过；`check: output` 查汇总 |
| `publish` | 外部审批：`via: mcp` 经 MCP 工具建单、轮询。审批系统是 `tools/approvals_mcp.py` 模拟的；要 Agix 的 `mcp.callTool`（harnixai/Agix#111） |

### `brief` 的每一站

```
outline（主会话）→ sections（每节一个子 Agent，haiku）→ assemble（独立会话，sonnet）→ review（人签发）
```

| 站 | 交付 | 门禁 |
|---|---|---|
| outline | `outline.json`：2–3 节，每节列出来源笔记 | `outline-shape`（`check: schema`） |
| sections | `sections/<节>.md` | `sections-written`（`check: files`，不能有 TODO） |
| assemble | `brief.md` | `brief-shape`（`check: text`，要有摘要、结论、来源）、`sources-cited`（`script`：提纲里的每个来源都列进了简报）、`brief-stats`（`command`：`tools/brief-stats.sh` 数字数，打印 JSON 结果） |
| review | — | `untouched`（`check: sealed`：过了门禁的文件后来没被改过），然后 `confirm` |

交付物在 `briefs/brief-<日期><序号>/`。门禁不过时，`on_unmet` 把运行打回到那一站，新一轮的会话会拿到每条 finding。

### 人的决定

确认、驳回、放行、强制放行只有人能做，模型没有这几个工具。在输入框里打：

```
/confirm            确认（运行和站不写，取流程标签里选中的运行、唯一在等的站）
/reject 结论太空     驳回，要写理由
```

`who: editor` 按 `.conix/roles.yaml` 核对：列表是空的，任何人都能确认；写上名字（操作系统用户名）就只有他们能。

### 外部审批（`publish`）

`.conix/settings.json` 配了一个本地的 MCP server `approvals`（`tools/approvals_mcp.py`，要 python3）。运行走到
审批站时建一张单，之后每 30 秒查一次。在终端里批：

```sh
python3 tools/approvals_mcp.py list
python3 tools/approvals_mcp.py approve <单号>     # 或 reject
```

## 作业（`.conix/jobs.json`）

| 作业 | 什么时候 | 做什么 |
|---|---|---|
| 笔记巡检 | 每天 9:00 | 简单作业：开一个会话，检查 `notes/` 里有没有空文件、TODO，回报结果 |
| 每周简报 | 每周一 10:00 | 流程作业：起一次 `brief` |

## 文件

```
notes/                      素材
tools/brief-stats.sh        command 门禁跑的脚本
tools/approvals_mcp.py      模拟的外部审批系统（MCP server）
.conix/flows/*.yaml         流程
.conix/flows/gates/         门禁
.conix/flows/scripts/       script 门禁的 JS
.conix/flows/skills/        每一站照着做的 skill
.conix/roles.yaml           确认的角色
.conix/jobs.json            作业
.conix/settings.json        项目的 Agix 设置（MCP server）
```

写流程、写门禁的规则见流程库的指南：`crates/flow/docs/guide.zh-CN.md` 与 `crates/flow/docs/guide/`（会话里是
`flow_guide`，门禁一章是 `flow_guide {topic: "gates"}`）。
