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
   (`adr/0001-substrate-is-standalone-and-flux-free.md`). `cargo xtask check-packages` asserts the
   dependency half on every manifest in the workspace.
2. **No sibling-component implementation dependency**, in either direction. A consumer embeds
   substrate; substrate embeds nothing of theirs — `cargo xtask check-packages` refuses a
   `beyond10x` Git dependency in any dependency table.
3. **A missing isolation or capability guarantee is a named refusal, never silent degradation.**
   Absent delegation keeps exec facts *absent*; it never manufactures an optimistic one.
4. **Drivers implement one substrate contract and expose verified capability facts.** Clients do not
   branch on driver internals — a client that reads which driver answered has made the driver part
   of the contract.
5. **Operations are durable before driver dispatch**
   (`adr/0005-operations-are-durable-before-driver-dispatch.md`).
6. **Every released contract bundle directory is immutable.** `contracts/substrate-wire/0.1.0`
   through `0.23.0` exist; `0.23.0` adds no route and publishes ADR 0010's declared read-only
   roots: the optional `read_only_roots` start list (closed `{host_path, mount}` entries, at most
   `MAX_READ_ONLY_ROOTS`), the optional `exec.read-only-roots` fact (`max_roots`), the applied
   `read_only_roots` record and the three refusals a root has always raised, now published with a
   refusal vector each and held to its `substrate_wire` constant by `cargo xtask check-bundle
   0.23.0`; the daemon refuses a start naming a root `exec.sandbox-unavailable` (501) before
   dispatch when the fact is absent, and the exec applied record admits every member the wire
   crate emits, correcting `secret_slots`, which `0.5.0` to `0.22.0` omit, and `read_only_files`,
   which `0.18.0` to `0.22.0` omit; and, by design 27, publishes `scratch`, `measurements` and
   `workspace_access` on the session start, with the refusals they raise on both starts, each held
   to its `substrate_wire` constant;
   `0.22.0` added ADR 0038: the optional
   `writable_roots` start list (a closed union of a declared host root and a Git checkout
   workspace, at most eight), the optional `exec.writable-roots.host` and `exec.writable-roots.git`
   facts, the applied `writable_roots` record and eight named refusals, each held to its
   `substrate_wire` constant by `cargo xtask check-bundle 0.22.0`;
   `0.21.0` added design 26: the optional `workdir`
   start field and its `exec.workdir` fact, the optional `apertures` list beside `aperture`, the
   array form of the applied network whose elements state `listen`, the applied `workdir`, and
   seven named refusals;
   `0.20.0` added the host-process family of ADR 0036 S1 to S7 and S11: seven
   `/v1/processes` routes under the new `process` scope, the optional `process.host` fact, the
   closed `process` record (always `profile: host`, `confinement: none`), its bounds, seventeen
   named refusals and the shared `lease.expired` on renewal;
   `0.19.0` added the explicit `workspace.adopt` route at
   `POST /v1/workspaces/adopt` and two required read-only members, `origin` and `root_name`, on the
   closed workspace resource, with `workspace-id` unchanged and still `ws_`-prefixed (ADR 0035; its compatibility block
   records that client-visible change); `0.18.0` made `exec.resource-usage.block_io` a published boolean and
   the block I/O usage counters optional, absent on a host without the io controller (ADR 0032),
   and added the optional `read_only_files` start field, the `exec.read-only-files` fact and nine
   named refusals for binding a single host file read-only (ADR 0033);
   `0.17.0` added policy-controlled unrecorded terminal streaming (design 23). `0.16.0` added
   Connector-authorized
   Git source fields, the conditional `workspace.git` capability and two bounded Git observation
   routes. `0.15.0` replaced exactly eight
   `/v1/pipe-sessions` route addresses with `/v1/sessions` without an alias (ADR 0028 and Atlas
   ADR 0022); `0.14.0` added proof-bound network session attachment authority and its four named
   refusals (ADR 0027); `0.13.0` added the hosted
   Identity admission profile and its four named refusals (ADR 0026); `0.12.0` added exact read-only and
   scoped workspace access (ADR 0023); `0.11.0` added hard persistent and per-exec
   writable-storage quotas plus explicit exact resource observations and two metrics routes
   (ADRs 0020–0021); `0.10.0` added the `pty` session mode, the `resize` frame and the
   `sessions.pty` fact (ADR 0019); `0.9.0` declared all served API
   majors and the v1 catch-all file paths in one registry (ADR 0018); `0.8.0` added the declared
   aperture byte ceiling and the named bound on an exec observation (ADR 0014); `0.7.0` added delegated
   context and grant attribution (ADR 0011); `0.6.0` added destination-bound egress
   apertures (ADR 0013), and **every earlier directory is frozen** (`STATUS.md:36`,
   `xtask/src/json.rs:152`, `contracts/substrate-wire/0.2.0/README.md:13`).
   The daemon and Rust SDK advertise `substrate-wire/0.23.0` with the SHA-256 of that bundle's
   inner `bundle.json` (`crates/substrate-wire/src/lib.rs`); ADR 0010 records the read-only roots, ADR 0038 the writable-root
   additions, design 26 records the working
   directory and aperture-list additions, design 21 records the Git source and
   observation additions, ADR 0028 records the preceding route promotion, and the gate retains the
   one additional 0.11.0-to-0.12.0 lineage bridge. Moving this
   pair again is its own coordinated change with its own clients to notify. A wire change **adds a successor bundle**; it
   never rewrites bytes in a released one. The compatibility block of a successor states its
   predecessor and its exact route counts, and the checker pins them. Successors are additive by
   default; `0.15.0` is the one exact `breaking-development-v1` exception and records all eight
   removed and replacement addresses.
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
  atomic replacement, cleared and shaped environment, namespace no-egress, **no nested user
  namespace** — the child's own user namespace with `--disable-userns`, so it cannot create
  another one and hold `CAP_SYS_ADMIN` in it — pids and memory+swap
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
  one has to contain one — and they carry no credential. Each is a pre-signed EdDSA compact JWS
  (`kid` `vector-key-1`, issuer `https://issuer.invalid`) that verifies only with the **public**
  key the vector itself carries in its setup (`delegated-context-key`); the signing seed is
  deliberately not committed (`docs/design/09-delegated-context-and-grant-attribution.md` § 6), so
  the vectors are verify-only. They do **not** verify with `delegated_signing_key` in
  `crates/substrate-daemon/tests/runtime_vectors.rs`, whose published seed signs only the
  documents that runner mints at run time. This holds for bundles `0.17.0` through `0.23.0`. The exception is scoped by rule
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
`cargo clippy --workspace --all-targets --release --locked -- -D warnings`, then `cargo xtask check-links`,
`cargo xtask check-adrs`, `cargo xtask check-secrets`, `cargo xtask check-advisories`,
`cargo xtask check-licenses`, `cargo xtask check-packages`, `cargo xtask check-mcp-boundary`,
`check-contract-bundle.py`, `check-contract-bundle-0.2.0.py`,
`-0.3.0.py`, `-0.4.0.py`, one bounded
`cargo xtask check-bundles 0.5.0 ... 0.23.0`,
`cargo xtask check-json` and `cargo xtask check-toolchain`.
Green here is the bar for `main`.

