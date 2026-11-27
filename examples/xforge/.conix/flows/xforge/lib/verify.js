// 工作包的验证（docs/design/xforge.md §1、§4）：治理文件里人声明的 verify 菜单，工作包只按名字引用、给参数。
// 命令 xpkg 划包时用它核对 --verify，门禁 package-tests 跑之前再核对一次（文件可能被改过）、展开成 argv。
// argv 交给流程库直接起进程，不经 shell：参数值要整串匹配菜单里的正则，模型写进工作包的东西只能落在人留的空里。

const PLACEHOLDER = /\{([A-Za-z_][A-Za-z0-9_]*)\}/g;
const NAME = /^[a-z0-9][a-z0-9-]*$/;
const FIELDS = ["argv", "params", "cwd", "description", "writable", "network"];

const placeholders = (arg) => [...arg.matchAll(PLACEHOLDER)].map((m) => m[1]);
const anchored = (re) => new RegExp(`^(?:${re})$`);

// 菜单本身的问题：[message]。没有菜单（null）不算问题。
export function menuProblems(menu) {
  if (menu === undefined || menu === null) return [];
  if (typeof menu !== "object" || Array.isArray(menu)) return ["verify 要写成 {<名字>: {argv: [...], params?, cwd?}}"];
  const out = [];
  for (const [name, m] of Object.entries(menu)) {
    const at = `verify.${name}`;
    if (!NAME.test(name)) out.push(`${at}：名字是小写字母、数字、连字符`);
    if (!m || typeof m !== "object" || Array.isArray(m)) {
      out.push(`${at} 要写成 {argv: [...]}`);
      continue;
    }
    const extra = Object.keys(m).filter((k) => !FIELDS.includes(k));
    if (extra.length) out.push(`${at} 没有字段 ${extra.join(", ")}（有 ${FIELDS.join(", ")}）`);
    if (!Array.isArray(m.argv) || !m.argv.length || m.argv.some((a) => typeof a !== "string" || !a) || !m.argv[0].trim()) {
      out.push(`${at}.argv 要是非空字符串的数组：程序与它的参数，一项一个，不经 shell`);
      continue;
    }
    const params = m.params === undefined ? {} : m.params;
    if (!params || typeof params !== "object" || Array.isArray(params)) {
      out.push(`${at}.params 要写成 {<参数>: "<正则>"}`);
      continue;
    }
    for (const [p, re] of Object.entries(params)) {
      if (typeof re !== "string" || !re) {
        out.push(`${at}.params.${p} 要是一个正则（字符串）`);
        continue;
      }
      try {
        anchored(re);
      } catch (e) {
        out.push(`${at}.params.${p} 不是正则：${e.message}`);
      }
    }
    const used = new Set(m.argv.flatMap(placeholders));
    for (const p of used) if (!(p in params)) out.push(`${at}.argv 里的 {${p}} 在 params 里没有声明`);
    for (const p of Object.keys(params)) if (!used.has(p)) out.push(`${at}.params.${p} 在 argv 里没用到`);
    if (m.cwd !== undefined && (typeof m.cwd !== "string" || m.cwd.startsWith("/") || m.cwd.split("/").includes(".."))) {
      out.push(`${at}.cwd 要是项目内的相对目录`);
    }
    for (const p of writableProblems(m.writable)) out.push(`${at}.writable ${p}`);
    if (m.network !== undefined && typeof m.network !== "boolean") out.push(`${at}.network 要是 true 或 false`);
  }
  return out;
}

// 门禁的进程能写的目录（verify 菜单的一项、治理文件的 checks）：项目内的相对目录，每个都要被 git 忽略——门禁判活不干活，
// 能写的只有构建与缓存（target/、node_modules/.cache），流程库起进程前再查一次忽略。没写是 []。
export function writableProblems(w) {
  if (w === undefined || w === null) return [];
  if (!Array.isArray(w)) return ["要写成目录的列表，如 [target]"];
  return w.flatMap((d) => (typeof d !== "string" || !d.trim() || d.startsWith("/") || d.split("/").includes("..") || d.split("/")[0] === ".git"
    ? [`里的 ${JSON.stringify(d)} 要是项目内的相对目录（不是 .git）`]
    : []));
}

// 两路合起来时跑的那一项（xforge/solid、xforge/major 的门禁 blackbox）：菜单里名叫 blackbox，不带参数。
export const BLACKBOX = "blackbox";

