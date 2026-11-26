# 流程的治理：门禁、确认、交付

本文是流程库（`crates/flow`）在"怎样才算过站"这件事上的设计：三类门禁、统一的门禁结果、确认、强制放行、
受管流程、子流程与子 Agent 的交付。流程的其余部分（站、执行者、派工、运行记录）沿用 Harnix flow，见
`architecture.md` §8.6。

| 依赖 | 内容 |
|---|---|
| Harnix-v0.4.7 | 流程库的分叉基线：`crates/flow` 从这个发布标签的 `crates/flow` 分出来，之后在 Conix 里改，稳定后整理成 issue 提给 Harnix |
| harnixai/Agix#111 | 宿主不带会话直接调 MCP 工具（`mcp.callTool`）：确认走外部审批要用它 |

## 1. 为什么要改，以及怎么分层

企业的治理要的是"每一站凭证据过，证据事后查得到"。分叉前的流程库做不到：

- **只有插件代码能检查文件。** 门禁三种：`tool`（调宿主的只读工具再断言结果）、`command`（跑一条命令看退出码）、
  `host`（宿主自己的检查）。Conix 不给 `tool` 提供工具、不开 `command`，只剩 `host`，也就是写插件 JS。
- **结论里没有证据。** 门禁结果只有 `{pass, detail}`：看了哪些文件、文件当时是什么内容，运行记录里查不到；门禁没跑起来
  （超时、脚本出错）和证据不够也分不出来。
- **谁都能确认、能强制放行。** Conix 用的是 `AllowAll`：主会话里的模型调 `flow_confirm` / `flow_force`，只靠 Agix 的权限
  询问拦着，会话切到 `bypassPermissions` 就拦不住。项目里存一份同名流程就盖掉了插件下发的那份，门禁随之可以删掉。
- **交付没有形状。** 子流程交给父任务的是子运行每一站输出拼成的对象；`agent` 站的交付由主 Agent 转述，子 Agent 实际
  有没有跑完，流程不核对。

### 1.1 分层与扩展点

流程的东西分三层，每一层的写法与读者不同：

| 层 | 写在哪 | 是什么 | 谁读 |
|---|---|---|---|
| 契约 | YAML（`bundle.yaml`、流程定义） | 有哪些流程与站、怎么出站、谁来批、能打回到哪、能写哪里、怎么选路 | 人、流程标签、校验与试运行、引擎、写流程的模型 |
| 机制 | Rust（`crates/flow` 与 Conix 宿主） | 持久的状态机、派工、选路与换路、授权、写入执法、确认与审批、证据、原语（文件、git、运行查询、锁） | 无：它执行契约 |
| 策略 | JS（包里的 QuickJS） | 门禁怎么判、命令怎么改数据、不用模型的步骤、工作项从哪来 | 无：它被契约引用 |

契约是声明的，所以能被工具检查、被界面画出来、被模型读懂；策略藏在脚本里，只能被执行。因此**能写进契约的不写进脚本**：
谁能批、能退到哪、能写哪里这类治理上的东西，必须在 YAML 里看得见。

新能力放哪，按下面的判据：

| 放哪 | 判据 |
|---|---|
| Rust | 要持久（跨重启）、要可信（流程作者与模型绕不过）、要并发与锁、要执法、跨流程通用 |
| 包 JS | 只依赖项目文件与运行数据、确定、随运行钉住、只为这一个应用 |
| Agix 插件 | 要 Agix 的工具管线（`tools.Bash`、MCP、权限规则、hook）、要独立于项目分发、要在流程之外给所有会话用 |

### 1.2 两个 QuickJS：分工，不合并

