# R-01 triage

Every S1 finding enters the change set. The S2 findings fill the rest in severity order, up to five
changes.

| Finding | Severity | Change | Target metric |
|---|---|---|---|
| UXF-003 | S1 | adapters, contracts and release-notes pages, `confine run` and the 0.12.0 status | 4 tasks from not reached to at most 2 clicks |
| UXF-004 | S1 | focusable, labelled table regions | axe serious 1 to 0 |
| UXF-001 | S2 | grouped page list with `aria-current` | 0 to 2 current-page markers per page |
| UXF-002 | S2 | closed page-list disclosure under 800 px | article top 381 to under 300 px at 390x844 |
| UXF-005 | S2 | bundle history split by bundle | largest block 864 to under 300 words |

UXF-006 came out of the after-change probe. It was fixed in the same set, because the axe gate
admits no serious violation. UXF-007 is input the site cannot control, so it is a known loss.
