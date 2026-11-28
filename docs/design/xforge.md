# 内置流程包 xforge：规格驱动的开发流程

xforge 是 Conix 内置的开发流程包（源头 `examples/xforge/.conix/flows/xforge/`，构建时编进 host，起来时放进全局流程库，`conix flow install xforge` 装进项目），能力对齐 XForge
（`harnixai/xforge`）：两条基线（规格、接口）、三条按风险选的路径、每条路径几条线（开发、测试、检视，各配各的模型）、
按工作包实施、人在几处把关、归档并进基线。
XForge 自己的状态机（`xflow`）、receipt 链、往 `.claude/` 的投影不搬：运行的状态就是流程（flows.md §7），
结构化的数据由包的命令写，判断由门禁做。本文是这个包的约定：数据的形状、命令、门禁、流程。

## 1. 治理开关

项目的 `.conix/governance/xforge.yaml`（进 git，人写）：

```yaml
governance:
  spec: true          # 规格基线：缺省 true
  interface: false    # 接口基线：缺省 false
review:
  separate: false     # 职责分离：缺省 false，作者也能批
  major: {spec: true, design: true, tests-design: true, code: false, tests: false, verify: false}   # 检视点（§5.2），这是缺省
  solid: {spec: false, design: false, tests-design: false, code: false, tests: false, verify: false}
lines:                # 三条线的模型（§5.1）：缺省都不写，用实例的缺省模型
  build: {model: claude-sonnet-5-5}   # 开发线；可另写 provider
  tests: {model: deepseek-chat}       # 测试线
  review: {model: deepseek-chat}      # 检视线：要与开发线不同
```

它是包的设置（`bundle.yaml` 的 `settings`，flows.md §7.1），缺省写在 `bundle.yaml` 里。整个文件**运行启动时读一次、钉进运行**：
运行里改这个文件不改这次的规则，只管之后启动的运行。为什么：开关决定欠不欠一条基线、谁能批，做到一半换了，前面的站是按旧规则
做的、却按新规则判。要新规则就取消重开。没有这个文件按缺省；写坏了（不是 YAML、`separate: yes` 这类类型不对）启动就拒。

- **基线**：关着的那条基线，它的命令仍能读，**写增量的子命令报"这个项目没开接口基线"**，它的门禁回 `pass`、`summary` 写
  "不欠（governance.interface=false）"。`lib/governance.js` 的 `governance(flow)` 从 `flow.settings` 取，所有脚本与命令都用这一个；
  运行之外调命令（`/flow:xforge:xspec list`）读的是文件此刻的内容。
- **`review.separate`**：每条路径的每个 `confirm`（批准破坏性改动、major 的范围定了、solid 与 major 的规格侧定了与两路对上了、验收）都写 `separate: "${settings.review.separate}"`。
  `true` 时这次运行里交付过、做过任务或发起运行的人不能确认（flows.md §4.1 的 `not: [author]`），要别人批；缺省 `false`，
  因为一个人用时作者就是批准人，强制了就走不通。
- **`review.solid`、`review.major`**：这条路径哪几个检视点由检视线检视（§5.2）。solid 只有设计环节的 `design`、`tests-design`，
  major 六个都有。检视站的 `when` 读它（flows.md §7.1 的
  `settings` 根）：关着的站进站即跳过，不调模型。只有人改文件能打开，模型改不了：它在钉住的设置里，运行里谁都动不了。
- **`lines`**：每条线一个模型，`model` 必写、`provider` 可选（flows.md §7.1：流程的 `main` 与执行方式的模型写成
  `${settings.lines.<线>.model}`）。没写的线用实例的缺省模型。开着检视点的路径，检视线该写明、且与开发线不同：同一个模型
  检视自己写的东西不是另一双眼睛。但只配了一家模型也要能用，所以相同（或没写）**只告警**：solid、major 的第一站（门禁 `lines`）
  照过、记一条 `warn`，实例日志与界面各说一次，运行的报告（`conix-report.json` 的 `warnings`）里也有；自检报 `warn`。
  缺省的配法是开发线 Claude、测试线与检视线 DeepSeek。

同一个文件可以带 **`verify` 菜单**：工作包做完时能跑的验证，人声明、进 git。

