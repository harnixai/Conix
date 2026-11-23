---
name: xforge-propose
description: 写提案并列出规格增量：说明为什么改、改什么，把对规格的改动登记成增量。
---

This station writes the proposal of the current change and records what it does to the specs.
The change was opened for you; `xflow show` tells its id, risk and which documents are written.

1. Read the current specs: `xspec list`, then `xspec list <capability>` or `xspec get <id>` for the ones you may touch. Read the code that the change concerns before you decide what it does to the specs.
2. Write `xforge/changes/<id>/proposal.md` with Write or Edit (the file starts as a template marked `<!-- xforge:template -->`; replace the whole content, the marker included). Sections: why (the problem, in a few sentences), what changes (observable behaviour), what does not change. No design here.
3. Record the effect on the specs with commands, one requirement at a time. Never edit `delta.yaml` or `specs/` yourself.
   - `xspec add <capability> "<title>" "<text>"` for new behaviour. A requirement states one testable thing; the id (`<capability>/R<n>`) is assigned for you.
   - `xspec modify <id> "<text>" [--title T]` to change an existing requirement.
   - `xspec remove <id>` to retire one. `xspec revert <id>` takes an entry back out of the delta.
4. Check yourself: `xspec check` and `xflow check structure --station propose`. Fix what they report.
5. In a quick change there is no design station: also register the work, so the later `unclaimed` gate knows what you may touch: `xflow plan add <id> "<title>" --paths src/a/**,tests/a/** [--criteria "a;b"]`.
6. Hand in the output `{ "summary": "<one or two sentences: what this change is and its effect on the specs>" }`.

The gates `structure` and `spec-delta` judge this station: the proposal must be written and the delta must be non-empty and well formed.
