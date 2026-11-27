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
| 包 JS | 只依赖项目文件与运行数据、随运行钉住、只为这一个应用；要系统命令经 `flow.exec`（§7.14） |
| Agix 插件 | 要 Agix 的工具管线（`tools.Bash`、MCP、权限规则、hook）、要独立于项目分发、要在流程之外给所有会话用 |

### 1.2 两个 QuickJS：分工，不合并

| | 流程库自己的（包 JS） | Agix 的（插件命令、插件函数） |
|---|---|---|
| 定位 | 流程的零件：门禁脚本、命令、命令步骤、选值脚本 | 通用扩展；要工具与 MCP 的门禁与步骤 |
| 生命周期 | 随运行钉住（`Pinned`） | 装了就生效 |
| 能力 | `flow.*`（§7.7）；进程只经 `flow.exec`（argv、档位由调用方定，§7.14），没有 shell | `tools.*`、`agix.*`、`conix.*` |
| 授权 | 流程库按范围限写，不问人 | Agix 的权限与披露 |
| 依赖 | 不依赖 Agix（往回提给 Harnix 也成立） | Agix |

不合并的原因：包 JS 要随运行钉住、要在没有 Agix 时也成立；插件函数跑在工具管线里，结论会随会话设置变。包 JS 不碰进程
时是确定的；用了 `flow.exec` 就不承诺确定（外部程序的输出流程库管不了），只读靠的是档位（§7.14），不再靠"JS 起不了进程"。

**对齐写法**：两边的入口都是 `export default function (input, flow)`；`flow.*` 的纯计算函数与 `agix.*` 同名同义
（`yaml`、`glob`、`schema`、`sha256`、`path`）。同一段逻辑在包与插件之间搬，只换宿主对象。包 JS 要跑系统命令（系统的
`git`）用 `flow.exec`：按 argv 起、经宿主的执行服务、档位由调用方定（§7.14）；要 shell 写成 `command` 门禁；要调 MCP
交给插件函数（`[x-conix.gates]` / `steps`）。

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
  这个开关管的是项目与包定义的**全部**执行：命令门禁、`plan` 的 run、包 JS 的 `flow.exec`（§7.14）。
- 进程经宿主的执行服务起（`sh -c <命令>`，档位 `read`），每起一个在运行历史里记一条 `exec`（§7.14）。

#### `plan`：脚本挑，Rust 跑

```yaml
name: package-tests
kind: command
plan: package-tests        # 件 scripts/package-tests.js，代替 command（两者只能写一个）
params: {plan: "${output_dir}/work-packages.yaml", item: "${item}"}
env: [CI_*]
timeout_seconds: 900       # 所有 run 加起来的时限
```

```js
// scripts/package-tests.js：input 与 §2.3 的一样（任务级出口里有 item），flow 只读
export default function (input, flow) {
  return { summary: "p1 的验证", runs: [{ name: "unit", argv: ["python3", "-m", "unittest", "discover", "-s", "tests"], cwd: "." }] };
}
```

- **为什么**：要跑哪些命令常常取决于模型写的数据（这个工作包要跑哪几项验证）。拼成一行交给 `sh -c`，模型写进数据里的
  `; rm -rf ~` 就成了程序。所以分两层：**脚本是策略**，与门禁脚本一样只读、确定，只从人声明过（进 git）的东西里挑，
  挑不出来就自己回门禁结果；**Rust 是机制**，按 `argv` 直接起进程，程序与参数原样传过去，中间没有 shell，`;`、`$(…)`、
  `|` 在参数里只是字符。流程库从不把字符串交给 shell。
- **脚本回什么**：§3 的门禁结果就照它判，什么都不跑（证据仍是它读过的文件）；`{runs: [{name, argv, cwd?, timeout_seconds?}], summary?}`
  就依次跑。别的形状、抛错、超时（脚本自己的时限是 30 秒，与 run 的无关）算 `error`。
- **先全部核对，一个不合就一个都不跑**（`error`）：`argv` 非空、程序非空、没有 NUL；`cwd` 相对运行的工作目录，绝对路径、
  `..`、经符号链接出界、不存在的都拒绝；`argv[0]` 是带 `/` 的相对路径时按 `cwd` 找；至多 32 个 run。
- **跑**：与命令门禁同一套——经宿主的执行服务、档位 `read`、环境变量白名单与宿主的拒绝表、`FLOW_*` 变量、自己的进程组、
  只在受信的项目里、`flows.command_gates: off` 一样关掉，每个 run 记一条 `exec`（§7.14）。门禁的 `timeout_seconds` 是所有 run 加起来的时限，run 自己的 `timeout_seconds` 只能更短。
- **结论**：每个 run 都退出 0 才 `pass`。退出非 0 的每个一条 finding（`rule` 是 run 的名字，消息带输出的尾巴），后面的照跑，
  结论 `fail`；起不来、超时、被信号杀掉算 `error`（同命令门禁），后面的不跑。`runs` 为空是 `pass`（`summary` 用脚本给的，
  没给是 "nothing to run"）。
