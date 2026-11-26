// design-sections：设计的每一节都有正文，「被否决的方案」至少一条有理由的替代。
// 不用 check: text：要的是"这一节有正文"、"那一节里有一条形状对的行"，正则看不出一节到哪为止。
import { section, lines, stripComments } from "../lib/markdown.js";

const SECTIONS = ["做法", "集成点", "失败模式", "迁移与回滚", "被否决的方案", "原则对照", "工作包"];

export default function (input, flow) {
  const p = input.params;
  const text = flow.read.text(p.design);
  if (text == null) return { verdict: "error", summary: `${p.design} 不存在` };
  const findings = [];
  const bad = (message) => findings.push({ rule: "design-sections", severity: "error", path: p.design, message });
  for (const s of SECTIONS) {
    const body = section(text, s);
    if (body == null) bad(`没有「## ${s}」一节`);
    else if (!body) bad(`「## ${s}」是空的`);
  }
  // 没有替代说明没真的比较过：每条写成"方案：为什么不选（它的代价）"。
  const rejected = lines(section(text, "被否决的方案")).filter((l) => {
    const m = /^[-*]\s*([^：:]+?)\s*[：:]\s*(.+)$/.exec(l);
    return m && [...m[2]].length >= 4;
  });
  if (section(text, "被否决的方案") && !rejected.length) bad("「被否决的方案」没有一条 `- 方案：为什么不选`");
  const body = stripComments(text);
  for (const w of ["TODO", "TBD"]) if (body.includes(w)) bad(`还有 ${w}`);
  return {
    verdict: findings.length ? "fail" : "pass",
    summary: findings.length ? `${findings.length} 处没写好` : `七节都写了，否决了 ${rejected.length} 个方案`,
    findings,
    metrics: { rejected: rejected.length },
  };
}
