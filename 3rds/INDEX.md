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
