# Deliveries

Every task **hands in** an output, which counts only when it satisfies the station's `output` contract; every
delivery records who handed it in, when, and as which kind of executor. Each executor hands in its own way:

| Executor | Hands in by | What is checked |
|---|---|---|
| `session` | the session calls `flow_submit {output}` | the contract |
| `agent` | the main session has a sub-agent do it, then `flow_submit`s its result | the contract; a sub-agent named as agreed has answered |
| `program` | the application's code returns | the contract |
| `external` | claimed from outside, then `flow.submit` | the contract; the lease |
| `subflow` | the child run finishes | the child flow's declared `result` |

## Sub-agent stations

```yaml
  - id: review
    executor: {kind: agent, agent_type: reviewer, model: claude-haiku-5-5}   # agent_type and model are optional
    skill: review
    output: {type: object, required: [verdict]}
    exit: [{kind: output}]
```

The main session getting this task is told by the protocol to start a sub-agent with the `Agent` tool, its
`description` the one the flow sets: `flow:<run>/<station>/<task>`. When the main session calls `flow_submit`, the
application looks for a sub-agent of that name among the main session's tasks:

- One has answered: taken; the sub-agent's task, its session and its usage are recorded.
- One is still running: not taken; hand in once it has answered.
- None, or all failed: not taken. Work the main session did itself is no delivery of a sub-agent station.

So in a sub-agent station, the skill is written for the sub-agent (it gets the task as the main session passes it
on); a sub-agent on another model says so in `model`.

## Subflows: `result` and `children`

A subflow station starts a child run for each task; the child run finishing is the delivery. The child flow declares
what it hands its parent:

```yaml
# the child flow review-one
name: review-one
inputs: {path: {schema: {type: string}}}
result:
  from: {path: "${inputs.path}", verdict: "${stations.decide.output.verdict}", notes: "${stations.decide.output.notes}"}
  schema: {type: object, required: [verdict], properties: {verdict: {enum: [approve, reject]}}}
stations:
  - ...
```

- With `result`: when the child run finishes, `from` has its references resolved (it may refer to `run`, `subject`,
  `inputs`, `stations` and `output_dir`) and is checked against `schema`; that value is what the parent task gets.
  Short of the schema, the parent task fails, saying where.
- Without it: `{run, stations: {<station>: output}}`.
- A cancelled child run fails the parent task.

In the parent flow, the subflow station's exit is `children`, whose `require` says how many child runs must deliver:

```yaml
  - id: each
    foreach: "${stations.list.output.files}"
    executor: {kind: subflow, flow: review-one, inputs: {path: "${item}"}}
    exit: [{kind: children, require: all}]     # all (default) | any | at least a number, say 3
```

- `all`: every child run must deliver; one failing fails the station.
- `any` or a number: some may fail. Once every child run has ended, the deliveries are counted, and enough of them
  pass the station; a failed child run is no longer the station's failure by itself.
- A later station refers to `${stations.each.output}`: for a `foreach` station an array in item order, `null` where
  the child failed.