| | 流程库自己的（包 JS） | Agix 的（插件命令、插件函数） |
|---|---|---|
| 定位 | 流程的零件：门禁脚本、命令、命令步骤、选值脚本 | 通用扩展；要工具与 MCP 的门禁与步骤 |
| 生命周期 | 随运行钉住（`Pinned`） | 装了就生效 |
| 能力 | `flow.*`（§7.7），没有 shell、没有网络 | `tools.*`、`agix.*`、`conix.*` |
| 授权 | 流程库按范围限写，不问人 | Agix 的权限与披露 |
| 依赖 | 不依赖 Agix（往回提给 Harnix 也成立） | Agix |

不合并的原因：包 JS 要随运行钉住、要确定、要在没有 Agix 时也成立；插件函数跑在工具管线里，结论会随会话设置变。

**对齐写法**：两边的入口都是 `export default function (input, flow)`；`flow.*` 的纯计算函数与 `agix.*` 同名同义
（`yaml`、`glob`、`schema`、`sha256`、`path`）。同一段逻辑在包与插件之间搬，只换宿主对象。包 JS 要跑 shell 或调 MCP
时不开口子：写成 `command` 门禁，或交给插件函数（`[x-conix.gates]` / `steps`）。

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
  - `flow.schema.validate(value, schema)`（回 `{ok, errors}`；§7.7 里叫 `flow.validate`，两个名字同义）；
  - `flow.log(...)`（进 `gate-<名>.log`）。

  路径规则同上。没有写、没有命令、没有网络。包内的 `import`、`flow.git`、`flow.runs` 见 §7.6–§7.7。
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
打回（`/rework [运行] <站> [理由]`）也能这样打，但它不是只有人能做：模型照样有 `flow_rework`。给人一条命令是因为检查留下
问题、要不要打回往往是人看了才定的，不该还要绕道对话叫模型去打；打回的目标站推断不出来，所以站一定要写，理由给重做的站看。

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

### 4.1 多人确认与绑定修订

```yaml
- kind: confirm
  who: reviewer
  min: 2                 # 至少几个人批准；缺省 1
  distinct: true         # 一人一票（缺省）
  reject: any            # any（缺省：一人驳回就不过）| majority
  not: [author]          # 可选：作者不能投
  separate: true         # 可选：true 就是 not: [author]
  binds: [deliverables]  # 批的是什么：deliverables（缺省）| 路径 glob 列表
```

- 每个 `/confirm` 记一票（`ConfirmRec` 加上票序），够 `min` 才过；`distinct` 下同一个人投两次只算一票。
- **职责分离不强制**：缺省作者也能投。要分开就写 `not: [author]`；作者指这次运行里交付过的会话、子 Agent 的发起人。为什么不缺省强制：
  一个人用 Conix 时作者与批准人就是同一人，强制了流程就走不通。
- **`separate`** 是 `not: [author]` 的开关写法：治理文件里放一个 `true` / `false` 就能管，不用让人往文件里写列表。
- **从包的设置取值**：`min`、`reject`、`separate` 和 `not` 的项可以写成 `${settings.<路径>}`（§7.1），按运行启动时钉住的设置取。
  设置里没有这个路径、或值的类型不对（`separate: "yes"`、`min: 0`），**启动就拒**，写明哪一站哪个字段；不回落到缺省：
  治理文件写错了却悄悄按"作者也能批"跑，比起不来更糟。别处的字段这次不开 `${settings.*}`。

```yaml
- {kind: confirm, who: reviewer, separate: "${settings.review.separate}"}
```
- **`reject: any`** 缺省：一人驳回，这条出口就不过；`majority` 是驳回票过半才不过。
- **绑定**：通过时记下 `binds` 每个文件的摘要，`deliverables` 是本站及之前各站声明的制品。之后每一站出站、运行结束前，引擎自动核对一遍
  （隐含的出口，同 `sealed` 的做法）；被改过就**不过**，findings 写"检查站批过的 design.md 改了"，出路是打回那一站重批。
- 要多个模型独立检查：站写 `tasks:`，每个任务一个执行者或模型，台账汇总，出口照旧；这与多人确认是两件事。

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

## 7. 流程包、制品与包的命令