- `gate-<名>.log` 里是脚本的 `flow.log` 与每个 run 的输出。门禁上不写 `command`、`cwd`、`result`；`params` 照样给脚本。

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

  路径规则同上。没有写；进程只经 `flow.exec`，档位 `read`（§7.14）。包内的 `import`、`flow.git`、`flow.runs`、
  `flow.exec` 见 §7.6–§7.7。
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
  "metrics": {"open": 0},
  "facts": {"breaking": false, "modules": ["textkit"]}
}
```

| `verdict` | 意思 | 站怎么办 |
|---|---|---|
| `pass` | 证据够 | 这一条出口过 |
| `fail` | 证据不够 | 站未达标；有 `on_unmet` 就自动打回，`findings` 随打回交给那一站的会话 |
| `error` | 门禁本身没跑成：超时、出错、路径出界、返回值不合规 | 站未达标，**不自动打回**（模型修不了门禁）；人看过后"重试"重新跑门禁（`Engine::rerun_gates`；TUI 的重试在没有失败任务时就是它） |

- `severity` 是 `error`（缺省）或 `warn`。门禁可以写 `fail_on: warn`：有 warn 也算不过；缺省只看 `verdict`。
- 证据与结论进运行记录（这一条出口的 `CheckRec`）和事件日志（`gate_result`），事后能复核。
- **`facts`**：门禁的代码顺手算出的事实，给出口的 `when` 读（§4.2）。每个值是字符串、数、布尔，或它们的列表；`null`、对象不收，
  形状不对就是 `error`：`when` 要分得清"没有这个事实"和"它说不"，能是 `null` 的事实把两者搅在一起。与 `metrics` 不同，`metrics`
  给人看，`facts` 会被判站读，所以要说清"这次有没有"（一个 `false`），而不是没有就不写。和结论一起记进 `CheckRec`、事件日志、
  `record.json`。`check` 门禁（Rust 内置）没有事实；`script` 门禁的脚本、`command` 门禁 `result: json` 的输出、宿主的 `function` 都能给。
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

### 4.2 只在某些情形下要的出口：`when`

有的人只在某些情形下才需要：xforge 里破坏性的接口改动要主人批，不破坏就不用问他。任何出口都可以带 `when`：

```yaml
- {kind: gate, gate: interface-compat}
- kind: confirm
  who: owner
  label: 批准破坏性改动
  when: {gate: interface-compat, fact: breaking}     # 门禁结果的 facts.breaking 为 true 才要
- {kind: confirm, who: security, when: {gate: impact-scan, fact: areas, in: [auth, payment]}}
- {kind: confirm, who: dba, when: {step: migrate-plan, path: summary.destructive}}   # 命令步骤的输出
```

| 写法 | 读什么 | 只能指向 |
|---|---|---|
| `{gate: <门禁>, fact: <键>}` | 那道门禁这一轮（之前的站：最近一轮）结果的 `facts.<键>`（§3） | 这一站或它之前（`needs` 上游）一站的门禁出口，那条出口自己不带 `when`；`check`、`tool` 门禁不给事实，不收 |
| `{step: <站>, path: <点分路径>}` | 那一站任务的输出里的这个路径 | 这一站或它之前一站，且那一站是**一个命令步骤**（`executor: {kind: command}`，没有 `foreach`、`tasks`） |

- 不写比较就要值是 `true`；`equals: <标量>`、`in: [<标量>…]` 比较，值是列表时含有一个对得上的就算。读定义时校验：两种来源只能写一种，
  `gate` 要带 `fact`、`step` 要带 `path`，`equals` 与 `in` 只能写一个；指向不在范围里的门禁或站，起不来。`task_exit` 不收 `when`。
- **来源只认确定的代码，不认模型交的东西。** 门禁的事实由门禁的代码从文件算出，命令步骤的输出由包的命令在没有模型的情况下算出；
  会话交的站输出是模型写的。要是 `when` 能读站的输出，模型写一个 `breaking: false` 就把该批的人绕过去了——"要人的时候就一定要人"
  在这里就破了。文件可能是模型写的，但从文件判出"是不是破坏性改动"的是门禁的代码，判法人能审、改了要过包的评审。
- **说不清就要（fail closed）。** 只有读到一个明确的"不"，这条出口才不要：门禁还没结果（之前的站的门禁没跑、被跳过）、门禁 `error`、
  结果里没有这个事实、值的类型不对（要布尔给了字符串，比较的是数给了字符串）、路径上什么都没有，一律当**要**，站上写明为什么
  （`CheckRec.when`，`explain` 带"required: …"，人看到的就是这句）。出了故障却悄悄少问一个人，比多问一次糟得多。
  同一站的门禁还在跑时，出口跟着它等，什么也不先做（不建外部审批单、不起计时），结果来了再判。
- **不要的也留记录。** 不要的出口在站上记一条 `not_required`（第几条、标签、哪一轮、`when`、读到的值），检查记成 `pass`、
  `detail` 写"not required: gate interface-compat fact breaking is false, not is true"，投票表里不列它。审计看得到"这一站本该有谁批、
  为什么这次没批"，不是这条出口不见了。
- **每次判站都重算。** 打回、重跑门禁之后事实变了，要不要跟着变：上一轮不要的确认，这一轮可能要，反过来也一样。
- 强制放行（§5）的规矩不变：要的出口没过才算 `unmet`，不要的不在里面。

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

### 6.5 子运行在自己的 worktree 里

双路开发：父运行写规格侧（提案、规格增量、接口增量），人批准；之后两个子运行各干各的——一个写实现，一个照同一份规格写
黑盒测试、看不到实现；最后一站把两边合起来，用测试跑实现。

```yaml
- id: dual
  tasks:
    - {id: impl,  executor: {kind: subflow, flow: xforge/dual-impl,  workspace: worktree}}
    - {id: tests, executor: {kind: subflow, flow: xforge/dual-tests, workspace: worktree}}
  exit: [{kind: children}]
