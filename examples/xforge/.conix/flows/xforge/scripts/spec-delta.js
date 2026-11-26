// spec-delta：规格增量对得上 specs/ 的基线。判法在 lib/delta.js，命令 xspec check 用的是同一份。
import { readSpecs } from "../lib/specs.js";
import { checkDelta, counts } from "../lib/delta.js";

export default function (input, flow) {
  const p = input.params;
  const delta = flow.read.yaml(p.delta);
  if (delta == null) return { verdict: "error", summary: `${p.delta} 不存在` };
  const specs = readSpecs(flow, p.specs);
  if (specs.byId.size === 0) return { verdict: "error", summary: `${p.specs} 下读不到一条需求` };

  const findings = checkDelta(specs, delta).map((message) => ({ rule: "spec-delta", severity: "error", path: p.delta, message }));
  const c = counts(delta);
  return {
    verdict: findings.length ? "fail" : "pass",
    summary: findings.length ? `${findings.length} 处对不上基线` : `增量对得上基线：新增 ${c.added}、修改 ${c.modified}、删除 ${c.removed}`,
    findings,
    metrics: { ...c, baseline: specs.byId.size },
  };
}
