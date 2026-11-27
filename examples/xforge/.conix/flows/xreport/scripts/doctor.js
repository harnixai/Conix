// doctor：这个项目跑得了 xreport 没有（docs/design/xreport.md §5；流程库的自检，flows.md §7.13）。宿主的 conix flow doctor
// 在运行之外调它。只读：每条发现说哪里不对、人该怎么改（fix），自己什么都不改。
//
//   error  照现在的样子统计站会失败：git 起不来、不是 git 仓库、没有提交、设置的类型不对
//   warn   判不了：宿主没给执行服务
//   info   知道就好：有 .mailmap（作者照它合并）、没有设置文件（用缺省）
import { pick, SETTINGS } from "../lib/args.js";
import { git } from "../lib/git.js";

// 与 bundle.yaml 的 settings.file 是同一份。
const FILE = ".conix/governance/xreport.yaml";

export default function (input, flow) {
  const out = [];
  const add = (level, message, path, fix) => out.push({ level, message, ...(path ? { path } : {}), ...(fix ? { fix } : {}) });
  checkSettings(flow, add);
  if (checkGit(flow, add)) checkRepo(flow, add);
  if (flow.stat(".mailmap")) add("info", "有 .mailmap：作者按它合并（%aN），报告里是合并后的名字", ".mailmap");
  return { findings: out };
}

function checkSettings(flow, add) {
  let s;
  try {
    s = flow.settings;
  } catch (e) {
    add("error", `设置文件读不成，xreport 会拒绝启动：${e.message}`, FILE, "改成 YAML 映射：range、privacy、limits、review（docs/design/xreport.md §3）");
    return;
  }
  if (!flow.stat(FILE)) add("info", `没有 ${FILE}：用缺省（近 30 天、提交说明给模型、不记邮箱）`, FILE);
  for (const [k, check] of Object.entries(SETTINGS)) {
    try {
      check(pick(s, k));
    } catch (e) {
      add("error", `设置 ${e.message}：统计站会拒绝`, FILE, `改 ${k}`);
    }
  }
}

// git 起得来：经 flow.exec，与统计站同一条路；起不来就说为什么、该做什么。
function checkGit(flow, add) {
  let r;
  try {
    r = git(flow, ["--version"]);
  } catch (e) {
    // 没有执行服务是这次自检的宿主没给，不是项目缺什么：只说判不了。没受信、总开关关着，运行里一样起不来，是 error。
    if (/没有执行服务/.test(e.message)) add("warn", `判不了 git 跑不跑得起来：${e.message}`);
    else add("error", `统计站会失败：${e.message}`);
    return false;
  }
  if (r.code !== 0) {
    add("error", `git --version 退出码 ${r.code}：${String(r.stderr || "").trim()}`, null, "装上 git，并确认它在 PATH 里");
    return false;
  }
  return true;
}

function checkRepo(flow, add) {
  const inside = git(flow, ["rev-parse", "--is-inside-work-tree"]);
  if (inside.code !== 0 || inside.stdout.trim() !== "true") {
    add("error", "项目不是 git 仓库：没有历史可统计", null, "git init 并提交");
    return;
  }
  const head = git(flow, ["rev-parse", "--verify", "--quiet", "HEAD^{commit}"]);
  if (head.code !== 0) add("error", "仓库还没有提交：没有历史可统计", null, "先提交一次");
}