```

- **只在父运行自己在 worktree 里时允许。** 分叉要往父运行的分支上提交，用户自己的分支一律不自动提交。定义里写明
  `workspace: project` 的流程读定义时就拒；没写的看启动时选了什么，派任务时父运行不在自己的 worktree 里，任务失败并写明原因。
  `workspace` 只认 `worktree`（不写就是原来的样子：子运行与父运行共用 worktree）。
- **分叉提交。** 这一站本轮第一次派这种任务时，派工器在父运行的 worktree 里把没提交的全部提交到父运行的分支
  （`Harnix task <父运行> (<流程>): forked at <站>`，什么都没改也取当时的 HEAD），记在站上（`StationRec::fork`：轮次、提交）。
  同一轮的子运行都从这一个提交分出去，哪怕前一个已经合回去、父运行的分支往前走了；打回后的新一轮另做一次。
- **子运行的 worktree** 是 `<worktrees_root>/<子运行>`，分支 `<前缀>/<子运行>`，从分叉提交建；与父运行的并排，不嵌套
  （Conix 只认 `.wkts/<运行>` 这一层）。子运行的 `base_commit` 就是分叉提交；`WorktreeRec::fork` 记父运行的 worktree、分支和映射过来的输出目录。
- **输出目录。** 父运行的输出目录按同样的相对位置映射进子运行的 worktree，就是 `${parent.output_dir}`：分叉时的规格侧，
  子运行自己的一份，父运行之后再改也不影响它（不让它写，用站的 `writes` 收紧）。子运行自己的输出目录是它下面的
  `children/<站>/<任务>/`：两个子运行合回去时各在各的目录，不会写到同一个文件上。git 不管的部分（比如忽略掉的运行目录
  `conix/runs/`）分叉时把父运行输出目录里子运行 worktree 缺的文件拷过去（`_run/` 不拷），合回去时把子运行的输出目录整个拷回父运行那里，盖掉上一轮留下的。
  子运行自己的输出目录分叉时**是空的**：打回之后，上一轮同一个任务的子运行留下的目录（合回来的、拷回来的）不带进新的子运行，
  不然它会把前任划的工作包、记的台账当成自己的（一个它没做过的包已经 done）。前任的改动照样在树里，它的目录在父运行那里留到新的合回来。
  子运行站的 skill 里可以写 `${parent.output_dir}`（与 `${output_dir}`、`${data_dir}` 一样，开会话时换成路径），会话据此找到规格侧。
- **合回去。** 子运行交付了（`finished`），而且父任务还在等它：派工器先把子运行 worktree 里的全部提交到它的分支，再把父运行
  worktree 里没提交的提交上，然后把子运行的分支合进父运行的分支（`git.rs` 的合并：合并提交在旁边的临时 worktree 里做，父运行的
  树不会停在合了一半）。合上了，子运行的 worktree 和分支都删掉，父任务拿到子运行的结果。冲突了，父任务失败，错误里一个文件一条
  （`<路径>: changed on both sides`），父运行的树不变，子运行的分支留着、worktree 删掉。这一步不用人决定；父运行自己结束时照旧由人决定合不合进用户的分支。
- **失败或取消的子运行**（或者父任务已经不等它了）：它的工作提交到它的分支，分支留着，worktree 删掉，记成 `kept`；父任务照旧失败。
- **读隔离。** 子运行的会话读不到父运行的 worktree，也读不到同一站其他子运行的 worktree：一个合回去之后父运行的 worktree 里就有它的东西了，
  另一个读了就不再独立。派工器开会话时给宿主 `OpenSpec::deny_read`（绝对路径），宿主翻成 Agix 的 `Read(...)` deny 规则，进沙箱。
  - **名单在分叉提交时就定全。** 同一站的子运行是一个一个分叉的，分叉了就起，主会话随即打开（会话站与子 Agent 站都在它里面跑），
    而会话开了就换不了名单（宿主按 spec 认会话，再开一次 spec 不同就拒）。所以不等兄弟建起来再算：这一轮第一次分叉、做分叉提交时，
    这一站这一轮要分叉的每个任务——已认领在等的、`max_parallel` 下还没轮到的——都先取好子运行的 id，记在 `ForkPoint::children`；
    worktree 的路径由 id 定，还没建也知道。子运行分叉时它的名单就定了，记在 `ForkRec::siblings`：这一轮其他任务的 worktree，
    加上这一站当时还在跑的其他子运行的；之后不再变，兄弟合回去、worktree 删了也留在名单里，重开会话时与原来一模一样。
    哪一路先分叉、中间有没有重启，两边都从第一个会话起互相读不到；重启后剩下的分叉照取好的 id 建（id 下有没记下来的残留就收回重建，
    换个 id 兄弟就挡不住它了）。
  - **只打回一部分任务**（`flow_rework {to, tasks: [impl]}`）：新的一轮只给打回的任务取 id。重做的那一路读不到父运行，也读不到当时
    还在跑的兄弟；已经合回去的兄弟 worktree 删了，它的东西在父运行的分支上、新的分叉提交里，本来就看得见（xforge.md §5.1）。
    反过来，还在跑的兄弟的名单是它分叉时定的，里面没有新的这一路——打回时兄弟还没交付才会这样，从后面的站打回时兄弟都已合回去了。
  - **同一轮里重试**（`attempts`、`flow_retry`）的任务另取新 id：原来的 id 是前一个子运行的。新的一路名单是全的，已经在跑的兄弟的名单里没有它。
- **为什么走 git，不让子运行直接读父运行的目录。** 父运行的目录一直在变，读它就看得见兄弟合回来的东西；分叉提交是定下来、不会再变的规格侧，
  两边看到的一模一样，事后也查得到它们是从哪个提交出发的。合回去也走 git，冲突是 git 判的，不是谁覆盖了谁。
- **重启之后**：派工器从父运行的记录里把没做完的分叉（任务已认领、还没有子运行）与合回（任务在等、子运行已结束、worktree 还没处理）
  重新交出来（`WorkItem::Fork`、`WorkItem::Join`），两者重复做也只做一次。

## 7. 流程包、制品与包的命令

流程的组织与项目里的目录见 [layout.md](layout.md)：一个**流程包**自洽地放一个或几个流程（同一件事的几条路径）和它们用的全部件，
件不跨包共享；运行在**制品目录**（缺省 `conix/`）里有自己的目录，结束后归档进包的数据；站用 `deliverables` **声明制品**
（协议列给会话、骨架进站时放好、隐含一道"写好了"的检查）；包可以带**自己的命令**（QuickJS），站里的会话经 `flow_command`
调，结构化的数据（规格增量、台账、工作包）由命令写。skill 只写这一站怎么干活，用户补充写 `local.md`。

Conix 内置一个按这些写的开发流程包 `xforge`（能力与 xforge 插件对齐：规格驱动、按工作包实施、独立检查、可以打回、归档时
并进规格），源头是演示项目 `examples/xforge/.conix/flows/xforge/`，起来时放进全局流程库，`conix flow install xforge` 装进项目（§7.12）。它的流程就是流程本身，没有 `xflow` 这一层：运行的状态
在流程里，结构化的数据由 `xspec`、`xledger`、`xpkg` 三个命令维护。另一个内置包 `xreport`（`examples/xforge/.conix/flows/xreport/`，
[xreport.md](xreport.md)）给这个项目的提交历史出报告：统计由命令经系统的 git 算（§7.14 的 `flow.exec`），模型只照统计写分析，人定稿。

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
doctor: doctor                         # 自检脚本 scripts/doctor.js（§7.13）
jobs:                                  # 建议的作业（示意：内置的 xforge 现在不带）：装包时问要不要加（§7.12），加进来是停用的
  - {id: spec-drift, name: 规格巡检, cron: "0 9 * * 1", kind: {simple: {prompt: 对照 specs/ 看上周的提交，有没写进规格的改动用三五行回报}}}
  - {id: weekly-quick, name: 每周小修, cron: "0 10 * * 5", kind: {flow: {flow: xforge/quick, subject: 每周小修, inputs: {risk: low}}}}
```

