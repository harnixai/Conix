# Conix 架构

需求见 `docs/reqs/conix-requirements.md`，交互见 `interaction.md`，测试见 `testing.md`。

## 1. 总览

Conix 采用 **"常驻实例 + 可接入的 TUI"** 两进程结构，和 tmux 的 server / client 类似，区别在于：**一个实例同一时刻只接受一个 TUI。**

```
本机（远程时就是 ssh 登录上去的那台机器）

┌──── conix（TUI 进程；接入同一实例的只能有一个）──────────────────┐
│  终端（按键 / 鼠标 / 尺寸）──► keys ──► app 事件循环              │
│                                         │                        │
│               model::update / model::view ──► tui::render        │
│                                         │                        │
│                    port::Backend  ◄─────┘   port::Instances      │
│                         │                  （列出 / 新建 / 接入） │
└─────────────────────────┼────────────────────────────────────────┘
                          │ unix socket（0600，校验对端 uid）
                          │ 消息 = port 的 Command / Reply / Event，编码为 NDJSON
┌─────────────────────────▼──── conix host（实例进程，常驻）───────┐
│  会话代理 ── Agix 客户端 ──► agix::embed（同进程，data_root = ~/.conix）│
│  流程（harnix-flow 的内核与派工器，会话经会话代理开在 Agix 上）  │
│  作业调度                                                         │
│  单一接入：占用、强制接入、空闲脱离、心跳；状态落盘在实例目录     │
└──────────────────────────────────────────────────────────────────┘
```

设计要点：

1. **TUI 只认 `port`。** 阶段 2 由 mock 实现 `port`，阶段 3 换成真实实现，界面一行都不用改。
2. **`port` 的消息就是实例协议。** `Command`、`Reply`、`Event` 都可以序列化，放到 unix socket 上就是 TUI 与实例之间的协议，不再另外设计。
3. **实例一律跑在独立进程里。** 新建实例就是拉起一个脱离终端的 `conix host` 进程，再让 TUI 接入它。退出 TUI 等于脱离，流程和作业不会因为终端关掉而停下。
4. **只有本机 socket。** 不监听任何网络端口。远程使用就是 ssh 登录后在那台机器上运行 `conix`，安全交给 ssh 和操作系统。

## 2. crate 划分

| crate | 阶段 | 职责 | 依赖 |
|---|---|---|---|
| `port` | 2 | 端口定义：`Instances` 与 `Backend` 两个 trait；`Command`、`Reply`、`Event`；领域数据类型（实例信息、会话摘要与归属、任务、goal、todo、流程、运行、站、作业……） | serde, async-trait, tokio（mpsc） |
| `tui` | 2 | 纯渲染：把 `FrameState` 画到屏幕上；`layout::compute` 是纯函数，算出各区域的矩形，同时给出鼠标命中测试用的区域表 | ratatui, crossterm, unicode-width |
| `keys` | 2 | 快捷键：解析、带上下文的绑定表、和弦状态机、用户覆盖与冲突校验 | crossterm, serde |
| `model` | 2 | view-model：`State`（领域状态 + 界面状态）、`update`、`view`；转录块的排版缓存；鼠标事件到动作的翻译 | port, tui（只用 `FrameState` 类型与 `layout`）, keys（只用动作与上下文类型） |
| `mock` | 2 | `MockInstances` 与 `ScriptedBackend`：用种子数据和场景脚本驱动 | port |
| `i18n` | 2 | 界面文字的语言资源与 `t!` 宏（§6.1） | serde_yaml |
| `app` | 2 | 二进制 `conix`：命令行解析、终端生命周期（尺寸协商、鼠标捕获）、事件循环、实例选择列表 | 以上全部 |
| `wire` | 3 | Agix 协议的 JSON-RPC 客户端与 Conix 用到的 serde 类型（§8.2）。遇到不认识的 `kind` 一律跳过 | serde, tokio |
| `host` | 3 | 实例进程：嵌入 Agix、把 Agix 帧翻译成领域事件、作业调度、socket 服务与单一接入（占用、强制接入、心跳）、实例锁与发现文件；流程（§8.6，harnix-flow） | port, wire, agix |
| `client` | 3 | `SocketInstances` 与 `SocketBackend`：扫描实例、拉起实例、通过 socket 接入 | port |

**依赖边界**（阶段 2 起用测试守住）：

- `tui` 不依赖 `port`、`model`，也不依赖 Agix。
- `model` 不做 IO、不起任务，`update` 和 `view` 都是同步的纯函数。
- `port` 不依赖任何 Agix 或 Harnix 的类型，翻译工作在 `host` 里做。
- `app` 的库不依赖 Agix，只通过 `port` 与后端打交道。`conix host` 子命令由二进制转交给 `host` crate 处理。

## 3. 端口 `port`

### 3.1 两个 trait

```rust
#[async_trait]
pub trait Instances: Send + Sync + 'static {
    async fn list(&self, all: bool) -> Result<Vec<InstanceInfo>, PortError>;
    async fn create(&self, opts: NewInstance) -> Result<Arc<dyn Backend>, PortError>;
    /// 实例已被别的 TUI 接入时返回 `PortError::Occupied { holder, idle_ms }`；
    /// `force` 为 true 则踢掉占用者再接入。
    async fn attach(&self, name: &str, opts: Attach) -> Result<Arc<dyn Backend>, PortError>;
    async fn start(&self, name: &str) -> Result<Arc<dyn Backend>, PortError>;
    async fn stop(&self, name: &str) -> Result<(), PortError>;
}

#[async_trait]
pub trait Backend: Send + Sync + 'static {
    async fn call(&self, cmd: Command) -> Result<Reply, PortError>;
    fn events(&self) -> mpsc::Receiver<Event>;   // 每个 Backend 只有一个订阅者，就是 app
}
```

- `InstanceInfo` 包含：名字、PID、主机名、项目、启动时间、版本、会话数、在等人的数量、状态（运行中 / 已停止 / 残留），以及 `holder: Option<ClientInfo>`，即当前接入者。
- `ClientInfo` 包含：用户、PID、终端（tty）、ssh 来源（取自 `SSH_CONNECTION`）、接入时刻、最后一次输入的时刻。
- `Attach` 包含：`force`，以及 `idle_timeout`（可选，覆盖配置里的值）。
- app 把每次 `call` 都 spawn 出去，结果通过 `Msg::Reply` 投回事件循环，没有哪个调用会卡住界面（N-03）。

### 3.2 命令（`Command`）

| 组 | 命令 |
|---|---|
| 连接 | `Hello{client, force}`：返回实例信息和能力位；`Heartbeat{last_input_ms}`；`Detach`；`StopInstance` |
| 会话 | `ListSessions{filter}`、`CreateSession{project, scene, model}`、`OpenSession{id}`（需要时先恢复）、`Watch{id}` / `Unwatch{id}`（已关闭的只读：读转录拼快照，不订阅）、`History{id, before}`、`Rename`、`Close`、`ListClosedSessions` / `ReopenSession{id}` / `DeleteSession{id}`（交互 §5.2「已关闭」；删除连同侧链，删了就没了） |
| 对话 | `Submit{session, text, mode: Turn \| Steer \| Queue}`、`Interrupt`、`Answer{session, prompt, answer}`、`SetModel`、`Commands{session}`、`SideQuestion{session, text}` |
| goal | `SetGoal`、`UpdateGoal{action: Pause \| Resume \| Edit \| Clear}` |
| 任务 | `StopTask`、`MessageTask`、`TaskOutput{task, from_byte}` |
| 流程 | `ListFlows`、`GetFlow{name}`、`ListRuns{flow}`、`ListEndedRuns{limit, before_ms}`（不分流程，结束新的在前）、`GetRun{id}`（带钉住的定义的摘要与它现在怎样了：`def_digest`、`def_state`）、`StartRun{flow, subject, inputs}`、`RunOp{run, op}` |
| 作业 | `ListJobs`、`SaveJob`、`DeleteJob`、`SetJobEnabled`、`RunJobNow`、`JobHistory{job}` |