流程的组织与项目里的目录见 [layout.md](layout.md)：一个**流程包**自洽地放一个或几个流程（同一件事的几条路径）和它们用的全部件，
件不跨包共享；运行在**制品目录**（缺省 `conix/`）里有自己的目录，结束后归档进包的数据；站用 `deliverables` **声明制品**
（协议列给会话、骨架进站时放好、隐含一道"写好了"的检查）；包可以带**自己的命令**（QuickJS），站里的会话经 `flow_command`
调，结构化的数据（规格增量、台账、工作包）由命令写。skill 只写这一站怎么干活，用户补充写 `local.md`。

Conix 内置一个按这些写的开发流程包 `xforge`（能力与 xforge 插件对齐：规格驱动、按工作包实施、独立检查、可以打回、归档时
并进规格），源头是演示项目 `examples/xforge/.conix/flows/xforge/`。它的流程就是流程本身，没有 `xflow` 这一层：运行的状态
在流程里，结构化的数据由 `xspec`、`xledger`、`xpkg` 三个命令维护。

### 7.1 包：`bundle.yaml`

```yaml
name: xforge
title: XForge 开发流程
version: 0.2.0
flows: [quick, solid, major]          # 选路的顺序：短的在前（§7.2）
start:
  route: [quick, solid, major]        # 缺省就是 flows 的顺序
  triage: triage                       # skill：选路要判断时，主会话照它判
pace: auto                             # auto | step（§7.4），启动时可改
commands:
  xspec:   {read_only: [list, get, delta, check, pending, help]}
  xledger: {read_only: [list, help]}
  xpkg:    {read_only: [list, help]}
settings:                              # 项目的治理文件，启动时读、钉进运行
  file: .conix/governance/xforge.yaml
  defaults: {governance: {spec: true, interface: false}, review: {separate: false}}
```

| 字段 | 意思 |
|---|---|
| `flows` | 包里的流程，也是缺省的选路顺序 |
| `start.route` | 选路的顺序；不写就是 `flows`。选用的 `script` 形式让脚本回一个流程名 |
| `start.triage` | 分诊 skill（§7.2） |
| `pace` | `auto`（缺省）或 `step`（§7.4） |
| `commands` | 包的命令（§7.6）。写成列表（`[xspec, xledger]`）时每个命令的全部子命令都算会写；写成映射时 `read_only` 列出只读的子命令 |
| `settings` | 项目里的设置文件（`file`，相对项目根、不出项目）与缺省（`defaults`，映射）。见下 |

- **`read_only`** 按命令名后的第一个词匹配，照 Agix `[[command]]` 的约定。声明它是为了让运行之外也能放心地查
  （`/flow:xforge:xspec list`）：会写的命令运行之外只有人能调（§7.6）。
- **包里的名字一个空间**：流程名、命令名、包里任一流程的站 id 不能撞。站 id 在几条流程之间可以重复，它们是同一个站的不同长度。
  保留名：`help`、`status`。读包时校验。为什么：地址 `/<包>:<名字>` 只有一个名字位（interaction §4.5），三种名字不能靠位置区分，
  只能靠不重名。
- 包目录：`flows/*.yaml`（或平铺的 `<流程>.yaml`）、`skills/`、`gates/`、`scripts/`、`commands/`、`lib/`、`templates/`、`policies/`。
- 不在包里的流程，包名就是流程名；选路、换路、步进照样能用。
- **`settings`：按运行钉住。** 运行启动时读项目根下的 `file`（YAML），深合并到 `defaults` 之上（映射逐键合并，别的值整个替换），
  结果钉进运行（`Pinned::settings`，与脚本、库一起）。之后改这个文件**不影响已经在跑的运行**，只影响之后启动的：设置是规则
  （谁能批、欠哪几条基线），规则半路变了，已经做完的站就是按别的规则做的、却按新规则判，证据对不上。要新规则就取消重开。
  - 没有文件就是缺省；没有项目的运行也是缺省。文件不是 YAML、或顶层不是映射，**拒绝启动**并写明文件：读不成的规则不能拿缺省顶。
  - 读的是项目根的文件，worktree 运行也是：规则是项目此刻说的，不是 worktree 检出那一刻提交里的。
  - 同一个包的子运行沿用父运行钉住的设置；换路（§7.3）保留运行原来的。
  - 用在确认出口的 `${settings.<路径>}`（§4.1）与包 JS 的 `flow.settings`（§7.7）。

