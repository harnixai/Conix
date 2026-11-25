// packages-planned：设计交出的工作包与 xpkg 划的一一对得上，每个都划了路径。
export default function (input, flow) {
  const p = input.params;
  const plan = flow.read.yaml(p.plan);
  if (plan == null) return { verdict: "fail", summary: "还没有工作包：用 xpkg plan 划", findings: [{ rule: "packages", severity: "error", path: p.plan, message: "没有 work-packages.yaml" }] };
  const planned = new Map((plan.packages || []).map((x) => [x.id, x]));
  const handed = (p.handed || []).map((x) => (typeof x === "string" ? x : x.id));
  const findings = [];
  const bad = (message) => findings.push({ rule: "packages", severity: "error", path: p.plan, message });
  for (const id of handed) if (!planned.has(id)) bad(`交出的工作包 ${id} 没有用 xpkg plan 划`);
  for (const id of planned.keys()) if (!handed.includes(id)) bad(`划了的工作包 ${id} 没有交出来`);
  for (const x of planned.values()) if (!(x.paths || []).length) bad(`工作包 ${x.id} 没有路径`);
  return {
    verdict: findings.length ? "fail" : "pass",
    summary: findings.length ? `${findings.length} 处对不上` : `${planned.size} 个工作包，路径都划了`,
    findings,
    metrics: { packages: planned.size },
  };
}
