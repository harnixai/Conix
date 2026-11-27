# Conix 测试方案

阶段 2 的目标是：只用 mock 后端，把 `interaction.md` 里的每一条交互都验证到。阶段 3 接缝时，这一套测试保持不动，再加上针对真实后端的用例。

## 1. 分层

| 层 | 测什么 | 怎么测 | 需要终端吗 |
|---|---|---|---|
| L1 单元 | `model::update` 的状态变化与产出的 Effect；转录排版（宽度、CJK、`\n`、折叠） | 直接构造 `State`，喂 `Msg`，断言结果 | 不需要 |
| L2 投影快照 | `model::view` → `tui::render` 画出来的画面 | 渲染到 ratatui 的 `TestBackend`，导出为文本快照，与存档比较 | 不需要 |
| L3 场景 | 一段完整的交互：mock 推事件 + 一串按键 → 在若干检查点比对快照或断言状态 | 无终端的事件循环 + `ScriptedBackend` + tokio 暂停时钟 | 不需要 |
| L4 守卫 | 键位表没有冲突、每个动作都有帮助文字；每个 `FrameState` 字段至少被一张快照覆盖；crate 依赖方向正确 | 普通测试 | 不需要 |
| L5 真终端 | 键盘协议、颜色、宽字符、resize、粘贴、tmux 和 ssh 下的表现 | 人工照 `docs/manual-smoke-checklist.md` 过一遍（阶段 2 编写）；接真实实例的验收另有 LIVE 的真终端一层（§8.1），伪终端里起真的 `conix` | 需要 |

L1–L4 全部离线，不需要 API key，在 CI 里跑。

**不写计时断言。** 性能用**计数**来验，不用耗时，因为计时在高负载下会抖：

- 一个 `TextDelta` 只重排 1 个块
- 一次排空消息队列最多画 1 帧
- 没有动画时，Tick 计时器处于关闭状态

## 2. 无终端的事件循环

`app` 的循环要拆成两部分：

- `Core`：持有 `State`、按键解析、后端句柄、时钟，提供 `step(input) -> Vec<Effect>` 和 `frame(size) -> FrameState`。
- 外壳：真实终端的读写，只在 `main` 里使用。

L3 驱动的是 `Core`：

```rust
let mut h = Harness::new(Scenario::load("m04-prompts"), Size::new(120, 40));
h.advance(ms(300));                     // 让场景时间线走 300ms
h.keys("Ctrl+R");                       // 进入提问框
h.snapshot("m04-ask-open");             // 与 tests/snapshots/m04-ask-open.txt 比较
h.keys("2 Enter");
assert_eq!(h.backend().calls().last(), Some(&Command::Answer { .. }));
```

鼠标的写法与按键相同，坐标是快照里的行和列：`h.click(col, row)`、`h.double_click(..)`、`h.drag((c0, r0), (c1, r1))`、`h.scroll(col, row, -3)`。也可以写成 `h.click_on("side.upper", "重构认证模块")`，按区域和文字定位，不受布局变化影响。

终端外壳的读写抽象成 `Term` trait，测试里用 `FakeTerm` 实现。启动时的尺寸协商可以测三种情况：终端照办、不响应、在 tmux 里不发请求。另外还要测退出时恢复原尺寸，以及鼠标捕获的开和关。

快照只记字符，不记样式。样式（高亮、颜色）需要验证时，在 L1 里断言 `FrameState` 上对应的字段。

快照的尺寸固定为三档：`80×24`、`120×40`、`200×50`。设置环境变量 `CONIX_UPDATE_SNAPSHOTS=1` 时重写快照；重写后的 diff 必须在提交里人工看过。

## 3. mock 后端

### 3.1 种子数据

`crates/mock/seeds/*.yaml`：一组可以复用的初始状态，包括会话（带若干轮历史）、任务树、goal、todo、流程定义（按 Harnix 的格式手写，方便以后对照）、运行、作业及其历史。

种子数据也包括实例列表：`MockInstances` 列出的实例覆盖运行中、已停止、残留三种状态，接入或新建一个实例时，就加载下表中的某个种子作为它的内容。

| 种子 | 内容 |
|---|---|
| `empty` | 什么都没有：新用户第一次打开 |
| `default` | 3 个会话（其中一个有 2 层子 Agent 和 4 个任务）、2 个流程（其中一个有 3 次运行）、3 个作业 |
| `busy` | 100 个会话、一个会话下挂 50 个任务、一个流程有 30 次运行：测列表和滚动 |

### 3.2 场景格式

场景文件放在 `crates/mock/scenarios/*.yaml`。其中的事件就是 `port::Event` 的 serde 形式，所以场景文件同时也是端口消息的样例。

```yaml
name: m04-prompts
seed: default
timeline:                       # 场景开始后自动发生的事
  - at: 100
    prompt_pending:
      session: s1
      prompt: {id: p1, kind: permission, tool: Bash, summary: "cargo test", from: {task: t2}}
  - at: 150
    prompt_pending:
      session: s1
      prompt: {id: p2, kind: question, question: "用哪个数据库？", options: ["Postgres", "SQLite"]}
  - at: 5000
    prompt_resolved: {session: s1, id: p1, resolution: {withdrawn: timed_out}}
rules:                          # 命令 → 反应
  - on: {answer: {prompt: p2}}
    reply: ok
    then:
      - {after: 0, prompt_resolved: {session: s1, id: p2, resolution: answered}}
      - {after: 50, play: answer-short}
scripts:                        # 可以复用的事件片段
  answer-short:
    - {after: 0, turn_started: {session: s1}}
    - {after: 30, text_delta: {session: s1, text: "好的，用 SQLite。"}}
    - {after: 30, turn_ended: {session: s1, stop: end_turn, usage: {input: 1200, output: 40}}}
```

- 没有规则匹配的命令，按命令类型给出缺省反应。例如 `Submit` 回放 `default-answer`，`RunOp` 推进运行状态。这样交互运行时随便操作都不会卡住。
- `then` 里的 `after` 相对于命令到达的时刻计算，`timeline` 里的 `at` 相对于场景开始的时刻计算。

### 3.3 交互运行

```sh
conix                                # 阶段 2 直接打开演示 demo（一个场景里什么都有）
conix --mock m11-flow-run            # 用某个测试场景启动真实终端界面
conix --mock m11-flow-run --speed 4  # 时间线加速 4 倍
conix --mock default                 # 只加载种子，不跑时间线
```

只有 mock 模式下，`/events` 才会打开**事件日志浮层**：滚动显示后端推来的每个事件和发出的每条命令，人工验收时用来对照"界面是不是如实反映了后端"。

## 4. 场景清单

每个场景对应若干条需求（需求编号）和若干节交互规格（交互 §），阶段 2 都要有 L3 用例。

