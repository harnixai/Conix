---
name: outline
description: 读笔记，列出简报的提纲。
---

读项目 `notes/` 下的每一份笔记（`notes/*.md`），为本次运行的主题列一份简报的提纲：

- 2 到 3 节。每节有 `id`（小写字母、数字、连字符，比如 `timeouts`）、`title`（中文标题）、`sources`
  （这一节依据的笔记路径，写成 `notes/<文件名>.md`，至少一份）。
- 每份笔记至少被一节用到。

把提纲写成 YAML，放在 `${output_dir}/outline.yaml`：

```yaml
title: <简报标题>
sections:
  - id: timeouts
    title: <这一节的中文标题>
    sources: [notes/timeouts.md]
```

交付的输出是 `{"sections": [ ... ]}`，和文件里的 `sections` 一样。
