// interface-compat：破坏性的接口改动要有交代——启动时声明了影响面 interface，且 decisions 台账里有一条指向它、结论 accepted。
// 判法在 lib/arch.js。
// 结果带事实 breaking（有没有破坏性改动）：站上"批准破坏性改动"那条确认的 when 读它。没开接口基线、没有增量也明确回 false：
// 不回这个事实，那条确认就当要（说不清就要人）。
import { governance, notOwed } from "../lib/governance.js";
import { readDelta, checkCompat, breakingOf } from "../lib/arch.js";

export default function (input, flow) {
  if (!governance(flow).interface) return { ...notOwed("interface"), facts: { breaking: false } };
  const p = input.params;
  const delta = readDelta(flow, p.delta);
  const breaking = breakingOf(delta);
  if (!breaking.length) return { verdict: "pass", summary: "没有破坏性的接口改动", findings: [], metrics: { breaking: 0 }, facts: { breaking: false } };
  const impact = (input.inputs && input.inputs.impact) || [];
  const findings = checkCompat(delta, impact, flow.read.yaml(p.decisions)).map((message) => ({ rule: "interface-compat", severity: "error", path: p.decisions, message }));
  return {
    verdict: findings.length ? "fail" : "pass",
    summary: findings.length ? `${breaking.length} 处破坏性改动交代不全` : `${breaking.length} 处破坏性改动都有交代：${breaking.join("、")}`,
    findings,
    metrics: { breaking: breaking.length },
    facts: { breaking: true },
  };
}
