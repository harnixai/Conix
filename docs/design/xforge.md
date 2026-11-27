# 内置流程包 xforge：规格驱动的开发流程

xforge 是 Conix 内置的开发流程包（源头 `examples/xforge/.conix/flows/xforge/`，构建时编进 host，起来时放进全局流程库，`conix flow install xforge` 装进项目），能力对齐 XForge
（`harnixai/xforge`）：两条基线（规格、接口）、三条按风险选的路径、独立检查、按工作包实施、人在几处把关、归档并进基线。
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
```

它是包的设置（`bundle.yaml` 的 `settings`，flows.md §7.1），缺省写在 `bundle.yaml` 里。`governance` 与 `review` **运行启动时读一次、钉进运行**（下面的 `verify` 菜单例外）：
运行里改这个文件不改这次的规则，只管之后启动的运行。为什么：开关决定欠不欠一条基线、谁能批，做到一半换了，前面的站是按旧规则
做的、却按新规则判。要新规则就取消重开。没有这个文件按缺省；写坏了（不是 YAML、`separate: yes` 这类类型不对）启动就拒。

- **基线**：关着的那条基线，它的命令仍能读，**写增量的子命令报"这个项目没开接口基线"**，它的门禁回 `pass`、`summary` 写
  "不欠（governance.interface=false）"。`lib/governance.js` 的 `governance(flow)` 从 `flow.settings` 取，所有脚本与命令都用这一个；
  运行之外调命令（`/flow:xforge:xspec list`）读的是文件此刻的内容。
- **`review.separate`**：每条路径的每个 `confirm`（检查通过、验收、major 与 dual 的范围定了、dual 的规格侧定了与两路对上了、批准破坏性改动）都写 `separate: "${settings.review.separate}"`。
  `true` 时这次运行里交付过、做过任务或发起运行的人不能确认（flows.md §4.1 的 `not: [author]`），要别人批；缺省 `false`，
  因为一个人用时作者就是批准人，强制了就走不通。

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
```

工作包只按名字选、给参数（`xpkg plan --verify test:file=test_trunc`），命令行从不由模型写：值只能落进 `{参数}` 的位置，
整串匹配人写的正则，起进程时是 argv 的一项（流程库按 argv 直接起，`flows.md` §2.2 的 `plan`）。`argv` 用了没声明的参数、
声明了没用的参数、正则写错，菜单就有问题，`--verify` 与门禁都拒绝，要人改。`lib/governance.js` 的 `verifyMenu(flow)` 读它，
核对与展开在 `lib/verify.js`（命令与门禁共用）。没有菜单，工作包就不带 `--verify`。菜单**不钉**：`verifyMenu` 读文件此刻的内容，
因为 `package-tests` 判的正是"包当初选的还合不合现在的菜单"，菜单在运行中改了由它报 `error` 给人看，不是悄悄按旧菜单跑。

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
- `xspec seal`：这次运行目录里每个文件（`children/`、`_run/` 除外）→ 字节的 sha256，作为返回值，不写文件。dual 的 `seal` 站用（§5.1）。
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
| `ledgers-settled` | script | §2.3 的规矩：没有 open / disputed；findings 有 ref；`rejected`/`deferred` 的 findings 是检查站或人给的；questions 是人答的 |
| `constitution-reply` | script | 检查站的 `constitution` 台账覆盖每条原则；`violates` 有说明 |
| `package-check` | script（**任务级出口**） | 这一包：`status: done`（blocked 就不过，说卡在哪）；每条判据有证据；`files` 都在包的路径里；`flow.git.changed()` 里落在这个包路径下的改动都登记在 `files` 里 |
| `package-tests` | command，`plan`（**任务级出口**） | 这一包 `verify` 登记的每一项按菜单重新核对、展开成 argv，流程库直接起进程，都退出 0 才过；没登记就不欠；登记的对不上菜单（有人改过其中一个文件）是 `error`，要人看 |
| `packages-done` | check: schema | 工作包都 done |
| `unclaimed` | script | `flow.git.changed()`（基线提交以来，含没提交的）里的每个改动都在某个包的路径里 |
| `unit-tests` | command | 跑 `.conix/governance/verify.sh` |
| `verification` | script | `verification.md` 的「覆盖」里新增、修改的每条需求（和接口增量里新增、修改的元素）一行，有结论、有带 `# <编号>` 的测试；有「连贯性」一节 |
| `archived` | script | 规格增量并进了基线 |
| `interfaces-archived` | script | 接口增量并进了基线 |
| `spec-unchanged` | script（dual 的子运行） | 子运行手里那份规格侧（`${parent.output_dir}`）里封存过的每个文件与 `inputs.spec_side` 的摘要一样；不是 dual 的子运行是 `error` |
| `blackbox` | command，`plan`（dual 的 join） | 跑 verify 菜单里名叫 `blackbox` 的一项，退出码 0 才过；菜单里没有它、或它带参数，是 `error` |

