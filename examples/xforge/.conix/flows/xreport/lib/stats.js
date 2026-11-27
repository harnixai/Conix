// stats.json：从 git log 的提交算出的全部数。分析站的模型只读它，定稿的表只从它生成；它的形状由门禁 stats-shape 守。
import { format, git, ok, parseLog } from "./git.js";

export const FORMAT = 1;
export const WEEKDAYS = ["周一", "周二", "周三", "周四", "周五", "周六", "周日"];
const BATCH = 2000;

// 范围的 git 选项。since/until 是本机时区的那一天的开始与结束，与 git log --since/--until 一样按提交时间（committer date）筛：
// 筛的是 git，不在这里重写一遍日期的规矩；时间分布另按作者时间算（见 bucket）。
function rangeArgs(q) {
  return [`--since=${q.since} 00:00:00`, `--until=${q.until} 23:59:59`];
}

// rev 之前放 --end-of-options：即使核对漏了，git 也不会把它读成选项。路径都在 -- 之后。
function tail(q) {
  return ["--end-of-options", q.rev, "--", ...q.paths];
}

// rev 指的提交；找不到就报出来，而不是统计出一份空报告。
export function resolve(flow, rev) {
  const r = git(flow, ["rev-parse", "--verify", "--quiet", "--end-of-options", `${rev}^{commit}`]);
  if (r.code !== 0) throw new Error(`rev ${rev} 在这个仓库里找不到提交（git rev-parse 退出码 ${r.code}）`);
  return r.stdout.trim();
}

// 不含合并的提交与它们的 numstat。一次取一批；一批的输出超过 flow.exec 的上限（4 MiB）就对半再取，
// 取到只剩一个提交还超就收下截断的那份、记一笔：一个提交改了几十万个文件，少数几个文件的数不全，不值得整份报告不出。
function commitsOf(flow, q, cap, emails) {
  const out = [];
  let partial = 0;
  let size = BATCH;
  while (out.length <= cap) {
    const n = Math.min(size, cap + 1 - out.length);
    const r = git(flow, [
      "log", "--no-merges", "--numstat", "-z", "-M", "--no-color", "--no-ext-diff", "--no-textconv", "--encoding=UTF-8",
      format(emails), ...rangeArgs(q), `--skip=${out.length}`, "-n", String(n), ...tail(q),
    ]);
    if (r.truncated && n > 1) {
      size = Math.max(1, Math.floor(n / 2));
      continue;
    }
    const got = parseLog(ok(r, "git log"), emails);
    if (r.truncated) {
      partial++;
      got.splice(1);
    }
    out.push(...got);
    if (got.length < n) break;
    size = BATCH;
  }
  return { commits: out, partial };
}

// 合并提交另数：它们的 numstat 是两边之差，算进行数就把别人的改动算到合并的人头上。
function mergesOf(flow, q, cap, emails) {
  const r = git(flow, ["log", "--merges", "-z", "--no-color", "--encoding=UTF-8", format(emails), ...rangeArgs(q), "-n", String(cap + 1), ...tail(q)]);
  return parseLog(ok(r, "git log --merges"), emails);
}

// 作者自己的时区里的星期、小时、日期（%aI：2026-11-02T09:15:00+08:00）：报告看的是人在他那边几点干活，不是本机几点。
export function bucket(at) {
  const m = /^(\d{4})-(\d{2})-(\d{2})T(\d{2})/.exec(at || "");
  if (!m) return null;
  const day = new Date(Date.UTC(Number(m[1]), Number(m[2]) - 1, Number(m[3]))).getUTCDay();
  return { date: `${m[1]}-${m[2]}-${m[3]}`, weekday: (day + 6) % 7, hour: Number(m[4]) };
}

function top(path) {
  const i = path.indexOf("/");
  return i < 0 ? "." : path.slice(0, i);
}

const churn = (x) => x.insertions + x.deletions;
const byChurn = (a, b) => churn(b) - churn(a) || b.commits - a.commits || (a.key < b.key ? -1 : a.key > b.key ? 1 : 0);

