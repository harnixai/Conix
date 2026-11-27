// 进 git 的 argv 之前，每个值先在这里核对：命令（collect）与自检（doctor）共用一份判法。
// 没有 shell，参数里的 ; | $() 本来就只是字符；要防的是 git 自己把值读成选项（- 开头）、范围（..）、
// 仓库外的路径（..、绝对路径）或 pathspec 的魔法写法（: 开头）。

// 不满足就抛错：命令抛错就是这次调用失败，信息给调用的人或模型看。
export function need(v, why) {
  if (!v) throw new Error(why);
  return v;
}

const DAY = 86400000;

// YYYY-MM-DD，且是日历上有的那一天（2026-02-30 不算）。
export function checkDate(s, what) {
  need(typeof s === "string" && /^\d{4}-\d{2}-\d{2}$/.test(s), `${what} 要写成 YYYY-MM-DD，现在是 ${JSON.stringify(s)}`);
  const [y, m, d] = s.split("-").map(Number);
  const t = new Date(Date.UTC(y, m - 1, d));
  need(t.getUTCFullYear() === y && t.getUTCMonth() === m - 1 && t.getUTCDate() === d, `${what} ${s} 不是日历上的一天`);
  return s;
}

// 一个提交的写法：分支、标签、HEAD、sha，可以带 ~n、^n。不收 ..（那是范围，这份报告只看一条线上的历史）、
// @{…}（reflog，只在本机有，换台机器就不是同一份报告）、- 开头（git 会当成选项）。
export function checkRev(s) {
  need(typeof s === "string" && s.length > 0 && s.length <= 200, "rev 要是 1 到 200 个字符");
  need(!s.startsWith("-"), `rev 不能以 - 开头：${JSON.stringify(s)}`);
  need(/^[A-Za-z0-9_][A-Za-z0-9._\/~^-]*$/.test(s), `rev 只收分支、标签、sha 的写法（字母、数字、. _ / - ~ ^）：${JSON.stringify(s)}`);
  need(!s.includes("..") && !s.includes("//") && !s.endsWith("/") && !s.endsWith(".") && !s.endsWith(".lock"), `rev 的写法不对：${JSON.stringify(s)}`);
  return s;
}

// 仓库里的一个路径（目录或文件，可以带 git 的通配 * ?）：相对项目根、不出仓库。
export function checkPath(p) {
  need(typeof p === "string" && p.length > 0 && p.length <= 1024, "路径要是 1 到 1024 个字符");
  need(!/[\0\n\r\\]/.test(p), `路径里不能有换行、反斜杠：${JSON.stringify(p)}`);
  need(!p.startsWith("/"), `路径要相对项目根：${p}`);
  need(!p.startsWith("-"), `路径不能以 - 开头：${p}`);
  // :(top)、:!x 是 pathspec 的魔法：能指到别处、能反选，报告的范围就不是人写下的那几个路径了。
  need(!p.startsWith(":"), `路径不能以 : 开头（git 的 pathspec 魔法）：${p}`);
  need(!p.split("/").includes(".."), `路径不能有 ..：${p}`);
  return p.replace(/\/+$/, "") || ".";
}

// 设置里的正整数：类型不对就用不成，不悄悄回落到缺省。
export function checkCount(v, what, max) {
  need(Number.isInteger(v) && v > 0 && v <= max, `${what} 要是 1 到 ${max} 的整数，现在是 ${JSON.stringify(v)}`);
  return v;
}

// 设置里的开关。
export function checkSwitch(v, what) {
  need(typeof v === "boolean", `${what} 要是 true 或 false（不带引号），现在是 ${JSON.stringify(v)}`);
  return v;
}

// 设置里每一项该是什么：与 bundle.yaml 的 settings.defaults 同一套名字。doctor 照它报，collect 照它拒。
export const SETTINGS = {
  "range.days": (v) => checkCount(v, "range.days", 3660),
  "privacy.subjects": (v) => checkSwitch(v, "privacy.subjects"),
  "privacy.emails": (v) => checkSwitch(v, "privacy.emails"),
  "limits.commits": (v) => checkCount(v, "limits.commits", 100000),
  "limits.listed": (v) => checkCount(v, "limits.listed", 5000),
  "limits.files": (v) => checkCount(v, "limits.files", 500),
  "review.separate": (v) => checkSwitch(v, "review.separate"),
};

export function pick(obj, dotted) {
  return dotted.split(".").reduce((o, k) => (o == null ? undefined : o[k]), obj);
}

// 设置的每一项都核对过的那份；不对就抛错，说哪一项。
export function settingsOf(s) {
  const out = {};
  for (const [k, check] of Object.entries(SETTINGS)) out[k] = check(pick(s, k));
  return out;
}

// 本机今天的日期。截止日缺省是"今天"，人说的今天是他这台机器的。
export function today() {
  const d = new Date();
  const p = (n) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}`;
}

// until 往前 days 天（含 until 那天共 days 天）。
export function daysBefore(until, days) {
  const [y, m, d] = until.split("-").map(Number);
  return new Date(Date.UTC(y, m - 1, d) - (days - 1) * DAY).toISOString().slice(0, 10);
}

// 选项：--k v 与 --k=v 两种写法；能写几次的（--path）收成列表。认不出的选项报错，不忽略：
// 写错一个名字悄悄按缺省统计，报告就不是人要的那份。
export function parseArgs(args, known, many) {
  const opt = {};
  for (const m of many) opt[m] = [];
  const pos = [];
  for (let i = 0; i < args.length; i++) {
    const a = args[i];
    if (!a.startsWith("--")) {
      pos.push(a);
      continue;
    }
    const eq = a.indexOf("=");
    const k = eq < 0 ? a.slice(2) : a.slice(2, eq);
    need(known.includes(k) || many.includes(k), `不认识的选项 --${k}（有 ${[...known, ...many].map((x) => `--${x}`).join("、")}）`);
    let v;
    if (eq >= 0) v = a.slice(eq + 1);
    else {
      need(i + 1 < args.length, `--${k} 后面要有值`);
      v = args[++i];
    }
    if (many.includes(k)) opt[k].push(v);
    else opt[k] = v;
  }
  return { pos, opt };
}