| 字段 | 意思 |
|---|---|
| `flows` | 包里的流程，也是缺省的选路顺序 |
| `start.route` | 选路的顺序；不写就是 `flows`。选用的 `script` 形式让脚本回一个流程名 |
| `start.triage` | 分诊 skill（§7.2） |
| `pace` | `auto`（缺省）或 `step`（§7.4） |
| `commands` | 包的命令（§7.6）。写成列表（`[xspec, xledger]`）时每个命令的全部子命令都算会写；写成映射时 `read_only` 列出只读的子命令 |
| `settings` | 项目里的设置文件（`file`，相对项目根、不出项目）与缺省（`defaults`，映射）。见下 |
| `doctor` | 包的自检脚本（`scripts/<名>.js`），宿主在运行之外为一个项目跑它，查这个项目给包备齐了没有（§7.13）。与别的脚本引用一样读包时校验：名字不对、文件不在，包里的流程都报错 |
| `exec` | 包起的进程（`flow.exec`、命令门禁、`plan` 的 run）能有什么：`{network: bool, env: [名字, 前缀*]}`，缺省都没有（§7.14）。读包时校验，启动时钉进运行 |
| `jobs` | 包建议的作业，每条与项目 `.conix/jobs.json` 的一条同形：`id`、`name`、`cron`、`kind`（`{simple: {prompt, skills?, model?}}` 或 `{flow: {flow, subject?, inputs?}}`）、`catch_up?`、`guard?`。不跑，装包时问（§7.12）。见下 |

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
- **`jobs`：建议，不是安装。** 包可以说"用这个包的项目通常还要定时做这几件事"，但作业会无人值守地花钱、改文件，装包不能顺手把它们
  跑起来：装的时候列出来问（§7.12），要了才写进项目的 `.conix/jobs.json`，而且**在这台机器上是停用的**（同克隆下来的仓库带着的作业，
  architecture §8.5），要人启用。读包时校验：`id` 是 `[A-Za-z0-9_-]+`、不重复（它进项目后是触发记录的文件名）；`name`、`prompt` 不空；
  `cron` 是 5 段，能不能解析由宿主判（流程库没有 cron 解析，宿主校验包时一并判，坏的进不了库也装不进项目）；`guard` 只收宿主护栏有的键；
  流程作业的 `flow` 只能是**这个包的**流程：`<包>/<流程>`，或只写 `<包>`——包自己的 `flow.yaml`，或包只有一条流程时就是它
  （写进项目时换成全名）。不能指别的包：装进来的包不能替项目安排别人的流程。`subject` 不写就是作业的 `name`。
  不收 `project`（那是这台机器上的路径）与启停、时刻（那是这台机器的状态）。

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
| 门禁脚本 | `scripts/<名>.js` | 门禁 `kind: script`；`kind: command` 的 `plan` | 否 | 门禁结果（§3）；`plan` 也可以回要跑的 `runs`（§2.2） |
| 选值脚本 | `scripts/<名>.js` | `foreach: {script: …}`、`start.route` 的 `script` | 否 | 一个值（工作项列表、流程名） |
| 自检脚本 | `scripts/<名>.js` | `bundle.yaml` 的 `doctor`，宿主在运行之外调（§7.13） | 否 | `{findings}` |
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
| 进程 | `exec({argv, cwd?, timeout_seconds?, stdin?})` → `{code, signal, stdout, stderr, timed_out, truncated, ms, enforced}`（§7.14） | 全部；档位由调用方定，不由脚本选 |
| 运行 | `runs.list({bundle?, active?})`、`runs.get(id)`：状态、输入、输出、运行目录 | 全部，只读 |
| 设置 | `settings`：包的设置（§7.1）。运行里是启动时钉住的；运行之外的命令、门禁试跑是文件此刻合并缺省的结果；包没声明是 `{}`；文件读不成时读它就抛错 | 全部，只读 |
| 日志 | `log` | 全部 |

读写都限在工作目录与各件允许的范围里，出界报错。`flow.git` 由 Rust 跑、参数固定；要别的系统命令用 `flow.exec`，它经宿主的
执行服务起进程，档位按调用的上下文定（§7.14）：门禁、doctor、只读的子命令拿到的是 `read`，所以它们的"只读"靠档位守，
不靠"JS 起不了进程"。§2.3 的门禁脚本只用只读的那几组；`import`、`git`、`runs`、`exec` 对它同样可用。

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
| 子运行看父运行 | 子运行在自己的 worktree 里（`executor: {kind: subflow, workspace: worktree}`）：从父运行的分叉提交分出去，`${parent.output_dir}` 是分叉时规格侧的一份，交付后合回父运行的分支，读不到父运行与兄弟的 worktree（§6.5） | 做 |

### 7.11 兼容

- 流程定义只加字段，旧定义照读；`bundle.yaml` 的 `commands` 列表写法照读。
- 插件带的流程包、受管流程不变；插件包里的流程包同样有 `/flow:<包>` 地址。
- 每一项在 `3rds/INDEX.md` 的分叉记录里记一行；只在 Conix 里有的（地址、分诊、授权）标"in Conix"。

### 7.12 流程包的管理：全局库、项目、包文件

管理只在命令行（`conix flow …`），不进界面、不给模型、不需要实例。流程包在三处：

