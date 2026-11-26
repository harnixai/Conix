// 项目的治理开关（docs/design/xforge.md §1）：.conix/governance/xforge.yaml 的 governance。
// 文件由 bundle.yaml 的 settings 声明，运行启动时合并缺省钉住，flow.settings 给的就是那一份：运行里改文件不改这次的规则。
// 不再自己读文件：读文件的宿主读不了带 settings 的 bundle.yaml，留个回退也走不到。缺省只写在 bundle.yaml 一处，
// 这里照旧按"规格不写 false 就开、接口写 true 才开"解释，文件里写得不全也是这个意思。
export function governance(flow) {
  const g = (flow.settings && flow.settings.governance) || {};
  return { spec: g.spec !== false, interface: g.interface === true };
}

// 关着的那条基线，它的门禁回这个：不是没过，是这个项目不欠它。
export function notOwed(which) {
  return { verdict: "pass", summary: `不欠（governance.${which}=false）`, findings: [] };
}
