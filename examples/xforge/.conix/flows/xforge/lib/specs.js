// 规格的读法：门禁 spec-delta、archived 与命令 xspec 共用这一份，判法与改法才不会各说各的。
// 规格在 specs/<能力>.md，每条需求是 `### <编号> <标题>` 加正文。

// 读 pattern（glob）匹配的规格文件。
//   files    能力名 → { cap, path, text, title, prefix, reqs }
//   byId     编号 → { id, title, text, file }（text 去掉了正文里的一二级标题行）
//   prefixes 前缀 → 已有的最大编号：新增的编号必须接在它后面，不复用
export function readSpecs(flow, pattern) {
  const files = new Map();
  const byId = new Map();
  const prefixes = new Map();
  for (const path of flow.glob(pattern)) {
    const text = flow.read.text(path) || "";
    const cap = flow.path.basename(path, ".md");
    const heads = [...text.matchAll(/^###\s+([A-Z]+-R\d+)\s+(.*)$/gm)];
    const marked = /<!--\s*xspec:\s*prefix\s+([A-Z]+)\s*-->/.exec(text);
    const f = {
      cap,
      path,
      text,
      title: ((/^#\s+(.*)$/m.exec(text) || [])[1] || cap).trim(),
      reqs: [],
      prefix: marked ? marked[1] : heads.length ? splitId(heads[0][1])[0] : cap.toUpperCase(),
    };
    heads.forEach((m, i) => {
      const end = i + 1 < heads.length ? heads[i + 1].index : text.length;
      const body = text.slice(m.index + m[0].length, end).replace(/^#{1,2}\s.*$/gm, "").trim();
      const r = { id: m[1], title: m[2].trim(), text: body, file: f };
      f.reqs.push(r);
      byId.set(r.id, r);
    });
    files.set(cap, f);
    prefixes.set(f.prefix, Math.max(prefixes.get(f.prefix) ?? 0, ...f.reqs.map((r) => splitId(r.id)[1])));
  }
  return { files, byId, prefixes };
}

// "TK-R12" → ["TK", 12]；不是编号就是 ["", 0]。
export function splitId(id) {
  const m = /^([A-Z]+)-R(\d+)$/.exec(id || "");
  return m ? [m[1], Number(m[2])] : ["", 0];
}

// 比正文只看字不看空白。
export function squash(s) {
  return String(s || "").replace(/\s+/g, "");
}

// 一条需求的正文要写成 WHEN / THEN 条目，才能直接写成黑盒测试（docs/design/xforge.md §2.1）：
// `- WHEN …` 与 `- THEN …`，或中文的 `- 当 … 时` 与 `- 应 …`。各至少一条。
// 为什么按行认：整段话里夹着 "when" 的叙述不算，条目必须是列表项，测试才有一条一条的对应。
const WHEN_ITEM = /^\s*[-*]\s+(?:WHEN\b|当.*时)/m;
const THEN_ITEM = /^\s*[-*]\s+(?:THEN\b|应)/m;

export const SCENARIO_SHAPE = "- WHEN <条件>\n- THEN <结果>（或中文：- 当 <条件> 时 / - 应 <结果>），一行一条";

// 回缺什么：[] 就是形状对。
export function scenarioProblems(text) {
  const out = [];
  if (!WHEN_ITEM.test(text || "")) out.push("没有 WHEN 条目（`- WHEN …` 或 `- 当 … 时`）");
  if (!THEN_ITEM.test(text || "")) out.push("没有 THEN 条目（`- THEN …` 或 `- 应 …`）");
  return out;
}

// 命令行里传多行正文不方便，模型常写成字面的 `\n`：正文里没有真换行时，把它们当换行。
export function unescapeBody(text) {
  const t = String(text || "");
  return t.includes("\n") ? t : t.replace(/\\n/g, "\n");
}
