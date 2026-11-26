// unclaimed：基线提交以来的每个改动（含没提交的；制品目录与 .conix/ 不算，改动列表里已经排除）都落在某个工作包的路径里。
import { changedFiles, inPaths, loadPlan } from "../lib/packages.js";

export default function (input, flow) {
  const p = input.params;
  const plan = loadPlan(flow, p.plan);
  if (plan == null) return { verdict: "error", summary: "没有工作包：用 xpkg plan 划" };
  let changed;
  try {
    changed = changedFiles(flow);
  } catch (e) {
    return { verdict: "error", summary: e.message };
  }
  const loose = changed.filter((f) => !plan.packages.some((pkg) => inPaths(flow, pkg, f)));
  return {
    verdict: loose.length ? "fail" : "pass",
    summary: loose.length ? `${loose.length} 个改动不在任何工作包的范围里` : `${changed.length} 个改动都在工作包的范围里`,
    findings: loose.map((path) => ({ rule: "unclaimed", severity: "error", path, message: "改了，但没有哪个工作包声明这个路径" })),
    metrics: { changed: changed.length },
  };
}
