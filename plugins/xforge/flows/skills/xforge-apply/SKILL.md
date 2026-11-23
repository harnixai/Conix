---
name: xforge-apply
description: 实施变更：按工作包（或快速变更的提案）改代码、验证，并用命令登记工作包的结果。
---

This station builds the change. Read the station message first: it says whether you were given one work package or the whole (quick) change.

**With a work package** (`xforge-executor`): do only that package.
1. `xflow plan list` shows it with its `paths`, `criteria` and `verify`. Change files only inside its paths.
2. Read `proposal.md`, `design.md` and `xspec delta` for what is wanted. Make the change in small steps and satisfy each criterion.
3. Verify: if the package has `verify`, run the command that `xforge/manifest.yaml` gives under that name and read its result yourself.
4. Register the outcome with a command, never by editing files under xforge/: `xflow plan set <id> done "<summary>"`, or `blocked "<why>"` when you cannot finish.
5. Hand in `{ "package": "<id>", "status": "done" | "blocked" | "needs-human", "summary": "<what changed and how it was checked>" }`.

**Without a package** (a quick change, in the main session): do the change described in the proposal and the delta, keep to the paths of the packages registered with `xflow plan list`, verify with the project's own checks, then `xflow plan set <id> done "<summary>"` for each package and hand in `{ "summary": "<what changed and how it was checked>" }`.

The gates judge the station afterwards: `integrated` (every package `done`), `unclaimed` (no changed path outside the packages' paths; the paths under xforge/ are not counted), `unit-tests` (the packages' verification commands pass). If a gate sends the run back, read `unmet` in the station message and fix exactly that.
