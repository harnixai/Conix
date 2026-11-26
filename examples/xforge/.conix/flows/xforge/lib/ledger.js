// 台账的规矩（docs/design/xforge.md §2.3）：命令 xledger 写之前查，门禁 ledgers-settled 读之后再查，
// 同一份判法，不会命令放行了、门禁又不认。这里不用 cli.js：门禁脚本不引它。

// 检查站的站 id：只有它和人能给 findings 的 rejected / deferred、能重开，能逐条答原则。
export const CHECK_STATION = "check";

// 种类 → 编号前缀、能给的结论、"还没结"的结论（离开站前不能停在这些上）。
export const KINDS = {
  decisions: { prefix: "D", conclusions: ["open", "accepted", "rejected", "resolved", "deferred"], unsettled: ["open"] },
  risks: { prefix: "R", conclusions: ["open", "accepted", "rejected", "resolved", "deferred"], unsettled: ["open"] },
  findings: { prefix: "F", conclusions: ["open", "resolved", "disputed", "rejected", "deferred"], unsettled: ["open", "disputed"] },
  questions: { prefix: "Q", conclusions: ["open", "answered", "deferred"], unsettled: ["open"] },
  // 原则编号就是条目编号；由 xledger reply 写，不走 add / set。
  constitution: { prefix: "P", conclusions: ["complies", "violates", "n-a"], unsettled: [] },
};

// 谁在调。by 是命令输入里的 by：模型在站的会话里调时 by.kind 是 model，站在 input.station；
// 站自己的命令步骤 by.kind 是 station，by.id 就是站 id。两种都记成 {kind: station, id: <站>}：
// 规矩只关心"哪一站"与"是不是人"，不关心站里是哪个模型。站之外的模型保留 {kind: model, id}。
export function caller(input) {
  const by = input.by || { kind: "model", id: "?" };
  if (by.kind === "person") return { kind: "person", id: by.id };
  const station = input.station || (by.kind === "station" ? by.id : null);
  if (station) return { kind: "station", id: station };
  return { kind: by.kind, id: by.id };
}

export const isPerson = (by) => !!by && by.kind === "person";
export const isCheck = (by) => !!by && by.kind === "station" && by.id === CHECK_STATION;

export function showBy(by) {
  if (!by) return "不明";
  return by.kind === "person" ? `人 ${by.id}` : by.kind === "station" ? `${by.id} 站` : `模型 ${by.id}`;
}

// 谁能给这个结论：能就回 null，不能回"该谁给"的话（命令原样抛给调用的人）。
export function refuseSet(kind, conclusion, who) {
  if (kind === "questions" && !isPerson(who)) {
    return `问题只有人能回答或搁置（by.kind 要是 person，现在是 ${showBy(who)}）：把问题留给人，在澄清站的确认里答`;
  }
  if (kind === "findings" && !isPerson(who) && !isCheck(who)) {
    if (conclusion === "resolved" || conclusion === "disputed") return null;
    return `发现的问题，作者只能给 resolved（改好了）或 disputed（不同意）；${conclusion} 只有检查站（${CHECK_STATION}）或人能给，现在是 ${showBy(who)}`;
  }
  return null;
}

// 读台账文件的内容：{entries: [...]}，没有就是空的。
export function entriesOf(doc) {
  return doc && Array.isArray(doc.entries) ? doc.entries : [];
}

// 一份台账里还有什么不合规矩：[{id, message}]。命令不一定拦得住手改的文件，门禁用这个兜。
export function ledgerProblems(kind, doc) {
  const k = KINDS[kind];
  if (!k) return [{ id: "", message: `不认识的台账 ${kind}：种类是 ${Object.keys(KINDS).join("、")}` }];
  if (!doc || !Array.isArray(doc.entries)) return [{ id: "", message: "台账要有 entries 数组" }];
  const out = [];
  const bad = (e, message) => out.push({ id: e.id || "?", message });
  for (const e of doc.entries) {
    if (!e || typeof e !== "object" || !e.id) { bad({}, "有一条没有 id"); continue; }
    if (!k.conclusions.includes(e.conclusion)) {
      bad(e, `结论 ${e.conclusion} 不是 ${kind} 的结论（${k.conclusions.join("、")}）`);
      continue;
    }
    if (kind === "constitution") continue; // 逐条答没答，由门禁 constitution-reply 判
    if (k.unsettled.includes(e.conclusion)) {
      bad(e, kind === "findings" && e.conclusion === "disputed"
        ? "作者不同意（disputed），算没结：检查站认可后给 rejected / resolved，或人来定"
        : kind === "questions"
          ? "问题还没答：要人来答（xledger set questions <编号> answered \"<回答>\"）"
          : `还是 ${e.conclusion}：xledger set ${kind} ${e.id} accepted|rejected|resolved|deferred`);
      continue;
    }
    if (kind === "findings") {
      if (!Array.isArray(e.refs) || !e.refs.length) bad(e, "没有 ref：发现的问题要指向它说的地方（路径#小节，或需求/元素编号）");
      if ((e.conclusion === "rejected" || e.conclusion === "deferred") && !isPerson(e.by) && !isCheck(e.by)) {
        bad(e, `${e.conclusion} 是 ${showBy(e.by)} 给的：只有检查站或人能给`);
      }
    }
    if (kind === "questions" && !isPerson(e.by)) bad(e, `结论是 ${showBy(e.by)} 给的：问题只有人能答`);
  }
  return out;
}
