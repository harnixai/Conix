// XForge 插件的全部命令与宿主函数（docs/design/plugins.md §6）。
//
// 为什么是一份文件：入口文件各自单独求值，没有 require / import（runext.md §4），
// 共用代码只能拼进每个入口；plugin.toml 的所有 entry 都指向这里，省掉构建。
// 数据都在项目根的 xforge/ 下；写文件只经 tools.Write（要先读过，见 writeText），
// YAML 用 agix.yaml，glob 用 agix.glob。

class XError extends Error {
  constructor(code, message) {
    super(message);
    this.xcode = code;
  }
}
const bad = (code, message) => new XError(code, message);

// 一次调用一个全新的运行时，这几个全局只活这一次调用。
let C = { root: "", input: null, ctx: null };
function begin(ctx, input) {
  C = { root: ctx.projectRoot, input: input || null, ctx };
}

const KINDS = ["decisions", "risks", "findings"];
const KIND_PREFIX = { decisions: "D", risks: "K", findings: "F" };
const CONCLUSIONS = ["open", "accepted", "rejected", "resolved", "deferred"];
const STATUSES = ["ready", "done", "blocked"];
const RISKS = ["low", "medium"];
const TEMPLATE_MARK = "<!-- xforge:template -->";
const TEMPLATES = {
  "proposal.md": `${TEMPLATE_MARK}\n# 提案\n\n## 为什么\n\n## 改什么\n\n## 不改什么\n`,
  "design.md": `${TEMPLATE_MARK}\n# 设计\n\n## 做法\n\n## 取舍\n\n## 原则对照\n`,
  "verification.md": `${TEMPLATE_MARK}\n# 验证\n\n## 怎么验证的\n\n## 结果\n`,
};
const ID_RE = /^[A-Za-z0-9][A-Za-z0-9._-]*$/;
const REQ_RE = /^([a-z][a-z0-9-]*)\/R(\d+)$/;
const NOISE = [".codix/", ".harnix/", ".agix/", ".wkts/", "xforge/"];

// ---------- 文件 ----------

const abs = (rel) => `${C.root}/${rel}`;

async function readText(rel) {
  try {
    const r = await tools.Read({ file_path: abs(rel) });
    if (r.truncated) throw bad("too_large", `${rel} is too large to read in one piece`);
    return r.content;
  } catch (e) {
    if (e instanceof XError) throw e;
    if (/no such file/i.test(String(e && e.message))) return null;
    throw e;
  }
}

// tools.Write 写已有文件前要读过：先读一次，读不到就是新文件。
async function writeText(rel, content) {
  await readText(rel);
  await tools.Write({ file_path: abs(rel), content });
}

async function readYaml(rel) {
  const text = await readText(rel);
  if (text === null) return null;
  try {
    return agix.yaml.parse(text);
  } catch (e) {
    throw bad("bad_yaml", `${rel}: ${e.message}`);
  }
}

const writeYaml = (rel, value) => writeText(rel, agix.yaml.stringify(value));

async function listFiles(pattern) {
  const r = await tools.Glob({ pattern, path: C.root });
  const prefix = `${C.root}/`;
  return r.paths.map((p) => (p.startsWith(prefix) ? p.slice(prefix.length) : p)).sort();
}

const isWritten = (text) => text !== null && text.trim() !== "" && !text.includes(TEMPLATE_MARK);

// ---------- 运行的上下文 ----------

async function flowCurrent() {
  try {
    return await codix.flow.current();
  } catch (_) {
    return null;
  }
}

async function runSummary(run) {
  if (!run) return null;
  try {
    const v = await codix.flow.run({ run });
    return { status: v.summary.status, stations: v.stations.map((s) => ({ id: s.id, status: s.status })) };
  } catch (_) {
    return null;
  }
}

// ---------- 变更 ----------

function checkId(id) {
  if (typeof id !== "string" || !ID_RE.test(id)) throw bad("invalid_change", `bad change id: ${JSON.stringify(id)}`);
  return id;
}

async function loadChange(id, { needOpen } = {}) {
  checkId(id);
  const doc = await readYaml(`xforge/changes/${id}/change.yaml`);
  if (!doc) throw bad("no_change", `no change ${id} (xforge/changes/${id}/change.yaml)`);
  if (needOpen && doc.status !== "open") throw bad("not_open", `change ${id} is ${doc.status}, not open`);
  return { id, dir: `xforge/changes/${id}`, doc };
}

async function openChanges() {
  const files = await listFiles("xforge/changes/*/change.yaml");
  const out = [];
  for (const f of files) {
    const doc = await readYaml(f);
    if (doc && doc.status === "open") out.push({ id: f.split("/")[2], doc });
  }
  return out;
}

