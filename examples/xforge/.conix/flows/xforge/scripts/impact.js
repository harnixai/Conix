// impact：提案的「影响面」与启动时声明的 inputs.impact、与增量实际动到的一致。声明是选路的依据（风险、要不要接口兼容的决定），
// 提案写的影响面是给批准的人看的：两边说的不一样，批的就不是跑的那条路。
// 增量看得出来的（接口增量有改动 → interface）比声明多出来时，不叫模型改提案去迁就声明——那等于叫它把实话删掉；
// 提案照实写，结果带事实 widened，由主人在「影响面扩大」上确认。
import { section, lines } from "../lib/markdown.js";
import { governance } from "../lib/governance.js";
import { readDelta, isEmpty } from "../lib/arch.js";

// 每种影响面认英文名，也认中文说法。
const KINDS = {
  interface: ["interface", "接口"],
  security: ["security", "安全"],
  "data-migration": ["data-migration", "数据迁移"],
  infra: ["infra", "基础设施"],
};

// 增量看得出来的影响面：现在只有接口（开着接口基线、增量不空）。
function seenInDeltas(flow, p) {
  const seen = [];
  if (p.delta && governance(flow).interface && !isEmpty(readDelta(flow, p.delta))) seen.push("interface");
  return seen;
}

export default function (input, flow) {
  const p = input.params;
  // 引用没解开（试门禁时没给 inputs）就取运行的输入。
  const declared = Array.isArray(p.impact) ? p.impact : (input.inputs && input.inputs.impact) || [];
  const widened = seenInDeltas(flow, p).filter((k) => !declared.includes(k));
  const want = declared.concat(widened);
  const facts = { widened: widened.length > 0, undeclared: widened };
  const text = flow.read.text(p.proposal);
  if (text == null) return { verdict: "error", summary: `${p.proposal} 不存在`, facts };
  const body = section(text, "影响面");
  const findings = [];
  const bad = (message, severity = "error") => findings.push({ rule: "impact", severity, path: p.proposal, message });
  for (const k of widened) {
    bad(`接口增量里有改动，启动时没有声明 ${k}：影响面多了 ${k}，提案照实写；主人在「影响面扩大」上确认后照它跑`, "warn");
  }
  if (body == null) {
    bad("没有「## 影响面」一节");
  } else if (!want.length) {
    if (!/^[-*\s]*无/.test(body)) bad("启动时没有声明影响面（impact 为空），增量里也看不出：「影响面」写\"无\"；确实有影响，就要换路或重开时声明它");
  } else {
    const rows = lines(body);
    for (const k of want) {
      const names = KINDS[k] || [k];
      const why = declared.includes(k) ? "启动时声明了" : "增量里动到了";
      const row = rows.find((l) => names.some((n) => l.includes(n)));
      if (!row) bad(`${why} ${k}，「影响面」里没写到（写成 ${names.join(" 或 ")}）`);
      else if ([...row.replace(new RegExp(names.join("|"), "g"), "").replace(/[-*：:\s]/g, "")].length < 4) bad(`${k} 只写了名字：说清影响什么`);
    }
  }
  if (body != null) {
    // 中文说法常出现在"不影响接口"这样的句子里：没声明的只看英文名，也只提醒。
    for (const k of Object.keys(KINDS)) {
      if (!want.includes(k) && new RegExp(`(^|[^\\w-])${k}(?![\\w-])`).test(body)) {
        bad(`「影响面」写到了 ${k}，启动时没有声明它、增量里也看不出：真有这一项就换路或重开时声明`, "warn");
      }
    }
  }
  const errors = findings.filter((f) => f.severity === "error").length;
  const summary = errors
    ? `${errors} 处与影响面不一致`
    : want.length
      ? `影响面 ${want.join("、")} 都写到了${widened.length ? `（${widened.join("、")} 是增量里看出来的，要主人确认）` : ""}`
      : "没有声明影响面，提案写了无";
  return { verdict: errors ? "fail" : "pass", summary, findings, metrics: { declared: declared.length, widened: widened.length }, facts };
}