## 5. 流程

`bundle.yaml`：`flows: [quick, solid, major, dual, dual-impl, dual-tests]`，`start: {route: [quick, solid, major], triage: triage}`，
`pace: auto`，`requires: {conix: ">=0.1.11"}`，`doctor: doctor`（§7）。按风险选路只在前三条里选；双路开发 `dual` 点名起（§5.1）。
三条流程的 `inputs` 一样：`title`（必填）、`risk`（**没有缺省**：没给就分诊）、`impact`（数组，取值 interface / security /
data-migration / infra，缺省空）；各自的 `risk` schema 就是 eligibility：quick 只收 low，solid 收 low、medium，major 都收。

| 站 | quick | solid | major | 执行 | 写入范围 | 出口 |
|---|---|---|---|---|---|---|
| propose 提案 | ✓（顺手划包） | ✓ | ✓ | 会话 | 运行目录 | output、proposal-sections、impact、spec-delta-shape、spec-delta、interface-delta；quick 另加 packages-planned |
| clarify 澄清 | | | ✓ | 会话 | 运行目录 | output、ledgers-settled（questions 由人答）、confirm owner |
| design 设计 | | ✓ | ✓ | 会话 | 运行目录 | output、design-sections、constitution、spec-delta、interface-delta、interface-compat、有破坏性改动时 confirm owner「批准破坏性改动」（§5.2）、packages-planned、ledgers-settled |
| check 独立检查 | | ✓ | ✓ | 子 Agent，只看不改 | 不能写 | output、ledgers-settled、constitution-reply、confirm reviewer |
| apply 实施 | ✓ | ✓ | ✓ | 每包一个任务（quick 会话，solid/major 子 Agent），按 `after` 排 | `${item.paths}` | 任务级 package-check、package-tests；站 packages-done、unclaimed、unit-tests |
| verify 验证 | ✓ | ✓ | ✓ | 会话 | 运行目录 | output、verification、unit-tests、confirm owner |
| archive 归档 | ✓ | ✓ | ✓ | 两个命令步骤：`xspec archive`、`xarch archive` | — | output、archived、interfaces-archived |

回退点：每站的 `rework_to` 列出能退到的站；门禁的 `rework_to` 指向问题通常出在哪（如 design 站的 `spec-delta` 不过退回 propose）；
检查站发现问题根在提案时用 `flow_propose` 建议退回提案（子 Agent 写进结果、由主 Agent 提）。确认缺省 `min: 1`，作者也能批；
`binds` 缺省绑本站及之前的制品，批完再改就过不了下一站；major 的澄清只绑 `questions` 台账（设计还要改提案）。

skill：`triage`（判风险与影响面、选路）、`propose`、`clarify`、`design`、`check`、`apply`、`verify`；`archive` 是命令步骤，没有 skill。
模板：`proposal.md`、`design.md`、`verification.md` 按 §4 的小节。

### 5.1 双路开发：`dual`

规格侧写一次、人批准；然后两个子运行各在自己的 worktree 里、从同一个分叉提交出发（flows.md §6.5）：一个照规格设计并实现，
一个只照规格设计一个黑盒测试项目、写测试；合起来用测试跑实现。两路互相看不到，所以测试是规格的另一份独立读法，不是照实现写的。
代价是两份设计、两次检查。它不在 `start.route` 里：值不值由人定，分诊可以向用户建议（高风险、行为能写成 WHEN / THEN 的），
点名起 `flow_start {flow: "xforge/dual"}`。输入与 major 一样。项目的 verify 菜单要有一项 `blackbox`（黑盒测试放哪、怎么跑，不带参数）：
两路都照它认测试的目录——测试那一路只写那里，实现那一路不碰那里，合回去才不冲突。

