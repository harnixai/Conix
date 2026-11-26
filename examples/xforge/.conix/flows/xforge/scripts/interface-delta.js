// interface-delta：接口增量对得上 interfaces/ 的基线，与别的在途运行不冲突。判法在 lib/arch.js，命令 xarch check 用的是同一份。
import { governance, notOwed } from "../lib/governance.js";
import { readBaseline, readDelta, checkDelta, otherRuns, counts, isEmpty } from "../lib/arch.js";

export default function (input, flow) {
  if (!governance(flow).interface) return notOwed("interface");
  const p = input.params;
  // 没改接口的变更没有增量文件，不是错：这道门只管写了的增量。
  const delta = readDelta(flow, p.delta);
  if (isEmpty(delta)) return { verdict: "pass", summary: "没有接口改动", findings: [], metrics: counts(delta) };
  const base = readBaseline(flow, p.interfaces);
  const findings = checkDelta(base, delta, { others: otherRuns(flow, input) }).map((message) => ({ rule: "interface-delta", severity: "error", path: p.delta, message }));
  const c = counts(delta);
  return {
    verdict: findings.length ? "fail" : "pass",
    summary: findings.length ? `${findings.length} 处对不上基线` : `接口增量对得上基线：新模块 ${c.modules}、新增 ${c.added}、修改 ${c.modified}、转正 ${c.promoted}、弃用 ${c.deprecated}、删除 ${c.removed}`,
    findings,
    metrics: { ...c, baseline: base.byId.size },
  };
}
