// spec-delta-shape：规格增量的格式。为什么不用 `check: schema`：关着规格基线的项目这道门禁要回"不欠"，
// 而 schema 检查不认治理开关；形状本身在 lib/delta.js 的 DELTA_SCHEMA。
import { governance, notOwed } from "../lib/governance.js";
import { DELTA_SCHEMA } from "../lib/delta.js";

export default function (input, flow) {
  if (!governance(flow).spec) return notOwed("spec");
  const file = input.params.file;
  const delta = flow.read.yaml(file);
  if (delta == null) return { verdict: "fail", summary: `${file} 不存在：用 xspec add / modify / remove 写增量`, findings: [{ rule: "spec-delta-shape", severity: "error", path: file, message: "没有规格增量" }] };
  const r = flow.schema.validate(delta, DELTA_SCHEMA);
  const errors = (r.errors || []).map((e) => (typeof e === "string" ? e : JSON.stringify(e)));
  return {
    verdict: r.ok ? "pass" : "fail",
    summary: r.ok ? "规格增量的形状对" : `${errors.length} 处形状不对`,
    findings: errors.map((message) => ({ rule: "spec-delta-shape", severity: "error", path: file, message })),
  };
}