// 当前变更：--change > 函数入参 outputs.open.change > 所在运行记在 change.yaml 里的 run > 唯一一个 open 的。
// RunView 里没有各站的输出，所以命令里靠 change.yaml 的 run 字段对上运行。
async function currentChange(explicit, { needOpen = true } = {}) {
  const id = explicit || (C.input && C.input.outputs && C.input.outputs.open && C.input.outputs.open.change);
  if (id) return loadChange(id, { needOpen });
  const open = await openChanges();
  const cur = await flowCurrent();
  if (cur && cur.run) {
    const mine = open.filter((c) => c.doc.run === cur.run);
    if (mine.length === 1) return loadChange(mine[0].id, { needOpen });
  }
  if (open.length === 1) return loadChange(open[0].id, { needOpen });
  if (open.length === 0) throw bad("no_change", "no open change: start one with `xforge start`");
  throw bad("ambiguous", `several open changes (${open.map((c) => c.id).join(", ")}): pass --change <id>`);
}

async function readDelta(ch) {
  const d = await readYaml(`${ch.dir}/delta.yaml`);
  if (!d || typeof d !== "object" || Array.isArray(d)) throw bad("bad_delta", `${ch.dir}/delta.yaml is missing or not a mapping`);
  for (const k of ["added", "modified", "removed"]) {
    if (d[k] === undefined || d[k] === null) d[k] = [];
    if (!Array.isArray(d[k])) throw bad("bad_delta", `${ch.dir}/delta.yaml: ${k} must be a list`);
  }
  return d;
}

const writeDelta = (ch, d) => writeYaml(`${ch.dir}/delta.yaml`, { added: d.added, modified: d.modified, removed: d.removed });

async function readPackages(ch) {
  const d = await readYaml(`${ch.dir}/work-packages.yaml`);
  if (d === null) return [];
  if (typeof d !== "object" || !Array.isArray(d.packages)) throw bad("bad_packages", `${ch.dir}/work-packages.yaml: packages must be a list`);
  return d.packages;
}

const writePackages = (ch, packages) => writeYaml(`${ch.dir}/work-packages.yaml`, { packages });

async function readLedger(ch, kind) {
  const d = await readYaml(`${ch.dir}/ledgers/${kind}.yaml`);
  if (d === null) return [];
  if (typeof d !== "object" || !Array.isArray(d.entries)) throw bad("bad_ledger", `${ch.dir}/ledgers/${kind}.yaml: entries must be a list`);
  return d.entries;
}

const writeLedger = (ch, kind, entries) => writeYaml(`${ch.dir}/ledgers/${kind}.yaml`, { entries });

async function loadSpecs() {
  const specs = {};
  for (const f of await listFiles("xforge/specs/*.yaml")) {
    const d = await readYaml(f);
    if (!d || typeof d.capability !== "string" || !Array.isArray(d.requirements)) throw bad("bad_spec", `${f}: needs capability and requirements`);
    specs[d.capability] = d;
  }
  return specs;
}

function indexReqs(specs) {
  const idx = {};
  for (const s of Object.values(specs)) for (const r of s.requirements) idx[r.id] = { cap: s.capability, req: r };
  return idx;
}

async function readManifest() {
  const m = await readYaml("xforge/manifest.yaml");
  if (!m) throw bad("not_initialised", "xforge/manifest.yaml is missing: run `xforge init`");
  return m;
}

function slugOf(title) {
  const s = String(title).toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "").slice(0, 40).replace(/-+$/, "");
  return s || "change";
}

function today() {
  const d = new Date();
  const p = (n) => String(n).padStart(2, "0");
  return `${d.getFullYear()}${p(d.getMonth() + 1)}${p(d.getDate())}`;
}

async function freshId(title) {
  const base = `${today()}-${slugOf(title)}`;
  const taken = new Set();
  for (const f of await listFiles("xforge/changes/*/change.yaml")) taken.add(f.split("/")[2]);
  for (const f of await listFiles("xforge/archive/*/change.yaml")) taken.add(f.split("/")[2]);
  let id = base;
  for (let n = 2; taken.has(id); n++) id = `${base}-${n}`;
  return id;
}

// ---------- 门禁 ----------

function nextReqNumber(cap, spec, delta) {
  let max = 0;
  const see = (id) => {
    const m = REQ_RE.exec(id || "");
    if (m && m[1] === cap) max = Math.max(max, Number(m[2]));
  };
  if (spec) {
    spec.requirements.forEach((r) => see(r.id));
    (spec.retired || []).forEach(see);
  }
  delta.added.forEach((a) => see(a.id));
  return max + 1;
}