| 场景 | 内容 | 覆盖 |
|---|---|---|
| m01-chat-basic | 一问一答，流式正文 + 思考 | C-01 C-10 C-11 · 交互 §4.2 §4.3 |
| m02-text-hard | 长 markdown、代码块、CJK、emoji、制表符、带 `\r\n` 的文本、超长单词 | N-04 · 交互 §4.2 |
| m03-tools | 并发工具调用、结果夹在正文中间、折叠阈值、文件修改的 diff | C-02 · 交互 §4.2 |
| m04-prompts | 审批和提问排队、在提问之间切换、选项与自由输入、超时撤回、在别处已答 | C-04 C-05 · 交互 §4.6 §3 |
| m05-interrupt | 流式中途中断；中断后马上再提交 | C-06 |
| m06-step-restart | 同一步重发，丢掉这一步已画的文字；`model_retry` 提示 | C-03 |
| m07-steer | 运行中插话：送达；没赶上被退回、回到输入框 | C-07 · 交互 §4.4 |
| m08-composer | 多行、历史、按词编辑、大段粘贴的占位、slash 补全 | C-08 C-09 · 交互 §4.4 §4.5 |
| m09-session-tree | 3 层子 Agent + 各种任务，状态频繁变化；进入子 Agent、看 Shell 输出、发消息、停止 | S-04 S-05 S-06 C-16 · 交互 §4.1 §5.2 |
| m10-goal-todo | goal 自动开轮、受阻、暂停和恢复；todo 逐项完成 | S-02 S-03 · 交互 §5.2 |
| m11-flow-run | 一次运行走完：并行的站、搁置后放行、等确认、未达标后返工、暂停后恢复、取消 | F-02 F-03 F-04 F-05 · 交互 §5.3 |
| m12-flow-author | 选中流程定义左边不动、右下是流程设计；在用户会话里说着改流程，设计跟着变；`m` 起话头启动运行；查看定义 | F-07 F-08 · 交互 §4.1 §5.3 |
| m13-jobs | 简单作业和流程作业按时触发、失败、立即运行、启用和停用、从历史跳转 | J-01 至 J-04 · 交互 §5.4 |
| m14-attention | 多处同时等人（会话、站、受阻的 goal、失败的作业），用 ^R 依次处理，角标随之变化 | L-04 L-05 |
| m15-reconnect | 与实例断线 → 重连中 → 恢复（`Resynced` + 快照），列表变灰后恢复；实例被停止后回到实例选择 | I-09 · 交互 §9 |
| m16-storm | 每秒 1000 个 `TextDelta`，同时 50 个任务在更新：用计数断言重排和画帧的次数 | N-02 |
| m17-layout | 在 200 / 120 / 100 / 80 / 60 列之间切换；收起右栏；调整比例；单栏下来回切换 | L-02 L-03 N-01 · 交互 §2 |
| m18-errors | 一轮出错、命令失败、列表加载失败后重试 | 交互 §9 |
| m19-many | 用 `busy` 种子：长列表的滚动、过滤、选中保持不变 | S-01 F-01 J-01 |
| m20-first-run | 用 `empty` 种子：每个标签的空状态和引导文字 | 交互 §9 |
| m21-instances | 实例选择：运行中、已停止、残留、已被占用四种状态；新建、接入、启动、停止；接入被占用的实例时被拒；强制接入（二次确认）以及被踢一方的"已脱离"界面；放着不动也不脱离；脱离和停止的区别；心跳丢失后实例断开连接 | I-01 至 I-06、I-10、I-11 · 交互 §2.2 §4.5 §9 |
| m22-mouse | 交互 §6 表中每一格都要点一遍：标签、列表、展开、工具块、提问选项、二次确认按钮、拖动分隔线、各区域的滚轮、关闭和恢复鼠标捕获；转录里拖选并复制（P1） | M-01 至 M-08 · 交互 §6 |
| m23-startup-size | 尺寸协商的三种情况：终端照办、不响应、在 tmux 里；退出时恢复原尺寸；配置成 `off` | L-06 · 交互 §2.1 |
| m24-footer-hint | 在每一种上下文、每一种浮层和二次确认下，分别在 60 / 80 / 120 / 160 列宽度检查底栏最右边都是 `^G 切块 · ^N 标签 · ^B 切栏 · ^X 退出 · ^H 帮助`；单栏模式下能切过去也能切回来。键位的其余用例（^G 一圈三格、单栏换栏、回左边进提问框，^N 轮换标签，^T 交还输入框，^O 看转录与各种离开都收回精简，转录里 `o` `[` `]`，^H 退格或帮助，兼容键，`/mouse` `/events`）都在 `crates/app/tests/keys.rs` | L-07 L-12 · 交互 §2.4 §3 §7 |
| m25-ownership | 顶栏与对话框标题右边覆盖每一种来源：用户、作业、流程站、子 Agent、分叉；窄宽度下的截断；会话被另一个实例占用时的提示与只读 | C-12 S-08 · 交互 §4.1 |

## 5. 守卫

| 守卫 | 内容 |
|---|---|
| 依赖方向 | `tui` 不依赖 `port` / `model`；`model` 不依赖 ratatui 的终端后端，也不依赖 tokio 运行时；`port` 不依赖 Agix 和 Harnix；`app` 不依赖 Agix |
| 键位 | 同一上下文里没有重复绑定；每个动作都有帮助文字；每个全局动作在 §6 的表里都有替代键或替代命令 |
| 字段覆盖 | 每个 `FrameState` 字段至少在一张快照里取到非缺省值，防止出现"契约里有、但没人产出"的字段 |
| 块首标记 | 只有 `model` 里那一个辅助函数可以读 `first_of_block`（用 grep 守卫） |
| 鼠标不另走逻辑 | 鼠标命中测试的结果只能是一个已有的动作，`update` 里没有专门处理鼠标的业务分支（只有命中测试和拖动状态） |
| 底栏保底 | 对每一种键位上下文渲染一次底栏，断言最后两项是"切换栏位"和"帮助"的实际绑定键 |

## 6. 阶段 2 的验收标准

1. `interaction.md` 里所有的 P0 交互都能在 mock 下操作出来。
2. 上面 25 个场景的 L3 用例全部通过；`cargo fmt --check`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo test --workspace` 全部通过。
3. `docs/manual-smoke-checklist.md` 已经写好，并在至少四种环境里逐项过完：一个 macOS 终端（尺寸协商、鼠标）、tmux（剪贴板、鼠标）、ssh 到 Linux、Linux 物理控制台或固定尺寸的终端（单栏切换、底栏保底提示）。
4. 维护者本人用 `conix --mock …` 亲手试过，确认交互没有问题。

四条都满足才进入阶段 3。

## 7. 阶段 3：真实后端

mock 那一套不动，真实后端按层另加用例，全部离线、不调模型：

| 层 | 测什么 | 怎么测 |
|---|---|---|
| `wire` | 分帧、按 `id` 配对乱序的回复、通知进 channel、连接断了在等的调用一律失败；不认识的 `kind` 跳过 | duplex 另一端的假服务端 |
| `host::hub` | 端口命令变成哪些 Agix 调用、帧变成哪些领域事件、推给谁（只推 Watch 着的会话的细节）；子 Agent 的帧与提问；作业工具与触发记录 | 脚本化的假引擎（`Engine` trait），通知直接塞进 channel |
| `host::jobs` | cron 的本地时区、存储与触发记录、补跑一次、改了就重排 | 注入时钟 + tokio 暂停时间 |
| `host::server` / `client` | 握手、占用、强制接入与被踢、脱离、心跳超时、停实例、事件排在回复前；断线重连；残留检测 | 临时目录里的 socket + 假 `Service` |
| `engine` | 真实嵌入的 Agix + `Hub`：建会话、跑一轮、这一轮有结局 | 没有凭据时那一轮以 `no_credentials` 结束，所以离线也能跑 |

调真实模型的用例标 `#[ignore]`，带上凭据手动跑：`source .env && cargo test -p engine -- --ignored`。
把实例进程、接入与模型整条接起来的用例见 §8。

## 8. LIVE：真实实例 + 真实模型

§7 的各层都是离线的，各自替掉了相邻的一层。LIVE 把整条路接起来：真实的 `conix host` 进程、嵌在里面
的 Agix、真实的模型，从 TUI 走的那条路接入——`SocketInstances` 拉起与接入，`SocketBackend` 收发
`port` 的命令与事件。断言大多落在端口上（回复、事件）和实例目录里的文件上；`live_tui.rs` 再往上一层，
用真的 `app::Core`（键位、model、视图）接真实实例，按键、看界面状态与屏幕；`live_tty*.rs` 是最上面一层，
伪终端里起真的 `conix` 二进制，只有键盘和屏幕（§8.1）。

**只测 Conix 自己做的事。** Agix 的特性（工具怎么跑、权限怎么判、goal 怎么推进、压缩……）由 Agix
自己的 LIVE 测，这里不重复。这里测的是 Conix 加在它外面的那一层：实例的锁、发现文件与 `meta.json`、
单一接入与心跳、帧到领域事件的翻译与路由、会话归属与名字（`sessions.json`）、作业的调度与触发记录、
宿主工具。用例要靠模型做某件事时，话说得短而确定（"只回复 OK"），并且点名引擎真有的工具（Agix 的 Bash
只在前台跑，后台命令要点名 Monitor），断言只看 Conix 的翻译：任务作为没有会话的任务出现、输出取得到、
停得掉，这一轮怎么收尾是模型的事。