```yaml
verify:
  unit:
    argv: [python3, -m, unittest, discover, -s, tests, -q]   # 程序与参数，一项一个，不经 shell
  test:
    argv: [python3, -m, unittest, discover, -s, tests, -q, -p, "{file}.py"]
    params: {file: "test_[a-z0-9_]+"}    # {file} 的值要整串匹配这个正则
    cwd: .                               # 可选：项目内的相对目录
    description: 一个测试文件
  build:
    argv: [cargo, test, --offline]
    writable: [target]                   # 可选：能写的目录，只给被 git 忽略的（门禁判活不干活）
    network: false                       # 可选：要不要联网
```

工作包只按名字选、给参数（`xpkg plan --verify test:file=test_trunc`），命令行从不由模型写：值只能落进 `{参数}` 的位置，
整串匹配人写的正则，起进程时是 argv 的一项（流程库按 argv 直接起，`flows.md` §2.2 的 `plan`）。`argv` 用了没声明的参数、
声明了没用的参数、正则写错，菜单就有问题，`--verify` 与门禁都拒绝，要人改。`lib/governance.js` 的 `verifyMenu(flow)` 读它，
核对与展开在 `lib/verify.js`（命令与门禁共用）。没有菜单，工作包就不带 `--verify`。菜单与整个文件一起**钉进运行**：
`verifyMenu` 读 `flow.settings`，运行跑到一半有人改了菜单，这次运行的判据不变，改了只管之后启动的运行；运行之外
（`xpkg verify-menu`、自检）读文件此刻的内容。

项目的验证（`verify.sh`，门禁 `unit-tests` 跑它）要写构建、缓存目录或要联网的，在同一个文件的 **`checks`** 里声明：
`checks: {writable: [target], network: false}`（缺省都空）。菜单项与 `checks` 的 `writable` 都只给被 git 忽略的目录
（`flows.md` §2.2），自检核对写法。

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
| `work-packages.yaml` | `xpkg` | `{packages: [{id, title, paths: [glob], after: [id], criteria: [{id: C1, text}], verify?: [{name, params}], status, summary?, files?: [path], evidence?: {C1: text}}]}`（以前的包 `verify` 是一句说明，照读，不跑） |

`by` 是命令输入里的 `by`（flows.md §7.6）：`person` / `model` / `station`（站 id）。

### 2.3 台账的种类与结论

| 种类 | 编号 | 结论 | 规矩 |
|---|---|---|---|
| `decisions` | D1… | open → accepted / rejected / resolved / deferred | 离开设计站前都有结论 |
| `risks` | R1… | 同上 | 同上 |
| `findings` | F1… | open / resolved / disputed / rejected / deferred | 至少一个 `--ref`；作者（检视站之外的站）只能给 `resolved`、`disputed`（不同意）；`rejected`、`deferred` 只有检视站（`review-*`）或人能给；检视站能重开（`open`）。`disputed` 在作者那一站算答复（交回检视站），在检视站与别处算没结 |
| `questions` | Q1… | open / answered / deferred | 澄清站用；**只有人能给结论**（`by.kind == person`），回答写在 note |
| `constitution` | 原则编号（P1） | complies / violates / n-a | 检视站逐条答（`xledger reply`）；`violates` 要 note |

### 2.4 归档

`archive` 站是两个命令步骤：`xspec archive` 把规格增量并进 `specs/`，`xarch archive` 把接口增量并进 `interfaces/`（没开接口基线时
回"没有要并的"）；各自先核对、对不上就不并，两者都过才出站。随后运行结束，流程库把运行目录留下的制品归档进 `${data_dir}/archive/<日期序号>-<标题>/`（已有）。

## 3. 命令

都在 `commands/<名>.js`，`bundle.yaml` 里声明只读的子命令。用法写在文件头的 `//` 注释里（`<命令> help` 看）。