async function gateSpecDelta(ch) {
  const delta = await readDelta(ch);
  const specs = await loadSpecs();
  const idx = indexReqs(specs);
  const retired = new Set(Object.values(specs).flatMap((s) => s.retired || []));
  const problems = [];
  const str = (v) => typeof v === "string" && v.trim() !== "";
  if (delta.added.length + delta.modified.length + delta.removed.length === 0) problems.push("the delta is empty: add, modify or remove at least one requirement");
  const seen = new Set();
  for (const a of delta.added) {
    const m = REQ_RE.exec(a.id || "");
    if (!str(a.capability) || !m || m[1] !== a.capability || !str(a.title) || !str(a.text)) {
      problems.push(`added ${a.id || "(no id)"}: needs capability, id <capability>/R<n>, title and text`);
      continue;
    }
    if (idx[a.id]) problems.push(`added ${a.id}: already in specs`);
    if (retired.has(a.id)) problems.push(`added ${a.id}: the number was retired and is not reused`);
    if (seen.has(a.id)) problems.push(`added ${a.id}: listed twice`);
    seen.add(a.id);
  }
  for (const m of delta.modified) {
    if (!str(m.id) || !str(m.text)) problems.push(`modified ${m.id || "(no id)"}: needs id and text`);
    else if (!idx[m.id]) problems.push(`modified ${m.id}: not in specs`);
  }
  for (const id of delta.removed) if (!idx[id]) problems.push(`removed ${id}: not in specs`);
  return verdict(problems, `delta: ${delta.added.length} added, ${delta.modified.length} modified, ${delta.removed.length} removed`);
}

function verdict(problems, okDetail) {
  return problems.length ? { pass: false, detail: problems.join("; ") } : { pass: true, detail: okDetail };
}

async function gateStructure(ch, station) {
  const problems = [];
  const written = async (name) => {
    if (!isWritten(await readText(`${ch.dir}/${name}`))) problems.push(`${ch.dir}/${name} is not written yet (empty or still the template)`);
  };
  if (station === "design") {
    await written("design.md");
    const pk = await readPackages(ch);
    if (pk.length === 0) problems.push("no work package: add one with `xflow plan add`");
    for (const p of pk) if (!Array.isArray(p.paths) || p.paths.length === 0) problems.push(`package ${p.id}: has no paths`);
  } else if (station === "verify") {
    await written("verification.md");
  } else {
    await written("proposal.md");
    try {
      await readDelta(ch);
    } catch (e) {
      problems.push(e.message);
    }
  }
  return verdict(problems, `structure ok at ${station || "propose"}`);
}

async function gateConstitution(ch) {
  const text = (await readText("xforge/constitution.md")) || "";
  const ids = [...text.matchAll(/^\s*-\s+\*\*(P\d+)\*\*/gm)].map((m) => m[1]);
  if (ids.length === 0) return { pass: true, detail: "constitution has no principles" };
  const design = await readText(`${ch.dir}/design.md`);
  if (!isWritten(design)) return { pass: false, detail: "design.md is not written yet" };
  const missing = ids.filter((p) => !new RegExp(`\\b${p}\\b`).test(design));
  return verdict(missing.length ? [`design.md does not mention ${missing.join(", ")}`] : [], `design mentions ${ids.join(", ")}`);
}

async function gateLedgers(ch) {
  const problems = [];
  for (const kind of KINDS) {
    for (const e of await readLedger(ch, kind)) if (e.conclusion === "open") problems.push(`${kind} ${e.id} is open: ${e.text}`);
  }
  return verdict(problems, "no open ledger entries");
}

async function gateIntegrated(ch) {
  const pk = await readPackages(ch);
  if (pk.length === 0) return { pass: false, detail: "no work package" };
  const rest = pk.filter((p) => p.status !== "done").map((p) => `${p.id} (${p.status})`);
  return verdict(rest.length ? [`not done: ${rest.join(", ")}`] : [], `${pk.length} package(s) done`);
}

// Bash 被拒的原因分两种，告诉人该动哪里。
function bashProblem(cmd, e) {
  const msg = String((e && e.message) || e);
  if (/without letting it write/.test(msg)) {
    return `\`${cmd}\` changes something, and a gate call may not write: use a read-only verification command, or run it in the station`;
  }
  if (/requires confirmation|Denied by permission/.test(msg)) {
    const prefix = cmd.split(/\s+/).slice(0, 2).join(" ");
    return `\`${cmd}\` was refused (a gate has nobody to ask): allow it in the project settings with permissions.allow "Bash(${prefix}:*)" — ${msg}`;
  }
  return `\`${cmd}\` failed: ${msg.slice(0, 400)}`;
}

function underNoise(path) {
  return NOISE.some((n) => path.startsWith(n));
}

