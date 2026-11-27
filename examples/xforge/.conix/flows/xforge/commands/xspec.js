// xspec：规格与这次变更的规格增量。规格在 ${data_dir}/specs/<能力>.md，每条需求是 `### <编号> <标题>` 加正文；
// 增量在这次运行的 spec-delta.yaml。模型不手写这两样，用这个命令。
//
// 正文写成 WHEN / THEN 条目（能直接写成黑盒测试），每条一行，各至少一条：
//   - WHEN 条件            （或 - 当 条件 时）
//   - THEN 结果            （或 - 应 结果）
// add 与 modify 的正文没有这两样就报错、不写。
//
//   xspec list [能力]                         列出需求（不写能力就列能力）
//   xspec get <编号>                          一条需求的正文
//   xspec delta                               看这次运行的增量
//   xspec check                               增量对得上基线没有：编号、WHEN/THEN、与别的在途运行有没有冲突
//                                             （与门禁 spec-delta 同一套规矩）
//   xspec pending                             别的在途运行（同一个包）的增量动了哪些编号
//   xspec init <能力> <前缀> "<标题>"           建一份新的规格（前缀如 TK，编号 TK-R1 起）
//   xspec add <能力> "<标题>" "<正文>"          增量里加一条新需求；编号排在基线与别的在途运行新增的编号之后
//   xspec modify <编号> "<正文>" [--title T]   增量里改一条已有的需求
//   xspec remove <编号>                       增量里删一条
//   xspec revert <编号>                       把一个编号从增量里拿掉
//   xspec archive                             把增量并进规格（归档站用；有冲突或对不上基线就不并）
//   xspec seal                                封存规格侧：这次运行目录里每个文件的摘要（solid、major 分两路之前的 seal 站用，
//                                             交给两个子运行，门禁 spec-unchanged 拿它核对子运行没改规格侧）
//
// 项目的 .conix/governance/xforge.yaml 里 governance.spec 为 false 时没有规格基线：只有读的子命令能用。
import { need, parseFlags } from "../lib/cli.js";
import { governance } from "../lib/governance.js";
import { readSpecs, scenarioProblems, unescapeBody, SCENARIO_SHAPE } from "../lib/specs.js";
import { checkDelta, inflight, highestTaken } from "../lib/delta.js";
import { sealSide } from "../lib/side.js";

const WRITES = ["init", "add", "modify", "remove", "revert", "archive"];
const NEEDS_RUN = ["add", "modify", "remove", "revert", "delta", "check", "archive", "seal"];

export default function (input, flow) {
  const [sub, ...rest] = input.args;
  const { pos, opt } = parseFlags(rest);
  need(governance(flow).spec || !WRITES.includes(sub), "这个项目没开规格基线（.conix/governance/xforge.yaml）：governance.spec 是 false，不能写规格与增量");
  need(input.output_dir || !NEEDS_RUN.includes(sub), `xspec ${sub} 要在运行里（没有这次变更的目录）`);
  const specsDir = `${input.data_dir}/specs`;
  const deltaFile = `${input.output_dir}/spec-delta.yaml`;
  const specs = readSpecs(flow, `${specsDir}/*.md`);
  const delta = (input.output_dir && flow.read.yaml(deltaFile)) || {};
  // 别的在途运行的增量：只在要排号、判冲突、列出来时才读。
  const others = () => inflight(flow, input.run);
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
      const [cap, title] = pos;
      const text = unescapeBody(pos[2]);
      const f = need(specs.files.get(cap), `没有能力 ${cap}：先 xspec init`);
      need(title && text, "xspec add <能力> \"<标题>\" \"<正文>\"");
      shaped(text);
      const used = [...f.reqs.map((r) => r.id), ...(delta.added || []).map((r) => r.id)].filter((id) => id.startsWith(f.prefix + "-R"));
      // 别的在途运行新增的编号也算占了：不然两个运行各自排出同一个编号，归档时撞车。
      const next = Math.max(0, highestTaken(others(), f.prefix), ...used.map((x) => Number(x.split("-R")[1]))) + 1;
      const id = `${f.prefix}-R${next}`;
      (delta.added = delta.added || []).push({ id, title, text });
      save();
      return `增量新增 ${id}`;
    }
    case "modify": {
      const [id] = pos;
      const text = unescapeBody(pos[1]);
      need(specs.byId.has(id), `没有需求 ${id}`);
      need(text, "xspec modify <编号> \"<正文>\" [--title T]");
      shaped(text);
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
    case "pending": {
      const o = others();
      if (!o.length) return "别的在途运行没有动规格";
      const ids = (label, a) => (a.length ? `${label} ${a.join("、")}` : "");
      return o.map((x) => `${x.run}${x.subject ? `（${x.subject}）` : ""}：${[ids("新增", x.added), ids("修改", x.modified), ids("删除", x.removed)].filter(Boolean).join("；")}`).join("\n");
    }
    case "check": {
      if (!governance(flow).spec) return "不欠（governance.spec=false）";
      const problems = checkDelta(specs, delta, { nonEmpty: true, others: others() });
      return problems.length ? `对不上基线：\n- ${problems.join("\n- ")}` : "增量对得上基线";
    }
    case "archive": {
      const problems = checkDelta(specs, delta, { nonEmpty: true, others: others() });
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
    case "seal": {
      const files = sealSide(flow, input.output_dir);
      need(Object.keys(files).length, `${input.output_dir} 里没有文件：规格侧还没写`);
      return { files, count: Object.keys(files).length };
    }
    default:
      throw new Error(`xspec 没有子命令 ${sub || "（空）"}：list|get|delta|check|pending|init|add|modify|remove|revert|archive|seal（xspec help 看用法）`);
  }
}

// add 与 modify 当场判正文的形状：写进增量再等门禁报，模型已经走远了。
function shaped(text) {
  const bad = scenarioProblems(text);
  need(!bad.length, `正文要写成 WHEN / THEN 条目：${bad.join("；")}。形如\n${SCENARIO_SHAPE}`);
}

function replaceReq(text, id, by) {
  const re = new RegExp(`^###\\s+${id}\\s.*$[\\s\\S]*?(?=^###\\s|(?![\\s\\S]))`, "m");
  return text.replace(re, by);
}

