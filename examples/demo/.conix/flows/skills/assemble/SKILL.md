---
name: assemble
description: 把各节汇总成一份简报。
---

读 `${output_dir}/outline.json` 和 `${output_dir}/sections/` 下的每一节，写 `${output_dir}/brief.md`：

```
# <提纲里的 title>

## 摘要
三到五句话，说清这份简报的要点。

## <第 1 节的 title>
（这一节的正文，可以在分节稿上精简、衔接，不加笔记里没有的内容）

## <第 2 节的 title>
……

## 结论
两到三条可执行的建议。

## 来源
- notes/xxx.md
（提纲里所有出现过的来源笔记，每行一个，路径原样照写）
```

- 每一节的标题必须和提纲里的 `title` 完全一样，单独一行 `## <title>`。
- 全文 300 到 3000 字。不要留 TODO、TBD。
- 写完不要再改分节稿和提纲。

交付 `{"file": "<brief.md 的路径>", "summary": "<摘要那几句>"}`。