### 3.3 事件（`Event`）

| 组 | 事件 |
|---|---|
| 连接 | `Connection(Connected \| Lost \| Reconnecting \| Resynced)`、`Kicked{by: ClientInfo, at}`（被强制接入的一方踢掉）、`IdleDetached{after_ms}` |
| 会话列表 | `SessionUpserted(SessionSummary)`、`SessionRemoved(id)` |
| 一轮（仅限被 Watch 的会话） | `TurnStarted`、`StepRestarted{step}`、`TextDelta`、`ThinkingDelta`、`ToolStarted`、`ToolInput`、`ToolProgress`、`ToolFinished`、`TurnEnded{stop, usage}`、`TurnFailed` |
| 插话 | `SteerDelivered`、`SteerReturned{messages}` |
| 提问 | `PromptPending(Prompt)`、`PromptResolved{id, resolution}` |
| 会话状态 | `GoalChanged`、`TodosChanged`、`Compacted`、`CommandsChanged`、`Notice{level, text}` |
| 任务 | `TaskUpserted(Task)`、`TaskOutput{task, bytes, from, next}` |
| 流程 | `RunUpserted(RunSummary)`、`RunDetail(RunView)` |
| 作业 | `JobUpserted(Job)`、`JobFired{job, entry}` |

**追赶由后端负责。** model 看到的永远是"先一个快照（`Reply`），再接着增量"。重连成功后，后端先推 `Connection(Resynced)`，再推一个新的快照。

### 3.4 数据类型

- id 一律用不透明的字符串新类型。时间一律是毫秒时间戳。用量统一为 `Usage{input, output, cache_read, cache_write}`。
- **会话归属** `Provenance`：`instance`、`host`、`project`、`scene`、`model`、`origin`。其中 `origin` 取值为 `User | Job{job} | FlowStation{run, station} | SubAgent{parent, task} | Fork{from}`（C-12）。
- 任务的 `kind`、站和运行的状态都是枚举，并带一个 `Unknown(String)` 兜底。站和运行的状态与 Harnix 保持一致。

## 4. view-model `model`

### 4.1 状态

```
State
├── domain        后端事实，只由 Event / Reply 写入
│   ├── instance      当前接入的实例信息、能力位
│   ├── sessions      会话摘要与归属；被 Watch 的会话另有 Transcript / AskQueue / Tasks / Goal / Todos
│   ├── flows / runs  流程定义、运行摘要、选中运行的详情
│   ├── jobs          作业与触发历史
│   └── conn          连接状态
└── ui            界面状态，只由输入（按键、鼠标）和 Tick 写入
    ├── screen        实例选择 / 工作台 / 已脱离（被踢、空闲超时）
    ├── layout        并排或单栏、两栏比例、右栏是否收起、当前显示哪一栏（单栏时）
    ├── focus         Composer / Transcript / Side(Upper | Lower) / Dialog
    ├── context       对话区的上下文栈（面包屑）
    ├── tabs          当前标签；每个标签各自的展开、选中、滚动状态
    ├── composer      草稿、光标、历史、补全
    ├── transcript    每个上下文各自的滚动位置、选中块、折叠集合、拖选范围
    ├── mouse         是否捕获鼠标、正在拖动的对象（分隔线 / 选区）
    └── idle          最后一次输入的时刻；空闲超时的时长
```

**`FrameState` 只有 `view` 一个写者。**

### 4.2 `update` / `view`

- `update(&mut State, Msg) -> Vec<Effect>`
  - `Msg`：`Action`、`Text`、`Paste`、`Mouse{kind, col, row}`、`Event`、`Reply`、`Tick`、`Resize`
  - `Effect`：`Call(Command)`、`Instances(op)`、`Quit{detach}`、`Clipboard(String)`、`SetMouseCapture(bool)`、`Bell`
- `view(&State, Size) -> FrameState`：先调用 `tui::layout::compute`，拿到各区域的矩形，再分别投影。
- **鼠标命中测试**：`layout::compute` 同时返回区域表（矩形 → 区域路径）。model 记住上一帧每一行对应什么（列表项、块头、选项、分隔线）。鼠标事件按"区域 + 行"找到目标，再翻译成和键盘相同的动作。鼠标没有任何独立的逻辑分支，所以键盘能做的鼠标都能做，测试也只需要覆盖一份动作逻辑。

### 4.3 转录：一条 entry 就是屏幕上的一行

- 转录由**块**组成，每个块有稳定的 id。每个块缓存它在当前宽度下排好的行，排版在 model 里完成：按显示宽度折行，按 `\n` 分行，CJK 和 emoji 按实际宽度计算。**渲染端不做折行。**
- 流式增量只把它所在的那个块标脏，与转录长度无关（N-02）。
- 行上带 `block_id` 和 `first_of_block`，后者只有一种语义：这一行是块的第一行。凡是"按块还原"的逻辑，都只能调用 `model` 里的同一个辅助函数。
- 拖选复制（M-07）：按"块 + 行 + 列"记录选区，复制时从块的原文中取，不从排版后的行拼。这样折行处不会混进多余的换行。

### 4.4 选中即切换与顶部面板

- 每次 `update` 之后（`follow::after_update`）：先把右栏每个标签两半的选中挪回它指着的对象
  （`ui.anchors` 记着行号与那一行的身份；行号被显式改过就照新的认）；右栏当前看得见的东西缺数据
  就去取（运行列表、流程定义、运行详情、作业历史、被选中会话的快照）；焦点在右栏、选中的对象变了
  且它带会话，就让对话区显示它；对话区从别处换了会话，在当前标签里看得见就把选中挪到它。
- 流程定义与作业没有会话（用户会话是一级公民，改它们在用户会话里说）：选中它们左边不动，流程设计、
  作业的执行情况画在右下。对话区顶部面板（`panel.rs`）每帧按当前上下文从领域数据现算：运行里的
  会话 → `RunView`，其余 → goal / todo。它不进转录，所以数据变了不用改块、不触发重排。内容最多
  10 行，滚动位置连同上下文一起记，上下文一换就归零。
- 右上三个列表的顺序与 `#n` 序号来自 `port::order`；实例侧的工具解析 `#n` 用同一组函数。
- 状态符号与颜色只在 `marks.rs` 定义一次，三个标签都从这里取。
- 新建、修改、清理流程与作业走对话；阶段 3 由引擎侧的流程、作业工具执行，界面不另开表单。

### 4.5 待人处理