**A change does not buy confidence with CI time.** A test runs its scenario once and finishes in
seconds. Repetition counts, soak loops and load generators are local evidence recorded in the
planning store, never a test, a CI step or a script the gate calls. A story whose acceptance needs
"N runs under load" proves it on a workstation and records the run; the case it adds to the suite
runs once.
The former brand is fenced org-wide by `scripts/check-org-brand.sh` in the **atlas** repo, not here.

**The gate's own checks are `cargo xtask` verbs**, in the `xtask/` workspace member — anything that
runs in a b10x foundation repository is Rust (`atlas/AGENTS.md` § *Language*) — while the four
frozen `render-contract-bundle*.py` / `check-contract-bundle*.py` pairs stay Python as the released
bundles' reproducibility proof (invariant 6), not as tooling.

| verb | what it refuses, or produces | in `gate.sh`? |
|---|---|---|
| `check-toolchain [--root <dir>]` | a Rust version the three pinning files disagree on | yes |
| `check-links` | a machine-local Markdown link, or a repository-relative target that is not there | yes |
| `check-mcp-boundary` | a publishable MCP crate, a Substrate implementation dependency, an unpinned/featured MCP stack, or a built-in unbounded/remote transport | yes |
| `mcp-codex-smoke --server <path>` | manually asks an authenticated ephemeral Codex run to exercise the shipped stdio adapter | no |
| `check-adrs` | an ADR whose identity, frontmatter, index row or supersession link does not agree | yes |
| `check-secrets` | a reachable Git object carrying a credential, or an incomplete history scan | yes |
| `check-advisories` | a RustSec vulnerability or forbidden HTTP/2 dependency | yes |
| `check-licenses` | non-Apache workspace metadata, an unreviewed dependency licence or third-party notice drift | yes |
| `check-packages` | a publishable workspace package, a loose internal version edge, a source runtime package without inherited SPDX metadata, its README and a public documentation target, or — invariants 1 and 2 — a Flux dependency or a `beyond10x` Git dependency in any dependency table, `[workspace.dependencies]` and `[patch.*]` included | yes |
| `package-bundle <version> --out <dir>` | produces a released bundle as a deterministic OCI image layout | no — under `cargo test` |
| `render-bundle <version> --out <dir>` | produces a bundle tree from `substrate-wire` and `xtask/bundle-source/<version>/`; refuses to write anywhere under `contracts/` | no — under `cargo test` |
| `check-bundle <version>` | a released bundle whose bytes are not the fixed point of `xtask/bundle-source/<version>/` | no — focused form of the batched gate check |
| `check-bundles <version>...` | the same checks with a bounded worker count and deterministic version-order reporting | yes, `0.5.0` through `0.23.0` in one invocation |
| `check-json [<version>...]` | JSON beneath a released bundle that no bundled schema classifies, that its schema rejects, or that is not in deterministic source form | yes, all twenty-three |