| 文件 | 覆盖 |
|---|---|
| `live_instance.rs` | 新建、列出、发现文件与 `meta.json`、目录 0700 与 socket 0600、同名第二个实例拿不到锁、同一项目的第二个实例被拒并报出占着它的实例、停掉再照 `meta.json` 拉起、实例被杀后看护进程把它拉起来（`list` 写出重启次数）、连看护进程一起被杀后列为残留并清理 |
| `live_attach.rs` | `Hello` 与能力位、被占用、强制接入踢人、脱离放开接入、心跳超时断开后重连、实例被停与 `StopInstance`、脱离期间的事在下一次接入时推一次 `AwaySummary` |
| `live_sessions.rs` | 只列本项目的会话、受信项目的会话放在项目里（不受信的在全局，都跨重启列得出）、第一句话做名字、改名与关闭跨重启保留、换模型、命令表、Watch 快照的历史与 todo、拒绝逐轮权限模式、建会话带模型与模式、已关闭的会话（列、看、重开、删）、重启后的累计用量 |
| `live_turns.rs` | 文字与用量、`/compact` 的压缩中与收尾、忙时退回、插话送达、排队、中断、权限与模型提问的作答、侧问分叉、goal 的事件与快照、后台任务的输出与停止、子 Agent 的侧链会话与给它发消息 |
| `live_jobs.rs` | 增删改与启停（定义进 `.conix/jobs.json`、启停进本机的 `jobs-state.json`）、立即运行触发的会话与结局、触发记录落盘、重启后没跑完的那次记为中断、错过的触发只补一次、停下正在跑的那一次记成手动停下、conix.yaml 给的超时到了被打断记成超时、停实例与崩溃留下的那次记成"实例停了"、一次失败的触发经配置的命令出口送出（TestNotify 回每个出口的结果）。作业定义要项目受信才读写，用例用 `start_trusted` |
| `live_flows.rs` | 流程：内置示例流程；项目没受信时 `flow_create` 存不进去、`/trust` 之后存进项目自己的流程包；用户会话经 `flow_validate` / `flow_create` 写流程；一次运行从头跑到尾、主会话的归属；`workspace: worktree` 的运行在仓库根的 `.wkts/<运行>/`、主仓库看不到它与项目状态；有运行时 `flow_update` 被拒并列出运行，取消后才能改；运行操作经端口落到派工器（搁置的站放行、暂停与恢复、返工、强制放行、确认）；运行里的会话动不了别的流程；流程作业触发一次运行；用户会话用 `job_validate` / `job_save` / `job_run` 在对话里建出流程作业并换主题跑一次；库的一致性套件对着 Conix 的 `SessionHost`（带凭据把测试进程再起一遍，引擎嵌在里面）；全局库里的包不装不跑；命令行的 `conix flow install` / `update` 对着在跑的实例：实例经 socket 收到 `ReloadFlows` / `ReloadJobs`，流程标签多出包的流程、作业标签多出包建议的作业（停用），`update` 只加新版新带来的（不调模型） |
| `live_tui.rs` | 界面经真实实例：经界面新建并接入、底栏那几项；^N 轮换标签（流程标签里是实例给的示例流程）、^G 一圈三格、单栏时换栏；^T、^H；`/trust` 的回复；^X 后台运行后实例还在、再接回来。草稿上第一句话建出会话并跑完一轮，^O 看转录，^G 到右上选中的就是它，`/rename` 经实例改名。真实的权限询问：^G 走开、^R 找回、转回左边进提问框、Enter 批准后工具真的跑了 |
| `live_tools.rs` | 宿主工具：用户会话经 `job_*` 增改停删作业、`session_rename` 改自己的名；查看不问、改东西经权限询问；`#n` 与 ID；作业触发的会话动不了别的作业 |
| `live_cli.rs` | 命令行子命令对着真实实例：`conix list` 的一行、`--all`、`conix stop`；首次运行写出的缺省配置按 `CONIX_LANG` |
| `live_diagnostics.rs` | 真实实例的 `/doctor`（引擎自检加实例自己的几项）、`@` 补全的文件列表、`/model` 的模型列表 |
| `live_prompts.rs` | 权限的拒绝与本会话总是允许、提问超时被引擎撤回、改文件的 `file_changes` 与 diff |
| `live_limits.rs` | 一轮因 `turn.max_model_calls` 以 `MaxTurns` 收尾、因预算以 `Budget` 收尾 |
| `live_config.rs` / `live_config_conix.rs` | 会话里配置自己：provider 的加、测、切、退；`config_show`、`conix_config_set`（校验、备份、jobs 当场生效）；排在前面的 provider 连不上时引擎换到下一个并推 `ProviderSwitched` |
| `live_demo.rs` | 演示项目 `examples/demo` 照原样跑。`brief` 一次运行走遍流程的功能：声明的门禁读模型写的 YAML（提纲）、JSONL（两份审读）、Markdown，`check` 的 schema / files / text / sealed 都真判过、证据进运行记录；`command` 门禁跑项目自己的命令行（`tools/notesctl.py`：数字数、逐条核对引文出自原文）；`script` 门禁读治理文件 `.conix/governance/policy.yaml` 下能不能签发的结论。多模型：主会话在 DeepSeek，子 Agent 与汇总的独立会话在 Claude，审读站两个工作项同时跑、一个 DeepSeek 一个 Claude（会话的模型、时间重叠都核对），两家都花了 token；子 Agent 的交付核实过子 Agent。两种授权：签发站模型确认不了、`roles.yaml` 名单外的人被拒、实例主人确认后记成 `person`；发布站经 MCP（Agix 的 `mcp.callTool`）建单、在演示审批系统里批、记下审批人与单的链接。`digest` 的子运行交给父任务的是子流程声明的 `result`（`require: any`）。`publish` 单独走一次外部审批。演示的作业读得出、是停用的，简单作业立即跑一次有结果。三个流程都是流程包（brief、digest 带子流程 note、publish）：运行在 `conix/runs/` 下、`conix/.gitignore` 是 `runs/`，brief 结束后连同 `record.json` 归档进 `conix/brief/archive/` |
| `live_xforge.rs` | Conix 内置的 xforge 流程包在 `examples/xforge` 上真跑 `xforge/solid`：制品目录配成 `xforge`，运行在 `xforge/runs/`；规格增量、台账、工作包由包的命令（`xspec`、`xledger`、`xpkg`）写；主会话在 Claude，独立检查的子 Agent 在 DeepSeek，检查留下 open 的问题时用例像人一样打回设计；两处人的确认后归档进 `xforge/archive/`，增量并进 `xforge/specs/`，项目的验证照样过。`xforge/dual`：用例演人（照默认答澄清的问题再重判、规格侧定了、两路对上了、验收；检查与合起来没过时照建议打回，有上限；合起来的会话判出错在一路时只打回分叉站的那个任务）；两个子运行由 fork 站起、从同一个分叉提交出发、各在 `.wkts/` 下自己的 worktree 里，记录里禁读的名单（`ForkRec` 的 `parent_path` 与 `siblings`）盖住父运行与兄弟的 worktree，交付后合回父运行的分支；测试那一路只写 `tests_blackbox/`、实现那一路改 `src/`，各自的目录合回 `children/fork/<任务>/`；每站的 spec-unchanged 都过；门禁 blackbox 的日志里真跑了黑盒测试、最后一轮过了、它起的进程记成运行历史的 `exec`；归档在父运行的 worktree 里并进两条基线、留着两路的设计；加一个函数不破坏接口，「批准破坏性改动」记在提案站的 `not_required` 里、人没被问；项目的分支直到人收尾才有改动：视图里 `worktree.decidable`，`RunOp::EndWorktree{merged}`（`/worktree merge`）之后项目有了实现与黑盒测试，worktree 与分支删了。dual 大约 1–2 小时 |
| `live_xreport.rs` | 内置的 xreport：`conix flow install xreport --yes --jobs yes` 装进项目（包的目录、lock 的条目、`xreport-weekly` 进 `.conix/jobs.json` 且本机停用、实例读得出）；造好的历史（两个作者、`.mailmap` 并掉一个旧名字）上跑一次：`stats.json` 是 git 数的，`REPORT.md` 的头与表对得上它、名字是并过的、没有邮箱；统计起的每个 git 都记成运行历史的 `exec`（argv、档位 `workspace`、不上网、`enforced` 照后端实记、不记输出）；主人定稿后归档留三个文件。全局 `conix.yaml` 关了命令（`flows.command_gates: off`）时统计站失败、说是哪个开关（不调模型） |
| `live_models.rs` | 一次运行用两个模型（DeepSeek 与 Anthropic）：一站生成、一站检视，各在自己的会话里、各走自己的 provider（正反各一次）；检视打回、人照结论退回生成站，两轮都还在各自的模型上（返工不带理由，第几轮该怎么写放在技能里）；主会话在 DeepSeek 上、子 Agent 站点名 Claude 的模型，子 Agent 的任务报它实际用的模型与 provider。看会话上记的模型、两家的用量记录（`<data_root>/quota/<id>.json`，只有带 `quota` 的 provider 才记）、运行时目录里的报告。`.env` 没有 `CLAUDE_API_KEY` / `CLAUDE_MODEL` 就跳过 |
| `live_plugins.rs` | 插件：包装进去、启用、插件的流程能跑；xforge 从 open 到 archive |
| `live_tty.rs` | 真终端（§8.1）：起实例、`/trust`、^N、后台运行与接回、直接退出停掉实例；一句话的回复与 `/rename`；权限询问 ^R 找回、Enter 批准；xreport 从键盘启动、确认、归档 |

