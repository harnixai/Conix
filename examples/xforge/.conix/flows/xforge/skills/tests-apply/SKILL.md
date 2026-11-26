---
name: tests-apply
description: 双路开发测试那一路，写一个工作包的黑盒测试：只照规格、只经公开接口，每条判据做到并留证据。
---

你的工作项是一个工作包。`xpkg list` 看它的路径、判据与先后；读这一路的 `${output_dir}/design.md` 里与它有关的几节，
以及规格侧 `${parent.output_dir}` 的 `spec-delta.yaml`、`interface-delta.yaml`（只读：批准过的，门禁 spec-unchanged 核对）。

每条 WHEN / THEN 写成用例：WHEN 是怎么调、给什么，THEN 是断言什么。只经公开接口（接口基线与增量里登记的元素）调用，
不引内部模块、不读被测模块的实现来决定怎么断言；规格没说的（报错的具体文字、内部结构）不断言。每个用例带注释 `# <编号>`
（`# TK-R3`、`# fn:textkit.truncate`）。

只改包的路径里的文件（黑盒测试的目录里）。新需求的用例在你这里**本来就过不了**：实现在另一路，合起来时才跑。
你能验证的是测试本身：能被发现、能加载，测已有需求的用例照旧过——照 `xpkg verify-menu` 里 `blackbox` 那一项的命令跑一遍，
没过的只能是测新需求、因为还没有实现而失败的那些，把这一点写进证据。

做完登记：`xpkg done <编号> "<写了哪些用例>" --files a,b --evidence "C1=…;C2=…"`，交出 `{package, status: done, summary}`。
做不下去就 `xpkg blocked <编号> "<卡在哪>"`，交出 `status: blocked`；规格说不清怎么测的，回设计记风险。