| 命令 | 子命令（**粗体**只读） |
|---|---|
| `xspec` | **list** [能力]、**get** <编号>、**delta**、**check**、**pending**、**seal**、init <能力> <前缀> "<标题>"、add <能力> "<标题>" "<正文>"、modify <编号> "<正文>" [--title T]、remove <编号>、revert <编号>、archive |
| `xarch` | **modules**、**list** [模块]、**get** <编号>、**delta**、**check**、**pending**、module <模块> "<标题>" --paths a,b、add <模块> <种类> <名字> "<签名>" "<说明>" [--stable]、modify <编号> [--signature S] [--description D] [--breaking]、promote <编号>、deprecate <编号>、remove <编号>、revert <编号>、archive |
| `xledger` | **list** [种类]、add <种类> "<内容>" --ref a,b、set <种类> <编号> <结论> ["<说明>"]、reply <原则> complies\|violates\|n-a "<说明>" |
| `xpkg` | **list**、**verify-menu**、plan <编号> "<标题>" --paths a,b --criteria "一;二" [--after p1,p2] [--verify <名字>[:参数=值,…]]…、done <编号> "<做了什么>" --files a,b --evidence "C1=…;C2=…"、blocked <编号> "<卡在哪>" |

- `xspec add/modify` 当场判正文有没有 WHEN / THEN（或"当…时 / 应"），没有就报错不写。
- `xspec pending` / `xarch pending`：同一个包**别的在途运行**（`flow.runs`）的增量动到了哪些编号。`add` 排编号时把它们占的也算上；
  `check` 与门禁把"别的在途运行也在改这一条"报成冲突。
- `xarch modify` 一个 `stable` 元素必须带 `--breaking`（不带就报错，告诉它要么改成兼容的、要么声明破坏）；`remove` 只能删基线里已经
  `deprecated` 的元素；新元素缺省 `experimental`，`--stable` 是补登记已经在用的接口。
- `xledger set/reply` 按 §2.3 的规矩查 `by`，不合就报错说谁能给。
- `xspec seal`：这次运行目录里每个文件（`children/`、`_run/` 与 `ledgers/` 下的发现、决定、风险除外：合起来、验证两站分叉之后还往里记）→ 字节的 sha256，作为返回值，不写文件。solid、major 的 `seal` 站用（§5.1）。
- `xpkg plan --verify` 可以写几次，每次一项，当场按 §1 的菜单核对：名字在菜单里、参数一个不多一个不少、值整串匹配；
  不合就报错（说菜单里有什么、该怎么写），什么都不写。`xpkg verify-menu` 列出菜单（只读，运行之外也能调）。

## 4. 门禁

脚本在 `scripts/`，共用的判法在 `lib/`（命令与门禁 import 同一份）。

| 门禁 | 种类 | 判什么 |
|---|---|---|
| `proposal-sections` | check: text | `proposal.md` 有 `## 背景`、`## 目标`、`## 非目标`、`## 改什么`、`## 影响面`、`## 为什么选这条流程`，没有 TODO/TBD |
| `impact` | script | 提案的「影响面」与启动时的 `inputs.impact` 一致：列了的都写到；`impact` 为空时写"无" |
| `spec-delta-shape` | check: schema | 规格增量的形状（关着规格基线时不欠） |
| `spec-delta` | script | 增量对得上基线（已有的规矩）；每条正文有 WHEN/THEN；与别的在途运行不冲突 |
| `interface-delta` | script | 接口增量对得上基线：编号形状、模块存在（或这次 `module` 新建）、`modified/promoted/deprecated/removed` 的元素存在、`removed` 的已 deprecated、stable 的改动都带了 `breaking`；与在途运行不冲突 |
| `interface-compat` | script | 有 `breaking` 的改动时，`inputs.impact` 含 `interface`，且 `decisions` 台账里有一条 refs 指向这个编号、结论 accepted。结果带事实 `breaking`（有没有破坏性改动；没开接口基线、没有增量都明确是 `false`），§5.2 的批准读它 |
| `design-sections` | script | `design.md` 有 `## 做法`、`## 集成点`、`## 失败模式`、`## 迁移与回滚`、`## 被否决的方案`（至少一条 `- 方案：为什么不选`）、`## 原则对照`、`## 工作包` |
| `constitution` | script | 设计逐条对照原则（已有） |
| `packages-planned` | script | 交出的 packages 与 `xpkg plan` 的一一对得上；每包有路径、至少一条判据；`after` 指的包存在、无环；包的路径不重叠 |
| `ledgers-settled` | script | §2.3 的规矩：没有 open / disputed；findings 有 ref；`rejected`/`deferred` 的 findings 是检视站或人给的；questions 是人答的。按在哪一站判（`input.station`）分两头：检视站只判**自己记的**发现（`history` 的第一条是它）；别的站可以把检视站记的发现标 `disputed` 交回去 |
| `constitution-reply` | script | 检视站的 `constitution` 台账覆盖每条原则；`violates` 有说明 |
| `lines` | script（solid、major 的第一站） | 写出三条线的模型；这条路径开着检视点时，`lines.review.model` 没写、或与 `lines.build.model` 相同（没写的开发线是实例的缺省模型），`pass` 带一条 `warn`：说改哪里、下次起的运行才算 |
| `package-check` | script（**任务级出口**） | 这一包：`status: done`（blocked 就不过，说卡在哪）；每条判据有证据；`files` 都在包的路径里；`flow.git.changed()` 里落在这个包路径下的改动都登记在 `files` 里 |
| `package-tests` | command，`plan`（**任务级出口**） | 这一包 `verify` 登记的每一项按菜单重新核对、展开成 argv，流程库直接起进程，都退出 0 才过；没登记就不欠；登记的对不上菜单（有人改过其中一个文件）是 `error`，要人看 |
| `packages-done` | check: schema | 工作包都 done |
| `unclaimed` | script | `flow.git.changed()`（基线提交以来，含没提交的）里的每个改动都在某个包的路径里 |
| `unit-tests` | command | 跑 `.conix/governance/verify.sh` |
| `verification` | script | `verification.md` 的「覆盖」里新增、修改的每条需求（和接口增量里新增、修改的元素）一行，有结论、有带 `# <编号>` 的测试；有「连贯性」一节 |
| `archived` | script | 规格增量并进了基线 |
| `interfaces-archived` | script | 接口增量并进了基线 |
| `spec-unchanged` | script（两路的子运行） | 子运行手里那份规格侧（`${parent.output_dir}`）里封存过的每个文件与 `inputs.spec_side` 的摘要一样；不是 solid、major 的子运行是 `error` |
| `blackbox` | command，`plan`（solid、major 的 join） | 跑 verify 菜单里名叫 `blackbox` 的一项，退出码 0 才过；菜单里没有它、或它带参数，是 `error` |

