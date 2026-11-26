// constitution-reply：检查站的 constitution 台账（xledger reply 写的）逐条答了项目的每条原则；violates 有说明。
// 设计站对照原则看 constitution 门禁，这是检查站对设计与实现的回答。
import { isCheck, isPerson, showBy } from "../lib/ledger.js";

export default function (input, flow) {
  const p = input.params;
  const doc = flow.read.yaml(p.constitution);
  const principles = (doc && doc.principles) || [];
  if (!principles.length) {
    // 项目还没写原则：没有可答的，不挡，提醒一声。
    return {
      verdict: "pass",
      summary: `项目没有原则（${p.constitution}），没有要答的`,
      findings: [{ rule: "constitution-reply", severity: "warn", path: p.constitution, message: "写一份 principles: [{id: P1, title, text}]，检查站就会逐条答" }],
    };
  }
  const ledger = flow.read.yaml(p.ledger);
  const entries = (ledger && ledger.entries) || [];
  const byId = new Map(entries.map((e) => [e.id, e]));
  const findings = [];
  const bad = (message) => findings.push({ rule: "constitution-reply", severity: "error", path: p.ledger, message });
  for (const x of principles) {
    const e = byId.get(x.id);
    if (!e) { bad(`没有答 ${x.id}（${x.title}）：xledger reply ${x.id} complies|violates|n-a "<说明>"`); continue; }
    if (!["complies", "violates", "n-a"].includes(e.conclusion)) bad(`${x.id} 的结论 ${e.conclusion} 不是 complies / violates / n-a`);
    else if (e.conclusion === "violates" && !String(e.note || "").trim()) bad(`${x.id} 判了 violates 但没有说明`);
    if (!isCheck(e.by) && !isPerson(e.by)) bad(`${x.id} 是 ${showBy(e.by)} 答的：只有检查站或人能答`);
  }
  const known = new Set(principles.map((x) => x.id));
  for (const e of entries) if (!known.has(e.id)) bad(`${e.id} 不是项目的原则`);
  const violated = entries.filter((e) => e.conclusion === "violates").map((e) => e.id);
  return {
    verdict: findings.length ? "fail" : "pass",
    summary: findings.length ? `${findings.length} 处没答好` : `${principles.length} 条原则都答了${violated.length ? `，违反：${violated.join("、")}` : ""}`,
    findings,
    metrics: { principles: principles.length, answered: entries.length, violates: violated.length },
  };
}
