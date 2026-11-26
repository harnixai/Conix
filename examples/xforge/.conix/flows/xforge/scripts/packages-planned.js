// packages-planned：交出的工作包与 xpkg plan 划的一一对得上；每个有路径、至少一条判据；
// after 指的包存在、不成环；包的路径不在工作区的同一个文件上重叠（lib/packages.js 讲了"重叠"怎么判）。
import { loadPlan, planProblems } from "../lib/packages.js";

export default function (input, flow) {
  const p = input.params;
  const plan = loadPlan(flow, p.plan);
  if (plan == null) return { verdict: "fail", summary: "还没有工作包：用 xpkg plan 划", findings: [{ rule: "packages", severity: "error", path: p.plan, message: "没有 work-packages.yaml" }] };
  const planned = new Map(plan.packages.map((x) => [x.id, x]));
  const handed = (p.handed || []).map((x) => (typeof x === "string" ? x : x.id));
  const messages = [];
  for (const id of handed) if (!planned.has(id)) messages.push(`交出的工作包 ${id} 没有用 xpkg plan 划`);
  for (const id of planned.keys()) if (!handed.includes(id)) messages.push(`划了的工作包 ${id} 没有交出来`);
  messages.push(...planProblems(flow, plan.packages));
  if (!planned.size) messages.push("一个工作包也没划");
  const findings = messages.map((message) => ({ rule: "packages", severity: "error", path: p.plan, message }));
  return {
    verdict: findings.length ? "fail" : "pass",
    summary: findings.length ? `${findings.length} 处对不上` : `${planned.size} 个工作包，路径、判据、先后都对`,
    findings,
    metrics: { packages: planned.size },
  };
}
