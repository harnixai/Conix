// governance：读项目的治理文件（governance/policy.yaml），对照提纲、简报和两份审读，给出这份简报能不能签发的结论。
//
// 规矩写在治理文件里，这里只照着判：改规矩不用改脚本。审读员（模型）判的是 style 里要读懂了才判得了的规矩，
// 这里不替它们重判，只核对它们每条都判了，再按治理文件定的 severity 决定哪些违反挡住签发。
export default function (input, flow) {
  const p = input.params;
  const policy = flow.read.yaml(p.policy);
  const outline = flow.read.yaml(p.outline);
  const brief = flow.read.text(p.brief);
  const missing = [[policy, p.policy], [outline, p.outline], [brief, p.brief]].filter(([v]) => v == null).map(([, f]) => f);
  if (missing.length) {
    // 没有可判的东西不是交付不合格，是判不了：error，要人看。
    return { verdict: "error", summary: `${missing.join("、")} 不存在` };
  }

  const findings = [];
  const add = (rule, severity, message, path) => findings.push({ rule, severity, message, path });
  const rules = policy.brief || {};
  const sections = outline.sections || [];

  // 1. 该有的节：治理文件要的，加上提纲里的每一节。
  for (const t of [...(rules.required_sections || []), ...sections.map((s) => s.title)]) {
    if (!new RegExp(`^##\\s+${escape(t)}\\s*$`, "m").test(brief)) add("brief.section", "error", `没有「## ${t}」这一节`, p.brief);
  }
  // 2. 提纲里每个来源都列进了「## 来源」。
  const at = brief.search(/^## 来源/m);
  const cited = at < 0 ? "" : brief.slice(at);
  const sources = new Set(sections.flatMap((s) => s.sources || []));
  for (const src of sources) {
    if (!cited.includes(src)) add("brief.source", "error", `「## 来源」里没有列出 ${src}`, p.brief);
  }
  // 3. 禁用的说法与篇幅。
  for (const w of rules.banned || []) {
    if (brief.includes(w)) add("brief.banned", "error", `用了治理文件禁用的说法「${w}」`, p.brief);
  }
  const chars = [...brief].length;
  if (rules.max_chars && chars > rules.max_chars) add("brief.length", "error", `${chars} 字，治理文件定的至多 ${rules.max_chars}`, p.brief);

  // 4. 事实审读：找不到依据的说法不超过治理文件定的条数。
  const rv = policy.reviews || {};
  const factsFile = `${p.reviews}/facts.jsonl`;
  const facts = flow.read.jsonl(factsFile);
  let unsupported = [];
  if (facts == null) {
    add("reviews.facts", "error", "没有事实审读", factsFile);
  } else {
    unsupported = facts.filter((r) => r.verdict === "unsupported");
    const max = (rv.facts && rv.facts.max_unsupported) ?? 0;
    const severity = unsupported.length > max ? "error" : "warn";
    for (const r of unsupported) add("reviews.facts", severity, `没有依据：${r.claim}`, factsFile);
  }

  // 5. 规范审读：style 的每条都判了；违反的按 severity 定挡不挡。
  const style = policy.style || [];
  const policyFile = `${p.reviews}/policy.jsonl`;
  const judged = flow.read.jsonl(policyFile);
  let violated = 0;
  if (judged == null) {
    add("reviews.policy", "error", "没有规范审读", policyFile);
  } else {
    const block = new Set((rv.policy && rv.policy.block) || ["error"]);
    const byId = new Map(judged.map((r) => [r.rule, r]));
    for (const s of style) {
      const r = byId.get(s.id);
      if (!r) {
        add("reviews.policy", "error", `治理文件的规矩 ${s.id} 没有判`, policyFile);
      } else if (r.verdict === "violated") {
        violated += 1;
        add(`style.${s.id}`, block.has(s.severity) ? "error" : "warn", `违反「${s.rule}」：${r.message}`, p.brief);
      }
    }
  }

  const errors = findings.filter((f) => f.severity === "error").length;
  const warns = findings.length - errors;
  flow.log(`policy v${policy.version}: ${errors} blocking, ${warns} warnings`);
  return {
    verdict: errors ? "fail" : "pass",
    summary: errors
      ? `不可签发：${errors} 处不合治理文件${warns ? `，另有 ${warns} 条提醒` : ""}`
      : `可以签发：治理文件的规矩都守住了${warns ? `，${warns} 条提醒留给编辑` : ""}`,
    findings,
    metrics: {
      sections: sections.length,
      sources: sources.size,
      chars,
      claims: facts ? facts.length : 0,
      unsupported: unsupported.length,
      rules: style.length,
      violated,
      blocking: errors,
    },
  };
}

function escape(s) {
  return s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}
