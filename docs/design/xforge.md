# 内置流程包 xforge：规格驱动的开发流程

xforge 是 Conix 内置的开发流程包（源头 `examples/xforge/.conix/flows/xforge/`，构建时编进 host），能力对齐 XForge
（`harnixai/xforge`）：两条基线（规格、接口）、三条按风险选的路径、独立检查、按工作包实施、人在几处把关、归档并进基线。
XForge 自己的状态机（`xflow`）、receipt 链、往 `.claude/` 的投影不搬：运行的状态就是流程（flows.md §7），
结构化的数据由包的命令写，判断由门禁做。本文是这个包的约定：数据的形状、命令、门禁、流程。

## 1. 治理开关

项目的 `.conix/governance/xforge.yaml`（进 git，人写）：

```yaml
governance:
  spec: true          # 规格基线：缺省 true
  interface: false    # 接口基线：缺省 false
```

没有这个文件按缺省。关着的那条基线，它的命令仍能读，**写增量的子命令报"这个项目没开接口基线"**，它的门禁回
`pass`、`summary` 写"不欠（governance.interface=false）"。`lib/governance.js` 的 `governance(flow)` 读它，所有脚本与命令都用这一个。

项目的其他治理文件照旧：`.conix/governance/constitution.yaml`（原则）、`.conix/governance/verify.sh`（项目的验证，人写）。

## 2. 数据

`${data_dir}` 是包的数据目录（如 `xforge/`），`${output_dir}` 是这次运行的目录（`xforge/runs/<运行>/`）。

### 2.1 基线（`${data_dir}`，进 git，只有归档写）

- **规格** `specs/<能力>.md`：`# 标题`、`<!-- xspec: prefix TK -->`，每条需求 `### <编号> <标题>` 加正文；编号 `<前缀>-R<n>`，只增不复用。
  正文是 WHEN / THEN 条目（`- WHEN …` / `- THEN …`，或中文 `- 当 … 时` / `- 应 …`），能直接写成黑盒测试。
- **接口** `interfaces/<模块>.yaml`：

```yaml
module: textkit
title: 文本工具
paths: ["src/textkit/**"]           # 这个模块的代码在哪
elements:
  - id: fn:textkit.slugify           # <种类>:<模块>.<名字>；种类 fn | type | endpoint | event | config
    signature: "slugify(text: str) -> str"
    description: 把文本转成 URL 片段
    stability: stable                # experimental | stable | deprecated
```

### 2.2 这次变更的数据（`${output_dir}`，命令写，模型不手写）

| 文件 | 谁写 | 形状 |
|---|---|---|
| `spec-delta.yaml` | `xspec` | `{added: [{id, title, text}], modified: [{id, title?, text}], removed: [id]}` |
| `interface-delta.yaml` | `xarch` | `{modules: [{module, title, paths}], added: [{id, signature, description, stability}], modified: [{id, signature?, description?, breaking}], promoted: [id], deprecated: [id], removed: [id]}` |
| `ledgers/<种类>.yaml` | `xledger` | `{entries: [{id, text, conclusion, refs: [..], note?, by: {kind, id}, history: [{conclusion, by, at, note?}]}]}` |
| `work-packages.yaml` | `xpkg` | `{packages: [{id, title, paths: [glob], after: [id], criteria: [{id: C1, text}], verify?, status, summary?, files?: [path], evidence?: {C1: text}}]}` |

`by` 是命令输入里的 `by`（flows.md §7.6）：`person` / `model` / `station`（站 id）。

### 2.3 台账的种类与结论

| 种类 | 编号 | 结论 | 规矩 |
|---|---|---|---|
| `decisions` | D1… | open → accepted / rejected / resolved / deferred | 离开设计站前都有结论 |
| `risks` | R1… | 同上 | 同上 |
| `findings` | F1… | open / resolved / disputed / rejected / deferred | 至少一个 `--ref`；作者（设计站）只能给 `resolved`、`disputed`（不同意，算没结）；`rejected`、`deferred` 只有检查站或人能给；检查站能重开（`open`） |
| `questions` | Q1… | open / answered / deferred | 澄清站用；**只有人能给结论**（`by.kind == person`），回答写在 note |
| `constitution` | 原则编号（P1） | complies / violates / n-a | 检查站逐条答（`xledger reply`）；`violates` 要 note |

