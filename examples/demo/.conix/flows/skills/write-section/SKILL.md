---
name: write-section
description: 按提纲里的一节，依据它的来源笔记写出这一节。
---

你的工作项是提纲里的一节：`id`、`title`、`sources`。

1. 读 `sources` 里列的每一份笔记。
2. 写 `${output_dir}/sections/<id>.md`：第一行是 `## <title>`，然后 2 到 3 段，只用笔记里有的内容，不要编造。
   最后一行写 `来源：` 加上这一节的来源路径，用顿号隔开。
3. 不要留 TODO、TBD 这类占位。

交付 `{"file": "<写好的文件路径>"}`。