| 站 | 执行 | 写入范围 | 出口 |
|---|---|---|---|
| propose 提案 | 会话 | 运行目录 | 同 major，另加 interface-compat 与有破坏性改动时的「批准破坏性改动」（§5.2）：父运行没有设计站，破坏性的接口改动在这里就要有决定 |
| clarify 澄清 | 会话 | 运行目录 | 同 major |
| check 规格侧检查 | 子 Agent，只看不改（`spec-check`：两个互不相见的人照字面会不会做出不一样的东西） | 不能写 | output、ledgers-settled、constitution-reply、confirm reviewer「规格侧定了」，绑提案、规格增量、接口增量、`questions` 台账 |
| seal 封存规格侧 | 命令步骤 `xspec seal` | — | output：运行目录每个文件的摘要 |
| fork 分两路 | 两个任务：`{kind: subflow, flow: dual-impl \| dual-tests, workspace: worktree, inputs: {title, spec_side: ${stations.seal.output.files}}}` | — | `children: all` |
| join 合起来 | 会话（`join`） | 运行目录 | output、blackbox、ledgers-settled、confirm owner「两路对上了」 |
| verify 验证 | 会话 | 运行目录 | 同 major |
| archive 归档 | 两个命令步骤 | — | 同 major；运行的归档另留 `children/**`、台账与增量 |

两个子运行 `dual-impl`、`dual-tests` 的站一样：design（会话，写自己目录里的 `design.md` 与工作包）→ check（子 Agent，`dual-check`）→
apply（每包一个子 Agent，任务级 package-check、package-tests；站 packages-done、unclaimed、unit-tests）。每一站都有 spec-unchanged。
测试那一路的设计用模板 `test-design.md`：同样七节，讲的是测试放哪、每条 WHEN / THEN 怎么落成用例、只经哪些公开接口。
子运行无人值守：检查留下没结的发现自动打回设计（`on_unmet`，最多两次），之后等人。`result` 有 schema：
`{side, summary, packages: 设计划的包（编号、路径）, done: 每包的交付（都要 done）}`，不合，父任务失败。

- **共享的是规格侧**：父运行目录在分叉时的一份，在子运行的 worktree 里，`${parent.output_dir}`（skill 里写它，开会话时换成路径）。
  子运行自己的目录是它下面的 `children/fork/<impl|tests>/`。两路的 `xspec delta`、`xarch delta` 看的是自己的目录（空的），skill 叫它们直接读
  规格侧的文件；基线照旧用 `xspec list/get`、`xarch list`。
- **子运行不改规格侧**：站的 `writes` 只放自己的目录（与包的路径），会话写不到规格侧；门禁 spec-unchanged 再兜一道
  （Bash 之类绕过写入范围的）。它不用 `flow.git.changed` 判：运行目录在制品目录里，git 不管（`xforge/runs/` 被忽略），
  `git.changed` 也把制品目录排除在外，子运行改了规格侧 git 看不见。所以父运行分叉前封存（`seal` 站，`xspec seal`），摘要放在运行的记录里
  （seal 站的输出、子运行的输入 `spec_side`）：放成规格侧旁边的文件，子运行改规格侧时顺手就能改掉它。只核对封存过的文件：
  分叉之后父运行目录多出来的（join 记的台账，打回后的新一轮会带进来）不算。
- **只做子运行**：包没有"只能由别的流程起"的写法（`flows` 列的都能点名起）。最省事又挡得住的是输入：`spec_side` 必填、没有缺省，
  单独起时输入就不合；硬给一个，第一站的 spec-unchanged 也因为没有父运行报 `error`。
- **读隔离**：子运行的会话读不到父运行与兄弟的 worktree（flows.md §6.5）。项目里已有的源码两路都读得到：测试那一路的 skill 要它只照
  规格的 WHEN / THEN 与接口基线、增量里的签名写，不读被测模块的实现，测试只经公开接口、每个用例带 `# <编号>`。
  两个子运行还是一个一个分叉、哪一路先不固定，但两路的 id 在分叉提交时就一起取好了，名单里都有对方的 worktree（还没建的也在），
  先分叉的那一路的主会话（它的各站都在里面跑）从一开始就读不到后分叉的那一路（flows.md §6.5）。
- **合起来**：两路交付了就由派工器合进这次运行的分支（冲突父任务失败，flows.md §6.5）。join 站的会话照 `blackbox` 一项跑黑盒测试，
  每个没过的（一个根因一条）`xledger add findings`，写明建议错在实现、测试还是规格，`flow_propose` 建议打回。门禁 blackbox 与
  ledgers-settled 不过，这一站就等人；不自动打回：错在哪一边要人定。
- **打回**：join 的 `rework_to` 是 `[fork, propose]`。打回 `fork` 有两种粒度：整站（`/rework fork`）两路都重新分叉；只打回一个任务
  （对话里让主 Agent `flow_rework {to: fork, tasks: [impl]}`）只重做那一路，另一路的子运行与结果留着。都是新一轮、新的分叉提交，
  而这时父运行的分支上已经有上一轮两路合回来的东西：重做的那一路看得见另一路上一轮的工作（实现看得见测试，反之亦然），
  两路不再互相独立——skill 要它照规格修，不照对面凑。重做的那一路的禁读名单里只有父运行的 worktree：另一路已经合回去，worktree 删了。
  新的子运行自己的目录是空的，上一轮为什么打回在父运行的 `ledgers/findings.yaml`。
  错在规格侧的打回 `propose`：规格侧重写、重批、重新封存，两路都重来。

