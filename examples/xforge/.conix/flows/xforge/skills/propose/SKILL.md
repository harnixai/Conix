---
name: propose
description: 把想法收敛成有边界的目标：写提案，把对规格与接口的改动记成增量。
---

先弄清现状：`xspec list`、`xspec get <编号>` 看现有需求，`xarch modules`、`xarch list <模块>` 看现有接口（接口基线开着时），
`xspec pending`、`xarch pending` 看别的在途运行占了哪些编号；再读这次要动的代码和测试。

**提案**写给批准它的人看：
- 背景只讲问题，不谈做法。目标一句话说清"做完是什么样"；说不清就是还没收敛，回去问用户，不替他定。
- 非目标列最容易被误加的那件事，不写泛泛的"不做别的"。
- 改什么写外面看得到的行为变化，每条能被测试验证，写到增量里的编号。
- 影响面与启动时的 `impact` 一一对应：列了的每项一行说清影响什么，没列就写"无"（门禁 impact 查）。
- 为什么选这条流程：风险为什么是这一档。读完代码发现这一档不对（比如要动接口、动数据），在这一站还能换：
  `flow_switch {to: "solid"|"major", inputs: {risk, impact}, reason}`；不换路而影响面不止声明的，停下告诉用户。

**规格增量**，一条需求只说一件可测的事，正文写成 WHEN / THEN 条目（`- WHEN …` / `- THEN …`，或 `- 当 … 时` / `- 应 …`），
能直接写成黑盒测试。先找能复用的能力与需求，改已有的用 modify，不另起一条：
- `xspec add <能力> "<标题>" "<正文>"`（编号自动排）；还没有这份规格就先 `xspec init <能力> <前缀> "<标题>"`。
- `xspec modify <编号> "<改后的完整正文>" [--title T]`；退役 `xspec remove <编号>`；记错了 `xspec revert <编号>`。
- 记完 `xspec check`，对不上就照它说的改。

**接口增量**（接口基线开着时；关着时命令会说）：先问是不是非破坏不可——能加一个新的、把旧的弃用，就不改旧的。
- 新元素 `xarch add <模块> <种类> <名字> "<签名>" "<说明>"`（缺省 experimental；补登记已经在用的接口加 `--stable`）；
  新模块先 `xarch module <模块> "<标题>" --paths a,b`。
- 改 `xarch modify <编号> [--signature S] [--description D]`，改 stable 的必须 `--breaking`；不再用的先 `xarch deprecate <编号>`，
  `remove` 只删已经 deprecated 的；升成 stable `xarch promote <编号>`；记错了 `xarch revert <编号>`。记完 `xarch check`。
- 改了 stable 的元素，启动时的 impact 必须有 interface；没有就照上面换路时带上，或停下告诉用户。
- 双路开发（xforge/dual）没有设计站：改了 stable 的元素，这一站就记接受了的决定（门禁 interface-compat 查）：
  `xledger add decisions "<为什么非破坏不可、调用方怎么办>" --ref <元素编号>`，再 `xledger set decisions <D#> accepted "<说明>"`。
  需求要写到两个互不相见的人照字面能做出对得上的东西：边界、出错时的表现都写进 WHEN / THEN。

**快速变更没有设计站**：这一站顺手划工作包，`xpkg plan <编号> "<标题>" --paths a,b --criteria "判据一;判据二" [--after p1] [--verify <名字>[:参数=值]]`，验证名字从 `xpkg verify-menu` 里选（没有合适的就不写）。
路径盖住它会改的每个文件，测试也算；判据是能验证的完成条件。交出的 `packages` 就是划的这些：`{id, title, paths, after}`，与 plan 一致。

被打回时先读打回的理由与 findings，照它改提案与增量，两边保持一致。
