---
name: verify
description: 对照规格与接口增量逐项验证，如实写验证记录。
---

读 `proposal.md`、`xspec delta`、`xarch delta`、`xpkg list`（每个包的结果与证据），有设计就读 `design.md`。

**覆盖**：增量里新增、修改的每条需求与每个元素各一行，清单从 `xspec delta`、`xarch delta` 取，不凭记忆列：
`- TK-R3 通过：tests/test_textkit.py#test_truncate，<怎么验证的、看到了什么>`。结论只用"通过""未通过""未验证"之一。
找证据：跑能跑的（测试、命令、一小段调用），看对应的测试是不是真测到了它（带 `# <编号>`）。没跑过的不写通过；没有对应测试的明说。

**连贯性**：指出提案、规格、设计、测试、实现在哪里描述了不同的行为，不复述它们；都一致就写明一致。最后写还剩的风险。

你不改代码也不改测试。验证发现改错了，如实写"未通过"，并建议打回：`flow_propose {kind: rework, to: apply, reason}`
（根在设计的回 design，快速变更里回 propose）。人验收时看这份记录决定。
