// xstat：这个项目的 git 提交历史，统计成 stats.json、定稿成 REPORT.md。经 flow.exec 跑系统的 git（只读），
// 只在受信的项目里、流程的执行没关时能跑（flows.md §7.14）。
//
//   xstat collect [--since YYYY-MM-DD] [--until YYYY-MM-DD] [--rev <提交>] [--path <路径>]…
//                                   统计，写这次运行的 stats.json；不给的取运行的输入，再不给取设置：
//                                   until 缺省今天，since 缺省 until 往前 days（输入）或 range.days（设置）天，rev 缺省 HEAD
//   xstat render                    由 stats.json 与 analysis.md 生成 REPORT.md（确定的：同样的两份永远同样的结果）
//   xstat show [<stats.json>]       一份统计的摘要；缺省是这次运行的，运行之外是最近一次归档的
//
// 范围按提交时间筛（同 git log --since/--until）；星期、小时按作者自己的时区。合并提交另数，不算行数。
// 名字照 git（.mailmap 合过）；邮箱只在设置 privacy.emails 为 true 时记；提交说明在 privacy.subjects 为 false 时不记。
import { checkCount, checkDate, checkPath, checkRev, daysBefore, need, parseArgs, settingsOf, today } from "../lib/args.js";
import { collect } from "../lib/stats.js";
import { render } from "../lib/render.js";

// 模型写的叫 analysis.md 而不叫 report.md：大小写不分的文件系统（macOS 缺省）上它与 REPORT.md 是同一个文件，定稿会盖掉分析。
const STATS = "stats.json", ANALYSIS = "analysis.md", FINAL = "REPORT.md";

export default function (input, flow) {
  const [sub, ...rest] = input.args;
  const out = input.output_dir;
  const at = (f) => `${out}/${f}`;

  if (sub === "collect") {
    need(out, "xstat collect 写的是这次运行的 stats.json：要在运行里调用");
    const s = settingsOf(flow.settings);
    const { pos, opt } = parseArgs(rest, ["since", "until", "rev"], ["path"]);
    need(!pos.length, `xstat collect 不收位置参数：${pos.join(" ")}`);
    const given = input.inputs || {};
    const until = checkDate(opt.until || given.until || today(), "until");
    const days = given.days ? checkCount(given.days, "days", 3660) : s["range.days"];
    const since = checkDate(opt.since || given.since || daysBefore(until, days), "since");
    need(since <= until, `since ${since} 在 until ${until} 之后`);
    const rev = checkRev(opt.rev || given.rev || "HEAD");
    const paths = (opt.path.length ? opt.path : given.paths || []).map(checkPath);
    need(paths.length <= 50, "路径至多 50 个");
    const stats = collect(flow, { since, until, rev, paths }, s);
    flow.write.json(at(STATS), stats);
    const t = stats.totals;
    return {
      stats: STATS, since, until, rev, head: stats.range.head, commits: t.commits, merges: t.merges, authors: t.authors,
      insertions: t.insertions, deletions: t.deletions, truncated: stats.truncated,
    };
  }

  if (sub === "render") {
    need(out, "xstat render 写的是这次运行的 REPORT.md：要在运行里调用");
    const stats = need(flow.read.json(at(STATS)), `没有 ${STATS}：先 xstat collect`);
    const report = need(flow.read.text(at(ANALYSIS)), `没有 ${ANALYSIS}：分析站还没写`);
    const text = render(stats, report);
    flow.write.text(at(FINAL), text);
    return { report: FINAL, sha256: flow.sha256(text) };
  }

  if (sub === "show") {
    const file = rest[0] || (out ? at(STATS) : latest(flow, input.data_dir));
    need(file, "运行之外没有归档过的统计：给一个 stats.json 的路径");
    const s = need(flow.read.json(file), `${file} 读不到`);
    return summary(file, s);
  }

  throw new Error(`子命令是 collect、render、show、help 之一，现在是 ${JSON.stringify(sub)}`);
}

// 包的数据目录里最近一次归档的 stats.json（归档名以日期序号开头，按名字排就是按时间）。
function latest(flow, data) {
  if (!data) return null;
  const all = flow.glob(`${data}/archive/*/${STATS}`).sort();
  return all.length ? all[all.length - 1] : null;
}

function summary(file, s) {
  const t = s.totals || {}, r = s.range || {};
  const lines = [
    `${file}：${r.since} 至 ${r.until}，${r.rev}（${String(r.head || "").slice(0, 12)}）${(r.paths || []).length ? `，路径 ${r.paths.join(", ")}` : ""}`,
    `提交 ${t.commits}（合并另有 ${t.merges}），作者 ${t.authors}，+${t.insertions} / -${t.deletions} 行，${t.files} 个文件${s.truncated ? "；截断了" : ""}`,
  ];
  for (const a of (s.authors || []).slice(0, 10)) {
    lines.push(`  ${a.name}：${a.commits} 个提交、${a.merges} 个合并，+${a.insertions} / -${a.deletions}，${a.files} 个文件`);
  }
  if ((s.authors || []).length > 10) lines.push(`  ……还有 ${s.authors.length - 10} 人`);
  return lines.join("\n");
}
