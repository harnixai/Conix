// verification：增量里新增、修改的每条需求，验证记录里有结论、测试里有一条带 `# <编号>` 的用例。
const RESULTS = ["通过", "未通过", "未验证"];

export default function (input, flow) {
  const p = input.params;
  const delta = flow.read.yaml(p.delta);
  const record = flow.read.text(p.verification);
  if (delta == null || record == null) {
    return { verdict: "error", summary: `${delta == null ? p.delta : p.verification} 不存在` };
  }
  const ids = [...(delta.added || []), ...(delta.modified || [])].map((r) => r.id);
  const tests = flow.glob(p.tests).map((f) => [f, flow.read.text(f) || ""]);
  const findings = [];
  const counts = { 通过: 0, 未通过: 0, 未验证: 0 };
  for (const id of ids) {
    const line = record.split("\n").find((l) => l.includes(id) && RESULTS.some((r) => l.includes(r)));
    if (!line) {
      findings.push({ rule: "verification", severity: "error", path: p.verification, message: `${id} 没有一行写明验证结论（${RESULTS.join(" / ")}）` });
    } else {
      // "未通过" 里有 "通过"：先认长的。
      const r = line.includes("未通过") ? "未通过" : line.includes("未验证") ? "未验证" : "通过";
      counts[r] += 1;
      if (r === "未通过") findings.push({ rule: "verification", severity: "error", path: p.verification, message: `${id} 验证未通过` });
      if (r === "未验证") findings.push({ rule: "verification", severity: "warn", path: p.verification, message: `${id} 没有验证：交给验收的人判断` });
    }
    const tagged = new RegExp(`#\\s*${id}\\b`);
    if (!tests.some(([, t]) => tagged.test(t))) {
      findings.push({ rule: "tested", severity: "error", path: p.tests, message: `没有一条测试带注释 # ${id}：每条新增、修改的需求都要有测试` });
    }
  }
  const errors = findings.filter((f) => f.severity === "error").length;
  return {
    verdict: errors ? "fail" : "pass",
    summary: errors ? `${errors} 处验证不足` : `${ids.length} 条需求：通过 ${counts["通过"]}，未验证 ${counts["未验证"]}`,
    findings,
    metrics: { requirements: ids.length, passed: counts["通过"], unverified: counts["未验证"], test_files: tests.length },
  };
}
