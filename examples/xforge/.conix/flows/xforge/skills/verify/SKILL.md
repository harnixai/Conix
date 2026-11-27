---
name: verify
description: 对照规格与接口增量逐项验证，如实写验证记录。
---

读 `proposal.md`、`xspec delta`、`xarch delta`、`xpkg list`（每个包的结果与证据），有设计就读 `design.md`。
solid、major 里设计与工作包在两路各自的目录：`${output_dir}/children/fork/build/`（开发线）、`${output_dir}/children/fork/tests/`
（测试线）（`design.md`、`work-packages.yaml`），黑盒测试是测试线写的；合起来那一站的结论在 `xledger list findings`。

**覆盖**：增量里新增、修改的每条需求与每个元素各一行，清单从 `xspec delta`、`xarch delta` 取，不凭记忆列：
`- TK-R3 通过：tests/test_textkit.py#test_truncate，<怎么验证的、看到了什么>`。结论只用"通过""未通过""未验证"之一。
找证据：跑能跑的（测试、命令、一小段调用），看对应的测试是不是真测到了它（带 `# <编号>`）。没跑过的不写通过；没有对应测试的明说。

**连贯性**：指出提案、规格、设计、测试、实现在哪里描述了不同的行为，不复述它们；都一致就写明一致。最后写还剩的风险。

你不改代码也不改测试。验证发现改错了，如实写"未通过"，并建议打回：`flow_propose {kind: rework, to: apply, reason}`
（根在设计的回 design，快速变更里回 propose）。人验收时看这份记录决定。

**检视线打回时**（`xledger list findings` 里有检视站 `review-*` 记的、open 的发现）：先读打回的理由，逐条答复：
改好了 `xledger set findings <编号> resolved "<怎么改的>"`；不同意就 `xledger set findings <编号> disputed "<理由>"`，交回检视线定。
每条都要有答复这一站才过；`rejected`、`deferred` 是检视线或人的，你不给。改的是验证记录：没跑的去跑，结论照实改。
