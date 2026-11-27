// analysis-filled：分析的每一节都有正文，没有留着待补的。
// 不用 check: text：要的是"这一节有正文"，正则看不出一节到哪为止；判法与 xstat render 搬节用同一份（lib/markdown.js）。
import { SECTIONS, section, stripComments } from "../lib/markdown.js";

// 只认"这一行是个占位"：行首（列表记号之后）就是 TODO、TBD、待补。提交说明里常有 TODO（"清理 TODO"），
// 分析引用它不算没写完。
const PLACEHOLDER = /^\s*(?:[-*+]\s+|\d+\.\s+)?[（(【\[]?\s*(TODO|TBD|待补)/;

export default function (input, flow) {
  const p = input.params;
  const text = flow.read.text(p.analysis);
  if (text == null) return { verdict: "error", summary: `${p.analysis} 不存在` };
  const findings = [];
  const bad = (message) => findings.push({ rule: "analysis-filled", severity: "error", path: p.analysis, message });
  for (const s of SECTIONS) {
    const body = section(text, s);
    if (body == null) bad(`没有「## ${s}」一节`);
    else if (!body) bad(`「## ${s}」是空的`);
  }
  stripComments(text).split("\n").forEach((l) => {
    const m = PLACEHOLDER.exec(l);
    if (m) bad(`还有占位的 ${m[1]}：${l.trim()}`);
  });
  return {
    verdict: findings.length ? "fail" : "pass",
    summary: findings.length ? `${findings.length} 处没写好` : "五节都写了",
    findings,
  };
}