async function gateUnclaimed(ch) {
  const pk = await readPackages(ch);
  let out;
  try {
    out = (await tools.Bash({ command: "git status --porcelain --untracked-files=all" })).stdout;
  } catch (e) {
    return { pass: false, detail: bashProblem("git status --porcelain --untracked-files=all", e) };
  }
  const changed = out
    .split("\n")
    .filter((l) => l.length > 3)
    .map((l) => {
      let p = l.slice(3);
      if (p.includes(" -> ")) p = p.split(" -> ").pop();
      return p.replace(/^"|"$/g, "");
    })
    .filter((p) => !underNoise(p));
  if (pk.length === 0) return { pass: true, detail: `no work package declared; ${changed.length} changed path(s) not checked` };
  const globs = pk.flatMap((p) => (Array.isArray(p.paths) ? p.paths : []));
  const patterns = globs.flatMap((g) => [g, `${g.replace(/\/+$/, "")}/**`]);
  const loose = changed.filter((p) => !agix.glob.test(patterns, p));
  return verdict(loose.length ? [`changed outside every package: ${loose.join(", ")}`] : [], `${changed.length} changed path(s), all claimed`);
}

async function gateUnitTests(ch) {
  const pk = await readPackages(ch);
  const names = [...new Set(pk.map((p) => p.verify).filter(Boolean))];
  if (names.length === 0) return { pass: true, detail: "no package declares a verification command" };
  const commands = ((await readManifest()).verification || {}).commands || {};
  const problems = [];
  for (const n of names) {
    const cmd = commands[n];
    if (!cmd) {
      problems.push(`verification command "${n}" is not in manifest.yaml`);
      continue;
    }
    try {
      await tools.Bash({ command: cmd });
    } catch (e) {
      problems.push(bashProblem(cmd, e));
    }
  }
  return verdict(problems, `verification passed: ${names.join(", ")}`);
}

const GATES = {
  structure: (ch, station) => gateStructure(ch, station),
  "spec-delta": (ch) => gateSpecDelta(ch),
  constitution: (ch) => gateConstitution(ch),
  ledgers: (ch) => gateLedgers(ch),
  integrated: (ch) => gateIntegrated(ch),
  unclaimed: (ch) => gateUnclaimed(ch),
  "unit-tests": (ch) => gateUnitTests(ch),
};

async function runGate(gate, explicitChange, station) {
  const name = String(gate || "").replace(/^xforge-/, "");
  if (!GATES[name]) throw bad("unknown_gate", `unknown gate ${gate}; the gates are ${Object.keys(GATES).join(", ")}`);
  const ch = await currentChange(explicitChange, { needOpen: false });
  const r = await GATES[name](ch, station);
  return { gate: `xforge-${name}`, change: ch.id, pass: r.pass, detail: r.detail };
}

// ---------- 宿主函数 ----------

async function fnCheck(input, ctx) {
  begin(ctx, input);
  try {
    const r = await runGate(input.gate, null, input.station);
    return { pass: r.pass, detail: r.detail };
  } catch (e) {
    if (e instanceof XError) return { pass: false, detail: e.message };
    throw e;
  }
}

async function fnOpen(input, ctx) {
  begin(ctx, input);
  const inputs = input.inputs || {};
  const title = String(inputs.title || "").trim();
  if (!title) throw new Error("inputs.title is required");
  const risk = inputs.risk || "low";
  if (!RISKS.includes(risk)) throw new Error(`inputs.risk must be one of ${RISKS.join(", ")}`);
  await readManifest();
  for (const c of await openChanges()) if (input.run && c.doc.run === input.run) return { change: c.id };
  const id = await freshId(title);
  const dir = `xforge/changes/${id}`;
  let baseline;
  try {
    const out = (await tools.Bash({ command: "git rev-parse HEAD" })).stdout.trim();
    if (/^[0-9a-f]{40}$/.test(out)) baseline = out;
  } catch (_) {
    // 读不到基线提交不算错
  }
  await writeYaml(`${dir}/delta.yaml`, { added: [], modified: [], removed: [] });
  for (const k of KINDS) await writeLedger({ dir }, k, []);
  for (const [name, text] of Object.entries(TEMPLATES)) await writeText(`${dir}/${name}`, text);
  const doc = { id, title, flow: input.flow || null, risk, status: "open", run: input.run || null };
  if (baseline) doc.baseline_commit = baseline;
  doc.created_at = new Date().toISOString();
  await writeYaml(`${dir}/change.yaml`, doc);
  return { change: id };
}

async function fnPackages(input, ctx) {
  begin(ctx, input);
  const ch = await currentChange(null, { needOpen: false });
  const items = (await readPackages(ch))
    .filter((p) => p.status !== "done")
    .map((p) => ({ package: p.id, title: p.title, paths: p.paths || [], criteria: p.criteria || [] }));
  return { items };
}

