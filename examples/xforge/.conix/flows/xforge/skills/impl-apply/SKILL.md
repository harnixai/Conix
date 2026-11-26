---
name: impl-apply
description: 双路开发实现那一路，实施一个工作包：照规格侧做，只改包的路径，每条判据做到并留证据。
---

你的工作项是一个工作包。`xpkg list` 看它的路径、判据、验证与先后；读这一路的 `${output_dir}/design.md` 里与它有关的几节，
以及规格侧 `${parent.output_dir}` 的 `spec-delta.yaml`、`interface-delta.yaml`（只读：批准过的，门禁 spec-unchanged 核对）。

照规格的 WHEN / THEN 做：另一路只照规格写了黑盒测试，规格没说的行为它不会测，规格说了的它一定测。签名与接口增量里登记的一字不差
（名字、参数、返回），测试经的就是它。

只改包的路径里的文件，不碰黑盒测试的目录（verify 菜单 `blackbox` 那一项跑的）。新增、修改的每条需求与接口元素，单测里各加一条用例，
用注释 `# <编号>` 标明。包登记了验证，交之前照 `xpkg verify-menu` 里的命令自己跑一遍；再跑项目的 `.conix/governance/verify.sh`。

做完登记：`xpkg done <编号> "<做了什么>" --files a,b --evidence "C1=…;C2=…"`，交出 `{package, status: done, summary}`。
登记会被核对：改了没列的、列了不在路径里的、判据没证据的，都会被退回来改。做不下去就 `xpkg blocked <编号> "<卡在哪>"`，
交出 `status: blocked`；包划错了，问题在设计，说清楚、建议打回设计。
