// 命令共用的小件：参数拆分与断言。门禁脚本不用它们。

// 不满足就抛错：命令抛错就是这次调用失败，信息给调用的人或模型看。
export function need(v, why) {
  if (!v) throw new Error(why);
  return v;
}

// 可以写几次的选项：["--k", "a", "--k", "b"] → ["a", "b"]（parseFlags 只留最后一个）。
export function allOf(args, flag) {
  const out = [];
  for (let i = 0; i < args.length; i++) if (args[i] === `--${flag}`) out.push(args[++i]);
  return out;
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