### 7.2 启动与选路

| 怎么起 | 结果 |
|---|---|
| 点名流程：`/flow:xforge:solid <主题> [--risk medium]`、`flow_start {flow: "xforge/solid"}` | 就是它；输入不合它的 schema 就拒。schema 就是这条流程的准入条件：solid 的 `risk` 只收 low、medium |
| 只给包：`/flow:xforge <主题> --risk low`、`flow_start {flow: "xforge", inputs}` | 按 `start.route` 的顺序取**第一个接受这份输入的流程**。确定，不用模型 |
| 只给包、输入定不了路（没给 `risk`） | **分诊**：Conix 往主会话发一轮（`start.triage` skill 加主题），模型读代码、判复杂度，再 `flow_start {flow: "xforge", inputs: {risk}, reason}`；选的理由记进运行 |

分诊放在主会话里：它要读项目代码，而主会话本来就是用户在对话的那个。模型在对话里自己决定开流程时，`flow_start` 的说明里列着包的
流程与各自接受的输入，可以点名，也可以只给包。

### 7.3 换路

做到一半发现选短了：

- `flow_switch {to, reason}`（模型）或流程上下文里的 `/xforge:solid <理由>`（人）。
- **只在第一站还没过时允许**：同一个运行、同一个运行目录，换钉新流程；当前站 id 必须在新流程里，已写的制品与输出保留；记
  `switched {from, to, by, reason}`。
- 过了第一站要换路，就取消再开，理由记进取消。为什么不允许中途换：后面的站已经按旧流程的出口过了，换钉会让已过的证据对不上新流程的定义。

### 7.4 步进

`pace: step`（`bundle.yaml` 定缺省，启动时 `--step` 改）：每一站进站都 hold，人用 `/xforge:<站>` 一站站推。放行只认人；
步进里模型照旧能打回。`/xforge:<站>` 按运行此刻与这一站的关系做事，见 interaction §4.5 的站操作表。

### 7.5 回退点与模型建议

回退点由流程定义，不由模型或人临时指定：

| 写在哪 | 意思 |
|---|---|
| 站的 `rework_to` | 这一站能退到的点 |
| `on_unmet.rework_to` | 出口没达标时自动打回的缺省点 |
| 门禁出口的 `rework_to` | `{kind: gate, gate: spec-delta, rework_to: propose}`：这道门不过时自动打回到这里，而不是缺省点 |

- 门禁级的回退点必须是这一站的 `rework_to`、`on_unmet.rework_to` 或这一站自己，读定义时校验。
- 几道门都不过时，取最靠前的回退点；打回次数仍算在 `on_unmet.max` 里，所以换了目标站也不会无限循环。
- **模型可以建议回退**：站里的会话 `flow_propose {kind: rework, to, reason}`；`to` 必须是这一站定义的回退点之一，否则报错并列出能退到哪。
  建议**记进运行记录**（`RunRecord.proposals`：id、站、任务、kind、to、reason、by、at、状态 `open` / `accepted` / `dismissed`），并发事件。
- 人用 `/xforge:<to> [理由]` 接受：就是打回，同时把建议标成 `accepted`。站往前走或被打回时没接受的建议标成 `dismissed`。
  Conix 在流程上下文的对话里与流程标签上把等着的建议列出来。
- 为什么站里的模型只能建议：它在做这一站，判断不好这一站的结果，由人决定退不退；主会话的模型仍可以直接 `flow_rework`，受同样的回退点约束。

### 7.6 包 JS：件与签名

