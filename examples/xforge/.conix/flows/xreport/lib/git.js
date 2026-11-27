// 系统的 git，经 flow.exec 起（flows.md §7.14）：不自己读对象库，git 怎么数就怎么数（.mailmap、改名检测都是 git 的）。
// 每次都关掉仓库配置里能让 git 起别的程序的几处：fsmonitor、分页、外部 diff 与 textconv、签名校验（gpg）。

// 放在子命令前的全局选项。
export const GIT = [
  "git", "--no-pager",
  "-c", "core.fsmonitor=false",
  "-c", "log.showSignature=false",
  // log.follow 在只给一个路径时会跟着改名走出那个路径：报告的范围就不是人写的那一个了。
  "-c", "log.follow=false",
];

// flow.exec 不让起进程（项目没受信、总开关关着、宿主没给）时它抛错；换成人看得懂、知道该做什么的话。
export function refusal(e) {
  const m = String((e && e.message) || e);
  if (/not turned on/.test(m)) {
    return "这台机器关掉了流程起进程：conix.yaml 的 flows.command_gates 是 off。xreport 要经它跑系统的 git，改回 trusted（缺省）再跑";
  }
  if (/not trusted/.test(m)) {
    return "这个项目还没受信：流程只在受信的项目里起进程（xreport 要跑 git log）。在 Conix 里打 /trust 信任这个项目再跑";
  }
  if (/not offered/.test(m)) {
    return "这里没有执行服务（宿主没给 flow.exec），跑不了 git";
  }
  return null;
}

// 跑一次 git；起不来是抛错（换成中文的理由），退出非 0 由调用的人判。
export function git(flow, args, opts) {
  let r;
  try {
    r = flow.exec({ argv: [...GIT, ...args], ...(opts || {}) });
  } catch (e) {
    const why = refusal(e);
    throw new Error(why || `git 起不来：${e.message}`);
  }
  return r;
}

export function ok(r, what) {
  if (r.timed_out) throw new Error(`${what}：git 超时了。缩小范围（since/until、paths）再跑`);
  if (r.code !== 0) throw new Error(`${what}：git 退出码 ${r.code}：${String(r.stderr || "").trim().split("\n").slice(-3).join(" ")}`);
  return r.stdout;
}

// 头：每个提交一条，字段以 \x1f 分开，以 \x1e 开头；-z 让头与 numstat 的每一项都以 \0 结尾，路径原样（不加引号、
// 改名的新旧路径各占一项），所以文件名里有空格、制表符、引号都拆得对。说明（%s）放最后：它里面有什么都不会把字段拆错。
export function format(emails) {
  return "--format=%x1e" + ["%H", "%aN", ...(emails ? ["%aE"] : []), "%aI", "%s"].join("%x1f");
}

function header(tok, emails) {
  const f = tok.slice(1).split("\x1f");
  const n = emails ? 4 : 3;
  const c = { sha: f[0], author: f[1], at: f[n - 1], subject: f.slice(n).join("\x1f"), files: [] };
  if (emails) c.email = f[2];
  // 先后按时刻比，不按字符串：%aI 带各自的时区，字符串的先后不是时间的先后。
  c.t = Date.parse(c.at) || 0;
  return c;
}

// git log -z --numstat 的输出 → [{sha, author, email?, at, subject, files: [{path, from?, ins, del, binary}]}]。
// numstat 一项是 "增\t删\t路径"；改名是 "增\t删\t" 后面跟旧路径、新路径两项；二进制文件的增删是 "-"。
export function parseLog(out, emails) {
  const toks = out.split("\0");
  const commits = [];
  let cur = null;
  for (let i = 0; i < toks.length; i++) {
    let t = toks[i];
    if (t.startsWith("\x1e")) {
      cur = header(t, emails);
      commits.push(cur);
      continue;
    }
    // 头的 \0 之后、第一项 numstat 之前 git 加了一个换行。
    if (t.startsWith("\n")) t = t.slice(1);
    if (!t || !cur) continue;
    const m = /^(\d+|-)\t(\d+|-)\t([\s\S]*)$/.exec(t);
    if (!m) continue;
    const binary = m[1] === "-" || m[2] === "-";
    const f = { path: m[3], ins: binary ? 0 : Number(m[1]), del: binary ? 0 : Number(m[2]), binary };
    if (!f.path) {
      // 改名：旧、新路径是接着的两项，不管它们以什么字符开头都照收。
      f.from = toks[++i] || "";
      f.path = toks[++i] || "";
    }
    cur.files.push(f);
  }
  return commits;
}