### 5.2 破坏性的接口改动要主人批

改了 stable 元素、声明了 `breaking`，光有 decisions 台账里一条 accepted 不够：那一条模型自己就能记、自己就能定结论。
所以判 interface-compat 的站（solid、major 的 design，dual 的 propose）都有一条只在有破坏性改动时才要的确认：

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
- **和别的确认一起**：它与 interface-compat 同在一站，判到它时门禁的结果已经在了（门禁还在跑，它就跟着等，不先问人）。
  批的是接口增量：之后的站再改 `interface-delta.yaml`（`xarch modify` 等），那一站过不去，回这一站重批。检查站（solid、major
  的检查通过，dual 的规格侧定了）照旧再批一遍整体；两次批的东西不同，一个是"这次可以破坏这个接口"，一个是"这份设计 / 规格侧可以照着做"。
  驳回就是这条出口不过，按站的 `on_unmet` 打回这一站重写（design 回 design，dual 的 propose 回 propose）。
- quick 的提案只判 interface-delta、不判 interface-compat，也就没有这条。

## 6. 演示项目

`examples/xforge/`：Python 小库 `textkit`；`xforge/specs/textkit.md` 是规格基线，`xforge/interfaces/textkit.yaml` 是接口基线（已有的
`slugify`、`word_count` 登记为 stable），`.conix/governance/xforge.yaml` 两条基线都开，verify 菜单有 `unit`、`test:file=…`、`compile`、
`blackbox`（`tests_blackbox/`，已有几条照规格写的黑盒测试）。`crates/flow/tests/xforge*.rs` 不调模型地查这个包（`xforge_dual.rs`
用真实的派工器把一次 dual 从提案走到归档，会话由测试代演），`crates/conix/tests/live_xforge.rs` 用真实模型走 solid。

## 7. 自检：`scripts/doctor.js`

包的自检（flows.md §7.13）：宿主的 `conix flow doctor` 在运行之外为一个项目跑它，查这个项目给 xforge 备齐了没有。只读，
每条发现带 `path`（项目里的文件）与 `fix`（人该怎么改），自己什么都不改。判法用命令与门禁用的那几份（`lib/governance.js`、
`lib/verify.js`、`lib/specs.js`、`lib/arch.js`），所以自检说没问题的，`xpkg plan --verify` 与门禁不会说有问题，反过来也一样。

| 查什么 | 不对时 |
|---|---|
| 治理文件（§1）读得成、顶层是映射 | `error`：每条流程都会拒绝启动；后两行不再查（在同一个文件里） |
| `governance.spec`、`governance.interface`、`review.separate` 是布尔值；`governance`、`review` 是映射 | `error`：`separate` 不是布尔值启动就拒；`spec: "no"` 不报错但意思反了（"不写 false 就开"） |
| 治理文件里 xforge 不认识的键（顶层与 `governance`、`review` 下） | `warn`：多半是写错了名字 |
| `verify` 菜单：`menuProblems`，与 `xpkg plan --verify`、`package-tests` 同一份（名字、字段、`argv`、`params` 的正则、没声明与没用到的参数、`cwd`） | 每条一个 `error`；没有菜单是 `info` |
| `verify.blackbox`：`blackboxProblem`，与门禁 `blackbox` 同一份 | 没有：`warn`（"xforge/dual 需要 verify.blackbox"，别的路径不用它）；带参数：`error` |
| 开着的基线：`<数据目录>/specs/*.md`、`<数据目录>/interfaces/*.yaml` 读得成 | 读不成 `error`；还是空的 `info`（第一次变更的 `xspec init`、`xarch module` 与归档会建） |
| `.conix/governance/constitution.yaml`：`principles` 是非空列表，编号是 `P<数字>`、不重复 | 没有文件 `warn`（门禁只提醒）；空的、编号认不出或重复 `error`（门禁对照不上）；没有 `title` 是 `warn` |
| `.conix/governance/verify.sh` 在 | `warn`：实施站、验证站的 `unit-tests` 不过 |
| 项目是 git 仓库、有提交（`flow.git.head()`） | `warn`：`unclaimed`、`package-check` 判不了改了什么 |

演示项目（§6）什么都不报；`crates/flow/tests/xforge_doctor.rs` 查它，以及上面每一种坏法。