`live_xforge.rs` 的 dual、`live_xreport.rs`、`live_flows.rs` 的命令行重读实例是在 Agix 的沙箱执行后端（harnixai/Agix#112）之前写的：
现在执行服务只有普通子进程，`enforced` 都是 `{backend: plain, filesystem: false, network: false}`，用例只断言它照实记了，不断言守住了；
后端换上之后先跑这几个。命令行这一层不要实例的（`conix flow doctor` 跑包自己的自检：xforge 缺的治理文件是 warn、坏的原则是 error，
xreport 没受信、关了命令说清楚开关）在 `crates/conix/tests/cli.rs`，随 `scripts/check.sh` 跑。

**怎么跑**：`scripts/live.sh`（全部）、`scripts/live.sh live_jobs`（一个文件）、`scripts/live.sh live_jobs
catch_up`（再按名字过滤）、`scripts/live.sh tty`（只跑真终端那一组 `live_tty*.rs`，后面同样可以跟名字过滤）。凭据取仓库根的 `.env`（worktree 里往上找；`CONIX_LIVE_ENV` 可指定），要有
`ANTHROPIC_AUTH_TOKEN`（或 `ANTHROPIC_API_KEY`）与 `ANTHROPIC_MODEL`，读不到就失败并说明。凭据只交给
实例进程的环境（`SocketInstances::with_env`），不进测试进程、不打印。同时起的实例数用 `LIVE_THREADS`
控制（缺省 4）；跑全部时流程的一致性套件不进并发，最后单独串行跑（它自己就要跑十几轮模型，挤在并发里会撞上单轮
超时）。用例全标 `#[ignore = "live"]`，`./scripts/check.sh` 只编译不跑。

流程只在项目里：`live_flows.rs` 的 `install` 把流程放进项目的 `.conix/flows/`，用例起实例时信任项目。`port::FlowOrigin`
只有 `Builtin` / `Project` / `Plugin`；项目里与内置同名的列出来是 `Project`。

**`.env`** 按厂商写：`DEEPSEEK_API_KEY` / `DEEPSEEK_BASE_URL` / `DEEPSEEK_MODEL`（缺省的模型，`DEEPSEEK_FAST_MODEL` 是"换一个模型"用的）、
`CLAUDE_API_KEY` / `CLAUDE_BASE_URL` / `CLAUDE_MODEL`（多模型的用例；`CLAUDE_FAST_MODEL` 可选）。引擎自带的 anthropic provider 读
`ANTHROPIC_*`，harness 由 `LIVE_DEFAULT` 点的那一家补出来：`deepseek`（缺省）或 `claude`，哪家额度多单模型的用例就走哪家；
多模型的用例按厂商的那组读，不受它影响。老写法（直接写 `ANTHROPIC_*`）照旧认。

**harness**（`crates/conix/tests/live/mod.rs`）：

- `Live`：一个用例的世界——/tmp 下短路径的 `CONIX_HOME`（socket 路径有 104 字节的上限）、一个 git
  示例项目、写好模型名的 Agix `settings.json`、可选的 `conix.yaml` 与只给实例的环境变量。`start`、
  `attach`、`stop`、`restart`、`start_again`；`read("meta.json")` 读实例目录的文件、`read_state("sessions.json")`
  读项目状态 `state/conix/` 里的；`Drop` 时
  不论成败停掉它起的所有实例。
- `tui::Tui`（`live/tui.rs`）：真的界面接真实实例。`Tui::create` / `Tui::attach` 经界面新建或接入；
  `keys("^G ^N")`、`type_str`、`say`（打字加回车）；`until(说明, 超时, |t| …)` 用墙上时间等，一边收实例的
  消息一边按 model 给的时刻 Tick，等不到就附上当时的屏幕；`text`、`last_line`、`transcript`、`idle`。
- `tty::Tty`（`live/tty.rs`）：伪终端里的真 `conix`，§8.1。
- `Client`：一个接入，相当于一个 TUI。`ok(cmd)`（失败就让用例失败并说是哪条命令）、`call(cmd)`；
  `wait_event(说明, 超时, |e| …)` 从游标往后等，等不到时附上最近 30 条事件；`new_session`、`watch`、
  `turn(会话, 话) -> TurnOutcome`（文字、工具、结果、提问、用量、结局）、`wait_prompt`、`wait_upsert`。
  心跳缺省像开着的 TUI 那样每 10 秒报一次，`heartbeat(Beat::None)` 停掉；提问缺省留给用例自己答，
  `answering(Answering::Auto)` 改成权限一律允许、问题选第一项（Watch 快照里悬着的也答）。

**加用例**：功能归哪个文件就加在哪个文件，或新开一个 `live_<功能>.rs`（`mod live;` 引入 harness，
`scripts/live.sh` 自动带上）。等事件，不比时长；跑不稳先找原因，不放宽断言。为可测性给实例加的
开关只走环境变量，名字以 `CONIX_TEST_` 开头（现有 `CONIX_TEST_HEARTBEAT_TIMEOUT_MS`），不进 `conix.yaml`。

### 8.1 真终端

前面几层都看得见 Conix 的里面：端口驱动的用例收发 `port` 的命令与事件，`live_tui.rs` 读 model 的状态。真终端这一层
什么都看不见，只有一台 160×45 的终端：`portable-pty` 开伪终端，在示例项目里起 `target` 下真的 `conix`（`conix new
--name lv`、`conix attach lv`，`CONIX_HOME` 是这个用例的家目录、`CONIX_LANG=zh`、`TERM=xterm-256color`），按键编成
xterm 发的字节写进去，`vt100` 照 `conix` 写出的 VT 序列还原屏幕。实例由这个 `conix` 照用户的路子拉起，凭据与模型
在它的环境里、由实例继承，和用户在自己的 shell 里跑一样。

**三层各管什么。** 端口驱动的 LIVE 测机制，断言细（哪个事件、哪个字段、哪份文件的哪一项）；`live_tui.rs` 管界面的
细节（焦点、标签、浮层、model 里的状态）；真终端是验收：人照着屏幕敲键盘，断言粗，落在屏幕上看得见的字和项目最后的
样子（交付物、归档）上。一件事在下面一层测细了，这里只走一遍人走的路。

**规矩**（写在 `live/tty.rs` 的模块文档里）：

- 动作只走键盘：说话、斜杠命令（`/flow:<包> <主题> [--k v]`、`/confirm`、`/reject`、`/rework`、`/worktree`、
  `/trust`）、键（^R 找到等人的地方、提问框里 Enter 批准、^X 退出）。不发端口命令。
- 观察主要看屏幕。屏幕上没有稳定的字可等时给界面加一个小标记，只改界面、不改协议，字进 i18n（现有的：运行在等人
  确认时流程标签里写「等你确认」，`flows.run.waiting_confirm`）。
- 要做决定时可以读项目里的文件，人也会去看仓库（定稿前读一遍 `REPORT.md`）。人在 Conix 之外做的事（装流程包、改
  治理文件、提交）就是普通的命令与文件读写。

**harness**（`live/tty.rs`）：

- `Tty::new(&lv)` / `Tty::attach(&lv)` / `Tty::spawn(&lv, args)`；`keys("^G ^G Enter")`（写法同键位配置，
  `keys::Key::parse`）、`type_text`、`line`（打字加回车）；`screen`、`row`、`last_line`、`contains`、`between`。
- 等：`wait(说明, 上限, |屏幕| …)`、`wait_text`、`wait_any`；`drive(说明, 上限, |tty, 屏幕| Step)` 是要中途动手的长
  等待（批准冒出来的权限询问、答问题），上限可以是一个钟头，每分钟往 stderr 打一行在等什么、底栏是什么。
- 读屏：`chat_state`（左边会话的 `● 运行中` / `◐ 等你` / `○ 空闲`）、`asking`（提问框拿着键盘）、`attention`
  （底栏 `^R 下一处待处理(n)`）；`say(话, 上限)` 说一句话等这一轮跑完（权限询问一律批准），`wait_idle`、`close_away`
  （关掉接入时弹的「离开期间」）。
- 收尾：`quit`（^X 直接退出，停掉实例）、`detach`（^X 后台运行）；`listed_state(&lv)` 是人在另一个终端里看
  `conix list --all`。等不到、`conix` 自己退了、用例 panic，都打出屏幕（带行号）、做过的动作（带时刻）、`host.log` 的
  末尾与项目里每次运行的状态；没走到 `quit` 时 `Drop` 杀掉 `conix`，`Live` 的 `Drop` 停实例。
