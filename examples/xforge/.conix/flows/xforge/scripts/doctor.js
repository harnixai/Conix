// doctor：这个项目给 xforge 备齐了没有（docs/design/xforge.md §7；流程库的自检，flows.md §7.13）。宿主的 conix flow doctor
// 在运行之外调它。只读：每条发现说哪里不对、人该怎么改（fix），自己什么都不改。
// 判法用命令与门禁用的那几份（lib/）：自检说菜单没问题，xpkg plan --verify 与门禁 package-tests、blackbox 就不会说有。
//
//   error  照现在的样子会被拒：启动拒绝、门禁必然 error、--verify 一项都选不了
//   warn   某条路径或某道门禁会缺东西，别的照跑
//   info   知道就好：第一次运行会建的
import { governance, verifyMenu } from "../lib/governance.js";
import { readBaseline } from "../lib/arch.js";
import { readSpecs } from "../lib/specs.js";
import { BLACKBOX, blackboxProblem, menuProblems, writableProblems } from "../lib/verify.js";
import { LINES, REVIEWED, STAGES, reviewLineProblem } from "../lib/lines.js";

// 与 bundle.yaml 的 settings.file、门禁 constitution 与 unit-tests 写的路径是同一份。
const SETTINGS = ".conix/governance/xforge.yaml";
const CONSTITUTION = ".conix/governance/constitution.yaml";
const VERIFY_SH = ".conix/governance/verify.sh";

// 治理文件里每个开关该是什么类型。review.separate 不是布尔值时启动就拒（确认出口的 separate）；
// governance 的两个按"spec 不写 false 就开、interface 写 true 才开"解释，写成 "no" 不报错却意思反了，所以一样算错；
// 检视点写成 "yes" 时站的 when 判它不等于 true，那一处就悄悄不检视了。
const SWITCHES = { governance: ["spec", "interface"], review: ["separate"], "review.solid": STAGES, "review.major": STAGES };
const TOP = ["governance", "review", "lines", "checks", "verify"];

export default function (input, flow) {
  const out = [];
  const add = (level, message, path, fix) => out.push({ level, message, ...(path ? { path } : {}), ...(fix ? { fix } : {}) });
  const settingsOk = checkSettings(flow, add);
  if (settingsOk) {
    checkLines(flow, add);
    checkChecks(flow, add);
    checkMenu(flow, add);
    checkBaselines(flow, input.data_dir, add);
  }
  checkConstitution(flow, add);
  if (!flow.stat(VERIFY_SH)) {
    add("warn", `没有 ${VERIFY_SH}：每条路径的实施站、验证站的门禁 unit-tests 跑它，没有就不过`, VERIFY_SH, "写一份 sh 脚本跑项目的单测（与 lint），退出码 0 算过");
  }
  if (!flow.git.head()) {
    add("warn", "项目不是 git 仓库（或还没有提交）：门禁 unclaimed、package-check 对 git 的账，判不了改了什么", null, "git init 并提交一次");
  }
  return { findings: out };
}

