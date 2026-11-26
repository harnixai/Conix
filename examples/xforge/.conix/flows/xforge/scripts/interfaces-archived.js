// interfaces-archived：接口增量都并进了 interfaces/：归档站用 xarch archive 并，这里逐条核对（lib/arch.js）。
import { governance, notOwed } from "../lib/governance.js";
import { readBaseline, readDelta, checkMerged, isEmpty } from "../lib/arch.js";

export default function (input, flow) {
  if (!governance(flow).interface) return notOwed("interface");
  const p = input.params;
  const delta = readDelta(flow, p.delta);
  if (isEmpty(delta)) return { verdict: "pass", summary: "没有接口增量要并", findings: [], metrics: { elements: 0 } };
  const base = readBaseline(flow, p.interfaces);
  const findings = checkMerged(base, delta).map((message) => ({ rule: "interfaces-archived", severity: "error", path: p.interfaces, message }));
  return {
    verdict: findings.length ? "fail" : "pass",
    summary: findings.length ? `${findings.length} 处没并好` : `接口增量都并进了基线，现在共 ${base.byId.size} 个元素`,
    findings,
    metrics: { elements: base.byId.size },
  };
}
