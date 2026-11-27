---
name: impl-design
description: 开发线那一路：照封存的规格侧设计实现，划工作包，逐条对照项目原则。
---

你在开发线那一路（xforge/solid、xforge/major 分出来的子运行），在自己的 worktree 里。测试线照同一份规格写黑盒测试，你看不到它，它也看不到你；
合起来时用它的测试跑你的实现。所以**照规格的字面做**：规格没说的行为别自己发明，说不清的地方记下来（见下）。

**规格侧**是父运行分叉时的一份，在 `${parent.output_dir}`：`proposal.md`、`spec-delta.yaml`（规格增量）、`interface-delta.yaml`
（接口增量，接口基线开着时）、`ledgers/questions.yaml`（澄清时人答过的问题）。它是批准过的，**只读**：不改、不删，
门禁 spec-unchanged 每一站都核对。`xspec delta`、`xarch delta` 看的是你自己的目录（是空的），读规格侧就直接读这几个文件；
基线照旧用 `xspec list`、`xspec get <编号>`、`xarch list <模块>`。再读要动的代码与测试，以及 `.conix/governance/constitution.yaml`。

**设计**写在 `${output_dir}/design.md`（七节）：做法、集成点、失败模式、迁移与回滚、被否决的方案
（至少一条 `- 方案：为什么不选`）、原则对照（每条原则一行 `- P1：…`）、工作包。

**工作包**用 `xpkg plan <编号> "<标题>" --paths a,b --criteria "判据一;判据二" [--after p1,p2] [--verify <名字>[:参数=值]]…` 划，
验证从 `xpkg verify-menu` 里选（没有合适的就不写）。路径盖住它会改的每个文件，单测也算，包之间不重叠。**不要碰黑盒测试的目录**
（verify 菜单 `blackbox` 一项跑的那个）：那是另一路的地方，两路都改它，合回去就冲突。交出的 `packages` 就是划的这些：
`{id, title, paths, after}`，与 plan 一致。

**台账**在你自己的目录：决定 `xledger add decisions "…" --ref <编号>`、风险 `xledger add risks "…" --ref …`，离开前每条都要有结论。
规格侧有说不清、做不到、互相矛盾的地方：你改不了它，`xledger add risks "<哪一条、两种读法、我按哪种做>" --ref <需求编号>`
并给结论（`accepted "<按哪种做>"`），在 summary 里写明；根本做不下去就停下告诉用户，要人把父运行打回提案。

**检视线打回时**（`xledger list findings` 里有检视站 `review-*` 记的、open 的发现）：先读打回的理由，逐条答复：
改好了 `xledger set findings <编号> resolved "<怎么改的>"`；不同意就 `xledger set findings <编号> disputed "<理由>"`，交回检视线定。
每条都要有答复这一站才过；`rejected`、`deferred` 是检视线或人的，你不给。改的是设计与工作包（`xpkg plan` 重划，交出的 `packages` 跟着改）。

父运行打回后重做的这一路，树里已经有上一轮两路合回来的东西（包括上一轮的黑盒测试）：
照规格修，不要照着测试凑。上一轮为什么打回在父运行的台账 `${parent.output_dir}/ledgers/findings.yaml` 里。
