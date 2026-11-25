// sources-cited：提纲里每一节的来源笔记，都要出现在简报的「## 来源」一节里；每一节的标题都要是简报里的一个二级标题。
export default function (input, flow) {
  const outline = flow.read.json(input.params.outline);
  const brief = flow.read.text(input.params.brief);
  if (outline == null || brief == null) {
    // 没有可判的东西不是交付不合格，是判不了：error，要人看。
    return { verdict: "error", summary: `${outline == null ? input.params.outline : input.params.brief} 不存在` };
  }
  const at = brief.search(/^## 来源/m);
  const cited = at < 0 ? "" : brief.slice(at);
  const findings = [];
  const sources = new Set();
  for (const s of outline.sections || []) {
    for (const src of s.sources || []) sources.add(src);
    if (!new RegExp(`^##\\s+${escape(s.title)}\\s*$`, "m").test(brief)) {
      findings.push({ rule: "section", severity: "error", path: input.params.brief, message: `没有「## ${s.title}」这一节` });
    }
  }
  for (const src of sources) {
    if (!cited.includes(src)) {
      findings.push({ rule: "source", severity: "error", path: input.params.brief, message: `「## 来源」里没有列出 ${src}` });
    }
  }
  flow.log(`sources: ${[...sources].join(", ")}`);
  return {
    verdict: findings.length ? "fail" : "pass",
    summary: findings.length ? `${findings.length} 处没对上` : `${sources.size} 个来源、${(outline.sections || []).length} 节都对上了`,
    findings,
    metrics: { sources: sources.size, sections: (outline.sections || []).length },
  };
}

function escape(s) {
  return s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}