**`cargo xtask package-bundle <version> --out <dir>`** packages a released bundle as a
deterministic OCI image layout. It is not a gate step of its own: its cases run under
`cargo test --workspace --release --locked`, the gate's first step.

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
already ahead of `0.4.0` — `ExecStartInput::read_only_roots` (the `ExecStartInput` struct in
`crates/substrate-wire/src/lib.rs`) has no `0.4.0` schema — and no bound sits *on* a type as a
derive attribute. The bounds the schemas state are free constants: the wire crate's own
`pub const MAX_…`/`MIN_…` items (`grep -c '^pub const \(MAX\|MIN\)_' crates/substrate-wire/src/lib.rs`
= 48), which a schema binds with the `{"$wire": "CONSTANT"}` marker, plus the daemon- and
host-local literals their enforcement sites hold — `PIPE_MAX_FRAME_BYTES`,
`PIPE_MAX_INPUT_BYTES` and `PIPE_MAX_QUEUED_FRAMES` in `crates/substrate-daemon/src/app.rs`, and
`PIPE_FRAME_BYTES` and `PIPE_QUEUED_FRAMES` in `crates/substrate-host/src/process.rs`.
`crates/substrate-daemon/src/app/operations.rs` holds none of its own: it imports
`MIN_LEASE_TTL_MS` and `MAX_LEASE_TTL_MS` from the wire crate and the pipe bounds from `app.rs`.
What the wire crate does own is taken from it: canonical hashing and those bounds constants. One
derivation is recorded as **lost** in the module documentation of
`xtask/src/render.rs` — the three unions in `schemas/vector.json` encode Python dict
insertion order, which a sorted-key bundle preserves nowhere.

**The clean-room runtime-vector runner is a gate step of its own no longer, for the same
reason.** `crates/substrate-daemon/tests/runtime_vectors.rs` spawns the *shipped* binary
(`env!("CARGO_BIN_EXE_substrate-daemon")`) and drives it over its Unix socket with a
hand-written HTTP/1.1 and WebSocket client, so it links no implementation and asserts only on
the wire; `cargo test --workspace --release --locked` runs it. Its portable lane asserts three named
refusals — `exec.sandbox-unavailable` (501), `exec.secret-slots-unserved` (501) and
`exec.secret-slot-descriptor-invalid` (422) — plus `session.pty-unserved` (501), the five lexical
`exec.read-only-file*` refusals (422), `exec.read-only-files-unserved` (501),
`process.host-unserved` (501), the two lexical `exec.workdir-*` and five aperture-list refusals
(422) of design 26, `exec.workdir-unserved` (501) and `exec.egress-apertures-unserved` (501) for
a list, and the six lexical writable-root refusals (422) of ADR 0038 with
`exec.writable-roots-host-unserved` and `exec.writable-roots-git-unserved` (501), which both lanes
answer for a host root the daemon did not declare and for every Git root, and the portable lane
also for the one host root the runner declares, which it cannot prove without confinement — across
118 cases, and its delegated lane 180, whose four writable-root cases bind that declared root and
check the host side: the write lands, its parent and sibling do not change, a working directory
beneath the mount runs there, and the declared path swapped for a symlink is
`exec.writable-root-not-a-directory` (422). Its read-only-root cases (ADR 0010, bundle `0.23.0`): portable, the `exec.read-only-roots` fact is absent, one root is refused `exec.sandbox-unavailable` (501) by the daemon before dispatch and five roots are `exec.read-only-root-invalid` (422), the lexical rule checked first; delegated, the fact is `{"max_roots": 4}`, four roots are bound, read and refused writes, listed in the applied record and left unchanged on the host, a fifth is `exec.read-only-root-invalid` (422), and an absent host path is `exec.read-only-root-absent` (422) while a plain file and a symlink to a directory are `exec.read-only-root-not-a-directory` (422). Both totals also count a second daemon that declares a
Git source served over HTTPS on loopback by `git upload-pack` (ADR 0038 kind 2): it serves
`workspace.git` and materializes a Git workspace; the portable lane then refuses that workspace as
a Git root `exec.writable-roots-git-unserved` (501), and the delegated lane mounts it, has a child
commit in it from a working directory beneath the mount, reads the commit on the host and in the
change-set route, and refuses an empty workspace, the start's own, one named twice and an unknown
one before dispatch, each recorded as refused. Both lanes also hold four omitted-entry cases (ADR 0043): a
FIFO beside a file is named by the `x-b10x-omitted-entries` header, count 1, on the v1 directory
page and on the v2 tree both under its limit and when the limit truncates it, and once the FIFO is gone
neither listing carries the header and the tree is not truncated.
Both totals include
one route/refusal probe for every operation in the promoted registry (the seven host-process routes
of `0.20.0` among them: unserved on the portable lane, served and refused before anything runs on
the delegated one), and an operator's directory adopted through `workspace.adopt` (ADR 0035): its
out-of-set neighbour refused as `request.schema-invalid` before dispatch, an exec addressing it by
root name refused, a destroy that leaves its files, and, delegated, an exec run in it plus every
`check_confined_isolation` case run again against it (portable: `exec.sandbox-unavailable`
instead); the delegated cases also include an interactive shell driven through a `pty` session,
three raw-pipe sessions that each name one exec field design 27 publishes on the session start
(`measurements` observed, `workspace_access` read-only applied and a write refused, `scratch`
refused `exec.scratch-quota-unserved` (501) before dispatch on a host without project quotas),
programs started in a working directory beneath the workspace and beneath a declared root, an
absent and a symlinked one refused `exec.workdir-missing` before dispatch, and an aperture list
refused `exec.aperture-undeclared` for an undeclared name, run with two apertures each reporting its
`listen` address, and refused `exec.aperture-host-listed-twice` for `localhost` listed second
(`crates/substrate-daemon/tests/runtime_vectors.rs`, `PORTABLE_CASES` and `DELEGATED_CASES`). Its delegated lane runs only when
`SUBSTRATE_VECTORS_CGROUP_ROOT` names a delegated cgroup v2 subtree the test process is inside;
unset, those cases are **absent, never reported as passed** (invariant 3). **`bash scripts/delegated-lane.sh`
runs that lane plus the host, host-process (`--test processes`), public-SDK and disposable-MCP delegated cases, and needs no privilege** — it asks
systemd for a delegated scope, moves itself into a
child group so the delegation root stays process-free, and sets the variable. Do not conclude the
delegated lane cannot run here: a user session's own scope is root-owned, so `mkdir` in it fails,
and an absent lane looks identical to a green one if you only read `cargo test`.

