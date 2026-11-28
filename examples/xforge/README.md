# Conix 演示项目：xforge 开发流程

一个很小的 Python 库（`src/textkit`，带规格、接口登记与测试），用 Conix 内置的 **xforge** 流程包做规格驱动的变更
（约定见 `docs/design/xforge.md`）。`.conix/flows/xforge/` 就是 Conix 内置的那个包的源头：构建时编进 Conix，起来时放进全局流程库
（`~/.conix/flows/`），别的项目 `conix flow install xforge` 装进去；这个项目里本来就有这个包，用的是项目自己的这份（改它就是改内置的）。

## 怎么跑

```sh
cp -r examples/xforge /tmp/xforge-demo && cd /tmp/xforge-demo && git init -q && git add -A && git commit -qm base
conix            # 起实例并接入
/trust           # 信任这个项目：读它的流程，命令门禁只在受信项目里跑
```

在对话里说"用 xforge 做一个变更：给 textkit 加 truncate(text, n)，标题 truncate"，或者打 `/flow:xforge truncate --risk low`。
没给风险时主 Agent 照 `triage` 读代码判风险与影响面，再选路启动；点名也行：`/flow:xforge:solid <主题> --risk medium`。
流程在几处停下等人：`/confirm` 放行，`/reject <理由>` 驳回；停下的站要重做时 `/rework <站> <理由>` 打回
（站会建议打回到哪一站，`/xforge:<站>` 接受建议）。solid、major 在自己的 worktree 里跑，结束后 `/worktree merge` 合进你的分支。

## 三条路径、三条线

**线**是一个角色和它的模型，在 `.conix/governance/xforge.yaml` 的 `lines` 里配：开发线 `build`（提案、设计、代码），
测试线 `tests`（只照规格写黑盒测试），检视线 `review`（检视）。这里开发线用 Claude，测试线与检视线用 DeepSeek。检视线该与开发线
不同（同一个模型检视自己写的不是另一双眼睛）；相同也照跑，第一站记一条告警，运行的报告里也有，`conix flow doctor` 也提醒。
没写的线用实例的缺省模型。`flow_start {flow: "xforge", inputs: {title, risk, impact}}` 按 `risk` 取第一条接受它的：

| 路径 | 收的风险 | 线 | 站 |
|---|---|---|---|
| `xforge/quick` | low | 开发 | 提案（顺手划工作包）→ 实施（每包一个任务）→ 验证（人验收）→ 归档 |
| `xforge/solid` | low、medium | 开发 + 测试 + 检视 | 提案 → 封存（人批「规格侧定了」）→ 分两路（只检视各自的设计）→ 合起来（黑盒测试跑实现，人确认）→ 验证 → 验收 → 归档 |
| `xforge/major` | 都收 | 开发 + 测试 + 检视 | 提案 → 澄清（人答问题）→ 检视规格侧 → 封存 → 分两路（各自的设计带检视）→ 合起来 → 验证 → 验收 → 归档 |

两路是两个子运行，各在自己的 worktree 里、从同一个提交出发，互相看不到：开发线（`xforge/line-build`）照规格设计并实现，
测试线（`xforge/line-tests`）只照规格（WHEN / THEN、接口登记的签名）写黑盒测试。合回运行的分支后，用黑盒测试跑实现。

**检视**（solid 只检视两路的设计，major 从规格侧检视起）：开发线交了一样东西，检视线逐条记发现；有没结的就自动打回开发线那一站，开发线逐条修好标
`resolved`、不同意的标 `disputed` 写理由；检视线再来一轮，核实修好的、认可或驳回不同意的。两轮还没结清就停下等你。

| 检视点 | 检视什么 | major 缺省（solid 只有 design、tests-design，缺省开） |
|---|---|---|
| `spec` | 提案、规格与接口增量：两个互不相见的人照它会不会做出不一样的东西 | 开 |
| `design` | 开发线的设计 | 开 |
| `tests-design` | 测试线的设计：测什么、怎么只经公开接口测 | 开 |
| `code` | 开发线的代码 | 关 |
| `tests` | 测试线写出的测试 | 关 |
| `verify` | 验证记录 | 关 |

全链条检视要你在治理文件里手工开：`review: {major: {code: true, tests: true, verify: true}}`；要检视 solid 没有的那几处就走 major。
检视不替代你：批准破坏性改动、影响面扩大、规格侧定了、两路对上了、验收，都照旧是你的。

`impact`（interface / security / data-migration / infra，缺省空）不改路径：提案的「影响面」要逐项交代，破坏兼容的接口改动要它带 interface，
还要你批「批准破坏性改动」。没声明 interface、接口增量却有改动：提案照实写接口，你批「影响面扩大」。