## 5. 流程

`bundle.yaml`：`flows: [quick, solid, major, line-build, line-tests]`，`start: {route: [quick, solid, major], triage: triage}`，
`pace: auto`，`requires: {conix: ">=0.1.11"}`，`doctor: doctor`（§7）。按风险只在前三条里选；`line-build`、`line-tests` 是 solid、major
分出来的两路，只由它们的 fork 站起。三条路径的 `inputs` 一样：`title`（必填）、`risk`（**没有缺省**：没给就分诊）、`impact`
（数组，取值 interface / security / data-migration / infra，缺省空）；各自的 `risk` schema 就是 eligibility：quick 只收 low，
solid 收 low、medium，major 都收。

**线**是一个角色和它的模型：

| 线 | 做什么 | 模型 | 在哪 |
|---|---|---|---|
| 开发线 `build` | 提案、澄清、设计、写代码、合起来、验证 | `lines.build` | 父运行与 `line-build` 的主会话（`main: {model: "${settings.lines.build.model}", …}`），实施的子 Agent |
| 测试线 `tests` | 照规格写黑盒测试 | `lines.tests` | `line-tests` 的主会话，实施的子 Agent |
| 检视线 `review` | 检视，只看不改（策略 `review-only`） | `lines.review` | 检视站 `review-*`：主会话起的子 Agent（`{kind: agent, model: "${settings.lines.review.model}"}`） |

| 路径 | 线 | 站 |
|---|---|---|
| quick | 开发 | propose（顺手划包）→ apply → verify（人验收）→ archive |
| solid | 开发 + 测试 + 检视（只检视两路的设计） | lines → propose → seal → fork（line-build、line-tests）→ join → verify → accept → archive |
| major | 开发 + 测试 + 检视 | lines → propose → clarify → review-spec → seal → fork → join → verify → review-verify → accept → archive |

两路：`line-build` 是 design → review-design → apply → review-code，`line-tests` 是 design → review-tests-design → apply → review-tests。

