# Confirmations and who decides

A gate judges whether a delivery is acceptable; a confirmation, whether a person agrees. Where a person must
decide, write a `confirm` exit: a task's session has no tool to ask questions, the model does not stop to ask, and it
**cannot confirm for a person** — confirming, releasing a held station and forcing one are a person's alone, and no
tool call does them.

## A person confirms in the application (`via: person`, the default)

```yaml
  - id: sign
    title: Sign-off
    exit:
      - kind: confirm
        who: reviewer        # optional: a role, for the application to interpret (it says who is a reviewer)
        label: Owner signs   # optional: shown in the application
        timeout: 3d          # optional: no decision by then does not pass (90s, 30m, 8h, 3d, 1h30m)
    rework_to: [draft]       # where a person may send the run back after a rejection
```

- Once entered, the station waits for confirmation ("awaiting confirmation"); a person passes or rejects it in the
  application, with a reason if they like.
- A rejection is this exit not met: the station is unmet, reworked by `on_unmet` when written, else waiting for a
  person to say where to go back to.
- No decision by `timeout` does not pass either, saying "no decision within 3d".
- Every decision records who, when, which round, pass or not, and why.

## Several people, and what they approved

```yaml
      - kind: confirm
        who: reviewer
        min: 2                  # approvals needed (default 1)
        distinct: true          # one vote per person, a later one replacing theirs (default)
        reject: any             # any (default): one rejection is enough; majority: as many as min
        not: [author]           # optional: whoever delivered in this run, or started it, may not vote
        binds: [deliverables, "${data_dir}/specs/*.md"]   # default: deliverables
```

- Each decision is one vote of the round; the exit passes once `min` approvals are in ("1 of 2 approvals" shows
  meanwhile). An outside approval (`via: mcp`) counts as one vote. A rework to the station starts the count again.
- Authors may confirm their own work unless the exit says `not: [author]`; then their vote is refused, saying why.
  `separate: true` says the same as an on/off.
- **From the project's settings**: in a bundle whose `bundle.yaml` declares `settings`, `min`, `reject`, `separate` and
  an item of `not` may be `"${settings.<path>}"` (`separate: "${settings.review.separate}"`). They are read from the
  settings the run pinned when it started; a path the settings lack, or a value of the wrong kind, refuses the start.
- **What was approved stays approved.** When the exit passes, the files it binds are recorded with their digests:
  `deliverables` are those of this station and of the stations before it (in the run's directory); paths and globs may
  start at `${output_dir}`, `${data_dir}` or `${workspace}` and stay in the workspace; `binds: []` binds nothing.
  Before every later station passes they are checked again: a changed or removed file leaves that station unmet
  (`confirm-stale`, naming the file and the station that approved it). Put the file back, or rework to that station
  so it is approved again.

## An outside approval system (`via: mcp`)

When someone else must approve (a manager, legal, another team) and they are not in this application, a request is
made in an outside approval system (Feishu, DingTalk, Jira…) through MCP tools, and its state polled:

```yaml
      - kind: confirm
        who: reviewer
        via: mcp
        timeout: 3d
        mcp:
          server: feishu                          # an MCP server configured in the application
          request:                                # called once when the station is entered: make the request
            tool: approval_create
            arguments: {title: "${subject}: ${station}", approver: "${inputs.reviewer}", run: "${run}"}
          status:                                 # called every `poll` after: read it
            tool: approval_get
            arguments: {id: "${request.id}"}     # ${request.*} is what making the request gave back
          decision:                               # read from the status call's result
            path: status                          # a field path in the result
            approved: [APPROVED]
            rejected: [REJECTED, CANCELED]        # any other value: still waiting
          by: approver.name                       # optional: who decided, in the result
          link: url                               # optional: the request's link, kept for people
          poll: 5m                                # default 5m, at least 30s
```

- Arguments may refer to `${run}`, `${subject}`, `${station}`, `${inputs.*}`, `${stations.*.output.*}`; the status
  call's also to `${request.*}`.
- The request is made once a round and kept in the run: after a restart the same request is polled, no new one made.
- Making the request failing, or reading it failing 5 times in a row, makes this exit an `error`: not reworked by
  itself; a person looks, then the station's checks run again.
- The decision records who decided (`by`) and the request's id or link.
- Find out the server and the tools first: which MCP servers the application has, what their tools are called, what
  making a request takes and what reading one gives back — ask the user or read the server's description, do not
  guess.

## Forcing, and governed flows

- A person may force a station past its conditions (with a reason; what was not met is recorded), or send the run
  back to a station.
- A plugin may declare its flows **governed**: a project cannot save one of the same name to replace it, the parts it
  uses come from the plugin alone, and its runs **cannot be forced**. To change a governed flow, change the plugin.

## Which one when

| Situation | Write |
|---|---|
| the delivery can be judged by a machine | a gate (see `flow_guide {topic: "gates"}`) |
| a person looks and decides | `confirm` (`via: person`) |
| a named person must approve, maybe not in the application | `confirm` + `who` + `via: mcp` |
| one tool call needs a person's yes (writing a release directory, deleting) | a permission with `effect: ask` |
| a person releases the station before it starts | `hold: true` on the station |
