# 实例

## 新建与接入

在项目目录里运行 `conix`：这个项目已有实例就接入，没有就新建一个（名字取项目目录名）再接入。也可以显式地：

```sh
conix new [名字]            # 在当前目录新建实例并接入
conix attach <名字>          # 接入
conix attach <名字> --force  # 实例正被别人接着：踢掉对方再接入
conix start <名字>           # 拉起一个停着的实例（照 meta.json 里记的项目）
conix stop <名字>            # 停掉
conix list [--all]          # 列出（--all 连停着的）
```

## 个人项目

家目录不能当项目（它的 `.conix` 就是全局目录 `~/.conix`）。在家目录、它的上级或 `~/.conix` 里运行 `conix`，
进的是**个人项目**：缺省是文档目录下的 `Conix`（macOS `~/Documents/Conix`；Linux 按 `XDG_DOCUMENTS_DIR`，
没有就 `~/Documents/Conix`；Windows `%USERPROFILE%\Documents\Conix`），没有就建。`conix.yaml` 的
`instance.personal_project` 可以换。它和别的项目一样：流程、作业放在它的 `.conix/` 里，`/trust` 之后才读写。
不在某个项目里、又想各处都能用的流程与作业，就放这里。`conix new --project ~` 会被拒绝。

**一个实例同时只给一个 TUI 接入**；空闲（缺省 30 分钟，`conix.yaml` 的 `instance.idle_timeout`）自动脱离，
让出实例。**一个项目同时只有一个实例**：在同一个项目上再起一个会被拒绝，并告诉你该接入哪个。

退出（`Ctrl+X`，或连按两次 `Ctrl+C` / `Ctrl+D`、`/quit`）时选：

- **后台运行**：TUI 退出，实例继续跑（会话、流程、作业都照常）；
- **直接退出**：停掉实例；
- **返回**。

## `conix list`

每行：名字、状态（运行中 / 已停 / 残留）、项目、pid、已运行多久、会话数、等你处理的数、重启过几次、
接入者是谁与它空闲多久。**残留**是进程已经不在了（连看护进程一起被杀、机器重启），列出时顺手清掉，
之后可以 `conix start`。

## 崩溃后自动拉起

`conix host` 先是一个看护进程，它再拉起真正的实例进程。实例被信号杀死（崩溃、`kill -9`）就按
1 秒 / 5 秒 / 30 秒退避重启，10 分钟里超过 5 次就放弃。起来不到 5 秒就自己报错退出的（锁被占、配置错）
不重启——那种情况重启也救不回来，去看日志。重启之前没跑完的作业触发记成"实例停了"，不算作业失败；
运行由流程派工器接着做。

## 日志与自检

- 日志：`~/.conix/instances/<名字>/host.log`，超过 10 MB 轮转，留 3 份旧的。
- 自检：在 TUI 里输入 `/doctor`，列出引擎的检查（设置、provider、hooks、沙箱……）与 Conix 自己的
  （项目状态、`.gitignore`、配置里读不懂的项、通知出口、流程有没有起来、键位覆盖里没生效的）。