`model` 维护一个派生列表："等你处理"的事项。来源有：各会话中在等人的项、搁置或待确认的站、受阻的 goal、失败的作业。标签角标和"跳到下一处"都读这一份列表。

## 5. 事件循环与终端 `app`

```
loop {
    select! {
        input = terminal.next()      => msgs.push(translate(input, &state)),  // 按键 / 鼠标 / 尺寸 / 粘贴
        ev    = backend_events.recv() => msgs.push(Msg::Event(ev)),
        reply = replies.recv()        => msgs.push(Msg::Reply(reply)),
        _     = ticker, if animating  => msgs.push(Msg::Tick),
    }
    while let Ok(m) = drain() { effects += update(&mut state, m) }
    run(effects);
    if dirty && frame_budget_ok { terminal.draw(view(&state, size)) }
}
```

- **合帧**：每轮循环先把这一刻已经到达的消息全部处理完，最多画一帧，帧间隔下限约 16ms。
- **Tick 只在有动画时开启**（比如 spinner、计时器）。

**终端生命周期**集中在一处，按下面的顺序进入，退出时按相反顺序恢复；panic 时也要先恢复终端：

| 步骤 | 内容 |
|---|---|
| 1 | 尺寸协商（L-06）：没有在 tmux 或 screen 里、且配置没有关掉时，先记下当前尺寸，再发送 `CSI 8 ; 行 ; 列 t` 请求缺省尺寸，最多等 300ms 的 Resize 事件。等不到就按实际尺寸继续。退出时如果改过尺寸，再请求恢复到原来的尺寸 |
| 2 | raw mode、备用屏 |
| 3 | 鼠标捕获（按下与松开、拖动、滚轮）。可以在运行中切换（M-08） |
| 4 | bracketed paste |
| 5 | 键盘增强协议（kitty），终端支持时才开启，用来区分 Shift+Enter 等组合键 |

**剪贴板**：写剪贴板用 OSC 52，ssh 上也能用。tmux 里需要打开 `set-clipboard`，这一点写进手册。

**命令行**：

| 命令 | 行为 |
|---|---|
| `conix` | 当前目录有在跑的实例就直接接入；没有而本机有别的实例，先显示实例选择列表；都没有就为当前目录新建一个并接入 |
| `conix new [--name] [--project]` | 新建实例并接入 |
| `conix list [--all]` | 用纯文本列出实例，不进入 TUI |
| `conix attach <名字> [--force] [--idle-timeout <时长>]` | 接入指定实例；`--force` 踢掉当前接入者 |
| `conix start <名字>` / `stop <名字>` | 启动 / 停止指定实例 |
| `conix --mock [场景]` | 用 mock 后端：缺省打开演示 `demo`，也可以指定测试场景（m01–m25 等） |
| `conix host --name <名字>` | 内部使用：实例进程的入口，由 `new` / `start` 拉起，用户不直接调用 |

## 6. 快捷键 `keys`

- 绑定的形式是 `(上下文, 按键或和弦) → 动作`。上下文有：`global`、`composer`、`transcript`、`sessions`、`flows`、`jobs`、`dialog`、`picker`、`instances`。
- 查找顺序：先查当前上下文，再查 `global`。同一个上下文里，同一个键只能绑定一个动作；有冲突时在加载阶段就报错。
- 在 `composer` 上下文里，普通字符一律作为输入；字母快捷键只在列表和对话框里生效。
- 和弦超时 1.5 秒。用户的覆盖文件放在 `~/.conix/keybindings.json`：一个数组，每条
  `{"context": "jobs", "keys": "x", "action": "delete"}`（`keys` 是按键或空格隔开的和弦，`action` 为 `null` 是解绑）。
  逐条叠在内置键位上，同一上下文的同一个键覆盖的替掉内置的；跟别的绑定冲突的那条不生效，原因在 `/doctor` 里。
- **底栏的保底提示**：不论处在哪个上下文，底栏的最后两项永远是"切换栏位"和"帮助"（L-07）。这两项由 `keys` 从绑定表里取出实际的键位，用户改了键位，底栏也跟着变。

### 6.1 多语言 `i18n`

- 文字放在 `crates/i18n/locales/<语言>/<模块>.yaml`，键是 YAML 路径拼起来的 `模块.名字`，
  代码里用 `t!("panel.collapse")`、`t!("flows.progress", done = 3, total = 7)` 取；`{名字}` 换成参数。
  整句进资源、不拼词，好让别的语言调整语序。
- `build.rs` 把 `locales/` 下每个目录编进二进制：加内置语言就是加一个目录。启动时再叠加
  `$CONIX_HOME/locales/<语言>/*.yaml`，用户不重编也能加语言或改个别说法。
- `zh` 是参照：每个键都有；其它语言缺键时退回中文，再缺就显示键名。测试守住两件事：每种内置
  语言的键与占位和中文一一对应；model / tui / keys / app 的源码里（注释与测试之外）不再写死中文。
- 当前语言是全局的（启动时 `i18n::init` 一次）。没调用时是中文，所以测试与快照照旧用中文写；
  英文的测试放在单独的测试文件里（单独的进程）。
- 不翻译的：会话内容、流程 / 作业 / 会话的名字这类数据；状态符号（● ✓ ◐ …）本来就不是文字。

## 7. mock `mock`

见 `testing.md`。`MockInstances` 用种子数据提供几个"在跑"的实例和几个"已停止"的实例，`create` 和 `attach` 返回一个 `ScriptedBackend`。阶段 2 不会真的起任何进程。

## 8. 阶段 3：实例

### 8.1 数据目录

**项目级的放项目，全局的放全局，本机的、过程性的不进 git。** 一个实例对应一个项目，会话、流程、作业都是
项目的；全局目录只留用户级的配置、实例的运行时文件与发现，以及认领过的项目的登记。

