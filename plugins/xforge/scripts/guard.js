// tool.around：模型不能直接写 xforge/ 下的结构化数据（docs/design/plugins.md §6.5、§6.6）。
// 只放行三份 .md 与 constitution.md；其余只经本插件的命令与函数写。
// 不用 agix.*：守卫是最外面一环，路径比较写成最朴素的字符串处理，读的人一眼看得完。

const WRITERS = ["Write", "Edit", "NotebookEdit"];
const FREE = /^xforge\/(constitution\.md|changes\/[^/]+\/(proposal|design|verification)\.md)$/;
const NAMES_PROTECTED = /xforge\/(manifest\.yaml|specs|archive|changes\/[^\s'"]*\/(delta|change|work-packages)\.yaml|changes\/[^\s'"]*\/ledgers)/;
const MUTATING = /(^|[\s;&|(])(rm|mv|cp|touch|tee|truncate|dd|install|ln|chmod|sed\s+-[a-zA-Z]*i|perl\s+-[a-zA-Z]*i)(\s|$)|>/;

function relativeTo(root, path) {
  const parts = [];
  const full = path.startsWith("/") || !root ? path : `${root}/${path}`;
  for (const seg of full.split("/")) {
    if (seg === "" || seg === ".") continue;
    if (seg === "..") parts.pop();
    else parts.push(seg);
  }
  const norm = parts.join("/");
  const base = root ? root.split("/").filter(Boolean).join("/") : "";
  if (!base) return norm;
  if (norm === base) return "";
  return norm.startsWith(`${base}/`) ? norm.slice(base.length + 1) : null;
}

function protectedPath(root, path) {
  const rel = relativeTo(root, path);
  return rel !== null && rel.startsWith("xforge/") && !FREE.test(rel);
}

function around(call) {
  const ours = call.plugin === "xforge" && (String(call.via).startsWith("RunExt:") || String(call.via).startsWith("PluginCall:"));
  if (ours) return null;
  const input = call.input || {};
  if (WRITERS.includes(call.tool)) {
    const path = String(input.file_path || input.notebook_path || "");
    if (path && protectedPath(call.projectRoot, path)) {
      return { action: "deny", reason: `xforge/ data changes only through the xflow / xspec / xforge commands (RunExt); ${path} is not written directly` };
    }
    return null;
  }
  if (call.tool === "Bash") {
    const cmd = String(input.command || "");
    if (NAMES_PROTECTED.test(cmd) && MUTATING.test(cmd)) {
      return { action: "deny", reason: "xforge/ data changes only through the xflow / xspec / xforge commands (RunExt), not through Bash" };
    }
  }
  return null;
}