- `on_claude(opts)`：主模型是 Claude 的 `Live`（DeepSeek 排在后面给要换一家的站），`.env` 没有 Claude 的 key 就是
  `None`、用例跳过。

| 用例（`live_tty.rs`） | 人做了什么 | 看什么 |
|---|---|---|
| `the_terminal_starts_an_instance_detaches_from_it_and_stops_it_on_quit` | `conix new --name lv`；`/trust`；^N 三下；^X 后台运行；`conix attach lv`；^X 直接退出 | 顶栏是这个实例、启动是草稿；「已信任这个项目」；流程标签「还没有流程」；`conix list` 里先是 Running、退出后 Stopped |
| `a_line_typed_in_the_draft_gets_its_reply_and_rename_renames_the_session` | 草稿上说一句只回 pong 的话；`/rename 乒乓` | 左边从运行中到空闲、转录里是 `⏺ pong`、会话列表有 #1；标题变成「会话 › 乒乓」 |
| `a_permission_prompt_is_found_with_ctrl_r_and_approved_with_enter` | 叫模型跑一条要批准的 Bash；^G 走开、^R、^G ^G 回到左边、Enter | 提问框拿着键盘、左边是「等你」；走开后交出键盘、^R 提示「待处理 1/1」；批准后跑完，命令的输出在转录里 |
| `xreport_is_started_confirmed_and_archived_from_the_keyboard` | 造好这一周的历史，`conix flow install xreport`；关掉「离开期间」；`/trust`；`/flow:xreport 上周的提交 --since <6 天前>`；待处理变成 1 时 ^R；读 `REPORT.md`；^G ^G、`/confirm 报告定稿` | 「已启动「上周的提交」（运行）」；流程标签里运行「等你确认」；报告里是并过的名字、没有邮箱、五节都在；「已确认 … 的站 publish」；运行进「已结束」、`conix/xreport/archive/` 里四个文件，`REPORT.md` 就是看过的那一份 |

xreport 的 `days` 是整数，而斜杠命令的 `--k v` 都按字符串交给实例，`--days 7` 过不了输入的 schema（`input days: "7"
does not satisfy its schema`）；用例写成同一个范围的 `--since`。

### 需求与用例的对照

每条需求（`docs/reqs/conix-requirements.md`）一行，再加需求文档之后做的几项。**层**：LIVE（`crates/conix/tests/live_*.rs`，
真实实例 + 真实模型）、mock 场景（`crates/app/tests/*.rs`，TUI 接 mock 后端）、真实引擎离线（`crates/engine/tests/`、
`crates/conix/tests/e2e.rs`、`crates/host/tests/`）、单测。纯界面的需求（排版、键位、鼠标）只在 mock 场景里验；
行为取决于真实的实例、引擎、模型、进程生命周期、磁盘上的文件或 socket 的，必须有 LIVE 行，没有的标 `缺`
（用例写完就把 `缺` 换成用例名）。`app/` 前缀的文件在 `crates/app/tests/`；`live_*` 在 `crates/conix/tests/`。

