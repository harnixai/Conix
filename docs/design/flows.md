# 流程的治理：门禁、确认、交付

本文是流程库（`crates/flow`）在"怎样才算过站"这件事上的设计：三类门禁、统一的门禁结果、确认、强制放行、
受管流程、子流程与子 Agent 的交付。流程的其余部分（站、执行者、派工、运行记录）沿用 Harnix flow，见
`architecture.md` §8.6。

| 依赖 | 内容 |
|---|---|
| Harnix-v0.4.7 | 流程库的分叉基线：`crates/flow` 从这个发布标签的 `crates/flow` 分出来，之后在 Conix 里改，稳定后整理成 issue 提给 Harnix |
| harnixai/Agix#111 | 宿主不带会话直接调 MCP 工具（`mcp.callTool`）：确认走外部审批要用它 |

## 1. 为什么要改

企业的治理要的是"每一站凭证据过，证据事后查得到"。分叉前的流程库做不到：

- **只有插件代码能检查文件。** 门禁三种：`tool`（调宿主的只读工具再断言结果）、`command`（跑一条命令看退出码）、
  `host`（宿主自己的检查）。Conix 不给 `tool` 提供工具、不开 `command`，只剩 `host`，也就是写插件 JS。
- **结论里没有证据。** 门禁结果只有 `{pass, detail}`：看了哪些文件、文件当时是什么内容，运行记录里查不到；门禁没跑起来
  （超时、脚本出错）和证据不够也分不出来。
- **谁都能确认、能强制放行。** Conix 用的是 `AllowAll`：主会话里的模型调 `flow_confirm` / `flow_force`，只靠 Agix 的权限
  询问拦着，会话切到 `bypassPermissions` 就拦不住。项目里存一份同名流程就盖掉了插件下发的那份，门禁随之可以删掉。
- **交付没有形状。** 子流程交给父任务的是子运行每一站输出拼成的对象；`agent` 站的交付由主 Agent 转述，子 Agent 实际
  有没有跑完，流程不核对。

## 2. 门禁：按"检查逻辑写在哪"分三类

| 种类 | 逻辑写在哪 | 谁执行 | 适合 |
|---|---|---|---|
| `check` | YAML（声明） | 流程库内置的检查器（Rust） | 文件在不在、是不是空的、结构对不对、字段取值、正则 |
| `command` | 外部程序 | 流程库起子进程 | 企业已有的检查脚本、第三方扫描器 |
| `script` | JS（或宿主的函数） | 流程库内置的 QuickJS；`function` 形式交给宿主 | 业务语义：要读几个文件、做比对、算指标 |

**能声明的用 `check`，有现成程序的用 `command`，剩下的写 `script`。** 门禁定义是流程包里的件（`gates/<名>.yaml`），
运行开始时与流程定义一起钉住（xops-04）；`script` 的源码是件 `scripts/<名>.js`，同样钉住。运行中改了文件，在跑的运行
仍按开始时的那份判。

分叉前的两种保留，读得出来、照旧能跑：`tool` 不变（Conix 不提供工具，等于用不了）；`host` 就是
`kind: script` 加 `function: <门禁名>`，见 §2.3。

所有路径都相对运行的工作目录（项目，或运行的 worktree），可以写引用（`${output_dir}`、`${inputs.k}`、
`${stations.<站>.output.<字段>}`）。绝对路径、`..` 出界、经符号链接出界的一律拒绝，算 `error`。

### 2.1 `check`

一道门禁一个检查器，`check` 字段选哪一个：

```yaml
name: design-written
kind: check
check: files
paths: ["${output_dir}/design.md", "${output_dir}/adr/*.md"]   # 每一项至少要匹配到一个文件
non_empty: true            # 缺省 true：空文件（只有空白）不算
not_containing: ["<!-- TEMPLATE -->"]   # 可选：还是模板原样的不算
```

```yaml
name: risks-closed
kind: check
check: schema
file: "${output_dir}/risks.jsonl"
as: jsonl                  # yaml | json | jsonl | frontmatter；缺省按扩展名（.md 是 frontmatter）。
                           # 不叫 format：定义文件的 format 是定义格式的版本号
schema:                    # JSON Schema；jsonl 读成数组，yaml / json 读成那个值，frontmatter 读成头部的对象
  type: array
  minItems: 1
  items:
    type: object
    required: [id, owner, status]
    properties: {status: {enum: [closed, accepted]}}
```

