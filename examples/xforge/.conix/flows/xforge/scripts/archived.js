// archived：规格增量都并进了 specs/：归档站用 xspec archive 并，这里逐条核对。
export default function (input, flow) {
  const p = input.params;
  const delta = flow.read.yaml(p.delta);
  if (delta == null) return { verdict: "error", summary: `${p.delta} 不存在` };
  const specs = readSpecs(flow, p.specs);
  const findings = [];
  const bad = (message) => findings.push({ rule: "archived", severity: "error", path: p.specs, message });
  for (const r of delta.added || []) {
    const s = specs.get(r.id);
    if (!s) bad(`没有并进 added 的 ${r.id}`);
    else if (squash(s.text) !== squash(r.text)) bad(`${r.id} 的正文和增量里的不一样`);
  }
  for (const r of delta.modified || []) {
    const s = specs.get(r.id);
    if (!s) bad(`modified 的 ${r.id} 不见了`);
    else if (squash(s.text) !== squash(r.text)) bad(`${r.id} 的正文还没换成增量里的`);
  }
  for (const id of delta.removed || []) {
    if (specs.has(id)) bad(`removed 的 ${id} 还在`);
  }
  return {
    verdict: findings.length ? "fail" : "pass",
    summary: findings.length ? `${findings.length} 处没并好` : `增量都并进了规格，现在共 ${specs.size} 条需求`,
    findings,
    metrics: { requirements: specs.size },
  };
}

function readSpecs(flow, pattern) {
  const out = new Map();
  for (const f of flow.glob(pattern)) {
    const text = flow.read.text(f) || "";
    const heads = [...text.matchAll(/^###\s+([A-Z]+-R\d+)\s+(.*)$/gm)];
    heads.forEach((m, i) => {
      const end = i + 1 < heads.length ? heads[i + 1].index : text.length;
      const body = text.slice(m.index + m[0].length, end).replace(/^#{1,2}\s.*$/gm, "").trim();
      out.set(m[1], { file: f, title: m[2].trim(), text: body });
    });
  }
  return out;
}

function squash(s) {
  return String(s || "").replace(/\s+/g, "");
}
