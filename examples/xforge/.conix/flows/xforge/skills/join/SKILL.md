---
name: join
description: 双路开发两路合起来之后：用黑盒测试跑实现，每个没过的判断错在实现还是测试，记进台账，建议打回哪一路。
---

两路都合进了这次运行的分支：实现那一路的代码，测试那一路的黑盒测试。它们是照同一份规格（`proposal.md`、`xspec delta`、
`xarch delta`）各自写的，互相没看过。两路各自交付的东西在 `${output_dir}/children/fork/impl/`、`${output_dir}/children/fork/tests/`
（设计 `design.md`、工作包 `work-packages.yaml`、台账 `ledgers/`）。

**跑黑盒测试**：`xpkg verify-menu` 里名叫 `blackbox` 的那一项就是这一站的门禁跑的，照它的命令自己跑一遍。菜单里没有这一项，
停下告诉用户：要在 `.conix/governance/xforge.yaml` 的 `verify` 里加（双路开发没有它就没有可判的）。

**每个没过的用例一条发现**，先按规格判错在哪一边，再记：
- 照规格的 WHEN / THEN，实现的行为不对：错在实现。
- 测试断言了规格没说的东西、读错了规格、碰了公开接口以外的东西（内部模块、私有函数、实现的细节）：错在测试。
- 规格本身两种读法都说得通：错在规格侧，两边都没错。

`xledger add findings "<哪个用例、期望什么、实际什么>。建议：实现错|测试错|规格不清，因为 <对照规格哪一条>" --ref <需求编号>,<测试文件>`。
一个根因的几个用例记一条。都过了就不记，summary 写跑了多少、都过了；交出 `failures` 是没过的用例数。

**建议打回**（这一站不自动打回，人决定）：
- 错在一路：`flow_propose {kind: rework, to: fork, reason: "只重做 impl|tests：F1、F2……"}`，交出的 `rework: {tasks: [impl] 或 [tests], reason}`。
  人可以只打回那一个任务（对话里让主 Agent `flow_rework {to: fork, tasks: [...]}`），另一路的结果留着；`/rework fork` 是两路都重做。
- 错在规格侧：`flow_propose {kind: rework, to: propose, reason}`，两路都要照新规格重来。

**打回重做后再来**：先 `xledger list findings` 看上一轮的。这一轮测试过了的，`xledger set findings <编号> resolved "<怎么好的>"`；
还没过的留 open，说清还差什么。你不改代码也不改测试：这一站只写运行目录。