### The real project-quota lane

**`cargo xtask quota-lane` is the only thing that runs the seven `real_quota_*` cases, and nothing
else ever did.** They live in
[`crates/substrate-host/src/git/quota_tests.rs`](crates/substrate-host/src/git/quota_tests.rs),
each `#[ignore]`d on the explicitly delegated project-quota fixture. `cargo test` does not select an
ignored case, and neither `scripts/gate.sh` nor `scripts/delegated-lane.sh` passes `--ignored` — so
provisioning the filesystem changed nothing on its own, because no script would have run them even
then. The one recorded execution is a hand-typed `docker exec` against a quota-lab image on
2026-09-05
([`review-result/git-workspace-quota-lifecycle-pass-1.md`](.engineering/planning/review-result/git-workspace-quota-lifecycle-pass-1.md)).
The verb is that invocation, repeatable:

```console
SUBSTRATE_TEST_QUOTA_ROOT=/quota SUBSTRATE_TEST_PROJECT_QUOTA_IDS=200000-200511 \
  cargo xtask quota-lane
```

`SUBSTRATE_TEST_QUOTA_ROOT` must be a directory on a filesystem mounted with project quotas
(`prjquota`, or XFS `pquota`), and `SUBSTRATE_TEST_PROJECT_QUOTA_IDS` an inclusive `START-END`
range of at least 128 identities that nothing else on that filesystem uses. A `docker exec` shape
also needs `SYS_ADMIN` in the inheritable and ambient sets for `quotactl`, as the recorded run's
`setpriv --inh-caps=+sys_admin --ambient-caps=+sys_admin` shows.

**With the fixture absent the verb refuses, naming each missing prerequisite and every case that
therefore did not run, and exits 1** — it never reports nothing and lets that read as green. It
selects each case with `--ignored --exact` and requires the harness's own `1 passed` for it, because
a filter that matches nothing still exits 0; a rename that emptied the selection would otherwise
make this lane green having executed no case, the same failure `scripts/delegated-lane.sh` refuses
to allow for the delegated cases. `cargo xtask quota-lane --inventory` states the same inventory and
runs nothing, and `scripts/delegated-lane.sh` closes with it so a green delegated lane cannot be
read as covering these cases.

**`cargo xtask quota-fixture run` provisions that fixture and runs the lane in it — one command.**
It builds the checked-in recipe [`fixtures/quota-lab/Dockerfile`](fixtures/quota-lab/Dockerfile)
(the release builder's exact pinned `rust` image plus `e2fsprogs`, tagged
`b10x-substrate-quota-lab:<key>`, never pushed) and runs one container with Docker's default
capability set plus `SYS_ADMIN` — what the recorded run had, never `--privileged` — and the loop
access a filesystem image needs: `--device /dev/loop-control`, `--device-cgroup-rule 'b 7:* rmw'`
and `--security-opt apparmor=unconfined`, because the default AppArmor profile denies `mount`.
The container never sees the live checkout: the verb reads `HEAD` once, extracts that commit with
`git archive` (modification times reset to the extraction, `tar -m`) into a fresh directory under
the target directory, builds the recipe from it and binds it read-only at `/src`, and removes it
after the run. The container's builds go to the Docker volumes
`b10x-substrate-quota-lab-target-<key>` and `b10x-substrate-quota-lab-cargo-registry-<key>`, never
to the checkout's `target/`; `<key>` is derived from the checkout's canonical path, so two
checkouts never share a container build target. Inside, the hidden `quota-fixture inside` creates a sparse 1 GiB ext4 image with
the `quota` and `project` features, attaches it through one free loop device with automatic detach,
mounts it `prjquota` at `/quota` (not XFS, which reports an exceeded project quota as `ENOSPC` where
the cases require `EDQUOT`),
hands it to the invoking uid and runs `quota-lane` as that uid with
`setpriv --inh-caps=+sys_admin --ambient-caps=+sys_admin` and `200000-200511`. It refuses to
provision anything outside that container. `--release` builds the cases in the release profile;
`--clean` removes this checkout's image tag and its two volumes afterwards, and nothing else. The loop device is a host-kernel object
for the duration of the run. Docker must work for the invoking user without `sudo`; where it does
not, the verb refuses and names the cases that did not run.

