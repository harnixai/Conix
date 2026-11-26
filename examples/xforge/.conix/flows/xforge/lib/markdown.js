// 提案、设计、验证记录的小节读法：门禁 proposal 以外的几道（impact、design-sections、constitution、verification）共用。
// 小节是 `## <标题>` 一行到下一个一二级标题之前；HTML 注释（模板里的提示）不算正文。

export function stripComments(text) {
  return String(text || "").replace(/<!--[\s\S]*?-->/g, "");
}

// 标题正好是 title 的那一节的正文（去掉注释、首尾空白）；没有这一节是 null。
export function section(text, title) {
  const lines = String(text || "").split("\n");
  const at = lines.findIndex((l) => l.replace(/\s+$/, "") === `## ${title}`);
  if (at < 0) return null;
  const rest = lines.slice(at + 1);
  const end = rest.findIndex((l) => /^#{1,2}\s/.test(l));
  return stripComments((end < 0 ? rest : rest.slice(0, end)).join("\n")).trim();
}

// 一节里的非空行，去掉首尾空白。
export function lines(body) {
  return String(body || "").split("\n").map((l) => l.trim()).filter(Boolean);
}

// 一行里是不是单独出现了这个编号（TK-R3 不算出现在 TK-R30 里；元素编号带 : 和 .）。
export function mentions(line, id) {
  const esc = id.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  return new RegExp(`(^|[^\\w:.-])${esc}(?![\\w.-])`).test(line);
}