```
~/.conix/                         CONIX_HOME 指定时以它为准；同时也是 Agix 的 data_root
├── settings.json                 ┐
├── sessions/<id>/  memory/       │ Agix 的全局数据，布局归 Agix（ARCHITECTURE.md §8）。sessions/<id>/ 下
├── scenes/  daemon/  plugins/ …  ┘ 对项目里的会话只有索引（§8.4）。模型、provider、权限都沿用 settings.json
├── conix.yaml                    Conix 自己的配置（YAML）：界面、实例（空闲超时）；不存在时写一份带注释的缺省配置
├── keybindings.json              快捷键覆盖
├── flows/                        全局流程包（可写层），所有实例共用：flows/、skills/、rules/、policies/…
├── known-projects.json           认领过的项目：凭证 → 项目路径（§8.9）
└── instances/<名字>/             目录 0700；只放这台机器上这个进程的东西
    ├── instance.json             发现文件：名字、pid、项目、启动时刻、版本、当前接入者；实例退出时删掉
    ├── meta.json                 实例自己的设定（项目）；停了也留着，`conix start` 照它重新拉起
    ├── instance.lock             flock：同名实例只能跑一个；拿不到锁的 `conix host` 立即退出
    ├── host.sock                 0600
    ├── groups.json               Agix 的 groups_file，回收上一次没收尾的进程组
    └── host.log                  实例进程的日志（stderr 也重定向到这里）

<项目>/                           实例必须有项目
├── .wkts/<运行>/                 `workspace: worktree` 的运行的 worktree，分支 `conix/flow/<运行>`；
│                                 和引擎自己的 worktree 同一个目录（在仓库根），`.wkts/.gitignore` 是 `*`
└── .conix/                       项目层目录（Agix 的 project_dir）
    ├── .gitignore                Agix 维护 state/、settings.local.json、mcp.local.json
    ├── settings.json  mcp.json   ┐
    ├── agents/  skills/  rules/  │ 进 git：项目的引擎配置，布局归 Agix
    ├── hooks/  commands/ …       ┘
    ├── settings.local.json …     个人配置，不进 git
    ├── flows/                    进 git：项目自己的流程包（项目受信才读、才写，§8.6）
    ├── jobs.json                 进 git：作业定义（项目受信才读、才写，§8.5）
    └── state/                    不进 git：本机数据与过程数据
        ├── history/<id>/         Agix：受信项目的会话（转录、memory、checkpoints）
        ├── memory/  scratchpad/  Agix：模型不问就能写的只有这两个
        └── conix/                Conix 的命名空间（Agix 的 `state/<产品名>/`），模型不能写
            ├── owner.json        认领凭证（§8.9）
            ├── host.lock         flock：一个项目同时只能有一个实例
            ├── sessions.json     会话归属（§8.4）
            ├── jobs-state.json   作业在这台机器上的启用状态与调度基线
            ├── jobs/<作业 id>.jsonl  触发记录，一次一行
            ├── flow-runs/        流程运行的记录（harnix-flow 的 FileStore）
            ├── run-policies.json 流程作业起的运行：按运行记下发起它的作业与开站会话用的网络（§8.5），续跑时沿用
            ├── runs/<运行>/      运行的过程目录（`ProjectLayout::with_runs_dir`，Harnix#69）：交付、门禁日志
            ├── workspaces/<运行>/ 没有项目的运行的工作目录（流程库要它；Conix 的运行都有项目）
            └── quarantine/<时刻>/ 认不出来的状态挪到这里，不读（§8.9）
```

- Conix 自己的文件和 Agix 的放在同一个目录，用自己的名字，不和 Agix 的顶层名字冲突；**不往 Agix 的
  `settings.json` 里写 Conix 的配置**。
- 项目层目录：`EmbedConfig.project_dir = ".conix"`（Agix#86）。项目里原有的 `.agix` 不再被读，要沿用其中的
  设置由用户自己搬。`product = "Conix"`：提示词、`User-Agent`、worktree 分支前缀、`state/conix/` 都用这个名字。
- **运行时文件留在全局**：`conix list` 要有一个地方扫得到所有实例；macOS 的 unix socket 路径不能超过 104 字节，
  放进项目，项目一深就绑不上。
- **流程的 worktree 不放在 `.conix/` 下**：那是引擎守着的配置目录，路径里有它，模型在 worktree 里写文件就会被拦。
  `.wkts/` 下只能有一层（`.wkts/<运行>`）：Agix 把 `.wkts/` 下第一层认作一个 worktree，据此把保护映射回主仓库。
- **作业属于项目**：一个项目同时只有一个实例（`state/conix/host.lock`），不会重复触发同一个作业。
  `conix start <名字>` 重启实例时作业接着排。
- 以前放在 `instances/<名字>/` 下的会话归属、作业、流程运行与 worktree 不迁移。

### 8.2 crate 与进程

```
conix（一个二进制）
├── conix [new|list|attach|start|stop]   TUI：app ─► client::SocketInstances ─► port
└── conix host --name N --project P     实例：app 把参数交给 host::run
                                          host ─► wire（JSON-RPC 客户端）─► agix::embed
```

| crate | 职责 |
|---|---|
| `wire` | Agix 协议的 JSON-RPC 客户端：NDJSON 分帧、按 `id` 配对回复、通知（`session.event` / `daemon.event`）进一个 channel；只放 Conix 用到的那几个参数与结果的 serde 类型，帧载荷按 `kind` 分派，不认识的跳过 |
| `host` | 实例进程：实例目录、锁、发现文件；`Engine`（嵌入的 Agix）；`Hub`（端口命令的实现、帧到领域事件的翻译、会话状态）；作业调度；socket 服务与单一接入 |
| `client` | `SocketInstances` 与 `SocketBackend`：扫实例目录、拉起实例、接入、断线重连、`stop` 发 SIGTERM |

- `Hub` 只经 `Engine` trait 用引擎（`call(method, params) -> Value` 加一条通知流）。真实实现是嵌入的
  Agix；测试用脚本化的假引擎，不联网、不起 Agix，就能把命令、帧翻译、作业、接入流程跑通。
- `app` 的库部分（`Core`、model 的驱动）仍不依赖 Agix；二进制依赖 `host` 与 `client`。
- `--mock <场景>` 保留：仍走 `mock`，测试与演示用。

### 8.3 TUI 与实例之间：链路

- 传输：`instances/<名>/host.sock`，NDJSON。每行是 `port::link` 里的一帧：
  - TUI → 实例：`{"id": n, "cmd": Command}`
  - 实例 → TUI：`{"id": n, "reply": {"ok": Reply} | {"err": PortError}}`，或 `{"event": Event}`
  - 回复与事件同一条有序的流：等到一个回复时，排在它前面的事件已经到了（model 的"先快照再增量"靠这一条）。
- **握手**：第一帧必须是 `Hello{client, force}`。实例空着 → 回 `Hello{instance, flows, jobs}`；
  有人接入且不带 `force` → 回 `Err(Occupied{holder, idle_ms})` 并关掉这条连接；带 `force` → 先给旧连接推
  `Kicked{by, at_ms}` 再关掉它，然后接受新连接。被踢的一方不重连。
- 对端校验：accept 后取对端 uid（`peer_cred`），与实例进程的不同就直接关掉。
- **心跳**：model 每 10 秒发 `Heartbeat{last_input_ms}`；实例 35 秒没收到任何帧就断开这条连接（TUI 僵死、
  ssh 半开）。空闲脱离由 TUI 判断（model 已有）；实例按 `last_input_ms` 兜底：超过 `idle_timeout`
  推 `IdleDetached` 后断开。
- **断线重连**（`SocketBackend`）：连接意外断开 → 推 `Connection(Lost)`，按 0.5s、1s、2s… 重连（推
  `Reconnecting{attempt}`），以不带 `force` 的 `Hello` 接回；成功 → `Connection(Resynced)`，model 重新
  拉列表与当前会话的快照。实例进程已不在（发现文件的 pid 死了）或 30 秒没接上 → `InstanceStopped`。
- 脱离（`Detach`、退出 TUI、被踢、空闲）不影响实例里正在进行的工作。`StopInstance` 回 `Ok` 之后实例开始关停。

### 8.4 Agix 接入：`Hub`

- 实例进程里起一个 `Embedded`：`data_root = ~/.conix`、`cwd = 项目`、`project_dir = .conix`、
  `product = Conix`、`groups_file = instances/<名>/groups.json`，**不传 `instance`**（Agix 的实例锁
  按"数据根 + 场景"算，一个场景只能锁一份；实例锁由 Conix 自己管）。经 `serve_stream` + `duplex`
  连上，`daemon.subscribeEvents` 订阅会话的生灭与轮的起止。