| id | 一句话 | 覆盖它的用例 | 层 |
|---|---|---|---|
| I-01 | 无参数 `conix`：有实例弹选择列表，没有就新建 | app/instances::m21_picker_lists_every_state、m21_attach_occupied_from_command_line_falls_back_to_picker（选择逻辑在终端里，进程级要 tty） | mock 场景 |
| I-02 | `conix new` 新建并接入 | live_instance::a_new_instance_is_listed_with_its_discovery_file_meta_and_private_socket；conix/e2e::create_attach_talk_kick_and_stop_a_real_instance | LIVE |
| I-03 | `conix list [--all]` 的纯文本输出 | app/instances::m21_picker_lists_every_state（选择列表）；live_cli::list_and_stop_work_from_the_command_line（纯文本一行：名字、pid、项目、接入者；`--all` 列出已停的） | LIVE |
| I-04 | `attach` / `start` / `stop` | live_attach::{hello_reports…, detaching_releases…}；live_instance::a_stopped_instance_is_started_again_from_its_meta；live_attach::the_stop_instance_command_shuts_the_instance_down；live_cli::list_and_stop_work_from_the_command_line（`conix stop`） | LIVE |
| I-05 | 退出是脱离，`/stop` 才停实例（二次确认） | live_tui::the_keys_walk_the_workbench_of_a_real_instance（^X 后台运行后实例还在）；app/workbench::ctrl_x_exit_dialog_background_stop_or_back、ctrl_c_twice_detaches_and_mouse_command_releases_the_mouse；app/instances::m21_stop_needs_confirmation | LIVE + mock 场景 |
| I-06 | 单一接入、被占用显示占用者、强制接入、被踢者看到谁踢的 | live_attach::a_second_client_is_refused_until_it_forces_and_then_the_first_is_kicked；host/server::second_client_is_told_who_holds_it、force_kicks_the_old_client；app/instances::m21_occupied_needs_confirmed_force、m21_kicked_goes_to_detached_then_back_to_picker | LIVE |
| I-07 | 只走 unix socket，0600/0700，校验对端 uid，不监听网络 | live_instance::a_new_instance_is_listed_with_its_discovery_file_meta_and_private_socket（权限）；对端 uid 校验在 `host/src/server.rs`，换一个 uid 接入造不出来，只能靠代码审阅 | LIVE（uid 校验无用例） |
| I-08 | 实例意外退出后 list 识别残留并清理 | live_instance::a_killed_instance_is_listed_as_stale_cleaned_and_can_be_started、a_crashed_instance_is_restarted_by_its_supervisor | LIVE |
| I-09 | 断线重连、补齐、被占用就不再重试 | live_attach::a_silent_client_is_dropped_after_the_heartbeat_timeout_and_reconnects；app/crosscut::m15_reconnect_shows_state_and_refreshes；live_tui::the_keys_walk_the_workbench_of_a_real_instance（再接回来） | LIVE |
| I-10 | 不做空闲脱离 | app/instances::m21_a_quiet_tui_stays_attached_and_keeps_beating；host/server::a_long_idle_holder_is_not_detached | 单测 + mock 场景 |
| I-11 | 心跳保活，几次没收到就断开 | live_attach::a_silent_client_is_dropped_after_the_heartbeat_timeout_and_reconnects；host/server::silent_client_is_dropped_after_heartbeat_timeout；app/instances::m21_heartbeat_every_ten_seconds | LIVE |
| L-01 | 左对话区、右三标签 | app/workbench::m17_wide_shows_both_panes；live_tui::the_keys_walk_the_workbench_of_a_real_instance | mock 场景 + LIVE |
| L-02 | 宽屏并排、窄屏单栏切换 | app/workbench::m17_wide_shows_both_panes、m17_single_pane_switches_back_and_forth；app/keys::ctrl_g_switches_the_pane_when_only_one_fits | mock 场景 |
| L-03 | 右栏可收起 | app/workbench::m17_collapse_and_ratio | mock 场景 |
| L-04 | 标签头上等你处理的数量 | app/crosscut::tabs_with_waiting_items_get_a_mark_not_the_selected_color；等待数来自实例：live_turns::a_permission_prompt_waits_for_the_answer（`waiting == 1`） | mock 场景 + LIVE |
| L-05 | 一个键跳到下一处等你处理 | app/crosscut::m14_ctrl_r_walks_every_waiting_place_across_tabs；live_tui::ctrl_r_finds_the_permission_prompt_and_the_answer_reaches_the_instance | mock 场景 + LIVE |
| L-06 | 启动时请求终端尺寸 | 终端行为，人工（manual-smoke-checklist） | 无（L5） |
| L-07 | 底栏保底几个键 | app/keys::the_basics_are_always_at_the_right_of_the_footer；app/workbench::m17_too_small_still_has_footer；live_tui（底栏那几项） | mock 场景 + LIVE |
| L-08 | 退出要问一句 | app/workbench::ctrl_x_exit_dialog_background_stop_or_back；live_tui::the_keys_walk_the_workbench_of_a_real_instance | mock 场景 + LIVE |
| L-09 | 配色可配；`conix.yaml` 不存在时生成带注释的缺省配置 | 配色：app 快照；生成文件：live_cli::the_first_run_writes_a_commented_default_config_in_the_chosen_language；app/src/config.rs 的单测 | LIVE + mock 场景 |
| L-10 | 滚动统一 | app/crosscut::scroll_keys_act_on_the_focused_area、m19_long_lists_scroll_and_filter；app/sessions::a_long_panel_scrolls_under_the_wheel_and_never_takes_the_focus | mock 场景 |
| L-11 | 多语言：界面、命令行帮助、配置模板 | app/i18n_en::{cli_help_resources_are_english, tui_footer_and_header_are_english}、app/i18n_guard::no_hardcoded_chinese_in_ui_code；conix/cli::cli_help_is_english；live_cli::list_and_stop_work_from_the_command_line（`list` 的行按语言）、the_first_run_writes_a_commented_default_config_in_the_chosen_language（配置模板按语言） | LIVE + mock 场景 |
| L-12 | 命令键只用 Ctrl+字母 | app/keys::{the_rare_ones_are_slash_commands, the_terminals_own_editing_keys_still_work}（键位守卫） | mock 场景 |
| M-01 | 点击切换焦点 | app/workbench::side_selection_by_keys_click_and_wheel；app/guards::mouse_only_hit_tests_and_drags | mock 场景 |
| M-02 | 点击标签、列表项、展开 | app/crosscut::m22_mouse_on_picker_exit_dialog_tabs_and_ask；app/workbench::side_selection_by_keys_click_and_wheel | mock 场景 |
| M-03 | 滚轮滚鼠标所在区域 | app/sessions::a_long_panel_scrolls_under_the_wheel_and_never_takes_the_focus；app/workbench::side_selection_by_keys_click_and_wheel | mock 场景 |
| M-04 | 点击工具块头展开 | app/chat::m03_select_block_and_toggle_with_enter、m03_tools_render_like_claude_code_and_ctrl_o_expands | mock 场景 |
| M-05 | 提问、确认、表单可点 | app/crosscut::mouse_click_ask_option_answers、m22_mouse_on_picker_exit_dialog_tabs_and_ask | mock 场景 |
| M-06 | 拖动分隔线 | app/workbench::m17_drag_divider；app/guards::mouse_only_hit_tests_and_drags | mock 场景 |
| M-07 | 拖选转录复制（OSC 52） | 拖选未做（P1），`y` 复制：app/chat::y_in_the_transcript_copies_the_last_reply | mock 场景 |
| M-08 | `/mouse` 交还鼠标 | app/workbench::ctrl_c_twice_detaches_and_mouse_command_releases_the_mouse | mock 场景 |
| C-01 | 流式正文与思考 | live_turns::a_turn_streams_text_and_ends_with_usage；app/chat::m01_streams_with_thinking_status_and_markdown | LIVE + mock 场景 |
| C-02 | 工具块、折叠、改文件显示 diff | 工具块：live_turns::a_permission_prompt_waits_for_the_answer；app/chat::m03_*；live_prompts::a_file_edit_arrives_as_a_change_with_a_diff（真实引擎的 `file_changes`：路径、种类、unified diff） | LIVE + mock 场景 |
| C-03 | 步骤重发丢掉这一步的文字 | app/chat::m06_step_restart_discards_partial_text；host/hub::a_mid_turn_snapshot_adds_only_the_unpersisted_step；真实模型造不出确定的步骤重发：缺（不可确定地触发） | 缺 |
| C-04 | 权限审批：允许一次、本会话总是、拒绝 | 允许一次：live_turns::a_permission_prompt_waits_for_the_answer；live_prompts::a_denied_permission_stops_the_tool_and_the_turn_goes_on（拒绝）、allow_for_the_session_is_asked_once（本会话总是允许）；app/chat::m04_permission_question_and_withdrawn | LIVE + mock 场景 |
| C-05 | 模型提问作答；引擎撤回时对话框消失 | 作答：live_turns::a_model_question_is_answered_with_a_choice；撤回（超时）：live_prompts::an_unanswered_prompt_is_withdrawn_by_the_engine_when_it_times_out；app/chat::m04_permission_question_and_withdrawn | LIVE + mock 场景 |
| C-06 | 中断这一轮 | live_turns::interrupt_ends_the_turn_as_cancelled；app/chat::m05_esc_interrupts_a_running_turn | LIVE |
| C-07 | 运行中插话；退回的放回输入框 | live_turns::a_turn_while_busy_is_returned_and_a_steer_is_delivered、a_queued_message_runs_as_the_next_turn；app/chat::m07_steer_delivered_and_returned | LIVE |
| C-08 | 输入框编辑 | app/chat::m08_composer_picker_modes_and_keys、m08_ctrl_c_and_ctrl_d_like_claude_code；app/keys::the_terminals_own_editing_keys_still_work | mock 场景 |
| C-09 | slash 补全：本地加引擎命令表 | live_sessions::set_model_is_recorded_and_the_command_table_is_listed；app/chat::m08_composer_picker_modes_and_keys | LIVE + mock 场景 |
| C-10 | 状态行：耗时、token、压缩中 | token：live_turns::a_turn_streams_text_and_ends_with_usage；压缩中：live_turns::compact_shows_compacting_and_then_how_it_ended（`/compact` 先有 `Compacting`，再以 `Compacted` 或 `CompactEnded` 收尾；历史短，多半是没有可压的，造出够长的历史去真压太贵，压成那一路由 Agix 自己的 LIVE 管）；host/hub::compacting_follows_the_engine_frames_and_says_why_it_did_not_compact（首尾帧的翻译、子 Agent 的落在侧链）；model chat::a_compaction_that_did_not_compact_says_why | LIVE |
| C-11 | 转录滚动与回到底部 | app/crosscut::scroll_keys_act_on_the_focused_area；app/chat::m16_storm_relayouts_one_block_per_delta | mock 场景 |
| C-12 | 归属：来源与状态 | live_sessions::the_first_line_names_a_session_and_renames_and_closes_survive_a_restart（User 与实例名）；live_turns::a_side_question_forks_a_session_and_reports_back（Fork）、a_sub_agent_shows_up_as_a_task_with_its_own_session（SubAgent）；live_jobs::run_now_fires_a_job_session_and_records_the_outcome（Job）；live_flows::a_run_goes_from_start_to_finish（FlowStation）；app/sessions::m25_ownership_line_for_every_origin | LIVE |
| C-13 | `/model` 切换，下一轮生效 | live_sessions::set_model_is_recorded_and_the_command_table_is_listed；live_diagnostics::models_and_project_files_are_listed_for_the_pickers（`ListModels`；本仓库的第三方端点不给模型列表，只验命令成功与每项的形状）；app/chat::slash_model_opens_a_picker_and_switches | LIVE |
| C-14 | 侧问 `/btw` | live_turns::a_side_question_forks_a_session_and_reports_back | LIVE |
| C-15 | 通知显示在底栏并消失 | live_turns::a_side_question_forks_a_session_and_reports_back（`Notice`）；live_tui::the_first_line_in_the_draft_makes_a_session_and_ctrl_o_reads_it（notice） | LIVE |
| C-16 | 子 Agent 上下文里输入送达；只读时禁用 | live_turns::a_sub_agent_shows_up_as_a_task_with_its_own_session；app/sessions::drill_into_sub_agent_message_it_and_come_back | LIVE + mock 场景 |
| C-17 | 对话区照 Claude Code | app/chat::demo_history_renders_every_block_kind 与快照 | mock 场景 |
| C-18 | 开场欢迎块 | app 快照；live_tui::the_keys_walk_the_workbench_of_a_real_instance | mock 场景 |
| C-19 | ^O、^T、`?` | app/keys::ctrl_o_reads_the_transcript_and_any_way_out_brings_brief_back、ctrl_t_toggles_the_panel_and_hands_the_keyboard_to_the_composer；live_tui（^T、^O） | mock 场景 + LIVE |
| C-20 | Shift+Tab 权限模式，会话的、跨重启 | live_sessions::the_permission_mode_is_the_sessions_and_takes_effect_at_once；app/permission_mode::shift_tab_sets_the_open_sessions_mode_and_the_display_follows_the_session、a_drafts_mode_goes_with_the_session_it_creates | LIVE |
| C-21 | `@` 补全、`!` 命令、搜历史、双击 Esc 回退 | 回退：live_sessions::checkpoints_list_turns_and_rewind_removes_the_later_ones；app/chat::double_esc_rewinds_to_before_a_turn_and_puts_it_back；live_diagnostics::models_and_project_files_are_listed_for_the_pickers（`@` 的文件列表 `ProjectFiles`，含未提交的新文件、不含被忽略的）；app/chat::at_completes_project_files | LIVE |
| S-01 | 上半会话列表 | live_sessions::only_this_projects_sessions_are_listed；app/sessions::upper_lists_main_agents_with_origin_and_expands_goal_and_todo | LIVE |
| S-02 | goal 与 todo | live_turns::goal_changes_are_pushed_and_in_the_snapshot；live_sessions::a_watch_snapshot_rebuilds_history_and_todos | LIVE |
| S-03 | goal 的设定、暂停、恢复、修改、清除 | live_turns::goal_changes_are_pushed_and_in_the_snapshot；app/sessions::goal_pause_and_clear | LIVE |
| S-04 | 下半拆分树 | live_turns::a_sub_agent_shows_up_as_a_task_with_its_own_session、a_background_task_streams_output_and_can_be_stopped；app/sessions::lower_tree_three_levels_and_finished_group | LIVE |
| S-05 | 进子 Agent 侧链；看任务输出 | live_turns::a_sub_agent_shows_up_as_a_task_with_its_own_session、a_background_task_streams_output_and_can_be_stopped；app/sessions::task_output_is_read_only_and_follows | LIVE |
| S-06 | 任务停止、发消息 | live_turns::a_background_task_streams_output_and_can_be_stopped、a_sub_agent_shows_up_as_a_task_with_its_own_session | LIVE |
| S-07 | 新建会话：项目、场景、模型 | 缺省的新建：每个 LIVE 的 `new_session`；live_sessions::a_session_is_created_with_the_chosen_model_scene_and_mode（带模型 / 场景 / 权限的 `CreateSession`）；live_sessions::the_permission_mode_is_the_sessions_and_takes_effect_at_once | LIVE |
| S-08 | 恢复历史、重命名、关闭；已关闭的列出、查看、重开、删除；被别的实例占着时提示 | 历史与改名关闭：live_sessions::a_watch_snapshot_rebuilds_history_and_todos、the_first_line_names_a_session_and_renames_and_closes_survive_a_restart；live_sessions::closed_sessions_are_listed_viewed_reopened_and_deleted（已关闭的列出、只读查看、跨重启、重开、删除；host/hub 里有离线的同名用例）；被别的实例占着（`held_by`，要两个实例争一个会话，一个项目只允许一个实例，造不出来）：只有 host/hub 的离线用例 | LIVE（`held_by` 无 LIVE） |
| S-09 | 结束的子 Agent 与任务缺省收起 | app/sessions::lower_tree_three_levels_and_finished_group | mock 场景 |
| S-10 | 选中即切换 | app/crosscut::selection_follows_in_sessions_tab_without_enter；app/side_follow::*；live_tui::the_first_line_in_the_draft_makes_a_session_and_ctrl_o_reads_it | mock 场景 + LIVE |
| S-11 | 顶部面板 Goal & Todo | app/sessions::a_long_panel_scrolls_under_the_wheel_and_never_takes_the_focus、m10_goal_rounds_and_blocked | mock 场景 |
| S-12 | 任务图标 | app/sessions::lower_tree_three_levels_and_finished_group；任务种类来自引擎：live_turns::a_background_task_streams_output_and_can_be_stopped | mock 场景 + LIVE |
| S-13 | 会话列表只列人发起的；第一行新建；启动即草稿 | live_sessions::only_this_projects_sessions_are_listed、live_turns::a_sub_agent_shows_up_as_a_task_with_its_own_session（子 Agent 不进列表）；live_flows::a_run_goes_from_start_to_finish（流程会话不进列表）；app/side_follow::the_workbench_starts_on_a_draft；app/sessions::new_session_row_opens_a_draft_and_the_first_words_create_it | LIVE + mock 场景 |
| S-14 | todo 带编号，完成的在下，面板 10 行 | app/sessions::todos_are_numbered_open_first_done_last | mock 场景 |
| S-15 | `#n` 与 ID | app/side_follow::numbers_follow_the_canonical_order_and_ignore_the_filter；live_tools::a_user_session_manages_jobs_and_names_itself_with_host_tools（`#n` 与 ID） | mock 场景 + LIVE |
| S-16 | 每个会话都有会话、流程、作业的工具；作用域与权限 | live_tools::{a_user_session_manages_jobs_and_names_itself_with_host_tools, a_job_session_cannot_touch_another_job}；host/hub::a_job_fired_session_only_looks_at_jobs_and_nobody_else_touches_it（外面的会话动不了触发里的会话、触发里的会话对作业只读）；host/flows/tools 的单测（运行里的会话不操作运行）；live_flows::a_user_session_validates_and_creates_a_flow_once_the_project_is_trusted、a_user_session_creates_and_runs_a_flow_job_with_the_job_tools | LIVE |
| F-01 | 列出流程定义（名字、来源、运行数） | live_flows::the_builtin_example_flow_is_offered；app/flows_jobs::selecting_a_flow_keeps_the_chat_and_shows_its_design_below | LIVE |
| F-02 | 展开流程看运行 | live_flows::a_run_goes_from_start_to_finish；app/flows_jobs::ended_runs_sit_under_ended_and_show_where_they_came_from | LIVE |
| F-03 | 运行的整棵树：主会话、站、站的任务、子 Agent 与任务，带模型 | live_flows::a_run_goes_from_start_to_finish；app/flows_jobs::m11_run_progresses_live、run_stations_are_flat_colored_and_switch_to_their_context（模型列）；host/hub::session_tasks_gather_the_whole_tree_down_the_sub_agents | LIVE + mock 场景 |
| F-04 | 选中运行切到主会话；站里的会话可进 | live_flows::a_run_goes_from_start_to_finish（主会话归属）；app/flows_jobs::selecting_a_run_follows_its_main_agent_and_shows_stations、run_stations_are_flat_colored_and_switch_to_their_context | LIVE |
| F-05 | 运行的操作在用户会话里说，界面不放键；`m` 放话头 | live_flows::run_ops_release_pause_resume_rework_force_and_confirm、a_flow_with_runs_not_finished_cannot_be_changed_until_they_are_cancelled（工具）；app/flows_jobs::run_and_station_ops_are_talked_about_not_keyed；host/flows::project_runs_span_flows_and_an_ended_run_is_run_again（重跑） | LIVE + mock 场景 |
| F-06 | 启动一次运行（用户会话里说） | live_flows::a_run_goes_from_start_to_finish；live_flows::a_user_session_creates_and_runs_a_flow_job_with_the_job_tools | LIVE |
| F-07 | 流程设计图；会话里设计、校验、保存；有运行时不许改 | live_flows::a_user_session_validates_and_creates_a_flow_once_the_project_is_trusted、a_flow_with_runs_not_finished_cannot_be_changed_until_they_are_cancelled；app/flows_jobs::m12_change_the_flow_by_talking_and_the_design_updates | LIVE |
| F-08 | 看 YAML 与每站配置 | live_flows::the_builtin_example_flow_is_offered（`yaml`、站的 `needs`）；app/flows_jobs::a_station_of_a_definition_opens_its_config_in_an_overlay | LIVE + mock 场景 |
| F-09a | 项目的流程与内置、插件的区分（`◎`）；项目里同名的取代内置的；不读全局目录 | 来源标记：app/flows_jobs::global_flows_are_marked_and_flow_jobs_jump_to_their_flow；live_flows::flows_live_in_the_project_and_a_projects_same_named_flow_replaces_a_builtin_one（项目的为 Project，与内置同名的取代它，`~/.conix/flows/` 里的不列） | LIVE |
| F-09b | 流程格式与 Harnix 兼容 | live_flows::our_session_host_passes_the_conformance_suite | LIVE |
| J-01 | 列作业：触发规则、类型、启用、下次、上次结果 | live_jobs::jobs_are_saved_listed_toggled_and_deleted；app/flows_jobs::jobs_list_details_history_and_follow | LIVE |
| J-02 | 触发历史，跳到那一次的会话 / 运行；简单作业的触发展开成任务树 | live_jobs::run_now_fires_a_job_session_and_records_the_outcome；app/flows_jobs::jobs_list_details_history_and_follow、jobs_show_guards_and_a_firing_is_talked_about_and_unfolds_its_tree | LIVE |
| J-03 | 作业定义、新建修改删除在会话里说 | live_tools::a_user_session_manages_jobs_and_names_itself_with_host_tools；live_flows::a_user_session_creates_and_runs_a_flow_job_with_the_job_tools；app/side_follow::n_and_m_put_a_lead_in_into_the_user_session | LIVE |
| J-04 | 启用 / 停用、立即运行、停下在会话里说，界面不放键 | live_jobs::jobs_are_saved_listed_toggled_and_deleted、run_now_fires_a_job_session_and_records_the_outcome；live_flows::a_flow_job_starts_a_run_when_it_fires；app/flows_jobs::jobs_list_details_history_and_follow（键不再发命令） | LIVE |
| J-05a | 右上一行一个作业；右下定义、统计、触发记录 | live_jobs::jobs_are_saved_listed_toggled_and_deleted、run_now_fires_a_job_session_and_records_the_outcome（触发记录落盘）；app/flows_jobs::jobs_list_details_history_and_follow | LIVE |
| J-05b | 作业属于实例；错过的触发按配置补跑或跳过 | live_jobs::a_missed_firing_is_caught_up_once_only_when_asked、a_firing_left_running_by_a_crash_is_marked_interrupted_on_start、a_firing_cut_short_by_a_stop_is_recorded_as_stopped_by_the_instance | LIVE |
| W-01 | 一次查到所有运行与触发 | host/flows::project_runs_span_flows_and_an_ended_run_is_run_again；host/hub::a_firing_keeps_a_report_that_work_tools_show_list_and_clean_up（work_list）；live_tools::a_user_session_finds_a_firing_with_work_list_and_reads_its_report | LIVE |
| W-02 | 运行、触发的运行时目录与报告 | host/hub::a_firing_keeps_a_report_that_work_tools_show_list_and_clean_up；host/flows::project_runs_span_flows_and_an_ended_run_is_run_again（conix-report.json）；live_tools::a_user_session_finds_a_firing_with_work_list_and_reads_its_report | LIVE |
| W-03 | 会话里清理已结束的 | host/hub::a_firing_keeps_a_report_that_work_tools_show_list_and_clean_up（先列后清、作业会话清不了）；流程运行的清理等 Harnix#70 | 缺 LIVE |
| W-04 | 会话里自检、测试通知 | host/hub::sessions_run_the_self_check_and_send_a_test_notification（两种出口照实报、作业会话不发）；live_tools::a_user_session_runs_the_self_check_and_sends_a_test_notification（只读不问、往外发问人、出口真收到） | LIVE |
| D-01 | `/doctor`：生效配置与引擎自检 | host/hub::doctor_reports_the_engines_checks_and_the_instances_own（离线、假引擎）；live_diagnostics::doctor_reports_the_engines_checks_and_the_instances_own（真实实例的每一项）、doctor_flags_an_inherited_job_network_and_an_unreadable_config | LIVE |
| D-02 | 键位覆盖、冲突启动时报出 | keys 单测（`crates/keys`）；app/keys::*；app/guards | 单测 + mock 场景 |
| D-03 | 一键帮助 | app/keys::the_help_lists_the_focused_context_with_shared_keys_on_one_line、ctrl_h_erases_while_typing_and_is_help_otherwise | mock 场景 |
| D-04 | 数据放 `~/.conix`（`CONIX_HOME`），Agix 数据同处 | 每个 LIVE 用例都用 `CONIX_HOME`（harness 的 `Live::new`）；live_sessions::a_trusted_projects_sessions_are_kept_in_the_project；live_config::a_provider_is_added_tested_switched_to_and_rolled_back_from_a_session（`settings.json` 在 `CONIX_HOME`） | LIVE |
| N-01 | 80×24 可用，120 列并排 | app/workbench::m17_too_small_still_has_footer、m17_wide_shows_both_panes | mock 场景 |
| N-02 | 流式开销与转录长度无关 | app/chat::m16_storm_relayouts_one_block_per_delta | mock 场景 |
| N-03 | IO 不阻塞界面 | host/server::slow_command_does_not_block_heartbeats；app/crosscut::m15_reconnect_shows_state_and_refreshes | 真实引擎离线 + mock 场景 |
| N-04 | CJK、emoji、制表符按显示宽度 | app/chat::m02_hard_text_never_overflows；`crates/tui`、`model` 的排版单测 | mock 场景 + 单测 |
| N-05 | 阶段 2 不需要网络、key、Agix | app/deps::layers_point_one_way；mock 场景全部离线 | mock 场景 |
| N-06 | 常见终端都能用 | 人工（manual-smoke-checklist） | 无（L5） |
| N-07 | 不支持的按键有替代键 | app/keys::the_rare_ones_are_slash_commands、ctrl_g_walks_composer_upper_lower_and_skips_transcript_and_panel | mock 场景 |

