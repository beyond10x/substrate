# UX trend

One row per round. The values come from that round's `round.json`.

| Round | Date | axe serious | Tasks completed | Article top 390x844 | Largest block | Findings fixed |
|---|---|---|---|---|---|---|
| R-01 | 2026-10-27 | 1 → 0 | 2 → 6 of 6 | 381 → 256 px | 864 → 241 words | UXF-001 to UXF-006 |
| R-02 | 2026-11-22 | 90 → 0 | 12 → 12 of 12 | 90 → 90 px | 235 → 137 words | UXF-008, 009, 013, 015 |

R-01 measured the site before its port to Docusaurus, and R-02 the ported one. Between the two
rows only the click counts and the largest block compare. R-02 counts 22 routes at 5 viewports
in the dark theme and each task at 2 viewports; its article top is the top of the article, where
R-01's was the top of the first heading (186 px in R-02).

## R-02 in detail

The measurements are in [rounds/R-02/round.json](./rounds/R-02/round.json), the change set in
[triage](./rounds/R-02/triage.md) and the layout decision in
[decision](./rounds/R-02/decision.md).

| Gate at 1280x800 and 390x844, per theme | Before | After |
|---|---|---|
| horizontal page scroll | 0 | 0 |
| elements running past their box | 0 | 0 |
| always-visible elements not in view | 21 | 21 |
| axe serious or critical | 36 | 0 |
| first Tab not on the skip link | 0 | 0 |
| tasks the screen-only bot completes | 12 of 12 | 12 of 12 |

The 21 are the theme toggle of the 21 docs pages at 390x844: it is behind the menu below 997 px
(UXF-020, a known loss, filed with the Docs System).

| Measure | Before | After |
|---|---|---|
| links told from their text by colour alone, per viewport and theme | 73 on 18 pages | 0 |
| viewports where the start page stays dark with light asked | 5 of 5 | 0 of 5 |
| code blocks / table headers at 1280x800 | 13.12 / 10.88 px | 14 / 14 px |
| pages naming a superseded release, bundle or tag | 7 | 0 |
| largest text block | 235 words | 137 words |
| clicks per task, six tasks at two viewports | 1 to 2 | 1 to 2 |
| widest overflow of a matrix inside its scroll region at 390x844 | 614 px | 614 px |
| highest page at 390x844 | 25 screens | 25.7 screens |

The pages got higher at 390x844 because code blocks are larger. Verdict chips stay at 11.23 px:
at 14 px the fact matrix would run 382 px past its scroll region where it runs 269 px now.

Decision 1 was answered no: this site keeps no copy of the Docs System's section list. UXF-010,
011, 020 and 021 are the Docs System's and are filed there; UXF-008 and UXF-015 are held here by
this site's stylesheet until the Docs System carries them. Eleven findings stay open for the next
round: UXF-010 to UXF-012, UXF-014, UXF-016 to UXF-019 and UXF-021 to UXF-023.

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

Two commands hold the site to the ratchets in [baseline.json](./baseline.json) on every merge,
without a browser:

- `cargo xtask check-docs` checks the largest and the median text block of each page from the
  Markdown, and needs no Node toolchain;
- `cargo xtask docs-provenance` checks the click count of each task in [tasks.json](./tasks.json)
  from the landing page of the built site.

`check-docs` also checks the finding records and holds the derived tables to the specification. The browser measurements above are taken once per round, outside the repository.
