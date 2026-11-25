# 门禁

门禁是一站的出口条件里**确定性的检查**：`exit: [{kind: gate, gate: <名>}]` 引用包里的 `gates/<名>.yaml`。会话交了
输出之后，派工器跑这一站的门禁，全过才放行。模型说"我做完了"不算，门禁读到的文件才算。

## 先定判据，再选种类

写门禁之前先回答一句话：**这一站交付了什么、什么样算合格**。判据要从站的 `output` 契约和交付物（文件）出发，
写成能机械判的条件；判不了的（"写得好不好"）交给评审站或 `confirm`，不要写成门禁。

| 要查的 | 用 | 例子 |
|---|---|---|
| 文件在不在、空不空、有没有残留的模板文字 | `check: files` | 报告写出来了、不含 `TODO` |
| 文件是不是某个格式、字段齐不齐 | `check: schema` | `report.json` 有 `verdict`、`findings` |
| 文本里必须有 / 不能有某段 | `check: text` | 有"## 结论"一节，没有密钥样的串 |
| 前面某站交的输出满足条件 | `check: output` | `coverage` ≥ 0.8 |
| 交付之后没人再改过 | `check: sealed` | 放在最后一站 |
| 要算、要跨文件比对、要按规则统计 | `kind: script` | 每个需求编号在测试里都出现过 |
| 要跑第三方工具（测试、lint、编译） | `kind: command` | `cargo test`、`npm run lint` |
| 应用自己带的检查 | `kind: script` + `function:` | 插件提供的函数 |

能声明就用 `check`：不写代码，结果最稳。`script` 次之：只读、在沙箱里跑、证据由运行时记。`command` 最后：它在
机器上跑任意命令，要应用打开命令门禁、项目受信才跑。

## 共同的字段

```yaml
name: report-ready        # 与文件名相同：gates/report-ready.yaml
title: 报告已写好          # 界面与站消息里显示
kind: check               # check | command | script
fail_on: warn             # 可选：带警告（severity: warn）的 pass 也算不过；不写，verdict 是什么就是什么
check: files              # 下面是这一种要的字段，见后文
paths: ["${output_dir}/report.md"]
```

路径都相对运行的工作目录（项目、worktree，或运行自己的目录），可以引用 `${output_dir}`、`${inputs.<k>}`、
`${stations.<站>.output.<字段>}`；绝对路径必须落在工作目录里，`..` 与指到外面的链接都不行（那是 `error`）。

## `check`：声明的检查

### `files`

```yaml
name: report-ready
kind: check
check: files
paths: ["${output_dir}/report.md", "${output_dir}/data/*.json"]   # 每个路径或 glob 都要至少有一个文件
not_containing: ["TODO", "（在此填写）"]                           # 可选：任何一个都不能出现
non_empty: true                                                    # 缺省 true：空文件不算
```

glob 支持 `*`、`?`、`**`。一个路径一个文件都没匹配到就是一条 finding（`exists`），所以 glob 写错了不会"永远通过"，
而是不过——用 `flow_gate_try` 一试就知道。

### `schema`

```yaml
name: report-shape
kind: check
check: schema
file: "${output_dir}/report.json"     # 可以是 glob：匹配到的每个文件都要满足
as: json                              # json | yaml | jsonl（读成数组）| frontmatter；不写按扩展名
schema:
  type: object
  required: [verdict, findings]
  properties:
    verdict: {enum: [approve, reject]}
    findings: {type: array, items: {type: object, required: [path, message]}}
```

`frontmatter` 读 Markdown 开头 `---` 之间的 YAML。解析不了是一条 finding（`parse`），不是 `error`。

### `text`

```yaml
name: report-sections
kind: check
check: text
file: "${output_dir}/report.md"
matches: ["(?m)^## 结论", "(?m)^## 风险"]         # 每条都要匹配到
not_matches: ["(?i)api[_-]?key\\s*[:=]", "sk-[A-Za-z0-9]{20,}"]   # 任何一条都不能匹配；每处一条 finding，带行号
```

正则是 Rust 的 `regex` 语法：`(?m)` 让 `^`、`$` 按行，`(?i)` 不分大小写；YAML 里反斜杠要写两次。

### `output`

```yaml
name: enough-coverage
kind: check
check: output
value: "${stations.test.output}"       # 不写：整个引用上下文
schema: {type: object, required: [coverage], properties: {coverage: {type: number, minimum: 0.8}}}
```

`output` 只看交上来的值，不看文件：适合把"契约之外更严的要求"放到某一站，或在后面的站检查前面的输出。

### `sealed`

```yaml
name: untouched
kind: check
check: sealed
```

这次运行前面的门禁读过的每个文件（证据里的 sha256），现在还是那样；被改了、删了都是 finding。放在最后一站的出口里，
防止过了门禁之后又被改。它只在运行里判，试不了。

## `script`：项目里的 JS

```yaml
name: req-covered
kind: script
script: req-covered            # 跑 scripts/req-covered.js
params:                        # 可选：交给脚本的参数，引用先解析
  spec: "${output_dir}/requirements.md"
  tests: "tests/**/*.rs"
timeout_seconds: 30            # 缺省 30，至多 600
```

