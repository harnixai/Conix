# 流程

流程是按站推进的多步工作（流程库来自 Harnix）。**跑的流程只在项目里**：项目自己的在 `<项目>/.conix/flows/`，
随项目提交，**项目受信才读、才写**（见 `/trust`）；插件带来的是只读的，名字前标 `◎`，项目里存一份同名的就取代它。
全局只有一个**流程库**（`~/.conix/flows/`）：放可以装进各个项目的包，内置的 `xforge`、`xreport` 也在这里，但库里的包不跑，
要用就 `conix flow install <包>` 装进项目（见下面的「管理流程包」）。

## 新建与修改

都在对话里说：每个会话都有流程工具（`flow_create`、`flow_update`、`flow_validate`、`flow_dry_run` ……）。
流程标签里 `n` 放新建的话头、`m` 放选中的那个流程（或运行、站）的话头，接着说要做什么。还有没结束的运行时，
流程不许改、不许删——先取消或等它们跑完。

## 门禁、确认与交付

一站放不放行由它的**出口条件**定。模型说"做完了"不算，下面这些才算：

| 出口 | 判什么 | 谁判 |
|---|---|---|
| `output` | 交上来的输出满足站的输出契约（JSON Schema） | 流程库 |
| `gate` | 门禁：读交付的文件、跑工具，回 `pass` / `fail` / `error` | 流程库、项目的脚本、命令、插件 |
| `confirm` | 有人同意 | 你，或外部审批系统 |
| `children` | 子流程的运行交付了，够 `require` 要的个数 | 流程库 |

**门禁分三类**，放在 `.conix/flows/gates/<名>.yaml`：

- `check`：声明的检查，不写代码。文件在不在、空不空（`files`），JSON / YAML / JSONL / front matter 满足 schema（`schema`），
  文本有没有某段（`text`，正则），前面某站的输出（`output`），过了门禁的文件后来有没有被改（`sealed`）。
- `script`：项目里的 `.conix/flows/scripts/<名>.js`，在流程库自己的 JS 引擎里跑，只能读工作目录里的文件，不能跑命令、
  不能联网；读过哪些文件由运行时记下 hash，作为证据。
