# 插件与 XForge

本文是 Codix 的插件机制，以及第一个插件 `xforge` 的设计。写的是依赖落地之后的样子：

| 依赖 | 内容 |
|---|---|
| harnixai/Agix#90 | JS 基础库只叫 `agix`，`codix` 这个全局名留给宿主 |
| harnixai/Agix#91 | 嵌入方经 `EmbedConfig` 给插件命令注册宿主 JS 库（JSON 进出，不交出 rquickjs 类型），插件以 `uses` 声明 |
| harnixai/Agix#92 | `ext.run`：宿主经 RPC 运行插件命令，取回结构化结果 |
| harnixai/Harnix#68 | 流程包来源加一种"插件"（`Origin::Plugin`） |

XForge 原来的实现（TypeScript + Node，`harnixai/xforge`）只作参考：**数据归插件命令，过程归 Harnix 流程**。
XForge 自己的状态机、编排者、往 `.claude/` `.codex/` 投影的脚手架、PreToolUse 钩子都不搬。

## 1. 三条通道

```
                 plugin.* · ext.run              （JSON-RPC，运行时）
  Codix 实例 ─────────────────────────────────►  Agix
  （Hub、流程派工器）                               │ RunExt：插件命令跑在 QuickJS 里
        ▲                                          │
        └──── codix.* 宿主库（Rust 注册，进程内）───┘
```

| 方向 | 机制 | Codix 拿它做什么 |
|---|---|---|
| 宿主 → 插件 | `plugin.*`（装、列、启停、卸） | `/plugin` |
| 宿主 → 插件 | `ext.run` | 流程的门禁（`Gates`）与程序步骤（`Programs`）跑插件命令 |
| 插件 → 宿主 | `codix.*` 宿主库 | 插件命令开流程运行、查运行状态 |
| 插件 → 引擎 | `tools.*`、`agix.*` | 读写项目文件、YAML / JSON Schema 等纯计算 |

**边界的变化**：引擎仍然只经 JSON-RPC 打交道；**启动时的装配是 Rust 的**——`EmbedConfig` 里本来就有
`project_dir`、`product`，宿主 JS 库同样在这里注册。动工时 CLAUDE.md「已定的边界」那一条照此改写。

## 2. 目录

```
仓库
plugins/
├── xforge/                       插件源码（见 §6.1），打包成 xforge-<版本>.zip
└── types/codix.d.ts              宿主库的类型声明，插件作者用；与 Agix 的 runext.d.ts、agix.d.ts 放一起

发行包
bin/codix
share/codix/plugins/*.zip         自带插件目录：随程序装上，里面是包，不是已安装的插件

~/.codix/                         （architecture §8.1）
├── plugin-store/*.zip            用户自己放的包，可选；/plugin 一并列出
├── plugins/cache/<名>/<版本>/    已安装的插件，归 Agix（global 层）
└── plugins/enabled.json          启停状态，归 Agix

项目
<项目>/xforge/                    XForge 的数据：manifest、constitution、specs、changes/…，随项目提交
<项目>/.codix/state/codix/runs/<运行>/  流程运行的过程目录（harnix-flow，不进 git）
```

- 自带插件目录从 `current_exe()` 推：`<exe 所在目录>/../share/codix/plugins/`；`CODIX_PLUGIN_DIRS`
  （`:` 分隔）在开发与测试时替换它。开发时 `scripts/build-plugins.sh` 把 `plugins/*` 打到
  `target/plugins/`，`scripts/demo.sh` 与 LIVE 用它。
- 装好 Codix 时**一个插件都没装**：cache 是空的，自带目录里只放着包，要用 `/plugin install` 装。
- 包名约定 `<名>-<版本>.zip`，列表只按文件名列，不解包；内容以安装时 Agix 读出来的为准。

## 3. `/plugin`

本地斜杠命令（`chat_update` 的 LOCAL），不进对话：

| 写法 | 做什么 |
|---|---|
| `/plugin` | 浮层列出可装的（自带目录 + `plugin-store/`）与已装的（版本、启停、来源、冲突） |
| `/plugin install <名或 zip 路径>` | 安装，先看披露再确认（下面） |
| `/plugin enable <名>` / `disable <名>` | 启停 |
| `/plugin remove <名>` | 卸载 |
| `/plugin info <名>` | 再看一次披露 |