- **实例是这个引擎唯一的客户端。** 它订阅内存里的每一个会话（`daemon.event` 的 `session_created` /
  `session_resumed` 时订阅），所以每个会话的状态、任务、goal、todo、待答提问都由 `Hub` 记着，不依赖 TUI
  在不在。翻译出的领域事件分两类推给 TUI：
  - 摘要类（`SessionUpserted`、`TaskUpserted`、`JobUpserted` …）全推；
  - 一轮的细节（文字增量、工具、提问……）只推 TUI `Watch` 着的会话；子 Agent 的侧链会话被 Watch 时，
    父会话上的 `subagent_progress` 解包成那个侧链会话的事件。
- **只列这个项目的会话**：`session.list` 的条目按 `project_root == 实例的项目` 过滤。
- **会话放在项目里**（Agix#94）：项目受信时，会话的转录与附属数据在 `.conix/state/history/<id>/`，全局只留索引；
  不受信时照旧在全局。Agix 只读索引登记过的会话，项目失去信任后其中的会话不读（`PROJECT_UNTRUSTED`）。
- **会话归属**记在 `state/conix/sessions.json`：Conix 建的会话记下 `origin`（用户、作业的哪次触发、分叉）
  与建会话的时刻（会话列表的顺序、`#n` 按它）；没记过的按 `User`，时刻取引擎报的。子 Agent 的上级来自任务的
  `agent_session_id`。
- 建会话一律带 `prompt_types: ["permission", "question"]`，项目根就是实例的项目。会话的其余选项（模式、ask / deny 规则、
  `model.effort`、`prompt.append`、`sandbox.network.mode`、`session.token_budget`）进 `options.settings`（`hub::session_opts`），空的不写；
  0.4 不再收 `options` 顶层的 `permission_mode` / `permission_rules` / `effort` / `prompt`（传了就 `INVALID_PARAMS`）。
  `session.create` 回的 `clamped_options` 记日志。

帧、方法与领域事件、命令的对应：

| Agix | 领域事件 / 命令 |
|---|---|
| `turn_start` / 同一 `step` 再来的 `step_start` / `model_retry` | `TurnStarted` / `StepRestarted` / `ModelRetry` |
| `text_delta` / `thinking_delta` | `TextDelta` / `ThinkingDelta` |
| `tool_use_start` / `tool_use` / `tool_progress` / `tool_result` | `ToolStarted` / `ToolInput`（按工具名摘要参数）/ `ToolProgress` / `ToolFinished` |
| 工具名为 `TodoWrite` 的 `tool_use` | 另推 `TodosChanged` |
| `prompt` / `prompt_resolved` | `PromptPending` / `PromptResolved`，会话状态随之在 Waiting 与 Running 间变 |
| `turn_complete` / `error` 帧或 `run_turn` 的错误响应 | `TurnEnded` / `TurnFailed` |
| `user_message_delivered` / `user_messages_returned` | `SteerDelivered` / `SteerReturned` |
| `task_started` / `task_updated` / `task_finished` / `task_progress` | `TaskUpserted` / `TaskOutput` |
| `goal_changed` / `compact` / `skills_changed` / `notification` | `GoalChanged` / `Compacted` / `CommandsChanged` / `Notice` |
| `host_tool_call` | 由 `Hub` 自己执行（宿主工具，§8.5），`session.respondToolCall` 作答 |
| `daemon.event` 的 `turn_started` / `turn_completed` / `session_*` | `SessionUpserted` / `SessionRemoved` |
| `Submit{mode}` | `session.run_turn{on_busy}`：`run_turn` 是长调用，`Hub` 另起任务等它，命令立刻回 `Submitted` |
| `Watch` | `session.subscribe{resume: true}` → `session.history`（最后 500 条）→ `session.goal` / `session.tasks`，拼成 `Watched` |
| `Interrupt` / `Answer` / `SetModel` / `Rename` / `Close` | `session.interrupt` / `session.respondToPrompt` / `session.setModel` / `session.rename` / `session.close` |
| `SetPermissionMode` / `permission_mode_changed` 帧 | `session.setPermissionMode`（回实际生效的模式与是否被夹）；帧（宿主切的、模型 `EnterPlanMode` / `ExitPlanMode` 切的）改会话摘要的 `permission`。订阅之后用 `session.resume` 读一次 `permission.mode`：重建、恢复、设置夹紧都不推帧。`CreateSession` 带的模式进 `options.settings.permissions.mode`（0.4 删了 `options.permission_mode`） |
| `SetGoal` / `UpdateGoal` | `session.setGoal` / `session.updateGoal` |
| `SideQuestion` | `session.fork`，再在分叉出的会话上 `run_turn`（归属记为 `Fork`） |
| `StopTask` / `MessageTask` / `TaskOutput` | `session.stopTask` / `session.messageTask` / `session.taskOutput`（任务所属的会话由 `Hub` 记着） |
| `Commands` | `commands.list` |

**接入时的几件事**：

- **协议版本**：`Hub::start` 先 `daemon.ping`，`protocol_version` 不是 5、或缺 `session_settings` / `config_explain` 能力位，
  就拒起实例并写明要 Agix 0.4。不按"大于等于"放过：0.4 对旧写法一律拒，版本不同就是不同的协议，这里不说清，
  后面每个调用都会以 `INVALID_PARAMS` 之类的面目失败。
- **cli 层**（`EmbedConfig.settings`，由 `host::hub::cli_settings` 给）：`permissions.allow` = 只读工具 + 流程自己的工具
  （`flow_state` / `flow_submit` / `flow_propose`），`permissions.client_may_loosen: true`，另加 `app.history: "project"`
  （0.4 删了 `EmbedConfig.history`）。为什么要在这一层写：0.4 的会话层（`options.settings`）只能收紧，比设置宽的值被夹回，
  会话选项里的 allow 规则被扣下，除非 `client_may_loosen`；流程站的 `acceptEdits` 比项目缺省宽，没有它就被夹回、无人看着的运行会停在权限提问上。
  协议把 `client_may_loosen` 叫安全缺省而不是安全边界；部署方的 managed 层仍压过它，能把它关掉，所以 `/doctor` 的
  `conix.session_loosen` 按 `config.explain` 报实际值，关了报 degraded。
- **缺省模型**：`providers` 是列表（0.4 没有 `model.model_name`）；缺省取 `model.order` 第一个 provider 的 balanced 档，
  没写取 strong 档。没定义 `providers` 时引擎用自带的 anthropic，其模型是进程参数、设置里读不到，由 `conix host` 读
  `CONIX_MODEL` 交给引擎（`EngineConfig.model`）。

### 8.5 作业

- 调度器在实例进程里，按本地时区解释 5 段 cron。实例没在跑时错过的触发，`catch_up` 为真就在启动后补一次
  （只补一次，不按错过的次数补）。
- 简单作业一次触发：新建一个会话（归属 `Job{job, name, fired_ms}`），把提示词作为第一轮提交；作业里列了
  skills 的，附加进这个会话的系统提示词。触发记录先写 `Running`，那一轮结束时补上结局、耗时、用量。