**A release requires a recorded run of it for the tagged commit** (§ *Releases*). The run ends,
only when every declared case passed, the checkout was clean before the run and is still clean
after it, and `HEAD` did not move, with the line
`Quota-lane: <source-sha> <case-count> <RFC 3339 UTC>`; `Confinement-lane:` does not cover these
cases, because they are in a different crate and outside `DELEGATED_CASES`.

**The gate verifies every released bundle, not just `0.1.0`.** The four
`run python3 scripts/check-contract-bundle*.py` lines in `scripts/gate.sh` run the
four frozen Python checkers, and the line after them runs `cargo xtask check-bundles` for `0.5.0`
through `0.23.0`, so
a green gate *is* evidence that all twenty-three still hold. Cutting a successor bundle therefore means
**adding its check to `scripts/gate.sh`** — a bundle whose check is not in the gate is unverified
from the next commit onward.

**Editing `xtask/src/render.rs` breaks every bundle it has already rendered.** A rendered
`bundle.json` carries `generator.digest`, which is the sha256 of the file named at
`generator.name` (the `generator_digest` binding in `Source::load`, `xtask/src/render.rs`, which
calls `digest_of` on that path) — so one byte changed there and `0.5.0` stops being
a fixed point of its own source, with no way to fix it that does not rewrite a frozen directory.
A successor that needs a new `{"$wire": …}` binding therefore **cannot have one**: bind the constant
from `xtask/src/bundle.rs` instead, which no bundle hashes (`check_aperture_additions`, added for
`0.6.0`, does exactly this for `MAX_EGRESS_APERTURES`).

**From `0.5.0` on, the focused check is `cargo xtask check-bundle <version>`, not a fifth Python
checker; the gate batches those checks through `check-bundles`.** It re-renders each bundle from
`xtask/bundle-source/<version>/` and compares bytes, so it verifies strictly more than a hand-written
checker: a released tree that is no longer the fixed point of its own source fails, whatever else
about it still looks well-formed. The four Python checkers stay for `0.1.0`–`0.4.0`, which have no
authored source tree and never will.

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

**No Substrate crate is published to crates.io.** Every workspace package sets `publish = false`;
Rust consumers use a local path or an exact Git revision. `cargo xtask check-packages` fails closed
if any member becomes publishable and retains the runtime source boundary: fixed package names,
exact internal versions, inherited SPDX metadata, checked-in READMEs and public documentation
targets. Tagged releases publish GitHub and GHCR artifacts only. Reintroducing a registry is a new
distribution decision, not a release convenience (ADR 0030).

**Pushing that tag runs [`.github/workflows/release.yml`](.github/workflows/release.yml)**, which
builds the distinct `daemon` and `mcp` runtime targets from one shared pinned builder in `Dockerfile`
and packages the explicitly pinned current development bundle with
`cargo xtask package-bundle`. It publishes
`ghcr.io/beyond10x/b10x-substrate-daemon:<version>` and
`ghcr.io/beyond10x/b10x-substrate-mcp:<version>`, plus
`ghcr.io/beyond10x/b10x-substrate-wire:<bundle-version>`; keyless-signs all three digests and verifies
all three signatures **before** it announces anything. It also exports `substrate-daemon` and the
`b10x-substrate` CLI from that same builder stage (Dockerfile target `release-binaries`) and attaches
four files to the GitHub release:
`substrate-daemon-<version>-x86_64-unknown-linux-gnu.tar.gz`,
`b10x-substrate-<version>-x86_64-unknown-linux-gnu.tar.gz`, `SHA256SUMS` over both, and
`SHA256SUMS.sigstore.json`, a keyless `cosign sign-blob --bundle` signature by the same workflow
identity, which `cosign verify-blob` checks before the release is created. The names are a public
interface that install instructions link to; do not rename them. Before any image tag moves it refuses a binary that needs a glibc
newer than 2.35 or a shared library outside the set README.md documents, or that does not start in
a digest-pinned Ubuntu 22.04 image, or an archived daemon that is not byte for byte the daemon of
the image being published (a recovery run that reuses a published image archives that image's
daemon). Only a recovery dispatch of a tag whose source predates the `release-binaries` stage
publishes without the four files, and its release notes and summary say so; a tag push without the
stage is refused. It refuses — publishing nothing — a tag that is
not the bare version form (so `0.2.2-rc.0` produces nothing), a lightweight tag, a commit that is not
an ancestor of `main`, a version that disagrees with `[workspace.package] version`, a commit for
which `gate.yml` has not concluded `success`, an existing daemon or MCP version tag on an initial
tag-push run, or an existing bundle tag whose digest differs from the deterministic local package.
An explicit protected-`main` recovery dispatch may reuse an image already written by that same
release only after its source-revision label and exact shipped binary pass again; it never replaces
the write-once tag. A later daemon release may reuse a byte-identical bundle: the workflow verifies
the digest, signs it again with that release's exact workflow identity, and never replaces the tag.
Release runs serialize globally because
consecutive daemon releases can name that same bundle. The workflow reads the recorded gate
conclusion for the tagged SHA rather than re-running a lookalike, and copies the packager's exact OCI
layout with ORAS rather than constructing another manifest at publication time. It proves the
bundle is anonymously retrievable before mutating the daemon-image tag, so correcting first-push
package visibility leaves a safe retry path.