// 黑盒那一项能不能拿来跑：null 是能；"missing" 是菜单里没有，"params" 是它带了参数（合起来时没有谁来填）。
// 菜单本身的毛病归 menuProblems。门禁 blackbox 与自检 doctor 共用这一份，自检说能跑的，门禁不会说不能。
export function blackboxProblem(menu) {
  if (!menu || typeof menu !== "object" || !Object.prototype.hasOwnProperty.call(menu, BLACKBOX)) return "missing";
  const m = menu[BLACKBOX];
  return m && typeof m === "object" && m.params && typeof m.params === "object" && Object.keys(m.params).length ? "params" : null;
}

// --verify 的写法 <名字>[:<参数>=<值>[,<参数>=<值>]] → {name, params}。
export function parseEntry(text) {
  const s = String(text || "").trim();
  const at = s.indexOf(":");
  const name = at < 0 ? s : s.slice(0, at);
  if (!NAME.test(name)) throw new Error(`--verify ${s}：写成 <名字>[:参数=值,参数=值]，名字来自 verify 菜单（xpkg verify-menu 看）`);
  const params = {};
  for (const part of at < 0 ? [] : s.slice(at + 1).split(",")) {
    const eq = part.indexOf("=");
    const k = eq < 0 ? "" : part.slice(0, eq).trim();
    if (!k) throw new Error(`--verify ${s}：参数写成 参数=值，这一段不是：${part}`);
    if (k in params) throw new Error(`--verify ${s}：参数 ${k} 给了两次`);
    params[k] = part.slice(eq + 1);
  }
  return { name, params };
}

// 一项验证给人看的名字：unit、test:file=test_x。
export function label(entry) {
  const ps = Object.entries(entry.params || {}).map(([k, v]) => `${k}=${v}`);
  return ps.length ? `${entry.name}:${ps.join(",")}` : entry.name;
}

// 一项验证（{name, params}）按菜单展开成 {argv, cwd?}；对不上就抛错，说怎么写对。
export function expand(menu, entry) {
  const names = Object.keys(menu || {});
  const m = menu && Object.prototype.hasOwnProperty.call(menu, entry.name) ? menu[entry.name] : null;
  if (!m) throw new Error(`verify 菜单里没有 ${entry.name}：有 ${names.join(", ") || "（空）"}（xpkg verify-menu 看）`);
  const declared = m.params || {};
  const given = entry.params || {};
  const usage = `--verify ${entry.name}${Object.keys(declared).length ? `:${Object.keys(declared).map((k) => `${k}=…`).join(",")}` : ""}`;
  const missing = Object.keys(declared).filter((p) => !(p in given));
  if (missing.length) throw new Error(`验证 ${entry.name} 要参数 ${missing.join(", ")}：${usage}`);
  const extra = Object.keys(given).filter((p) => !(p in declared));
  if (extra.length) throw new Error(`验证 ${entry.name} 没有参数 ${extra.join(", ")}：${usage}`);
  for (const [p, re] of Object.entries(declared)) {
    const v = String(given[p]);
    if (!anchored(re).test(v)) throw new Error(`验证 ${entry.name} 的参数 ${p}=${v} 不合菜单的 ${re}（整串匹配）`);
  }
  const argv = m.argv.map((a) => a.replace(PLACEHOLDER, (_, p) => String(given[p])));
  return {
    argv,
    ...(m.cwd ? { cwd: m.cwd } : {}),
    ...(m.writable && m.writable.length ? { writable: m.writable } : {}),
    ...(typeof m.network === "boolean" ? { network: m.network } : {}),
  };
}

// 给人和模型看的菜单：每项一行命令，有参数的另起一行写参数与用法。
export function describeMenu(menu) {
  return Object.entries(menu || {}).map(([name, m]) => {
    const cmd = m.argv.map((a) => (/^[\w@%+=:,./{}-]+$/.test(a) ? a : JSON.stringify(a))).join(" ");
    const params = Object.entries(m.params || {});
    const access = [m.writable && m.writable.length ? `可写 ${m.writable.join("、")}` : "", m.network ? "联网" : ""].filter(Boolean).join("，");
    return `${name}：${cmd}${m.cwd ? `（在 ${m.cwd}）` : ""}${access ? `（${access}）` : ""}${m.description ? `\n  ${m.description}` : ""}`
      + (params.length
        ? `\n  参数：${params.map(([k, re]) => `${k} 整串匹配 ${re}`).join("；")}\n  用法：--verify ${name}:${params.map(([k]) => `${k}=…`).join(",")}`
        : `\n  用法：--verify ${name}`);
  }).join("\n");
}