| 处 | 位置 | 跑不跑 |
|---|---|---|
| 内置的 | 编进二进制（源头 `examples/xforge/.conix/flows/` 下的每个包：`xforge`、`xreport`），各当一个包文件看 | 否：全局库的种子 |
| 全局库 | `~/.conix/flows/<包>/`（`CONIX_HOME`），旁边 `.registry.yaml`、`.backups/`、`.staging/`、`.lock` | 否 |
| 项目 | `<项目>/.conix/flows/<包>/`，`.conix/flows.lock.yaml` 记从库里装的哪一版（进 git） | 是 |
| 插件 | 插件目录（§7.11） | 是 |

- **项目只跑项目的与插件的。** 没有内置层（以前的 `MemPacks::examples()`、每次起实例重写的 `state/conix/flows-builtin/` 都去掉了），
  全局库也不读：库是"可以装的"，不是"在用的"。为什么：同一个包在库里改了，不该让所有项目的下一次运行悄悄换规矩；装进项目、进 git，
  哪一版在用就有记录。
- **包文件**：zip，`<包>-<版本>.zip`（版本取 `bundle.yaml`，没写是 `0.0.0`），放的就是包目录原样，根上一份生成的 `MANIFEST.yaml`：

```yaml
format: 1
bundle: {name, version, title, description}
conix: {version: "0.1.10", build: "0.1.10-2026112502", exported_at: "2026-11-25T08:00:00Z"}   # 导出它的 Conix
requires: {conix: ">=0.1.10", flow_definition_format: 1, flow_api: 1}
files: [{path, sha256, size}, …]
```

  - **不带身份**：没有用户名、主机名、路径。`requires.conix` 取 `bundle.yaml` 里可选的 `requires: {conix: ">=x.y.z"}`（只认这一种写法），
    没写就是导出它的那个版本。
  - **不进包的**：`skills/*/local.md`（项目自己的补充）、点开头的文件、`__pycache__`、符号链接（遇到就报错）。
  - **摘要**：`files` 按路径排好，每个一行 `<sha256> <size> <path>`，整段的 SHA-256。目录、包文件、内置的同样的内容摘要一样；
    registry 与 lock 记的都是它，所以 `local.md` 改了不算包改了。
- **写进库或项目都先核对再动手**：内容在暂存目录（库的 `.staging/`、项目的 `.conix/state/conix/`）里放好、逐项核对，过了才改名换上，
  换下来的进备份（库的 `.backups/`、项目的 `.conix/state/conix/flow-backups/<包>-<时刻>/`）。拒绝：`format` 不认识；文件与 MANIFEST
  对不上（缺的、多的、SHA-256 或大小不符）；绝对路径、`..`、反斜杠、隐藏文件、`local.md`、符号链接；超过 512 个文件、总共 10 MiB、
  单个 2 MiB；运行的 Conix 低于 `requires.conix`，或定义格式、流程库接口比这个构建新；流程库校验（每个流程、每个件、`commands`
  列的文件）不过。从目录来的（install、update、backfill）与内置的同样走这一套。写库拿 `.lock`（flock），几个实例与命令行同时动也不乱。
- **registry**（`~/.conix/flows/.registry.yaml`）：每个包 `source`（`builtin` / `import` / `backfill`，最后写它的是谁）、`version`、`digest`、
  `at`；内置的再记 `shipped: {version, digest}`——库里这份是从哪一版内置的解出来的；`remove --global` 删掉的内置包留一条 `removed: true`。
- **lock**（`<项目>/.conix/flows.lock.yaml`）：每个从库里装的包 `from: global`、`version`、`digest`、`at`。项目自己建的、`import --project`
  进来的没有条目（"只在项目"）。项目的摘要对不上 lock 就是"改过"。

**内置的包进全局库。** 任何一条 `conix`（含 `conix host`）起来时查一次：库里没有某个内置包（也没删过）就解开；有、没改过
（目录的摘要等于 `shipped.digest`）而这个 Conix 带的不同且版本不更旧，就换成它，打一行提示；改过的不动，`doctor` 报"有新的内置版"。
`conix flow reset <包> | --all` 把库里的内置包恢复成这个 Conix 带的那版（当前的先备份），不碰项目。
同版本内容不同（开发中没改版本号）也算可以换：没改过的那份本来就是上一版内置的，换掉不丢东西。

**命令**

```
conix flow list    [--global | --all]            项目的（缺省）/ 全局库的 / 两边对照，带状态
conix flow show    <包> [--global]                说明、流程、命令、版本、要求的 Conix、来源
conix flow diff    <包>                           项目 vs 全局库：版本、增删改的文件
conix flow install <包> [--force] [--jobs ask|yes|no]          全局库 → 项目；项目已有同名就拒绝（用 update）
conix flow update  [<包> | --all] [--force] [--jobs ask|yes|no]  全局库 → 项目
conix flow backfill <包> [--force]                项目 → 全局库（回填；库里没有就新加）
conix flow remove  <包> [--global] [--force]      删项目的（缺省）或库里的；都先备份
conix flow clean   [--global]                     清备份、暂存残留、孤儿条目
conix flow reset   <包> | --all                   库里的内置包恢复初始
conix flow import  <文件> [--project] [--force]   包文件 → 全局库（缺省）或项目
conix flow export  <包> [--global] [-o 文件]      项目的（缺省）或库里的 → 包文件
conix flow inspect <文件>                         看包文件、核对
conix flow verify  <归档目录> | --all             核对归档（layout.md §3；--all 扫项目的 conix/*/archive/*/）
conix flow doctor  [--global] [--fix]             体检（§7.13）
```

公共的选项：`-C <目录>`（项目，缺省是当前目录所在的项目；不叫 `--project`，那是 `import` 的"进项目"）、`--yes`、`--dry-run`。

update（全局库 → 项目），用 lock 判项目改过没有：

| 情况 | 动作 |
|---|---|
| 库里更新、项目没改 | 更新，`local.md` 带过去，lock 跟上 |
| 项目改过（或没有 lock 条目、内容又不同） | 拒绝，列出更新会改的文件；`--force` 覆盖（先备份） |
| 同版本 | 无事；内容一样而 lock 缺条目就只补 lock |
| 库里更旧 | 拒绝；`--force` 才降级 |