| 检查器 | 字段 | 不过的时候 |
|---|---|---|
| `files` | `paths`（glob 可）、`non_empty`、`not_containing` | 每个没匹配到、为空、含禁止串的文件一条 finding |
| `schema` | `file`（glob 可，每个文件各校验）、`as`、`schema` | 解析不了的一条；schema 每个错误一条，带 JSON 指针 |
| `text` | `file`（glob 可）、`matches`、`not_matches`（正则数组） | 每个没匹配上的 `matches`、每处匹配上的 `not_matches` 一条，带行号 |
| `output` | `value`（引用，缺省整个运行上下文）、`schema` | 同 `schema`；用来判前面站交上来的值，比如程序步骤算出的覆盖率 |
| `sealed` | 无 | 这次运行到目前为止记下的每份证据重新算 hash，不一致的一条 |

`sealed` 放在最后一站：过站之后被改过的文件在这里被发现，运行结束不了。

### 2.2 `command`

```yaml
name: semgrep
kind: command
command: semgrep --config p/ci --json . | jq '{verdict: (if (.results|length)==0 then "pass" else "fail" end), summary: "\(.results|length) findings"}'
env: [SEMGREP_APP_TOKEN, CI_*]     # 透传的环境变量，缺省为空
result: json                       # exit_code（缺省）| json
timeout_seconds: 900
```

- **环境变量按白名单透传**，缺省一个不传（只有 `PATH`、`HOME`、语言区域、`TMPDIR` 这些基本的）。名字可以以 `*` 结尾。
  宿主另给一张拒绝表，白名单写了也不传：Conix 的是模型 provider 的密钥（`*_API_KEY`、`ANTHROPIC_*`、`OPENAI_*`、
  `DEEPSEEK_*`）与它自己的 `CONIX_*`、`AGIX_*`。
- **`result: json`**：stdout 整体（或最后一个非空行）是一个门禁结果（§3），按它判；解析不了算 `error`。
  **`exit_code`**：0 过，非 0 不过，stdout/stderr 的尾巴进 `summary`；起不来、超时算 `error`。
- 运行目录下的 `gate-<名>.log` 留着读到的输出（沿用）。
- **谁能跑**：命令门禁会在本机执行任意命令，只在受信的项目里跑；`conix.yaml` 的 `flows.command_gates: off` 整机关掉
  （缺省 `trusted`）。这与 Agix 对项目 hook、项目 MCP stdio server 的规矩一致：项目受信，它的配置才能在本机执行命令。

### 2.3 `script`

```yaml
name: ledgers-closed
kind: script
script: ledgers            # 件 scripts/ledgers.js
params: {kinds: [risk, question]}
timeout_seconds: 30        # 缺省 30，至多 600
```

```js
// scripts/ledgers.js
export default function (input, flow) {
  // input: { gate, run, flow, station, round, inputs, outputs: {<站>: 输出}, output_dir, workspace, params }
  const findings = [];
  for (const kind of input.params.kinds) {
    for (const row of flow.read.jsonl(`${input.output_dir}/${kind}s.jsonl`) ?? []) {
      if (row.status === "open") findings.push({ rule: "closed", path: `${kind}s.jsonl`, message: `${row.id} 未结` });
    }
  }
  return { verdict: findings.length ? "fail" : "pass", summary: `${findings.length} 条未结`, findings };
}
```

- **运行时**：流程库自己起 rquickjs，一次调用一个运行时，跑完就丢。不经 Agix：门禁要的是只读、确定、不碰模型，
  Agix 的插件函数跑在工具管线里（权限、hook、`tool.around`），结论会随会话设置变；运行时也只有几 MB。
- **`flow` 只有只读的东西**：
  - `flow.read.text / json / yaml / jsonl / frontmatter(path)`：读不到是 `null`；
  - `flow.glob(pattern)`、`flow.stat(path)`（`{size, sha256}` 或 `null`）；
  - `flow.schema.validate(value, schema)`（回 `{ok, errors}`）；
  - `flow.log(...)`（进 `gate-<名>.log`）。

  路径规则同上。没有写、没有命令、没有网络、没有 `import`。
- **证据由运行时记**：脚本读过、stat 过的每个文件，路径与 sha256 自动进结果的 `evidence`，脚本自己写的 `evidence`
  不算数——证据造不了假。
- **限额**：时限（中断回调）、内存上限（64 MiB）、返回值至多 1 MiB。
- **返回值**按 §3 校验；不合规、抛错、超时都算 `error`。
- **`function` 形式**：`kind: script` + `function: <名>`（不写 `script`）由宿主执行，流程库只交出名字
  （宿主的 `Gates`）。Conix 里它就是插件函数：名字按插件 `[x-conix.gates]` 找到函数（`plugins.md` §4–§5.2），
  要跑命令、要更长时限的门禁（xforge 跑单测）用它。分叉前的 `kind: host` 读成这个。

## 3. 门禁的结果

三类门禁回同一个形状，流程库按它决定：