需求文档之后做的：

| 项 | 一句话 | 覆盖它的用例 | 层 |
|---|---|---|---|
| 插件 | 装、披露、启用、停用、卸载，插件带来的流程能跑 | live_plugins::a_plugin_from_the_package_dirs_is_installed_enabled_and_its_flow_runs；engine/plugins::a_plugin_is_installed_enabled_and_its_flow_runs_on_its_functions；app/plugins::*（`/plugin` 的界面） | LIVE |
| xforge | 内置的 xforge 流程从 open 到 archive | live_plugins::xforge_quick_goes_from_open_to_archive_with_a_real_model；engine/xforge::xforge_installs_offers_both_flows_and_carries_a_change_from_open_to_archive | LIVE |
| 会话里配置：provider | 加、测、切、退 provider | live_config::a_provider_is_added_tested_switched_to_and_rolled_back_from_a_session；engine/config::an_unreachable_provider_is_added_tested_and_not_switched_to | LIVE |
| 多模型的运行 | 一站生成、一站检视，用不同的 provider | live_models::{deepseek_writes_and_claude_reviews_in_one_run, claude_writes_and_deepseek_reviews_in_one_run}；host/flows::a_station_session_keeps_the_model_it_was_opened_with | LIVE |
| 多模型的返工 | 检视打回、退回生成站，两轮各在自己的模型上 | live_models::the_reviewer_sends_the_draft_back_and_both_rounds_stay_on_their_models | LIVE |
| 子 Agent 用另一家 | 子 Agent 站点名另一家的模型；任务与侧链会话记实际的模型 | live_models::a_sub_agent_station_runs_on_the_other_provider；host::a_sub_agent_task_carries_the_model_that_really_answered；app/sessions::lower_tree_three_levels_and_finished_group | LIVE |
| 会话里配置：`config_show` 与 `conix_config_set` | 看生效配置；改 `conix.yaml`（jobs / notify 当场重读） | live_config_conix::a_session_shows_the_config_and_sets_a_conix_yaml_value | LIVE |
| provider_switched 通知 | 引擎换了 provider，转录里出一条提示 | model/chat（`ProviderSwitched` 的排版）；live_config_conix::a_dead_first_provider_falls_over_and_the_switch_is_pushed（排在前面的连不上，引擎换到下一个） | LIVE |
| 作业网络 | `jobs.defaults.network` 与作业的 `network` 下发到会话沙箱 | live_jobs::a_job_with_no_network_still_runs_local_commands | LIVE |
| 作业预算 | `max_tokens` 下发为会话预算，那次记成被预算停下 | live_jobs::a_job_over_its_token_budget_is_stopped_by_the_engine | LIVE |
| CronCreate | 作业会话里模型不能自建定时任务 | live_jobs::a_job_session_cannot_create_scheduled_tasks | LIVE |
| `budget_exceeded` | 引擎因预算收尾，端口上是 `StopReason::Budget` | live_jobs::a_job_over_its_token_budget_is_stopped_by_the_engine（作业侧）；live_limits::a_turn_over_the_token_budget_ends_as_budget（普通会话里的 `TurnEnded { stop: Budget }`） | LIVE |
| `max_turns` | 一轮里调用次数到顶，端口上是 `StopReason::MaxTurns` | host/src/hub/state.rs 的 `stop_reason_of` 单测；app 的 model/chat 单测；live_limits::a_turn_over_the_model_call_limit_ends_as_max_turns（`turn.max_model_calls`） | LIVE |
| `/doctor`：`conix.session_loosen` | 部署方关了放宽时报降级 | host/hub::doctor_reports_the_engines_checks_and_the_instances_own；live_sessions::the_permission_mode_is_the_sessions_and_takes_effect_at_once（放宽生效）；engine/settings::the_cli_layer_lets_a_session_loosen_its_mode；live_diagnostics::doctor_reports_the_engines_checks_and_the_instances_own（真实实例里是 ok） | LIVE |
| `/doctor`：`conix.jobs_network` | `inherit` 时报降级 | host/hub::doctor_reports_the_engines_checks_and_the_instances_own；live_diagnostics::doctor_flags_an_inherited_job_network_and_an_unreadable_config | LIVE |
| 会话累计用量 | 重启后会话摘要的 usage 来自引擎的 `session.get` 累计 | host/hub::a_watched_sessions_usage_is_what_the_engine_reports；live_sessions::a_sessions_cumulative_usage_survives_a_restart | LIVE |
| 离开期间摘要 | 脱离期间作业失败等，再接入时推一次 `AwaySummary` | host/hub::what_happened_while_detached_is_shown_on_attach；app/flows_jobs::the_away_summary_lists_what_happened_and_enter_goes_there；live_attach::what_happened_while_detached_is_pushed_once_on_the_next_attach | LIVE |