| 站 | 执行 | 写入范围 | 出口 |
|---|---|---|---|
| lines 核对三条线 | 检查点（不调模型） | — | `lines` |
| propose 提案 | 会话（开发线） | 运行目录 | output、proposal-sections、impact、spec-delta-shape、spec-delta、interface-delta、interface-compat、有破坏性改动时的「批准破坏性改动」（§5.3）、增量动到的比声明的多时的「影响面扩大」（§5.4）；solid、major 另加 ledgers-settled，quick 另加 packages-planned |
| clarify 澄清（major） | 会话 | 运行目录 | output、ledgers-settled（questions 由人答）、confirm owner「范围定了」，只绑 `questions` 台账 |
| review-* 检视 | 子 Agent（检视线），只看不改 | 不能写 | output、ledgers-settled（只判自己记的）；规格侧与两路的设计另加 constitution-reply（不过回自己）；`on_unmet: {rework_to: <作者那一站>, max: 2}` |
| seal 封存规格侧 | 命令步骤 `xspec seal` | — | output：运行目录每个文件的摘要；confirm owner「规格侧定了」，绑提案、规格增量、接口增量（major 加 `questions` 台账） |
| fork 分两路 | 两个任务：`{kind: subflow, flow: line-build \| line-tests, workspace: worktree, inputs: {title, spec_side: ${stations.seal.output.files}, review: ${settings.review.<路径>}}}` | — | `children: all` |
| design 设计（两路） | 会话（`impl-design` / `tests-design`，测试线用模板 `test-design.md`） | 运行目录 | output、spec-unchanged、design-sections、constitution、packages-planned、ledgers-settled |
| apply 实施 | 每包一个任务（quick 会话，两路是这一路的模型上的子 Agent），按 `after` 排 | `${item.paths}` | 任务级 package-check、package-tests；站 packages-done、unclaimed、unit-tests（两路另加 spec-unchanged） |
| join 合起来 | 会话（`join`） | 运行目录 | output、blackbox、ledgers-settled、confirm owner「两路对上了」 |
| verify 验证 | 会话 | 运行目录 | output、verification、unit-tests；solid、major 另加 ledgers-settled；quick 另加 confirm owner「验收」 |
| accept 验收（solid、major） | 检查点 | — | confirm owner「验收」：在检视之后，检视不替代人 |
| archive 归档 | 两个命令步骤：`xspec archive`、`xarch archive` | — | output、archived、interfaces-archived；solid、major 的归档另留 `children/**`、台账与增量 |

回退点：每站的 `rework_to` 列出能退到的站；门禁的 `rework_to` 指向问题通常出在哪。确认缺省 `min: 1`，作者也能批；
`binds` 批完再改就过不了下一站。skill：`triage`、`propose`、`clarify`、`impl-design`、`tests-design`、`apply`、`impl-apply`、
`tests-apply`、`join`、`verify`，检视的 `review-spec`、`review-design`、`review-tests-design`、`review-code`、`review-tests`、
`review-verify`；`archive` 是命令步骤，没有 skill。模板：`proposal.md`、`design.md`、`test-design.md`、`verification.md` 按 §4 的小节。

### 5.1 两路：开发线与测试线

规格侧写一次、人批准（seal 站的「规格侧定了」）；然后两个子运行各在自己的 worktree 里、从同一个分叉提交出发（flows.md §6.5）：
开发线照规格设计并实现，测试线只照规格设计一个黑盒测试项目、写测试；合起来用测试跑实现。两路互相看不到，所以测试是规格的另一份
独立读法，不是照实现写的。项目的 verify 菜单要有一项 `blackbox`（黑盒测试放哪、怎么跑，不带参数）：两路都照它认测试的目录——
测试线只写那里，开发线不碰那里，合回去才不冲突。solid、major 都在自己的 worktree 里跑（`workspace: worktree`）：分叉往运行自己的
分支上提交，结束后合不合进项目的分支由人定（`/worktree merge`）。

子运行的 `result` 有 schema：`{side, summary, packages: 设计划的包（编号、路径）, done: 每包的交付（都要 done）}`，不合，父任务失败。

- **共享的是规格侧**：父运行目录在分叉时的一份，在子运行的 worktree 里，`${parent.output_dir}`（skill 里写它，开会话时换成路径）。
  子运行自己的目录是它下面的 `children/fork/<build|tests>/`。两路的 `xspec delta`、`xarch delta` 看的是自己的目录（空的），skill 叫它们直接读
  规格侧的文件；基线照旧用 `xspec list/get`、`xarch list`。
