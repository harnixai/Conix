---
name: outline
description: 读笔记，列出简报的提纲。
---

读项目 `notes/` 下的每一份笔记，为本次运行的主题列一份简报的提纲，写在 `${output_dir}/outline.yaml`
（`title` 加 `sections`）。

- 按笔记的内容分节，2 到 3 节；相近的笔记可以合成一节，每份笔记至少被一节用到。
- 节的 `id` 用能看出内容的英文短词（比如 `timeouts`），`title` 是给读者看的中文标题。
- `sources` 只列这一节真正依据的笔记。
