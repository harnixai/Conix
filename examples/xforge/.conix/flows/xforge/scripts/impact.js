// impact：提案的「影响面」与启动时声明的 inputs.impact 一致。声明是选路的依据（风险、要不要接口兼容的决定），
// 提案写的影响面是给批准的人看的：两边说的不一样，批的就不是跑的那条路。
import { section, lines } from "../lib/markdown.js";

// 每种影响面认英文名，也认中文说法。
const KINDS = {
  interface: ["interface", "接口"],
  security: ["security", "安全"],
  "data-migration": ["data-migration", "数据迁移"],
  infra: ["infra", "基础设施"],
};

export default function (input, flow) {
  const p = input.params;
  // 引用没解开（试门禁时没给 inputs）就取运行的输入。
  const declared = Array.isArray(p.impact) ? p.impact : (input.inputs && input.inputs.impact) || [];
  const text = flow.read.text(p.proposal);
  if (text == null) return { verdict: "error", summary: `${p.proposal} 不存在` };
  const body = section(text, "影响面");
  const findings = [];
  const bad = (message, severity = "error") => findings.push({ rule: "impact", severity, path: p.proposal, message });
  if (body == null) {
    bad("没有「## 影响面」一节");
  } else if (!declared.length) {
    if (!/^[-*\s]*无/.test(body)) bad("启动时没有声明影响面（impact 为空）：「影响面」写\"无\"；确实有影响，就要换路或重开时声明它");
  } else {
    const rows = lines(body);
    for (const k of declared) {
      const names = KINDS[k] || [k];
      const row = rows.find((l) => names.some((n) => l.includes(n)));
      if (!row) bad(`启动时声明了 ${k}，「影响面」里没写到（写成 ${names.join(" 或 ")}）`);
      else if ([...row.replace(new RegExp(names.join("|"), "g"), "").replace(/[-*：:\s]/g, "")].length < 4) bad(`${k} 只写了名字：说清影响什么`);
    }
  }
  if (body != null) {
    // 中文说法常出现在"不影响接口"这样的句子里：没声明的只看英文名，也只提醒。
    for (const k of Object.keys(KINDS)) {
      if (!declared.includes(k) && new RegExp(`(^|[^\\w-])${k}(?![\\w-])`).test(body)) {
        bad(`「影响面」写到了 ${k}，启动时没有声明它：真有这一项就换路或重开时声明`, "warn");
      }
    }
  }
  const errors = findings.filter((f) => f.severity === "error").length;
  return {
    verdict: errors ? "fail" : "pass",
    summary: errors ? `${errors} 处与声明的影响面不一致` : declared.length ? `影响面 ${declared.join("、")} 都写到了` : "没有声明影响面，提案写了无",
    findings,
    metrics: { declared: declared.length },
  };
}
