# UX trend

One row per round. The values come from that round's `round.json`.

| Round | Date | axe serious | Tasks completed | Article top 390x844 | Largest block | Findings fixed |
|---|---|---|---|---|---|---|
| R-01 | 2026-10-27 | 1 → 0 | 2 → 6 of 6 | 381 → 256 px | 864 → 241 words | UXF-001 to UXF-006 |

## R-01 in detail

The measurements are in [rounds/R-01/round.json](./rounds/R-01/round.json), the change set in
[triage](./rounds/R-01/triage.md) and the layout decision in
[decision](./rounds/R-01/decision.md).

| Gate at 1280x800 and 390x844 | Before | After |
|---|---|---|
| horizontal page scroll | 0 | 0 |
| elements running past their box | 0 | 0 |
| header links outside the viewport | 0 | 0 |
| axe serious or critical | 1 | 0 |
| first Tab not on the skip link | 0 | 0 |
| tasks the screen-only bot completes | 2 of 6 | 6 of 6 |

| Task | Clicks before | Clicks after |
|---|---|---|
| install the CLI | 2 | 2 |
| what a refusal means | 2 | 2 |
| choose an adapter for Kubernetes | not reached | 1 |
| what changed in 0.12.0 | not reached | 1 |
| the `confine run` flags | not reached | 2 |
| the contract families | not reached | 1 |

UXF-007 is a known loss: the release-note bullets come from `CHANGELOG.md`.

## The per-merge gate

`cargo xtask check-docs` holds the site to the ratchets in [baseline.json](./baseline.json) on
every merge, without a browser. It checks:

- the click count of each task in [tasks.json](./tasks.json) from the landing page;
- the largest and the median text block of each page.

The same command also checks the finding records and holds the derived tables to the
specification. The browser measurements above are taken once per round, outside the repository.