**端口**（architecture §3）：`ListPlugins` → `Plugins { available, installed }`；`InstallPlugin { source }` →
`PluginInstalled { plugin, disclosure }`；`SetPluginEnabled { name, enabled }`；`UninstallPlugin { name }`；
事件 `PluginsChanged`。mock 用脚本化的插件实现同一组命令。

**安装**：
1. `Hub` 算 zip 的 SHA-256，`plugin.install { name, version, download_url: "file://…", checksum, scope: "global" }`。
2. 浮层摆出两份披露，确认一次：
   - **Agix 的** `disclosure`：能力（命令能写什么、`uses` 了哪个宿主库）、到达模型的每段文字。
   - **Codix 的**：Agix 不读 `x-` 段，`[x-codix]` 带来的由 Codix 自己披露——流程（名字、标题）、站的 skill
     正文（到达模型）、门禁与步骤各对应哪条插件命令、是否要 `command` 门禁。
3. Agix 0.4 起 `plugins.enable_on_install` 缺省为 false，装完插件是停用的：确认后 `plugin.enable`；不确认就 `plugin.uninstall`。

**启停之后**：Agix 的命令只进此后新建的会话，提示里照实说；Codix 立刻重读插件流程包（§4），推
`FlowUpserted` / `FlowRemoved`。在跑的运行不受影响（启动时已钉住定义与件）。

## 4. 插件里的流程包：`[x-codix]`

```toml
[x-codix]
flows = "flows"                                  # 一个 harnix-flow 流程包：flows/ skills/ rules/ policies/ gates/

[x-codix.gates]                                  # host 门禁名 → 插件命令（只读）
xforge-structure = "xflow check structure"

[x-codix.steps]                                  # program 步骤名 → 插件命令（可写）
xforge-open = "xflow new"
```

- **分层**：Codix 实现一层 `PluginPacks`，挂所有已启用插件的流程包。叠加顺序：项目 > `~/.codix/flows/`
  可写层 > 插件 > 内置示例。来源记 `Origin::Plugin`（Harnix 0.4.7，不带名字；插件名从 `Packs::source(flow)` / `DefRow::source` 取）；可写层存同名的就是
  覆盖，删掉回到插件那份。两个插件同名的流程、件、门禁名、步骤名，按插件名排序先到的胜出，后到的报在
  `/plugin` 里：被盖掉的同名流程在库里是 `Withheld::Shadowed`，`DirPacks::shadowed()` 列出它们。
- **目录**（`Catalog`）：所有已启用插件 `[x-codix.gates]` 与 `[x-codix.steps]` 的名字并进 `gates` / `steps`。
  流程只用目录里有的名字，引用了停用插件的门禁的流程照校验规则报出来。
- **信任**：插件的流程是人看过披露后装的，不受项目信任的约束；插件命令在会话里照常受 Agix 的项目信任与权限。
- 插件的站 skill 放在流程包自己的 `skills/` 里，**不**写进 Agix 的 `[skills]`：它们只给站用，不该出现在每个
  会话的 skill 列表里。

## 5. 宿主侧的两件事

### 5.1 宿主库 `codix.*`

实例起来时在 `EmbedConfig` 注册（`host/src/plugins/hostlib.rs`），执行走 `Hub` 的 `dispatch`，和端口命令同一条路，
TUI 照常收到事件。

| 函数 | 做什么 |
|---|---|
| `codix.flow.defs()` | 这个项目能用的流程（名字、标题、来源） |
| `codix.flow.start({ flow, title, subject?, inputs?, workspace? })` | 开一次运行，回 `{ run }` |
| `codix.flow.runs({ flow?, active? })` | 运行列表 |
| `codix.flow.run(id)` | 一次运行：阶段、各站状态与输出 |
| `codix.flow.current()` | 发起调用的会话所在的运行与站；不在运行里为 `null` |

**谁能调**，按 `HostCallContext`：
- 会话的作用域规则同宿主工具（architecture §8.5）：流程运行里的会话只看得到自己的运行、不能 `start`；作业会话对流程只能看。
- `via` 是 `host`（经 `ext.run`，即门禁与步骤里）或者没有会话时：**只开放读的函数**，免得一次门禁检查反过来推进了流程。
- `start` 本身不再问人：调它的插件命令不是 `read_only` 的子命令，那次 `RunExt` 已经过了 Agix 的权限询问（用户亲手敲的算已批准）。