- **子运行不改规格侧**：站的 `writes` 只放自己的目录（与包的路径），会话写不到规格侧；门禁 spec-unchanged 再兜一道
  （Bash 之类绕过写入范围的）。它不用 `flow.git.changed` 判：运行目录在制品目录里，git 不管（`xforge/runs/` 被忽略），
  `git.changed` 也把制品目录排除在外，子运行改了规格侧 git 看不见。所以父运行分叉前封存（`seal` 站，`xspec seal`），摘要放在运行的记录里
  （seal 站的输出、子运行的输入 `spec_side`）：放成规格侧旁边的文件，子运行改规格侧时顺手就能改掉它。只核对封存过的文件：
  分叉之后父运行目录多出来的不算；发现、决定、风险三本台账不封（合起来、验证两站在分叉之后还往里记）。
- **只做子运行**：包没有"只能由别的流程起"的写法（`flows` 列的都能点名起）。最省事又挡得住的是输入：`spec_side` 必填、没有缺省，
  单独起时输入就不合；硬给一个，第一站的 spec-unchanged 也因为没有父运行报 `error`。
- **读隔离**：子运行的会话读不到父运行与兄弟的 worktree（flows.md §6.5）。项目里已有的源码两路都读得到：测试线的 skill 要它只照
  规格的 WHEN / THEN 与接口基线、增量里的签名写，不读被测模块的实现，测试只经公开接口、每个用例带 `# <编号>`。
  两路的 id 在分叉提交时就一起取好了，名单里都有对方的 worktree（还没建的也在），先分叉的那一路从一开始就读不到后分叉的那一路。
- **合起来**：两路交付了就由派工器合进这次运行的分支（冲突父任务失败）。join 站的会话照 `blackbox` 一项跑黑盒测试，
  每个没过的（一个根因一条）`xledger add findings`，写明建议错在实现、测试还是规格，`flow_propose` 建议打回。门禁 blackbox 与
  ledgers-settled 不过，这一站就等人；不自动打回：错在哪一边要人定。
- **打回**：join 的 `rework_to` 是 `[fork, propose]`。打回 `fork` 有两种粒度：整站（`/rework fork`）两路都重新分叉；只打回一个任务
  （`/rework fork/build`，或对话里让主 Agent `flow_rework {to: fork, tasks: [build]}`）只重做那一路，另一路的子运行与结果留着。都是新一轮、
  新的分叉提交，而这时父运行的分支上已经有上一轮两路合回来的东西：重做的那一路看得见另一路上一轮的工作，两路不再互相独立——
  skill 要它照规格修，不照对面凑。新的子运行自己的目录是空的，上一轮为什么打回在父运行的 `ledgers/findings.yaml`。
  错在规格侧的打回 `propose`：规格侧重写、重批、重新封存，两路都重来。

### 5.2 检视线：检视点与四步

检视点是一个检视站，开关在治理文件的 `review.<路径>`（§1），每条路径一份：

| 检视点 | 站 | 检视什么 | 打回到 | major 缺省 | solid |
|---|---|---|---|---|---|
| `spec` | 父运行 `review-spec` | 提案、规格与接口增量：两个互不相见的人照它会不会做出不一样的东西 | propose | 开 | 没有 |
| `design` | `line-build` 的 `review-design` | 开发线的设计 | design | 开 | 开（可关） |
| `tests-design` | `line-tests` 的 `review-tests-design` | 测试线的设计（测什么、怎么只经公开接口测） | design | 开 | 开（可关） |
| `code` | `line-build` 的 `review-code` | 开发线的代码（对着设计与规格） | apply | 关 | 没有 |
| `tests` | `line-tests` 的 `review-tests` | 测试线写出的测试 | apply | 关 | 没有 |
| `verify` | 父运行 `review-verify` | 验证记录 | verify | 关 | 没有 |

三条路径由此拉开：quick 只有开发线；solid 三条线，检视线**只检视设计环节**（两路的设计，`review.solid` 只有这两个开关，缺省开）；
major 先澄清、检视规格侧与两路的设计，全链条写 `review: {major: {code: true, tests: true, verify: true}}`，只能人改文件打开。
要检视 solid 没有的那几处，走 major；`review.solid` 里写了别的检视点，自检报 `warn`「不起作用」。major 父运行的两个检视站
读 `settings.review.major.<点>`；两路的读输入 `inputs.review.<点>`——子运行不知道自己是哪条路径的，fork 站把这条路径的那一份传进去
（solid 的没有 `code`、`tests`，那两站跳过）。

