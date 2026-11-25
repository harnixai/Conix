// constitution：设计逐条对照了项目原则（.conix/governance/constitution.yaml）。
export default function (input, flow) {
  const p = input.params;
  const doc = flow.read.yaml(p.constitution);
  const design = flow.read.text(p.design);
  if (design == null) return { verdict: "error", summary: `${p.design} 不存在` };
  if (doc == null) {
    // 项目还没写原则：没有可对照的，不挡设计，提醒一声。
    return {
      verdict: "pass",
      summary: `项目没有原则文件（${p.constitution}），没有对照`,
      findings: [{ rule: "constitution", severity: "warn", path: p.constitution, message: "写一份 principles: [{id: P1, title, text}]，设计站就会逐条对照" }],
    };
  }
  const principles = doc.principles || [];
  if (!principles.length) return { verdict: "error", summary: `${p.constitution} 里没有原则` };

  const at = design.search(/^## 原则对照/m);
  const findings = [];
  if (at < 0) {
    findings.push({ rule: "constitution", severity: "error", path: p.design, message: "没有「## 原则对照」一节" });
  }
  const rest = at < 0 ? [] : design.slice(at).split("\n").slice(1);
  const end = rest.findIndex((l) => /^## /.test(l));
  const lines = (end < 0 ? rest : rest.slice(0, end)).map((l) => l.trim()).filter(Boolean);
  const known = new Set(principles.map((x) => x.id));
  const answered = new Map();
  for (const l of lines) {
    const m = /^[-*]?\s*\**(P\d+)\**\s*[：:、.\s]\s*(.*)$/.exec(l);
    if (!m) continue;
    if (!known.has(m[1])) {
      findings.push({ rule: "constitution", severity: "error", path: p.design, message: `${m[1]} 不是项目的原则` });
    }
    answered.set(m[1], m[2]);
  }
  if (at >= 0) {
    for (const x of principles) {
      const why = answered.get(x.id);
      if (why == null) {
        findings.push({ rule: "constitution", severity: "error", path: p.design, message: `没有对照 ${x.id}（${x.title}）` });
      } else if ([...why].length < 10) {
        findings.push({ rule: "constitution", severity: "error", path: p.design, message: `${x.id} 只写了「${why}」：说清设计怎么守它` });
      }
    }
  }
  return {
    verdict: findings.length ? "fail" : "pass",
    summary: findings.length ? `${findings.length} 处没对照好` : `${principles.length} 条原则都对照了`,
    findings,
    metrics: { principles: principles.length, answered: answered.size },
  };
}
