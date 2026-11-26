// verification：增量里新增、修改的每条需求与每个接口元素，验证记录的「覆盖」里一行结论，测试里一条带 `# <编号>` 的用例；
// 「连贯性」写了。关着的基线不欠它的增量（lib/governance.js）。
import { governance } from "../lib/governance.js";
import { section, lines, mentions } from "../lib/markdown.js";

// "未通过" 里有 "通过"：先认长的。
const RESULTS = ["未通过", "未验证", "通过"];

export default function (input, flow) {
  const p = input.params;
  const g = governance(flow);
  const record = flow.read.text(p.verification);
  if (record == null) return { verdict: "error", summary: `${p.verification} 不存在` };

  const ids = [];
  if (g.spec) {
    const d = flow.read.yaml(p.spec_delta);
    if (d == null) return { verdict: "error", summary: `${p.spec_delta} 不存在` };
    for (const r of [...(d.added || []), ...(d.modified || [])]) ids.push(r.id);
  }
  if (g.interface) {
    // 没动接口的变更没有接口增量：不欠。
    const d = flow.read.yaml(p.interface_delta) || {};
    for (const e of [...(d.added || []), ...(d.modified || [])]) ids.push(e.id);
  }

  const findings = [];
  const bad = (rule, message, severity = "error") => findings.push({ rule, severity, path: p.verification, message });
  const coverage = section(record, "覆盖");
  if (coverage == null) bad("coverage", "没有「## 覆盖」一节：每条需求、每个元素一行，写结论与测试");
  const coherence = section(record, "连贯性");
  if (!coherence) bad("coherence", coherence == null ? "没有「## 连贯性」一节" : "「## 连贯性」是空的：写提案、规格、设计、测试、实现在哪里说得不一样，或者写明一致");

  const rows = lines(coverage);
  const tests = flow.glob(p.tests).map((f) => flow.read.text(f) || "");
  const counts = { 通过: 0, 未通过: 0, 未验证: 0 };
  for (const id of ids) {
    const row = rows.find((l) => mentions(l, id) && RESULTS.some((r) => l.includes(r)));
    if (coverage != null && !row) {
      bad("coverage", `「覆盖」里没有一行写 ${id} 的结论（${RESULTS.join(" / ")}）`);
    } else if (row) {
      const r = RESULTS.find((x) => row.includes(x));
      counts[r] += 1;
      if (r === "未通过") bad("coverage", `${id} 验证未通过`);
      if (r === "未验证") bad("coverage", `${id} 没有验证：交给验收的人判断`, "warn");
    }
    if (!tests.some((t) => lines(t).some((l) => /#/.test(l) && mentions(l.slice(l.indexOf("#") + 1), id)))) {
      findings.push({ rule: "tested", severity: "error", path: p.tests, message: `没有一条测试带注释 # ${id}：每条新增、修改的需求与元素都要有测试` });
    }
  }
  const errors = findings.filter((f) => f.severity === "error").length;
  return {
    verdict: errors ? "fail" : "pass",
    summary: errors ? `${errors} 处验证不足` : `${ids.length} 项：通过 ${counts["通过"]}，未验证 ${counts["未验证"]}`,
    findings,
    metrics: { items: ids.length, passed: counts["通过"], unverified: counts["未验证"], test_files: tests.length },
  };
}