### 5.2 门禁与步骤：`ext.run`

- **门禁**（`PluginGates`，实现 harnix-flow 的 `Gates`）：门禁名按 `[x-codix.gates]` 找到命令；
  `ext.run { command, cwd: 运行的工作目录, writes: false, input }`，**不带会话**——结果只取决于项目里的数据，与站的会话还在不在无关。
  `input` 是 `{ run, flow, station, gate, inputs, outputs: { <站>: <输出> }, workspace, project_root }`；
  命令回 `{ pass, detail }`，原样成为 `GateResult`。`ok: false`（插件出错、要问人）算不过，`detail` 写原因，同时记进事件日志。
- **要跑命令的门禁**（单测）：同样是 host 门禁，`ext.run` 带上这一站解析好的权限（换成 Agix 的规则，同
  `session_host` 现有的换法），命令里的 `tools.Bash` 照这些规则判；要问人的直接失败。
- **步骤**（`PluginPrograms`，实现 `Programs`）：步骤名按 `[x-codix.steps]` 找到命令，`writes: true`，`input` 同上再加 `item`；
  返回值交给 `Engine::submit`，错误交给 `Engine::fail`。
- 门禁缺省时限 600 秒，步骤 300 秒；实例停时在跑的 `ext.run` 取消，派工器下次启动重做（同现有的租约规则）。
- `ext.run` 带会话的用法这一版不用，留给以后面板上的操作。

## 6. 插件 `xforge`

### 6.1 包

```
plugins/xforge/
├── plugin.toml
├── commands/             xflow.js、xspec.js、xarch.js、xforge.js 与共用的 lib/*.js（ES2023，不经构建）
├── scripts/guard.js      tool.around 守卫
├── agents/xforge-executor.md
└── flows/                harnix-flow 流程包
    ├── flows/            xforge-quick.yaml、xforge-solid.yaml（xforge-major.yaml 第二版）
    ├── skills/           xforge-propose/ xforge-design/ xforge-check/ xforge-apply/ xforge-verify/ 各一个 SKILL.md
    ├── policies/         xforge-station.yaml
    └── gates/            每道门禁一份，kind: host
```

```toml
name = "xforge"
version = "0.1.0"
description = "规格驱动的变更：规格增量、台账、门禁，按 quick / solid 流程推进"
uses = ["codix"]

[[command]]
name = "xflow"
entry = "commands/xflow.js"
description = "变更：新建、修订、台账、计划、检查、查看"
read_only = ["show", "ledger list", "plan list", "check"]
writes = ["xforge/**"]

[[command]]
name = "xspec"
entry = "commands/xspec.js"
description = "规格与规格增量"
read_only = ["list", "get", "delta", "pending", "check"]
writes = ["xforge/**"]

[[command]]
name = "xforge"
entry = "commands/xforge.js"
description = "项目初始化、体检、开始一次变更、查看进行中的变更"
read_only = ["doctor", "status"]
writes = ["xforge/**"]

[[agent]]
name = "xforge-executor"
description = "按工作包实施并验证"
prompt = "agents/xforge-executor.md"

[[script]]
point = "tool.around"
entry = "scripts/guard.js"

[x-codix]
flows = "flows"

[x-codix.gates]
xforge-structure    = "xflow check structure"
xforge-spec-delta   = "xflow check spec-delta"
xforge-constitution = "xflow check constitution"
xforge-ledgers      = "xflow check ledgers"
xforge-integrated   = "xflow check integrated"
xforge-unclaimed    = "xflow check unclaimed"
xforge-unit-tests   = "xflow check unit-tests"

[x-codix.steps]
xforge-open     = "xflow new"
xforge-packages = "xflow plan list"
xforge-archive  = "xflow close"
```

- 命令写成普通的 ES2023，不引入 Node 构建链；XForge 的 TS 代码按段参考着重写，文件 I/O 换成 `tools.Read` /
  `tools.Write` / `tools.Glob`，YAML、frontmatter、JSON Schema、SHA-256 用 `agix.*`。
- 返回值保留 XForge 的信封：`{ ok, data }` / `{ ok: false, error: { code, message } }`；退出码换成 `code`。

