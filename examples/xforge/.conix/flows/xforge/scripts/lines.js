// lines：solid、major 的第一站，进站就判，不调模型（lib/lines.js，自检用同一份）。
// 检视线与开发线同一个模型只告警、不挡：运行照跑，告警记在这一站、进运行的报告，实例也给人一条提示。
import { REVIEWED, modelOf, reviewLineWarning, stagesOn } from "../lib/lines.js";

const FILE = ".conix/governance/xforge.yaml";

export default function (input, flow) {
  const path = String(input.flow || "").split("/").pop();
  const s = flow.settings || {};
  if (!REVIEWED.includes(path)) return { verdict: "pass", summary: `${path} 没有检视线`, findings: [] };
  const on = stagesOn(s, path);
  const line = (l) => modelOf(s, l) || "实例的缺省模型";
  const said = `开发线 ${line("build")}，测试线 ${line("tests")}${on.length ? `，检视线 ${line("review")}（检视点 ${on.join("、")}）` : "，检视点都关着"}`;
  const warning = reviewLineWarning(s, path);
  if (!warning) return { verdict: "pass", summary: said, findings: [] };
  return {
    verdict: "pass",
    summary: `${said}；告警：${warning}`,
    findings: [{ rule: "lines", severity: "warn", path: FILE, message: `${warning}。要另一双眼睛就改 ${FILE} 的 lines.review.model，下次起的运行才算` }],
  };
}