| 站 | 交付 | 把关 |
|---|---|---|
| 核对三条线（solid、major） | — | 写出三条线的模型；开着检视点时检视线与开发线同一个模型（或没写）只告警 |
| 提案 | `proposal.md`（六节）、规格增量（`xspec`）、接口增量（`xarch`）；quick 另有工作包（`xpkg plan`） | 提案六节；影响面与声明、增量一致（增量多出来的你批「影响面扩大」）；增量的形状、对得上基线、与别的在途运行不冲突；破坏兼容有决定 |
| 澄清（major） | 会改变范围的问题（`xledger add questions`），人答 | 问题都是人答的；人确认 |
| 检视（`review-*`） | 发现（`xledger add findings`）；设计阶段的逐条答原则（`xledger reply`） | 这一站记的发现都结了；原则都答了 |
| 封存 | 命令步骤 `xspec seal`：规格侧每个文件的摘要，交给两个子运行 | 人确认「规格侧定了」，绑住提案与两份增量 |
| 两路的设计 | `design.md`（七节）、工作包、决定与风险 | 七节与被否决的方案；原则对照；工作包对得上；台账都有结论；规格侧没被改（spec-unchanged） |
| 实施 | 代码与测试；每包登记（`xpkg done`） | 每包交付时：做完、判据有证据、改动都登记了；站：包都做完、没有无人认领的改动、项目的验证 |
| 合起来 | 跑 verify 菜单的 `blackbox`，没过的记成发现、写明错在实现还是测试 | 黑盒测试都过、台账结清；人确认 |
| 验证 | `verification.md`：覆盖、连贯性 | 每条需求与元素有结论、有带 `# <编号>` 的测试；项目的验证 |
| 验收 | — | 人验收（quick 在验证站） |
| 归档 | 命令步骤 `xspec archive`、`xarch archive`，不用模型 | 规格与接口增量都并进了基线 |

规格、接口、增量、台账、工作包都由包的命令写（`flow_command`）：策略 `by-commands` 不让模型直接改这些文件；各站只能写运行目录，
实施的每个包只能写它的路径，检视站只看不改。

黑盒测试放在 `tests_blackbox/`：verify 菜单里 `blackbox` 一项跑它，两路都照这一项认测试的目录（测试线只写那里，开发线不碰）。
里面已经有几条照规格写的、只经公开接口调用的测试。合起来没过时，`/rework fork <理由>` 两路都重新分叉；只想重做一路，
`/rework fork/build`（或 `fork/tests`）。重做的那一路看得见另一路上一轮合回来的东西。

## 提交历史报告：xreport

`.conix/flows/xreport/` 是 Conix 内置的另一个包（约定见 `docs/design/xreport.md`）：这个项目的 git 历史在一段时间里的报告。
在仓库里提交过几次之后打 `/flow:xreport 近一个月`，或给范围 `/flow:xreport 十一月 --since 2026-11-01 --until 2026-11-30`。

| 站 | 做什么 | 把关 |
|---|---|---|
| 统计 | 命令步骤 `xstat collect`：经系统的 git 数提交、增删行、文件，按作者、星期、小时（作者自己的时区）、目录、文件，写 `stats.json` | 形状对 |
| 分析 | 会话只读 `stats.json`，写 `analysis.md`：概览、贡献者、时间分布、热点、观察 | 五节都在、都有正文 |
| 定稿 | 命令步骤 `xstat render`：头、分析、从统计生成的表，拼成 `REPORT.md` | 你确认「报告定稿」 |

名字照 git（`.mailmap` 合过）；邮箱缺省不记，提交说明缺省给模型看，在 `.conix/governance/xreport.yaml` 里改
（`privacy: {emails: true}`、`privacy: {subjects: false}`）。它要跑 git，项目没受信时统计站会说先 `/trust`。

## 目录

```
.conix/                     脚手架（进 git）
  conix.yaml                flows.artifacts_dir: xforge —— 制品目录叫 xforge
  flows/xforge/             流程包：bundle.yaml、quick/solid/major.yaml、line-build/line-tests.yaml（两路）、skills/、gates/、scripts/、lib/、policies/、templates/、commands/
  flows/xreport/            流程包：bundle.yaml、flow.yaml（统计、分析、定稿）、commands/xstat.js、lib/、gates/、scripts/、skills/、templates/
  governance/               xforge.yaml（开哪几条基线、确认要不要职责分离、检视点、三条线的模型、工作包能选的 verify 菜单；整个文件运行启动时钉住）、constitution.yaml（项目原则）、verify.sh（项目的验证）
  roles.yaml                确认的角色（reviewer、owner）
xforge/                     制品目录，也就是 xforge 包的数据
  specs/textkit.md          规格基线（进 git）
  interfaces/textkit.yaml   接口基线（进 git）
  archive/<日期序号>-<标题>/  每次变更的归档：提案、设计、验证、增量、台账、工作包、record.json（进 git）
  xreport/archive/          每份提交历史报告的归档：REPORT.md、analysis.md、stats.json、record.json（进 git）
  runs/<运行>/              运行中的目录（不进 git）
src/  tests/                被改的代码与单测
tests_blackbox/             黑盒测试（verify 菜单的 blackbox；双路开发的测试那一路写在这里）
```

在别的项目里用内置的 xforge：`conix flow install xforge`，再准备 `.conix/governance/verify.sh`（可选 `constitution.yaml`、
`xforge.yaml`；`conix flow doctor` 会说缺什么），规格在 `conix/xforge/specs/`（没有就让提案站 `xspec init <能力> <前缀> "<标题>"` 建一份）。
只想补几句做法，写 `.conix/flows/xforge/skills/<站>/local.md`，它接在 skill 后面，`conix flow update` 时留着。