**Two of those three images ship for adopters outside this collection, and that is deliberate.** No
repository in the `beyond10x` collection pulls `ghcr.io/beyond10x/b10x-substrate-wire` or
`ghcr.io/beyond10x/b10x-substrate-mcp`: the only in-collection consumer of the wire contract,
`harness/crates/harness-substrate`, pins the *crate* `b10x-substrate-wire` by Git revision, not the
image, and nothing outside this repository names either image (`grep -rn
'ghcr.io/beyond10x/b10x-substrate-wire'` over the collection on 2026-09-15 matched nothing outside `substrate/` and its two corrupt sibling copies, i.e. nothing outside
`substrate/`). The bundle image exists so an external consumer can pin the wire contract by OCI
digest instead of copying a directory tree (`story:contract-bundle-oci-artifact`), and the evidence
that makes that pin worth more than the copy — keyless signature, digest pin, write-once tag — has to
be collected while the bytes are development ([ADR 0006](adr/0006-substrate-publishes-its-own-contract-bundle.md),
addendum). The disposable MCP image ships for local stdio development and conformance testing. Do not
read either image's absence from the collection as an unused artifact, and do not add an in-collection
consumer to justify it.

**A release also requires a recorded delegated confinement-lane run for the tagged commit.** A green
`gate.yml` is the portable lane only: the delegated lane needs bubblewrap and an exclusive delegated
cgroup v2 subtree, which a hosted runner does not have, so that lane is reported **absent — not
executed, not passed** (§ *The gate*). Before tagging, run `bash scripts/delegated-lane.sh` at the
commit you are about to tag and put one line in the **annotated tag's own message**:

```
Confinement-lane: <source-sha> <delegated-case-count> <RFC 3339 UTC>
```

for example `Confinement-lane: 3fafeae65fe6ee9f36c3ba17d49b36a1a0d1a3a3 97 2026-09-10T22:00:00Z`.
[`release.yml`](.github/workflows/release.yml) refuses to publish without that record, refuses a
record naming any other commit, refuses a case count other than the `DELEGATED_CASES` the tagged
source itself declares in
[`crates/substrate-daemon/tests/runtime_vectors.rs`](crates/substrate-daemon/tests/runtime_vectors.rs),
and refuses a timestamp that is not an RFC 3339 UTC instant, by shape and by calendar (GNU `date`
must read it back unchanged, so `2026-02-30T10:00:00Z` is refused). The record is one line of
exactly three fields: a tag carrying two `Confinement-lane:` lines, a value with a line break or
carriage return, or a fourth field is refused. The recorded line is written into the
GitHub release under *Confinement lane* and into both job summaries. A protected-`main` recovery
dispatch may pass the same value as the workflow's `confinement_evidence` input instead, which is
the only way to release a tag that was annotated before this condition existed. This records a local
run; it does not run the lane in CI, and neither the workflow nor the release notes describe it as if
it had. Giving the lane a self-hosted runner with both prerequisites would replace this record with a
required check, and is a separate decision.

**A release also requires a recorded project-quota-lane run for the tagged commit**, in the same
shape. Before tagging, run `cargo xtask quota-fixture run` at that commit with a clean checkout
(§ *The real project-quota lane*) and put the line it prints in the annotated tag's message:

```
Quota-lane: <source-sha> <case-count> <RFC 3339 UTC>
```

`release.yml` refuses to publish without it, refuses a record naming any other commit, refuses a
case count other than the number of `#[ignore]`d `real_quota_*` functions the tagged source declares
in [`crates/substrate-host/src/git/quota_tests.rs`](crates/substrate-host/src/git/quota_tests.rs)
(the definition `declared_cases` in `xtask/src/quota_lane.rs` uses, so the fixture's record and the
step agree), and refuses a timestamp that is not an RFC 3339 UTC instant, with the same calendar
check and the same one-line, three-field rule as `Confinement-lane:`; a `Confinement-lane:` line does
not satisfy it. The line is written into the GitHub release under *Quota lane* and into both job summaries, and a
recovery dispatch may pass it as the `quota_evidence` input instead. The step's refusals are executed
offline against throwaway tagged repositories by `xtask/tests/release_workflow.rs`.