```json
{
  "verdict": "pass",
  "summary": "一句话",
  "findings": [{"rule": "closed", "severity": "error", "path": "risks.jsonl", "line": 3, "message": "R-2 未结"}],
  "evidence": [{"path": "flows/x-2026112401/risks.jsonl", "sha256": "…", "size": 1024}],
  "metrics": {"open": 0}
}
```

| `verdict` | 意思 | 站怎么办 |
|---|---|---|
| `pass` | 证据够 | 这一条出口过 |
| `fail` | 证据不够 | 站未达标；有 `on_unmet` 就自动打回，`findings` 随打回交给那一站的会话 |
| `error` | 门禁本身没跑成：超时、出错、路径出界、返回值不合规 | 站未达标，**不自动打回**（模型修不了门禁）；人看过后"重试"重新跑门禁（`Engine::rerun_gates`；TUI 的重试在没有失败任务时就是它） |

- `severity` 是 `error`（缺省）或 `warn`。门禁可以写 `fail_on: warn`：有 warn 也算不过；缺省只看 `verdict`。
- 证据与结论进运行记录（这一条出口的 `CheckRec`）和事件日志（`gate_result`），事后能复核。
- 兼容：插件函数回的旧形状 `{pass, detail}` 读成 `verdict: pass|fail`、`summary: detail`。

## 4. 确认

```yaml
exit:
  - kind: confirm
    who: reviewer          # 角色；不写是"项目的人"
    via: person            # person（缺省）| mcp
    timeout: 3d            # 可选：到时没有决定算不过
```

**确认是人的决定，任何工具调用都不能完成它。** 会话的工具表里没有 `flow_confirm`、`flow_force`、`flow_release`；
授权（`ConixAuthorizer`）也只认人：会话里的模型（不论权限模式）、子 Agent 做这几件事一律拒绝，告诉它"请用户打 `/confirm`"这类；
运行里的会话本来就不能操作运行（沿用）。

**人怎么做**：流程标签不放操作键（interaction §5），这几件事又不能"在对话里说给模型"，所以是人在输入框里打的斜杠命令：
`/confirm`、`/reject <理由>`、`/release`、`/force <理由>`，写法 `[运行] [站] [理由]`，运行与站不写时取选中的运行与唯一在等的站。
命令不进会话，经端口（`RunOp`）直接发给实例，记成 `Actor{kind: "person", id: <操作系统用户名>}`。

**`via: person`**：实例的 socket 是 0600，接得进来的只有实例的主人，所以"人"就是这个操作系统用户。
- `who` 写了角色：项目的 `.conix/roles.yaml`（进 git，`reviewer: [alice, bob]`）里列了这个角色、主人不在里面，
  就不能在本机确认——这种站要"别人"批，用 `via: mcp`。角色没列出来，任何人都行。文件不放在流程包目录
  `.conix/flows/` 里：那里的每个 yaml 都当流程读。

**`via: mcp`**：走外部审批系统（飞书、钉钉、Jira……）。

```yaml
  - kind: confirm
    who: reviewer
    via: mcp
    mcp:
      server: feishu
      request: {tool: approval_create, arguments: {title: "${run} ${station}", approver: "${inputs.reviewer}"}}
      status:  {tool: approval_get, arguments: {id: "${request.id}"}}
      decision: {path: status, approved: [APPROVED], rejected: [REJECTED, CANCELED]}
      by: approver.name     # 可选：结果里谁批的
      link: url             # 可选
      poll: 5m
    timeout: 3d
```

- 站等确认时，派工器调 `request` 建单，单的结果（`${request.*}`）记进运行；之后按 `poll` 调 `status`，按 `decision`
  的路径与取值判批准、驳回或还在等。Conix 不监听端口，收不到回调，只能轮询；实例重启后从运行记录里的单接着查。
- 工具经宿主调（`Approvals`）。Conix 的实现走 Agix 的 `mcp.callTool`（Agix#111）；引擎没有这个能力时，这条出口是
  `error`，写明缺什么。

**每个决定都留记录**：`ConfirmRec{by, via, at, round, pass, reason, ref}`，`ref` 是外部审批单号或链接。驳回就是这一条出口
不过，站未达标，按 `on_unmet` 打回（沿用）。`timeout` 到了没有决定，同样不过，`summary` 写"3 天内没有决定"。

## 5. 强制放行与受管流程

- **强制放行要人、要理由**（理由沿用），记 `ForcedRec{by, reason, unmet}`。
- **受管流程**：插件在 `[x-conix]` 写 `governed = true`（整个包）或 `governed = ["xforge-solid"]`，这些流程：
  - 项目里存同名的不覆盖它：项目那份在库里是被挡下的（`Withheld::Governed`），`/plugin` 与流程标签里说明；
    `flow_create` / `flow_update` 这个名字直接拒绝。
  - 它引用的件（门禁、脚本、规则、策略、skill）只从插件自己的包里取，项目里同名的件不顶替。
  - 运行记录带 `governed: true`；这样的运行不能强制放行。
