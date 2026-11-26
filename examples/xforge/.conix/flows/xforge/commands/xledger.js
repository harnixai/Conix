// xledger：这次变更的台账，在这次运行的 ledgers/<种类>.yaml。种类：decisions（决定）、risks（风险）、findings（发现的问题）。
// 每条都先是 open，离开那一站前要有结论：accepted、rejected、resolved 或 deferred（台账门禁查）。
//
//   xledger add <种类> "<内容>"                      记一条，回它的编号（D1、R1、F1）
//   xledger set <种类> <编号> <结论> ["<说明>"]       给一条定结论（open 也行：重新打开）
//   xledger list [种类]                              列出来
import { need } from "../lib/cli.js";

const KINDS = { decisions: "D", risks: "R", findings: "F" };
const CONCLUSIONS = ["open", "accepted", "rejected", "resolved", "deferred"];

export default function (input, flow) {
  const [sub, kind, ...rest] = input.args;
  const file = (k) => `${input.output_dir}/ledgers/${k}.yaml`;
  const load = (k) => (flow.read.yaml(file(k)) || { entries: [] });
  if (sub === "list") {
    const kinds = kind ? [kind] : Object.keys(KINDS);
    return kinds.map((k) => load(k).entries.map((e) => `${e.id} [${e.conclusion}] ${e.text}${e.note ? ` —— ${e.note}` : ""}`).join("\n")).filter(Boolean).join("\n") || "台账是空的";
  }
  need(KINDS[kind], `种类是 ${Object.keys(KINDS).join("、")} 之一`);
  const l = load(kind);
  if (sub === "add") {
    const text = need(rest.join(" ").trim(), "xledger add <种类> \"<内容>\"");
    const id = `${KINDS[kind]}${l.entries.length + 1}`;
    l.entries.push({ id, text, conclusion: "open" });
    flow.write.yaml(file(kind), l);
    return `记下 ${id}（open）：离开这一站前给它结论`;
  }
  if (sub === "set") {
    const [id, conclusion, ...note] = rest;
    const e = need(l.entries.find((e) => e.id === id), `没有 ${kind} ${id}`);
    need(CONCLUSIONS.includes(conclusion), `结论是 ${CONCLUSIONS.join("、")} 之一`);
    e.conclusion = conclusion;
    if (note.length) e.note = note.join(" ");
    flow.write.yaml(file(kind), l);
    return `${id} → ${conclusion}`;
  }
  return "xledger add|set|list（看文件头的说明）";
}
