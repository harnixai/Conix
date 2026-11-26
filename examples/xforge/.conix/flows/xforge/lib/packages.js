// 工作包的读法与判法：命令 xpkg 与门禁 packages-planned、package-check、unclaimed 共用。
// 工作包在这次运行的 work-packages.yaml（docs/design/xforge.md §2.2）；这里不用 cli.js：门禁脚本不引它。

export function loadPlan(flow, path) {
  const doc = flow.read.yaml(path);
  return doc && Array.isArray(doc.packages) ? doc : null;
}

// 路径写成 ./a 与 a 是一回事。
export const norm = (p) => String(p).trim().replace(/^\.\//, "");

// 这个文件在不在这个包的路径里。
export function inPaths(flow, pkg, file) {
  return flow.glob.test((pkg.paths || []).map(norm), norm(file));
}

// 基线提交以来的改动（含没提交的）。运行有基线用基线；没有（门禁试跑、工作区不在运行里）只看没提交的。
// 没有 git 就抛错：判不了的事不能判成"没有改动"。
export function changedFiles(flow) {
  const base = flow.git.base();
  if (base) return flow.git.changed();
  const head = flow.git.head();
  if (!head) throw new Error("工作区不是 git 仓库（或还没有提交）：判不了改了什么");
  return flow.git.changed({ since: head });
}

// after 里的环：回环上的编号链（a → b → a），没有环回 null。
export function findCycle(packages) {
  const after = new Map(packages.map((p) => [p.id, p.after || []]));
  const state = new Map(); // 1 在走，2 走完
  const trail = [];
  const walk = (id) => {
    if (state.get(id) === 2) return null;
    if (state.get(id) === 1) return [...trail.slice(trail.indexOf(id)), id];
    state.set(id, 1);
    trail.push(id);
    for (const next of after.get(id) || []) {
      if (!after.has(next)) continue; // 不存在的另报
      const c = walk(next);
      if (c) return c;
    }
    trail.pop();
    state.set(id, 2);
    return null;
  };
  for (const id of after.keys()) {
    const c = walk(id);
    if (c) return c;
  }
  return null;
}

// 工作包之间的问题：[message]。交出的 packages 对不对得上 plan 另判（门禁 packages-planned）。
// "路径重叠"只认工作区里真有的文件：展开每个包的 glob，同一个文件落进两个包就是重叠。
// 还没建出来的新文件、两个都写 `src/**` 之类的重叠要靠路径写得不重叠；这里不猜。
export function planProblems(flow, packages) {
  const out = [];
  const ids = new Set(packages.map((p) => p.id));
  for (const p of packages) {
    if (!(p.paths || []).length) out.push(`工作包 ${p.id} 没有路径`);
    if (!(p.criteria || []).length) out.push(`工作包 ${p.id} 没有判据：xpkg plan 要 --criteria "一;二"`);
    for (const a of p.after || []) {
      if (a === p.id) out.push(`工作包 ${p.id} 的 after 指着自己`);
      else if (!ids.has(a)) out.push(`工作包 ${p.id} 的 after 指的 ${a} 不存在`);
    }
  }
  const cycle = findCycle(packages);
  if (cycle) out.push(`after 成环：${cycle.join(" → ")}`);
  const owner = new Map(); // 文件 → 先盖住它的包
  const reported = new Set();
  for (const p of packages) {
    const mine = new Set();
    for (const pat of (p.paths || []).map(norm)) {
      let files;
      try {
        files = flow.glob(pat);
      } catch (e) {
        out.push(`工作包 ${p.id} 的路径 ${pat} 展开不了：${e.message}`);
        continue;
      }
      for (const f of files) mine.add(f);
    }
    for (const f of mine) {
      const o = owner.get(f);
      if (o && o !== p.id) {
        const key = `${o}|${p.id}`;
        if (!reported.has(key)) {
          reported.add(key);
          out.push(`工作包 ${o} 与 ${p.id} 的路径重叠（都盖住 ${f}）：让每个文件只属于一个包`);
        }
      } else owner.set(f, p.id);
    }
  }
  return out;
}
