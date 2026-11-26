---
name: design
description: 把提案落成技术路径与工作包，逐条对照项目原则，决定与风险记进台账。
---

先读这次运行目录里的 `proposal.md`、`xspec delta`、`xarch delta`、`xledger list`（重大变更里有人答过的问题），要动的代码与测试，
以及项目的 `.conix/governance/constitution.yaml`（有的话）。

**设计**（`design.md` 的七节）：
- 做法：怎么实现、落到哪些函数和文件。要让一个不看实现的人能照规格写出黑盒测试。
- 集成点：和哪些已有代码、接口、数据接上。失败模式：会怎么坏、各自怎么处理。迁移与回滚：已有的怎么过渡、出事怎么退；不需要就说为什么。
- 被否决的方案：至少一条 `- 方案：为什么不选（它的代价）`。没有替代说明没真的比较过。
- 原则对照：每条原则一行 `- P1：<这个设计怎么守它>`，守不住的写明为什么、影响多大。
- 工作包：怎么切、为什么这样切。

**工作包**用 `xpkg plan <编号> "<标题>" --paths a,b --criteria "判据一;判据二" [--after p1,p2] [--verify "<怎么验证>"]` 划。
切分的标准是每个包能独立验证：自己的路径、自己的验证、能定位的判据。路径盖住它会改的每个文件，测试也算，包之间不重叠；
实施时一个包只能写它的路径。项目的验证跑整棵树：一个包改了别处在用的东西（签名、返回值），会因此失败的测试和被测文件都要进这个包。
有先后用 `--after`。交出的 `packages` 就是划的这些：`{id, title, paths, after}`，与 plan 一致。

**台账**：以后要回头看的决定 `xledger add decisions "<决定了什么、为什么>" --ref <需求或元素编号>`，可能出的问题
`xledger add risks "<风险与对策>" --ref …`。离开前每条都要有结论：`xledger set <种类> <编号> accepted|rejected|resolved|deferred "<说明>"`。
接口增量里有 `breaking` 的改动时，要一条 accepted 的决定 `--ref` 指向那个元素，启动时的 impact 里也要有 interface。

规格之间有矛盾、或提案的目标做不到，不要自己裁决：问题根在提案，用 `flow_propose {kind: rework, to: propose, reason}` 建议打回，
说清是哪两条、为什么不能两立。

**被打回时**先 `xledger list findings` 看 open 的发现：照它改设计、规格增量（xspec）或接口增量（xarch），再
`xledger set findings <编号> resolved "<怎么解决的>"`；不同意就 `disputed "<理由>"`，由检查站或人定。改了增量就同步改
`proposal.md` 的「改什么」：批准的人看的是提案，提案与增量对不上就是没解决。
