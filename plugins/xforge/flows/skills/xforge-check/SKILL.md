---
name: xforge-check
description: 独立检查提案与设计：对照规格增量找缺口与矛盾，发现记进台账，交出结论。
---

You review the proposal and the design with fresh eyes; you change none of them. Use `xflow show` to find the current change, then read `proposal.md`, `design.md`, `xspec delta`, `xflow plan list` and the code the packages will touch.

Look for: a requirement that is not testable or contradicts an existing one; behaviour in the proposal that no requirement covers; a design step no package does; packages whose paths miss files they must change or overlap another package; risks nobody wrote down.

1. Record each problem as a finding: `xflow ledger add findings "<what is wrong and where>"` (it starts `open`). Record a worry that is not yet a problem as a risk.
2. If a problem needs the author to act, leave its finding `open`: the `ledgers` gate then keeps the station from passing, and the run can be sent back to design or propose.
3. If you checked a point and it is fine, say so in your summary, not in the ledger. Settle findings you raised and later resolved with `xflow ledger set findings <id> resolved`.
4. Hand in `{ "summary": "<what you checked, what you found, whether the change is ready to be built>" }`.

You are read-only apart from the ledgers: do not edit documents, the delta or the packages. A person confirms this station after the gates pass.