- 流程作业：启动一次运行（触发方式记为 `Schedule`），触发记录指着那次运行；运行自己有结局，触发记录只记启动成没成。
- 作业没有自己的会话：新建、修改、清理都在用户会话里说，由宿主工具执行（下面）。界面不另开表单（§4.4）。
- **定义与本机状态分开**：
  - 定义（名字、cron、内容、`catch_up`、创建时刻）在 `.conix/jobs.json`，随项目进 git，团队共用。
  - 启用与否、调度基线（上次触发、恢复启用的时刻）在 `state/conix/jobs-state.json`，只属于这台机器。
    没有记录的作业是停用的：克隆下来的仓库带着作业，不会拉下代码就在别人的机器上无人值守地跑起来。
    在这台机器上新建的作业缺省启用。
  - **项目受信才读、才写定义**，与项目的流程包一样（§8.6）：不受信时作业标签是空的，`job_save` 照实说存不进去；
    `/trust` 之后重新读，作业跟着出现或消失（`JobUpserted` / `JobRemoved`）。
- 作业 ID 是随机的短 ID（`j` 加 8 位十六进制）：定义进 git，几个人各自新建，按序号编的 ID 合并时会撞。
  列表按创建时刻排（`port::order::jobs`），`#n` 照旧。

**护栏**（无人值守 = cron × 模型 × 没人看着）。Agix 已经做的不重复：`attended:false` 的会话权限询问直接拒绝、
设置里一轮的调用次数与 token 上限、询问超时按拒绝。Conix 加三层上限，下层只能更紧，外加一项网络：

```yaml
# ~/.conix/conix.yaml
jobs:
  defaults: {timeout: 30m, max_tokens: 2M, overlap: skip, disable_after: 3, network: deny_all}   # 作业没写时用
  limits:   {timeout: 4h,  max_tokens: 20M}                                   # 天花板
  daily_tokens: 50M      # 这个实例所有无人值守的工作一天合计；0 = 不限
  max_concurrent: 2      # 同时在跑的触发
```

作业定义里的 `guard`（随项目提交）与天花板取较小值：仓库里的作业放不宽这台机器的上限。执行（`host::jobs::guard`）：

| 护栏 | 怎么执行 | 结局 |
|---|---|---|
| 超时 | 触发时起计时器 | 简单作业 `session.interrupt`；流程作业**暂停**它起的运行（工作留着、进待处理）。`Stopped{timeout}` |
| 单次 token | `Hub::tap` 看着触发出来的会话：`step_end.usage` 加子 Agent 的 `task_updated.usage` 累计；流程作业按运行里所有会话累计 | 同上，`Stopped{budget}` |
| 重叠 | 到期时这个作业还有 `Running` 的触发：`skip` 记 `Skipped{overlap}`；`queue` 至多排一个；`replace` 先停旧的 | |
| 连续失败 | 连着 `disable_after` 次 `Failed` / `Stopped`：在本机状态里停用，记 `auto_disabled` | 进待处理；启用时清掉 |
| 每日额度 | `state/conix/usage/<YYYYMMDD>.json` 按作业累计 | 超了在跑的停下、当天剩下的 `Skipped{daily_budget}` |
| 并发 | 在跑的触发数到 `max_concurrent` 就排队，排过一个周期 `Skipped` | |

手动从流程页启动的运行不受作业护栏管（有人在）。

- **用量口径**（`port::Usage::billed`）：未命中缓存的输入 + 缓存写入 + 输出，缓存命中不计，与 Agix 的预算、配额一致。
  单次上限、每日额度、作业页与 `job_history` 都用它；会话页、流程页的 token 仍是全部。作业页的旧记录不改写，口径与新记录不同。
  计数平时按 `step_end` 与后台任务的累计估，一轮结束用 `turn_complete.usage`（0.4 起含子 Agent、团队成员与旁路调用）校准，只增不减。
- **单次 token 交给引擎**：简单作业的 `max_tokens` 下发为会话的 `session.token_budget`，引擎在每次调用前按剩余拦，
  不会像事后数帧那样冲过头；一轮以 `budget_exceeded` 结束就记 `Stopped{budget}`，与 Conix 自己数到超额一样。
  一次触发就是一个新会话，会话预算即这次触发的预算。流程作业仍按运行累计。
- **网络 `network`**：`deny_all`（缺省）| `allowlist` | `inherit`，可写在 `jobs.defaults` 与作业定义里，没有天花板
  （放开网络是作业作者与这台机器的用户各自的决定）。缺省是 `deny_all` 的原因：Agix 0.4 的
  `sandbox.unattended_needs_network_policy` 缺省为真，无人值守的会话里没有 allow 规则的命令要沙箱兜得住**且**网络受限才执行，
  而会话的网络缺省不受限，升级后作业里的命令大多会被拒。简单作业写进会话的 `sandbox.network.mode`；`inherit` 下 Agix 那条开关照样守着，
  `job_validate` 给一条 notes 说清命令会被拒、怎么写 allow 规则；`/doctor` 的 `conix.jobs_network` 对缺省 `inherit` 报 degraded。
- **禁 `CronCreate`**：作业会话与作业起的运行的站会话 deny 它；模型在无人值守时自建定时任务，就在作业与护栏之外了。
- **流程作业的网络按运行记一次**（`state/conix/run-policies.json`），在第一次开站会话时按作业当时的护栏取并快照：引擎按
  `session_id` 认会话，同一个键再开而 options 不同就拒绝，现查作业的话，改过护栏后续跑的站就开不出来。
  取在第一次开站而不是启动运行时，免得与派工器开第一个会话抢先后。

**宿主工具**（`hub/tools.rs`）：`session_list`、`session_show`、`session_rename`、`session_close`、`session_delete`、
`job_list`、`job_show`、`job_save`、`job_validate`、`job_history`、`job_stop`、`job_delete`、`job_enable`、`job_run`
（作业一套对齐流程：`job_save` 收 `kind: simple | flow`，流程作业的 `flow` 收 `#n` 或名字、存前校验流程在、`inputs` 合它的
schema；`job_validate` 不落盘，回下 5 次触发时刻与护栏的实际生效值；`job_history` 翻页、按结局筛；`job_stop` 停下正在跑的那次；
`job_run` 可以这一次换主题或补一句）；流程的 22 个 `flow_*` 是流程库的编写与
运行工具（`harnix_flow::author`），声明与执行都在库里，`flows/tools.rs` 只在外面加 Conix 的三样：`flow` 参数收
`#n`（换成名字再交给库）、按会话限作用域、有运行不许改。库要宿主答的（`AuthorHost`）：会话在实例的项目里；
会话刚建的流程是它的"当前流程"，运行里的会话的当前流程与当前运行是它自己那个——别的会话没有"当前"，
作用于流程或运行的工具要给出目标。

- 经 `daemon.setHostTools` 挂在实例与引擎的连接上，**每个会话都有**，子 Agent 也有。只对此后建起来的
  引擎生效：老会话在下一次恢复或重建时拿到。
- **能动什么由发起调用的会话决定**，`Hub` 执行前查（子 Agent 的调用以根会话回答，算根会话的）：
  用户会话（新建、分叉）什么都能动；作业触发出来的会话只能动触发它的那个作业和它自己；流程运行里的
  会话只能看流程、只能操作它自己所在的运行（不能改定义、不能启新运行），会话只动它自己；作业触发出来的
  会话对流程只能看；不是本实例的会话什么都不给。提示词只说明，不当约束。
