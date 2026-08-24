# AGENTS.md — substrate

The contract for changing **this** repository. Org-wide rules — the naming convention, the
former-brand rule (atlas ADR 0001) and its four exemption categories, and the rule that renaming
anything another repo verifies is a coordinated migration with an ADR — live in `atlas/AGENTS.md`
and are not restated here.

`README.md` orients a reader and shows how to run the daemon; `STATUS.md` records observed progress
and `ROADMAP.md` ordered exit criteria. This file says what must not break.

## What this repository owns

The standalone b10x execution data plane: confined workspaces, bounded processes, workloads, images,
volumes, endpoints, leases, execution capsules, a durable operation ledger, and observed state.
Substrate **runs things and reports what it observed**.

## Invariants

Each is a claim that can be checked. Breaking one is a design change, not a refactor.

1. **Substrate is Flux-free.** No Flux crate and no Flux type may appear in any dependency kind or in
   any public or private implementation. Flux is prior art and a possible client
   (`adr/0001-substrate-is-standalone-and-flux-free.md`).
2. **No sibling-component implementation dependency**, in either direction. A consumer embeds
   substrate; substrate embeds nothing of theirs.
3. **A missing isolation or capability guarantee is a named refusal, never silent degradation.**
   Absent delegation keeps exec facts *absent*; it never manufactures an optimistic one.
4. **Drivers implement one substrate contract and expose verified capability facts.** Clients do not
   branch on driver internals — a client that reads which driver answered has made the driver part
   of the contract.
5. **Operations are durable before driver dispatch**
   (`adr/0005-operations-are-durable-before-driver-dispatch.md`).
6. **Every released contract bundle directory is immutable.** `contracts/substrate-wire/0.1.0`
   through `0.4.0` exist; `0.4.0` is the current development bundle and **every earlier directory is
   frozen** (`STATUS.md:30`, `scripts/contract_json_gate.py:283`,
   `contracts/substrate-wire/0.2.0/README.md:13`). A wire change **adds a successor bundle**; it
   never rewrites bytes in a released one. The compatibility block of a successor states its
   predecessor and its exact `adds_routes`/`preserves_routes` counts, and the checker pins them.
   **One recorded exception, 2026-08-24:** the brand rename rewrote every frozen bundle in place,
   because the former brand name is in their bytes and no successor bundle can remove it from
   them. It was an identifier rename with no semantic wire change, and each bundle was
   re-rendered by its own renderer rather than hand-edited, so each remains a reproducible fixed
   point. Immutability applies again from that commit forward; a second such rewrite needs an ADR.
7. **Every created JSON authority has exactly one schema classification and validates in the gate.**
   Unclassified JSON **fails closed**. Every JSON Schema validates offline against its declared Draft
   2020-12 meta-schema with the pinned standards validator. Immutable historical bundle bytes are
   classified *externally*, without rewriting them.
8. **Implementation follows the accepted design-closure gate** (`docs/plan/01-design-closure.md`). A
   contract or capability change beyond its named decisions and deferrals needs a design document or
   an ADR **before code**.
9. **This repository is private**, and any future b10x repository stays private unless an accepted
   architecture decision explicitly authorises otherwise.

## Safety envelope

Substrate is confinement. Everything below is the reason it can be trusted with a process.

- **The daemon derives its subject from kernel peer credentials and never from HTTP data.** Startup
  requires at least one explicit `--allow-uid`; the subject is `local:<uid>`. Never accept a subject,
  a tenant or a uid from a request body or header.
- **The enforced isolation set is a floor, not a menu**: `openat2` beneath/no-link/no-mount I/O,
  atomic replacement, cleared and shaped environment, namespace no-egress, pids and memory+swap
  bounds with cumulatively observed CPU, backend-identity-bound capability snapshots, output
  draining, timeout, whole-tree kill, exact capsule-byte verification, read-only `/runtime` beside a
  separate writable `/workspace`, owner-private durable state, and bounded normal/restart capsule
  cleanup. Removing or weakening any of these is invariant 3's named refusal, never a quiet
  downgrade.
- **Execution capsules are verified read-only inputs** (`adr/0009`), and declared host roots are
  mounted read-only (`adr/0010`). A capsule's bytes are verified exactly; a mutable ref is input
  convenience and is always resolved to and recorded as an immutable commit
  (`docs/design/04-security-and-isolation.md:122`).
- **Static-bearer TCP is explicitly development-only.** The accepted short-lived scoped hosted
  trust-envelope profile is what replaces it. Never make the development posture reachable in a
  hosted one.