### 2.4 归档

`archive` 站是两个命令步骤：`xspec archive` 把规格增量并进 `specs/`，`xarch archive` 把接口增量并进 `interfaces/`（没开接口基线时
回"没有要并的"）；各自先核对、对不上就不并，两者都过才出站。随后运行结束，流程库把运行目录留下的制品归档进 `${data_dir}/archive/<日期序号>-<标题>/`（已有）。

## 3. 命令

都在 `commands/<名>.js`，`bundle.yaml` 里声明只读的子命令。用法写在文件头的 `//` 注释里（`<命令> help` 看）。

| 命令 | 子命令（**粗体**只读） |
|---|---|
| `xspec` | **list** [能力]、**get** <编号>、**delta**、**check**、**pending**、init <能力> <前缀> "<标题>"、add <能力> "<标题>" "<正文>"、modify <编号> "<正文>" [--title T]、remove <编号>、revert <编号>、archive |
| `xarch` | **modules**、**list** [模块]、**get** <编号>、**delta**、**check**、**pending**、module <模块> "<标题>" --paths a,b、add <模块> <种类> <名字> "<签名>" "<说明>" [--stable]、modify <编号> [--signature S] [--description D] [--breaking]、promote <编号>、deprecate <编号>、remove <编号>、revert <编号>、archive |
| `xledger` | **list** [种类]、add <种类> "<内容>" --ref a,b、set <种类> <编号> <结论> ["<说明>"]、reply <原则> complies\|violates\|n-a "<说明>" |
| `xpkg` | **list**、plan <编号> "<标题>" --paths a,b --criteria "一;二" [--after p1,p2] [--verify "<怎么验证>"]、done <编号> "<做了什么>" --files a,b --evidence "C1=…;C2=…"、blocked <编号> "<卡在哪>" |

- `xspec add/modify` 当场判正文有没有 WHEN / THEN（或"当…时 / 应"），没有就报错不写。
- `xspec pending` / `xarch pending`：同一个包**别的在途运行**（`flow.runs`）的增量动到了哪些编号。`add` 排编号时把它们占的也算上；
  `check` 与门禁把"别的在途运行也在改这一条"报成冲突。
- `xarch modify` 一个 `stable` 元素必须带 `--breaking`（不带就报错，告诉它要么改成兼容的、要么声明破坏）；`remove` 只能删基线里已经
  `deprecated` 的元素；新元素缺省 `experimental`，`--stable` 是补登记已经在用的接口。
- `xledger set/reply` 按 §2.3 的规矩查 `by`，不合就报错说谁能给。

## 4. 门禁

脚本在 `scripts/`，共用的判法在 `lib/`（命令与门禁 import 同一份）。

| 门禁 | 种类 | 判什么 |
|---|---|---|
| `proposal-sections` | check: text | `proposal.md` 有 `## 背景`、`## 目标`、`## 非目标`、`## 改什么`、`## 影响面`、`## 为什么选这条流程`，没有 TODO/TBD |
| `impact` | script | 提案的「影响面」与启动时的 `inputs.impact` 一致：列了的都写到；`impact` 为空时写"无" |
| `spec-delta-shape` | check: schema | 规格增量的形状（关着规格基线时不欠） |
| `spec-delta` | script | 增量对得上基线（已有的规矩）；每条正文有 WHEN/THEN；与别的在途运行不冲突 |
| `interface-delta` | script | 接口增量对得上基线：编号形状、模块存在（或这次 `module` 新建）、`modified/promoted/deprecated/removed` 的元素存在、`removed` 的已 deprecated、stable 的改动都带了 `breaking`；与在途运行不冲突 |
| `interface-compat` | script | 有 `breaking` 的改动时，`inputs.impact` 含 `interface`，且 `decisions` 台账里有一条 refs 指向这个编号、结论 accepted |
| `design-sections` | script | `design.md` 有 `## 做法`、`## 集成点`、`## 失败模式`、`## 迁移与回滚`、`## 被否决的方案`（至少一条 `- 方案：为什么不选`）、`## 原则对照`、`## 工作包` |
| `constitution` | script | 设计逐条对照原则（已有） |
| `packages-planned` | script | 交出的 packages 与 `xpkg plan` 的一一对得上；每包有路径、至少一条判据；`after` 指的包存在、无环；包的路径不重叠 |
| `ledgers-settled` | script | §2.3 的规矩：没有 open / disputed；findings 有 ref；`rejected`/`deferred` 的 findings 是检查站或人给的；questions 是人答的 |
| `constitution-reply` | script | 检查站的 `constitution` 台账覆盖每条原则；`violates` 有说明 |
| `package-check` | script（**任务级出口**） | 这一包：`status: done`（blocked 就不过，说卡在哪）；每条判据有证据；`files` 都在包的路径里；`flow.git.changed()` 里落在这个包路径下的改动都登记在 `files` 里 |
| `packages-done` | check: schema | 工作包都 done |
| `unclaimed` | script | `flow.git.changed()`（基线提交以来，含没提交的）里的每个改动都在某个包的路径里 |
| `unit-tests` | command | 跑 `.conix/governance/verify.sh` |
| `verification` | script | `verification.md` 的「覆盖」里新增、修改的每条需求（和接口增量里新增、修改的元素）一行，有结论、有带 `# <编号>` 的测试；有「连贯性」一节 |
| `archived` | script | 规格增量并进了基线 |
| `interfaces-archived` | script | 接口增量并进了基线 |

