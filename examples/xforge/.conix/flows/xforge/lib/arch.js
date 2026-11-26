// 接口基线与接口增量的读法与判法（docs/design/xforge.md §2.1、§2.2、§4）：命令 xarch 与门禁
// interface-delta / interface-compat / interfaces-archived 共用这一份，命令放行的增量门禁也放行。
//
// 基线 ${data_dir}/interfaces/<模块>.yaml；增量 ${output_dir}/interface-delta.yaml：
//   {modules: [{module, title, paths}], added: [{id, signature, description, stability}],
//    modified: [{id, signature?, description?, breaking}], promoted: [id], deprecated: [id], removed: [id]}

export const KINDS = ["fn", "type", "endpoint", "event", "config"];
export const STABILITY = ["experimental", "stable", "deprecated"];
const LISTS = ["added", "modified", "promoted", "deprecated", "removed"];
const ID_RE = /^(fn|type|endpoint|event|config):([a-z0-9][a-z0-9_-]*)\.(\S.*)$/;
const MODULE_RE = /^[a-z0-9][a-z0-9_-]*$/;

// "fn:textkit.slugify" → {kind, module, name}；不是编号就是 null。
// 模块名不含点，所以第一个点以后都是名字（endpoint 的名字里常有点和斜杠）。
export function parseId(id) {
  const m = ID_RE.exec(id || "");
  return m ? { kind: m[1], module: m[2], name: m[3] } : null;
}

export function makeId(kind, module, name) {
  return `${kind}:${module}.${name}`;
}

export function validModule(name) {
  return MODULE_RE.test(name || "");
}

// 读基线。
//   modules  模块名 → {module, title, paths, elements, path, raw}
//   byId     编号 → {id, signature, description, stability, module}
// 没有 stability 的元素按 stable 读：没有状态时只能把它当承诺，朝安全的方向错（xarch.md D3）。
export function readBaseline(flow, pattern) {
  const modules = new Map();
  const byId = new Map();
  for (const path of flow.glob(pattern)) {
    const raw = flow.read.yaml(path);
    if (!raw || typeof raw !== "object") continue;
    const name = raw.module || flow.path.basename(path, ".yaml");
    const m = { module: name, title: raw.title || name, paths: raw.paths || [], elements: [], path, raw };
    for (const e of raw.elements || []) {
      const el = { id: e.id, signature: e.signature || "", description: e.description || "", stability: e.stability || "stable", module: name };
      m.elements.push(el);
      byId.set(el.id, el);
    }
    modules.set(name, m);
  }
  return { modules, byId };
}

// 读增量，缺的列补成空。文件不存在就是空增量。
export function readDelta(flow, file) {
  const d = (file && flow.read.yaml(file)) || {};
  const out = { modules: d.modules || [] };
  for (const k of LISTS) out[k] = d[k] || [];
  return out;
}

// 写回的样子：空的列不写。
export function deltaToFile(delta) {
  const d = {};
  for (const k of ["modules", ...LISTS]) if (delta[k] && delta[k].length) d[k] = delta[k];
  return d;
}

export function isEmpty(delta) {
  return !["modules", ...LISTS].some((k) => (delta[k] || []).length);
}

export function counts(delta) {
  const c = {};
  for (const k of ["modules", ...LISTS]) c[k] = (delta[k] || []).length;
  return c;
}

// 这份增量动到的编号；新建的模块记成 module:<名>。
export function touched(delta) {
  const s = new Set();
  for (const m of delta.modules || []) s.add(`module:${m.module}`);
  for (const r of delta.added || []) s.add(r.id);
  for (const r of delta.modified || []) s.add(r.id);
  for (const k of ["promoted", "deprecated", "removed"]) for (const id of delta[k] || []) s.add(id);
  return s;
}

