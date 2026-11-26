// ledgers-settled：运行目录 ledgers/ 里的每份台账都合 §2.3 的规矩（lib/ledger.js，xledger 写之前查的是同一份）。
import { ledgerProblems } from "../lib/ledger.js";

export default function (input, flow) {
  const dir = input.params.dir;
  const files = flow.glob(`${dir}/*.yaml`);
  const findings = [];
  let entries = 0;
  for (const path of files) {
    const kind = flow.path.basename(path, ".yaml");
    let doc;
    try {
      doc = flow.read.yaml(path);
    } catch (e) {
      findings.push({ rule: "ledger", severity: "error", path, message: `读不了：${e.message}` });
      continue;
    }
    entries += ((doc && doc.entries) || []).length;
    for (const p of ledgerProblems(kind, doc)) {
      findings.push({ rule: "ledger", severity: "error", path, message: `${p.id ? p.id + "：" : ""}${p.message}` });
    }
  }
  return {
    verdict: findings.length ? "fail" : "pass",
    summary: findings.length ? `${findings.length} 处台账没结清` : files.length ? `${files.length} 份台账、${entries} 条都结清了` : "没有台账，没有悬而未决的",
    findings,
    metrics: { ledgers: files.length, entries },
  };
}
