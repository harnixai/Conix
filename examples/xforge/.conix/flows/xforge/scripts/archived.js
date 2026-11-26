// archived：规格增量都并进了 specs/：归档站用 xspec archive 并，这里逐条核对（lib/delta.js）。
import { readSpecs } from "../lib/specs.js";
import { checkMerged } from "../lib/delta.js";

export default function (input, flow) {
  const p = input.params;
  const delta = flow.read.yaml(p.delta);
  if (delta == null) return { verdict: "error", summary: `${p.delta} 不存在` };
  const specs = readSpecs(flow, p.specs);
  const findings = checkMerged(specs, delta).map((message) => ({ rule: "archived", severity: "error", path: p.specs, message }));
  return {
    verdict: findings.length ? "fail" : "pass",
    summary: findings.length ? `${findings.length} 处没并好` : `增量都并进了规格，现在共 ${specs.byId.size} 条需求`,
    findings,
    metrics: { requirements: specs.byId.size },
  };
}
