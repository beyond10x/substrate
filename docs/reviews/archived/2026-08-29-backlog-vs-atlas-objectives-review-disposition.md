# Disposition: backlog against atlas objectives

**Date:** 2026-08-29 · **Review:** [the review](2026-08-29-backlog-vs-atlas-objectives-review.md) ·
**Operator instruction:** "good, apply this" (2026-08-29).

| finding | disposition | evidence |
|---|---|---|
| O1's exit evidence had no story | **created** `story:ledger-rows-carry-the-declared-grant` (draft, no epic; tags `o1`, `ledger`, `trust`) | `.engineering/planning/story/ledger-rows-carry-the-declared-grant.md` |
| secrets without egress unlock no vendor-harness run | **created** `story:destination-bound-egress` (draft; `decomposes` byte-plane epic, `depends_on` secret slots); epic body lists it | `.engineering/planning/story/destination-bound-egress.md`, `epic/byte-plane-completion.md` rev 3 |
| the Docker entry gate's structural test does not wait for phase 4 | **split**: `story:driver-port-carries-no-host-types` (draft, no `depends_on`); `story:docker-driver-entry-gate` re-scoped to the two phase-5-bound halves | both story files; `docker-driver-entry-gate.md` rev 3. The original story's `depends_on` edges stay — relations are machine-owned and no un-relate verb exists |
| `signed-daemon-image`, `pty-sessions`, `network-session-authority` have no consumer at HEAD | **stay `draft`**, ranked last; no status moved | `protocol artifact board` |
| hygiene stories move no objective | **kept** as stories under `epic:release-hardening`; task-sized, done in passing | — |
| add O4 to `AGENTS.md` § *Serves* | **default taken: not added** — an atlas grounding change is the operator's | — |
| harness → substrate arrow: embed vs daemon+wire | **default taken: embedding stays**; `signed-daemon-image` stays draft until atlas decides | `harness/crates/harness-substrate/Cargo.toml:26-30` |
| phase 4 exits without PTY / network authority? | **default taken: no roadmap change** | `ROADMAP.md` phase 4 |
| `review-result` cannot be authored through the CLI | **filed** in `engineering-protocols` as a story (see the store there) | — |

Store after disposition: `protocol artifact validate` → 16 artifacts, valid. Proposed working
order, from the review: ledger-grant → CI gate → secret slots + egress → bundle artifact →
hygiene → driver-port test → daemon image → PTY → network authority.
