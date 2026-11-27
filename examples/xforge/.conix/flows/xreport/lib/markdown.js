// analysis.md 的小节读法：门禁 analysis-filled 判有没有写，xstat render 把它们搬进 REPORT.md，用同一份，
// 两边对"一节到哪为止""注释算不算正文"不会各说各的。小节是 `## <标题>` 一行到下一个一二级标题之前。

// 分析的五节，按这个顺序：skill、模板、门禁 analysis-sections 与这里一致。
export const SECTIONS = ["概览", "贡献者", "时间分布", "热点", "观察"];

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

// 表格的一格：| 与换行会把表拆坏。名字、路径是 git 里的原样，什么字符都可能有。
export function cell(v) {
  return String(v == null ? "" : v).replace(/\\/g, "\\\\").replace(/\|/g, "\\|").replace(/[\r\n]+/g, " ");
}

export function table(head, rows) {
  const line = (r) => `| ${r.map(cell).join(" | ")} |`;
  return [line(head), `|${head.map(() => "---").join("|")}|`, ...rows.map(line)].join("\n");
}