**一个检视点里的四步**：开发线设计 → 检视线检视 → 开发线修复 → 检视线确认。

1. 开发线那一站交付（如设计）。
2. 检视站（检视线的模型，只看不改）逐条 `xledger add findings`，`by` 记的是这一站（`review-design`）。设计阶段的检视站还逐条答原则。
3. 有没结的发现，这一站的 ledgers-settled 不过：`on_unmet: {rework_to: <作者那一站>, max: 2}` 自动打回。开发线逐条答：修好的
   `resolved`，不同意的 `disputed` 写理由。作者那一站的 ledgers-settled 认 `disputed`（交回检视站定），认不了 `open`。
4. 检视站再来一轮：核实修好的（`resolved "核实：…"`），没修好的重开；认可 `disputed` 的理由就 `rejected`，不认可就重开。
   它的 ledgers-settled 只判自己记的发现：别的站记的不归它结，也挡不住它。
5. 两轮还没结清（自动打回用完了）：站停下等人（`/rework`、`/force <理由>`，或改了再重判）。

台账的历史里看得到这四步：`open（review-design）→ disputed（design）→ rejected（review-design）`。
检视不替代人的批准：批准破坏性改动、影响面扩大、规格侧定了、两路对上了、验收，都照旧是主人的；major 的验收在 `review-verify` 之后（accept 站）。

**模型**：`lines` 在运行启动时钉住，检视站的子 Agent 被告知 `model: <lines.review.model>`。开着检视点时检视线与开发线是同一个模型
（或检视线没写），第一站 `lines` 照过、带一条告警（§1）；自检报 `warn`（§7）。测试线与检视线可以是同一个模型（缺省就都是 DeepSeek）。
没写的线用实例的缺省模型。

### 5.3 破坏性的接口改动要主人批

改了 stable 元素、声明了 `breaking`，光有 decisions 台账里一条 accepted 不够：那一条模型自己就能记、自己就能定结论。
所以判 interface-compat 的站（三条路径都在 propose：没有哪条路径在规格侧定之前有设计站）都有一条只在有破坏性改动时才要的确认：

```yaml
- kind: confirm
  who: owner
  label: 批准破坏性改动
  when: {gate: interface-compat, fact: breaking}
  separate: "${settings.review.separate}"
  binds: ["${output_dir}/interface-delta.yaml"]
```

- **要不要由门禁定**：`when` 读的是 interface-compat 从接口增量算出的事实 `breaking`（flows.md §4.2），不是站的输出——输出是
  模型交的，写个 `breaking: false` 就把人绕过去了。增量本身是命令 `xarch` 写的，门禁按基线判它是不是破坏性的。
- **说不清就要**：门禁出错、没跑出结果、没给这个事实，都当有破坏性改动，要主人批。所以没开接口基线、没有增量时门禁也明确回
  `breaking: false`，不是什么都不回。没有破坏性改动时这条确认记在站的 `not_required` 里（带读到的值），不是悄悄没了。
- **和别的确认一起**：它与 interface-compat 同在一站，判到它时门禁的结果已经在了。批的是接口增量：之后的站再改
  `interface-delta.yaml`，那一站过不去，回这一站重批。solid、major 的「规格侧定了」再批一遍整个规格侧；两次批的东西不同，
  一个是"这次可以破坏这个接口"，一个是"两路可以照这份规格各干各的"。驳回就是这条出口不过，按站的 `on_unmet` 打回提案重写。
- quick 也判：走小路不省人。

### 5.4 增量动到的比声明的多：影响面扩大

启动时的 `impact` 是人（或替人起运行的主 Agent）说的，选路靠它；提案的「影响面」要和它一一对应。可做下去才发现接口增量里有改动，
而启动时没声明 `interface`——以前门禁 impact 只拿提案对声明，没声明就要提案写"无"，模型为了过门禁会把「影响面」改成"无"，
增量里明明「新增 1」它也照过（LIVE 里真出过）。

- 门禁 impact 同时读接口增量（开着接口基线时）：有改动而没声明 `interface`，影响面就是声明的加上它。提案照实写，写"无"不过；
  结果带事实 `widened`、`undeclared`。