backfill（项目 → 全局库），`local.md` 不回填：

| 情况 | 动作 |
|---|---|
| 库里没有 | 加，`source: backfill` |
| 项目版本更高 | 替换（先备份），lock 跟上 |
| 同版本、内容不同 | 拒绝：改 `version`，或 `--force` |
| 库里更新 | 拒绝（会盖掉新的）；`--force` |

- 回填的是内置包：库里那份成"改过的内置"，`reset` 能回去。
- install 已有同名拒绝（`--force` 换掉，先备份）；remove 在项目里有进行中的运行用这个包时拒绝——定义、门禁、脚本钉在运行里，
  skill 与模板不钉，删了就断——`--force` 才删。`remove --global` 也删得掉内置包，起来时不再解开，`reset` 找回。
- import 已有同名拒绝，`--force` 覆盖；`--project` 进项目、不记 lock。export 缺省导项目的，`--global` 导库里的；插件的包不在这里导。
- **确认**：改东西的命令先列出要做什么（包、版本从→到、增删改的文件、备份到哪、要留意的：命令门禁、回填内置包……），问 `[y/N]`；
  `--yes` 跳过；不是终端（脚本里）不给 `--yes` 就拒绝；`--dry-run` 只列不做。确认之后拿锁再看一眼目标，确认期间被别处改了就不做。
- **包建议的作业**（§7.1 的 `jobs`）：install，以及带来新作业的 update（新版有、装着的那版没有的；旧版提过的人当时没要，不再追着问），
  计划里列出每个作业（ID、名字、cron、做什么），项目里已有同 ID 的标出来、**不加也不改**。包装好之后**另问一句**
  `把这 N 个建议的作业加进项目（停用）？[y/N]`：装包与让它定时做事是两件事，前一句的 y 不替后一句答。`--jobs yes` 不问就加、`no` 不加、
  `ask`（缺省）问；给了 `--yes` 而没写 `--jobs` 就**不加**，并说一声怎么加——不问的时候替人多加会定时跑的东西，哪怕停着，也该是人明说的。
  不是终端、没有 `--yes` 照旧拒绝；`--dry-run` 连作业一起列。加进去的写 `.conix/jobs.json`，本机的 `jobs-state.json` 记停用；
  在跑的实例经 socket 收一帧 `ReloadJobs`（同 `ReloadFlows`），重读作业，作业标签跟着变。命令行写作业定义不看项目受不受信：
  是人在终端里让它做的，加的又都停着；实例那边照旧按信任读。
- **删包不删作业**：remove 列出起这个包的流程的作业（`<包>` 或 `<包>/…`），提醒它们之后会因为找不到流程失败；删不删在作业标签里说。
- **在跑的实例**：命令行改了项目的包，这个项目若有实例在跑，经它的 socket 发一帧 `ReloadFlows`（不接入、不踢人，architecture §8.3），
  实例重列流程、流程标签跟着变；不在、连不上都不算错。界面里没有导出导入（以前的 `/flow:export`、`/flow:import` 与端口的
  `ExportFlow`/`ImportFlow` 去掉了）；地址或 `flow_start` 指的包项目里没有、库里有时，回的话里写 `conix flow install <包>`
  （界面经 `ListLibrary` 问实例库里有什么）。实例起来时有进行中的运行而它的包不在项目里，接入时提示装哪个。
- **`export`、`import` 仍是包的保留名**（`bundle::RESERVED_BUNDLES`）：留着不给包用，以后界面要加管理的入口时不与包的地址混。
- **迁移**：以前直接用内置 xforge 的项目，升级后 xforge 不再自动出现：`conix flow install xforge`；有进行中运行的，实例提示或
  `doctor --fix` 装上。
- 落点：`crates/host/src/flows/package.rs`（包文件、摘要、暂存与换上）、`library.rs`（全局库、registry、内置包的解开与升级）、
  `manage.rs`（lock、规矩、计划与执行）、`doctor.rs`（体检）、`builtin.rs`；`crates/conix/src/flow_cli.rs`（参数、确认、通知实例）。

### 7.13 包的自检：`doctor`

`conix flow doctor [--global] [--fix]` 查全局的流程库与一个项目装的包，有 `error` 时退出码非 0（`crates/host/src/flows/doctor.rs`）：

- 全局库：目录与 registry 对不上（孤儿条目、没登记的目录、手改过的）；每个包的校验与 `requires.conix`；内置包未改 / 改过 / 删了 /
  有新的内置版（给 `reset`）；暂存残留、备份占了多少。
- 项目：每个包的校验与要求；lock 与目录对不上；改过（对 lock）；库里有新版（给 `update`）；只在项目（给 `backfill`）；遮住了插件的同名包
  （看 `<家目录>/plugins/` 下的包，启没启用命令行不知道，只提醒）；用了保留名；进行中的运行用的包不在项目里；有命令门禁而项目没受信
  （照 Agix 的 `app.trusted_projects` 读）；制品目录与 `.wkts/` 已经有了、git 却没忽略 `runs/` 与 `.wkts/`；包自己的体检（下面）；
  归档（`verify --all` 的摘要）。
- `--fix` 只做不丢东西的：清暂存、去孤儿条目、内容与库里一样却没进 lock 的补记、缺了（没删过）的内置包重新解开、进行中的运行要的包
  库里有就装上。别的只报，给出该跑的命令。修完再查一遍定退出码。

通用的检查归宿主（上面）。**包自己要项目备什么，
只有包知道**（xforge 要治理文件读得成、verify 菜单合规），所以这一段是包的脚本：`bundle.yaml` 写 `doctor: <脚本>`，宿主为项目调
流程库的 `doctor(packs, layout, 包, 项目)`，按条列出结果。

```js
// scripts/doctor.js：input = {bundle, project, data_dir, params: {}}
export default function (input, flow) {
  return { findings: [{ level: "warn", message: "缺 .conix/governance/constitution.yaml", path: ".conix/governance/constitution.yaml", fix: "写一份 principles: […]" }] };
}
```