async function fnClose(input, ctx) {
  begin(ctx, input);
  const ch = await currentChange(null, { needOpen: false });
  if (ch.doc.status === "archived") return { change: ch.id, specs: [] };
  const check = await gateSpecDelta(ch);
  if (!check.pass) throw new Error(`the delta does not pass: ${check.detail}`);
  const delta = await readDelta(ch);
  const specs = await loadSpecs();
  const touched = new Set();
  const specFor = (cap) => {
    touched.add(cap);
    if (!specs[cap]) specs[cap] = { capability: cap, title: cap, requirements: [], retired: [] };
    if (!specs[cap].retired) specs[cap].retired = [];
    return specs[cap];
  };
  for (const a of delta.added) specFor(a.capability).requirements.push({ id: a.id, title: a.title, text: a.text });
  for (const m of delta.modified) {
    const s = specFor(REQ_RE.exec(m.id)[1]);
    const r = s.requirements.find((x) => x.id === m.id);
    r.text = m.text;
    if (m.title) r.title = m.title;
  }
  for (const id of delta.removed) {
    const s = specFor(REQ_RE.exec(id)[1]);
    s.requirements = s.requirements.filter((x) => x.id !== id);
    if (!s.retired.includes(id)) s.retired.push(id);
  }
  for (const cap of touched) await writeYaml(`xforge/specs/${cap}.yaml`, specs[cap]);

  const archived = { ...ch.doc, status: "archived" };
  const prefix = `${ch.dir}/`;
  for (const f of await listFiles(`${ch.dir}/**/*`)) {
    const dest = `xforge/archive/${ch.id}/${f.slice(prefix.length)}`;
    if (f === `${ch.dir}/change.yaml`) await writeYaml(dest, archived);
    else await writeText(dest, (await readText(f)) ?? "");
  }
  // 没有删文件的工具：留在 changes/ 下的那份只改状态，openChanges 认 status。
  await writeYaml(`${ch.dir}/change.yaml`, archived);
  return { change: ch.id, specs: [...touched].sort() };
}

async function fnCommand(input, ctx) {
  const argv = Array.isArray(input && input.argv) ? input.argv.map(String) : [];
  const name = argv[0];
  if (!COMMANDS[name]) return { ok: false, error: { code: "usage", message: `argv[0] must be one of ${Object.keys(COMMANDS).join(", ")}` } };
  const args = argv.slice(1);
  if (input.change) args.push("--change", String(input.change));
  return runCommand(name, args, input, ctx);
}

// ---------- 命令 ----------

const OPTIONS = {
  change: { type: "string" },
  refs: { type: "string" },
  paths: { type: "string" },
  verify: { type: "string", multiple: true },
  criteria: { type: "string" },
  title: { type: "string" },
  risk: { type: "string" },
  station: { type: "string" },
};

function parse(args) {
  return agix.cli.parseArgs(args, { options: OPTIONS, allowPositionals: true });
}

async function runCommand(name, args, input, ctx) {
  begin(ctx, input);
  try {
    if (args.length === 0 || args[0] === "help") return { ok: true, data: USAGE[name] };
    return { ok: true, data: await COMMANDS[name](args) };
  } catch (e) {
    if (e instanceof XError) return { ok: false, error: { code: e.xcode, message: e.message } };
    if (e && e.name === "AgixError" && String(e.code).startsWith("cli.")) {
      return { ok: false, error: { code: "usage", message: `${e.message}\n${USAGE[name]}` } };
    }
    return { ok: false, error: { code: "failed", message: String((e && e.message) || e) } };
  }
}

// 与 plugin.toml 的 read_only 一致：这些子命令在会话里直接跑。
const READ_ONLY = {
  xflow: ["help", "show", "ledger list", "plan list", "check"],
  xspec: ["help", "list", "get", "delta", "pending", "check"],
  xforge: ["doctor", "status", "start", "help"],
};

function readOnly(name, args) {
  const line = args.join(" ");
  return args.length === 0 || READ_ONLY[name].some((p) => line === p || line.startsWith(`${p} `));
}

// 会改数据的子命令不在会话里写：流程站的会话里派工器要逐次判 Write（会话上有 Write 的 ask 规则），
// 而命令里的子调用问不了人，一写就失败。所以先在会话里认出当前变更（要 codix.flow.current），再交给本包的
// 函数 xforge.command 去做——宿主以不带会话的 plugin.call 调它，写入只限函数声明的 xforge/**。
// `xforge start` 例外：它要以人的会话的身份开运行，留在会话里。
async function commandMain(name, argv, input, ctx) {
  if (readOnly(name, argv)) return runCommand(name, argv, input, ctx);
  begin(ctx, input);
  const args = [...argv];
  if (!args.includes("--change") && !(name === "xforge" && args[0] === "init")) {
    try {
      const ch = await currentChange(null, { needOpen: false });
      args.push("--change", ch.id);
    } catch (_) {
      // 认不出就交给函数自己报
    }
  }
  try {
    return await codix.plugin.call({ function: "xforge.command", input: { argv: [name, ...args] } });
  } catch (e) {
    return { ok: false, error: { code: (e && e.code) || "failed", message: String((e && e.message) || e) } };
  }
}