// 全部数。q = {since, until, rev, paths}；s 是核对过的设置（args.settingsOf）。
export function collect(flow, q, s) {
  const emails = s["privacy.emails"], subjects = s["privacy.subjects"], cap = s["limits.commits"];
  const head = resolve(flow, q.rev);
  const plain = commitsOf(flow, q, cap, emails);
  const merges = mergesOf(flow, q, cap, emails);
  const truncated = plain.commits.length > cap || merges.length > cap;
  const commits = plain.commits.slice(0, cap);
  const mergeList = merges.slice(0, cap);

  const authors = new Map();
  const author = (c) => {
    // 名字照 git（%aN，.mailmap 合过）：同一个名字是同一个人；开了邮箱时名字加邮箱才是同一个人。
    const key = emails ? `${c.author}\x00${c.email}` : c.author;
    if (!authors.has(key)) {
      authors.set(key, { name: c.author, ...(emails ? { email: c.email } : {}), commits: 0, merges: 0, insertions: 0, deletions: 0, files: new Set(), days: new Set(), first: null, last: null });
    }
    return authors.get(key);
  };
  const seen = (a, c) => {
    if (!a.first || c.t < a.first.t) a.first = c;
    if (!a.last || c.t > a.last.t) a.last = c;
  };
  const weekday = WEEKDAYS.map((day) => ({ day, commits: 0, insertions: 0, deletions: 0 }));
  const hour = Array.from({ length: 24 }, (_, h) => ({ hour: h, commits: 0 }));
  const days = new Map();
  const dirs = new Map();
  const files = new Map();
  const all = new Set();
  let ins = 0, del = 0, binary = 0;

  for (const c of commits) {
    const a = author(c);
    const b = bucket(c.at);
    let ci = 0, cd = 0;
    const cdirs = new Set();
    for (const f of c.files) {
      ci += f.ins;
      cd += f.del;
      if (f.binary) binary++;
      a.files.add(f.path);
      all.add(f.path);
      const fr = files.get(f.path) || { key: f.path, commits: 0, insertions: 0, deletions: 0, binary: false };
      fr.commits++;
      fr.insertions += f.ins;
      fr.deletions += f.del;
      fr.binary = fr.binary || f.binary;
      files.set(f.path, fr);
      const d = top(f.path);
      const dr = dirs.get(d) || { key: d, commits: 0, insertions: 0, deletions: 0, files: new Set() };
      dr.insertions += f.ins;
      dr.deletions += f.del;
      dr.files.add(f.path);
      dirs.set(d, dr);
      cdirs.add(d);
    }
    for (const d of cdirs) dirs.get(d).commits++;
    a.commits++;
    a.insertions += ci;
    a.deletions += cd;
    ins += ci;
    del += cd;
    seen(a, c);
    c.insertions = ci;
    c.deletions = cd;
    if (b) {
      a.days.add(b.date);
      weekday[b.weekday].commits++;
      weekday[b.weekday].insertions += ci;
      weekday[b.weekday].deletions += cd;
      hour[b.hour].commits++;
      const dy = days.get(b.date) || { date: b.date, commits: 0, insertions: 0, deletions: 0 };
      dy.commits++;
      dy.insertions += ci;
      dy.deletions += cd;
      days.set(b.date, dy);
    }
  }
  for (const m of mergeList) {
    const a = author(m);
    a.merges++;
    seen(a, m);
  }

  const people = [...authors.values()]
    .map((a) => ({
      name: a.name, ...(emails ? { email: a.email } : {}), commits: a.commits, merges: a.merges,
      insertions: a.insertions, deletions: a.deletions, files: a.files.size, active_days: a.days.size, first: a.first && a.first.at, last: a.last && a.last.at,
    }))
    .sort((x, y) => y.commits - x.commits || churn(y) - churn(x) || (x.name < y.name ? -1 : x.name > y.name ? 1 : 0));
  const stamps = [...commits, ...mergeList].sort((x, y) => x.t - y.t);
  // 给模型看的提交清单：新的在前，至多 limits.listed 条；说明（subject）只在设置许可时给。
  const listed = [...commits.map((c) => ({ c, merge: false })), ...mergeList.map((c) => ({ c, merge: true }))]
    .sort((x, y) => y.c.t - x.c.t || (x.c.sha < y.c.sha ? -1 : 1))
    .slice(0, s["limits.listed"])
    .map(({ c, merge }) => ({
      sha: c.sha.slice(0, 12), author: c.author, at: c.at,
      ...(merge ? { merge: true } : { insertions: c.insertions, deletions: c.deletions, files: c.files.length }),
      ...(subjects ? { subject: c.subject } : {}),
    }));

  return {
    format: FORMAT,
    generated_at: new Date().toISOString(),
    range: { since: q.since, until: q.until, rev: q.rev, head, paths: q.paths, by: "commit-date" },
    privacy: { subjects, emails },
    truncated,
    limits: { commits: cap, listed: s["limits.listed"], files: s["limits.files"] },
    ...(plain.partial ? { partial: plain.partial } : {}),
    totals: {
      commits: commits.length, merges: mergeList.length, authors: people.length,
      insertions: ins, deletions: del, files: all.size, binary_changes: binary,
      active_days: days.size, first: stamps.length ? stamps[0].at : null, last: stamps.length ? stamps[stamps.length - 1].at : null,
    },
    authors: people,
    by_weekday: weekday,
    by_hour: hour,
    by_day: [...days.values()].sort((a, b) => (a.date < b.date ? -1 : 1)),
    by_dir: [...dirs.values()].sort(byChurn).map((d) => ({ dir: d.key, commits: d.commits, insertions: d.insertions, deletions: d.deletions, files: d.files.size })),
    top_files: [...files.values()].sort(byChurn).slice(0, s["limits.files"])
      .map((f) => ({ path: f.key, commits: f.commits, insertions: f.insertions, deletions: f.deletions, ...(f.binary ? { binary: true } : {}) })),
    commits: listed,
    commits_listed: listed.length,
  };
}
