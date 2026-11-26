// 规格增量（spec-delta.yaml：added、modified、removed）的判法。门禁 spec-delta 与命令
// xspec check / archive 用同一个函数：命令放行的增量，门禁也放行。
import { splitId, squash, scenarioProblems, SCENARIO_SHAPE } from "./specs.js";

export function counts(delta) {
  return { added: (delta.added || []).length, modified: (delta.modified || []).length, removed: (delta.removed || []).length };
}

// 增量对得上基线吗？回一串说明，空就是对得上。
//   nonEmpty  空增量也算不对（命令要并之前问；门禁不管，空不空由 spec-delta-shape 管）
//   others    别的在途运行的增量（inflight 的结果）：动了同一个编号就是冲突
export function checkDelta(specs, delta, { nonEmpty = false, others = [] } = {}) {
  const out = [];
  const seen = new Map();
  const mark = (id, where) => {
    if (seen.has(id)) out.push(`${id} 同时出现在 ${seen.get(id)} 和 ${where}`);
    seen.set(id, where);
  };
  const c = counts(delta);
  if (nonEmpty && !c.added && !c.modified && !c.removed) out.push("增量是空的");
  for (const r of delta.added || []) {
    mark(r.id, "added");
    const [pre, n] = splitId(r.id);
    if (specs.byId.has(r.id)) out.push(`added 的 ${r.id} 已经有了：改它用 modified`);
    else if (!specs.prefixes.has(pre)) out.push(`added 的 ${r.id}：前缀 ${pre} 不是哪份规格用的（有 ${[...specs.prefixes.keys()].join("、")}）`);
    else if (n <= specs.prefixes.get(pre)) out.push(`added 的 ${r.id}：编号只增不复用，${pre} 已经到 R${specs.prefixes.get(pre)}`);
  }
  for (const r of delta.modified || []) {
    mark(r.id, "modified");
    if (!specs.byId.has(r.id)) out.push(`modified 的 ${r.id} 不存在`);
    else if (squash(specs.byId.get(r.id).text) === squash(r.text)) out.push(`modified 的 ${r.id} 正文和现在一样`);
  }
  for (const id of delta.removed || []) {
    mark(id, "removed");
    if (!specs.byId.has(id)) out.push(`removed 的 ${id} 不存在`);
  }
  // 新增与修改的正文要写成 WHEN / THEN：门禁与命令报同一句，各条各报。
  for (const [where, list] of [["added", delta.added || []], ["modified", delta.modified || []]]) {
    for (const r of list) {
      const bad = scenarioProblems(r.text);
      if (bad.length) out.push(`${where} 的 ${r.id} 的正文要写成 WHEN / THEN 条目：${bad.join("；")}。形如\n${SCENARIO_SHAPE}`);
    }
  }
  out.push(...conflicts(delta, others));
  return out;
}

// 别的在途运行（同一个包、还没结束）的规格增量：每个 { run, subject, added, modified, removed }（各是编号数组）。
// 为什么每次现读它们的 spec-delta.yaml，不记在哪里：增量是各运行目录里命令写的，只有读它才是现在的样子。
export function inflight(flow, run) {
  const me = run ? flow.runs.get(run) : null;
  const bundle = (me && me.bundle) || "xforge";
  const out = [];
  for (const r of flow.runs.list({ bundle, active: true })) {
    if (r.id === run || !r.output_dir) continue;
    let d = null;
    try { d = flow.read.yaml(`${r.output_dir}/spec-delta.yaml`); } catch (e) { flow.log(`读 ${r.id} 的增量失败：${e}`); }
    if (!d) continue;
    const item = {
      run: r.id,
      subject: r.subject || "",
      added: (d.added || []).map((x) => x.id),
      modified: (d.modified || []).map((x) => x.id),
      removed: d.removed || [],
    };
    if (item.added.length || item.modified.length || item.removed.length) out.push(item);
  }
  return out;
}

// 我这边动的编号，别的在途运行也动了：回一串说明。
export function conflicts(delta, others) {
  const out = [];
  const mine = [];
  for (const r of delta.added || []) mine.push([r.id, "新增"]);
  for (const r of delta.modified || []) mine.push([r.id, "修改"]);
  for (const id of delta.removed || []) mine.push([id, "删除"]);
  for (const [id, how] of mine) {
    for (const o of others) {
      for (const [k, theirs] of [["added", "新增"], ["modified", "修改"], ["removed", "删除"]]) {
        if (o[k].includes(id)) {
          out.push(`冲突：${id} 我这边${how}，在途运行 ${o.run}${o.subject ? `（${o.subject}）` : ""}也${theirs}了它：谁先归档谁算，另一边要退到提案重做，或用 xspec revert ${id} 放弃`);
        }
      }
    }
  }
  return out;
}

// 别的在途运行占了的某个前缀的最大编号：新增排号要躲开。
export function highestTaken(others, prefix) {
  let max = 0;
  for (const o of others) for (const id of o.added) {
    const [pre, n] = splitId(id);
    if (pre === prefix) max = Math.max(max, n);
  }
  return max;
}

// 规格增量的形状（门禁 spec-delta-shape 用）。
const ID = { type: "string", pattern: "^[A-Z]+-R[0-9]+$" };
export const DELTA_SCHEMA = {
  type: "object",
  additionalProperties: false,
  properties: {
    added: { type: "array", items: { type: "object", required: ["id", "title", "text"], properties: { id: ID, title: { type: "string", minLength: 1 }, text: { type: "string", minLength: 10 } } } },
    modified: { type: "array", items: { type: "object", required: ["id", "text"], properties: { id: ID, title: { type: "string" }, text: { type: "string", minLength: 10 } } } },
    removed: { type: "array", items: ID },
  },
  anyOf: [
    { required: ["added"], properties: { added: { minItems: 1 } } },
    { required: ["modified"], properties: { modified: { minItems: 1 } } },
    { required: ["removed"], properties: { removed: { minItems: 1 } } },
  ],
};

// 并完之后：增量里的每一条都落在规格里了吗？回一串说明，空就是并好了。
export function checkMerged(specs, delta) {
  const out = [];
  for (const r of delta.added || []) {
    const s = specs.byId.get(r.id);
    if (!s) out.push(`没有并进 added 的 ${r.id}`);
    else if (squash(s.text) !== squash(r.text)) out.push(`${r.id} 的正文和增量里的不一样`);
  }
  for (const r of delta.modified || []) {
    const s = specs.byId.get(r.id);
    if (!s) out.push(`modified 的 ${r.id} 不见了`);
    else if (squash(s.text) !== squash(r.text)) out.push(`${r.id} 的正文还没换成增量里的`);
  }
  for (const id of delta.removed || []) {
    if (specs.byId.has(id)) out.push(`removed 的 ${id} 还在`);
  }
  return out;
}
