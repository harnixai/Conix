# Gates

A gate is a **deterministic check** among a station's exit conditions: `exit: [{kind: gate, gate: <name>}]` names
`gates/<name>.yaml` of the pack. Once a session has handed in, the dispatcher runs the station's gates; the station
passes only when all of them do. The model saying "done" does not count; what the gate reads in the files does.

## Decide what passes, then pick the kind

Before writing a gate, answer one sentence: **what does this station deliver, and what makes it acceptable**. Start
from the station's `output` contract and its deliverables (files), and write that as conditions a machine can
judge. What it cannot judge ("is it well written") belongs to a review station or a `confirm`, not a gate.

| To check | Use | Example |
|---|---|---|
| a file exists, is not empty, has no template text left | `check: files` | the report is written, with no `TODO` |
| a file has a format, its fields are there | `check: schema` | `report.json` has `verdict` and `findings` |
| a text must / must not contain something | `check: text` | a "## Conclusion" section, nothing like a key |
| an earlier station's output meets a condition | `check: output` | `coverage` ≥ 0.8 |
| nothing changed after it was checked | `check: sealed` | on the last station |
| counting, comparing files, rules over many files | `kind: script` | every requirement id appears in a test |
| a third-party tool (tests, lint, build) | `kind: command` | `cargo test`, `npm run lint` |
| a check the application brings | `kind: script` + `function:` | a plugin's function |

Use `check` whenever it can be declared: no code, the steadiest result. `script` next: read-only, sandboxed, its
evidence kept by the runtime. `command` last: it runs anything on the machine, and runs only where the application
turned command gates on and the project is trusted.

## Fields every gate has

```yaml
name: report-ready        # the file's name: gates/report-ready.yaml
title: Report written     # shown in the application and in the station's message
kind: check               # check | command | script
fail_on: warn             # optional: a pass with a warning (severity: warn) does not pass; absent, the verdict stands
check: files              # then what the kind takes, below
paths: ["${output_dir}/report.md"]
```

Paths are relative to where the run works (the project, its worktree, or the run's own directory) and may use
`${output_dir}`, `${inputs.<k>}`, `${stations.<station>.output.<field>}`. An absolute path must be inside that
directory; `..` and links out of it are refused (that is an `error`).

## `check`: declared checks

### `files`

```yaml
name: report-ready
kind: check
check: files
paths: ["${output_dir}/report.md", "${output_dir}/data/*.json"]   # each path or glob must have at least one file
not_containing: ["TODO", "(fill in)"]                              # optional: none of these may appear
non_empty: true                                                    # default true: an empty file does not count
```

Globs take `*`, `?` and `**`. A path matching no file is a finding (`exists`), so a mistyped glob does not "always
pass" — it fails, and `flow_gate_try` shows it at once.

### `schema`

```yaml
name: report-shape
kind: check
check: schema
file: "${output_dir}/report.json"     # may be a glob: every file it matches must satisfy the schema
as: json                              # json | yaml | jsonl (read as an array) | frontmatter; absent: by extension
schema:
  type: object
  required: [verdict, findings]
  properties:
    verdict: {enum: [approve, reject]}
    findings: {type: array, items: {type: object, required: [path, message]}}
```

`frontmatter` reads the YAML between the leading `---` lines of a Markdown file. A file that does not parse is a
finding (`parse`), not an `error`.

### `text`

```yaml
name: report-sections
kind: check
check: text
file: "${output_dir}/report.md"
matches: ["(?m)^## Conclusion", "(?m)^## Risks"]                  # each must match
not_matches: ["(?i)api[_-]?key\\s*[:=]", "sk-[A-Za-z0-9]{20,}"]   # none may match; one finding per match, with its line
```

Regular expressions are Rust `regex` syntax: `(?m)` makes `^` and `$` work per line, `(?i)` ignores case; in YAML
double quotes a backslash is written twice.

### `output`

```yaml
name: enough-coverage
kind: check
check: output
value: "${stations.test.output}"       # absent: the whole reference context
schema: {type: object, required: [coverage], properties: {coverage: {type: number, minimum: 0.8}}}
```

`output` looks at the value handed in, not at files: for a stricter requirement than the contract, or for checking
an earlier station's output from a later one.

### `sealed`

```yaml
name: untouched
kind: check
check: sealed
```

Every file the run's earlier gates read (the sha256 in their evidence) is still what it was; one changed or gone is
a finding. Put it on the last station, so nothing is changed after it passed its gates. It is judged only in a run
and cannot be tried.

## `script`: JavaScript in the project

```yaml
name: req-covered
kind: script
script: req-covered            # runs scripts/req-covered.js
params:                        # optional: given to the script, references resolved
  spec: "${output_dir}/requirements.md"
  tests: "tests/**/*.rs"
timeout_seconds: 30            # default 30, at most 600
```

```js
// scripts/req-covered.js — every requirement id (REQ-<n>) appears in a test at least once
export default function (input, flow) {
  const spec = flow.read.text(input.params.spec);
  if (spec == null) {
    // Nothing to judge is not a failure of the work: it is an error, for a person to look at
    return { verdict: "error", summary: `${input.params.spec} does not exist` };
  }
  const ids = [...new Set(spec.match(/REQ-\d+/g) || [])];
  const tests = flow.glob(input.params.tests).map((p) => flow.read.text(p)).join("\n");
  const missing = ids.filter((id) => !tests.includes(id));
  return {
    verdict: missing.length ? "fail" : "pass",
    summary: `${ids.length - missing.length}/${ids.length} requirements have tests`,
    findings: missing.map((id) => ({ rule: "req-covered", severity: "error", path: input.params.spec, message: `${id} has no test` })),
    metrics: { requirements: ids.length, missing: missing.length },
  };
}
```

- An ES module whose default export is `function (input, flow)`; it may be `async`.
- `input`: `gate`, `run`, `flow`, `station`, `round`, `inputs`, `outputs` (each station's output), `output_dir`,
  `workspace`, `params`.
- `flow`, read-only: `flow.read.text / json / yaml / jsonl / frontmatter(path)` (`null` when the file is not there;
  a parse error throws), `flow.glob(pattern)` (an array of paths), `flow.stat(path)` (`{size, sha256}` or `null`),
  `flow.schema.validate(value, schema)` (`{ok, errors}`), `flow.log(...)` (into the station's `gate-<gate>.log`).
- No `import`, no commands, no network; paths are held to the working directory: reading outside it is an `error`,
  even if the script catches it.
- Return `{verdict, summary?, findings?, metrics?}`; a throw, a timeout or anything else is an `error`. **Do not
  report evidence**: the runtime records the hash of every file the script read.
- Do not trust what `input.outputs` claims ("all tests passed"): read the files. That is what a gate is for.

With `function: <name>` instead of `script:`, the application's (a plugin's) function of that name runs, given the
same `params`.

## `command`: third-party tools

```yaml
name: unit-tests
kind: command
command: cargo test --quiet
cwd: "."                       # optional: relative to the working directory
timeout_seconds: 600           # default 600; past it the whole process group is killed, an error
env: [CARGO_HOME, RUSTUP_HOME] # optional: variables passed through; none by default (PATH, HOME, the locale are always there)
result: json                   # optional: judged by the result the command prints, see below
```

- Exit status 0 is `pass`, any other `fail`; the end of the output is the `summary`, all of it in the station's
  `gate-<gate>.log`.
- Not starting, running out of time, being killed by a signal: `error`.
- The run's context is in the environment: `FLOW_RUN`, `FLOW_STATION`, `FLOW_WORKSPACE`, `FLOW_CONTEXT_FILE` (the
  whole reference context as a JSON file), and `FLOW_CONTEXT` when it is small. Do not splice `${...}` into the
  command line.
- The application's secrets (the model's API keys…) are never passed, even when named in `env`.
- `result: json`: the command prints the result JSON (`{verdict, summary, findings, metrics}`) on standard output
  — all of the output, or its last line that is not blank; printing none is an `error`. Good for wrapping a linter:
  turn its report into findings.