- **与门禁脚本同一个沙箱**（§2.3）：只读，时限 30 秒，同样的内存与返回值上限；宿主给了执行服务时有 `flow.exec`
  （档位 `read`，在项目里，记进宿主日志，§7.14）。`flow.read.*`、`glob`、`stat` 限在项目里；
  `flow.settings` 是项目此刻的文件合并缺省（同运行之外的命令，读不成时读它就抛错，由脚本报成 `error`）；`flow.git.*` 在项目里、
  没有基线（`changed` 要给 `since`）；包内 `import` 照常。**没有 `flow.runs`**：项目备没备好与在途的运行无关，连查两次要说一样的话。
  `data_dir` 是包的数据目录（相对项目根，宿主没给布局时是 `null`）。
- **回什么**：`{findings: [{level: error|warn|info, message, path?, fix?}]}`，`path` 是项目里的文件，`fix` 是给人看的建议。
  `error`：照项目现在的样子包跑不了（某条流程会拒绝启动、某道门禁必然 `error`）；`warn`：会缺什么、某处会被拒，别的能跑；
  `info`：知道就好（第一次运行会建的目录）。形状不对（多了字段、`level` 不认识、`message` 空）、抛错、超时、读出界，都成一条点名脚本的
  `error`。没写 `doctor` 的包回空报告。
- **从不修**：没有写，`flow.exec` 只拿到 `read`，`fix` 也只是一句话，宿主不执行它。为什么：自检是"看一眼"，跑一次就改了文件的检查，人得先审它改了什么；
  修由人或由包的命令（经人）做。
- 落点：`crates/flow/src/runner/doctor.rs`（`doctor`、`DoctorReport`、`DoctorFinding`、`DoctorLevel`）、`bundle.rs`（`BundleMeta::doctor`、
  `doctor_problems`）；宿主的 `conix flow doctor` 调它。

### 7.14 执行：一个服务，档位按上下文定

起进程的地方原来各有各的规矩：命令门禁（`sh -c`、环境白名单、只在受信项目、无隔离）、`plan` 的 run（按 argv）、流程库
自己的 git、宿主的通知出口与体检；包 JS 一个进程都起不了。现在**流程库替项目或包起的每个进程都经宿主的执行服务**
（`kernel/host.rs` 的 `Exec`）：命令门禁、`plan` 的 run、包 JS 的 `flow.exec`。谁能跑、带什么环境、能不能上网、能写哪里、
怎么记，一处定。

```js
// 内置包 xreport 的 commands/xstat.js（经 lib/git.js）：在运行的工作目录里跑系统的 git
const r = flow.exec({ argv: ["git", "--no-pager", "-c", "core.fsmonitor=false", "log", "-z", "--numstat", "--end-of-options", rev, "--", ...paths] });
// r = {code, signal, stdout, stderr, timed_out, truncated, ms, enforced: {backend, filesystem, network}}
```

- **只收 argv**：程序与参数原样传过去，中间没有 shell，`;`、`$(…)`、`|` 在参数里只是字符；给一条命令行（字符串）就抛错。
  要 shell 自己写 `["sh", "-c", …]`——那是包作者明着写的，与模型写进数据里的字符串无关（§2.2 `plan` 的道理）。
- **`cwd`** 相对脚本工作的目录：运行里是运行的工作目录（项目或 worktree），运行之外（doctor、运行之外的命令、试门禁）是项目
  （或试的目录）；绝对路径、`..`、经符号链接出界、不存在的都抛错。`argv[0]` 是带 `/` 的相对路径时按 `cwd` 找。
- **输出**：stdout、stderr 各至多 4 MiB（留开头），多了截掉、`truncated` 为真。**时限**：`timeout_seconds` 缺省、写多了都取
  脚本剩下的时间（脚本自己的时限，命令是 30 秒）；超时整个进程组杀掉，`timed_out` 为真。`stdin` 是一段文本。
- **退出非 0、被信号杀、超时是回答**，脚本自己判；**起不来、被拒、调用写错是抛错**（错误的 `code` 是 `exec`），起之前整份核对。

**档位由调用方的上下文定，不由脚本选**：脚本能选，就会选最宽的。

| 谁在调 | 档位 | 记到 |
|---|---|---|
| 门禁脚本（`kind: script`）、`plan` 的脚本、`foreach` 脚本 | `read` | 运行历史 |
| 命令门禁、`plan` 的 run | `read`：门禁判活，不干活 | 运行历史 |
| 运行里的命令：只读的子命令 | `read` | 运行历史 |
| 运行里的命令：会写的子命令、命令步骤（`executor: {kind: command}`） | `workspace`：运行目录、包的数据目录，站有 `writes` 时加上每条 glob 第一个通配符之前的那段目录 | 运行历史 |
| 运行之外的命令（人调、模型调都一样）、试门禁（`flow_gate_try`）、doctor | `read` | 宿主日志 |
| 宿主自己的：通知出口（用户自己在全局 `conix.yaml` 里配的） | `host`：不隔离 | 宿主日志 |

运行之外的命令写的东西比每个运行都活得长，写由命令自己的写入器管（§7.6），它起的进程只给 `read`。`workspace` 给的是目录，
`writes` 的 glob 本身照旧由写入器与会话的写入执法守（§7.9）。

- **网络与环境由 `bundle.yaml` 的 `exec` 声明**：`exec: {network: true, env: [CI_TOKEN, CI_*]}`。缺省不上网、环境只有基本的
  （`PATH`、`HOME`、语言区域、`TMPDIR`……）；`env` 按名字或以 `*` 结尾的前缀透传，再减去宿主的拒绝表（§2.2：写了也不给，
  拒绝表的项以 `*` 开头是后缀）。读包时校验（未知字段、不是变量名的项都报错），启动时钉进运行（`Pinned::exec`），运行之外读包
  此刻的。命令门禁与 `plan` 的 run 也按它定网络；不在包里的门禁不上网。命令门禁仍带 `FLOW_*` 变量，`flow.exec` 的进程不带。
- **谁能跑**：只在受信的项目里，且 `flows.command_gates` 没关——这个开关就是**项目与包定义的全部执行**的总开关。不行时
  `flow.exec` 抛错并说为什么（开关关着、项目没受信），命令门禁照旧是 `error`。宿主没给执行服务的地方（没有运行时、`doctor`
  不带执行服务地调）`flow.exec` 抛 "not offered"。