- **有运行不许改**：`flow_update` / `flow_delete` 在这个流程还有没结束（`Active`）的运行时拒绝，回答里列出
  那些运行的 ID，要先取消或等它们跑完。运行启动时库会把定义钉住，改定义本不会弄坏在跑的运行；这是 Conix
  自己的规矩，免得"改了的流程"和"正在跑的流程"对不上。
- 目标用 `#n` 或 ID：`#n` 是右栏列表此刻的位置，两边都用 `port::order` 排，所以说的是同一个；
  改东西的工具回显名字与 ID，号对错了一眼看得出。
- 查看类的工具建会话时给 allow 规则，不用每次问；改东西的走 Agix 正常的权限询问。
- 执行走和端口命令同一条路（`dispatch`），所以 TUI 照常收到 `JobUpserted`、`SessionUpserted` 这些事件。

### 8.6 流程

流程用 Harnix 拆出来的库 `harnix-flow`（`3rds/harnix-flow/`，原样拷入，`scripts/fetch-3rds.sh`）：内核、
存储、流程包、派工器（runner）都是库的，实例只做装配与宿主的那一半（`host/src/flows/`）。

- **装配**（`flows::start`，实例起来时一次）：
  - 运行记录：`FileStore`，`.conix/state/conix/flow-runs/`。
  - 流程包三层：`~/.conix/flows/`（`DirPacks` 的可写层）；垫在下面的内置包（库的示例包，`MemPacks::examples()`）；
    项目自己的 `<项目>/.conix/flows/`，**项目受信才读、才写**。会话都在实例的项目里，`flow_create` 存进项目自己的
    流程包，所以项目没受信时存不进去，工具照实说。信任取 Agix 的 `project.status`；人用 `/trust`（端口
    `TrustProject` → `project.trust`）信任或取消，之后派工器重新问一遍，项目的流程跟着出现或消失。
  - `ProjectLayout`：项目层目录 `.conix`（与 Agix 的同一个），worktree 在仓库根的 `.wkts/<运行>/`、分支
    `conix/flow/<运行>`（§8.1），任务会话可以当自己的来写的子目录 `state/scratchpad`、`state/memory`
    （Agix#93：模型不问就能写的只有这两个）。
  - 场景（定义里的 profile）就是 Agix 的场景名，目前只开 `code`，交付走 `flow_submit`。
- **会话宿主**（`SessionHost`，`flows/session_host.rs`）经 `Hub` 开会话，`Hub` 仍是引擎唯一的客户端：
  - `open`：会话 id 由派工器的键算出（键的 v5 UUID，照引擎的写法写成 BASE58），`session.create` 带
    `session_id`——同一个键再开，实例重启之后也一样，是同一个会话；`options` 只由 spec 定（权限模式
    `acceptEdits`、spec 的 `asks` 变成 `ask` 规则、模型、effort、`output_schema`、流程自己的三个工具
    `flow_state` / `flow_submit` / `flow_propose` 作为这个会话的 `host_tools`）。归属记作
    `Origin::FlowStation`，主会话的站记作 `@main`。实例只有一个，站里写的 `instance` / `template` /
    `space` 拒开。
  - `watch`：`Hub` 把看着的会话的帧原样再分一路（`Hub::tap`），翻成派工器的 `Frame`（`flows/frames.rs`）：
    一轮的开始带起它的那句话（`Hub` 记着经 `run_turn` 发出的每一句，按轮 id 认，认不出取最早的一句）；
    `run_turn` 回的错误也补成那一轮的开始与结束，**每一轮都结束**；只有权限问句给派工器答，模型的提问照常
    进 TUI 的提问框。
  - `send` = `run_turn` 排队；`answer` = `respondToPrompt`；`note` = `session.appendEntry`（命名空间
    `conix.flow`）；`close` 之后看着它的流结束，Conix 里关掉的会话不会被再带回来。
  - 库的一致性套件（`conformance::run_all`）对着它跑，见 testing.md §8。
- **端口**：`ListFlows` / `GetFlow` / `ListRuns` / `ListEndedRuns` / `GetRun` / `StartRun` / `RunOp` 是库的读取与运行操作
  （`ListEndedRuns` 要结束时刻与站数：第一次要时把运行记录扫一遍记在内存里，之后跟着派工器的变化更新；`GetRun` 拿
  钉住的定义的摘要与流程包里现在的比）；派工器
  每改一次运行（`Runner::changes`）推 `RunUpserted` + `RunDetail` 与那个流程的 `FlowUpserted`（在跑的个数）；
  定义经工具存了推 `FlowUpserted` + `FlowDetail`，删了推 `FlowRemoved`。状态与库一致，"等待 / 暂停中 /
  已暂停"取库算出的 `Phase`。`Hello` 的 `flows` 为真。
- 实例停时派工器的任务不再等，运行不取消；下次启动 `Runner::start` 接着做上次派出去还没答复的活。

### 8.7 实例进程的进程级工作

- `conix new` / `start`：子进程跑 `conix host`，`setsid` 脱离终端，stdin 接 `/dev/null`、stdout 与 stderr
  接 `host.log`；拉起方等 `host.sock` 能连上（至多 30 秒：macOS 第一次执行新装的二进制要先扫描）再接入。
- **看护**：拉起的 `conix host` 是看护进程，它用同样的参数（环境里带 `CONIX_HOST_CHILD`）再拉起真正的实例进程
  并等着；SIGTERM / SIGINT 转给实例。实例正常退出（停止）就跟着结束；被信号杀死（崩溃、kill -9）就按 1s / 5s
  / 30s 退避重启，10 分钟里超过 5 次就放弃并在 `meta.json` 记 `crashed`；起来不到 5 秒就自己报错退出的（锁被占、
  配置错）不重启。重启次数记在 `meta.json`，`conix list` 写出来；发现文件里记着看护进程的 pid。
- `host.log` 超过 10 MB 轮转成 `host.log.1`（留 3 份）：拉起前看一次，常驻时每小时看一次，轮转后把 stdout /
  stderr dup2 到新文件。
- 日志过滤用 `agix::process::log_filter`（否则 rmcp 会把 OAuth 令牌写进日志）；装 panic 钩子。
- SIGTERM / SIGINT：通知 TUI（`InstanceStopped`）、停调度器、`Embedded::shutdown`，删掉 socket 与发现文件。
  SIGHUP 忽略。
- 残留：`list` 时 pid 已不在、或者 socket 连不上的实例标为"残留"并删掉它的 socket 与发现文件；作业与
  会话归属留着，`start` 能再拉起来。

### 8.8 插件

插件就是 Agix 的插件包，Conix 加三样：`/plugin`（安装前摆出 Agix 与 Conix 两份披露）、给插件命令的宿主 JS 库
`conix.*`、插件经 `[x-conix]` 带来的流程包（门禁与程序步骤经 `plugin.call` 调插件函数）。第一个插件是 `xforge`。
设计见 `plugins.md`，依赖 Agix#90、#91、#92 与 Harnix#68。

### 8.9 项目状态的认领

`.conix/state/` 在 `.gitignore` 里，但一个仓库照样可以把它提交进去。克隆下来，里面的 `jobs-state.json` 会让作业
无人值守地跑，`flow-runs/` 里没结束的运行会被派工器接着做。所以 `state/conix/` 里的东西**只有认得出是这台机器
自己写的才读**，与项目受不受信无关（会话历史由 Agix 按同样的思路只读索引登记过的，§8.4）。

