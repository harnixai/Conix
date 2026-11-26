---
name: check
description: 用新的眼光核对提案、规格、接口、设计是不是在说同一件事；问题记进台账，原则逐条答。
---

你没参与写提案和设计，从读者的角度挑毛病。读这次运行目录里的 `proposal.md`、`design.md`，`xspec delta`（逐条 `xspec get` 对照基线）、
`xarch delta`、`xpkg list`、`xledger list`，以及设计说要改的代码与测试。

重点找：
- 一条需求测不了、和已有需求矛盾；提案里说的行为没有需求覆盖，或增量里有提案没说的。
- 接口改动没必要地破坏兼容，或者破坏了却没标 `breaking`、没有决定。
- 设计里有一步没有哪个包做；包的路径漏了它必须改的文件、两个包重叠；判据验证不了。
- 项目的验证跑整棵树：有没有包会因为别的包名下的测试过不了。
- 失败模式、迁移与回滚没想到的；没记的风险。

每个问题一条，指向能定位的东西：`xledger add findings "<哪里不对、为什么>" --ref <文件#小节 或 需求、元素编号>`。
要作者改的留 open：台账门禁会挡住这一站，人看了决定打回；只是提醒的 `xledger set findings <编号> deferred "<说明>"`。
查过没问题的点写在 summary 里，不进台账。

**原则逐条答**（`.conix/governance/constitution.yaml` 的每一条）：`xledger reply <原则> complies|violates|n-a "<说明>"`；
`n-a` 要说得出为什么不相干，`violates` 写清违反在哪、影响多大。

**建议打回到问题出生的那一站**：根在提案或增量（提案没跟上增量、需求互相矛盾、漏了需求）的回 `propose`，根在设计的回 `design`。
能调 `flow_propose` 就 `flow_propose {kind: rework, to, reason: "<哪几条发现、为什么回那里>"}`；你是子 Agent、调不到它时，
在交出的 `rework: {to, reason}` 里写明，由主 Agent 代为提。人按建议决定。`ready` 如实写：有 open 的发现就是 false。

**打回重做后再来**：先看上一轮的发现。作者标 resolved 的到文档里核实，没解决就 `xledger set findings <编号> open "<还差什么>"`；
标 disputed 的，你认可作者的理由就 `rejected "<为什么撤回>"`，不认可就重开并说清。