`packages: write` and `id-token: write` exist on the release job and nowhere else; that job holds
`contents: write` only to create the GitHub release. Everything it does uses the run's own
`GITHUB_TOKEN`; **the release needs no repository secret at all** (§ *Bot identity*). The GitHub
release records all artifact digests and the exact `cosign verify` and `cosign verify-blob` commands. **`0.7.7` is the published
release**: daemon `ghcr.io/beyond10x/b10x-substrate-daemon:0.7.7` at
`sha256:5e20467bd03ad7e619ffcf8664dcc6d3323c62fb62eb1f0e25f001333092a409`, disposable MCP
`ghcr.io/beyond10x/b10x-substrate-mcp:0.7.7` at
`sha256:e40c16ae56fa06353e508e5245b8d78b336ecd1cebf3fde20b8dddaf18b42b7c`, and development bundle
`ghcr.io/beyond10x/b10x-substrate-wire:0.16.0` at
`sha256:4c4e57a1b2427cb004a05cb475c1193e979777c5c79d9a9505ba5facbe10daf7`. All three were
keyless-signed, `cosign verify`-ed with the exact tagged workflow identity
(`release.yml@refs/tags/0.7.7`), and anonymously read back before the GitHub release was announced.
Do not maintain a second digest list here: the digests of every release are the GitHub release's own
([0.7.7](https://github.com/beyond10x/substrate/releases/tag/0.7.7)) and
[CHANGELOG.md](CHANGELOG.md)'s.

**Ruleset `main-requires-the-full-gate` (id 23425552, no bypass actors) requires that check on `main` (`gh api repos/beyond10x/substrate/rules/branches/main` → `required_status_checks` beside `creation`, `update`, `deletion`, `non_fast_forward` and the two email patterns); the legacy `GET …/branches/main/protection` still answers 404 `Branch not protected`, because rulesets are not branch protection.** The gate is also enforced at release time: `release.yml` reads
`gate.yml`'s own recorded conclusion for the exact commit from the Actions API and refuses to
release a tag without a `success`. The workflow still never pushes a changelog commit
or creates a `GITHUB_TOKEN` pull request whose events GitHub would suppress. After every signature
verifies and the GitHub release exists, its summary emits the exact daemon, disposable-MCP and
contract-bundle digest lines for a workstation's bot-authored pull request. That PR receives the
same gate as every other change.

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
| The plan — epics and stories, with kinds, statuses and legal moves from the `aep` CLI | `.engineering/planning/`, validated by `aep plan artifact validate` |
| Archived reviews, retained as immutable review input | `docs/reviews/archived/` |
| What shipped | `CHANGELOG.md` |

## Public website

The public site is repository-owned at `https://beyond10x.github.io/substrate/`.
`cargo xtask check-docs` validates the explicit public page allowlist; `cargo xtask build-docs
--out <empty-directory> --commit <full-sha>` builds static HTML, `.well-known/b10x-site.json`
provenance and `.well-known/b10x-routes.json`, every published route with its rendered anchor IDs
at the same commit. The organization Website reads that inventory, as it does Metaharness's, to
redirect the former `/docs/substrate/` pages; keep page paths and heading IDs stable.
`pages.yml` uploads the exact successful main artifact; `b10x-docs-site.yml` uses the pinned
standalone project-site workflow, like Mantle. Do not add unified Website bundles, `b10x.docs.yaml`
route manifests, redirect façades, or global docs reconciliation. Publish no internal plans, ADRs or
work logs.

`check-docs` also holds what the site derives. The adapter matrices, the family, fact and lane tables
and the CLI Commands table must match `spec/confinement/`, `spec/confinement-suite.json` and
`spec/generated/cli/binding.json` (`xtask/src/site_drift.rs`). The *Release notes* page renders the
newest released `CHANGELOG.md` section at build time. Site changes follow the UI iteration loop in
`website/ux/` (`xtask/src/site_ux.rs`):

- `baseline.json` holds lower-is-better ratchets: the clicks per task in `tasks.json` and the reading
  load per page. A value that improves is written back with `cargo xtask docs-baseline` in the same
  change. A worse value needs `--decision <id>`.
- Every finding is a `findings/UXF-nnn.md` record. A fixed S1 or S2 finding names a test that
  exists.
- A round's browser measurements (Playwright and axe) are taken outside the repository, and only
  their compact `rounds/R-nn/round.json` is committed.

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

Use the managed `worktree` CLI and keep primary checkouts clean. Existing worktrees and their changes belong to their active owners; never clear another session's lease.

Three such siblings predate this rule and are **not** admitted by it: `../substrate-wt/` (empty on
2026-09-15), `../substrate-wt-corrupt-20260831-MNFXtyRu/` (holds `contract-gate` and `pty-sessions`)
and `../substrate-corrupt-20260831-MNFXtyRu/` (a checkout of `wave/2026-08-30-byte-plane` with
uncommitted changes). Removing them is the operator's call, not an agent's; until the operator
removes them, they are the exception this paragraph records rather than a second admitted location.

## Bot identity

Automated commits and pushes use the GitHub App via `scripts/as-bot.sh` and `scripts/bot-gh.sh`,
never a human identity. `scripts/bot-token.sh` mints the token, and the bot-org default it applies
at `scripts/bot-token.sh:8` — `org="${B10X_BOT_ORG:-beyond10x}"` — **is** the org this repository
lives in (`git remote -v` shows `github.com/beyond10x/substrate`), so the default is right here. Set
`B10X_BOT_ORG` only to mint against a different org.

**Bot authentication does not bypass `main`'s ruleset.** A direct `scripts/as-bot.sh push origin
main` is rejected with `GH006`: the ruleset restricts `creation`, `update`, `deletion` and
`non_fast_forward` on that ref (ruleset `b10x-bot-branch-authority`, id 22044994). A rule *does*
require a status check — ruleset `main-requires-the-full-gate` (id 23425552) requires the `Full
gate` context on `main` (§ *Releases*) — but that is not what rejects the direct push, so satisfying
it is not what opens the path: pushing a bot-owned branch is. Push a bot-owned branch, open its pull
request with
`scripts/bot-gh.sh`, let `Full gate` run on that exact head, and merge through the pull request. Do
not retry the direct push or loosen the ruleset; this is the ordinary path for every
workstation-authored change, including release preparation.

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
`aep` CLI owns, and a body the agent and operator own. `.engineering/project.yaml` pins the
governing document tree to one commit of `beyond10x/aep` (the repository `engineering-protocols` was
renamed to on 2026-09-01); advancing the pin is an explicit change to that file. The `aep:planning` skill carries the current model and store conventions.

Kinds, relations, statuses and legal moves come from validated lifecycle documents. Ask the CLI —
`aep plan artifact kinds`, `relations`, `lifecycle <kind>`, `list`, `board`, `graph` — instead of
reciting them. Before the first planning-store write of a session, run `aep plan artifact list`.

1. **A status changes only through `aep plan artifact move`.** Never edit `status:` directly.
2. **Never edit a planning-store file directly.** `new` creates, `relate` links, `move` moves,
   `body <id> --from <path|->` writes prose.
3. **After a batch, run `aep plan artifact validate` and relay its output verbatim.**
4. **A refusal is an answer.** Relay the legal moves the CLI names; do not route around it.
5. **An already-satisfied or wrong request still gets an artifact** recording the finding.

New artifacts start in the lifecycle's initial state. Lifecycle moves are claims about project
state: propose them and wait for the operator unless the operator asked for the specific move.
`aep` must be on `PATH` (use the official current release); if it is absent, do not improvise machine-owned frontmatter.

A story that changes a contract or a capability still owes its ADR or design document **before
code** (invariant 8); the story body names which. `ROADMAP.md` keeps the phase order and
`STATUS.md` the observed state; the store holds the work items and their status, and nothing else
restates it.

## Standalone documentation authority

Operator decision, 2026-10-02: Substrate owns its project site, using the same static artifact publication approach as Mantle. This supersedes the former unified docs manifest, source-bundle and redirect-façade instructions for this repository. Do not reconcile this repository back into the global documentation pipeline. App credentials never belong in this public repository.

<!-- b10x-release-operations:start -->
## Release completion

An ordinary release completes after this repository's exact tag, required source checks,
published release and required artifacts are verified. A pushed tag with unfinished checks or
uploads is queued; report it as released only after those requirements succeed.

Atlas reconciliation and public documentation publication run asynchronously. Do not wait for
Atlas or Website, update Website source locks or bootstrap snapshots, promote consumer pins,
release shared docs tooling, or redeploy documentation façades as part of an ordinary source
release. Report documentation as pending unless its publication was actually verified. A background
documentation failure does not invalidate a successful source release.

Keep this repository's provenance, correctness, security, compatibility and artifact verification
requirements. Shared rendering, routing or delivery-control changes still require their relevant
integration gates. A release request does not authorize deployment or downstream releases.
Repositories without a release unit retain their existing publication policy. This completion
boundary supersedes older instructions that attach synchronous documentation ceremony to each
source release.
<!-- b10x-release-operations:end -->


## Executable specification

`spec/ess-inputs.yaml` pins ESS. `cargo xtask check-ess` validates the specification and regenerates the committed suite to check drift. The first projection is the accepted-operation ledger, with its coverage and excluded boundaries documented in `spec/coverage.md`. Existing runtime and immutable-bundle gates remain required. Never describe this bounded projection as full service conformance.

Moving the ESS pin also moves `ESS_VERSION` **and** `ESS_SHA256` in `.github/workflows/gate.yml`; the local gate uses the installed `ess` and does not check the archive, so a stale digest fails only in CI (it happened at 0.67.0 and again at 0.68.0). Take the digest from `gh release view <version> -R beyond10x/ess --json assets` and confirm it with `sha256sum` on the downloaded archive.
