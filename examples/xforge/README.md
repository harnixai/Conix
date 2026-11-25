# Conix 演示项目：xforge 开发流程

一个很小的 Python 库（`src/textkit`，带规格与测试），用 Conix 内置的 **xforge** 流程包做规格驱动的变更。
`.conix/flows/xforge/` 就是 Conix 内置的那个包的源头：构建时编进 Conix，每个项目里都能用 `xforge/solid`、`xforge/quick`；
这个项目里有同名的包，所以用的是项目自己的这份（改它就是改内置的）。

## 怎么跑

```sh
cp -r examples/xforge /tmp/xforge-demo && cd /tmp/xforge-demo && git init -q && git add -A && git commit -qm base
conix            # 起实例并接入
/trust           # 信任这个项目：读它的流程，命令门禁只在受信项目里跑
```

在对话里说"用 xforge/solid 做一个变更：给 textkit 加 truncate(text, n)，标题 truncate"，或者 `flow_start`。
`solid` 在独立检查与验收两处停下等人：`/confirm` 放行，`/reject <理由>` 驳回；检查留下没解决的问题时，在对话里让 Agent 把运行打回设计（`flow_rework`）。

## 两条路径

| 路径 | 站 |
|---|---|
| `xforge/solid` | 提案 → 设计 → 独立检查（子 Agent，人确认）→ 实施（每个工作包一个子 Agent）→ 验证（人验收）→ 归档 |
| `xforge/quick` | 提案（顺手划工作包）→ 实施 → 验证（人验收）→ 归档 |

| 站 | 交付 | 把关 |
|---|---|---|
| 提案 | `proposal.md`（骨架）、规格增量 `spec-delta.yaml`（`xspec add/modify/remove`） | 制品写好了；提案三节（`check: text`）；增量格式（`check: schema`）；增量对得上基线（`script`） |
| 设计 | `design.md`（骨架）、工作包（`xpkg plan`）、台账（`xledger`） | 设计四节；原则对照（`script`，读 `.conix/governance/constitution.yaml`）；增量再判一次；工作包对得上；台账都有结论 |
| 独立检查 | 发现的问题（`xledger add findings`） | 台账没有 open 的；人确认 |
| 实施 | 代码与测试；工作包结果（`xpkg done`） | 工作包都做完（`check: schema`）；改动都在工作包的路径里（`command`：sh + git）；项目的验证（`command`：`.conix/governance/verify.sh`） |
| 验证 | `verification.md`（骨架） | 每条新增、修改的需求有结论、有带 `# <编号>` 的测试（`script`）；人验收 |
| 归档 | `xspec archive` 把增量并进规格 | 增量都并进去了（`script`） |

规格、增量、台账、工作包都由包的命令写（`flow_command`）：策略 `by-commands` 不让模型直接改这些文件。

## 目录

```
.conix/                     脚手架（进 git）
  conix.yaml                flows.artifacts_dir: xforge —— 制品目录叫 xforge
  flows/xforge/             流程包：bundle.yaml、solid.yaml、quick.yaml、skills/、gates/、scripts/、policies/、templates/、commands/
  governance/               constitution.yaml（项目原则）、verify.sh（项目的验证）
  roles.yaml                确认的角色（reviewer、owner）
xforge/                     制品目录，也就是 xforge 包的数据
  specs/textkit.md          规格基线（进 git）
  archive/<日期序号>-<标题>/  每次变更的归档：proposal.md、design.md、verification.md、spec-delta.yaml、ledgers/、
                            work-packages.yaml、record.json（进 git）
  runs/<运行>/              运行中的目录（不进 git）
src/  tests/                被改的代码
```

在别的项目里用内置的 xforge：准备 `.conix/governance/verify.sh`（可选 `constitution.yaml`），规格在 `conix/xforge/specs/`
（没有就让提案站 `xspec init <能力> <前缀> "<标题>"` 建一份）。只想补几句做法，写 `.conix/flows/xforge/skills/<站>/local.md`
（不放 `bundle.yaml`），它接在内置的 skill 后面。
