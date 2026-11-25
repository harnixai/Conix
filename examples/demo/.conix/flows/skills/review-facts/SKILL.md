---
name: review-facts
description: 逐条核对简报里的说法在笔记里有没有依据，写成 JSONL。
---

你是事实核对员。读 `${output_dir}/brief.md`，以及 `notes/` 下它列为来源的每一份笔记。

从简报正文（摘要、各节、结论）里挑出 3 到 8 条具体的说法（一个数字、一条做法、一个因果），逐条到笔记里找依据，
写进 `${output_dir}/reviews/facts.jsonl`，**一行一个 JSON 对象**，不要写成数组，不要加别的行：

```
{"claim": "<简报里的那句说法>", "verdict": "supported", "source": "notes/retries.md", "quote": "<笔记里支撑它的原句，一字不差地抄>"}
{"claim": "<另一句>", "verdict": "unsupported", "source": "", "quote": "", "note": "<为什么找不到>"}
```

- `supported` 的 `quote` 必须是那份笔记里原样有的一句（可以是一句的一部分，但至少十来个字），工具会逐字核对。
- 笔记里确实找不到的才写 `unsupported`。
- 只写这一个文件。不要改简报、提纲和分节稿。

交付 `{"file": "<facts.jsonl 的路径>", "count": <写了几条>}`。
