---
name: xforge-design
description: 写设计并拆成工作包：做法与取舍、对每条原则的对照、决定与风险进台账。
---

This station turns the proposal into a design and into work packages. Use `xflow show` to find the current change.

1. Read `proposal.md`, the spec delta (`xspec delta`) and the code it touches. Read `xforge/constitution.md`: each principle is a line `- **P<n>** …`.
2. Write `xforge/changes/<id>/design.md` with Write or Edit (replace the template, the marker included): the approach, the alternatives you rejected and why, and a section that names every principle (`P1`, `P2`, …) with how the design meets it. The `constitution` gate fails when a principle is not mentioned.
3. Put what must be remembered in the ledgers, with commands:
   - `xflow ledger add decisions "<text>"` for a choice made, `xflow ledger add risks "<text>"` for what could go wrong, `xflow ledger add findings "<text>"` for facts discovered.
   - Every entry starts `open`. Settle each one before you leave: `xflow ledger set <kind> <id> accepted|rejected|resolved|deferred`. The `ledgers` gate fails while any entry is `open`.
4. Split the work into packages that can be done and verified one at a time: `xflow plan add <id> "<title>" --paths src/a/**,tests/a/** [--verify <name>] [--criteria "criterion one;criterion two"]`. `--verify` names a command of `xforge/manifest.yaml` (`xflow plan list` shows packages; `xforge doctor` shows the manifest problems). Paths must cover everything the package changes, including its tests: the `unclaimed` gate fails on a changed path no package claims.
5. Check yourself: `xflow check structure --station design`, `xflow check constitution`, `xflow check ledgers`.
6. Hand in `{ "summary": "<the approach in two or three sentences and how many packages>" }`.
