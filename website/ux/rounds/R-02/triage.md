# R-02 triage

## Caveats

- R-01 was measured on the site before its port to Docusaurus (R-01's `round.json` was committed on
  2026-10-27 at 10:14, the port at 13:39 the same day). Only the click counts and the largest text
  block compare with R-01. The axe count and the always-visible count describe the ported site for
  the first time; they are not a drift since R-01's fixes.
- No person took part. The trial is the screen-only bot on the six tasks of
  [tasks.json](../../tasks.json) and one rotating task, "deny an executable". The bot knows every
  page before it starts, so its counts are the best case.
- 390x844 is a narrow desktop Chromium window, without touch and without a mobile user agent.
- axe ran on the load state of each page. Its undecided results were not kept, so the contrast of
  table text is not verified (336 undecided cells on concepts/adapters).
- "Sessions" below are the seven tasks.

## Did the last change set move its metrics?

| R-01 change | Target | Now |
|---|---|---|
| UXF-003 four tasks reachable | at most 2 clicks | holds: 1, 1, 2 and 1 clicks, equal to `baseline.json` |
| UXF-005 bundle history split | largest block under 300 words | holds: 235 words on `main`, 87 after the prose edit of 2026-11-22 |
| UXF-004, UXF-006 axe serious 0 | 0 | not comparable: the port replaced the page list and the tables' markup; their tests still pass |
| UXF-001 current-page marker | 2 markers per page | marker present; on 5 of 21 pages it is outside the list's box (UXF-010) |
| UXF-002 article top under 300 px | under 300 px | h1 at 186 px on 20 pages, 134 px on one |

## Findings and priority

`priority = severity weight × frequency × confidence ÷ cost`, with S1 = 9, S2 = 3, S3 = 1, cost
S = 1, M = 2, L = 4, and confidence 2 where two sources agree.

| Finding | Severity | Sessions | Confidence | Cost | Priority |
|---|---|---|---|---|---|
| UXF-008 links told by colour alone | S1 | 7 of 7 | 2 | S | 18.0 |
| UXF-009 superseded releases named as current | S1 | 3 of 7 | 2 | M | 3.86 |
| UXF-013 start page ignores the light theme | S2 | 7 of 7 | 2 | M | 3.0 |
| UXF-015 text under 14 px | S2 | 4 of 7, both anchor tasks | 1 | S | 1.71 |
| UXF-011 no wayfinding deep in a page on a phone | S2 | 4 of 7, one anchor task | 2 | M | 1.71 |
| UXF-010 page list taller than its box | S2 | 2 of 7 | 2 | S | 1.71 |
| UXF-016 no search | S2 | 2 of 7 | 2 | M | 0.86 |
| UXF-019 two first steps | S3 | 4 of 7 | 1 | S | 0.57 |
| UXF-017 pages have several names | S3 | 7 of 7 | 1 | M | 0.5 |
| UXF-012 matrices hide two adapters | S2 | 1 of 7 | 2 | M | 0.43 |
| UXF-018 Install shows only the source build | S2 | 1 of 7, an anchor task | 1 | S | 0.43 |
| UXF-021 phone menu by keyboard | S3 | 1 of 7 | 1 | S | 0.14 |
| UXF-022 release-notes lead | S3 | 1 of 7 | 1 | S | 0.14 |
| UXF-014 brand link opens a new tab | S3 | 0 of 7 | 2 | S | 0 |
| UXF-023 hidden-line markers in an example | S2 | 0 of 7 | 2 | S | 0 |

UXF-020 is a known loss by this round's reading: below 997 px the menu button is the
always-visible control, and the theme toggle costs one action there.

## Change set (five at most)

Every S1 finding enters. The rest fill the set in priority order; a tie goes to the finding on an
anchor task.

| Finding | Change | Target metric |
|---|---|---|
| UXF-008 | links in article text are underlined | axe serious or critical 90 to 0 |
| UXF-009 | every release, install version, bundle number and tag link is the current one, the fact list has all 14 facts, and `check-docs` holds each to its source | pages naming a superseded one 7 to 0 |
| UXF-013 | the start page has light colours and follows the chosen or the system's theme | viewports showing dark with light asked 5 to 0 |
| UXF-015 | code blocks and table headers at least 14 px; verdict chips too where the matrices do not get wider | words under 14 px in articles 3,419 to 0 |
| UXF-011 | none here: [decision 1](./decision.md) was answered no on 2026-11-22, and the finding is filed with the Docs System | actions from a deep section to the section list stays 3 |

Six findings sit in components of the shared Docs System: UXF-008, 010, 011, 015, 020 and 021.
UXF-008 and UXF-015 are held here by this site's own stylesheet; the others are not changed here.
All six are draft stories in the planning store of the repository `docs-system` (its pull
request 46, 2026-11-22).

No change may make a measured value worse: the sideways-scroll px of the matrices (UXF-012) and
the reading loads in [baseline.json](../../baseline.json) are checked again after the set.
