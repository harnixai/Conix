// 两路开发（xforge/solid、xforge/major，docs/design/xforge.md §5）的规格侧：父运行在分叉前封存（xspec seal），两个子运行核对
// 自己手里那一份没改（门禁 spec-unchanged）。两边用同一份判法：哪些文件算规格侧、摘要怎么算。

// 不算规格侧的：两路各自交付的目录 children/（分叉之后才有，打回后的新一轮里有上一轮的），流程库的过程目录 _run/，
// 以及父运行一路记下去的台账（发现、决定、风险）：合起来、验证两站在分叉之后还往里记（合起来没对上记一条发现、
// 打回一路），封住它们，打回后新一轮的子运行一开工就判"规格侧改了"、又改不回去。问题台账留在规格侧：澄清时人答的，
// 「规格侧定了」的确认绑着它。
const SKIP = ["children/", "_run/", "ledgers/findings.yaml", "ledgers/decisions.yaml", "ledgers/risks.yaml"];

// dir（相对工作目录：flow.glob 回的就是相对的）下规格侧的每个文件（相对 dir）→ 它字节的 sha256。
export function sealSide(flow, dir) {
  const files = {};
  const at = trimmed(dir);
  for (const path of flow.glob(`${at}/**`)) {
    const rel = path.slice(at.length + 1);
    if (SKIP.some((s) => rel.startsWith(s))) continue;
    files[rel] = flow.stat(path).sha256;
  }
  return files;
}

// 对照封存的摘要：[{path, message}]，改了的、删了的各一条。只核对封存过的文件：分叉之后父运行目录里多出来的
// 不是子运行写的，子运行的会话也写不到那里（站的 writes）。
export function sideProblems(flow, dir, sealed) {
  const out = [];
  for (const [rel, sha] of Object.entries(sealed)) {
    const path = `${trimmed(dir)}/${rel}`;
    const now = flow.stat(path);
    if (!now) out.push({ path, message: `${rel} 不见了：规格侧是父运行批准过的，子运行不删` });
    else if (now.sha256 !== sha) out.push({ path, message: `${rel} 改了：规格侧是父运行批准过的，子运行只读；照它做不到就记进自己的台账，交给人看` });
  }
  return out;
}

const trimmed = (dir) => String(dir).replace(/\/+$/, "").replace(/^\.\//, "");
