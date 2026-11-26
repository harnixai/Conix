// xspec：规格与这次变更的规格增量。规格在 ${data_dir}/specs/<能力>.md，每条需求是 `### <编号> <标题>` 加正文；
// 增量在这次运行的 spec-delta.yaml。模型不手写这两样，用这个命令。
//
//   xspec list [能力]                         列出需求（不写能力就列能力）
//   xspec get <编号>                          一条需求的正文
//   xspec init <能力> <前缀> "<标题>"           建一份新的规格（前缀如 TK，编号 TK-R1 起）
//   xspec add <能力> "<标题>" "<正文>"          增量里加一条新需求，编号接着排
//   xspec modify <编号> "<正文>" [--title T]   增量里改一条已有的需求
//   xspec remove <编号>                       增量里删一条
//   xspec revert <编号>                       把一个编号从增量里拿掉
//   xspec delta                               看增量
//   xspec check                               增量对得上基线没有（与门禁 spec-delta 同一套规矩）
//   xspec archive                             把增量并进规格（归档站用）
import { need, parseFlags } from "../lib/cli.js";
import { readSpecs } from "../lib/specs.js";
import { checkDelta } from "../lib/delta.js";

export default function (input, flow) {
  const [sub, ...rest] = input.args;
  const { pos, opt } = parseFlags(rest);
  const specsDir = `${input.data_dir}/specs`;
  const deltaFile = `${input.output_dir}/spec-delta.yaml`;
  const specs = readSpecs(flow, `${specsDir}/*.md`);
  const delta = flow.read.yaml(deltaFile) || {};
  const save = () => {
    const d = {};
    for (const k of ["added", "modified", "removed"]) if (delta[k] && delta[k].length) d[k] = delta[k];
    flow.write.yaml(deltaFile, d);
  };
  switch (sub) {
    case "list": {
      if (!pos[0]) return [...specs.files.entries()].map(([cap, f]) => `${cap}（${f.prefix}）：${f.reqs.length} 条 — ${f.title}`).join("\n") || "还没有规格：xspec init <能力> <前缀> \"<标题>\"";
      const f = need(specs.files.get(pos[0]), `没有能力 ${pos[0]}`);
      return f.reqs.map((r) => `${r.id} ${r.title}`).join("\n");
    }
    case "get": {
      const r = need(specs.byId.get(pos[0]), `没有需求 ${pos[0]}`);
      return `### ${r.id} ${r.title}\n${r.text}`;
    }
    case "init": {
      const [cap, prefix, title] = pos;
      need(cap && /^[a-z0-9-]+$/.test(cap), "能力名是小写字母、数字、连字符");
      need(prefix && /^[A-Z]+$/.test(prefix), "前缀是大写字母，如 TK");
      need(!specs.files.has(cap), `已经有 ${cap}`);
      flow.write.text(`${specsDir}/${cap}.md`, `# ${title || cap}\n\n<!-- xspec: prefix ${prefix} -->\n`);
      return `建好 ${specsDir}/${cap}.md，编号 ${prefix}-R1 起`;
    }
    case "add": {
      const [cap, title, text] = pos;
      const f = need(specs.files.get(cap), `没有能力 ${cap}：先 xspec init`);
      need(title && text, "xspec add <能力> \"<标题>\" \"<正文>\"");
      const used = [...f.reqs.map((r) => r.id), ...(delta.added || []).map((r) => r.id)].filter((id) => id.startsWith(f.prefix + "-R"));
      const id = `${f.prefix}-R${Math.max(0, ...used.map((x) => Number(x.split("-R")[1]))) + 1}`;
      (delta.added = delta.added || []).push({ id, title, text });
      save();
      return `增量新增 ${id}`;
    }
    case "modify": {
      const [id, text] = pos;
      need(specs.byId.has(id), `没有需求 ${id}`);
      need(text, "xspec modify <编号> \"<正文>\" [--title T]");
      delta.modified = (delta.modified || []).filter((r) => r.id !== id);
      delta.modified.push(opt.title ? { id, title: opt.title, text } : { id, text });
      save();
      return `增量修改 ${id}`;
    }
    case "remove": {
      need(specs.byId.has(pos[0]), `没有需求 ${pos[0]}`);
      delta.removed = [...new Set([...(delta.removed || []), pos[0]])];
      save();
      return `增量删除 ${pos[0]}`;
    }
    case "revert": {
      const id = pos[0];
      for (const k of ["added", "modified"]) delta[k] = (delta[k] || []).filter((r) => r.id !== id);
      delta.removed = (delta.removed || []).filter((x) => x !== id);
      save();
      return `拿掉了 ${id}`;
    }
    case "delta":
      return flow.read.text(deltaFile) || "增量是空的";
    case "check": {
      const problems = checkDelta(specs, delta, { nonEmpty: true });
      return problems.length ? `对不上基线：\n- ${problems.join("\n- ")}` : "增量对得上基线";
    }
    case "archive": {
      const problems = checkDelta(specs, delta, { nonEmpty: true });
      need(!problems.length, `增量对不上基线，不能并：\n- ${problems.join("\n- ")}`);
      const touched = new Set();
      for (const r of delta.added || []) {
        if (specs.byId.has(r.id)) continue;
        const f = [...specs.files.values()].find((f) => r.id.startsWith(f.prefix + "-R"));
        f.text = f.text.trimEnd() + `\n\n### ${r.id} ${r.title}\n${r.text.trim()}\n`;
        touched.add(f);
      }
      for (const r of delta.modified || []) {
        const old = specs.byId.get(r.id);
        const f = old.file;
        f.text = replaceReq(f.text, r.id, `### ${r.id} ${r.title || old.title}\n${r.text.trim()}\n`);
        touched.add(f);
      }
      for (const id of delta.removed || []) {
        const f = specs.byId.get(id).file;
        f.text = replaceReq(f.text, id, "");
        touched.add(f);
      }
      for (const f of touched) flow.write.text(f.path, f.text.replace(/\n{3,}/g, "\n\n"));
      return `并进了 ${[...touched].map((f) => f.path).join("、") || "（没有要并的）"}`;
    }
    default:
      return "xspec list|get|init|add|modify|remove|revert|delta|check|archive（看文件头的说明）";
  }
}

function replaceReq(text, id, by) {
  const re = new RegExp(`^###\\s+${id}\\s.*$[\\s\\S]*?(?=^###\\s|(?![\\s\\S]))`, "m");
  return text.replace(re, by);
}
