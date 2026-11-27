# 内置流程包 xreport：这个项目的提交历史报告

xreport 是 Conix 内置的第二个流程包（源头 `examples/xforge/.conix/flows/xreport/`，与 xforge 一样构建时编进 host、起来时放进全局流程库，
`conix flow install xreport` 装进项目）：一段时间里谁提交了什么、改了多少行、时间怎么分布、热点在哪。只看这个项目自己的 git 历史；
数由系统的 git 算、由命令整理，模型只照这些数写文字，最后由人定稿。

## 1. 站

流程只有一条（`flow.yaml`，地址 `/flow:xreport`），三站：

| 站 | 执行 | 交付 | 出口 |
|---|---|---|---|
| `collect` 统计 | 命令步骤 `xstat collect`，不用模型 | `stats.json` | 输出；`stats-shape`（`check: schema`） |
| `analyze` 分析 | 会话，skill `analyze`；只能写运行目录 | `analysis.md`：概览、贡献者、时间分布、热点、观察 | 输出；`analysis-sections`（`check: text`，五节都在）；`analysis-filled`（每节有正文，没有占位的 TODO/TBD/待补）；不过就退回本站，至多两次 |
| `publish` 定稿 | 命令步骤 `xstat render`，不用模型 | `REPORT.md` | 输出；确认「报告定稿」（`owner`，`separate` 读设置，绑住 `REPORT.md`）；驳回退回分析站 |

- 输入：`since`、`until`（`YYYY-MM-DD`，都可以不写：截止日缺省是本机的今天，起始日缺省是截止日往前 `range.days` 天）、`rev`（缺省
  `HEAD`）、`paths`（缺省整个仓库）。输入的 schema 先挡一遍形状，`xstat collect` 进 argv 之前再逐项核对（§4）。
- 归档留 `REPORT.md`、`analysis.md`、`stats.json` 与 `record.json`，在包的数据目录 `<制品目录>/xreport/archive/<日期序号>-xreport/`。
- 分析的文件叫 `analysis.md` 而不是 `report.md`：大小写不分的文件系统（macOS 缺省）上 `report.md` 与 `REPORT.md` 是同一个文件，
  定稿会盖掉分析。
- 统计不重跑：主人驳回时退回的是分析站。范围要换就取消重开，统计是对着那一刻的仓库算的。

## 2. 数据：`stats.json`

`xstat collect` 写，形状由门禁 `stats-shape` 守。分析站只读它，`REPORT.md` 的表只从它生成。

| 字段 | 内容 |
|---|---|
| `range` | `since`、`until`、`rev`、`head`（rev 指的提交）、`paths`；`by: commit-date` |
| `privacy`、`limits`、`truncated`、`partial` | 这次按什么设置统计；提交超过上限时 `truncated`；个别提交的 numstat 超过 4 MiB 被截时 `partial` 记几个 |
| `totals` | 提交（不含合并）、合并、作者、增删行、文件、二进制改动、活跃天数、最早与最晚 |
| `authors` | 每人：名字（邮箱只在设置许可时）、提交、合并、增删行、文件、活跃天数、首次、最近 |
| `by_weekday`、`by_hour`、`by_day` | 按作者自己的时区（`%aI` 的偏移）：星期一到日、0–23 点、日期 |
| `by_dir`、`top_files` | 顶层目录与改动最多的文件（至多 `limits.files` 个），按增删行排 |
| `commits` | 给模型的清单：新的在前，至多 `limits.listed` 条；sha（12 位）、作者、时刻、增删行与文件数、说明（设置许可时）；合并标 `merge` |

- **怎么数**：`git log --no-merges -z --numstat -M`，作者用 `%aN`（git 照 `.mailmap` 合并）。合并提交另用 `git log --merges` 数个数，
  不算行数：它的 numstat 是两边之差，算进去就把别人的改动记到合并的人头上。二进制文件的 numstat 是 `-`，算一次改动、不算行。
  改名的新旧路径分开收，按新路径记。