- `command`：跑一条命令（测试、lint）。只在受信的项目里跑，`conix.yaml` 的 `flows.command_gates: off` 可以整个关掉
  （见[配置](config.md#conixyaml)）。环境变量按门禁的 `env` 白名单透传，缺省一个不传，模型的密钥永远不传。

门禁的命令、包脚本里的 `flow.exec` 都在**引擎的沙箱**里跑（与会话里的 `Bash` 同一套）：门禁只读，项目里一个文件都写不了，
只有这次调用自己的临时目录（`$TMPDIR`）可写——测试工具要写缓存的，让它写进 `$TMPDIR`（Python 设
`PYTHONPYCACHEPREFIX="$TMPDIR/pycache"`，xforge 的 `verify.sh` 就是这么写的）；要写构建目录的（cargo 的 `target/`），在
门禁上写 `writable: [target]`，用 xforge 就在 `.conix/governance/xforge.yaml` 的 `checks.writable`（项目的验证）或 verify 菜单
那一项的 `writable` 里声明。**只给被 git 忽略的目录**：跟踪着的文件门禁永远写不了。要联网的同样写 `network: true`。缺省不联网（包在 `bundle.yaml` 的 `exec`
里声明才联网），`~/.ssh` 这类凭据读不到。每次起的进程都记进运行历史，带沙箱守住了什么（`enforced`）。macOS 自带沙箱；
Linux 要装 `bwrap`（bubblewrap），没装时这些命令不跑、门禁报错，确实要在沙箱外跑就在全局 `conix.yaml` 写
`flows.exec_unconfined: allow`。macOS 上，门禁的进程自己另起进程组再放到后台的（`setsid`），门禁结束后还会留着（仍在沙箱里，
Agix 在修：harnixai/Agix#113）。

门禁回的结果都一样：`verdict`、`summary`、`findings`（哪个文件、哪一行、哪里不对）、`metrics`，读过的文件连同 hash
记进运行。`fail` 是交付要改：站写了 `on_unmet` 就自动打回，新一轮的会话会拿到 findings。`error` 是门禁没得出结论
（文件读不了、命令起不来、脚本出错）：不自动打回，你看过、修好之后让模型重跑这一站的门禁（`flow_retry` 不带任务）。

**确认只有你能做。** 确认、驳回、放行搁置的站、强制放行，会话里的模型都做不了，它的工具表里也没有这几个。在输入框里打：

| 命令 | 做什么 |
|---|---|
| `/confirm [运行] [站] [理由]` | 确认一站（要多人批的站，你这一票算一票，够数才过） |
| `/reject [运行] [站] <理由>` | 驳回：这条出口不过，按 `on_unmet` 打回或等你说退回哪一站 |
| `/release [运行] [站]` | 放行搁置（`hold: true`）的站 |
| `/force [运行] [站] <理由>` | 不管没满足的条件强制放行（受管流程不能） |
| `/rework [运行] <站> [理由]` | 把运行打回到这一站重做（它和后面的站重来）；理由给重做的站看。这个模型也能做（`flow_rework`） |
| `/rework [运行] <站>/<任务>[,<任务>] [理由]` | 只重做这一站的这几个任务（比如双路只重做实现那一路），别的任务的交付留着 |
| `/worktree [运行] merge \| keep \| discard` | 结束了的 worktree 运行怎么收尾：合进项目当前的分支、只留分支、丢掉（丢掉先问你）。不写就告诉你能做什么 |

运行不写，是流程标签里选中的那次、或左边正对着的那次运行的会话；站不写，是这次运行里唯一在等这件事的站。命令不进会话，
直接发给实例，记成你（操作系统用户）做的。确认写了角色（`who: reviewer`）的，按项目的 `.conix/roles.yaml` 核对：

```yaml
# .conix/roles.yaml —— 角色 → 能确认的操作系统用户名，随项目提交，改它走代码评审
reviewer: [alice, bob]
release: [carol]
```

没列出来的角色任何人都能确认。你不在名单里的站，用 `via: mcp` 让名单上的人在外部审批系统（飞书、钉钉、Jira……）里批：
流程经 Agix 调那个系统的 MCP 工具建单、轮询，批完自动记下审批人和单号，你在 Conix 里不能代批。这要 Agix 的
`mcp.callTool`（harnixai/Agix#111），引擎还没有它时这条出口是 `error`。确认可以写 `timeout`（`3d`），到时没有决定算不过。

**交付有出处。** 每个任务交付时都记下谁交的、什么时候、以什么身份。子 Agent 站要主 Agent 真的派了子 Agent、子 Agent 答完了
才收；子流程声明 `result` 的，交给父运行的就是它；`children` 出口写 `require: any` 或一个数，允许个别子运行失败。

**插件可以把流程声明为受管**（`[x-conix]` 的 `governed`）：项目里不能存同名的去顶替它，它用到的门禁、脚本、规则只取插件
自己的，它的运行不能强制放行。`/plugin` 的披露里写明。

### 让模型写流程

在对话里说要什么流程就行。模型先读流程库的指南（`flow_guide`，门禁、确认、交付各有一章，写门禁前它会读
`flow_guide {topic: "gates"}`），写好后：

1. `flow_gate_try` 拿一份合格的样例和一份不合格的各试一次门禁：合格的要 `pass`，不合格的要 `fail` 并且说中问题；
2. `flow_validate` 校验，`flow_dry_run` 试运行（不开会话、不花钱，门禁是模拟的）；
3. 存进项目（`flow_create`），你说开跑再 `flow_start`。

指南原文在仓库的 `crates/flow/docs/guide.zh-CN.md` 与 `crates/flow/docs/guide/`。

## 流程包、制品目录与内置的包

- **流程包**：一个流程一个目录，`.conix/flows/<包>/`，里面有 `bundle.yaml`、流程文件（同一件事的几条路径可以放在一起，
  比如 `solid.yaml`、`quick.yaml`）和它们用的全部件（`skills/`、`gates/`、`scripts/`、`policies/`、`templates/`、`commands/`）。
  件不跨包共享，整个目录拷到别的项目就能用。流程的名字是 `<包>/<流程>`（`flow.yaml` 就叫包名）。
- **制品目录**：项目下的 `conix/`（项目的 `.conix/conix.yaml` 或全局 `conix.yaml` 的 `flows.artifacts_dir` 可以改名）。
  每次运行在 `conix/runs/<运行>/` 有自己的目录，不进 git；流程包的流程结束后，交付物连同 `record.json`（各站结论、门禁证据、
  谁确认的）归档进 `conix/<包>/archive/`，进 git。制品目录与包同名时（比如 `xforge`），包的数据直接放在它下面。
- **补充一个 skill**：在装进来的包里写 `skills/<名>/local.md`——它接在原 skill 后面（"本项目的补充"），
  `conix flow update` 时带过去，不算对包的改动、不进包。
- **包的字段**：`bundle.yaml` 写 `flows`（流程与选路的先后）、`start`（选路顺序、分诊用的 skill）、`pace`（`auto` 或 `step`）、
  `commands`（哪些子命令只读）。包里的流程名、命令名、站 id 不能重名，`help`、`status` 保留。包里的 `lib/` 放脚本与命令共用的 JS，只能 import 包内的文件。
- **核对归档**：`conix flow verify <归档目录>` 看一份归档在写下之后有没有被改过：文件对着 `record.json` 的清单，
  门禁证据与确认绑定的摘要重算，`record.json` 再对着这台机器上的运行账本。有问题就列出来，退出码非 0。
  `conix flow verify --all` 扫项目里所有的 `conix/*/archive/*/`。
- **内置的 `xforge`**（`conix flow install xforge` 装进项目）：规格驱动的开发流程，按风险三条路径，每条路径几条线：
  `xforge/quick`（只有开发线：提案、实施、验证、归档）、`xforge/solid`（开发线与测试线：规格侧定了分两路，各在自己的 worktree 里，
  一路实现、一路只照规格写黑盒测试，互相看不到，合起来用测试跑实现；检视线只检视两路的设计）、`xforge/major`（先澄清，检视线检视规格侧
  与两路的设计，可以开到整条链）。
  规格在 `conix/xforge/specs/`，站里的会话用 `xspec`、`xarch`、`xledger`、`xpkg` 记规格与接口增量、台账、工作包，归档时增量并进基线。
  项目要准备的：`.conix/governance/verify.sh`（实施站的出口跑它：单测、lint），可选的 `.conix/governance/constitution.yaml`
  （设计逐条对照的原则），`.conix/governance/xforge.yaml` 里：
  - `lines`：三条线的模型，`build`（开发线）、`tests`（测试线）、`review`（检视线）。比如开发线 Claude、测试线与检视线 DeepSeek。
    检视线该与开发线不同：相同（或没写）也照跑，运行的第一站记一条告警、界面提示一次、运行的报告里也有；没写的线用实例的缺省模型；
  - `review.solid`、`review.major`：哪几个检视点由检视线检视。solid 只有设计环节的 `design`、`tests-design`，缺省开；major 缺省开
    `spec`、`design`、`tests-design`，全链条（`code`、`tests`、`verify`）要你手工开。检视线记下问题就自动打回开发线修，再检视一轮确认；
    两轮还没结清停下等你；
  - verify 菜单的 `blackbox` 一项（黑盒测试放哪、怎么跑）：solid、major 合起来那一站跑它。

  合起来没过时会话判错在哪一边，你决定打回哪一路（`/rework fork` 两路都重做，`/rework fork/build` 只重做开发线）。solid、major
  结束后 `/worktree merge` 合进项目。`conix flow doctor` 查这些备齐了没有。完整的例子是仓库里的 `examples/xforge`。
- **内置的 `xreport`**（`conix flow install xreport` 装进项目）：这个项目的提交历史报告，`/flow:xreport <主题> [--since 2026-11-01] [--until …]`
  （都不给就是近 30 天）。统计由 `xstat` 经系统的 git 算，模型照统计写分析，表由统计生成，你确认「报告定稿」；归档在 `conix/xreport/archive/`。
  名字照 git（`.mailmap` 合过）；邮箱缺省不记，提交说明缺省给模型看，`.conix/governance/xreport.yaml` 的 `privacy` 可以改。
  要跑 git，所以项目要受信（`/trust`）。约定见 `docs/design/xreport.md`。

## 管理流程包

只在命令行，不需要实例；在项目目录里跑（或 `-C <项目>`）。改东西的命令先列出要做什么（包、版本从哪到哪、增删改的文件、
备份到哪）再问 `[y/N]`；`--yes` 不问，`--dry-run` 只列不做；在脚本里（不是终端）不给 `--yes` 就拒绝。改了项目的包，
这个项目若有实例在跑，它会重读流程。

| 命令 | 做什么 |
|---|---|
| `conix flow list [--global \| --all]` | 项目的包（缺省）、全局库的、两边对照，带状态：已装、改过、全局库有新版、只在项目、未装 |
| `conix flow show <包> [--global]` | 说明、流程、命令、版本、要求的 Conix、来源 |
| `conix flow install <包> [--jobs ask\|yes\|no]` | 全局库 → 项目；项目里已有同名的用 `update`。包带建议的作业时另问要不要加（见[作业](jobs.md#流程包带来的作业)） |
| `conix flow update [<包> \| --all] [--jobs ask\|yes\|no]` | 全局库的新版 → 项目。项目里改过的、全局库更旧的拒绝，`--force` 才覆盖（先备份）。新版新带来的建议作业同样问 |
| `conix flow backfill <包>` | 项目 → 全局库：项目里改好的包放回库里，别的项目再 `update`。版本没变而内容不同的拒绝：改 `bundle.yaml` 的 `version` |
| `conix flow remove <包> [--global]` | 删项目的（缺省）或库里的，都先备份；项目里有进行中的运行用着它就拒绝，`--force` 才删 |
| `conix flow diff <包>` | 项目的与库里的差在哪：版本、增删改的文件 |
| `conix flow import <文件> [--project]` | 包文件 → 全局库（缺省）或项目 |
| `conix flow export <包> [--global] [-o 文件]` | 项目的（缺省）或库里的 → `<包>-<版本>.zip` |
| `conix flow inspect <文件>` | 只看包文件的 `MANIFEST.yaml` 并核对 |
| `conix flow reset <包> \| --all` | 库里的内置包恢复成这个 Conix 带的那版（当前的先备份） |
| `conix flow clean [--global]` | 清掉备份、暂存的残留、对不上目录的记录 |
| `conix flow doctor [--global] [--fix]` | 体检，见下 |

- **项目记着装了什么**：`.conix/flows.lock.yaml`（进 git）记每个从库里装的包的版本与摘要；拿它判"项目里改过没有"。
  `skills/*/local.md` 不算。
- **内置的包**：Conix 带的 `xforge`、`xreport` 在你第一次运行 `conix` 时放进全局库。库里那份没改过、新的 Conix 带了新版时自动换上
  （打一行提示）；改过的不动，`doctor` 会说有新的内置版，`conix flow reset xforge` 换上（你的那份先备份）。
  `remove --global` 也删得掉它，`reset` 找回。
- **以前直接用内置 xforge 的项目**：升级后 xforge 不再自动出现，`conix flow install xforge` 一条命令装进来。有进行中的运行时，
  实例起来会提示这一句（流程的定义、门禁、脚本钉在运行里，skill 与模板没钉，不装就跑不下去）；`conix flow doctor --fix` 也会装。
- **包文件**：zip，带 `MANIFEST.yaml`（导出它的 Conix、要求、每个文件的 SHA-256）。导入先核对再校验，过了才装；
  包里有 `command` 门禁时，它们只在受信任的项目里运行。`local.md` 不进包；`export`、`import` 不能当包名。

### 体检

`conix flow doctor` 查项目（加 `--global` 查全局库），有错误时退出码非 0：

- 全局库：目录与记录对不上、每个包的校验、这个 Conix 满不满足包要求的版本、内置包没改过 / 改过 / 有新的内置版、暂存残留、备份占了多少。
- 项目：每个包的校验、lock 与目录对不上、包改过、库里有新版、只在项目的包（可以 `backfill`）、遮住了插件的同名包、用了保留名、
  进行中的运行用的包不见了、项目没受信（命令门禁不会跑）、`.gitignore` 里有没有 `conix/runs/` 与 `.wkts/`、包自己的体检
  （`bundle.yaml` 的 `doctor:`）、归档的核对摘要。

`--fix` 只做安全的：清暂存、去掉对不上目录的记录、内容与库里一样却没记进 lock 的补记、缺了的内置包重新放进库里、
进行中的运行用的包库里有而项目没有的装上。别的只报，给出该跑的命令。

## 启动与推进一个流程包

流程包的运行用 `<包>:<名字>` 的地址启动与推进，在输入框里打，不经模型。以内置的 `xforge` 为例：

| 在哪 | 打什么 | 做什么 |
|---|---|---|
| 任何对话 | `/flow:xforge` | 看包的说明：有哪些流程、命令、在跑的运行 |
| 任何对话 | `/flow:xforge <主题> [--risk low] [--step]` | 启动，由包选路（见下） |
| 任何对话 | `/flow:xforge:solid <主题> [--risk medium] [--step]` | 点名流程启动 |
| 任何对话 | `/flow:xforge:xspec list` | 不进运行也能跑包的命令（只读的子命令；会写的在运行里由模型调，你也可以在运行里打） |
| 运行的对话里 | `/xforge` | 这次运行在哪一站、欠什么、等谁 |
| 运行的对话里 | `/xforge:apply` | 对站做事：见下表 |
| 运行的对话里 | `/xforge:check <理由>` | 退回到前面的检查站重做，理由给重做的站看 |
| 运行的对话里 | `/xforge:solid [理由]` | 换到另一条流程（只在第一站还没过时） |
| 运行的对话里 | `/xforge:xspec list` | 跑包的命令，运行就是当前这一次 |

"运行的对话里"指主会话或某一站的会话。不在运行里时只能用带 `/flow:` 的全写，`/xforge:apply` 这样的简写指不到运行。
站、流程、命令的名字都能补全；补全项上标着是哪一种。

**怎么选路。** 点名流程就是它，输入不合它的要求（比如 solid 的 `risk` 只收 low、medium）就拒。只给包名时，包按流程的先后取第一个接受这份输入的；
输入定不了路（没给 `--risk`）时，主会话会先读代码、判复杂度，选好再启动，选的理由记在运行里。做到一半发现选短了，用上表的换路；
第一站过了就换不了，取消重开。

**对站做事。** `/xforge:<站>` 看这一站此刻的状态：

| 这一站 | 会怎样 |
|---|---|
| 当前站，被搁置（步进里每一站都搁置） | 放行，开始干活 |
| 当前站，在干活 | 显示任务与还欠的出口 |
| 当前站，等确认 | 显示，提示用 `/confirm`、`/reject` |
| 当前站，没达标或任务失败 | 重跑：失败的任务重派，没有失败的就重判出口（你改过东西后用） |
| 前面已过的站 | 退回到它重做 |
| 后面的站 | 不跳，告诉你当前站在等什么 |
| 被条件跳过的站 | 告诉你为什么跳过 |

**步进。** 启动时加 `--step`（或包的 `bundle.yaml` 写了 `pace: step`），每一站进站都先搁置，你用 `/xforge:<站>` 一站一站推。放行只有你能做；
步进里模型照样能退回。

**模型的回退建议。** 站里干活的模型觉得该退回前面时，不能自己退，它给出建议：退到哪、为什么。建议记在运行里，显示在运行的对话里与流程标签上。
退到的站必须是流程为这一站定好的回退点。你同意就打 `/xforge:<那一站> [理由]`，等于退回并把建议标成已接受；站往前走或被退回时没接受的建议自动作废。

**多人确认。** 确认出口可以要求几个人批（`min`）、一人一票、驳回规则（缺省一人驳回就不过）；作者缺省也能投，流程要分开时写明作者不能投。
批过之后，确认绑着的文件（缺省是本站及之前各站的交付物）被改了，之后的出站检查与结束前的检查都会不过，要退回那一站重批。

## 运行的操作

也都在对话里说，界面上不放这些键：启动（`flow_start`）、暂停 / 恢复（`flow_pause` / `flow_resume`）、
取消（`flow_cancel`）、重跑一次已结束的（`flow_rerun`：同流程、同主题、同输入）、返工到某一站（`flow_rework`）、
重试失败的任务或重跑门禁（`flow_retry`）。改东西的都会先问你。站上的确认、驳回、放行、强制放行由你用
`/confirm`、`/reject`、`/release`、`/force` 做（见上），模型没有这几个工具；打回你也可以自己打 `/rework <站> [理由]`。

**运行里面的会话只在界面上处理**：选中主会话、站或子 Agent，左边切过去，在那里插话、答它的提问、批它的权限。
别的会话的工具动不了它们（改名、关、删、发消息、代答都不行）；运行里的会话也不能操作自己所在的运行。

所有流程的运行一次查：`work_list`（在跑的、等人的、已结束的，可按流程、状态筛），一次运行的详情与它的运行时
目录：`work_show`。

## 流程标签

上半是流程定义，展开是它**还没结束**的运行；末尾 `▸ 已结束 (n)` 是完成与取消的运行，不分流程、结束的新的
在前，一次取 50 条，`… 更多` 再取。

| 选中的行 | 左边 | 下半 | 键 |
|---|---|---|---|
| 流程定义 | 不动 | 流程设计：节点图，每站的执行方式、skill、出口 | `v` 原始 YAML · 下半选一站 `Enter` 看它的配置 |
| 运行 | 这次运行的主会话 | 整棵树：主 Agent、各站、站的任务、子 Agent 与任务，带模型 | `m` 话头 |
| 已结束的运行 | 同上（在那里说话不影响运行） | 同上；标题写出它跑的定义，改过、删了会注明 | `m` 话头 |

下半选中任一个有会话的节点，左边切到那个会话；Shell、监视器这类任务看它的输出。

站上带条件的出口（`when`，比如"破坏性改动才要主人批"）在站下面各占一行：`⊘` 淡色是这次**不需要**，括号里是判它的依据
（`interface-compat: breaking = false`）；`◇` 是**要**，括号里是为什么要（读到"要"，或判不清按要算）。它们不是门禁、确认的结果，
那些看站行右边的红字与票数。

有哪些内置的流程包、装没装、怎么起，帮助浮层（`Ctrl+H`）翻到「样例」页看。

## worktree

流程写 `workspace: worktree` 的运行不直接改项目，在仓库根的 `.wkts/<运行>/` 里干活（分支
`conix/flow/<运行>`），主仓库的 `git status` 看不到它。运行结束后流程标签里运行下面多一行 `⎇`，写着 worktree 等你定；
`/worktree merge` 合进项目当前的分支（冲突时项目不动，列出冲突的文件），`/worktree keep` 只留分支，`/worktree discard`
丢掉（先问你）。