| 件 | 位置 | 由谁调 | 能写 | 返回 |
|---|---|---|---|---|
| 门禁脚本 | `scripts/<名>.js` | 门禁 `kind: script` | 否 | 门禁结果（§3） |
| 选值脚本 | `scripts/<名>.js` | `foreach: {script: …}`、`start.route` 的 `script` | 否 | 一个值（工作项列表、流程名） |
| 命令 | `commands/<名>.js` | `flow_command`、`/…:<命令>`、站 `executor: {kind: command}` | 运行目录（`_run/` 除外）、`${data_dir}` | 文本或值 |
| 库 | `lib/<名>.js` | 上面三种 `import` | 无 | 无 |

- **入口**：`export default function (input, flow)`。
- **`import`**：只认包内的相对路径（`import { readSpecs } from "../lib/specs.js"`），不出包、没有 npm，随运行一起钉住。
  门禁与命令共用一份判法：spec-delta 门禁与 `xspec check` 调同一个函数，不会各判各的。
- **命令步骤**：站写 `executor: {kind: command, command: xspec, args: [archive]}`，不用模型，返回值就是这一站的输出（不是对象时包成
  `{result}`），抛错就是任务失败。不另立"步骤"这种件：一个命令同时给人、模型、站用。
- **命令的 `input`**：`{args, by: {kind: person|model|station, id}, run?, output_dir?, data_dir, inputs?, outputs?, item?}`。
  `by` 让命令区分"人答的"和"模型写的"（澄清问题要人答：命令要求 `by.kind == person`）。运行之外 `run`、`output_dir` 是空的，
  只有 `${data_dir}`，用到运行目录的子命令报"要在运行里"。
- **谁能调会写的命令**：运行里的模型可以；运行之外只有人（`/flow:<包>:<命令>`）。`read_only` 里的子命令运行之外模型也能调。

### 7.7 `flow.*`

| 组 | 函数 | 谁能用 |
|---|---|---|
| 读 | `read.text/json/yaml/jsonl/frontmatter`、`glob`、`stat` | 全部 |
| 写 | `write.text/yaml/json`、`append`、`remove`、`print` | 命令 |
| 算 | `validate(value, schema)`、`yaml` / `json` 编解码、`sha256`、`path.*` | 全部 |
| git | `git.base()`（运行的基线提交）、`git.changed({since?})`（基线以来的改动加没提交的，排除制品目录与 `.conix/`）、`git.head()` | 全部，只读 |
| 运行 | `runs.list({bundle?, active?})`、`runs.get(id)`：状态、输入、输出、运行目录 | 全部，只读 |
| 设置 | `settings`：包的设置（§7.1）。运行里是启动时钉住的；运行之外的命令、门禁试跑是文件此刻合并缺省的结果；包没声明是 `{}`；文件读不成时读它就抛错 | 全部，只读 |
| 日志 | `log` | 全部 |

读写都限在工作目录与各件允许的范围里，出界报错。git 由 Rust 跑，不给 JS 起进程：JS 里有了进程就绕过了"只读、确定"。
§2.3 的门禁脚本只用只读的那几组；`import`、`git`、`runs` 对它同样可用。

### 7.8 工作项

- **从脚本来**：`foreach: {script: pending-packages}`，脚本读工作包、只回没做完的；旧的引用写法照旧。
- **先后**：工作项可以带 `after: [<id>]`，派工器按它排，`max_parallel` 里也守着；有环就拒绝，任务失败。
- **任务级出口**：`task_exit: [{kind: gate, gate: package-verify}]`，在任务 `flow_submit` 时跑，引用里有 `${item}`；不过就把 findings
  交回这个任务去改（同输出不合 schema），不算站没达标；重试次数受 `retry` 管。"每包一道验证门、交付对 git 账"写在这里。

### 7.9 写入范围与基线