// 同一个包别的在途运行的增量动到了哪些编号：[{run, subject, ids: Set}]。
// 门禁在试跑时没有运行（flow.runs 是空的），自然没有冲突。
export function otherRuns(flow, input) {
  const self = input.run ? flow.runs.get(input.run) : null;
  const bundle = (self && self.bundle) || "xforge";
  const out = [];
  for (const r of flow.runs.list({ bundle, active: true })) {
    if (r.id === input.run || !r.output_dir) continue;
    let delta;
    try {
      delta = readDelta(flow, `${r.output_dir}/interface-delta.yaml`);
    } catch (e) {
      continue; // 读不到（在别处的工作区里）就当它没动：不为别人的运行让自己过不了
    }
    const ids = touched(delta);
    if (ids.size) out.push({ run: r.id, subject: r.subject, ids });
  }
  return out;
}

// 这次动的编号里，别的运行也在动的。
export function conflicts(delta, others) {
  const out = [];
  const mine = touched(delta);
  for (const o of others) {
    for (const id of o.ids) if (mine.has(id)) out.push(`${id} 在途运行 ${o.run}（${o.subject}）也在改：先和它对齐，谁后归档谁重新核对`);
  }
  return out;
}

// 增量对得上基线吗？回一串说明，空就是对得上。
//   nonEmpty  空增量也算不对（命令要并之前问；门禁不管：没改接口的变更本来就没有增量）
//   others    otherRuns() 的结果，给了就一并查冲突
export function checkDelta(base, delta, { nonEmpty = false, others = [] } = {}) {
  const out = [];
  if (nonEmpty && isEmpty(delta)) out.push("增量是空的");

  const fresh = new Set();
  for (const m of delta.modules) {
    if (!validModule(m.module)) out.push(`modules 的 ${m.module}：模块名是小写字母、数字、连字符、下划线`);
    else if (base.modules.has(m.module)) out.push(`modules 的 ${m.module} 基线里已经有了`);
    else if (fresh.has(m.module)) out.push(`modules 的 ${m.module} 出现了两次`);
    fresh.add(m.module);
    if (!m.title) out.push(`modules 的 ${m.module} 缺 title`);
    if (!Array.isArray(m.paths) || !m.paths.length || m.paths.some((p) => typeof p !== "string" || !p)) out.push(`modules 的 ${m.module} 要写 paths（这个模块的代码在哪）`);
  }
  const known = (mod) => base.modules.has(mod) || fresh.has(mod);

  // 编号在各列之间的关系：改 + 转正可以同时，其余都互斥。
  const ops = new Map();
  const mark = (id, op) => {
    const prev = ops.get(id) || [];
    for (const p of prev) {
      const ok = p !== op && [p, op].sort().join() === "modified,promoted";
      if (!ok) out.push(p === op ? `${id} 在 ${op} 里出现了两次` : `${id} 同时在 ${p} 和 ${op}`);
    }
    ops.set(id, [...prev, op]);
  };
  const shape = (id, where) => {
    if (parseId(id)) return true;
    out.push(`${where} 的 ${id}：编号是 <种类>:<模块>.<名字>，种类是 ${KINDS.join("、")}`);
    return false;
  };

  for (const r of delta.added) {
    mark(r.id, "added");
    if (!shape(r.id, "added")) continue;
    const { module } = parseId(r.id);
    if (base.byId.has(r.id)) out.push(`added 的 ${r.id} 基线里已经有了：改它用 modified`);
    if (!known(module)) out.push(`added 的 ${r.id}：没有模块 ${module}（基线里有 ${[...base.modules.keys()].join("、") || "（无）"}；新模块先 xarch module）`);
    if (!r.signature) out.push(`added 的 ${r.id} 缺 signature`);
    if (r.stability !== "experimental" && r.stability !== "stable") out.push(`added 的 ${r.id}：stability 是 experimental 或 stable（现在是 ${r.stability}）`);
  }
  for (const r of delta.modified) {
    mark(r.id, "modified");
    const e = base.byId.get(r.id);
    if (!e) {
      out.push(`modified 的 ${r.id} 基线里没有${delta.added.some((a) => a.id === r.id) ? "（这次新增的直接改 added 那一条）" : ""}`);
      continue;
    }
    if (r.signature === undefined && r.description === undefined) out.push(`modified 的 ${r.id} 没有改 signature 或 description`);
    else if ((r.signature === undefined || r.signature === e.signature) && (r.description === undefined || r.description === e.description)) out.push(`modified 的 ${r.id} 和基线一样`);
    if (e.stability === "deprecated") out.push(`modified 的 ${r.id} 已经弃用，只能删掉（xarch remove），不再演进`);
    else if (e.stability === "stable" && r.breaking !== true) out.push(`modified 的 ${r.id} 是 stable：要么不改它、改成兼容的做法（另加一个元素，旧的 xarch deprecate），要么声明破坏（xarch modify ${r.id} … --breaking，增量里 breaking: true）`);
    else if (e.stability === "experimental" && r.breaking === true) out.push(`modified 的 ${r.id} 是 experimental，不构成承诺，不用 breaking`);
  }
  for (const id of delta.promoted) {
    mark(id, "promoted");
    const e = base.byId.get(id);
    if (!e) out.push(`promoted 的 ${id} 基线里没有`);
    else if (e.stability !== "experimental") out.push(`promoted 的 ${id} 现在是 ${e.stability}：只有 experimental 能转正`);
  }
  for (const id of delta.deprecated) {
    mark(id, "deprecated");
    const e = base.byId.get(id);
    if (!e) out.push(`deprecated 的 ${id} 基线里没有`);
    else if (e.stability === "deprecated") out.push(`deprecated 的 ${id} 已经弃用`);
  }
  for (const id of delta.removed) {
    mark(id, "removed");
    const e = base.byId.get(id);
    if (!e) out.push(`removed 的 ${id} 基线里没有`);
    else if (e.stability !== "deprecated") out.push(`removed 的 ${id} 基线里是 ${e.stability}：删除只收已经 deprecated 的，先 xarch deprecate，归档后另开一次变更再 remove`);
  }
  out.push(...conflicts(delta, others));
  return out;
}

