---
name: design
description: 把提案落成设计和工作包，逐条对照项目原则，决定与风险记进台账。
---

读这次运行目录里的 `proposal.md`、`xspec delta`，以及要动的代码与测试，再读项目的 `.conix/governance/constitution.yaml`（有的话）。

设计写清：怎么实现、落到哪些函数和文件（做法）；考虑过、没选的做法和原因（取舍）；每条原则一行
`- P1：<这个设计怎么守它>`，守不住的写明为什么、影响多大（原则对照）；实施拆成哪几包、每包做什么（工作包）。

工作包用 `xpkg plan <编号> "<标题>" --paths "<路径模式，逗号隔开>" --verify "<怎么验证>" --criteria "<标准一;标准二>"` 划：
`paths` 要盖住它会改的每个文件，测试也算，包之间不重叠；之后实施站一包一个子 Agent 照着做。交出的 packages 就是划的这些。

以后要回头看的记进台账：`xledger add decisions "<做了什么决定、为什么>"`，`xledger add risks "<可能出什么问题>"`。
每条都先是 open，离开前都要有结论：`xledger set <种类> <编号> accepted|rejected|resolved|deferred "<说明>"`。

被检查站打回来时，先 `xledger list findings` 看 open 的问题：照它改提案、规格增量（xspec）或设计，再
`xledger set findings <编号> resolved "<怎么解决的>"`；不同意的标 rejected 并写理由。
