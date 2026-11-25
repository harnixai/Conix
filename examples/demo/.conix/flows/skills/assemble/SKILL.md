---
name: assemble
description: 把各节汇总成一份简报。
---

读 `${output_dir}/outline.yaml` 和 `${output_dir}/sections/` 下的每一节，写 `${output_dir}/brief.md`：

```
# <提纲里的 title>

## 摘要
## <每一节的 title，和提纲里完全一样，按提纲的顺序>
## 结论
## 来源
```

- 摘要三到五句，读完它就知道要做什么。
- 各节在分节稿上精简、衔接，去掉重复；不加笔记里没有的内容。
- 结论两到三条，每条是能照着做的建议。
- 来源列出提纲里出现过的每份笔记，一行一个，路径原样照写。
- 读一遍 `governance/policy.yaml`：那里禁用的说法不要用。
- 分节稿和提纲是前面几站的交付，不要改。