- **范围按提交时间**，与 `git log --since/--until` 一样（本机时区那一天的开始与结束）；时间分布按作者时间。为什么不自己按作者时间筛：
  那要把全部历史取出来再筛，而 git 的这条规矩人本来就认得。
- **上限**：按批取（一批 2000 个），一批的输出超过 `flow.exec` 的 4 MiB 就对半再取；至多 `limits.commits` 个（缺省 20000），多了记
  `truncated`。一次 `xstat` 的时限是 30 秒（包的命令的时限），大仓库要缩小范围。

## 3. 设置

项目的 `.conix/governance/xreport.yaml`（`bundle.yaml` 的 `settings`，启动时读、钉进运行；没有就是缺省）：

```yaml
range: {days: 30}                         # 不给 since 时往前看几天
privacy: {subjects: true, emails: false}  # 提交说明给不给模型；记不记邮箱
limits: {commits: 20000, listed: 300, files: 20}
review: {separate: false}                 # 定稿要不要别人批
```

类型不对（`emails: "no"`、`days: 0`）时 `xstat collect` 拒绝、doctor 报 `error`，不回落到缺省：写错了却悄悄按缺省统计，报告就不是人要的那份。

## 4. 安全与隐私

- **只用系统的 git，只读。** `xstat` 经 `flow.exec`（flows.md §7.14）跑 `git`，只收 argv、没有 shell；命令步骤的档位是 `workspace`，
  但 git 只读。每次都关掉仓库配置里能让 git 起别的程序的几处：`core.fsmonitor`、分页、外部 diff 与 textconv、签名校验（gpg）；
  `log.follow` 也关掉，免得只给一个路径时跟着改名走出那个路径。不声明 `exec.network`，进程不带模型的密钥（宿主的拒绝表）。
- **进 argv 之前逐项核对**：日期是日历上的一天；`rev` 是分支、标签、sha 的形状（可以带 `~n`、`^n`），不以 `-` 开头，不收 `..`（范围）与
  `@{…}`（reflog），git 那边再放 `--end-of-options`；路径相对项目根、没有 `..`、不以 `-` 或 `:`（pathspec 的魔法）开头，放在 `--` 之后。
  认不出的选项报错。
- **只在受信的项目里、`flows.command_gates` 没关时跑**（执行的总开关）。不行时 `xstat` 说中文的理由与该做什么：项目没受信就 `/trust`，
  开关关着就改回 `trusted`。
- **名字照 git。** 报告里的作者就是 git 里的名字（`.mailmap` 合过），不起代号：进了 git 的不是秘密。**邮箱缺省不记**：`privacy.emails`
  为 `false` 时连向 git 要都不要，`stats.json`、分析、`REPORT.md` 里都没有。**提交说明缺省给模型**（`privacy.subjects: true`），关掉后
  `stats.json` 里没有说明，模型只凭数写。
- 模型只读 `stats.json`：skill 说不读源码、不跑 git，每个数都要能在 `stats.json` 里找到；分析站只能写运行目录。

## 5. 命令与自检

```
xstat collect [--since D] [--until D] [--rev R] [--path P]…   统计，写 stats.json；不给的取运行的输入、再取设置
xstat render                                                  由 stats.json 与 analysis.md 生成 REPORT.md
xstat show [<stats.json>]                                     摘要（只读）；运行之外缺省是最近一次归档的
```

- `render` 是确定的：不看时钟（头里的时刻是 `stats.json` 的 `generated_at`）、不用模型、千分位自己加，同样的两份永远同样的
  `REPORT.md`，人批的就是能重算出来的那份。表里的名字、路径转义 `|`。
- `scripts/doctor.js`：git 经 `flow.exec` 起得来（没受信、开关关着是 `error`；宿主没给执行服务只是 `warn`，判不了）；项目是 git
  仓库且有提交；设置的每一项类型对；有 `.mailmap` 报 `info`。只读、不修。
- 演示项目 `examples/xforge` 里就有这个包；`crates/flow/tests/xreport.rs` 在临时仓库上把这些都跑一遍。