## What the result does

Every kind gives the same result: `{verdict: pass | fail | error, summary, findings: [{rule, severity, path, line,
message}], metrics}`; the files `check` and `script` read are kept as evidence with their hashes.

- **`fail`**: the delivery is not acceptable, and the station must change it. With `on_unmet: {rework_to: <station>,
  max: <times>}` the run goes back to that station by itself, and the new round's session is told every finding;
  without it, the station waits as unmet for a person.
- **`error`**: the gate came to no conclusion (a file not readable, a command not starting, a script throwing). It
  is not reworked by itself — changing the delivery does not fix it. Once a person has looked and fixed it, the
  station's gates are run again (ask the agent: `flow_retry` without `task`).
- Findings say **which file, which line, what is wrong**: they are the list of changes the session gets on a rework.

## Writing a gate that holds

1. Write down what passes (the sentence above) and pick the kind.
2. Write the gate (`flow_part_set {kind: gate}`; a script is `{kind: script}`), with the right references:
   `${output_dir}` or a path in the project.
3. **Try it**: `flow_gate_try` on a sample that should pass (`files`) must give `pass`; on one that should not, `fail`,
   with findings that name the problem. A gate that passes both checks nothing. Give `outputs` when it reads earlier
   stations' outputs, `inputs` when it reads the start's inputs.
4. Put it in the station's `exit`; for automatic rework add `on_unmet: {rework_to: <station>, max: 2}`; consider
   `sealed` on the last station.
5. `flow_validate`, `flow_dry_run` (gates are mocked there: `mocks.gates.<name>: false` shows the path a failure takes).

## Pitfalls

- Writing "well done" as a regular expression: what cannot be judged goes to a review or a `confirm`.
- Checking the model's words instead of the files: `check: output` checks the value handed in; to verify, read files.
- Answering `fail` when it cannot judge: the session would go and change a delivery that is fine. Nothing to read,
  nothing to judge: `error`.
- A `command` gate needing what only a developer's machine has (a tool installed globally, a variable): name the
  variables in `env`, keep the tools in the project.
- A regular expression not escaped for YAML: `\d` is `"\\d"` in double quotes, or `'\d'` in single quotes.