// 破坏性的改动：修改 stable 元素时声明了 breaking 的那些编号。
export function breakingOf(delta) {
  return (delta.modified || []).filter((r) => r.breaking === true).map((r) => r.id);
}

// 破坏性的改动要有交代：影响面含 interface，且决策台账里有一条指向它、结论 accepted。
//   impact     运行启动时的 inputs.impact
//   decisions  ledgers/decisions.yaml 读出来的（可以是 null）
export function checkCompat(delta, impact, decisions) {
  const out = [];
  const breaking = breakingOf(delta);
  if (!breaking.length) return out;
  const entries = (decisions && decisions.entries) || [];
  if (!(impact || []).includes("interface")) out.push(`有破坏性改动（${breaking.join("、")}），启动时的 inputs.impact 要含 interface（现在是 [${(impact || []).join(", ")}]）：这改不了，取消重开并声明影响面`);
  for (const id of breaking) {
    if (!entries.some((e) => (e.refs || []).includes(id) && e.conclusion === "accepted")) {
      out.push(`${id} 是破坏性改动：decisions 台账里没有指向它、结论是 accepted 的一条（xledger add decisions "<为什么破坏、怎么迁移>" --ref ${id}，再 xledger set decisions <编号> accepted）`);
    }
  }
  return out;
}

// 把增量并进基线，纯函数：回 {modules: 并完的全部模块, touched: 动过的模块名}。并之前要先 checkDelta 过。
export function mergeDelta(base, delta) {
  const mods = new Map();
  for (const [name, m] of base.modules) mods.set(name, { ...m, elements: m.elements.map((e) => ({ ...e })) });
  const changed = new Set();
  for (const m of delta.modules) {
    mods.set(m.module, { module: m.module, title: m.title, paths: m.paths, elements: [], raw: {}, path: null });
    changed.add(m.module);
  }
  const find = (id) => {
    const mod = mods.get(parseId(id).module);
    changed.add(mod.module);
    return [mod, mod.elements.find((e) => e.id === id)];
  };
  for (const r of delta.added) {
    const mod = mods.get(parseId(r.id).module);
    mod.elements.push({ id: r.id, signature: r.signature, description: r.description || "", stability: r.stability || "experimental", module: mod.module });
    changed.add(mod.module);
  }
  for (const r of delta.modified) {
    const e = find(r.id)[1];
    if (r.signature !== undefined) e.signature = r.signature;
    if (r.description !== undefined) e.description = r.description;
  }
  for (const id of delta.promoted) find(id)[1].stability = "stable";
  for (const id of delta.deprecated) find(id)[1].stability = "deprecated";
  for (const id of delta.removed) {
    const mod = find(id)[0];
    mod.elements = mod.elements.filter((e) => e.id !== id);
  }
  return { modules: mods, touched: changed };
}