### 6.2 命令

| XForge | 插件 | 说明 |
|---|---|---|
| `xspec list/get/delta/pending/check`、`add/modify/remove/revert` | `xspec` 同名 | 全部保留 |
| `xflow new/amend` | `xflow new/amend` | `new` 由步骤 `xforge-open` 跑，id 与基线提交由命令定 |
| `xflow ledger …`、`xflow plan …` | 同名 | 保留 |
| `xflow close` | `xflow close` | 由步骤 `xforge-archive` 跑：增量并进 `specs/`、归档 |
| — | `xflow check <门禁>` | 新的：七道门禁的检查逻辑，只读，回 `{ pass, detail }` |
| `xflow state/show/inspect` | `xflow show` | 数据加上 `codix.flow.run` 的运行状态 |
| `xflow run/advance/attest/guide` | 不要 | 推进与署名是派工器与 `confirm` 的事；站要做什么由 skill 与站消息说 |
| `xforge init/doctor/repair` | `xforge init/doctor/repair` | 只管 `xforge/` 下的数据 |
| `xforge update/sync/remove` | 不要 | 不再往 `.claude/`、`.codex/` 投影，装插件就是安装 |
| — | `xforge start <流程> "<标题>" --risk <级>` | 新的：按流程的 `eligibility` 查风险等级，再 `codix.flow.start` |
| — | `xforge status` | 进行中的变更与它们的运行 |
| `xarch …` | `xarch`，第二版 | 接口与模块 |

### 6.3 流程

XForge 的概念到 harnix-flow：

| XForge | harnix-flow |
|---|---|
| `quick` / `solid` / `major` | 流程 `xforge-quick` / `xforge-solid` / `xforge-major`；`eligibility` 由 `xforge start` 查 |
| stage + skill | 站 + 站的 `skill`（流程包里的） |
| `human: body` | 站在运行的主会话里做（`executor: {kind: session}`，不写 `scene`），人在主会话里插话 |
| `human: tail` | 站做完后的 `confirm` 出口 |
| `exit: gate` / `ledger all-resolved` / `unclaimed-changes` | `gate` 出口，host 门禁 → `xflow check …` |
| `exit: attested` / `approval` | `confirm` 出口；署名者就是确认的人，记在运行上 |
| 工作包 ready → running → integrated | `packages` 步骤列出工作包，`apply` 站 `foreach` 每包一个子 Agent 工作项 |
| `XFORGE-REPORT: done\|blocked\|needs-human` | 工作项的输出契约 `{ package, status: done\|blocked\|needs-human, summary }` |
| `rework_to` | `rework_to`；门禁不过自动返工用 `on_unmet` |
| 回执链推导当前阶段 | 运行记录就是唯一的状态 |
| 编排者 `/xforge` | Codix 的派工器 |

`xforge-solid`（示意，字段以 harnix-flow 格式 1 为准）：

```yaml
name: xforge-solid
title: 常规变更
inputs: {risk: {type: string, enum: [low, medium]}, title: {type: string}}
policies: [xforge-station]
stations:
  - id: open
    executor: {kind: program, step: xforge-open}           # 建 xforge/changes/<id>/，回 {change}
    exit: [{kind: output}]
  - id: propose
    executor: {kind: session}
    skill: xforge-propose
    output: {type: object, required: [summary], properties: {summary: {type: string}}}
    exit: [{kind: output}, {kind: gate, gate: xforge-structure}, {kind: gate, gate: xforge-spec-delta}]
  - id: design
    executor: {kind: session}
    skill: xforge-design
    output: {type: object}
    exit: [{kind: output}, {kind: gate, gate: xforge-structure},
           {kind: gate, gate: xforge-constitution}, {kind: gate, gate: xforge-ledgers}]
    rework_to: [propose]
  - id: check
    executor: {kind: agent}
    skill: xforge-check
    output: {type: object}
    exit: [{kind: output}, {kind: gate, gate: xforge-ledgers}, {kind: confirm}]
    rework_to: [design, propose]
  - id: packages
    executor: {kind: program, step: xforge-packages}       # 回 {items: [...]}
    exit: [{kind: output}]
  - id: apply
    foreach: ${stations.packages.output.items}
    max_parallel: 1
    executor: {kind: agent, agent_type: xforge-executor}
    skill: xforge-apply
    output:
      type: object
      required: [package, status]
      properties:
        package: {type: string}
        status: {enum: [done, blocked, needs-human]}
        summary: {type: string}
    exit: [{kind: output}, {kind: gate, gate: xforge-integrated},
           {kind: gate, gate: xforge-unclaimed}, {kind: gate, gate: xforge-unit-tests}]
    rework_to: [design]
  - id: verify
    executor: {kind: session}
    skill: xforge-verify
    output: {type: object}
    exit: [{kind: output}, {kind: gate, gate: xforge-structure}, {kind: confirm}]
  - id: archive
    executor: {kind: program, step: xforge-archive}
    exit: [{kind: output}]
```