```js
// scripts/req-covered.js —— 每个需求编号（REQ-数字）在测试里都至少出现一次
export default function (input, flow) {
  const spec = flow.read.text(input.params.spec);
  if (spec == null) {
    // 判不了：没有可判的东西不是"不过"，是 error，要人看
    return { verdict: "error", summary: `${input.params.spec} 不存在` };
  }
  const ids = [...new Set(spec.match(/REQ-\d+/g) || [])];
  const tests = flow.glob(input.params.tests).map((p) => flow.read.text(p)).join("\n");
  const missing = ids.filter((id) => !tests.includes(id));
  return {
    verdict: missing.length ? "fail" : "pass",
    summary: `${ids.length - missing.length}/${ids.length} 个需求有测试`,
    findings: missing.map((id) => ({ rule: "req-covered", severity: "error", path: input.params.spec, message: `${id} 没有测试` })),
    metrics: { requirements: ids.length, missing: missing.length },
  };
}
```

- 一个 ES 模块，默认导出 `function (input, flow)`，可以是 `async`。
- `input`：`gate`、`run`、`flow`、`station`、`round`、`inputs`、`outputs`（每一站交的输出）、`output_dir`、`workspace`、
  `params`。
- `flow`：只读。`flow.read.text / json / yaml / jsonl / frontmatter(路径)`（文件不存在得 `null`；解析不了抛错）、
  `flow.glob(模式)`（路径数组）、`flow.stat(路径)`（`{size, sha256}` 或 `null`）、`flow.schema.validate(值, schema)`
  （`{ok, errors}`）、`flow.log(...)`（写进这一站的 `gate-<门禁>.log`）。
- 不能 `import`、不能跑命令、不能联网，路径限在工作目录里：读到外面就是 `error`，脚本自己 catch 了也一样。
- 返回 `{verdict, summary?, findings?, metrics?}`；抛错、超时、返回别的都是 `error`。**证据不用报**：脚本读过的文件
  由运行时记下 hash。
- 别信 `input.outputs` 里的说法（"测试全过了"），去读文件核实：门禁就是为这个存在的。

写 `function: <名>` 而不写 `script:`，跑的是应用（插件）提供的同名函数，`params` 一样传过去。

## `command`：第三方工具

```yaml
name: unit-tests
kind: command
command: cargo test --quiet
cwd: "."                       # 可选：相对工作目录
timeout_seconds: 600           # 缺省 600；超时整组进程杀掉，算 error
env: [CARGO_HOME, RUSTUP_HOME] # 可选：透传的环境变量；缺省一个不传（PATH、HOME、语言这些基础的总在）
result: json                   # 可选：按命令打印的结果判，见下
```

- 退出码 0 为 `pass`，其他为 `fail`，输出的末尾进 `summary`，全文在这一站的 `gate-<门禁>.log`。
- 起不来、超时、被信号杀掉是 `error`。
- 运行的上下文从环境变量读：`FLOW_RUN`、`FLOW_STATION`、`FLOW_WORKSPACE`、`FLOW_CONTEXT_FILE`（整个引用上下文的 JSON
  文件），小的还有 `FLOW_CONTEXT`。不要把 `${...}` 拼进命令行。
- 应用的密钥（模型的 API key 等）永远不传，写进 `env` 也不传。
- `result: json`：命令把结果 JSON（`{verdict, summary, findings, metrics}`）打印到标准输出——全部输出是它，或最后一行
  非空的是它；没打印就是 `error`。适合包一层 lint：把工具的报告转成 findings。

## 结果怎么用

每种门禁回同一个结果：`{verdict: pass | fail | error, summary, findings: [{rule, severity, path, line, message}], metrics}`，
`check` 和 `script` 读过的文件连同 hash 记为证据。

- **`fail`**：交付不合格，是这一站要改的。站写了 `on_unmet: {rework_to: <站>, max: <次数>}` 就自动打回到那一站，新一轮的会话会被告知
  每条 finding；没写就停在"未达标"等人。
- **`error`**：门禁没得出结论（文件读不了、命令起不来、脚本抛错）。不自动打回——改交付修不好它。人看过、修好之后在
  对话里让 Agent 重跑这一站的门禁（`flow_retry` 不带 `task`）。
- findings 写清楚**哪个文件、哪一行、哪里不对**：它就是打回时会话拿到的修改清单。

## 写一个合格的门禁

1. 写下判据（上面那句话），选种类。
2. 写门禁（`flow_part_set {kind: gate}`；脚本是 `{kind: script}`），引用写对：`${output_dir}` 还是项目里的路径。
3. **试**：`flow_gate_try` 拿一份合格的样例（`files`）跑一次，要 `pass`；再拿一份不合格的，要 `fail`，并且 findings
   说中了问题。两份都过的门禁什么也没查。用到前面站的输出就带上 `outputs`，用到启动参数就带上 `inputs`。
4. 放进站的 `exit`；要自动打回就写 `on_unmet: {rework_to: <站>, max: 2}`；最后一站可以加 `sealed`。
5. `flow_validate`、`flow_dry_run`（试运行里门禁是模拟的：`mocks.gates.<名>: false` 看不过时走哪条路）。

## 常见的坑

- 把"写得好"写成正则：判不了的交给评审或 `confirm`。
- 门禁去查模型的话而不是文件：`check: output` 查的是交上来的值，要核实就读文件。
- 判不出来时回 `fail`：会让会话去改一份没问题的交付。读不到、没有可判的，回 `error`。
- `command` 门禁依赖开发机上才有的东西（全局装的工具、某个环境变量）：要的变量写进 `env`，工具写进项目。
- 正则在 YAML 里没转义：`\d` 要写成 `"\\d"`，或者用单引号 `'\d'`。
