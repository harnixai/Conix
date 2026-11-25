---
name: review-policy
description: 按治理文件的 style 规矩逐条审读简报，写成 JSONL。
---

你是规范审读员。读项目的治理文件 `governance/policy.yaml` 和 `${output_dir}/brief.md`，需要时对照 `notes/` 下的笔记。

治理文件的 `style` 里每条规矩（`id`、`severity`、`rule`）都要判一次，写进 `${output_dir}/reviews/policy.jsonl`，
**一行一个 JSON 对象**，每条规矩一行：

```
{"rule": "<规矩的 id>", "verdict": "ok", "message": "<为什么算守住了，一句话>"}
{"rule": "<规矩的 id>", "verdict": "violated", "message": "<哪一句违反了，引出原文>"}
```

- 确实违反才写 `violated`，并在 `message` 里引出简报里违反的那句；拿不准的写 `ok`，在 `message` 里写下提醒。
- 不要判治理文件里没有的规矩。只写这一个文件，不要改简报。

交付 `{"file": "<policy.jsonl 的路径>", "count": <写了几条>}`。