- 运行开始记 `base_commit`（在 worktree 里跑的记 worktree 的 HEAD），`${run.base}` 可引用，`flow.git.*` 用它。
- 站与任务可以写 `writes: [glob]`（白名单，`${item.paths}` 展开成列表）：项目内的写入不在白名单里就拒，理由给模型看。
  运行目录永远可写（`_run/` 除外）；策略里的 `deny` 照旧优先（`by-commands` 仍挡结构化文件）。
- **项目外照 `guard`**：`writes` 只收紧项目内的写入，不放宽，也不管项目外。
- 不写 `writes` 的站与以前一样。

### 7.10 跨运行与其他

| 机制 | 内容 | 状态 |
|---|---|---|
| 数据目录的锁 | 同一个包的 `${data_dir}` 写入跨运行串行（命令、归档）；`flow.runs` 让命令算"别的在途运行占了哪些编号" | 做 |
| 自描述与测试 | 每个命令的 `help` 来自文件头的用法；`flow test`：`tests/*.yaml` 给一份样例目录与期望（门禁结论、命令输出），不用模型跑完 | 做 |
| 证据链 | 运行历史与门禁证据成哈希链，`inspect` 能验 | 后做 |
| 子运行看父运行 | 子运行能读 `${parent.output_dir}`（只读）：双路开发时两个子运行（各在 worktree）共用父运行的规格侧 | 需要双路时再做 |

### 7.11 兼容

- 流程定义只加字段，旧定义照读；`bundle.yaml` 的 `commands` 列表写法照读。
- 插件带的流程包、受管流程不变；插件包里的流程包同样有 `/flow:<包>` 地址。
- 每一项在 `3rds/INDEX.md` 的分叉记录里记一行；只在 Conix 里有的（地址、分诊、授权）标"in Conix"。

### 7.12 导出与导入

一个流程包可以导出成一个文件、装进另一个项目。包文件是 zip，`<包>-<版本>.zip`（版本取 `bundle.yaml`，没写是 `0.0.0`），
放的就是包目录原样，外加根上一份导出时生成的 `MANIFEST.yaml`：

```yaml
format: 1
bundle: {name, version, title, description}
conix: {version: "0.1.10", build: "0.1.10-2026112502", exported_at: "2026-11-25T08:00:00Z"}   # 导出它的 Conix
requires: {conix: ">=0.1.10", flow_definition_format: 1, flow_api: 1}
files: [{path, sha256, size}, …]
```

- **不带身份**：没有用户名、主机名、路径。`requires.conix` 取 `bundle.yaml` 里可选的 `requires: {conix: ">=x.y.z"}`
  （只认这一种写法，写别的拒绝），没写就是导出它的那个版本。
- **不进包的**：`skills/*/local.md`（用户自己的补充）、点开头的文件、`__pycache__`、符号链接（遇到就报错）。
  项目的、插件的、内置的包都能导出，内置的 `xforge` 也是。
- **导入先核对再动手**：包在暂存目录（`.conix/state/conix/` 下）里解开，逐项核对后才装。拒绝：`format` 不认识；
  文件与 MANIFEST 对不上（缺的、多的、SHA-256 或大小不符）；绝对路径、`..`、反斜杠、隐藏文件、`local.md`、符号链接；
  超过 512 个文件、总共 10 MiB、单个 2 MiB。不兼容也拒绝：运行的 Conix 低于 `requires.conix`，或定义格式、流程库接口比这个构建新
  （`DEFINITION_FORMAT`、`FLOW_API_VERSION`）。最后用流程库校验整个包（每个流程、每个件、`commands` 列的文件），有问题就拒绝。
- **装**：`<项目>/.conix/flows/<包>/`。已有同名的拒绝，`--force` 才覆盖：旧的先整个移到
  `.conix/state/conix/flow-backups/<包>-<时刻>/`，旧的 `skills/*/local.md` 带到新的里。报告包名、版本、导出它的 Conix 版本、
  流程与命令；遮住了同名的插件或内置的包时提醒；包里有 `command` 门禁时提醒它们只在受信任的项目里跑。