const xflowMain = (argv, input, ctx) => commandMain("xflow", argv, input, ctx);
const xspecMain = (argv, input, ctx) => commandMain("xspec", argv, input, ctx);
const xforgeMain = (argv, input, ctx) => commandMain("xforge", argv, input, ctx);

const USAGE = {
  xflow: [
    "xflow show [--change ID]",
    "xflow ledger list [decisions|risks|findings]",
    'xflow ledger add <kind> "<text>" [--refs a,b]',
    "xflow ledger set <kind> <id> open|accepted|rejected|resolved|deferred",
    'xflow plan list | plan add <id> "<title>" --paths a,b [--verify NAME] [--criteria "a;b"]',
    'xflow plan set <id> ready|done|blocked ["<summary>"]',
    "xflow check <gate> [--station propose|design|verify]",
    "Every subcommand takes --change ID; without it the change of the current run, or the only open one, is used.",
  ].join("\n"),
  xspec: [
    "xspec list [capability] | get <id> | delta | pending | check",
    'xspec add <capability> "<title>" "<text>"',
    'xspec modify <id> "<text>" [--title T]',
    "xspec remove <id> | revert <id>",
    "Every subcommand takes --change ID.",
  ].join("\n"),
  xforge: [
    "xforge init [--verify NAME=COMMAND ...]",
    "xforge doctor | status",
    'xforge start <quick|solid> "<title>" [--risk low|medium]',
  ].join("\n"),
};

const need = (cond, usage) => {
  if (!cond) throw bad("usage", `usage: ${usage}`);
};

function checkKind(k) {
  if (!KINDS.includes(k)) throw bad("usage", `kind must be one of ${KINDS.join(", ")}`);
  return k;
}

const csv = (s) =>
  String(s || "")
    .split(",")
    .map((x) => x.trim())
    .filter(Boolean);

async function xflowCmd(args) {
  const { values: v, positionals: p } = parse(args);
  const [group, verb] = p;
  if (group === "show") return show(v);
  if (group === "ledger") {
    const ch = await currentChange(v.change, { needOpen: verb !== "list" });
    if (verb === "list") {
      const kinds = p[2] ? [checkKind(p[2])] : KINDS;
      const out = {};
      for (const k of kinds) out[k] = await readLedger(ch, k);
      return out;
    }
    if (verb === "add") {
      need(p[2] && p[3], 'xflow ledger add <kind> "<text>" [--refs a,b]');
      const kind = checkKind(p[2]);
      const entries = await readLedger(ch, kind);
      const n = Math.max(0, ...entries.map((e) => Number(String(e.id).slice(1)) || 0)) + 1;
      const entry = { id: `${KIND_PREFIX[kind]}${n}`, text: p[3], conclusion: "open", refs: csv(v.refs) };
      entries.push(entry);
      await writeLedger(ch, kind, entries);
      return entry;
    }
    if (verb === "set") {
      need(p[2] && p[3] && p[4], "xflow ledger set <kind> <id> <conclusion>");
      const kind = checkKind(p[2]);
      if (!CONCLUSIONS.includes(p[4])) throw bad("usage", `conclusion must be one of ${CONCLUSIONS.join(", ")}`);
      const entries = await readLedger(ch, kind);
      const e = entries.find((x) => x.id === p[3]);
      if (!e) throw bad("no_entry", `no ${kind} entry ${p[3]}`);
      e.conclusion = p[4];
      await writeLedger(ch, kind, entries);
      return e;
    }
  }
  if (group === "plan") {
    const ch = await currentChange(v.change, { needOpen: verb !== "list" });
    const pk = await readPackages(ch);
    if (verb === "list") return { packages: pk };
    if (verb === "add") {
      need(p[2] && p[3] && v.paths, 'xflow plan add <id> "<title>" --paths a,b [--verify NAME] [--criteria "a;b"]');
      if (!ID_RE.test(p[2])) throw bad("usage", `bad package id ${p[2]}`);
      if (pk.some((x) => x.id === p[2])) throw bad("duplicate", `package ${p[2]} exists`);
      const entry = { id: p[2], title: p[3], paths: csv(v.paths) };
      if (v.verify && v.verify[0]) {
        const commands = ((await readManifest()).verification || {}).commands || {};
        if (!commands[v.verify[0]]) throw bad("no_verify", `verification command "${v.verify[0]}" is not in manifest.yaml`);
        entry.verify = v.verify[0];
      }
      entry.criteria = String(v.criteria || "").split(";").map((x) => x.trim()).filter(Boolean);
      entry.status = "ready";
      pk.push(entry);
      await writePackages(ch, pk);
      return entry;
    }
    if (verb === "set") {
      need(p[2] && p[3], 'xflow plan set <id> ready|done|blocked ["<summary>"]');
      if (!STATUSES.includes(p[3])) throw bad("usage", `status must be one of ${STATUSES.join(", ")}`);
      const e = pk.find((x) => x.id === p[2]);
      if (!e) throw bad("no_package", `no package ${p[2]}`);
      e.status = p[3];
      if (p[4]) e.summary = p[4];
      await writePackages(ch, pk);
      return e;
    }
  }
  if (group === "check") {
    need(p[1], "xflow check <gate>");
    const cur = await flowCurrent();
    return runGate(p[1], v.change, v.station || (cur && cur.station) || null);
  }
  throw bad("usage", USAGE.xflow);
}

