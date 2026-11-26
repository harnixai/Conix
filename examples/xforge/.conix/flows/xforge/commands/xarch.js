// xarch：接口基线与这次变更的接口增量。基线在 ${data_dir}/interfaces/<模块>.yaml，每个模块一份，元素的编号是
// <种类>:<模块>.<名字>（种类 fn | type | endpoint | event | config），生命周期 experimental → stable → deprecated；
// 增量在这次运行的 interface-delta.yaml。模型不手写这两样，用这个命令。只有项目开了接口基线才能写增量
// （.conix/governance/xforge.yaml 的 governance.interface）。
//
//   xarch modules                                    列出模块（基线，加上这次新建的）
//   xarch list [模块]                                列出元素，带状态与这次增量里的动作
//   xarch get <编号>                                 一个元素：基线的样子与这次的改动
//   xarch delta                                      看增量
//   xarch check                                      增量对得上基线没有，与别的在途运行有没有冲突（与门禁 interface-delta 同一套）
//   xarch pending                                    别的在途运行的增量动到了哪些编号
//   xarch module <模块> "<标题>" --paths a,b          增量里新建一个模块（paths 是它的代码在哪）
//   xarch add <模块> <种类> <名字> "<签名>" "<说明>" [--stable]
//                                                    增量里加一个元素，缺省 experimental；--stable 是补登记已经在用的接口
//   xarch modify <编号> [--signature S] [--description D] [--breaking]
//                                                    改一个已有的元素；stable 的必须带 --breaking（声明破坏）
//   xarch promote <编号>                             experimental → stable
//   xarch deprecate <编号>                           标成弃用
//   xarch remove <编号>                              删一个基线里已经 deprecated 的元素
//   xarch revert <编号|模块>                         把一个编号（或这次新建的模块）从增量里拿掉
//   xarch archive                                    把增量并进基线（归档站用）
import { need } from "../lib/cli.js";
import { governance } from "../lib/governance.js";
import {
  KINDS, parseId, makeId, validModule, readBaseline, readDelta, deltaToFile, isEmpty, counts, touched, otherRuns,
  checkDelta, breakingOf, mergeDelta, moduleFile, parseArgs,
} from "../lib/arch.js";

// archive 不在其中：它对没开接口基线的项目是空操作，不是错（见 case "archive"）。
const WRITES = ["module", "add", "modify", "promote", "deprecate", "remove", "revert"];

