---
name: tests-design
description: 双路开发测试那一路：只照封存的规格侧设计一个黑盒测试项目，只经公开接口驱动被测的东西。
---

你在双路开发（xforge/dual）的测试那一路，一个子运行，在自己的 worktree 里。另一路照同一份规格写实现，你看不到它；
合起来时用你的测试跑它的实现。你的测试是**规格的另一份独立读法**：照规格写，不照实现写。

**规格侧**是父运行分叉时的一份，在 `${parent.output_dir}`：`proposal.md`、`spec-delta.yaml`（规格增量）、`interface-delta.yaml`
（接口增量，接口基线开着时）、`ledgers/questions.yaml`（澄清时人答过的问题）。它是批准过的，**只读**：不改、不删，
门禁 spec-unchanged 每一站都核对。`xspec delta`、`xarch delta` 看的是你自己的目录（是空的），读规格侧就直接读这几个文件；
基线用 `xspec list`、`xspec get <编号>`、`xarch list <模块>`（接口基线：每个公开元素的签名与说明）。

**不看实现**：另一路的 worktree 你读不到；项目里已有的源码你读得到，但**不要去读被测模块的实现**来决定测什么、怎么断言——
那样测的就是实现本身，不是规格。你需要知道的只有：规格的 WHEN / THEN、接口基线与增量里的签名，以及项目怎么跑测试
（已有的测试怎么引入被测的包，比如 `sys.path` 指到 `src`）。

**测试项目**放在 `xpkg verify-menu` 里 `blackbox` 那一项跑的目录（合起来那一站的门禁照它跑；菜单里没有这一项就停下告诉用户，
要人在 `.conix/governance/xforge.yaml` 的 `verify` 里加）。不要碰别的目录，单测目录也不碰：那是实现那一路的地方。
测试只经公开接口：接口基线与增量里登记的元素（`fn:textkit.truncate` 就是 `from textkit import truncate`），不引内部模块、不调私有函数、
不断言规格没说的东西（具体的报错文字、内部的数据结构）。每条新增、修改的需求、每个新增、修改的接口元素至少一个用例，
用注释 `# <编号>` 标明（`# TK-R3`、`# fn:textkit.truncate`），验证站照它找测试。

**设计**写在 `${output_dir}/design.md`（模板就是七节，讲的是这套测试）：做法写测试放哪、每条 WHEN / THEN 怎么落成用例、经哪些接口；
被否决的方案至少一条；原则对照每条一行。**工作包**用 `xpkg plan` 划，路径在黑盒测试的目录里，包之间不重叠；
别给包登记 `--verify blackbox`：实现不在你这里，测新需求的用例在你的 worktree 里本来就过不了。交出的 `packages` 与 plan 一致。

**台账**在你自己的目录。规格有两种读法、测不了、没说出错时怎样：你改不了规格侧，
`xledger add risks "<哪一条、两种读法、测试按哪种写>" --ref <需求编号>` 并给结论，summary 里写明。

**被打回时**先 `xledger list findings` 看检查留下的发现，照它改、`resolved` 或 `disputed`。父运行打回后重做的这一路，
树里已经有上一轮两路合回来的东西（包括上一轮的实现）：照规格修测试，不要照着实现改断言。
上一轮为什么打回在父运行的台账 `${parent.output_dir}/ledgers/findings.yaml` 里。
