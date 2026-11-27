// blackbox：两路合起来那一站（xforge/solid、xforge/major 的 join），命令门禁的 plan（docs/design/flows.md §2.2）。
// 从项目治理文件的 verify 菜单里取名叫 blackbox 的一项（人声明的：测试子运行把黑盒测试写在哪、怎么跑），展开成 argv，
// 交给流程库直接起进程，不经 shell。菜单是运行启动时钉住的那份（lib/governance.js 的 verifyMenu）。
// 没有这一项是 error 不是 pass：双路开发就是为了用独立写的测试跑实现，没有可跑的就没有可判的，要人补菜单。
import { verifyMenu } from "../lib/governance.js";
import { BLACKBOX as NAME, blackboxProblem, expand, menuProblems } from "../lib/verify.js";

const MENU = ".conix/governance/xforge.yaml";

export default function (input, flow) {
  const menu = verifyMenu(flow);
  const need = (summary) => ({
    verdict: "error",
    summary,
    findings: [{ rule: "blackbox", path: MENU, message: summary }],
  });
  if (blackboxProblem(menu) === "missing") {
    return need(`${MENU} 的 verify 菜单要有 ${NAME} 一项：双路开发合起来时用它跑测试子运行写的黑盒测试（如 ${NAME}: {argv: [python3, -m, unittest, discover, -s, tests_blackbox, -q]}）`);
  }
  const problems = menuProblems(menu);
  if (problems.length) return need(`verify 菜单有问题（要人改 ${MENU}）：${problems.join("；")}`);
  if (blackboxProblem(menu) === "params") {
    return need(`verify.${NAME} 不能带参数：合起来时跑的是整套黑盒测试，没有谁来填参数`);
  }
  return { summary: `黑盒测试（verify.${NAME}）`, runs: [{ name: NAME, ...expand(menu, { name: NAME, params: {} }) }] };
}
