# Review: the planning-store backlog against atlas O1–O6

**Status:** closed by disposition 2026-08-29 · **Date:** 2026-08-29 · **Subject:** the 13 artifacts in `.engineering/planning/` at `e3f0a28`

Reviewed against `atlas/ROADMAP.md` (objectives O1–O6, arrows table),
`atlas/AGENTS.md` § *Grounding* ("a change that moves none of the named objectives is a question
for the operator, not a task"), `atlas/architecture/adr/0003-substrate-is-public.md`, and what the
consumers actually pull from substrate at HEAD. Substrate's `AGENTS.md` § *Serves* names **O1 only**.

## What the consumers pull today

| fact | evidence |
|---|---|
| harness embeds the driver crate; it does not talk to the daemon | `harness/crates/harness-substrate/Cargo.toml:29-30` — `substrate-host`, `substrate-wire` at `tag = "0.2.1"`; the comment at `:26-30` says substrate's README wants the daemon artifact + wire, "this imports the driver … because it needs no deployment" |
| harness uses execs, machine facts, workspaces and files — nothing else | route strings in `harness-substrate/src`: `/v1/execs` ×9, `/v1/machine` ×7, `/v1/workspaces` ×4, `…/files/{path}` ×4 |
| harness uses **no** session, PTY, secret slot or TCP | `grep -rnw -i 'pty\|sessions\|/v1/sessions' harness-substrate/src` → 0 lines; `secret`, `tcp` → 0 files |
| no copy of the 0.4.0 bundle in harness | `find harness -path '*substrate-wire*'` → nothing; plan 04's "Agent consumes an exact copy" has no verified consumer at HEAD (**unverified** elsewhere — metaharness not searched for a copy) |
| atlas names as undone exactly: stable packaging, signing, digest pinning | `atlas/architecture/adr/0003` § *Decision*, citing `substrate/AGENTS.md:129` |
| O1's cross-seam evidence is open | `atlas/ROADMAP.md` O1 row: "connectors grants and identity audiences: not yet verified across a seam"; exit evidence: "every effectful call in a run's record is attributable to a declared grant; a call outside it is a named refusal" |

## Verdict per artifact

| artifact | objective it moves | verdict |
|---|---|---|
| `epic:release-hardening` | **O4** (released foundation seams; ADR 0003 names packaging/signing as undone) more than O1 | makes sense; **mis-grounded** — substrate's `## Serves` names O1 only. Either O4 joins `## Serves`, or the epic is honest that it is O1-adjacent hygiene |
| `story:ci-runs-the-full-gate` | O1 — the confinement claims are a fence only if the gate runs; O6 — a component that records what it did | **yes, first.** Caveat: a hosted runner proves the *portable* lane (refusals), not the *delegated* lane (confinement). The O1-relevant half needs a self-hosted runner; the story's open question is the O1 question |
| `story:agents-md-matches-the-scripts` | none directly (atlas § *Evidence*: claims carry their source) | keep; task-sized, objective-neutral. Cheap, do it in passing |
| `story:pinned-rust-toolchain` | none | keep; task-sized. Note harness compiles substrate crates at `rust-version = 1.97` too — a bump here is a bump there |
| `story:status-md-re-observed` | none directly; `atlas/scripts/check-map.sh` greps evidence files, so drift here becomes an atlas red | keep; task-sized |
| `story:signed-daemon-image` | O4 — *if* the harness → substrate arrow is meant to become daemon + wire | **no consumer at HEAD.** harness embeds the driver by choice (`Cargo.toml:26-30`). Whether that arrow moves to the daemon is an **atlas decision**, not a substrate story. Keep `draft` behind the bundle |
| `story:contract-bundle-oci-artifact` | O4 — "products consume released foundation contracts"; identity/cloud "publish owner-signed evidence against released Substrate seams" (O4 row, phase 8) | right shape, **no consumer by bytes at HEAD** (harness uses the `substrate-wire` crate, not the JSON bundle). Ahead of the image; still a bet until a consumer names the bundle |
| `epic:byte-plane-completion` | substrate's own phase-4 exit; atlas O1/O3 only through the vendor-harness case below | partially. See the three stories |
| `story:sealed-secret-slots` | **O1** — credential authority declared before the run, refused by name; **O3/O4** — a vendor harness (`codex`, `claude`, which need model credentials) run under substrate confinement, which design 05 § *Progress* says is refused "until the required secret and egress capabilities exist" | **yes — but incomplete on its own.** Secrets without egress unlock no vendor-harness run. The backlog holds secrets and not egress (my epic put egress out of scope). **Gap: `destination-bound egress` (design 04 § 6) belongs beside it** |
| `story:pty-sessions` | none named in atlas or in any consumer's code | phase-4 exit criterion by substrate's `ROADMAP.md`, **zero consumer pressure**. Substrate's own rule for later phases — pressure demonstrated before implementation — argues for deferring it. Question for the operator: does phase 4 exit without PTY? |
| `story:network-session-authority` | O5 at most (a person "sees how the work is being done") — speculative | same as PTY, weaker. Keep `draft`, last |
| `epic:container-driver-entry` / `story:docker-driver-entry-gate` | O1 invariant 4 (clients never branch on the driver) — the structural test; phase 5 otherwise | the **entry gate does not depend on phase 4** — only Docker *code* does. My `depends_on` edges to the three byte-plane stories are wrong for the test + design-04 section half. Proposal: split the entry gate out as an unblocked story; leave the Docker driver behind phase order |

## What is missing, measured against O1's exit evidence

1. **`story:ledger-rows-carry-the-declared-grant`** — an exec or session operation row records the
   delegated principal and grant reference the caller presents, tamper-safe, separate from the
   immediate actor (`docs/design/06-authentication-secrets-and-trust.md` § 2), so atlas O1's
   "attributable to a declared grant … verified across a seam" can be checked from substrate's
   record. This is the one item that moves the objective substrate actually serves, and it is not
   in the backlog.
2. **`story:destination-bound-egress`** — design 04 § 6 apertures; pairs with secret slots; the
   two together are what unlocks a confined vendor harness (O3, O4).
3. **The hosted trust-envelope verifier** (design 06 § 1, atlas ADR 0015 lineage; phase 7) — the
   O4 phase-8 prerequisite. Out of scope by phase order; named so nobody thinks it was forgotten.

## Proposed order, if the findings are accepted

1. new `ledger-rows-carry-the-declared-grant` (O1) · 2. `ci-runs-the-full-gate` · 3.
`sealed-secret-slots` + new `destination-bound-egress` · 4. `contract-bundle-oci-artifact` · 5. the
three hygiene tasks · 6. `docker-driver-entry-gate` (test + design section, unblocked) · 7.
`signed-daemon-image` (after the atlas arrow decision) · 8. `pty-sessions` · 9.
`network-session-authority`.

## Decisions this review leaves with the operator

| decision | default if nobody answers |
|---|---|
| add **O4** to substrate `AGENTS.md` § *Serves*? | not added; release-hardening stays O1-adjacent |
| harness → substrate arrow: keep embedding the driver crate, or move to daemon + wire (an atlas ADR)? | keep embedding; `signed-daemon-image` stays `draft` |
| does phase 4 exit without PTY and network authority (substrate `ROADMAP.md` edit)? | no change; both stay `draft`, ranked last |
| create the two missing stories now? | not created until asked |
| split the Docker entry gate from its phase-4 dependency? | not split |

No status was moved by this review.
