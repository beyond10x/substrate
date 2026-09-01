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
   through `0.12.0` exist; `0.12.0` is the current development bundle, adding exact read-only and
   scoped workspace access (ADR 0023); `0.11.0` added hard persistent and per-exec
   writable-storage quotas plus explicit exact resource observations and two metrics routes
   (ADRs 0020–0021); `0.10.0` added the `pty` session mode, the `resize` frame and the
   `sessions.pty` fact (ADR 0019); `0.9.0` declared all served API
   majors and the v1 catch-all file paths in one registry (ADR 0018); `0.8.0` added the declared
   aperture byte ceiling and the named bound on an exec observation (ADR 0014); `0.7.0` added delegated
   context and grant attribution (ADR 0011); `0.6.0` added destination-bound egress
   apertures (ADR 0013), and **every earlier directory is frozen** (`STATUS.md:36`,
   `xtask/src/json.rs:152`, `contracts/substrate-wire/0.2.0/README.md:13`).
   The daemon and Rust SDK advertise `substrate-wire/0.12.0` with the SHA-256 of that bundle's
   inner `bundle.json` (`crates/substrate-wire/src/lib.rs`); Atlas ADR 0019 records the explicit
   promotion and the one additional 0.11.0-to-0.12.0 lineage bridge the gate proves. Moving this
   pair again is its own coordinated change with its own clients to notify. A wire change **adds a successor bundle**; it
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
9. **This repository is public and Apache-2.0 by explicit decisions.** Atlas ADR 0003 authorises
   Substrate's public visibility after a full-history secret scan; Atlas ADR 0010 grants
   Apache-2.0 across all beyond10x-owned Substrate history without rewriting frozen bundle bytes.
   Third-party material retains its own licence, and no development contract becomes stable. Any
   other b10x repository keeps its own visibility and licence until an accepted decision changes it.

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
  It scans the **whole history** with a checksum-pinned Gitleaks and reads `.gitleaks.toml`, which
  keeps the default rule set (`useDefault = true`) and adds exactly one exception: the `jwt` rule is
  allowed in the delegated-context conformance vectors, and nowhere else. Those vectors are JWTs by
  definition — ADR 0011 fixes the document as a compact JWS, so a vector proving substrate verifies
  one has to contain one — and they carry no credential: each is signed by a key whose seed is the
  SHA-256 of a sentence published in this repository
  (`crates/substrate-daemon/tests/runtime_vectors.rs:2405-2409`). The exception is scoped by rule
  **and** by path; a JWT anywhere else, including any other vector, is still a finding. Proven, not
  asserted: the same token in `contracts/substrate-wire/0.7.0/vectors/http/delegated-context-*.json`
  is allowed and in `crates/substrate-daemon/src/` is caught. **Widening this allowlist is a
  security change, not a fix for a red scan.**

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

In order: `cargo test --workspace --release --locked`, `cargo fmt --all --check`,
`cargo clippy --workspace --all-targets --locked -- -D warnings`, then `cargo xtask check-links`,
`cargo xtask check-adrs`, `cargo xtask check-secrets`, `cargo xtask check-advisories`,
`cargo xtask check-licenses`, `cargo xtask check-packages`,
`check-contract-bundle.py`, `check-contract-bundle-0.2.0.py`,
`-0.3.0.py`, `-0.4.0.py`, `cargo xtask check-bundle 0.5.0`, `check-bundle 0.6.0`, `check-bundle 0.7.0`,
`check-bundle 0.8.0`, `check-bundle 0.9.0`, `check-bundle 0.10.0`, `check-bundle 0.11.0`,
`check-bundle 0.12.0`,
`cargo xtask check-json` and `cargo xtask check-toolchain`.
Green here is the bar for `main`.
The former brand is fenced org-wide by `scripts/check-org-brand.sh` in the **atlas** repo, not here.

**The gate's own checks are `cargo xtask` verbs**, in the `xtask/` workspace member — anything that
runs in a b10x foundation repository is Rust (`atlas/AGENTS.md` § *Language*) — while the four
frozen `render-contract-bundle*.py` / `check-contract-bundle*.py` pairs stay Python as the released
bundles' reproducibility proof (invariant 6), not as tooling.

| verb | what it refuses, or produces | in `gate.sh`? |
|---|---|---|
| `check-toolchain [--root <dir>]` | a Rust version the three pinning files disagree on | yes |
| `check-links` | a machine-local Markdown link, or a repository-relative target that is not there | yes |
| `check-adrs` | an ADR whose identity, frontmatter, index row or supersession link does not agree | yes |
| `check-secrets` | a reachable Git object carrying a credential, or an incomplete history scan | yes |
| `check-advisories` | a RustSec vulnerability or forbidden HTTP/2 dependency | yes |
| `check-licenses` | non-Apache workspace metadata, an unreviewed dependency licence or third-party notice drift | yes |
| `check-packages` | a registry package outside the five-name allowlist, a loose internal version edge, or a package without inherited SPDX metadata and its README | yes |
| `package-bundle <version> --out <dir>` | produces a released bundle as a deterministic OCI image layout | no — under `cargo test` |
| `render-bundle <version> --out <dir>` | produces a bundle tree from `substrate-wire` and `xtask/bundle-source/<version>/`; refuses to write anywhere under `contracts/` | no — under `cargo test` |
| `check-bundle <version>` | a released bundle whose bytes are not the fixed point of `xtask/bundle-source/<version>/` | yes, `0.5.0` through `0.12.0` |
| `check-json [<version>...]` | JSON beneath a released bundle that no bundled schema classifies, that its schema rejects, or that is not in deterministic source form | yes, all twelve |

