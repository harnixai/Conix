# Conix 演示项目：xforge 开发流程

一个很小的 Python 库（`src/textkit`，带规格、接口登记与测试），用 Conix 内置的 **xforge** 流程包做规格驱动的变更
（约定见 `docs/design/xforge.md`）。`.conix/flows/xforge/` 就是 Conix 内置的那个包的源头：构建时编进 Conix，每个项目里都能用；
这个项目里有同名的包，所以用的是项目自己的这份（改它就是改内置的）。

## 怎么跑

```sh
cp -r examples/xforge /tmp/xforge-demo && cd /tmp/xforge-demo && git init -q && git add -A && git commit -qm base
conix            # 起实例并接入
/trust           # 信任这个项目：读它的流程，命令门禁只在受信项目里跑
```

在对话里说"用 xforge 做一个变更：给 textkit 加 truncate(text, n)，标题 truncate"，或者打 `/flow:xforge truncate --risk low`。
没给风险时主 Agent 照 `triage` 读代码判风险与影响面，再选路启动；点名也行：`/flow:xforge:solid <主题> --risk medium`。
流程在几处停下等人：`/confirm` 放行，`/reject <理由>` 驳回；检查留下没解决的问题时，`/rework design <理由>` 打回设计
（检查站会建议打回到哪一站，`/xforge:<站>` 接受建议）。

## 三条路径

`flow_start {flow: "xforge", inputs: {title, risk, impact}}` 按 `risk` 取第一条接受它的：

| 路径 | 收的风险 | 站 |
|---|---|---|
| `xforge/quick` | low | 提案（顺手划工作包）→ 实施（每包一个任务）→ 验证（人验收）→ 归档 |
| `xforge/solid` | low、medium | 提案 → 设计 → 独立检查（子 Agent，人确认）→ 实施（每包一个子 Agent）→ 验证（人验收）→ 归档 |
| `xforge/major` | 都收 | 提案 → 澄清（人答问题、确认）→ 设计 → 独立检查 → 实施 → 验证 → 归档 |

`impact`（interface / security / data-migration / infra，缺省空）不改路径：提案的「影响面」要逐项交代，破坏兼容的接口改动要它带 interface。

| 站 | 交付 | 把关 |
|---|---|---|
| 提案 | `proposal.md`（六节）、规格增量（`xspec`）、接口增量（`xarch`）；quick 另有工作包（`xpkg plan`） | 提案六节；影响面与声明一致；增量的形状、对得上基线、与别的在途运行不冲突 |
| 澄清（major） | 会改变范围的问题（`xledger add questions`），人答 | 问题都是人答的；人确认 |
| 设计 | `design.md`（七节）、工作包、决定与风险（`xledger`） | 七节与被否决的方案；原则对照；增量再判一次（不过退回提案）；破坏兼容有决定；工作包对得上；台账都有结论 |
| 独立检查 | 发现（`xledger add findings`）、逐条答原则（`xledger reply`） | 台账没有悬着的；原则都答了；人确认 |
| 实施 | 代码与测试；每包登记（`xpkg done`） | 每包交付时：做完、判据有证据、改动都登记了；站：包都做完、没有无人认领的改动（不过退回设计）、项目的验证 |
| 验证 | `verification.md`：覆盖、连贯性 | 每条需求与元素有结论、有带 `# <编号>` 的测试；项目的验证；人验收 |
| 归档 | 命令步骤 `xspec archive`、`xarch archive`，不用模型 | 规格与接口增量都并进了基线 |

规格、接口、增量、台账、工作包都由包的命令写（`flow_command`）：策略 `by-commands` 不让模型直接改这些文件；各站只能写运行目录，
实施的每个包只能写它的路径。

## 目录

```
.conix/                     脚手架（进 git）
  conix.yaml                flows.artifacts_dir: xforge —— 制品目录叫 xforge
  flows/xforge/             流程包：bundle.yaml、quick/solid/major.yaml、skills/、gates/、scripts/、lib/、policies/、templates/、commands/
  governance/               xforge.yaml（开哪几条基线、确认要不要职责分离；运行启动时钉住）、constitution.yaml（项目原则）、verify.sh（项目的验证）
  roles.yaml                确认的角色（reviewer、owner）
xforge/                     制品目录，也就是 xforge 包的数据
  specs/textkit.md          规格基线（进 git）
  interfaces/textkit.yaml   接口基线（进 git）
  archive/<日期序号>-<标题>/  每次变更的归档：提案、设计、验证、增量、台账、工作包、record.json（进 git）
  runs/<运行>/              运行中的目录（不进 git）
src/  tests/                被改的代码
```

在别的项目里用内置的 xforge：准备 `.conix/governance/verify.sh`（可选 `constitution.yaml`、`xforge.yaml`），规格在 `conix/xforge/specs/`
（没有就让提案站 `xspec init <能力> <前缀> "<标题>"` 建一份）。只想补几句做法，写 `.conix/flows/xforge/skills/<站>/local.md`
（不放 `bundle.yaml`），它接在内置的 skill 后面。
