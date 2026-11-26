// xpkg：这次变更的工作包，在这次运行的 work-packages.yaml。模型不手写，用这个命令。
// 一个包 = 一块能单独做、单独验证的改动：它的路径、做完的判据、先后。
//
//   xpkg plan <编号> "<标题>" --paths a/**,b/** --criteria "判据一;判据二" [--after p1,p2] [--verify <名字>[:参数=值,…]]…
//        划一个包（同编号再写一次是改它）。路径是 glob，逗号隔开，改到的每个文件（测试也算）都要盖住；
//        判据用分号隔开，依次编成 C1、C2…，做完时每条都要有证据；after 是要先做完的包；
//        --verify 可以写几次，每次一项这个包做完时要跑的验证：名字来自项目的 verify 菜单（xpkg verify-menu 看），
//        参数要整串匹配菜单里的正则。任务交付时门禁 package-tests 照它跑，退出码 0 才过
//   xpkg done <编号> "<做了什么>" --files a,b --evidence "C1=…;C2=…"
//        登记做完：--files 是改了的文件（都要在包的路径里）；--evidence 给每条判据一句证据（跑了什么、看到什么）
//   xpkg blocked <编号> "<卡在哪>"        登记做不下去（门禁 package-check 会把它报给人）
//   xpkg list                             列出工作包与状态
//   xpkg verify-menu                      列出项目声明的验证（.conix/governance/xforge.yaml 的 verify），运行之外也能看
import { allOf, need, parseFlags } from "../lib/cli.js";
import { verifyMenu } from "../lib/governance.js";
import { inPaths, loadPlan, norm } from "../lib/packages.js";
import { describeMenu, expand, label, menuProblems, parseEntry } from "../lib/verify.js";

const csv = (s) => (s || "").split(",").map(norm).filter(Boolean);
const semi = (s) => (s || "").split(/[;；]/).map((x) => x.trim()).filter(Boolean);

