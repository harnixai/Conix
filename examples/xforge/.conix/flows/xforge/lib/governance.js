// 项目的治理开关（docs/design/xforge.md §1）：.conix/governance/xforge.yaml 的 governance。
// 没有这个文件按缺省：规格基线开、接口基线关。所有命令与门禁都从这里读，免得各写一份缺省。
export function governance(flow) {
  const doc = flow.read.yaml(".conix/governance/xforge.yaml");
  const g = (doc && doc.governance) || {};
  return { spec: g.spec !== false, interface: g.interface === true };
}

// 人声明的验证命令（docs/design/xforge.md §1 的 verify 菜单），没有就是 null。命令与门禁都从这里读：
// 治理设置换个来处时只改这一处。
export function verifyMenu(flow) {
  const doc = flow.read.yaml(".conix/governance/xforge.yaml");
  return (doc && doc.verify) || null;
}

// 关着的那条基线，它的门禁回这个：不是没过，是这个项目不欠它。
export function notOwed(which) {
  return { verdict: "pass", summary: `不欠（governance.${which}=false）`, findings: [] };
}