- 第一次在一个项目上启动：生成一个随机凭证，写进 `state/conix/owner.json` 与全局的 `known-projects.json`
  （凭证 → 项目路径）。
- 之后启动：`owner.json` 的凭证在 `known-projects.json` 里才认。路径变了是项目搬了家，改写登记的路径。凭证按项目
  登记、不按实例名，实例改名或重建都还认得。
- 认不出来（凭证没登记、没有 `owner.json` 却有别的文件）：`state/conix/` 下除了锁的东西整个挪到
  `quarantine/<时刻>/`，从空状态开始，对话区提示一次。不删：万一是自己的（比如换了 `CONIX_HOME`），可以手动搬回来。
- `.conix`、`state`、`conix` 三级目录都不能是符号链接，否则拒绝启动：指向别处的链接会让认领和锁都落到项目外。
- 顺序：先拿 `host.lock`（拿不到就是这个项目已有实例，报出锁文件里记的实例名后退出），再认领，再打开里面的东西。
  `known-projects.json` 的读改写另加一把 flock（`known-projects.lock`），几个实例同时启动也不会互相覆盖。

### 8.10 通知

实例里产生"等人处理"的事时（TUI 可能不在），经用户配置的出口送出去；没配置就攒着，接入时一次摆出来。

- **Alert**（`host::attention`，订阅自己的领域事件）：有人值守的会话在等回答而 TUI 没接着（`prompt`）、运行的站等确认 /
  搁置 / 未达标（`station`）、作业起的运行结束（`run_ended`，info）、触发失败或被护栏中止（`job_failed`）、自动停用
  （`job_disabled`）、今日额度用完（`daily_budget`）、项目状态被挪进 quarantine（`quarantined`）。去重键是种类加对象，
  同一个对象没变就不再发；60 秒内的合成一条。同一份数据给 `Hello` 的"等你"计数。
- **出口只有一种：命令**，只认全局 `conix.yaml`（项目里的配置不能让别人的机器执行命令或往外发东西）：

  ```yaml
  notify:
    when: detached       # detached（没 TUI 接着才发，缺省）| always
    min_level: warn      # info | warn | error
    sinks:
      - {name: desktop, command: ["terminal-notifier", "-title", "Conix {instance}", "-message", "{text}"]}
      - {name: phone, command: ["sh", "-c", "curl -s -d @- https://ntfy.sh/$TOPIC"], stdin: json, min_level: error}
  ```

  桌面、ntfy、webhook、邮件都是一条命令；Conix 不内建网络客户端，凭据留在用户自己的环境里。占位符
  `{instance} {project} {kind} {level} {title} {text} {attach}` 只做参数级替换（不经 shell）；`{attach}` 是
  `ssh <host> -t conix attach <名>`。每条至多 10 秒，失败重试一次，记到 `state/conix/notify.jsonl`。`TestNotify`
  （`/notify test`）发一条测试，回每个出口的结果。
- **离开期间**：接入时实例推 `AwaySummary{since_ms, alerts}`（上一次有人脱离之后的 Alert），TUI 弹浮层（交互 §5.5）。

### 8.11 配置与在会话里配置自己

**谁的配置在哪**（手册 `config.md` 有全表）：

| 面 | 归谁 | 读的时机 |
|---|---|---|
| `~/.conix/settings.json`、`<项目>/.conix/settings.json`（+ `.local`） | Agix：模型、provider、权限、沙箱…… | 引擎随写随读（`config.set` / 监视文件） |
| cli 层（`host::hub::cli_settings`） | Conix 写给 Agix：只读工具与流程工具不问、`client_may_loosen`、`app.history` | 实例启动 |
| `~/.conix/conix.yaml` | Conix：`ui`（TUI）、`instance`、`jobs`、`notify`、`engine` | 启动；`jobs`、`notify` 经 `conix_config_set` 改时实例当场重读 |
| `~/.conix/keybindings.json` | Conix TUI | TUI 启动 |
| 环境变量 | `CONIX_HOME`、`CONIX_LANG`、`CONIX_PLUGIN_DIRS`；`CONIX_MODEL` 覆盖 `engine.model` | 进程启动 |

`engine.model`：引擎自带 `anthropic` 的模型（相当于 `agixd --model`），只在设置里没定义 `providers` 时用到；它是进程
参数、不是 Agix 的设置键，所以归 `conix.yaml`。

**在会话里配置自己：改与切分开。** 模型在会话里改配置，最怕"一改就生效，新的不通就没有模型可用"。所以：

1. **加**：`config_provider_add` 经 `config.setProvider`（先 `dry_run`）加一个 provider 定义，**不动 `model.order`**——
   现在的路由不变，会话照常用旧模型。
2. **测**：`config_provider_test` 先 `config.listModels`（端点与凭据），再开一个临时会话（`provider` / `model` 指定、
   `turn.token_budget` 极小、`attended: false`、不进列表）打一句、看它答没答、答了多久，测完删掉会话。
3. **切**：`config_switch` 先把现在的 `model.order` 与 revision 记进 `~/.conix/config-backups/`，再 `config.set
   model.order`（带 `if_revision`），立刻用新的缺省打一次；不通就写回备份，回"切换失败、已退回"与原因。
4. **退**：`config_rollback` 回到上一次切换前。

`conix.yaml` 的改动（`conix_config_set`）：按启动时同一个解析先校验，备份后写；`jobs`、`notify` 当场重读，`ui`、
`instance`、`engine` 回"下次启动生效"。写配置的工具都不是只读的，照常问人；只有人开的会话有这些工具。

## 9. 旧实现踩过的坑

| 坑 | 本设计如何避开 |
|---|---|
| "块首行"标记在产出方和消费方的理解正好相反，而测试辅助函数是按错误的理解造数据的，所以测试全绿 | §4.3：只有一种语义，只有一个辅助函数；测试只能用 `view` 的真实输出 |
| 渲染端自动折行，破坏了"一条 entry 一行"的约定 | §4.3：由 model 排版，渲染端不折行 |
| ratatui 的 `Line` 会把 `\n` 吞掉 | 排版时先按 `\n` 拆行 |
| 每来一个 token 就重建整段转录，再整帧 clone 一次 | §4.3 按块缓存，§5 合帧 |
| 在 `select!` 的分支里直接 await 读盘 | §3.1：所有调用 spawn 出去，结果以消息投回 |
| `FrameState` 由两方各填一半 | §4.1：只有 `view` 一个写者 |
| 键位层没有上下文概念，只能给每个模态起别名 | §6：绑定带上下文 |
| 契约里有一批字段从来没人产出 | 守卫：每个字段都必须被某张快照覆盖到 |

## 10. 待定

| 问题 | 倾向 |
|---|---|
| 空闲超时的缺省值 | 30 分钟（`conix.yaml` 的 `instance.idle_timeout`），设为 `0` 表示不超时 |
| 缺省终端尺寸 | 160×45，写在 `conix.yaml` 的 `ui.initial_size`，可以设为 `off` |
| 宽屏下左右两栏的缺省比例 | 60 : 40，可调，并记住 |
| 实例开机自启 | P2：在手册里给出 systemd user unit 和 launchd plist 的示例 |
