# 3rds 索引

`3rds/` 里放从别的仓库拷进来的代码。**只有这份索引入库**，代码本身不入库，在本地按下表拷到对应目录。

规矩：

- 原样拷入，本地不改。需要改的，到来源仓库提 issue（标签 `codix`），等对方发布后再重新拷。
- 来源有新版本时重新拷一次，同时更新下表的版本列。

| 本地目录 | 来源仓库 | 来源路径 | 版本 | 用途 | 状态 |
|---|---|---|---|---|---|
| `3rds/harnix-flow/` | [harnixai/Harnix](https://github.com/harnixai/Harnix) | `crates/flow` | `Harnix-v0.4.7-2026111901`（`FLOW_API_VERSION` 1，格式 1） | 流程内核、存储、流程包、派工器与编写工具 | 已拷入 |

拷贝用 `scripts/fetch-3rds.sh`：从本地的 Harnix 仓库（缺省 `../Harnix`，`HARNIX_DIR` 可改）按表里的发布标签整目录取出（`git archive`），与检出停在哪里无关。拷进来的 crate 不是 workspace 成员（`exclude`），格式与 clippy 不管它；它自己的测试单独跑：`cargo test --manifest-path 3rds/harnix-flow/Cargo.toml`。

Agix 不在这里：它是 git submodule，位于 `agix/`。