// 要写进文件的样子：保留文件里别的键，元素只留四个字段。
export function moduleFile(m) {
  return {
    ...m.raw,
    module: m.module,
    title: m.title,
    paths: m.paths,
    elements: m.elements.map((e) => ({ id: e.id, signature: e.signature, description: e.description, stability: e.stability })),
  };
}

// 并完之后：增量里的每一条都落在基线里了吗？回一串说明，空就是并好了。
export function checkMerged(base, delta) {
  const out = [];
  for (const m of delta.modules) {
    const b = base.modules.get(m.module);
    if (!b) out.push(`没有并进 modules 的 ${m.module}`);
    else if (JSON.stringify(b.paths) !== JSON.stringify(m.paths)) out.push(`模块 ${m.module} 的 paths 和增量里的不一样`);
  }
  for (const r of delta.added) {
    const e = base.byId.get(r.id);
    if (!e) out.push(`没有并进 added 的 ${r.id}`);
    else {
      if (e.signature !== r.signature) out.push(`${r.id} 的 signature 和增量里的不一样`);
      if (e.stability !== (r.stability || "experimental")) out.push(`${r.id} 的 stability 是 ${e.stability}，增量里是 ${r.stability || "experimental"}`);
    }
  }
  for (const r of delta.modified) {
    const e = base.byId.get(r.id);
    if (!e) out.push(`modified 的 ${r.id} 不见了`);
    else {
      if (r.signature !== undefined && e.signature !== r.signature) out.push(`${r.id} 的 signature 还没换成增量里的`);
      if (r.description !== undefined && e.description !== r.description) out.push(`${r.id} 的 description 还没换成增量里的`);
    }
  }
  for (const [list, want] of [["promoted", "stable"], ["deprecated", "deprecated"]]) {
    for (const id of delta[list]) {
      const e = base.byId.get(id);
      if (!e) out.push(`${list} 的 ${id} 不见了`);
      else if (e.stability !== want) out.push(`${list} 的 ${id} 现在是 ${e.stability}，应是 ${want}`);
    }
  }
  for (const id of delta.removed) if (base.byId.has(id)) out.push(`removed 的 ${id} 还在`);
  return out;
}

// 参数拆分：`--stable`、`--breaking` 不带值（带了 true / false 也认），其余的 `--k v`。
// cli.js 的 parseFlags 一律吃下一个词，会把 --stable 后面的词吞掉，所以这里自己拆。
export function parseArgs(args, { bool = [], value = [] } = {}) {
  const pos = [], opt = {};
  for (let i = 0; i < args.length; i++) {
    const a = args[i];
    if (!a.startsWith("--")) {
      pos.push(a);
      continue;
    }
    const k = a.slice(2);
    if (bool.includes(k)) {
      const next = args[i + 1];
      if (next === "true" || next === "false") {
        opt[k] = next === "true";
        i++;
      } else opt[k] = true;
    } else if (value.includes(k)) {
      if (i + 1 >= args.length) throw new Error(`--${k} 后面要跟值`);
      opt[k] = args[++i];
    } else {
      const all = [...bool, ...value].map((x) => `--${x}`);
      throw new Error(`不认识 --${k}；${all.length ? `这个子命令认 ${all.join("、")}` : "这个子命令不带选项"}`);
    }
  }
  return { pos, opt };
}
