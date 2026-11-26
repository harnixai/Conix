// spec-delta：规格增量对得上 specs/ 的基线。判法在 lib/delta.js，命令 xspec check 用的是同一份：
// 编号与基线、每条正文的 WHEN / THEN、与别的在途运行（flow.runs）的冲突。
import { governance, notOwed } from "../lib/governance.js";
import { readSpecs } from "../lib/specs.js";
import { checkDelta, counts, inflight } from "../lib/delta.js";

export default function (input, flow) {
  if (!governance(flow).spec) return notOwed("spec");
  const p = input.params;
  const delta = flow.read.yaml(p.delta);
  if (delta == null) return { verdict: "error", summary: `${p.delta} 不存在` };
  const specs = readSpecs(flow, p.specs);
  if (specs.byId.size === 0) return { verdict: "error", summary: `${p.specs} 下读不到一条需求` };

  const others = inflight(flow, input.run);
  const findings = checkDelta(specs, delta, { others }).map((message) => ({ rule: "spec-delta", severity: "error", path: p.delta, message }));
  const c = counts(delta);
  return {
    verdict: findings.length ? "fail" : "pass",
    summary: findings.length ? `${findings.length} 处对不上基线` : `增量对得上基线：新增 ${c.added}、修改 ${c.modified}、删除 ${c.removed}`,
    findings,
    metrics: { ...c, baseline: specs.byId.size, in_flight: others.length },
  };
}