async function show(v) {
  const ch = await currentChange(v.change, { needOpen: false });
  const docs = {};
  for (const n of Object.keys(TEMPLATES)) docs[n] = isWritten(await readText(`${ch.dir}/${n}`));
  const delta = await readDelta(ch).catch(() => null);
  const ledgers = {};
  for (const k of KINDS) {
    const es = await readLedger(ch, k);
    ledgers[k] = { total: es.length, open: es.filter((e) => e.conclusion === "open").length };
  }
  return {
    change: ch.doc,
    written: docs,
    delta: delta && { added: delta.added.length, modified: delta.modified.length, removed: delta.removed.length },
    packages: (await readPackages(ch)).map((x) => ({ id: x.id, title: x.title, status: x.status })),
    ledgers,
    run: await runSummary(ch.doc.run),
  };
}

async function xspecCmd(args) {
  const { values: v, positionals: p } = parse(args);
  const verb = p[0];
  if (verb === "list") {
    const specs = await loadSpecs();
    if (p[1]) {
      if (!specs[p[1]]) throw bad("no_capability", `no capability ${p[1]}`);
      return specs[p[1]];
    }
    return Object.values(specs).map((s) => ({ capability: s.capability, title: s.title, requirements: s.requirements.length }));
  }
  if (verb === "get") {
    need(p[1], "xspec get <id>");
    const hit = indexReqs(await loadSpecs())[p[1]];
    if (!hit) throw bad("no_requirement", `no requirement ${p[1]}`);
    return { capability: hit.cap, ...hit.req };
  }
  if (verb === "pending") {
    const out = [];
    for (const c of await openChanges()) {
      const d = await readDelta({ dir: `xforge/changes/${c.id}` }).catch(() => null);
      if (d) out.push({ change: c.id, added: d.added.map((x) => x.id), modified: d.modified.map((x) => x.id), removed: d.removed });
    }
    return out;
  }
  const ch = await currentChange(v.change, { needOpen: !["delta", "check"].includes(verb) });
  if (verb === "delta") return readDelta(ch);
  if (verb === "check") return gateSpecDelta(ch);
  const delta = await readDelta(ch);
  const specs = await loadSpecs();
  const idx = indexReqs(specs);
  if (verb === "add") {
    need(p[1] && p[2] && p[3], 'xspec add <capability> "<title>" "<text>"');
    if (!/^[a-z][a-z0-9-]*$/.test(p[1])) throw bad("usage", "a capability is lower-case letters, digits and -");
    const entry = { capability: p[1], id: `${p[1]}/R${nextReqNumber(p[1], specs[p[1]], delta)}`, title: p[2], text: p[3] };
    delta.added.push(entry);
    await writeDelta(ch, delta);
    return entry;
  }
  if (verb === "modify") {
    need(p[1] && p[2], 'xspec modify <id> "<text>" [--title T]');
    const added = delta.added.find((x) => x.id === p[1]);
    if (added) {
      added.text = p[2];
      if (v.title) added.title = v.title;
    } else {
      if (!idx[p[1]]) throw bad("no_requirement", `no requirement ${p[1]}`);
      let m = delta.modified.find((x) => x.id === p[1]);
      if (!m) delta.modified.push((m = { id: p[1] }));
      if (v.title) m.title = v.title;
      m.text = p[2];
      delta.removed = delta.removed.filter((x) => x !== p[1]);
    }
    await writeDelta(ch, delta);
    return { id: p[1] };
  }
  if (verb === "remove") {
    need(p[1], "xspec remove <id>");
    if (!idx[p[1]]) throw bad("no_requirement", `no requirement ${p[1]} in specs (an added one is taken back with revert)`);
    delta.modified = delta.modified.filter((x) => x.id !== p[1]);
    if (!delta.removed.includes(p[1])) delta.removed.push(p[1]);
    await writeDelta(ch, delta);
    return { id: p[1] };
  }
  if (verb === "revert") {
    need(p[1], "xspec revert <id>");
    const n = delta.added.length + delta.modified.length + delta.removed.length;
    delta.added = delta.added.filter((x) => x.id !== p[1]);
    delta.modified = delta.modified.filter((x) => x.id !== p[1]);
    delta.removed = delta.removed.filter((x) => x !== p[1]);
    if (delta.added.length + delta.modified.length + delta.removed.length === n) throw bad("not_in_delta", `${p[1]} is not in the delta`);
    await writeDelta(ch, delta);
    return { id: p[1] };
  }
  throw bad("usage", USAGE.xspec);
}

