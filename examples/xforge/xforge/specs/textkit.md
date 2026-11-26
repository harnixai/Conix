# textkit 的规格

<!-- xspec: prefix TK -->

每条需求一个三级标题：`### <编号> <标题>`，下面是正文。编号是 `TK-R<n>`，只增不复用。
正文写成 WHEN / THEN 条目（`- WHEN …` / `- THEN …`，或中文 `- 当 … 时` / `- 应 …`），能直接写成黑盒测试。
变更不直接改这里：xforge 流程里用 `xspec` 把改动记成增量，归档站用 `xspec archive` 并进来。

### TK-R1 生成短名
- WHEN 对一段文本调用 `slugify(text)`
- THEN 返回小写的短名：非字母数字的连续字符换成一个 `-`，去掉首尾的 `-`

### TK-R2 数单词
- WHEN 对一段文本调用 `word_count(text)`
- THEN 返回以空白分隔的非空片段个数
- WHEN 文本全是空白
- THEN 返回 0
