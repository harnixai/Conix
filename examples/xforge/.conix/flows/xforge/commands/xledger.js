// xledger：这次变更的台账，在这次运行的 ledgers/<种类>.yaml。模型不手写这些文件，用这个命令。
// 种类：decisions（决定 D）、risks（风险 R）、findings（发现的问题 F）、questions（要人答的问题 Q）、
// constitution（对原则的逐条回答，编号就是原则编号 P1…）。
//
//   xledger add <种类> "<内容>" [--ref a,b]            记一条，回它的编号；先是 open
//                                                      findings 至少一个 --ref：路径#小节，或需求/元素编号
//   xledger set <种类> <编号> <结论> ["<说明>"]         定结论（open 是重开）；每次都追加进这一条的 history
//   xledger reply <原则> complies|violates|n-a "<说明>"  逐条答原则（只有检查站或人）；violates 要说明
//   xledger list [种类]                                 全列出来，含谁给的结论
//
// 结论与谁能给：
//   decisions、risks   open → accepted | rejected | resolved | deferred，谁都能给；离开设计站前都要有结论
//   findings           open | resolved | disputed | rejected | deferred
//                      作者（设计等站）只能给 resolved（改好了）、disputed（不同意，算没结）；
//                      rejected、deferred、重开（open）只有检查站（check）或人能给
//   questions          open | answered | deferred；只有人能给结论，回答写在说明里
//   constitution       complies | violates | n-a；只能用 reply
import { need, parseFlags } from "../lib/cli.js";
import { KINDS, caller, entriesOf, isCheck, isPerson, refuseSet, showBy } from "../lib/ledger.js";

const list = (s) => (s || "").split(",").map((x) => x.trim()).filter(Boolean);

export default function (input, flow) {
  const [sub, ...rest] = input.args;
  need(input.output_dir, "xledger 写的是这次运行的台账：要在运行里调用");
  const file = (k) => `${input.output_dir}/ledgers/${k}.yaml`;
  const load = (k) => ({ entries: entriesOf(flow.read.yaml(file(k))) });
  const who = caller(input);
  const at = new Date().toISOString();

  if (sub === "list") {
    const kinds = rest[0] ? [need(KINDS[rest[0]] && rest[0], `种类是 ${Object.keys(KINDS).join("、")} 之一`)] : Object.keys(KINDS);
    return kinds.map((k) => load(k).entries.map((e) => {
      const refs = (e.refs || []).length ? `；refs ${e.refs.join(", ")}` : "";
      return `${e.id} [${e.conclusion}] ${e.text}${e.note ? ` —— ${e.note}` : ""}（${showBy(e.by)}${refs}）`;
    }).join("\n")).filter(Boolean).join("\n") || "台账是空的";
  }

  if (sub === "reply") {
    const [pid, conclusion, ...why] = rest;
    need(isPerson(who) || isCheck(who), `只有检查站（check）或人能答原则，现在是 ${showBy(who)}`);
    need(pid && conclusion, "xledger reply <原则> complies|violates|n-a \"<说明>\"");
    need(KINDS.constitution.conclusions.includes(conclusion), `结论是 ${KINDS.constitution.conclusions.join("、")} 之一`);
    const note = why.join(" ").trim();
    if (conclusion === "violates") need(note, "violates 要说明：违反了它的哪一点、设计或实现里的哪里");
    const doc = flow.read.yaml(".conix/governance/constitution.yaml");
    const principles = (doc && doc.principles) || [];
    need(principles.length, "项目没有原则（.conix/governance/constitution.yaml 里没有 principles）：没有可答的");
    const p = need(principles.find((x) => x.id === pid), `项目没有原则 ${pid}，有：${principles.map((x) => x.id).join("、")}`);
    const l = load("constitution");
    let e = l.entries.find((x) => x.id === pid);
    if (!e) l.entries.push((e = { id: pid, text: p.title || p.id, conclusion, refs: [], history: [] }));
    e.conclusion = conclusion;
    e.by = who;
    if (note) e.note = note; else delete e.note;
    e.history = (e.history || []).concat([{ conclusion, by: who, at, ...(note ? { note } : {}) }]);
    flow.write.yaml(file("constitution"), l);
    return `${pid} → ${conclusion}`;
  }

  const kind = rest[0];
  need(kind && KINDS[kind], `种类是 ${Object.keys(KINDS).join("、")} 之一`);
  need(kind !== "constitution", "constitution 台账用 xledger reply <原则> complies|violates|n-a \"<说明>\" 写");
  const k = KINDS[kind];
  const l = load(kind);

  if (sub === "add") {
    const { pos, opt } = parseFlags(rest.slice(1));
    const text = need(pos.join(" ").trim(), "xledger add <种类> \"<内容>\" [--ref a,b]");
    const refs = list(opt.ref);
    if (kind === "findings") need(refs.length, "发现的问题至少一个 --ref：它说的是哪里（路径#小节，或需求/元素编号），如 --ref design.md#做法,TK-R2");
    const n = Math.max(0, ...l.entries.map((e) => Number(String(e.id).slice(k.prefix.length)) || 0)) + 1;
    const id = `${k.prefix}${n}`;
    l.entries.push({ id, text, conclusion: "open", refs, by: who, history: [{ conclusion: "open", by: who, at }] });
    flow.write.yaml(file(kind), l);
    return `记下 ${id}（open）：${kind === "questions" ? "要人来答" : "离开这一站前给它结论"}`;
  }

  if (sub === "set") {
    const [id, conclusion, ...note] = rest.slice(1);
    need(id && conclusion, "xledger set <种类> <编号> <结论> [\"<说明>\"]");
    const e = need(l.entries.find((e) => e.id === id), `没有 ${kind} ${id}`);
    need(k.conclusions.includes(conclusion), `${kind} 的结论是 ${k.conclusions.join("、")} 之一`);
    const refused = refuseSet(kind, conclusion, who);
    if (refused) throw new Error(refused);
    const text = note.join(" ").trim();
    if (kind === "questions" && conclusion === "answered") need(text, "回答写在说明里：xledger set questions <编号> answered \"<回答>\"");
    e.conclusion = conclusion;
    e.by = who;
    if (text) e.note = text;
    e.history = (e.history || []).concat([{ conclusion, by: who, at, ...(text ? { note: text } : {}) }]);
    flow.write.yaml(file(kind), l);
    return `${id} → ${conclusion}`;
  }
  return "xledger add|set|reply|list（看文件头的说明）";
}