async function xforgeCmd(args) {
  const { values: v, positionals: p } = parse(args);
  const verb = p[0];
  if (verb === "init") {
    const commands = {};
    for (const kv of v.verify || []) {
      const i = kv.indexOf("=");
      need(i > 0, "xforge init --verify NAME=COMMAND");
      commands[kv.slice(0, i)] = kv.slice(i + 1);
    }
    const had = await readText("xforge/manifest.yaml");
    if (had === null) await writeYaml("xforge/manifest.yaml", { version: 1, verification: { commands } });
    if ((await readText("xforge/constitution.md")) === null) {
      await writeText("xforge/constitution.md", "# 原则\n\n<!-- 一行一条：- **P1** 原则正文。编号 P<n>；有原则时 design.md 要逐条提到。 -->\n");
    }
    return { initialised: had === null, manifest: "xforge/manifest.yaml" };
  }
  if (verb === "doctor") return doctor();
  if (verb === "status") {
    const out = [];
    for (const c of await openChanges()) out.push({ change: c.id, title: c.doc.title, flow: c.doc.flow, risk: c.doc.risk, run: c.doc.run, run_state: await runSummary(c.doc.run) });
    return out;
  }
  if (verb === "start") {
    need(["quick", "solid"].includes(p[1]) && p[2], 'xforge start <quick|solid> "<title>" [--risk low|medium]');
    const allowed = p[1] === "quick" ? ["low"] : RISKS;
    const risk = v.risk || (p[1] === "quick" ? "low" : "medium");
    if (!allowed.includes(risk)) throw bad("risk", `xforge-${p[1]} takes risk ${allowed.join(" or ")}, not ${risk}: use another flow`);
    await readManifest();
    try {
      const r = await codix.flow.start({ flow: `xforge-${p[1]}`, subject: p[2], inputs: { title: p[2], risk } });
      return { run: r.run, flow: `xforge-${p[1]}`, risk };
    } catch (e) {
      throw bad(e.code || "start_failed", e.message);
    }
  }
  throw bad("usage", USAGE.xforge);
}

async function doctor() {
  const problems = [];
  const note = (path, message) => problems.push({ path, message });
  const trying = async (path, f) => {
    try {
      return await f();
    } catch (e) {
      note(path, e.message);
      return null;
    }
  };
  const m = await trying("xforge/manifest.yaml", readManifest);
  if (m && m.version !== 1) note("xforge/manifest.yaml", `version must be 1, found ${m.version}`);
  const specs = await trying("xforge/specs", loadSpecs);
  for (const s of Object.values(specs || {})) {
    for (const r of s.requirements) {
      const mm = REQ_RE.exec(r.id || "");
      if (!mm || mm[1] !== s.capability) note(`xforge/specs/${s.capability}.yaml`, `requirement id ${r.id} does not fit the capability`);
      else if (!r.title || !r.text) note(`xforge/specs/${s.capability}.yaml`, `${r.id} needs title and text`);
    }
  }
  for (const f of await listFiles("xforge/changes/*/change.yaml")) {
    const id = f.split("/")[2];
    const ch = await trying(f, () => loadChange(id));
    if (!ch) continue;
    if (!["open", "archived"].includes(ch.doc.status)) note(f, `status must be open or archived, found ${ch.doc.status}`);
    await trying(`${ch.dir}/delta.yaml`, () => readDelta(ch));
    await trying(`${ch.dir}/work-packages.yaml`, async () => {
      for (const p of await readPackages(ch)) {
        if (!p.id || !STATUSES.includes(p.status)) note(`${ch.dir}/work-packages.yaml`, `package ${p.id}: bad status ${p.status}`);
      }
    });
    for (const k of KINDS) {
      await trying(`${ch.dir}/ledgers/${k}.yaml`, async () => {
        for (const e of await readLedger(ch, k)) if (!CONCLUSIONS.includes(e.conclusion)) note(`${ch.dir}/ledgers/${k}.yaml`, `${e.id}: bad conclusion ${e.conclusion}`);
      });
    }
  }
  return { ok: problems.length === 0, problems };
}

const COMMANDS = { xflow: xflowCmd, xspec: xspecCmd, xforge: xforgeCmd };