- **Wire-visible identifiers carry a former brand name and are frozen by invariant 6.** The
  `urn:b10x:substrate-wire:*` schema `$id` values, the `x-b10x-contract*` HTTP headers,
  the `https://b10x.invalid/` URI namespace, the `b10x.execution-capsule.v1` hash domain
  separator and the `origin: b10x` bundle marker are **protocol bytes another party verifies**
  (atlas ADR 0001 § *Wire-visible identifiers*; the exemption list is documented in
  `scripts/check-brand.sh`). Renaming one is a **coordinated migration with an ADR in atlas**, done
  by cutting a new bundle version — never by rewriting a frozen one.
- **Never commit credentials, tokens or key files.** `scripts/check-secrets.sh` exists for this.

## Out of scope

Substrate does not decide product policy, run agent loops, understand connector vendors, or depend
on Flux.

| Belongs elsewhere | Repo |
|---|---|
| The agent loop — turn assembly, tool round trips, budgets | `harness` |
| Driving a vendor harness | `metaharness` |
| Connector vendor semantics, the grant engine | `connectors` |
| Principal identity and token audiences | `identity` |
| LLM request termination and model routing | `llmgw` |
| Durable domain event storage | `eventlog` |
| Fleet scheduling and cloud product policy | the product |

## The gate

```console
bash scripts/gate.sh
```

In order: `cargo test --workspace --locked`, `cargo fmt --all --check`,
`cargo clippy --workspace --all-targets --locked -- -D warnings`, then
`check-links.py`, `check-adrs.py`, `check-contract-bundle.py`, `check-runtime-vectors.py`, and
`check-brand.sh`. Green here is the bar for `main`.

**`scripts/gate.sh` verifies the 0.1.0 bundle only.** `check-contract-bundle-0.2.0.py`,
`-0.3.0.py` and `-0.4.0.py` exist and are **not** run by the gate. Touching a successor bundle means
running its checker by hand; a green gate is not evidence that `0.4.0` still holds.

**A green local gate does not guarantee a green CI.** The steps mirror each other; the toolchain does
not — CI installs whatever `stable` is that day, and a newer clippy can fail a commit that passed
locally. Run `rustup update` before pushing, and read the gate's own exit status, never a pipeline's
(`gate.sh 2>&1 | tail` reports `tail`'s status, not the gate's).

The monorepo-era cross-component suite (`scripts/check-local.sh` at the predecessor's root) no longer
applies here.

## Releases

Maintain `CHANGELOG.md` in Keep a Changelog form and cut it under a version heading at release. The
tag is the bare version — `0.2.0`, the version and nothing else (atlas § *Naming*) — annotated, at a
fully gated `main` commit. The full gate comes first; component steps alone are not enough.

The bundle is **not** a published stable release: OCI packaging, signing and digest pinning are
separate release work. Do not describe a development bundle as stable.

## Where work is tracked

| What | Where |
|---|---|
| Current accepted system boundary and dependency direction | `architecture/` |
| Draft contract work — each document states its status | `docs/design/` |
| Accepted component decisions, YAML frontmatter with `date` and `status` | `adr/` |
| Review gates and implementation slices | `docs/plan/` |
| Ordered exit criteria | `ROADMAP.md` |
| Observed progress | `STATUS.md` |
| Archived reviews, retained as immutable review input | `docs/reviews/archived/` |
| What shipped | `CHANGELOG.md` |

**Document placement is a rule, not a habit.** Current architecture goes in `architecture/`;
sequencing in `ROADMAP.md`; observed progress in `STATUS.md`. `docs/plan/` turns design into gates
and slices and contains no implementation.

**Links.** Use repository-relative Markdown links for material in this repository and canonical HTTPS
links only for reachable external sources. Cite material that lives outside this repository **by path
in inline code**, not by URL into a predecessor repository. Never commit machine-local paths,
sibling-checkout links, `file://` URLs or editor URIs. `scripts/check-links.py` is the gate.

## Change discipline

Keep changes reviewable and preserve the direction from composition and products *toward* substrate.
A contract change must identify affected capabilities, refusal behaviour, observations, events and
consumer compatibility **before implementation begins**.

## Bot identity

Automated commits and pushes use the GitHub App via `scripts/as-bot.sh` and `scripts/bot-gh.sh`,
never a human identity. `scripts/bot-token.sh` mints the token, and **the bot-org default it applies
at `scripts/bot-token.sh:8` is not the org this repository lives in** — set that variable explicitly
to `beyond10x` rather than relying on the default.