**`cargo xtask package-bundle <version> --out <dir>`** packages a released bundle as a
deterministic OCI image layout. It is not a gate step of its own: its cases run under
`cargo test --workspace --locked`, the gate's first step.

**`cargo xtask render-bundle <version>` is how a successor bundle is cut.** The original renderer
is frozen for `0.5.0`–`0.8.0`; `0.9.0` and later select the versioned multi-major renderer.
`xtask/bundle-source/<version>/` holds what a human authored — one file per
emitted path, plus `routes.json`, `coverage.json`, `hash-cases.json`, `vector-order.json` and
`executable-vectors.json`; the renderer computes 30 of `0.4.0`'s 200 files whole and splices
computed values into 14 more. It lives outside `contracts/` because every directory there is a
released bundle and `cargo xtask check-json` fails closed on JSON beneath one
(`xtask/src/json.rs:165`). Rendering
into `contracts/` is a named refusal, not a warning. A test asserts that rendering `0.4.0` still
reproduces the frozen tree byte for byte, so the renderer cannot drift away from what shipped.

**Not everything in a bundle is derivable, and the renderer says which parts are not.** No schema
*shape* comes from the Rust types: `schemars` is not a workspace dependency, and the types are
already ahead of `0.4.0` — `ExecStartInput::read_only_roots`
(`crates/substrate-wire/src/lib.rs:761`) has no `0.4.0` schema — while the bounds the schemas state
are literals in `crates/substrate-daemon/src/app/operations.rs:245-248` and
`crates/substrate-host/src/process.rs:824-826`, not on the types. What the wire crate does own is
taken from it: canonical hashing and 22 bounds constants. One derivation is recorded as **lost** at
`xtask/src/render.rs:19-25` — the three unions in `schemas/vector.json` encode Python dict
insertion order, which a sorted-key bundle preserves nowhere.

**The clean-room runtime-vector runner is a gate step of its own no longer, for the same
reason.** `crates/substrate-daemon/tests/runtime_vectors.rs` spawns the *shipped* binary
(`env!("CARGO_BIN_EXE_substrate-daemon")`) and drives it over its Unix socket with a
hand-written HTTP/1.1 and WebSocket client, so it links no implementation and asserts only on
the wire; `cargo test --workspace --locked` runs it. Its portable lane asserts three named
refusals — `exec.sandbox-unavailable` (501), `exec.secret-slots-unserved` (501) and
`exec.secret-slot-descriptor-invalid` (422) — plus `session.pty-unserved` (501) — across 68 cases,
and its delegated lane 95. Both totals include one route/refusal probe for every operation in the
promoted registry; the delegated cases also include an interactive shell driven through a `pty` session
(`crates/substrate-daemon/tests/runtime_vectors.rs`, `PORTABLE_CASES` and `DELEGATED_CASES`). Its delegated lane runs only when
`SUBSTRATE_VECTORS_CGROUP_ROOT` names a delegated cgroup v2 subtree the test process is inside;
unset, those cases are **absent, never reported as passed** (invariant 3). **`bash scripts/delegated-lane.sh`
runs that lane plus the host and public-SDK delegated cases, and needs no privilege** — it asks
systemd for a delegated scope, moves itself into a
child group so the delegation root stays process-free, and sets the variable. Do not conclude the
delegated lane cannot run here: a user session's own scope is root-owned, so `mkdir` in it fails,
and an absent lane looks identical to a green one if you only read `cargo test`.

**The gate verifies every released bundle, not just `0.1.0`.** `scripts/gate.sh:20-23` runs the
four frozen Python checkers, and the lines after them run `cargo xtask check-bundle` for `0.5.0`
through `0.12.0`, so
a green gate *is* evidence that all twelve still hold. Cutting a successor bundle therefore means
**adding its check to `scripts/gate.sh`** — a bundle whose check is not in the gate is unverified
from the next commit onward.

**Editing `xtask/src/render.rs` breaks every bundle it has already rendered.** A rendered
`bundle.json` carries `generator.digest`, which is the sha256 of the file named at
`generator.name` (`xtask/src/render.rs:308-312`) — so one byte changed there and `0.5.0` stops being
a fixed point of its own source, with no way to fix it that does not rewrite a frozen directory.
A successor that needs a new `{"$wire": …}` binding therefore **cannot have one**: bind the constant
from `xtask/src/bundle.rs` instead, which no bundle hashes (`check_aperture_additions`, added for
`0.6.0`, does exactly this for `MAX_EGRESS_APERTURES`).

