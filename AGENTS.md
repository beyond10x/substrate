# AGENTS.md — substrate

The contract for changing **this** repository. Org-wide rules — the naming convention, the language rule (anything that runs is Rust, not Python), the
former-brand rule (atlas ADR 0001) and its four exemption categories, and the rule that renaming
anything another repo verifies is a coordinated migration with an ADR — live in `atlas/AGENTS.md`
and are not restated here.

`README.md` orients a reader and shows how to run the daemon; `STATUS.md` records observed progress
and `ROADMAP.md` ordered exit criteria. This file says what must not break.

## Serves

The objectives of the collection this repository moves, by id from `atlas/ROADMAP.md` — the only
cross-repository roadmap, and the page that says what each id means and which evidence closes it:

- **O1 — governed reach.** Confinement is where *may reach* becomes *can reach*: a confined workspace, argv-only exec, host roots read-only, and a named refusal for everything else.

A change here that moves none of these is a question for the operator, not a task.
`atlas/scripts/check-map.sh` fails a repository whose `AGENTS.md` names no objective.

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
   frozen** (`STATUS.md:36`, `scripts/contract_json_gate.py:283`,
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
9. **This repository is public by explicit decision.** Atlas ADR 0003 authorises Substrate's public
   visibility after a full-history secret scan. It does not change the proprietary licence or make a
   development contract stable. Any other b10x repository stays private unless its own accepted
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
  atlas' org-wide fence). Renaming one is a **coordinated migration with an ADR in atlas**, done
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
`cargo clippy --workspace --all-targets --locked -- -D warnings`, then `cargo xtask check-links`,
`cargo xtask check-adrs`, `check-contract-bundle.py`, `check-contract-bundle-0.2.0.py`,
`-0.3.0.py`, `-0.4.0.py`, `test_contract_json_gate.py`, `test_package_contract_bundle.py`,
`check-runtime-vectors.py` and `cargo xtask check-toolchain`. Green here is the bar for `main`.
The former brand is fenced org-wide by `scripts/check-org-brand.sh` in the **atlas** repo, not here.

**The gate's own checks are `cargo xtask` verbs**, in the `xtask/` workspace member — anything that
runs in a b10x foundation repository is Rust (`atlas/AGENTS.md` § *Language*) — while the four
frozen `render-contract-bundle*.py` / `check-contract-bundle*.py` pairs stay Python as the released
bundles' reproducibility proof (invariant 6), not as tooling.

**The gate verifies every released bundle, not just `0.1.0`.** `scripts/gate.sh:20-23` runs the
`0.1.0` checker and the `0.2.0`, `0.3.0` and `0.4.0` checkers, so a green gate *is* evidence that
all four still hold. Cutting a successor bundle therefore means **adding its checker to
`scripts/gate.sh`** alongside those four — a bundle whose checker is not in the gate is unverified
from the next commit onward.

**A green local gate does not guarantee a green CI.** The steps mirror each other, and
`.github/workflows/gate.yml` runs the same `bash scripts/gate.sh` on push and pull request. The
toolchain is pinned by `rust-toolchain.toml`, not by whatever `stable` is that day: bumping it is
**one commit** that moves `rust-toolchain.toml`, the `rust-version` in `Cargo.toml` and the
`Dockerfile` builder tag together, and `cargo xtask check-toolchain` fails the gate when the three
disagree. Read the gate's own exit status, never a pipeline's (`gate.sh 2>&1 | tail` reports
`tail`'s status, not the gate's).

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
| The plan — epics and stories, with kinds, statuses and legal moves from the `protocol` CLI | `.engineering/planning/`, validated by `protocol artifact validate` |
| Archived reviews, retained as immutable review input | `docs/reviews/archived/` |
| What shipped | `CHANGELOG.md` |

## Public website

For any public documentation website change, read and follow the repository-local `website-docs`
skill at `atlas/.agents/skills/website-docs/SKILL.md` before taking action. The public site is a
self-contained projection: do not publish or link internal designs, ADRs, plans, reviews, work logs,
contributor status material, or private source.

**Document placement is a rule, not a habit.** Current architecture goes in `architecture/`;
sequencing in `ROADMAP.md`; observed progress in `STATUS.md`. `docs/plan/` turns design into gates
and slices and contains no implementation. Work items — epics and stories — are planning
artifacts in `.engineering/planning/`, never a second ledger in prose.

**Links.** Use repository-relative Markdown links for material in this repository and canonical HTTPS
links only for reachable external sources. Cite material that lives outside this repository **by path
in inline code**, not by URL into a predecessor repository. Never commit machine-local paths,
sibling-checkout links, `file://` URLs or editor URIs. `cargo xtask check-links` is the gate.

## Change discipline

Keep changes reviewable and preserve the direction from composition and products *toward* substrate.
A contract change must identify affected capabilities, refusal behaviour, observations, events and
consumer compatibility **before implementation begins**.

## Bot identity

Automated commits and pushes use the GitHub App via `scripts/as-bot.sh` and `scripts/bot-gh.sh`,
never a human identity. `scripts/bot-token.sh` mints the token, and the bot-org default it applies
at `scripts/bot-token.sh:8` — `org="${B10X_BOT_ORG:-beyond10x}"` — **is** the org this repository
lives in (`git remote -v` shows `github.com/beyond10x/substrate`), so the default is right here. Set
`B10X_BOT_ORG` only to mint against a different org.

## Planning artifacts

Plan items are markdown files under `.engineering/planning/<kind>/<slug>.md`: YAML frontmatter the
`protocol` CLI owns, and a body the agent and operator own. `.engineering/project.yaml` pins the
governing document tree to one `engineering-protocols` commit; advancing the pin is an explicit
change to that file. The `engineering-protocols` Claude Code plugin (installed user-scope; skill
`/engineering-protocols:planning`) carries the full model and store conventions.

Kinds, relations, statuses and legal moves come from validated lifecycle documents. Ask the CLI —
`protocol artifact kinds`, `relations`, `lifecycle <kind>`, `list`, `board`, `graph` — instead of
reciting them. Before the first planning-store write of a session, run `protocol artifact list`.

1. **A status changes only through `protocol artifact move`.** Never edit `status:` directly.
2. **Never edit a planning-store file directly.** `new` creates, `relate` links, `move` moves,
   `body <id> --from <path|->` writes prose.
3. **After a batch, run `protocol artifact validate` and relay its output verbatim.**
4. **A refusal is an answer.** Relay the legal moves the CLI names; do not route around it.
5. **An already-satisfied or wrong request still gets an artifact** recording the finding.

New artifacts start in the lifecycle's initial state. Lifecycle moves are claims about project
state: propose them and wait for the operator unless the operator asked for the specific move.
`protocol` must be on `PATH` (`cargo install --path crates/protocol-cli` in an
`engineering-protocols` checkout); if it is absent, do not improvise machine-owned frontmatter.

A story that changes a contract or a capability still owes its ADR or design document **before
code** (invariant 8); the story body names which. `ROADMAP.md` keeps the phase order and
`STATUS.md` the observed state; the store holds the work items and their status, and nothing else
restates it.
