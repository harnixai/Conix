// spec-unchanged：两路的子运行（xforge/line-build、xforge/line-tests）没改规格侧。规格侧是父运行目录分叉时的一份
// （${parent.output_dir}），父运行分叉前封存了它每个文件的摘要（xspec seal），作为子运行的输入 spec_side 传进来。
// 不用 flow.git.changed 判：运行目录在制品目录里，git 不管它（git.changed 也把制品目录排除在外），子运行改了它 git 看不见。
// 摘要也不放在规格侧旁边的文件里：那份文件子运行改得到，运行的输入改不到。
import { sideProblems } from "../lib/side.js";

const NOT_A_CHILD = "这条流程只做 xforge/solid、xforge/major 的子运行（规格侧来自父运行的分叉）：不要单独起它，起 xforge/solid 或 xforge/major";

export default function (input, flow) {
  const p = input.params;
  const unresolved = (v) => v == null || (typeof v === "string" && v.includes("${"));
  if (unresolved(p.dir)) return { verdict: "error", summary: `没有父运行：${NOT_A_CHILD}` };
  const sealed = p.sealed;
  if (unresolved(sealed) || typeof sealed !== "object" || Array.isArray(sealed) || !Object.keys(sealed).length) {
    return { verdict: "error", summary: `输入 spec_side 不是父运行封存的摘要：${NOT_A_CHILD}` };
  }
  // ${parent.output_dir} 是绝对路径（子运行 worktree 里的那一份）；flow.glob 回相对工作目录的路径。
  let dir = String(p.dir);
  if (dir.startsWith("/")) {
    if (!input.workspace) return { verdict: "error", summary: "判不了：门禁不知道工作目录在哪" };
    dir = flow.path.relative(input.workspace, dir);
  }
  const findings = sideProblems(flow, dir, sealed).map(({ path, message }) => ({ rule: "spec-unchanged", severity: "error", path, message }));
  const n = Object.keys(sealed).length;
  return {
    verdict: findings.length ? "fail" : "pass",
    summary: findings.length ? `规格侧 ${findings.length} 个文件与分叉时不一样` : `规格侧 ${n} 个文件与分叉时一样`,
    findings,
    metrics: { sealed: n, changed: findings.length },
  };
}
