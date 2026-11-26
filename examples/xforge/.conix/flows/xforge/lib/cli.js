// 命令共用的小件：参数拆分与断言。门禁脚本不用它们。

// 不满足就抛错：命令抛错就是这次调用失败，信息给调用的人或模型看。
export function need(v, why) {
  if (!v) throw new Error(why);
  return v;
}

// ["a", "--k", "v", "b"] → { pos: ["a", "b"], opt: { k: "v" } }
export function parseFlags(args) {
  const pos = [], opt = {};
  for (let i = 0; i < args.length; i++) {
    if (args[i].startsWith("--")) opt[args[i].slice(2)] = args[++i];
    else pos.push(args[i]);
  }
  return { pos, opt };
}