// 文件读得成、开关的类型对。读不成就不往下查菜单与基线：它们在同一个文件里。
function checkSettings(flow, add) {
  let s;
  try {
    s = flow.settings;
  } catch (e) {
    add("error", `治理文件读不成，xforge 的每条流程都会拒绝启动：${e.message}`, SETTINGS, "改成 YAML 映射：governance、review、lines、checks、verify（docs/design/xforge.md §1）");
    return false;
  }
  let ok = true;
  for (const k of Object.keys(s)) {
    if (!TOP.includes(k)) add("warn", `治理文件里的 ${k} xforge 不认识（有 ${TOP.join("、")}），写错了名字？`, SETTINGS, `删掉或改成 ${TOP.join("、")} 之一`);
  }
  for (const [group, keys] of Object.entries(SWITCHES)) {
    const g = group.split(".").reduce((v, k) => (v && typeof v === "object" ? v[k] : undefined), s);
    if (g === undefined || g === null) continue;
    if (typeof g !== "object" || Array.isArray(g)) {
      add("error", `${group} 要写成映射（${keys.map((k) => `${k}: true|false`).join("，")}），现在是 ${JSON.stringify(g)}`, SETTINGS, `${group}: {${keys.map((k) => `${k}: false`).join(", ")}}`);
      ok = false;
      continue;
    }
    const nested = group === "review" ? REVIEWED : [];
    for (const k of Object.keys(g)) {
      if (!keys.includes(k) && !nested.includes(k)) add("warn", `${group}.${k} xforge 不认识（有 ${keys.concat(nested).join("、")}），写错了名字？`, SETTINGS, `删掉或改成 ${keys.concat(nested).join("、")} 之一`);
    }
    for (const k of keys) {
      if (g[k] !== undefined && typeof g[k] !== "boolean") {
        add("error", `${group}.${k} 要是 true 或 false，现在是 ${JSON.stringify(g[k])}`, SETTINGS, `${group}.${k}: true（或 false），不带引号`);
        ok = false;
      }
    }
  }
  return ok;
}

// lines：每条线一个模型（字符串）；开着检视点的路径，检视线要与开发线不同（门禁 lines 同一份判法）。
function checkLines(flow, add) {
  const s = flow.settings || {};
  const l = s.lines;
  if (l !== undefined && l !== null) {
    if (typeof l !== "object" || Array.isArray(l)) {
      add("error", "lines 要写成映射：{build: {model}, tests: {model}, review: {model}}", SETTINGS);
      return;
    }
    for (const [k, v] of Object.entries(l)) {
      if (!LINES.includes(k)) {
        add("warn", `lines.${k} xforge 不认识（有 ${LINES.join("、")}），写错了名字？`, SETTINGS, `删掉或改成 ${LINES.join("、")} 之一`);
        continue;
      }
      if (!v || typeof v !== "object" || Array.isArray(v)) {
        add("error", `lines.${k} 要写成 {model: <模型>, provider: <可选>}`, SETTINGS);
        continue;
      }
      for (const f of ["model", "provider"]) {
        if (v[f] !== undefined && (typeof v[f] !== "string" || !v[f].trim())) add("error", `lines.${k}.${f} 要是一个名字`, SETTINGS);
      }
    }
  }
  for (const path of REVIEWED) {
    const p = reviewLineProblem(s, path);
    if (p) add("error", `${p}：xforge/${path} 第一站（核对三条线）就会停下`, SETTINGS, `lines.review.model 写一个与开发线不同的模型，或关掉 review.${path} 的检视点`);
  }
}

// checks：项目的验证（门禁 unit-tests）能写的目录与联网，与菜单的一项同一份核对。被不被 git 忽略由流程库起进程前查。
function checkChecks(flow, add) {
  const c = (flow.settings || {}).checks;
  if (c === undefined || c === null) return;
  if (typeof c !== "object" || Array.isArray(c)) {
    add("error", "checks 要写成 {writable: [...], network: false}", SETTINGS);
    return;
  }
  for (const p of writableProblems(c.writable)) add("error", `checks.writable ${p}`, SETTINGS, "写项目内被 git 忽略的目录，如 [target]");
  if (c.network !== undefined && typeof c.network !== "boolean") add("error", "checks.network 要是 true 或 false", SETTINGS);
}

