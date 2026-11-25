# Conix 演示项目：笔记整理成简报

一个很小的项目，用来看 Conix 的**流程**和**作业**都能做什么。`notes/` 里是三份关于服务可靠性的笔记；
流程把它们整理成一份简报：每一站都有门禁把关，两个模型并行审读，按项目的治理文件下结论，人签发之后再经外部审批发布；
作业每天、每周自动跑一次。

## 怎么跑

```sh
cp -r examples/demo /tmp/conix-demo && cd /tmp/conix-demo && git init -q && git add -A && git commit -qm demo
conix            # 在这个目录里起实例并接入
/trust           # 信任这个项目：读它自己的流程、作业与设置
```

然后在流程标签里看流程，在对话里说"用 brief 流程把笔记整理成简报，主题是服务可靠性"，或者直接
`flow_start`。作业在作业标签里，克隆下来是停用的，看过再启用。

流程里写的 provider（`deepseek`、`claude`）与模型（`deepseek-flash`、`claude-haiku-5-5`、`claude-sonnet-5-5`）
换成你的设置里有的。

## 流程

| 流程 | 演示什么 |
|---|---|
| `brief` | 一次运行走遍流程的功能：多站、`foreach` 子 Agent、静态 `tasks` 两个工作项并行且各用一家模型；三类门禁（`check` 读 YAML / JSONL / Markdown，`command` 跑项目自己的命令行，`script` 读治理文件下结论）；不过自动打回；`sealed`；人签发；MCP 外部审批 |
| `digest` / `digest-note` | 子流程：每份笔记开一个子运行，子流程声明 `result`；`children` 出口 `require: any`，个别失败也能过；`check: output` 查汇总 |
| `publish` | 单独的外部审批：`via: mcp` 经 MCP 工具建单、轮询。审批系统是 `tools/approvals_mcp.py` 模拟的 |

### `brief` 的每一站

```
outline（主会话）→ sections（每节一个子 Agent）→ assemble（独立会话）
  → reviews（facts 在 DeepSeek ∥ policy 在 Claude，同时跑）→ govern（治理文件下结论）
  → sign-off（人签发）→ publish（外部审批）
```

| 站 | 交付 | 门禁与出口 |
|---|---|---|
| outline | `outline.yaml`：2–3 节，每节列出来源笔记 | `outline-shape`（`check: schema`，按扩展名读 YAML） |
| sections | `sections/<节>.md` | `sections-written`（`check: files`，不能有 TODO） |
| assemble | `brief.md`；交付前自己跑一次 `notesctl stats` | `brief-shape`（`check: text`，要有摘要、结论、来源）、`brief-stats`（`command`：`tools/notesctl.py stats` 数字数） |
| reviews | `reviews/facts.jsonl`（每条说法、依据的笔记、原文引文）、`reviews/policy.jsonl`（治理文件的每条 style 规矩判没判过） | `facts-shape`、`policy-shape`（`check: schema`，JSONL 读成数组）、`reviews-verified`（`command`：`tools/notesctl.py verify-reviews` 逐条到笔记里找引文） |
| govern | — | `governance`（`script`，QuickJS：读 `governance/policy.yaml`，对照提纲、简报、两份审读，结论是"可以签发"或"不可签发"加上每一处） |
| sign-off | — | `untouched`（`check: sealed`：过了门禁的文件后来没被改过），然后人 `confirm` |
| publish | — | `confirm via: mcp`：在外部审批系统建单，批了才过 |

交付物在 `briefs/brief-<日期><序号>/`。门禁不过时，`on_unmet` 把运行打回到那一站（govern 不过打回 assemble，
审读跟着重做），新一轮的会话会拿到每条 finding。

### 治理文件

`governance/policy.yaml` 是这个项目给简报定的规矩：要有哪几节、不能用哪些说法、多长；事实审读里至多几条没依据；
`style` 里几条要读懂了才判得了的规矩和它们的 severity。审读员（模型）照 `style` 逐条判，脚本门禁 `governance`
照整份文件下结论。改规矩改这个文件，不改门禁。

### 项目自己的命令行

`tools/notesctl.py`（python3，只用标准库）是 command 门禁跑的工具，也可以在终端里用：

```sh
python3 tools/notesctl.py stats briefs/<运行>/brief.md
python3 tools/notesctl.py verify-reviews briefs/<运行>/reviews
```

每个子命令打一行门禁结果的 JSON（`{verdict, summary, findings, metrics}`）；门禁写的是 `result: json`。

### 人的决定

确认、驳回、放行、强制放行只有人能做，模型没有这几个工具。在输入框里打：

```
/confirm            确认（运行和站不写，取流程标签里选中的运行、唯一在等的站）
/reject 结论太空     驳回，要写理由
```

`who: editor` 按 `.conix/roles.yaml` 核对：列表是空的，任何人都能确认；写上名字（操作系统用户名）就只有他们能。

### 外部审批（`brief` 的最后一站，和 `publish`）

`.conix/settings.json` 配了一个本地的 MCP server `approvals`（`tools/approvals_mcp.py`，要 python3）。运行走到
审批站时经 Agix 的 `mcp.callTool` 建一张单，之后每 30 秒查一次。在终端里批：

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
governance/policy.yaml      治理文件：简报要守的规矩
tools/notesctl.py           项目自己的命令行，command 门禁跑它
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
