# Conix

Conix（Console + *nix）是 Agent native 的 console shell：跑在终端里的 agent 工作台，引擎是 [Agix](https://github.com/harnixai/Agix)。可以在本地用，也可以远程用，不限定于编程场景。

- 左边是对话区。
- 右边是三个标签：
  - **会话**：主 Agent，下面挂着 goal / todo / 子 Agent / 任务
  - **流程**：流程定义，以及它的运行实例
  - **作业**：定时任务

Conix 不跟 Claude Code 这类聊天式编程 CLI 拼打磨。它的位置是 Agix 的参考宿主，把 Agix 独有的能力摆到用户面前：一个进程跑多个会话、无人值守、子 Agent 与任务可观测、按流程执行。

需求见 `docs/reqs/`，架构见 `docs/design/`。阶段 1 产出这两份，之后以它们为准。

## 开发顺序：先打桩，后接缝

1. **阶段 0**：仓库与约定。
2. **阶段 1**：需求与设计文档，不写代码。
3. **阶段 2**：TUI 接 **mock 后端**。不碰 Agix，也不碰流程；数据来自脚本化的场景，各种交互全部验证通过才算完。
4. **阶段 3**：接缝。按块把 mock 换成真实后端（实例进程、Agix 协议客户端、流程、作业），mock 保留下来给测试用。

**阶段 2 没验收，不接缝。** 每个阶段结束都要输出简报，由维护者决定是否进入下一阶段。

## 已定的边界

- **常驻实例 + 可接入的 TUI。** 实例是本机上一个常驻的 `conix host` 进程，引擎、流程、作业都跑在里面。TUI 通过本机 unix socket 接入实例，退出 TUI 就是脱离。**一个实例同时只允许一个 TUI 接入**：可以强制接入（踢掉对方），不做空闲超时脱离。可以用 `conix list` 查看实例、`conix attach` 接入。
- **不监听任何网络端口。** 远程使用就是 ssh 登录到那台机器后运行 `conix`，安全交给 ssh 和操作系统。
- **引擎只走 Agix 的 JSON-RPC 协议**（`agix/docs/daemon_rpc_protocol.md`）：实例进程内部用 `agix::embed` 嵌入引擎，通过 duplex 通信。不使用 `agix::pool` 这一层 Rust 门面，因为它不承诺兼容。唯一的例外是启动时的装配：`EmbedConfig` 本来就是 Rust 的，cli 层设置与给插件的宿主 JS 库（`conix.*`，`js_libs`）在这里登记（`docs/design/plugins.md` §5.1）。
- **项目级的放项目，全局的放全局。** 会话、流程、作业都是项目的，放在项目的 `.conix/`（Agix 的项目层目录）里：定义（流程、作业）进 git，本机数据与过程数据在 `.conix/state/` 下、不进 git，Conix 自己的在 `state/conix/`。全局目录是 `~/.conix`（可以用 `CONIX_HOME` 改），Agix 的 `data_root` 也指向这里，不使用 `~/.agix`；它只放用户级的配置（`conix.yaml`、键位等）和实例的运行时文件（`instances/`）；没有全局的流程与作业，要各处都用的放进个人项目（`<文档目录>/Conix`），不往 Agix 的 `settings.json` 里写东西。布局见 `docs/design/architecture.md` §8.1。
- **TUI 与后端之间只有一个端口**：`Instances` 和 `Backend` 两个 trait 加上领域事件，定义见 `docs/design/architecture.md` §3。mock 与真实实现走的是同一个端口；端口的消息序列化之后就是 TUI 与实例之间的协议。
- **流程来自 Harnix**：从 Harnix-v0.4.7 分叉到 `crates/flow`，在 Conix 里改（门禁、确认、交付的治理，`docs/design/flows.md`），稳定后整理成 issue 提给 Harnix。分叉前的流程定义照旧能读。流程不放进 Agix。
- **不做**：ACP（已作为需求提给 Agix：harnixai/Agix#74）、Web 界面。

## 仓库布局

```
agix/          Agix submodule，跟 main
crates/flow/   流程库，分叉自 Harnix；不跟 Conix 的格式化（目录里的 rustfmt.toml）
3rds/          拷进来的第三方代码；只有 INDEX.md 入库
docs/reqs/     需求规格
docs/design/   架构与交互设计
examples/demo/ 演示项目：流程（三类门禁、治理文件、多模型并行、人与 MCP 两种授权、子流程）与作业都有，LIVE 测试照原样跑它
archive/       旧实现 AttaCode 的本地副本，不入库，只作参考
.local/        过程文档（分析、评估、草稿），不入库
```

## 子模块 `agix/`

- 跟 `origin/main`，不钉版本。gitlink 记录的仍然是具体 SHA，所以任何一次 checkout 都能复现。
- 跟到最新：`git submodule update --remote --merge agix`，然后 `git -C agix checkout main`，不要停在 detached HEAD。
- 升级之前读两个 Agix 标签之间的标签消息，嵌入面有变化的发布会单列出来。
- Agix 本身不在这里改。缺什么就到 harnixai/Agix 提 issue：标签 `codix`，标题 `[Codix NN] …`。

## `3rds/`：拷贝来的代码

- `3rds/INDEX.md` 记录每份代码的来源仓库、路径、版本、用途。代码本身不入库，按索引拷到本地。
- **原样拷入，本地不改。** 需要调整就到来源仓库提 issue，等它发布后重新拷。Harnix 的 issue 标签是 `codix`，标题 `codix-NN：…`。
- 来源有新版本时重新拷一次，并更新索引里的版本。
- 现在 `3rds/` 里没有代码：流程库原先在这里，已分叉到 `crates/flow`。分叉的基线与每一项改动记在 `3rds/INDEX.md` 的分叉记录里，往回提给 Harnix 时按它整理。

## 旧实现

`archive/attacode/` 是 AttaCode 在 `acc3ca4` 的代码；`archive/attacode.git` 是它原来的 git 目录。远端是 harnixai/AttaCode，不再改动。

旧代码可以拿来参考，**需要哪段就拷哪段**，不整体搬。旧实现踩过的坑写在 `docs/design/` 里。

## 文档

- `docs/` 只放两类：需求与设计（系统怎么构建、为什么），使用手册与规范。
- 状态分析、对比、评估、迁移进度这类过程文档不入库，放在 `.local/` 或 scratchpad。结论要落到代码、提交消息或 `docs/` 的设计里。

## 版本与提交

- **每次推送到远程就是一次发布，必须带版本标签**（与 Agix 一致）。
- 版本标签：`Conix-v<x.y.z>-<YYYYMMDD><NN>`。每次发布 `z` 加 1；`NN` 是当天第几次发布，从 `01` 开始，
  用 `git tag -l 'Conix-v*-<YYYYMMDD>*'` 查当天已有的取最大加 1。
- `Cargo.toml` 的 `[workspace.package] version` 写成 `x.y.z-YYYYMMDDNN`，不带程序名前缀。界面上只显示 `x.y.z`。
- 发布步骤：改版本号并 `cargo update -w` 同步 `Cargo.lock`；`scripts/check.sh` 全过（发布前跑一遍 `scripts/live.sh`）；
  版本号改动单独一个提交；推送 `main`，再打附注标签（标签消息写本次发布的要点）并推送标签。
- 提交前缀：`Conix:`。小步提交，每一步都要能构建。
- 子模块更新单独提交，消息为 `Conix: bump Agix to <sha>`。

## 代码风格

- `cargo fmt`（不带 `--all`：`--all` 会连 agix 子模块一起格式化）；`crates/flow` 不格式化，新代码照它原有的风格写；`cargo clippy --workspace --all-targets -- -D warnings`。一批改动做完跑 `scripts/check.sh`。
- 注释写"为什么不能写成看起来更自然的那样"，不复述代码。变更经过写进提交消息。