// verify 菜单：与 xpkg plan --verify、门禁 package-tests 同一份核对；黑盒一项与门禁 blackbox 同一份判法。
function checkMenu(flow, add) {
  const menu = verifyMenu(flow);
  if (!menu) {
    add("info", "治理文件里没有 verify 菜单：工作包不带 --verify，交付时只核对登记", SETTINGS);
  } else {
    for (const p of menuProblems(menu)) {
      add("error", `verify 菜单：${p}（xpkg plan --verify 与门禁 package-tests 都会拒绝）`, SETTINGS, "照 docs/design/xforge.md §1 的写法改，改完 /flow:xforge:xpkg verify-menu 看");
    }
  }
  const bb = blackboxProblem(menu);
  if (bb === "missing") {
    add("warn", `xforge/solid、xforge/major 需要 verify.${BLACKBOX}：合起来那一站的门禁 blackbox 用它跑测试线写的黑盒测试，没有就是 error；quick 不用它`, SETTINGS,
      `在 verify 下加一项 ${BLACKBOX}: {argv: [python3, -m, unittest, discover, -s, tests_blackbox, -q]}（黑盒测试放哪、怎么跑，不带参数）`);
  } else if (bb === "params") {
    add("error", `verify.${BLACKBOX} 不能带参数：两路合起来时跑的是整套黑盒测试，没有谁来填参数，门禁 blackbox 会 error`, SETTINGS, `去掉 verify.${BLACKBOX} 的 params 与 argv 里的 {…}`);
  }
}

// 开着的基线：读得成；还是空的只提一句。基线只由命令写（xspec init、归档），第一次运行会建。
function checkBaselines(flow, dataDir, add) {
  if (!dataDir) {
    add("info", "宿主没有给包的数据目录：基线没查");
    return;
  }
  const g = governance(flow);
  const specs = `${dataDir}/specs`;
  if (g.spec) {
    try {
      if (!readSpecs(flow, `${specs}/*.md`).files.size) {
        add("info", `规格基线还是空的（${specs}/）：第一次变更的提案站用 xspec init <能力> <前缀> "<标题>" 建一份，归档时并进去`, specs);
      }
    } catch (e) {
      add("error", `规格基线读不成：${e.message}`, specs, "修好那个文件：规格只由 xspec 与归档写");
    }
  }
  const ifaces = `${dataDir}/interfaces`;
  if (g.interface) {
    try {
      if (!readBaseline(flow, `${ifaces}/*.yaml`).modules.size) {
        add("info", `接口基线还是空的（${ifaces}/）：变更里 xarch module <模块> 登记模块，归档时建 ${ifaces}/<模块>.yaml`, ifaces);
      }
    } catch (e) {
      add("error", `接口基线读不成：${e.message}`, ifaces, "修好那个文件：接口基线只由 xarch 归档写");
    }
  }
}

// 原则：门禁 constitution 与 constitution-reply 按 P<n> 的编号逐条对照，认不出的编号永远答不上。
function checkConstitution(flow, add) {
  let doc;
  try {
    doc = flow.read.yaml(CONSTITUTION);
  } catch (e) {
    add("error", `原则文件读不成，设计站的门禁 constitution 会 error：${e.message}`, CONSTITUTION, "改成 principles: [{id: P1, title, text}]");
    return;
  }
  if (doc == null) {
    add("warn", `没有原则文件 ${CONSTITUTION}：设计站与检视站没有可对照的（门禁只提醒、不挡）`, CONSTITUTION, "写一份 principles: [{id: P1, title, text}]，一条一个项目原则");
    return;
  }
  const ps = doc && doc.principles;
  if (!Array.isArray(ps) || !ps.length) {
    add("error", `${CONSTITUTION} 里没有原则（principles 是空的或不是列表）：设计站的门禁 constitution 会 error`, CONSTITUTION, "principles: [{id: P1, title, text}]，或删掉这个文件");
    return;
  }
  const seen = new Set();
  ps.forEach((p, i) => {
    const id = p && p.id;
    if (typeof id !== "string" || !/^P\d+$/.test(id)) {
      add("error", `principles[${i}] 的编号 ${JSON.stringify(id)} 不是 P<数字>：设计与检查对照不上它`, CONSTITUTION, "编号写成 P1、P2…");
    } else if (seen.has(id)) {
      add("error", `原则编号 ${id} 用了两次`, CONSTITUTION, "每条原则一个编号");
    } else {
      seen.add(id);
    }
    if (p && typeof p === "object" && !String(p.title || "").trim()) {
      add("warn", `原则 ${id || `principles[${i}]`} 没有 title：门禁报"没有对照"时只能报编号`, CONSTITUTION);
    }
  });
}
