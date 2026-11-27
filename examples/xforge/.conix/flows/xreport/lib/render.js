// REPORT.md：头（范围、版本、统计时刻）+ 模型写的分析（analysis.md 的五节）+ 从 stats.json 生成的表。
// 不用模型、不看时钟：同一份 stats.json 与 analysis.md 永远得到同一份 REPORT.md，人批的就是能重算出来的那份。
import { SECTIONS, section, table } from "./markdown.js";

// 千分位自己加：toLocaleString 随运行时与区域设置变，同一份数据就不一定得到同一份报告。
const n = (v) => String(Math.trunc(Number(v || 0))).replace(/\B(?=(\d{3})+(?!\d))/g, ",");
const lines = (ins, del) => `+${n(ins)} / −${n(del)}`;
const day = (iso) => (iso ? String(iso).slice(0, 10) : "");

// 一格里的条：数相对最大的那个，至多 20 格。
function bar(v, max) {
  return max > 0 && v > 0 ? "█".repeat(Math.max(1, Math.round((v / max) * 20))) : "";
}

function who(a, emails) {
  return emails && a.email ? `${a.name} <${a.email}>` : a.name;
}

export function render(stats, report) {
  const s = stats, t = s.totals || {}, r = s.range || {};
  const emails = !!(s.privacy && s.privacy.emails);
  const out = [];
  out.push("# 提交历史报告", "");
  out.push(`- 范围：${r.since} 至 ${r.until}（按提交时间，与 git log --since/--until 相同）`);
  out.push(`- 版本：${r.rev}（${String(r.head || "").slice(0, 12)}）`);
  out.push(`- 路径：${(r.paths || []).length ? r.paths.join("、") : "整个仓库"}`);
  out.push(`- 统计于：${s.generated_at}`);
  out.push(`- 提交 ${n(t.commits)} 个（不含合并），合并 ${n(t.merges)} 个；作者 ${n(t.authors)} 人；${lines(t.insertions, t.deletions)} 行，${n(t.files)} 个文件`);
  if (s.truncated) out.push(`- **截断**：提交超过上限 ${n(s.limits && s.limits.commits)}，只统计了最新的那些；缩小范围或调高 limits.commits`);
  if (s.partial) out.push(`- 有 ${s.partial} 个提交改动的文件太多，git 的输出被截断，这些提交只算了一部分文件`);
  out.push("");

  for (const title of SECTIONS) {
    out.push(`## ${title}`, "", section(report, title) || "（没有写）", "");
  }

  out.push("## 附：数据", "", "以下的表由 `xstat render` 从 stats.json 生成，没有经过模型。", "");
  const people = s.authors || [];
  out.push("### 贡献者", "");
  out.push(people.length
    ? table(["作者", "提交", "合并", "增删行", "文件", "活跃天数", "首次", "最近"],
      people.map((a) => [who(a, emails), n(a.commits), n(a.merges), lines(a.insertions, a.deletions), n(a.files), n(a.active_days), day(a.first), day(a.last)]))
    : "这段时间没有提交。");
  out.push("");

  const wd = s.by_weekday || [];
  const wmax = Math.max(0, ...wd.map((w) => w.commits));
  out.push("### 按星期（作者本地时间）", "");
  out.push(table(["星期", "提交", "增删行", ""], wd.map((w) => [w.day, n(w.commits), lines(w.insertions, w.deletions), bar(w.commits, wmax)])));
  out.push("");

  const hrs = (s.by_hour || []).filter((h) => h.commits > 0);
  const hmax = Math.max(0, ...hrs.map((h) => h.commits));
  out.push("### 按小时（作者本地时间）", "");
  out.push(hrs.length
    ? table(["时段", "提交", ""], hrs.map((h) => [`${String(h.hour).padStart(2, "0")}:00–${String(h.hour).padStart(2, "0")}:59`, n(h.commits), bar(h.commits, hmax)])) + "\n\n其余时段没有提交。"
    : "没有提交。");
  out.push("");

  // 按日一行；跨度长（超过 31 个有提交的日子）时按月合起来，表才看得完。
  const days = s.by_day || [];
  const monthly = days.length > 31;
  const rows = monthly ? byMonth(days) : days.map((d) => ({ key: d.date, ...d }));
  const dmax = Math.max(0, ...rows.map((d) => d.commits));
  out.push(monthly ? "### 按月" : "### 按日", "");
  out.push(rows.length ? table([monthly ? "月份" : "日期", "提交", "增删行", ""], rows.map((d) => [d.key, n(d.commits), lines(d.insertions, d.deletions), bar(d.commits, dmax)])) : "没有提交。");
  out.push("");

  out.push("### 目录（顶层）", "");
  const dirs = s.by_dir || [];
  out.push(dirs.length
    ? table(["目录", "提交", "增删行", "文件"], dirs.map((d) => [d.dir === "." ? "（根目录）" : `${d.dir}/`, n(d.commits), lines(d.insertions, d.deletions), n(d.files)]))
    : "没有改动。");
  out.push("");

  out.push("### 改动最多的文件", "");
  const files = s.top_files || [];
  out.push(files.length
    ? table(["文件", "提交", "增删行"], files.map((f) => [f.path, n(f.commits), f.binary ? `${lines(f.insertions, f.deletions)}（含二进制）` : lines(f.insertions, f.deletions)]))
    : "没有改动。");
  out.push("");
  return out.join("\n");
}

function byMonth(days) {
  const m = new Map();
  for (const d of days) {
    const k = d.date.slice(0, 7);
    const r = m.get(k) || { key: k, commits: 0, insertions: 0, deletions: 0 };
    r.commits += d.commits;
    r.insertions += d.insertions;
    r.deletions += d.deletions;
    m.set(k, r);
  }
  return [...m.values()];
}
