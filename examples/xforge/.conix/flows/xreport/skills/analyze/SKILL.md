---
name: analyze
description: 只照 stats.json 写这段时间提交历史的分析 analysis.md。
---

只读 `${output_dir}/stats.json`（`xstat show` 给一份摘要）。不读源码、不跑 git、不翻别的文件：要的数都在 stats.json 里。
报告里的每个数都要能在 stats.json 里找到，不估算、不补它没有的（工时、效率、谁好谁差）；看不出的明说"从数据看不出"。

人名照 stats.json 写（就是 git 里的名字，`.mailmap` 合过），不改写、不起代号。stats.json 里没有邮箱就不写邮箱。
`commits[].subject` 是提交说明，没有（设置关掉了）就只凭数说；`truncated` 为 true 时统计只到上限，概览里要说。

`analysis.md` 五节，标题照模板：

- **概览**：范围（`range`）、提交与合并数、作者数、增删行、文件数。
- **贡献者**：每位作者的提交、合并、增删行、文件、活跃天数、首次与最近。只陈述。
- **时间分布**：`by_weekday`、`by_hour`（作者自己的时区）、`by_day`：集中在哪、哪段没有。
- **热点**：`by_dir`、`top_files`：改得最多的目录与文件。
- **观察**：几条从数与提交说明看出来的事（主要在做什么、大块的改动、只有一个人碰的目录）。

表不用画：定稿时 `xstat render` 从 stats.json 把表生成在报告后面，这里写文字、引用关键的数。
写完交 `flow_submit {output: {summary: "<一句话>"}}`。