**From `0.5.0` on, that check is `cargo xtask check-bundle <version>`, not a fifth Python checker.**
It re-renders the bundle from `xtask/bundle-source/<version>/` and compares bytes, so it verifies
strictly more than a hand-written checker: a released tree that is no longer the fixed point of its
own source fails, whatever else about it still looks well-formed. The four Python checkers stay for
`0.1.0`–`0.4.0`, which have no authored source tree and never will.

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

The five approved crates are published manually from that fully gated annotated tag, in dependency
order: `b10x-substrate-wire`, `b10x-substrate-store`, `b10x-substrate-host`,
`b10x-substrate-daemon`, then `b10x-substrate-sdk`. Use an operator-held scoped crates.io token and
`cargo publish --locked -p <package>` for each; wait for each dependency to become visible before
publishing its consumer. Never add that token to GitHub or automate this through the image release
workflow. `cargo xtask check-packages` proves the closed allowlist and package contents before the
tag; crates.io is the authority for whether an already-published version can be uploaded.

**Pushing that tag runs [`.github/workflows/release.yml`](.github/workflows/release.yml)**, which
builds the `Dockerfile` and packages the explicitly pinned current development bundle with
`cargo xtask package-bundle`. It publishes
`ghcr.io/beyond10x/b10x-substrate-daemon:<version>` and
`ghcr.io/beyond10x/b10x-substrate-wire:<bundle-version>`, keyless-signs both digests and verifies
both signatures **before** it announces anything. It refuses — publishing nothing — a tag that is
not the bare version form (so `0.2.2-rc.0` produces nothing), a lightweight tag, a commit that is not
an ancestor of `main`, a version that disagrees with `[workspace.package] version`, a commit for
which `gate.yml` has not concluded `success`, an existing daemon version tag, or an existing bundle
tag whose digest differs from the deterministic local package. A later daemon release may reuse a
byte-identical bundle: the workflow verifies the digest, signs it again with that release's exact
workflow identity, and never replaces the write-once tag. Release runs serialize globally because
consecutive daemon releases can name that same bundle. The workflow reads the recorded gate
conclusion for the tagged SHA rather than re-running a lookalike, and copies the packager's exact OCI
layout with ORAS rather than constructing another manifest at publication time. It proves the
bundle is anonymously retrievable before mutating the daemon-image tag, so correcting first-push
package visibility leaves a safe retry path.

`packages: write` and `id-token: write` exist on the release job and nowhere else; that job holds
`contents: write` only to create the GitHub release. Everything it does uses the run's own
`GITHUB_TOKEN`; **the release needs no repository secret at all** (§ *Bot identity*). The GitHub
release records both digests and exact `cosign verify` commands. **`0.2.3` is published**:
`ghcr.io/beyond10x/b10x-substrate-daemon:0.2.3` at
`sha256:ab10158266b579d705ce8422c7d2a6e783cde950d30e100f61ca6befc4d0beda`, keyless-signed and
`cosign verify`-ed in the run before anything announced. No contract-bundle tag has been published
yet; the bundle workflow landed after the latest release tag.

`main` is protected by the `Full gate` status check, so the workflow never pushes a changelog commit
or creates a `GITHUB_TOKEN` pull request whose events GitHub would suppress. After both signatures
verify and the GitHub release exists, its summary emits the exact daemon and contract-bundle digest
lines for a workstation's bot-authored pull request. That PR receives the same gate as every other
change.

A signed, digest-pinned OCI bundle is still **not a stable contract release**. The published
artifact is annotated `dev.b10x.contract.status=development`; atlas ADR 0019 governs any later
stability decision. Do not describe a development bundle as stable.

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

**Bot authentication does not bypass protected `main`.** A direct `scripts/as-bot.sh push origin
main` is rejected with `GH006` because the required checks are only created after the commit is
published on a branch. Push a bot-owned branch, open its pull request with `scripts/bot-gh.sh`, wait
for the required checks on that exact head, and merge through the protected-branch path. Do not
retry the direct push or weaken protection; this is the ordinary path for every workstation-authored
change, including release preparation.

**One exception, and it is narrower rather than looser: CI releases use `GITHUB_TOKEN` for registry
and GitHub-release API writes.** `.github/workflows/release.yml` uses the run's own token for both
GHCR artifacts and the GitHub release, makes no git commit or push, and holds no App key. The App is
installed org-wide with `administration:write` and `workflows:write` on every repository in
`beyond10x`, and **this repository is public** (invariant 9): its private key as an Actions secret
here would put an org-wide credential in the repository with the widest audience for proposing
workflow changes. `GITHUB_TOKEN` cannot leave this repository, is minted per run and expires with
it.

Do not add `B10X_BOT_PRIVATE_KEY` to this repository's secrets. Recording the emitted release
digests in `CHANGELOG.md`, or anything else that needs the App's identity or cross-repository reach,
runs from a workstation through `as-bot.sh`, or from a private repository.

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
