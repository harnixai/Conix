// xpkg：这次变更的工作包，在这次运行的 work-packages.yaml（给人看的）和 packages.paths（门禁 unclaimed 读的，一行一个路径模式）。
//
//   xpkg plan <编号> "<标题>" --paths "src/a/**,tests/a/**" [--verify "<怎么验证>"] [--criteria "标准一;标准二"]
//                                       划一个工作包（同编号再写一次是改它）
//   xpkg list                           列出工作包
//   xpkg done <编号> "<做了什么、怎么验证的>" [--files "a,b"]   登记做完
//   xpkg blocked <编号> "<卡在哪>"      登记做不下去
export default function (input, flow) {
  const [sub, id, ...rest] = input.args;
  const pos = [], opt = {};
  for (let i = 0; i < rest.length; i++) {
    if (rest[i].startsWith("--")) opt[rest[i].slice(2)] = rest[++i];
    else pos.push(rest[i]);
  }
  const file = `${input.output_dir}/work-packages.yaml`;
  const plan = flow.read.yaml(file) || { packages: [] };
  const need = (v, why) => { if (!v) throw new Error(why); return v; };
  const list = (s) => (s || "").split(/[,;]/).map((x) => x.trim()).filter(Boolean);
  const save = () => {
    flow.write.yaml(file, plan);
    flow.write.text(`${input.output_dir}/packages.paths`, plan.packages.flatMap((p) => p.paths).join("\n") + "\n");
  };
  if (sub === "list") {
    return plan.packages.map((p) => `${p.id} [${p.status}] ${p.title}\n  路径：${p.paths.join(", ")}${p.verify ? `\n  验证：${p.verify}` : ""}${(p.criteria || []).length ? `\n  标准：${p.criteria.join("；")}` : ""}${p.summary ? `\n  结果：${p.summary}` : ""}`).join("\n") || "还没有工作包";
  }
  need(id && /^[a-z0-9-]+$/.test(id), "工作包编号是小写字母、数字、连字符");
  if (sub === "plan") {
    const paths = need(list(opt.paths), "--paths 至少一个路径模式（改到的每个文件都要盖住，测试也算）");
    const p = { id, title: need(pos[0], "xpkg plan <编号> \"<标题>\" --paths …"), paths, status: "planned" };
    if (opt.verify) p.verify = opt.verify;
    if (opt.criteria) p.criteria = list(opt.criteria);
    plan.packages = plan.packages.filter((x) => x.id !== id).concat([p]);
    save();
    return `工作包 ${id}：${paths.length} 个路径模式`;
  }
  const p = need(plan.packages.find((x) => x.id === id), `没有工作包 ${id}`);
  if (sub === "done" || sub === "blocked") {
    p.status = sub;
    p.summary = need(pos.join(" ").trim(), `xpkg ${sub} <编号> "<说明>"`);
    if (opt.files) p.files = list(opt.files);
    save();
    return `${id} → ${sub}`;
  }
  return "xpkg plan|list|done|blocked（看文件头的说明）";
}
