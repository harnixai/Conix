# 交付

每个工作项都要**交付**一份输出，交付满足站的 `output` 契约才算数；每次交付都记下谁交的、什么时候、以哪种执行者交的。
不同执行者交付的方式不同：

| 执行者 | 怎么交付 | 核实什么 |
|---|---|---|
| `session` | 会话调 `flow_submit {output}` | 契约 |
| `agent` | 主会话派子 Agent 去做，拿到结果后 `flow_submit` | 契约；有一个按约定命名的子 Agent 已经答完 |
| `program` | 应用的代码返回 | 契约 |
| `external` | 外面认领后 `flow.submit` | 契约；租约 |
| `subflow` | 子运行结束 | 子流程声明的 `result` |

## 子 Agent 站

```yaml
  - id: review
    executor: {kind: agent, agent_type: reviewer, model: claude-haiku-5-5}   # agent_type、model 可选
    skill: review
    output: {type: object, required: [verdict]}
    exit: [{kind: output}]
```

主会话收到这个工作项时，协议会让它用 `Agent` 工具派一个子 Agent，`description` 是流程定下的
`flow:<运行>/<站>/<工作项>`。主会话 `flow_submit` 时，应用去主会话的任务里找这个名字的子 Agent：

- 有一个已经答完：收下，记下子 Agent 的任务、它的会话和用量。
- 还在跑：不收，等它答完再交。
- 一个都没有、都失败了：不收。主会话自己做完的不算子 Agent 站的交付。

所以写子 Agent 站时，skill 写给子 Agent 看（它拿到的是主会话转交的任务）；要子 Agent 用别的模型，写在 `model`。

## 子流程：`result` 与 `children`

子流程站给每个工作项开一个子运行，子运行结束就是交付。子流程自己声明它交给父运行什么：

```yaml
# 子流程 review-one
name: review-one
inputs: {path: {schema: {type: string}}}
result:
  from: {path: "${inputs.path}", verdict: "${stations.decide.output.verdict}", notes: "${stations.decide.output.notes}"}
  schema: {type: object, required: [verdict], properties: {verdict: {enum: [approve, reject]}}}
stations:
  - ...
```

- 写了 `result`：子运行结束时按 `from` 解析引用（能引用 `run`、`subject`、`inputs`、`stations`、`output_dir`），按
  `schema` 校验，交给父工作项的就是这个值。不合 schema，父工作项失败，写明哪里不对。
- 没写：交的是 `{run, stations: {<站>: 输出}}`。
- 子运行取消，父工作项失败。

父流程里，子流程站的出口写 `children`，`require` 定要几个子运行交付：

```yaml
  - id: each
    foreach: "${stations.list.output.files}"
    executor: {kind: subflow, flow: review-one, inputs: {path: "${item}"}}
    exit: [{kind: children, require: all}]     # all（缺省）| any | 至少几个，比如 3
```

- `all`：每个子运行都要交付，有一个失败这一站就不过。
- `any` 或一个数：允许个别失败，等所有子运行都结束后数交付了几个，够数就过；失败的子运行不再单独算这一站不过。
- 后面的站引用 `${stations.each.output}`：`foreach` 站是按项排的数组，失败的那项是 `null`。
