# 3rds 索引

`3rds/` 里放从别的仓库拷进来的代码。**只有这份索引入库**，代码本身不入库，在本地按下表拷到对应目录。

规矩：

- 原样拷入，本地不改。需要改的，到来源仓库提 issue（标签 `codix`），等对方发布后再重新拷。
- 来源有新版本时重新拷一次，同时更新下表的版本列。

| 本地目录 | 来源仓库 | 来源路径 | 版本 | 用途 | 状态 |
|---|---|---|---|---|---|
| ~~`3rds/harnix-flow/`~~ | [harnixai/Harnix](https://github.com/harnixai/Harnix) | `crates/flow` | `Harnix-v0.4.7-2026111901`（`FLOW_API_VERSION` 1，格式 1） | 流程内核、存储、流程包、派工器与编写工具 | 已分叉到 `crates/flow`，见下 |

Agix 不在这里：它是 git submodule，位于 `agix/`。

## 分叉记录：`crates/flow`

流程库从 `Harnix-v0.4.7-2026111901` 的 `crates/flow` 分叉（`git archive` 原样取出），包名仍是 `harnix-flow`，成为 Conix
workspace 的成员。为什么分叉、改成什么样见 `docs/design/flows.md`。目录里的 `rustfmt.toml` 关掉了格式化，与 Harnix 那份
逐行可比：`diff -r <(git -C ../Harnix archive Harnix-v0.4.7-2026111901 crates/flow …) crates/flow` 看得到的就是全部改动。

每一项改动落地时在这里记一行；往回提给 Harnix（issue 标签 `codix`，标题 `codix-NN：…`）时按这张表整理。

| 改动 | 设计 | Conix 提交 | 已提给 Harnix |
|---|---|---|---|
| 门禁分 check / command / script 三类；统一的门禁结果（verdict、findings、evidence、metrics）与 `CheckStatus::Error`；`rerun_gates`；命令门禁的 `env` 白名单与 `result: json`；打回时把没过的检查告诉新一轮（`StationRec::reworked`）；脚本件 `scripts/*.js` | flows.md §2–§3 | 9e18354 | 否 |
| 确认：`timeout`（`duration_ms`，`tick` 与 `next_expiry` 算进去）、`via: mcp` 外部审批（`Approvals`、`WorkItem::Approval`、`approval_*`，单号记在 `StationRec::approvals`）；`ConfirmRec` 带 `via` 与 `ref`；受管流程（`DirPacks::with_governed`、`Packs::governed`、`Withheld::Governed`、`RunRecord::governed`，不能强制放行） | flows.md §4–§5 | 00d6545 | 否 |
| 交付：`TaskRec::delivery`（`DeliveryRec`）；子 Agent 站的交付先经 `SessionHost::agent_tasks` 核实子 Agent 已答完（`AgentDelivery`、`Engine::submit_from_agent`）；子流程声明 `result`（`FlowResult`）；`children` 出口的 `require`（`Require`：all / any / 至少几个） | flows.md §6 | 0eb4c6e | 否 |
| 试门禁 `flow_gate_try`（`TOOL_GATE_TRY`；同一段代码判、什么都不记）；指南分章 `flow_guide {topic}`（`GUIDE_TOPICS`、`guide_topic`：gates / confirm / deliver），总览指过去；`AuthorHost::guide_notes` 让宿主接上自己的做法 | flows.md §2–§6 | ad5c8bf | 否 |
| 试运行用会话读的流程包（`Sandbox::start_in`）：项目里的流程以前试运行时找不到 | — | 594f782 | 否 |
| 流程包（`bundle` 模块，`<包>/<名>`，自洽、项目里整包代替或加 `local.md`，`Packs::bundle_file`）；制品目录（`ProjectLayout::with_artifacts_dir`，运行的目录与 `_run/` 投影，`${data_dir}`、`RunRecord::data_dir`）；归档（`FlowDef::archive`、`record.json`）；制品声明（`Station::deliverables`、骨架、隐含的 deliverables 门禁）；包的命令（`flow_command`、`Pinned::commands`、可写的 QuickJS）；运行 id 由流程名把 `/` 换成 `.` | layout.md | b73663e | 否 |
| `flow_gate_try` 带 `data_dir`；包里门禁的脚本按包的名字找（校验与试门禁） | layout.md §2 | 1bea032 | 否 |
| 确认计票：`min`、`distinct`、`reject`（`RejectRule`）、`not: [author]`；`binds`（`Binds`）通过时记摘要（`ConfirmRec::binds`、`cond`），之后各站通过前隐含的 `confirm-stale` 检查（保留名 `confirm-binds@<出口>`、`confirm-stale@` 的门禁工作项）；`RunView::votes`（`Votes`） | flows.md §4 | c942e5c | 否 |
| 流程包的 `requires: {conix: ">=x.y.z"}`（`BundleMeta::requires`、`parse_requirement`、`parse_version`）；包名保留 `export`、`import`（`RESERVED_BUNDLES`、`is_bundle_name`）。导出与导入本身在 Conix（`crates/host`） | flows.md §7.12 | — | 否（导出导入 in Conix） |
| 按包启动：`bundle.yaml` 的 `flows`、`start {route, triage}`、`pace`；选路与分诊（`kernel::route`、`Engine::route`、`resolve_start`、`RunRecord::routed`）；换路（`Engine::switch`、`Op::Switch`、`flow_switch`、`RunRecord::switched`）；步进（`Pace`）；门禁的回退点（`ExitCond::Gate.rework_to`、`Station::rework_points`）；建议记进运行（`Engine::propose`、`dismiss_proposal`、`RunRecord::proposals`、`RunView::proposals`） | flows.md（重构稿 §3–§4、§13） | e45b17e | 否（分诊 in Conix） |
| 命令步骤（`Executor::Command`，只认包声明的命令）；`foreach: {script}`（`Foreach`、`WorkItem::Items`、`Engine::foreach_items`）；工作项的 `after`；任务级出口 `Station::task_exit`（`WorkItem::TaskGate`、`Engine::task_gate_result`、`TaskRec::task_exit`、`task.checked`、`Gates::run_for_task`）；`Work::Foreach::items` 改成 `Foreach` | flows.md（M3、M9） | c2f5292 | 否 |
| 归档可事后核对：`record.json` 的 `format`、`files`（`FileEntry`）；`RunRecord::archive`（`ArchiveRef`）与 `Engine::note_archive`；`archive` 模块（`verify`、`ArchiveReport`、`ledger_record`、`list_files`、`hash_file`） | layout.md §3 | 5c77f7e | 否 |
| 命令门禁的 `plan`（`GateDef::plan`、`GateDef::script_part`；`runner/plan.rs`：脚本回门禁结果或 `runs`，按 argv 直接起进程、`cwd` 限在工作目录、总时限）；`command_at` 抽出 `gate_env`、`wait_for`；`script::value_logged` | flows.md §2.2 | 18eca88 | 否 |
| 包的设置按运行钉住：`bundle.yaml` 的 `settings {file, defaults}`（`SettingsDecl`、`read_settings`、`merge`），`Prepared::settings`、`Pinned::settings`（子运行沿用父的、换路保留）；确认出口的 `min`、`reject` 改成 `OrSetting<T>`，`not` 的项与新字段 `separate` 可写 `${settings.<路径>}`，启动时核对；包 JS 的 `flow.settings` | flows.md §4.1、§7.1、§7.7 | 059b4af | 否 |
| 子运行在自己的 worktree 里：`Executor::Subflow::workspace`（只认 `worktree`，父运行要在自己的 worktree 里）；分叉提交（`WorkItem::Fork`、`Engine::subflow_forked`、`StationRec::fork`/`ForkPoint`，`worktree::make` 带起点）；合回父运行的分支（`WorkItem::Join`、`Engine::subflow_joined`，冲突时父任务失败）；`WorktreeRec::fork`（`ForkRec`）、`${parent.output_dir}`、子运行的输出目录 `children/<站>/<任务>/`；`OpenSpec::deny_read` | flows.md §6.5 | fcf0355 | 否 |
| 子运行的 skill 里 `${parent.output_dir}` 换成路径（`refs::parent_output_dir`、`protocol::skill_with_parent_output_dir`）；分叉时子运行自己的输出目录先清空（打回后的新一轮不带前任的目录） | flows.md §6.5 | 80ae1d8 | 否 |
| 子运行的禁读名单在分叉提交时定全：一轮第一次分叉时给这一轮要分叉的每个任务取好子运行的 id（`ForkPoint::children`，`kernel::fork::to_fork`，`max_parallel` 下没轮到的也取），`worktree::make` 带取好的 id（残留收回重建）；子运行分叉时定下兄弟的 worktree（`ForkRec::siblings`、`Forked::children`/`siblings`），`deny_read` 只读它，旧记录照旧现查 | flows.md §6.5 | 5320bd5 | 否 |
| 包的自检：`bundle.yaml` 的 `doctor: <脚本>`（`BundleMeta::doctor`、`doctor_problems`，随包里的流程校验）；`doctor(packs, layout, bundle, project)` 在运行之外为项目跑它，沙箱同脚本门禁（`DoctorReport`、`DoctorFinding`、`DoctorLevel`、`DOCTOR_TIMEOUT`；`Env::for_doctor`、`GitView::in_layout`） | flows.md §7.13 | 92df148 | 否（in Conix） |
| 出口的 `when`（`ExitWhen`：`{gate, fact}` 读这一站或之前一站门禁的事实，`{step, path}` 读命令步骤的输出，`equals` / `in`；`WhenAnswer`）；门禁结果的 `facts`（`GateResult::facts`、`facts_problem`、`CheckRec::facts`，演练的 `Mocks::facts`）；说不清就要（`CheckRec::when`、`WhenRec`，`CheckRec::held_by_when`），不要的记在 `StationRec::not_required`（`NotRequiredRec`），投票表不列；`kernel/when.rs`；`task_exit` 不收 `when` | flows.md §3、§4.2 | da8d48c | 否 |
| `runner/git.rs` 的每次调用 `--no-pager -c core.fsmonitor=false`，读 diff 的加 `--no-ext-diff --no-textconv`：仓库自己的配置里写的程序不跑 | flows.md §7.14 | — | 否 |
| 执行服务：宿主 trait `Exec`（`ExecRequest`、`ExecOutcome`、`ExecProfile`、`Enforcement`、`ExecError`、`OutputKeep`），`runner::PlainExec` / `run_plain` / `exec_record`，`RunnerConfig::exec`、`Runner::exec`；命令门禁与 `plan` 的 run 经它（档位 `read`），每个进程记一条历史 `exec`（`Engine::note_exec`），运行之外记日志；环境变量名以 `*` 开头按后缀匹配（修拒绝表）；`bundle.yaml` 的 `exec {network, env}`（`ExecDecl`、`Pinned::exec`）；包 JS 的 `flow.exec`（档位由调用方定，`ExecScope`）；`doctor_with`、`DoctorExec` | flows.md §7.14 | — | 否 |
