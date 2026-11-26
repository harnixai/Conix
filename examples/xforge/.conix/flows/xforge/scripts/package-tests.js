// package-tests：一个工作包的任务级出口（task_exit），命令门禁的 plan（docs/design/flows.md §2.2）。
// 读这一包登记的 verify（xpkg plan --verify：菜单里的名字与参数），按项目治理文件里人声明的 verify 菜单再核对一次
// （两个文件都可能被改过）、展开成 argv，交给流程库直接起进程，不经 shell。没登记验证就不欠。
import { verifyMenu } from "../lib/governance.js";
import { loadPlan } from "../lib/packages.js";
import { expand, label, menuProblems } from "../lib/verify.js";

const MENU = ".conix/governance/xforge.yaml";

export default function (input, flow) {
  const p = input.params;
  const id = p.item && typeof p.item === "object" ? p.item.id : p.item;
  if (!id) return { verdict: "error", summary: "门禁要 params.item（任务级出口里是 ${item}）" };
  const plan = loadPlan(flow, p.plan);
  const pkg = plan && plan.packages.find((x) => x.id === id);
  if (!pkg) {
    return { verdict: "fail", summary: `没有工作包 ${id}：xpkg plan 划`, findings: [{ rule: "package-tests", path: p.plan, message: `没有工作包 ${id}` }] };
  }
  // 以前的包把验证写成一句说明（字符串）：那是给人看的，不是能跑的东西。
  const entries = Array.isArray(pkg.verify) ? pkg.verify : [];
  if (!entries.length) return { verdict: "pass", summary: "没有要跑的", findings: [] };

  // 登记了验证却对不上菜单：划包时核对过，现在不合是有人改了治理文件或工作包。模型改代码修不了，要人看。
  const menu = verifyMenu(flow);
  const problems = menu ? menuProblems(menu) : [`工作包 ${id} 登记了验证 ${entries.map(label).join("、")}，项目的 ${MENU} 却没有 verify 菜单`];
  const runs = [];
  if (!problems.length) {
    for (const e of entries) {
      try {
        runs.push({ name: label(e), ...expand(menu, e) });
      } catch (err) {
        problems.push(err.message);
      }
    }
  }
  if (problems.length) {
    return {
      verdict: "error",
      summary: `工作包 ${id} 的验证跑不了（需要人看）：${problems[0]}`,
      findings: problems.map((message) => ({ rule: "package-tests", path: MENU, message })),
    };
  }
  return { summary: `工作包 ${id}`, runs };
}