export default function (input, flow) {
  const [sub, id, ...rest] = input.args;
  if (sub === "verify-menu") {
    const menu = verifyMenu(flow);
    if (!menu) return "项目没有声明验证（.conix/governance/xforge.yaml 的 verify）：工作包不带 --verify，交付时只核对登记";
    const problems = menuProblems(menu);
    need(!problems.length, `verify 菜单有问题（要人改 .conix/governance/xforge.yaml）：${problems.join("；")}`);
    return describeMenu(menu) || "verify 菜单是空的";
  }
  need(input.output_dir, "xpkg 写的是这次运行的工作包：要在运行里调用");
  const { pos, opt } = parseFlags(rest);
  const file = `${input.output_dir}/work-packages.yaml`;
  const plan = loadPlan(flow, file) || { packages: [] };

  if (sub === "list") {
    return plan.packages.map((p) => {
      const crit = (p.criteria || []).map((c) => `${c.id} ${c.text}${p.evidence && p.evidence[c.id] ? `（证据：${p.evidence[c.id]}）` : ""}`);
      return `${p.id} [${p.status}] ${p.title}\n  路径：${(p.paths || []).join(", ")}`
        + ((p.after || []).length ? `\n  先做：${p.after.join(", ")}` : "")
        + (shownVerify(p) ? `\n  验证：${shownVerify(p)}` : "")
        + (crit.length ? `\n  判据：${crit.join("；")}` : "")
        + (p.files ? `\n  改了：${p.files.join(", ")}` : "")
        + (p.summary ? `\n  结果：${p.summary}` : "");
    }).join("\n") || "还没有工作包";
  }

  need(id && /^[a-z0-9-]+$/.test(id), "工作包编号是小写字母、数字、连字符");

  if (sub === "plan") {
    const title = need(pos[0], "xpkg plan <编号> \"<标题>\" --paths a/**,b/** --criteria \"一;二\"");
    const paths = csv(opt.paths);
    need(paths.length, "--paths 至少一个路径模式（改到的每个文件都要盖住，测试也算），逗号隔开");
    for (const p of paths) need(!p.startsWith("/") && !p.split("/").includes(".."), `路径 ${p} 要是项目内的相对路径，不能以 / 开头或含 ..`);
    const texts = semi(opt.criteria);
    need(texts.length, "--criteria 至少一条判据（做完怎么算对），分号隔开：--criteria \"空串返回空串;超长截断\"");
    const after = csv(opt.after);
    need(!after.includes(id), `工作包 ${id} 的 after 指着自己`);
    const old = plan.packages.find((x) => x.id === id);
    need(!old || old.status !== "done", `工作包 ${id} 已经 done，不能重划：要改就另划一个包`);
    const verify = plannedVerify(flow, rest);
    const p = { id, title, paths, after, criteria: texts.map((text, i) => ({ id: `C${i + 1}`, text })), status: "planned" };
    if (verify.length) p.verify = verify;
    plan.packages = plan.packages.filter((x) => x.id !== id).concat([p]);
    flow.write.yaml(file, plan);
    return `工作包 ${id}：${paths.length} 个路径模式，${texts.length} 条判据（C1${texts.length > 1 ? `…C${texts.length}` : ""}）`
      + (verify.length ? `，交付时跑 ${verify.map(label).join("、")}` : "");
  }

  const p = need(plan.packages.find((x) => x.id === id), `没有工作包 ${id}：xpkg plan 划，xpkg list 看有哪些`);

  if (sub === "blocked") {
    p.status = "blocked";
    p.summary = need(pos.join(" ").trim(), "xpkg blocked <编号> \"<卡在哪>\"");
    flow.write.yaml(file, plan);
    return `${id} → blocked`;
  }

  if (sub === "done") {
    const summary = need(pos.join(" ").trim(), "xpkg done <编号> \"<做了什么>\" --files a,b --evidence \"C1=…;C2=…\"");
    const files = csv(opt.files);
    need(files.length, "--files 要列出改了的文件（逗号隔开）：门禁拿它与 git 的改动对账");
    const outside = files.filter((f) => !inPaths(flow, p, f));
    need(!outside.length, `这些文件不在工作包 ${id} 的路径（${p.paths.join(", ")}）里：${outside.join(", ")}。那是别的包的地方：要么不该改，要么先 xpkg plan 把路径划进来`);
    const evidence = {};
    for (const part of semi(opt.evidence)) {
      const at = part.indexOf("=");
      need(at > 0, `证据写成 C1=…;C2=…，这一段不是：${part}`);
      evidence[part.slice(0, at).trim()] = part.slice(at + 1).trim();
    }
    const known = (p.criteria || []).map((c) => c.id);
    const unknown = Object.keys(evidence).filter((c) => !known.includes(c));
    need(!unknown.length, `工作包 ${id} 没有判据 ${unknown.join(", ")}，有：${known.join(", ")}`);
    const missing = known.filter((c) => !evidence[c]);
    need(!missing.length, `判据 ${missing.join(", ")} 没有证据：--evidence "${known.map((c) => `${c}=…`).join(";")}"（跑了什么、看到什么）`);
    p.status = "done";
    p.summary = summary;
    p.files = files;
    p.evidence = evidence;
    flow.write.yaml(file, plan);
    return `${id} → done（${files.length} 个文件，${known.length} 条判据有证据）`;
  }
  return "xpkg plan|done|blocked|list（看文件头的说明）";
}

// --verify 的每一项，按项目的 verify 菜单核对：名字在菜单里、参数一个不多一个不少、值整串匹配正则。
function plannedVerify(flow, args) {
  const texts = allOf(args, "verify");
  if (!texts.length) return [];
  const menu = verifyMenu(flow);
  need(menu, "项目没有声明验证（.conix/governance/xforge.yaml 的 verify），--verify 无从选：不带它，或请人在治理文件里加");
  const problems = menuProblems(menu);
  need(!problems.length, `verify 菜单有问题（要人改 .conix/governance/xforge.yaml）：${problems.join("；")}`);
  const out = [];
  for (const t of texts) {
    const e = parseEntry(t);
    expand(menu, e);
    need(!out.some((x) => label(x) === label(e)), `--verify ${label(e)} 写了两次`);
    out.push(e);
  }
  return out;
}

// 列表里的验证：登记的几项；以前的包写的是一句说明，照原样给。
function shownVerify(p) {
  if (Array.isArray(p.verify)) return p.verify.map(label).join("、");
  return p.verify ? String(p.verify) : "";
}