- 变更 id 由 `open` 步骤定，后面的站和门禁都从 `${stations.open.output.change}` 拿，不靠模型转述。
- 工作包清单由 `packages` 步骤从 `xforge/changes/<id>/work-packages.yaml` 读，不靠 `design` 站的模型输出，两边不会对不上。
- `quick` 去掉 `design`、`check`、`packages`，`apply` 是一个会话工作项。

### 6.4 门禁

| 门禁 | 第一版 | 说明 |
|---|---|---|
| structure、spec-delta、constitution、ledgers | ✔ | 只读，读 `xforge/` 下的数据 |
| integrated、unclaimed | ✔ | 工作包都 `done`；树里的改动都落在某个包的路径里（`git status`，只读命令） |
| unit-tests | ✔ | 跑工作包登记的验证命令，按站的权限（§5.2） |
| interface-delta、interface-compat | 第二版 | 随 `xarch` |

### 6.5 护栏

- **守卫脚本**（`tool.around`）：`via` 是 `model` 时，拒绝直接写 `xforge/**`（Write、Edit、NotebookEdit、明显的 Bash 写）；
  `via` 是 `RunExt:<命令>` 且 `plugin` 是 `xforge` 的放行。守卫要同时核对命令名与包名（`runext.md` §6）。
- **站的策略** `xforge-station`：再在流程这一层拒绝写 `${workspace}/xforge/**`，允许 `RunExt` 的 `xflow` / `xspec`；`deny` 永远赢。
- 命令自己的 `writes = ["xforge/**"]` 管住它经文件工具能写到哪。

### 6.6 第一版不做

`major` 流程与 `clarify` 站、`xarch` 与两道接口门禁、XForge 的回执链与审计链（含 HMAC）、多人审批
（`min_approvers`、实现者与审批者分离、MCP 审批）、多方案并行、工作包并行（要各自的 worktree）。
运行记录已经记了谁在什么时候确认了什么，第一版以它为准。

## 7. 实施顺序

1. **依赖**（已完成）：Agix 0.4.0（2bc0b39a）已 bump，harnix-flow 0.4.7 已拷入。
2. **mock**：`/plugin` 浮层、两份披露、端口命令接 mock（脚本化插件），`crates/app/tests` 里验完。
3. **接缝**：`Hub` 的 `plugin.*`、自带目录与 `plugin-store/`、`PluginPacks`。LIVE 用一个测试用的小插件（`tests/fixtures`，带一个流程、一道门禁、一个步骤）。
4. **宿主库与 `ext.run`**：`codix.flow.*`、`PluginGates`、`PluginPrograms`。用 `ext.run` 直接跑插件命令就能测，不需要模型。
5. **xforge 第一版**：命令、`quick` 与 `solid`、§6.4 的门禁、站的 skill、守卫。命令的用例经 `ext.run` 跑，不需要模型；
   LIVE 用真实模型把 `xforge-quick` 走完一次。
6. **第二版**：§6.6。

## 8. 待定

| 问题 | 倾向 |
|---|---|
| ~~装完不自动启用~~ | 已解决：Agix 0.4 的 `plugins.enable_on_install` 缺省 false，装完披露、确认后 `plugin.enable` |
| XForge 已有项目 | `xforge/` 的格式不变就直接认；`.claude/`、`.codex/` 里的投影由人用原来的 `xforge remove` 清掉 |
| 插件命令的测试 | 插件源码里带用例：经 `ext.run` 跑，放进 Codix 的 `cargo test`（嵌入真实引擎，不联网） |