export default function (input, flow) {
  const [sub, ...rest] = input.args;
  const ifaceDir = `${input.data_dir}/interfaces`;
  const deltaFile = input.output_dir ? `${input.output_dir}/interface-delta.yaml` : null;
  const base = readBaseline(flow, `${ifaceDir}/*.yaml`);
  const delta = readDelta(flow, deltaFile);
  const inRun = () => need(deltaFile, "要在运行里：xarch 的增量是这次运行的（运行之外只能看基线：modules、list、get、pending）");
  const others = () => otherRuns(flow, input);
  const save = () => flow.write.yaml(inRun(), deltaToFile(delta));

  if (WRITES.includes(sub)) {
    need(governance(flow).interface, "这个项目没开接口基线（.conix/governance/xforge.yaml）");
    inRun();
  }
  // 这个编号这次已经动过哪几列。
  const inDelta = (id) => ["modified", "promoted", "deprecated", "removed"].filter((k) => (k === "modified" ? delta.modified.some((r) => r.id === id) : delta[k].includes(id)));
  const baseline = (id) => {
    const e = base.byId.get(id);
    if (e) return e;
    need(parseId(id), `编号是 <种类>:<模块>.<名字>，种类是 ${KINDS.join("、")}：${id}`);
    const added = delta.added.find((r) => r.id === id);
    throw new Error(
      added
        ? `${id} 是这次新增的，基线里还没有：改它用 xarch revert ${id} 拿掉再 add，转正就 add 时带 --stable`
        : `基线里没有 ${id}（xarch list 看有什么；新增用 xarch add）`,
    );
  };
  const trail = (id) => {
    const o = others().filter((x) => x.ids.has(id));
    return o.length ? `；注意在途运行 ${o.map((x) => x.run).join("、")} 也动了它` : "";
  };

  switch (sub) {
    case "modules": {
      const lines = [...base.modules.values()].map((m) => {
        const n = { experimental: 0, stable: 0, deprecated: 0 };
        for (const e of m.elements) n[e.stability] = (n[e.stability] || 0) + 1;
        return `${m.module}（${m.paths.join("、")}）：${m.elements.length} 个元素（stable ${n.stable}、experimental ${n.experimental}、deprecated ${n.deprecated}）— ${m.title}`;
      });
      for (const m of delta.modules) lines.push(`${m.module}（${m.paths.join("、")}）：这次新建 — ${m.title}`);
      return lines.join("\n") || "还没有模块：xarch module <模块> \"<标题>\" --paths a,b";
    }
    case "list": {
      const mod = rest[0];
      if (mod) need(base.modules.has(mod) || delta.modules.some((m) => m.module === mod), `没有模块 ${mod}（有 ${[...base.modules.keys(), ...delta.modules.map((m) => m.module)].join("、") || "（无）"}）`);
      const lines = [];
      const act = (id) => {
        const a = inDelta(id).map((k) => (k === "modified" && delta.modified.find((r) => r.id === id).breaking ? "modified breaking" : k));
        return a.length ? `  ← 这次：${a.join("、")}` : "";
      };
      for (const m of base.modules.values()) {
        if (mod && m.module !== mod) continue;
        for (const e of m.elements) lines.push(`${e.id} [${e.stability}] ${e.signature}${act(e.id)}`);
      }
      for (const r of delta.added) if (!mod || parseId(r.id)?.module === mod) lines.push(`${r.id} [${r.stability}] ${r.signature}  ← 这次：added`);
      return lines.join("\n") || "没有元素";
    }
    case "get": {
      const id = need(rest[0], "xarch get <编号>");
      const e = base.byId.get(id);
      const added = delta.added.find((r) => r.id === id);
      need(e || added, `没有 ${id}（xarch list 看有什么）`);
      const cur = e || added;
      const out = [`${id} [${cur.stability}]`, `签名：${cur.signature}`, `说明：${cur.description || ""}`];
      if (added) out.push("这次：新增（基线里还没有）");
      for (const r of delta.modified.filter((r) => r.id === id)) out.push(`这次：修改${r.signature !== undefined ? `，签名 → ${r.signature}` : ""}${r.description !== undefined ? `，说明 → ${r.description}` : ""}${r.breaking ? "（breaking）" : ""}`);
      for (const k of ["promoted", "deprecated", "removed"]) if (delta[k].includes(id)) out.push(`这次：${k}`);
      return out.join("\n");
    }
    case "delta":
      inRun();
      return isEmpty(delta) ? "增量是空的" : flow.read.text(deltaFile);
    case "check": {
      inRun();
      const problems = checkDelta(base, delta, { nonEmpty: true, others: others() });
      return problems.length ? `对不上基线：\n- ${problems.join("\n- ")}` : `增量对得上基线${breakingOf(delta).length ? `；有破坏性改动（${breakingOf(delta).join("、")}），interface-compat 门禁要影响面含 interface 与一条 accepted 的决策` : ""}`;
    }
    case "pending": {
      const o = others();
      if (!o.length) return "没有别的在途运行动接口";
      return o.map((x) => `运行 ${x.run}（${x.subject}）：${[...x.ids].join("、")}`).join("\n");
    }
    case "module": {
      const { pos, opt } = parseArgs(rest, { value: ["paths"] });
      const [name, title] = pos;
      need(name && title && opt.paths, "xarch module <模块> \"<标题>\" --paths a,b");
      need(validModule(name), "模块名是小写字母、数字、连字符、下划线");
      need(!base.modules.has(name), `基线里已经有模块 ${name}`);
      need(!delta.modules.some((m) => m.module === name), `这次已经新建了 ${name}`);
      const paths = opt.paths.split(",").map((p) => p.trim()).filter(Boolean);
      need(paths.length, "--paths 要写这个模块的代码在哪，如 src/textkit/**");
      delta.modules.push({ module: name, title, paths });
      save();
      return `增量新建模块 ${name}${trail(`module:${name}`)}`;
    }
    case "add": {
      const { pos, opt } = parseArgs(rest, { bool: ["stable"] });
      const [module, kind, name, signature, description] = pos;
      need(module && kind && name && signature && description, "xarch add <模块> <种类> <名字> \"<签名>\" \"<说明>\" [--stable]");
      need(KINDS.includes(kind), `种类是 ${KINDS.join("、")}，不是 ${kind}`);
      need(base.modules.has(module) || delta.modules.some((m) => m.module === module), `没有模块 ${module}（有 ${[...base.modules.keys(), ...delta.modules.map((m) => m.module)].join("、") || "（无）"}；新模块先 xarch module）`);
      const id = makeId(kind, module, name);
      need(parseId(id), `编号不合形：${id}`);
      need(!base.byId.has(id), `基线里已经有 ${id}：改它用 xarch modify`);
      need(!delta.added.some((r) => r.id === id), `这次已经加了 ${id}`);
      delta.added.push({ id, signature, description, stability: opt.stable === true ? "stable" : "experimental" });
      save();
      return `增量新增 ${id}（${opt.stable === true ? "stable" : "experimental"}）${trail(id)}`;
    }
    case "modify": {
      const { pos, opt } = parseArgs(rest, { bool: ["breaking"], value: ["signature", "description"] });
      const id = need(pos[0], "xarch modify <编号> [--signature S] [--description D] [--breaking]");
      need(opt.signature !== undefined || opt.description !== undefined, "要改什么：--signature S 或 --description D");
      const patch = {};
      if (opt.signature !== undefined) patch.signature = opt.signature;
      if (opt.description !== undefined) patch.description = opt.description;
      const added = delta.added.find((r) => r.id === id);
      if (added && !base.byId.has(id)) {
        need(opt.breaking === undefined, `${id} 是这次新增的，还没有人依赖它，不用 --breaking`);
        Object.assign(added, patch);
        save();
        return `改了增量里新增的 ${id}`;
      }
      const e = baseline(id);
      need(e.stability !== "deprecated", `${id} 已经弃用，只能删（xarch remove），不再演进`);
      if (e.stability === "stable") {
        need(
          opt.breaking === true,
          `${id} 是 stable，别人在依赖它：改它必须声明破坏——xarch modify ${id} … --breaking（还要在 decisions 台账里记一条 accepted，影响面含 interface）；` +
            `不想破坏就别改它，另加一个元素（xarch add），再把旧的 xarch deprecate`,
        );
      } else need(opt.breaking === undefined, `${id} 是 experimental，不构成承诺，不用 --breaking`);
      need(
        (patch.signature !== undefined && patch.signature !== e.signature) || (patch.description !== undefined && patch.description !== e.description),
        `和基线里的 ${id} 一样，没有要改的`,
      );
      for (const k of ["deprecated", "removed"]) need(!delta[k].includes(id), `${id} 这次已经在 ${k} 里：先 xarch revert ${id}`);
      const prev = delta.modified.find((r) => r.id === id);
      delta.modified = delta.modified.filter((r) => r.id !== id);
      delta.modified.push({ id, ...(prev || {}), ...patch, breaking: e.stability === "stable" });
      save();
      return `增量修改 ${id}${e.stability === "stable" ? "（breaking）" : ""}${trail(id)}`;
    }
    case "promote": {
      const id = need(rest[0], "xarch promote <编号>");
      const e = baseline(id);
      need(e.stability === "experimental", `${id} 现在是 ${e.stability}：只有 experimental 能转正`);
      for (const k of ["deprecated", "removed"]) need(!delta[k].includes(id), `${id} 这次已经在 ${k} 里：先 xarch revert ${id}`);
      delta.promoted = [...new Set([...delta.promoted, id])];
      save();
      return `增量转正 ${id}${trail(id)}`;
    }
    case "deprecate": {
      const id = need(rest[0], "xarch deprecate <编号>");
      const e = baseline(id);
      need(e.stability !== "deprecated", `${id} 已经是 deprecated，要删它用 xarch remove（归档后另开一次变更）`);
      for (const k of ["promoted", "removed"]) need(!delta[k].includes(id), `${id} 这次已经在 ${k} 里：先 xarch revert ${id}`);
      need(!delta.modified.some((r) => r.id === id), `${id} 这次已经改了：先 xarch revert ${id}，弃用的元素不再演进`);
      delta.deprecated = [...new Set([...delta.deprecated, id])];
      save();
      return `增量弃用 ${id}${trail(id)}`;
    }
    case "remove": {
      const id = need(rest[0], "xarch remove <编号>");
      const e = baseline(id);
      need(delta.deprecated.includes(id) === false, `${id} 是这次刚弃用的：删它要等这次归档之后，另开一次变更`);
      need(
        e.stability === "deprecated",
        `${id} 在基线里是 ${e.stability}，不能直接删：先 xarch deprecate ${id}，这次归档之后另开一次变更再 remove（删弃用的元素是破坏性的，要给调用方一个过渡）`,
      );
      for (const k of ["modified", "promoted"]) need(!inDelta(id).includes(k), `${id} 这次已经在 ${k} 里：先 xarch revert ${id}`);
      delta.removed = [...new Set([...delta.removed, id])];
      save();
      return `增量删除 ${id}${trail(id)}`;
    }
    case "revert": {
      const id = need(rest[0], "xarch revert <编号|模块>");
      const had = touched(delta);
      const mod = delta.modules.find((m) => m.module === id);
      if (mod) {
        // 新建的模块带着它名下新增的元素一起拿掉：留着它们会指向一个不存在的模块。
        delta.modules = delta.modules.filter((m) => m.module !== id);
        delta.added = delta.added.filter((r) => parseId(r.id)?.module !== id);
      }
      delta.added = delta.added.filter((r) => r.id !== id);
      delta.modified = delta.modified.filter((r) => r.id !== id);
      for (const k of ["promoted", "deprecated", "removed"]) delta[k] = delta[k].filter((x) => x !== id);
      need(had.has(id) || mod, `增量里没有 ${id}`);
      save();
      return `拿掉了 ${id}`;
    }
    case "archive": {
      // 归档站对每次运行都跑它：项目没开接口基线或这次没动接口，不是错，只是没有要并的。
      if (!governance(flow).interface) return "这个项目没开接口基线（.conix/governance/xforge.yaml），没有要并的";
      inRun();
      if (isEmpty(delta)) return "接口增量是空的，没有要并的";
      const problems = checkDelta(base, delta, { nonEmpty: true, others: [] });
      need(!problems.length, `增量对不上基线，不能并：\n- ${problems.join("\n- ")}`);
      const merged = mergeDelta(base, delta);
      const files = [];
      for (const name of merged.touched) {
        const m = merged.modules.get(name);
        const path = m.path || `${ifaceDir}/${name}.yaml`;
        flow.write.yaml(path, moduleFile(m));
        files.push(path);
      }
      const c = counts(delta);
      return `并进了 ${files.join("、")}（新模块 ${c.modules}、新增 ${c.added}、修改 ${c.modified}、转正 ${c.promoted}、弃用 ${c.deprecated}、删除 ${c.removed}）`;
    }
    default:
      return "xarch modules|list|get|delta|check|pending|module|add|modify|promote|deprecate|remove|revert|archive（xarch help 看用法）";
  }
}