- 多出来的由主人认：三条路径的 propose 都有一条 `{kind: confirm, who: owner, label: 影响面扩大, when: {gate: impact, fact: widened}}`，
  绑提案与接口增量。没多出来时它记在 `not_required` 里。声明是人定的，模型改不了、也不该为了过门禁去迁就它。
- 破坏性的改动不在这里放宽：interface-compat 仍要启动时声明了 `interface`（选路与风险都跟着它），没声明就换路或重开。

## 6. 演示项目

`examples/xforge/`：Python 小库 `textkit`；`xforge/specs/textkit.md` 是规格基线，`xforge/interfaces/textkit.yaml` 是接口基线（已有的
`slugify`、`word_count` 登记为 stable），`.conix/governance/xforge.yaml` 两条基线都开，verify 菜单有 `unit`、`test:file=…`、`compile`、
`blackbox`（`tests_blackbox/`，已有几条照规格写的黑盒测试），`lines` 开发线 Claude、测试线与检视线 DeepSeek（换成实例里配了的）。
`crates/flow/tests/xforge*.rs` 不调模型地查这个包（`xforge_lines.rs` 用真实的派工器把 solid 与 major 从提案走到归档，major 走一遍
检视的来回，会话由测试代演）；`crates/conix/tests/live_xforge.rs` 用真实模型走三条路径，`live_tty_xforge.rs` 在真终端里只用键盘
把一个样例项目改三次（testing.md §8）。

## 7. 自检：`scripts/doctor.js`

包的自检（flows.md §7.13）：宿主的 `conix flow doctor` 在运行之外为一个项目跑它，查这个项目给 xforge 备齐了没有。只读，
每条发现带 `path`（项目里的文件）与 `fix`（人该怎么改），自己什么都不改。判法用命令与门禁用的那几份（`lib/governance.js`、
`lib/verify.js`、`lib/specs.js`、`lib/arch.js`），所以自检说没问题的，`xpkg plan --verify` 与门禁不会说有问题，反过来也一样。

| 查什么 | 不对时 |
|---|---|
| 治理文件（§1）读得成、顶层是映射 | `error`：每条流程都会拒绝启动；后两行不再查（在同一个文件里） |
| `governance.spec`、`governance.interface`、`review.separate`、`review.solid.*`、`review.major.*` 是布尔值；`governance`、`review` 是映射 | `error`：`separate` 不是布尔值启动就拒；`spec: "no"` 不报错但意思反了（"不写 false 就开"）；检视点写成 `"yes"` 那一处就悄悄不检视了 |
| 治理文件里 xforge 不认识的键（顶层与 `governance`、`review`、`lines` 下） | `warn`：多半是写错了名字 |
| `lines`：每条线 `{model, provider?}`，都是名字；开着检视点的路径，检视线写了、且与开发线不同（`lib/lines.js`，与门禁 `lines` 同一份） | 形状不对 `error`；检视线没写或与开发线相同 `warn`：照跑，第一站记告警 |
| `review.solid` 只有 `design`、`tests-design` | 写了别的检视点 `warn`「不起作用」：solid 只检视设计环节，要检视那一处走 major |
| `verify` 菜单：`menuProblems`，与 `xpkg plan --verify`、`package-tests` 同一份（名字、字段、`argv`、`params` 的正则、没声明与没用到的参数、`cwd`） | 每条一个 `error`；没有菜单是 `info` |
| `verify.blackbox`：`blackboxProblem`，与门禁 `blackbox` 同一份 | 没有：`warn`（solid、major 需要它，quick 不用）；带参数：`error` |
| 开着的基线：`<数据目录>/specs/*.md`、`<数据目录>/interfaces/*.yaml` 读得成 | 读不成 `error`；还是空的 `info`（第一次变更的 `xspec init`、`xarch module` 与归档会建） |
| `.conix/governance/constitution.yaml`：`principles` 是非空列表，编号是 `P<数字>`、不重复 | 没有文件 `warn`（门禁只提醒）；空的、编号认不出或重复 `error`（门禁对照不上）；没有 `title` 是 `warn` |
| `.conix/governance/verify.sh` 在 | `warn`：实施站、验证站的 `unit-tests` 不过 |
| 项目是 git 仓库、有提交（`flow.git.head()`） | `warn`：`unclaimed`、`package-check` 判不了改了什么 |

演示项目（§6）什么都不报；`crates/flow/tests/xforge_doctor.rs` 查它，以及上面每一种坏法。