- **`export`、`import` 是包的保留名**（`bundle::RESERVED_BUNDLES`）：`/flow:export`、`/flow:import` 因此不会与哪个包的地址混，
  地址路由在查包之前先接下它们。
- **入口只给人，不给模型**：命令行 `conix flow export <包> [-o 文件] [--project 目录]`、`conix flow import <文件> [--force]
  [--project 目录]`、`conix flow inspect <文件>`（打印 MANIFEST 与核对结果）不需要实例，项目的与内置的包直接读（插件的要在实例里导）；
  输入框里 `/flow:export <包> [文件]`、`/flow:import <文件> [--force]` 发 `ExportFlow` / `ImportFlow` 给实例（文件相对项目根，
  导出不写文件就放项目根），导入后实例重列流程，流程标签随之更新。打包与核对在 `crates/host/src/flows/package.rs`，命令行与实例共用。

## 8. 落点

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
| 流程包、制品目录、归档、制品声明、包的命令 | `crates/flow/src/bundle.rs`、`packs.rs`、`kernel/layout.rs`（`with_artifacts_dir`）、`runner/archive.rs`、`runner/project.rs`（骨架）、`runner/script.rs`（`Writer`、`command`）、`runner/tools.rs`（`flow_command`） |
| 包的字段（`flows`、`start`、`pace`、`commands.read_only`）、名字空间与保留名校验 | `crates/flow/src/bundle.rs`、`kernel/validate.rs` |
| 选路、分诊、换路、步进（hold）、站操作 | `crates/flow/src/kernel/route.rs`（新）、`kernel/engine.rs`、`kernel/txn.rs`；分诊那一轮由 `crates/host/src/flows/`（新：`triage.rs`）发给主会话 |
| 回退点（门禁级 `rework_to`）、模型的回退建议（`RunRecord.proposals`） | `kernel/def.rs`、`kernel/validate.rs`、`kernel/run.rs`、`kernel/txn.rs`、`runner/tools.rs`（`flow_propose`） |
| 多人确认、绑定与过期核对 | `kernel/def.rs`（`ExitCond::Confirm`）、`kernel/run.rs`（`ConfirmRec`）、`kernel/txn.rs` |
| 包 JS：`import`、`lib/`、`flow.*`、命令步骤、命令的 `by` | `runner/script.rs`、`runner/tools.rs`（`flow_command`）、`runner/dispatch.rs` |
| `flow.git`、基线、`writes` 执法 | `runner/git.rs`、`runner/script.rs`、`kernel/run.rs`（`base_commit`）、`crates/host/src/flows/policy.rs`（写入执法） |
| 工作项（`foreach` 脚本、`after`、`task_exit`） | `kernel/work.rs`、`kernel/def.rs`、`kernel/txn.rs`、`runner/dispatch.rs` |
| 流程地址与本地路由、补全 | `crates/model/src/flows.rs`、`crates/model/src/composer.rs`（interaction §4.5） |
| 流程包的导出与导入（zip、`MANIFEST.yaml`、核对、`/flow:export`、`conix flow …`） | `crates/host/src/flows/package.rs`、`crates/conix/src/flow_cli.rs`、`crates/flow/src/bundle.rs`（`requires`、`RESERVED_BUNDLES`）、`crates/model/src/flow_addr.rs`（路由）、`port::Command::{ExportFlow, ImportFlow}` |
| 内置的 xforge | `examples/xforge/`（项目与流程包）、`crates/host/build.rs`、`crates/host/src/flows/builtin.rs`；`crates/flow/tests/xforge.rs`、`crates/conix/tests/live_xforge.rs` |

## 9. 往回迁

`crates/flow` 不跟 Conix 的格式化（目录里的 `rustfmt.toml` 关掉了），保持与 Harnix 那份逐行可比；新代码按原有风格写。
每一项落地后在 `3rds/INDEX.md` 的分叉记录里记一行，整理成给 Harnix 的 issue 时按这张表来。
