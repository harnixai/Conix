---
name: xforge-verify
description: 验证并写验证记录：对照规格增量与验收标准，如实写下怎么验证的、结果如何。
---

This station checks that the change does what the proposal and the spec delta say, and records the evidence.

1. Read `proposal.md`, `xspec delta` and `xflow plan list` (criteria and summaries of each package).
2. For every added or modified requirement, find the evidence: a test that exercises it, a command and its output, or the code that implements it. Run what can be run; do not report a check you did not run.
3. Settle the ledgers: `xflow ledger list` must show no `open` entry (`xflow ledger set <kind> <id> resolved|accepted|deferred` with the reason in your summary).
4. Write `xforge/changes/<id>/verification.md` with Write or Edit (replace the template, the marker included): per requirement, how it was verified and the result; anything not verified, said plainly.
5. Check yourself: `xflow check structure --station verify`.
6. Hand in `{ "summary": "<verified, not verified, remaining risk>" }`. A person confirms this station; when it is confirmed the archive step merges the delta into the specs.

If verification shows the change is wrong, say so in the summary and do not write a passing record: the person sends the run back.
