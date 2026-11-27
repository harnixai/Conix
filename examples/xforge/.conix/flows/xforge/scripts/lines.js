// lines：solid、major 的第一站，进站就判，不调模型（lib/lines.js，自检用同一份）。
import { REVIEWED, modelOf, reviewLineProblem, stagesOn } from "../lib/lines.js";

const FILE = ".conix/governance/xforge.yaml";

export default function (input, flow) {
  const path = String(input.flow || "").split("/").pop();
  const s = flow.settings || {};
  if (!REVIEWED.includes(path)) return { verdict: "pass", summary: `${path} 没有检视线`, findings: [] };
  const on = stagesOn(s, path);
  const line = (l) => modelOf(s, l) || "实例的缺省模型";
  const said = `开发线 ${line("build")}，测试线 ${line("tests")}${on.length ? `，检视线 ${line("review")}（检视点 ${on.join("、")}）` : "，检视点都关着"}`;
  const problem = reviewLineProblem(s, path);
  if (!problem) return { verdict: "pass", summary: said, findings: [] };
  return {
    verdict: "error",
    summary: `${problem}。改 ${FILE} 的 lines.review.model（或关掉 review.${path} 的检视点），再重新起一次运行：设置钉在运行里，这次改了也不算`,
    findings: [{ rule: "lines", severity: "error", path: FILE, message: problem }],
  };
}
