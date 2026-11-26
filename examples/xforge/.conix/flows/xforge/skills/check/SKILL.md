---
name: check
description: 用新的眼光检查提案与设计，问题记进台账。
---

你没参与写提案和设计，就从读者的角度挑毛病。读这次运行目录里的 `proposal.md`、`design.md`，`xspec delta`、
`xpkg list`、`xledger list`，以及设计说要改的代码。

重点找：一条需求测不了，或和已有需求矛盾；提案里说的行为没有需求覆盖；设计里有一步没有哪个工作包做；
工作包的路径漏了它必须改的文件，或者两个包重叠；没写下来的风险。

每个问题 `xledger add findings "<哪里不对、为什么>"`。要作者改的留 open——台账门禁会挡住这一站，人看了决定打回；
只是提醒的 `xledger set findings <编号> deferred "<说明>"`。查过没问题的点写在交出的 summary 里，不进台账。
`ready` 如实写：有 open 的问题就是 false。

问题的根在提案或规格增量（提案没跟上增量、需求互相矛盾、漏了需求）时，除了记进台账，再建议打回到提案；
根在设计的建议打回 `design`。建议用 `flow_propose {kind: rework, to: propose|design, reason: "<哪条发现、为什么回那里>"}` 提；
你是子 Agent、调不到它时，在交回的结果里写明"建议打回到 <站>：<理由>"，由主 Agent 代为 `flow_propose`。人按建议决定打回到哪。

打回重做之后再来检查时，先看上一轮的问题：作者标成 resolved 的到文档里核实真的解决了，没解决就
`xledger set findings <编号> open "<还差什么>"`。