- 不受管的流程照旧：项目可以覆盖，可以强制放行。

## 6. 交付

### 6.1 子流程的结果

```yaml
name: review
result:
  from: {verdict: "${stations.decide.output.verdict}", notes: "${stations.decide.output.notes}"}
  schema: {type: object, required: [verdict], properties: {verdict: {enum: [approve, reject]}}}
```

- 写了 `result`：子运行结束时按 `from` 取值、按 `schema` 校验，交给父任务的就是它；不合规，父任务失败，写明哪里不对。
- 没写：沿用分叉前的形状（`{<站>: 输出}`）。
- 子运行取消或失败，父任务失败（沿用）。

### 6.2 `children` 出口

```yaml
exit: [{kind: children, require: all}]   # all（缺省）| any | 正整数：至少几个交付了
```

`foreach` 起一批子运行、允许个别失败时用 `require: 3` 或 `any`；还有在跑的就等，都结束了数交付的个数。

### 6.3 子 Agent 的交付要有出处

`agent` 站由主 Agent 派 Agix 子 Agent 去做、再 `flow_submit`。Agix 前台的 `Agent` 调用结果里没有任务 id（只有后台任务的才有），
模型给不出，所以不让模型报：派活时协议定下子 Agent 的 description（`flow:<运行>/<站>/<任务>`），`flow_submit` 时派工器问宿主
主会话里叫这个名字的子 Agent（`SessionHost::agent_tasks`）：

- 有一个答完了的：收下，任务 id、侧链会话、用量记进 `delivery.agent`。
- 还在跑：不收，叫它等答完（工具结果偶尔比任务结束的帧先到，派工器先等几秒）。
- 一个都没有、都失败了：不收，告诉它用这个 description 调 `Agent`。
- 宿主分不出（缺省）：照旧收下，`delivery.agent` 为空。

### 6.4 交付的记录

每个任务交付时记 `delivery`：谁交的（`Actor`）、什么时候、以什么身份（会话、子 Agent 任务、程序步骤、外部认领、子运行），
与输出放在一起。审计时一个任务"谁做的、凭什么"都在这一条里。

## 7. 落点

| 内容 | 位置 |
|---|---|
| 门禁定义、脚本件、校验 | `crates/flow/src/kernel/parts.rs`、`kernel/validate.rs` |
| 门禁结果、`CheckRec` 的证据、重跑门禁 | `kernel/host.rs`（`GateResult`）、`kernel/run.rs`、`kernel/engine.rs`（`gate_result`、`rerun_gates`） |
| 执行 `check` / `command` / `script` | `runner/gate.rs`；`runner/files.rs`（限在工作目录里读、记证据）、`runner/checks.rs`、`runner/script.rs`（rquickjs，与 Agix 同一版本） |
| 打回时告诉会话为什么 | `kernel/run.rs`（`StationRec::reworked`）、`kernel/protocol.rs` |
| 确认、轮询审批 | `kernel/def.rs`（`ExitCond::Confirm`）、`kernel/txn.rs`、`runner/approvals.rs`（新） |
| 受管流程 | `packs.rs`（`ProjectPacks`、`DirPacks`）、`kernel/run.rs`（`governed`） |
| 子流程结果、`children`、交付 | `kernel/def.rs`、`kernel/txn.rs`、`runner/tools.rs`（`flow_submit`） |
| Conix 的授权（人、角色、受管） | `crates/host/src/flows/authorize.rs`（新） |
| Conix 的审批（MCP） | `crates/host/src/flows/approvals.rs`（新） |
| `[x-conix] governed`、`command_gates` | `crates/host/src/plugins/mod.rs`、`crates/host/src/config` |
| 人做的决定（`/confirm` 等） | `crates/model/src/flows.rs`（`person_op`）、`port::RunOp::Confirm{reject, reason}` |
| 写流程的会话：试门禁、指南分章 | `crates/flow/src/author.rs`（`flow_gate_try`、`flow_guide {topic}`）、`runner/gate.rs`（`try_gate`）、`docs/guide/` |
| 演示项目 | `examples/demo/`（流程、门禁、脚本、作业）；`crates/flow/tests/demo.rs` 不调模型地查它，`crates/conix/tests/live_demo.rs` 用真实模型跑它 |

## 8. 往回迁

`crates/flow` 不跟 Conix 的格式化（目录里的 `rustfmt.toml` 关掉了），保持与 Harnix 那份逐行可比；新代码按原有风格写。
每一项落地后在 `3rds/INDEX.md` 的分叉记录里记一行，整理成给 Harnix 的 issue 时按这张表来。
