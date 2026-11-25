// spec-delta：规格增量对得上 specs/ 的基线。
export default function (input, flow) {
  const p = input.params;
  const delta = flow.read.yaml(p.delta);
  if (delta == null) return { verdict: "error", summary: `${p.delta} 不存在` };
  const specs = readSpecs(flow, p.specs);
  if (specs.size === 0) return { verdict: "error", summary: `${p.specs} 下读不到一条需求` };

  const findings = [];
  const bad = (message) => findings.push({ rule: "spec-delta", severity: "error", path: p.delta, message });
  const prefixes = new Map(); // 前缀 → 已有的最大编号
  for (const id of specs.keys()) {
    const [pre, n] = split(id);
    prefixes.set(pre, Math.max(prefixes.get(pre) ?? 0, n));
  }
  const seen = new Map();
  const mark = (id, where) => {
    if (seen.has(id)) bad(`${id} 同时出现在 ${seen.get(id)} 和 ${where}`);
    seen.set(id, where);
  };
  for (const r of delta.added || []) {
    mark(r.id, "added");
    const [pre, n] = split(r.id);
    if (specs.has(r.id)) bad(`added 的 ${r.id} 已经有了：改它用 modified`);
    else if (!prefixes.has(pre)) bad(`added 的 ${r.id}：前缀 ${pre} 不是哪份规格用的（有 ${[...prefixes.keys()].join("、")}）`);
    else if (n <= prefixes.get(pre)) bad(`added 的 ${r.id}：编号只增不复用，${pre} 已经到 R${prefixes.get(pre)}`);
  }
  for (const r of delta.modified || []) {
    mark(r.id, "modified");
    if (!specs.has(r.id)) bad(`modified 的 ${r.id} 不存在`);
    else if (squash(specs.get(r.id).text) === squash(r.text)) bad(`modified 的 ${r.id} 正文和现在一样`);
  }
  for (const id of delta.removed || []) {
    mark(id, "removed");
    if (!specs.has(id)) bad(`removed 的 ${id} 不存在`);
  }
  const counts = { added: (delta.added || []).length, modified: (delta.modified || []).length, removed: (delta.removed || []).length };
  return {
    verdict: findings.length ? "fail" : "pass",
    summary: findings.length ? `${findings.length} 处对不上基线` : `增量对得上基线：新增 ${counts.added}、修改 ${counts.modified}、删除 ${counts.removed}`,
    findings,
    metrics: { ...counts, baseline: specs.size },
  };
}

// specs/*.md 里的需求：`### <编号> <标题>` 到下一个标题之间是正文。
function readSpecs(flow, pattern) {
  const out = new Map();
  for (const f of flow.glob(pattern)) {
    const text = flow.read.text(f) || "";
    const re = /^###\s+([A-Z]+-R\d+)\s+(.*)$/gm;
    const heads = [...text.matchAll(re)];
    heads.forEach((m, i) => {
      const end = i + 1 < heads.length ? heads[i + 1].index : text.length;
      const body = text.slice(m.index + m[0].length, end).replace(/^#{1,2}\s.*$/gm, "").trim();
      out.set(m[1], { file: f, title: m[2].trim(), text: body });
    });
  }
  return out;
}

function split(id) {
  const m = /^([A-Z]+)-R(\d+)$/.exec(id || "");
  return m ? [m[1], Number(m[2])] : ["", 0];
}

function squash(s) {
  return String(s || "").replace(/\s+/g, "");
}
