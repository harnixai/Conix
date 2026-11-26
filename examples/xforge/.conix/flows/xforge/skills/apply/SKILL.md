---
name: apply
description: 实施一个工作包：只改包的路径，每条判据做到并留证据。
---

你的工作项是一个工作包。`xpkg list` 看它的路径、判据、验证方法和先后；读 `proposal.md`、`xspec delta`、`xarch delta`，
有设计就读 `design.md` 里与这个包有关的几节。弄清这个包要满足什么，再动手。

只改包的路径里的文件（别处写不进去），小步改。新增、修改的每条需求与接口元素，测试里各加一条用例，用注释 `# <编号>`
标明它测的是哪条（`# TK-R3`、`# fn:textkit.truncate`）。照包写的验证方法自己跑一次，再跑项目的 `.conix/governance/verify.sh`：
出口跑的就是它，跑整棵树。

做完登记：`xpkg done <编号> "<做了什么>" --files a,b --evidence "C1=<怎么证明的，如 tests/test_x.py#test_y 通过>;C2=…"`。
每条判据一条证据，`--files` 列出你改的每个文件；交出 `{package, status: done, summary}`。登记会被核对：改了没列的、
列了不在路径里的、判据没证据的，都会被退回来改。

做不下去就 `xpkg blocked <编号> "<卡在哪>"`，交出 `status: blocked`，不要硬交。不改计划：包划错了（路径不够、判据做不到、
要先改别的包），问题在设计，说清楚、建议打回设计（快速变更里是提案）。