- **记录**：每个进程都记一笔，从不记输出。运行里是运行历史的一条 `exec`（`Engine::note_exec`）：`for`（`gate semgrep`、
  `flow.exec in xreport:xstat`）、`argv`、`cwd`、`profile`、`network`、`code`、`signal`、`timed_out`、`truncated`、`ms`、
  `enforced`，起不来时是 `error`；运行之外是宿主日志里 target `flow::exec` 的一行。argv 进记录，所以别把密钥写进参数，用 `env`。
- **后端与接缝**：现在只有普通子进程（流程库的 `PlainExec`，Conix 的 `host::exec::ConixExec` 缺省也是它）：档位与网络
  **照传不守**，每条结果与记录的 `enforced` 都写 `{backend: plain, filesystem: false, network: false}`，读记录的人不会把档位
  当成隔离。OS 级的隔离（seatbelt、bwrap，或由 Agix 提供）之后再定：它实现 Conix 的 `Confine`（`crates/host/src/exec.rs`），
  把一个请求包成在隔离里跑的请求并说它守住了什么，再交给 `run_plain` 起；调用方、档位规则、记录都不用改。
- **确定性**：不碰进程的包 JS 照旧确定；用了 `flow.exec` 就不承诺（外部程序的输出流程库管不了）。门禁的证据仍只是它读过的
  文件，进程的输出不算证据。
- **不经这个服务的**：参数由 Rust 写死的内部 git（`runner/git.rs`、宿主体检的 `check-ignore`、`@` 补全的 `ls-files`）。它们
  不跑项目给的命令，但仓库自己的配置能让 git 起程序，所以每次都关掉 `core.fsmonitor`、不分页，读 diff 的加 `--no-ext-diff
  --no-textconv`。

## 8. 落点

| 内容 | 位置 |
|---|---|
| 门禁定义、脚本件、校验 | `crates/flow/src/kernel/parts.rs`、`kernel/validate.rs` |
| 门禁结果、`CheckRec` 的证据、重跑门禁 | `kernel/host.rs`（`GateResult`）、`kernel/run.rs`、`kernel/engine.rs`（`gate_result`、`rerun_gates`） |
| 执行 `check` / `command` / `script` | `runner/gate.rs`、`runner/plan.rs`（命令门禁的 `plan`）；`runner/files.rs`（限在工作目录里读、记证据）、`runner/checks.rs`、`runner/script.rs`（rquickjs，与 Agix 同一版本） |
| 打回时告诉会话为什么 | `kernel/run.rs`（`StationRec::reworked`）、`kernel/protocol.rs` |
| 确认、轮询审批 | `kernel/def.rs`（`ExitCond::Confirm`）、`kernel/txn.rs`、`runner/approvals.rs`（新） |
| 出口的 `when`、门禁的 `facts` | `kernel/def.rs`（`ExitWhen`）、`kernel/when.rs`（读值、校验）、`kernel/txn.rs`（`judge`）、`kernel/host.rs`（`GateResult::facts`）、`kernel/run.rs`（`WhenRec`、`NotRequiredRec`）；`tests/when.rs` |
| 受管流程 | `packs.rs`（`ProjectPacks`、`DirPacks`）、`kernel/run.rs`（`governed`） |
| 子流程结果、`children`、交付 | `kernel/def.rs`、`kernel/txn.rs`、`runner/tools.rs`（`flow_submit`） |
| 子运行在自己的 worktree 里（分叉提交、合回、读隔离） | `crates/flow/src/kernel/fork.rs`、`runner/fork.rs`（新）、`runner/worktree.rs`（`make` 带起点与取好的 id）、`runner/host.rs`（`OpenSpec::deny_read`）；Conix 把 `deny_read` 翻成 `Read(...)` deny：`crates/host/src/flows/session_host.rs` |
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
| 流程包的管理（全局库、registry、lock、内置包、包文件与 `MANIFEST.yaml`、`conix flow …`、体检） | `crates/host/src/flows/package.rs`、`library.rs`、`manage.rs`、`doctor.rs`、`builtin.rs`、`crates/conix/src/flow_cli.rs`、`crates/flow/src/bundle.rs`（`requires`、`RESERVED_BUNDLES`）、`crates/model/src/flow_addr.rs`（找不到包时的提示）、`port::Command::{ReloadFlows, ListLibrary}` |
| 包的自检（`doctor`） | `crates/flow/src/runner/doctor.rs`、`runner/scriptenv.rs`（`Env::for_doctor`）、`bundle.rs`（`BundleMeta::doctor`）；`crates/flow/tests/doctor.rs` |
| 执行服务（`Exec`、档位、记录、`flow.exec`、`bundle.yaml` 的 `exec`） | `crates/flow/src/kernel/host.rs`（`Exec`、`ExecRequest`、`ExecProfile`、`Enforcement`）、`runner/exec.rs`（`PlainExec`、`run_plain`、记录、环境）、`runner/scriptenv.rs`（`ExecScope`）、`runner/script.rs`（`flow.exec`）、`bundle.rs`（`ExecDecl`）、`kernel/parts.rs`（`Pinned::exec`）、`kernel/engine.rs`（`note_exec`）；Conix 的后端与接缝 `crates/host/src/exec.rs`；`crates/flow/tests/exec.rs` |
| 内置的 xforge、xreport | `examples/xforge/`（项目与两个流程包）、`crates/host/build.rs`、`crates/host/src/flows/builtin.rs`（种子，进全局库）；`crates/flow/tests/xforge.rs`、`crates/flow/tests/xreport.rs`、`crates/conix/tests/live_xforge.rs` |

## 9. 往回迁

`crates/flow` 不跟 Conix 的格式化（目录里的 `rustfmt.toml` 关掉了），保持与 Harnix 那份逐行可比；新代码按原有风格写。
每一项落地后在 `3rds/INDEX.md` 的分叉记录里记一行，整理成给 Harnix 的 issue 时按这张表来。
