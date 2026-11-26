// package-check：一个工作包的任务级出口（task_exit，params.item 是 ${item}：包的编号，或带 id 的对象）。
// 这一包 status 是 done（blocked 就不过，说卡在哪）；每条判据有证据；files 都在包的路径里；
// 基线以来（含没提交的）的改动里，落在这个包路径下的，都登记在 files 里——登记的与账本上的改动要对得上。
import { changedFiles, inPaths, loadPlan, norm } from "../lib/packages.js";

export default function (input, flow) {
  const p = input.params;
  const id = p.item && typeof p.item === "object" ? p.item.id : p.item;
  const fail = (messages, extra = {}) => ({
    verdict: "fail",
    summary: messages.length === 1 ? messages[0] : `工作包 ${id}：${messages.length} 处不过`,
    findings: messages.map((message) => ({ rule: "package-check", severity: "error", path: p.plan, message })),
    ...extra,
  });
  if (!id) return { verdict: "error", summary: "门禁要 params.item（任务级出口里是 ${item}）" };
  const plan = loadPlan(flow, p.plan);
  const pkg = plan && plan.packages.find((x) => x.id === id);
  if (!pkg) return fail([`没有工作包 ${id}：xpkg plan 划`]);
  if (pkg.status === "blocked") return fail([`工作包 ${id} 卡住了：${pkg.summary || "（没写原因）"}。需要人看：不是改一改能过的`]);
  if (pkg.status !== "done") return fail([`工作包 ${id} 还是 ${pkg.status}：做完用 xpkg done ${id} "<做了什么>" --files … --evidence "C1=…;…" 登记`]);

  const messages = [];
  const evidence = pkg.evidence || {};
  for (const c of pkg.criteria || []) {
    if (!String(evidence[c.id] || "").trim()) messages.push(`判据 ${c.id}（${c.text}）没有证据`);
  }
  const files = (pkg.files || []).map(norm);
  const outside = files.filter((f) => !inPaths(flow, pkg, f));
  if (outside.length) messages.push(`登记的文件不在包的路径（${pkg.paths.join(", ")}）里：${outside.join(", ")}`);
  let changed;
  try {
    changed = changedFiles(flow);
  } catch (e) {
    return { verdict: "error", summary: e.message };
  }
  const mine = changed.filter((f) => inPaths(flow, pkg, f));
  const unregistered = mine.filter((f) => !files.includes(f));
  if (unregistered.length) messages.push(`改了但没登记在 files 里：${unregistered.join(", ")}（xpkg done 补上）`);
  const stale = files.filter((f) => !mine.includes(f) && !outside.includes(f));
  if (stale.length) messages.push(`登记了但工作区里没有这个改动：${stale.join(", ")}`);
  if (messages.length) return fail(messages, { metrics: { changed: mine.length, files: files.length } });
  return {
    verdict: "pass",
    summary: `工作包 ${id}：${(pkg.criteria || []).length} 条判据有证据，${files.length} 个文件与改动对得上`,
    findings: [],
    metrics: { changed: mine.length, files: files.length },
  };
}
