// 规格增量（spec-delta.yaml：added、modified、removed）的判法。门禁 spec-delta 与命令
// xspec check / archive 用同一个函数：命令放行的增量，门禁也放行。
import { splitId, squash } from "./specs.js";

export function counts(delta) {
  return { added: (delta.added || []).length, modified: (delta.modified || []).length, removed: (delta.removed || []).length };
}

// 增量对得上基线吗？回一串说明，空就是对得上。
//   nonEmpty  空增量也算不对（命令要并之前问；门禁不管，空不空由 spec-delta-shape 管）
export function checkDelta(specs, delta, { nonEmpty = false } = {}) {
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
  return out;
}

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