## 5. 流程

`bundle.yaml`：`flows: [quick, solid, major]`，`start: {route: [quick, solid, major], triage: triage}`，`pace: auto`。
三条流程的 `inputs` 一样：`title`（必填）、`risk`（**没有缺省**：没给就分诊）、`impact`（数组，取值 interface / security /
data-migration / infra，缺省空）；各自的 `risk` schema 就是 eligibility：quick 只收 low，solid 收 low、medium，major 都收。

| 站 | quick | solid | major | 执行 | 写入范围 | 出口 |
|---|---|---|---|---|---|---|
| propose 提案 | ✓（顺手划包） | ✓ | ✓ | 会话 | 运行目录 | output、proposal-sections、impact、spec-delta-shape、spec-delta、interface-delta；quick 另加 packages-planned |
| clarify 澄清 | | | ✓ | 会话 | 运行目录 | output、ledgers-settled（questions 由人答）、confirm owner |
| design 设计 | | ✓ | ✓ | 会话 | 运行目录 | output、design-sections、constitution、spec-delta、interface-delta、interface-compat、packages-planned、ledgers-settled |
| check 独立检查 | | ✓ | ✓ | 子 Agent，只看不改 | 不能写 | output、ledgers-settled、constitution-reply、confirm reviewer |
| apply 实施 | ✓ | ✓ | ✓ | 每包一个任务（quick 会话，solid/major 子 Agent），按 `after` 排 | `${item.paths}` | 任务级 package-check；站 packages-done、unclaimed、unit-tests |
| verify 验证 | ✓ | ✓ | ✓ | 会话 | 运行目录 | output、verification、unit-tests、confirm owner |
| archive 归档 | ✓ | ✓ | ✓ | 两个命令步骤：`xspec archive`、`xarch archive` | — | output、archived、interfaces-archived |

回退点：每站的 `rework_to` 列出能退到的站；门禁的 `rework_to` 指向问题通常出在哪（如 design 站的 `spec-delta` 不过退回 propose）；
检查站发现问题根在提案时用 `flow_propose` 建议退回提案（子 Agent 写进结果、由主 Agent 提）。确认缺省 `min: 1`，作者也能批；
`binds` 缺省绑本站及之前的制品，批完再改就过不了下一站；major 的澄清只绑 `questions` 台账（设计还要改提案）。

skill：`triage`（判风险与影响面、选路）、`propose`、`clarify`、`design`、`check`、`apply`、`verify`；`archive` 是命令步骤，没有 skill。
模板：`proposal.md`、`design.md`、`verification.md` 按 §4 的小节。

## 6. 演示项目

`examples/xforge/`：Python 小库 `textkit`；`xforge/specs/textkit.md` 是规格基线，`xforge/interfaces/textkit.yaml` 是接口基线（已有的
`slugify`、`word_count` 登记为 stable），`.conix/governance/xforge.yaml` 两条基线都开。`crates/flow/tests/xforge*.rs` 不调模型地
查这个包，`crates/conix/tests/live_xforge.rs` 用真实模型走 solid。
