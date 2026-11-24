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
   through `0.27.0` exist; `0.27.0` adds no route and moves three bounds (ADR 0013 and ADR 0010,
   amendments of 2026-11-20, issue #236): one run selects up to `MAX_EGRESS_APERTURES` (8)
   apertures, listening on `127.0.0.1` to `127.0.0.8`; the `exec.egress-apertures` fact lists up to
   `MAX_DEPLOYMENT_EGRESS_APERTURES` (12), the deployment bound that moved into the wire crate; and
   a start declares up to `MAX_READ_ONLY_ROOTS` (8) read-only roots, the fact's `max_roots`. Each is
   held to its wire constant, and the vectors that probe the run and root bounds to nine entries,
   by `cargo xtask check-bundle 0.27.0` (`xtask/src/bundle/run_bounds.rs`); the sources of `0.21.0`
   to `0.26.0` state their four as a literal and are held to it, so no released byte moved;
   `0.26.0` added no route and published design 30 (ADR 0048): one optional
   fact per confinement family, `confinement.family.<name>` in ADR 0044 decision 1's order
   (`ConfinementFamily`), and one per mechanism-neutral property, `confinement.<name>` in the
   `Fact` order of `spec/confinement/` (`ConfinementProperty`), each `const true` and absent when
   not proved; the optional closed `sandbox.requires` on both starts, which the applied record
   echoes; an optional applied `cgroup`; and two refusals at 501, `confinement.family-unserved`
   and `confinement.fact-unserved`, each published by a vector on a host machine and held to its
   `CONFINEMENT_UNSERVED_REFUSALS` row, its message computed by `confinement_requires_refusal`,
   by `cargo xtask check-bundle 0.26.0` (`xtask/src/bundle/confinement_facts.rs`);
   `0.25.0` added no route and published design 34 § 11 (ADR 0047): the
   capability snapshot's `driver` admits `kubernetes` beside `host` (`HostDriverKind`), the
   optional neutral fact `confinement.no-egress` (`CONFINEMENT_NO_EGRESS_FACT`), and three named
   refusals at 501, `exec.inline-capsule-unserved`, `exec.signal-unserved` and
   `workspace.adopt-unserved`, each published by a vector on a namespace-driver machine and held
   to its `DRIVER_UNSERVED_REFUSALS` row by `cargo xtask check-bundle 0.25.0`
   (`xtask/src/bundle/namespace_driver.rs`); the host driver publishes none of them and its
   answers do not change;
   `0.24.0` added ADR 0036 S12: one route,
   `GET /v1/processes/{process_id}/events`, a WebSocket under the `process` scope with no
   capability predicate, whose frames are `schemas/process-stream-item.json` (a record's
   `process.*` events and its log lines in one order, a `gap`, one terminal `ended`, an opaque
   per-process `pe1.` cursor that names its record, whose published pattern is held to the
   daemon's reader `ProcessEventCursor::parse`); the seven `process.*` variants of the closed
   event union; the `MAX_PROCESS_EVENT_READERS` (8), `MAX_PROCESS_EVENT_STREAMS_PER_SUBJECT` and
   `MAX_PROCESS_EVENT_STREAMS_PER_DEPLOYMENT` (512) and `MAX_PROCESS_EVENT_LINE_BYTES` (16 KiB)
   bounds; and the named refusal `process.events-readers-exceeded` (429), each held to its
   `substrate_wire` constant by `cargo xtask check-bundle 0.24.0`
   (`xtask/src/bundle/process_events.rs`);
   `0.23.0` added no route and published ADR 0010's declared read-only
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
   The daemon and Rust SDK advertise `substrate-wire/0.27.0` with the SHA-256 of that bundle's
   inner `bundle.json` (`crates/substrate-wire/src/lib.rs`); the 2026-11-20 amendments of ADR 0013
   and ADR 0010 record the bounds of `0.27.0`, design 30 records the confinement
   facts and `requires`, design 34 § 11 records the namespace-driver additions, ADR 0036 S12 the process event stream, ADR 0010 the read-only roots, ADR 0038 the writable-root
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
  another one and hold `CAP_SYS_ADMIN` in it — a seccomp program that refuses the `ioctl`
  requests that exchange or erase file blocks, pids and memory+swap
  bounds with cumulatively observed CPU, backend-identity-bound capability snapshots, output
  draining, timeout, whole-tree kill, exact capsule-byte verification, read-only `/runtime` beside a
  separate writable `/workspace`, owner-private durable state, and bounded normal/restart capsule
  cleanup. Removing or weakening any of these is invariant 3's named refusal, never a quiet
  downgrade. The refused requests are `EXT4_IOC_MOVE_EXT`, `XFS_IOC_SWAPEXT` and
  `F2FS_IOC_SEC_TRIM_FILE`, answered `EACCES`
  for every confined run and session of the host driver and the `linux` adapter, whatever the
  profile asks
  ([ADR 0050](adr/0050-the-floor-refuses-the-ioctl-requests-that-exchange-file-blocks.md)): rows
  of `REFUSED_IOCTLS` in `crates/substrate-host/src/seccomp.rs`, which the program's `ioctl`
  check is generated from and `seccomp::tests` declares a second time; the delegated lane holds
  the first with one real run on ext4 (`floor_refuses_block_exchange`), required on a hosted
  runner and absent only where a workstation's staging file system is not ext4, and nothing was
  run on XFS or f2fs. The `oci` and `pod` adapters keep their runtime's default seccomp profile,
  which passes all three, and `local` has no filter.
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
  documents that runner mints at run time. This holds for bundles `0.17.0` through `0.27.0`. The exception is scoped by rule
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

In order: `cargo xtask check-ess`, `cargo xtask check-docs`, `cargo fetch --locked` (every
target's dependencies, which the offline `cargo tree --target all` cases need), the test step
(`cargo nextest run --workspace --release --locked --no-run`, the same command with
`--test-threads 12 --retries 0` in place of `--no-run`,
`xtask slowest-tests` and `cargo test --workspace --release --locked --doc`; *The test step's
budget* below), `cargo fmt --all --check`,
`cargo clippy --workspace --all-targets --release --locked -- -D warnings`, the host driver without
its default features (`cargo clippy -p b10x-substrate-host --no-default-features --all-targets`,
then `cargo nextest run -p b10x-substrate-host --no-default-features` over `host_features`,
`ess_confinement_local` and `consumer_surface`, both `--release --locked`, § *Releases*), then
`cargo xtask check-links`, `cargo xtask check-adrs`, `cargo xtask check-secrets`, `cargo xtask check-advisories`,
`cargo xtask check-licenses`, `cargo xtask check-packages`, `cargo xtask check-mcp-boundary`,
`check-contract-bundle.py`, `check-contract-bundle-0.2.0.py`,
`-0.3.0.py`, `-0.4.0.py`, one bounded
`cargo xtask check-bundles 0.5.0 ... 0.27.0`,
`cargo xtask check-json` and `cargo xtask check-toolchain`.
Green here is the bar for `main`.

**A change does not buy confidence with CI time.** A test runs its scenario once and finishes in
seconds. Repetition counts, soak loops and load generators are local evidence recorded in the
planning store, never a test, a CI step or a script the gate calls. A story whose acceptance needs
"N runs under load" proves it on a workstation and records the run; the case it adds to the suite
runs once.

**No build or test job on the shared workstation runs over five minutes.** Timo, 2026-11-22 about
04:55 CET, relayed by the session that maintains the build pool: "There is no good reason
something compiles or tests longer than 5 minutes." A job known to go past that is not started.
A mutation run does not rebuild a crate per mutant: at most one mutant per guard, each as one job
that builds and runs only the test binary that should kill it, and the others are listed as not
run. A cancelled mutant job leaves its mutant in the source and its binary in `target/`: compare
the file with the tested state before anything else, and run no test binary by path afterwards
(2026-11-22: a cancelled job left a changed constant in `crates/substrate-host/src/seccomp.rs`).
Every sub-agent brief names this bound beside the 60 s and 120 s bounds of the collection.

**The test step's budget: the run ends within 120 s on the hosted runner, and no test runs longer
than 60 s** (`story:gate-test-step-ends-within-two-minutes`). The step is four commands:

1. `cargo nextest run --workspace --release --locked --no-run` builds every test binary and every
   binary a test starts, `substrate-daemon` among them. The compile is not in the budget.
2. `cargo nextest run --workspace --release --locked --test-threads 12 --retries 0` runs every
   test in its own process. This is the 120 s, and the runner holds it: at 120 s it kills what
   still runs, and the run fails.
3. `target/release/xtask slowest-tests --junit target/nextest/default/junit.xml` prints the ten
   slowest tests of that run, whether it passed or failed. It is the release `xtask` step 1 built:
   `cargo xtask` there would compile a second one.
4. `cargo test --workspace --release --locked --doc` runs the doctests, which cargo-nextest does
   not run. Nothing kills a doctest at 60 s and nothing ends this command at 120 s. Both doctests
   are compile-only today; one that runs code needs a bound before it is added.

`scripts/gate.sh` prints `gate-time: test build: <n> s`, `gate-time: test run: <n> s` and
`gate-time: doctests: <n> s`, so the run's time is read apart from the compile.
[`.config/nextest.toml`](.config/nextest.toml) holds both bounds, so they apply to every
`cargo nextest run` here, on a workstation and in CI:

- `slow-timeout` with a 30 s period, `terminate-after = 2` and `grace-period = "0s"`: a test is
  reported slow at 30 s and killed with `SIGKILL` at 60 s, and a killed test fails the run. The
  grace period is zero because cargo-nextest otherwise sends `SIGTERM` and waits 10 s, so a test
  that ignores the signal would run 70 s.
- `global-timeout = "120s"`: tests still running at 120 s are killed the same way and the run
  fails. The clock starts when the first test starts, after cargo's freshness check and the
  listing of the tests; `gate-time: test run` is the whole command and includes both, a few
  seconds. The compile is another command and outside the bound. A run over 120 s is a defect to
  fix (*An expensive product is made once*, below), never a bound to raise.
- `test-threads = 12`, more than a hosted runner's four CPUs on purpose: the suite's time is
  mostly waiting, so tests overlap where CPUs do not (measured on four CPUs, the four largest
  binaries: cancelled at 120 s with 4 threads, 48 s with 12, 103 s of CPU for 530 s of test time).

**The gate names the thread count and the retries on its command line, because a variable of the
caller wins over the configuration file.** cargo-nextest reads `NEXTEST_TEST_THREADS` and
`NEXTEST_RETRIES` before `.config/nextest.toml`. `scripts/gate.sh` therefore reads `test-threads`
from that file, its one home, and passes it as `--test-threads`, with `--retries 0`, on both of
its test runs: a caller's thread count does not decide the 120 s, and a caller's retries do not
turn a failed test into a passed one. A `cargo nextest run` typed by hand has neither option: in
a job of the workstation's build pool it runs with the pool's `NEXTEST_TEST_THREADS`, a default
for CPU-bound tests, unless the command names its own.

**A local run of the test phase asks the pool for the CPUs it uses:**

```console
SLOT_TOKENS=3 bash scripts/gate.sh tests
```

Three, because twelve threads of mostly waiting tests used about 2.3 CPUs (the 103 s of CPU over
530 s of test time above). The time a job waits in the pool's queue is before the run and outside
the 120 s. `gate.yml` installs cargo-nextest from its release archive, pinned by
`NEXTEST_VERSION` and `NEXTEST_SHA256`; moving one moves both.

What the budget asks of a test:

- **An expensive product is made once per run, never per test.** Every test is its own process,
  so a `OnceLock` is filled once per *test*, not once per binary. A product that costs more than a
  fraction of a second has one of two forms: a setup step before the tests, or a cache on disk
  under `target/`, keyed by the digest of its sources, built under a file lock and not writable by
  a confined run a test starts. Both products of this kind today are made by a setup step:
  - the daemon a test starts, by step 1. `test-support/daemon_binary.rs` finds it:
    `SUBSTRATE_TEST_DAEMON`, else the one beside the test executable, else the test fails and
    names the step. An empty variable names none. A test never runs `cargo build`: a
    package-scoped build resolves other features than step 1, recompiles what step 1 built and
    replaces the daemon other tests are starting. A package-scoped run of the CLI's tests
    (`cargo test -p b10x-substrate-cli`) therefore starts whatever daemon an earlier build left
    beside it, which may be older than the source: build the workspace's tests first;
  - the image the oci cases run, by `gate.yml`'s pull step. A case asks the runtime's store first
    and pulls only an image the store does not hold, so the test step makes no registry request.
- **A test binary with its own `main` (`harness = false`) answers what cargo-nextest asks of
  libtest**: `--list --format terse`, and `--exact <name>` to run one case.
  `crates/b10x-substrate-conformance/tests/local_endpoint.rs` is the one such binary.
- **Tests that cannot run beside each other share a test group** in `.config/nextest.toml`, never
  a global thread limit. A `static` lock in a test file orders only that one process. The groups:

  | group | limit | tests | why |
  |---|---|---|---|
  | `oci-runtime` | 1 | `ess_confinement_oci`, `adversary_oci` | opening the `oci` adapter removes every container whose embedder is gone; `open_removes_a_container_whose_embedder_is_gone` makes such a container and requires its own open to count the removal, so no other open may run beside it |
  | `systemd-user-manager` | 1 | `ess_confinement_systemd`, `security_review_systemd`, `adversary_systemd` | opening the `systemd` adapter kills and stops every leftover scope of a dead embedder under the one user manager; `a_dead_embedders_run_is_ended_and_its_scope_reclaimed_by_the_next_open` requires its own open to count the reclaim |

- **A test build carries no debug info.** The release profile, which the gate builds in, has
  none; `[profile.dev] debug = false` in `.cargo/config.toml` holds the dev profile to the same,
  which `scripts/delegated-lane.sh` and `cargo xtask` build in. It is set there and not in
  `Cargo.toml` because a cargo configuration file wins over the manifest, and a user's own may
  name another value; the repository's is nearer and wins over that one. Read the effect from a
  built binary (`readelf -S`: no `.debug_*` section), never from the manifest. A backtrace then
  names functions without lines; a panic still names its file and line.
- **Before a test is added it is timed.** A test over 5 s is named with its time in the change
  that adds it.

`scripts/delegated-lane.sh`, `scripts/pod-lane.sh` and the release lanes select their cases by
name with `cargo test`; they are lanes, not this step, and are not under this budget.
The former brand is fenced org-wide by `scripts/check-org-brand.sh` in the **atlas** repo, not here.

**The gate's own checks are `cargo xtask` verbs**, in the `xtask/` workspace member — anything that
runs in a b10x foundation repository is Rust (`atlas/AGENTS.md` § *Language*) — while the four
frozen `render-contract-bundle*.py` / `check-contract-bundle*.py` pairs stay Python as the released
bundles' reproducibility proof (invariant 6), not as tooling.

| verb | what it refuses, or produces | in `gate.sh`? |
|---|---|---|
| `check-ess` | an ESS specification under `spec/` that the pinned `ess` does not validate with `--strict-requires`, a scenario of the workspace, host-process or confinement projection that fails against the interpreted reference, or a committed projection that is not what the pinned `ess` regenerates: `spec/operations-suite.json`, the workspace, host-process and confinement suites, their generated Rust types, the capture type and the CLI contract under `spec/generated/cli` (§ *Executable specification*) | yes |
| `check-docs` | a public page missing from `PAGES` or from `website/sidebars.ts`, or out of their order or grouping; a release-notes page that does not end with the newest released `CHANGELOG.md` section; a start-page link that does not resolve; a derived site table that disagrees with `spec/`; a page or `STATUS.md` that names a release, bundle or pinned tag other than the current one, or a fact list other than `FACT_ORDER`; a site stylesheet or start page that breaks UXF-008, UXF-013 or UXF-015; a reading load above `website/ux/baseline.json`; or a `website/ux/findings/` record that breaks its rules (§ *Public website*) | yes |
| `check-toolchain [--root <dir>]` | a Rust version the three pinning files disagree on | yes |
| `check-links` | a machine-local Markdown link, or a repository-relative target that is not there | yes |
| `slowest-tests --junit <file> [--count <n>]` | prints the slowest tests of a cargo-nextest run from the JUnit report it wrote, ten unless `--count` says otherwise, each with its seconds, its test binary and `FAILED` when the run failed or killed it; refuses a missing report and one that is not what cargo-nextest writes | yes, as `target/release/xtask`, after the test run (*The test step's budget*) |
| `check-mcp-boundary` | a publishable MCP crate, a Substrate implementation dependency, an unpinned/featured MCP stack, or a built-in unbounded/remote transport | yes |
| `mcp-codex-smoke --server <path>` | manually asks an authenticated ephemeral Codex run to exercise the shipped stdio adapter | no |
| `check-adrs` | an ADR whose identity, frontmatter, index row or supersession link does not agree | yes |
| `check-secrets` | a reachable Git object carrying a credential, or an incomplete history scan | yes |
| `check-advisories` | a RustSec vulnerability or forbidden HTTP/2 dependency | yes |
| `check-licenses` | non-Apache workspace metadata, an unreviewed dependency licence or third-party notice drift | yes |
| `check-packages` | a publishable workspace package, a loose internal version edge, a source runtime package without inherited SPDX metadata, its README and a public documentation target, a host driver feature other than `git` and `kubernetes` or a default other than both, a host normal tree without default features (every target) that names gix, git2, libgit2-sys, curl, curl-sys, openssl-sys or the Kubernetes client, or — invariants 1 and 2 — a Flux dependency or a `beyond10x` Git dependency in any dependency table, `[workspace.dependencies]` and `[patch.*]` included; or a `[patch.crates-io]` entry that `Cargo.lock` does not resolve to: the lock file holds no package of the entry's name and version, or resolves that package from a source that is not the entry's, so the build would not take the entry's source; or a second package of a patched name from any source: beside the entry's package the lock file holds another package of that name, whatever its version, from the registry, a `sparse+` registry, a Git source or another path, so a dependent of that one would not build the entry's source; or, since only `[patch.crates-io]` in `Cargo.toml` carries a patch, another `[patch]` key or a `[replace]` in `Cargo.toml`, an entry `Cargo.lock` lists under `[[patch.unused]]`, two entries of one package name, or an entry with neither a `path` nor a `git` source; or a top-level key other than `alias` and `profile` in a cargo configuration beneath the root, a fixture that is a workspace of its own included (a test that builds a fixture offline writes its source replacement into the copy it runs), an alias that is not a string or an array of strings or that holds `--config` or `--config=<value>`, or a `.cargo` directory holding both `config` and `config.toml`, a link included; outside the check are configuration outside the repository (a file above the root, `CARGO_HOME`, `--config` on a command line or in a script or workflow, `CARGO_*` variables), files beneath a directory named `target` or `.git`, and what `profile` sets (`vendor/axum-0.8.9` and `vendor/gix-pack-0.74.2` are the two entries) | yes |
| `package-bundle <version> --out <dir>` | produces a released bundle as a deterministic OCI image layout | no — under `cargo test` |
| `render-bundle <version> --out <dir>` | produces a bundle tree from `substrate-wire` and `xtask/bundle-source/<version>/`; refuses to write anywhere under `contracts/` | no — under `cargo test` |
| `check-bundle <version>` | a released bundle whose bytes are not the fixed point of `xtask/bundle-source/<version>/` | no — focused form of the batched gate check |
| `check-bundles <version>...` | the same checks with a bounded worker count and deterministic version-order reporting | yes, `0.5.0` through `0.27.0` in one invocation |
| `check-json [<version>...]` | JSON beneath a released bundle that no bundled schema classifies, that its schema rejects, or that is not in deterministic source form | yes, all twenty-seven |
| `namespace-lane check [--bless]` | a lane object that differs from its golden in `xtask/tests/golden/namespace-lane/`, a Role that is not ADR 0047 decision 3's table as the ADR writes it, or a driver-side object that is cluster-scoped or lacks a design 34 § 4 ownership key | no — `.github/workflows/namespace-lane.yml`, and its cases under `cargo test` |
| `namespace-lane observe [--admin-kubeconfig <file>] [--executor-image <ref>]` | runs design 34 § 10.3's O1 to O4, O6 to O8 and O10 on a kind cluster of its own (`substrate-namespace-lane-<id>`) through `b10x-substrate-kubernetes-client`, never `kubectl`. It then runs design 34's steps 3 to 7 against the shipped daemon with `--driver kubernetes`: it builds that daemon and the vectors' test binary before the cluster exists, and pushes the executor image to the lane's registry unless `--executor-image` names one, which `--admin-kubeconfig` requires. It also refuses a kubeconfig whose server is not `https://127.0.0.1:<port>`, and never reads `KUBECONFIG`; a given one is refused before it is read under `~/.kube` (of `HOME` and of the uid's passwd entry) or as a file `KUBECONFIG` names, then for a cluster or user member kind never writes (`tls-server-name`, `proxy-url`, `exec`, `auth-provider`), a context or cluster not `kind-substrate-namespace-lane-<id>`, and, before any write, no node `substrate-namespace-lane-<id>-control-plane`; prints `Namespace-lane: <sha> <count> <RFC 3339 UTC>` only when every observation ran at a clean, unmoved `HEAD`, every step case passed or was declared absent with its reason, what the run created was removed and no signal arrived; `<count>` is still the observations' | no — a workstation run |
| `chart-lane static [--helm <path>]` | a rendering of `deploy/helm/substrate` that breaks ADR 0046 (a second Service selecting the pods, a Service certificate name, an unretained ledger claim, a token, a capability, host namespace or host path the profile does not grant, an open NetworkPolicy rule), or a privilege, cgroup, mount, quota, transport or egress posture the chart renders instead of refusing | no — `chart-lane.yml` |
| `chart-lane cluster --image <ref> --node-image <ref>` | creates and deletes its own kind cluster, then refuses a pod that is not ready on its own pool node, a certificate that verifies under the Service name, a ledger that does not survive a rolling restart, an undeclared egress that arrives, an execution pod `baseline` admits, or an uninstall that leaves more than the ledger claims; a scenario it cannot prove is reported absent. `--node-profiles` (K9 C, a workstation) runs one worker built from `fixtures/chart-lab/Dockerfile` and also refuses an image whose `org.opencontainers.image.revision` is not `HEAD`, a recipe not built on `--node-image`, and a worker without `AppArmor`, without `/sbin/apparmor_parser`, already holding the run's own profile name in the shared kernel, or without a cgroup v2 `cpu`/`memory`/`pids` hierarchy, naming every scenario that did not run; it names its profile `substrate-host-exec-v2-lane-<ulid>`, fresh per run (design 33 § 7), installs the execution-enabled chart with the node profiles under that `execution.appArmorProfile`, runs the four execution scenarios, removes only that profile, and prints `Chart-lane:` only when every scenario in `SCENARIOS` passed and the removal succeeded at an unmoved, clean `HEAD` | no — `chart-lane.yml`; `--node-profiles` by hand before a release |
| `package-chart --version <v> --image-digest <d> --out <dir>` | produces the released chart, a byte-for-byte reproducible archive, with its version set and its default image pinned to the daemon digest | no — `release.yml` |

**`cargo xtask package-bundle <version> --out <dir>`** packages a released bundle as a
deterministic OCI image layout. It is not a gate step of its own: its cases run under
the gate's test step (`cargo nextest run --workspace --release --locked`).

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
= 60), which a schema binds with the `{"$wire": "CONSTANT"}` marker, plus the daemon- and
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
the wire; the gate's test step runs it. Its portable lane asserts three named
refusals — `exec.sandbox-unavailable` (501), `exec.secret-slots-unserved` (501) and
`exec.secret-slot-descriptor-invalid` (422) — plus `session.pty-unserved` (501), the five lexical
`exec.read-only-file*` refusals (422), `exec.read-only-files-unserved` (501),
`process.host-unserved` (501), the two lexical `exec.workdir-*` and five aperture-list refusals
(422) of design 26, `exec.workdir-unserved` (501) and `exec.egress-apertures-unserved` (501) for
a list, and the six lexical writable-root refusals (422) of ADR 0038 with
`exec.writable-roots-host-unserved` and `exec.writable-roots-git-unserved` (501), which both lanes
answer for a host root the daemon did not declare and for every Git root, and the portable lane
also for the one host root the runner declares, which it cannot prove without confinement — across
136 cases, and its delegated lane 201, whose four writable-root cases bind that declared root and
check the host side: the write lands, its parent and sibling do not change, a working directory
beneath the mount runs there, and the declared path swapped for a symlink is
`exec.writable-root-not-a-directory` (422). Its read-only-root cases (ADR 0010, bundle `0.23.0`): portable, the `exec.read-only-roots` fact is absent, one root is refused `exec.sandbox-unavailable` (501) by the daemon before dispatch and nine roots are `exec.read-only-root-invalid` (422), the lexical rule checked first; delegated, the fact is `{"max_roots": 8}` (bundle `0.27.0`), eight roots are bound, read and refused writes, listed in the applied record and left unchanged on the host, a ninth is `exec.read-only-root-invalid` (422), and an absent host path is `exec.read-only-root-absent` (422) while a plain file and a symlink to a directory are `exec.read-only-root-not-a-directory` (422). Both totals also count a second daemon that declares a
Git source served over HTTPS on loopback by `git upload-pack` (ADR 0038 kind 2): it serves
`workspace.git` and materializes a Git workspace; the portable lane then refuses that workspace as
a Git root `exec.writable-roots-git-unserved` (501), and the delegated lane mounts it, has a child
commit in it from a working directory beneath the mount, reads the commit on the host and in the
change-set route, and refuses an empty workspace, the start's own, one named twice and an unknown
one before dispatch, each recorded as refused. Both lanes also hold four omitted-entry cases (ADR 0043): a
FIFO beside a file is named by the `x-b10x-omitted-entries` header, count 1, on the v1 directory
page and on the v2 tree both under its limit and when the limit truncates it, and once the FIFO is gone
neither listing carries the header and the tree is not truncated.
Both lanes also hold twelve root-page cases (ADR 0052): the path `.` on the v1 directory page
lists an empty root as one empty page; seven children at three a page come back complete, in name
order and each once, without the file beneath a directory; `.` is refused
`workspace.path-escape` (422) in file mode, on a write, on a delete, on every v2 file route and as
a component (`a/.`, `./a`, `..`), each mutation recorded as refused; a cursor past the end is
`workspace.cursor-invalid` (422); and, with entries the runner makes on the host, a root of 2,001
files is complete at 1,000 a page, a FIFO is counted on every page, a symbolic link is listed as a
link and its own page is refused, and `.git` is neither listed nor counted.
Both lanes also hold design 30's `sandbox.requires` cases (bundle `0.26.0`): the machine
publishes the families and properties the `linux` adapter proved and never an adapter name
(portable: `workspace` and `audit`; delegated: every family but `network-ingress`, with
`whole-tree-kill`, `ipc-socket-allowlist` and `ipc-signals-contained`), a start requiring
`network-ingress` is `confinement.family-unserved` (501), one requiring a property not proved
`confinement.fact-unserved` (501), and an unknown name, `{}`, an empty list and a duplicate are
`request.schema-invalid` (422), each recorded as refused; the delegated lane also runs one admitted
start whose applied record echoes `requires`, and refuses a session start requiring
`network-ingress`.
Both totals include
one route/refusal probe for every operation in the promoted registry (the seven host-process routes
of `0.20.0` among them: unserved on the portable lane, served and refused before anything runs on
the delegated one; the process event stream of `0.24.0` answers an absent record
`process.not-found` before the upgrade on both, and the delegated lane executes its vector
`process-events-beyond-the-readers-bound-is-exhausted`, holding a started record's readers bound
open and comparing the refusal with the vector's response), and an operator's directory adopted through `workspace.adopt` (ADR 0035): its
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
(`crates/substrate-daemon/tests/runtime_vectors.rs`, `PORTABLE_CASES` and `DELEGATED_CASES`). Each
lane spans five test functions of that binary, so that none outlasts the 60 s test budget: the
journey, the two exec-retention cases (delegated only), the adopted workspace and writable Git roots.
Each starts a daemon of its own and asserts its own count (`LaneCases`); the file does not compile
unless the counts sum to the two totals, and `scripts/delegated-lane.sh` refuses a run whose
functions did not report the whole delegated inventory. Its delegated lane runs only when
`SUBSTRATE_VECTORS_CGROUP_ROOT` names a delegated cgroup v2 subtree the test process is inside;
unset, those cases are **absent, never reported as passed** (invariant 3). **`bash scripts/delegated-lane.sh`
runs that lane plus the host, host-process (`--test processes`), public-SDK and disposable-MCP delegated cases, and needs no privilege** — it asks
systemd for a delegated scope, moves itself into a
child group so the delegation root stays process-free, and sets the variable. Do not conclude the
delegated lane cannot run here: a user session's own scope is root-owned, so `mkdir` in it fails,
and an absent lane looks identical to a green one if you only read `cargo test`.

**The delegated lane also proves the executable allow-list** (design 35). The helper is the
workspace member `crates/substrate-exec-gate` (package `b10x-substrate-exec-gate`, binary
`substrate-exec-gate`, no dependency but `libc`), and the `linux` adapter uses it only where
`HostConfig.exec_gate` names it; unset, `exec_subpaths` is not proved and a profile with a list is
refused `confinement.fact-unserved`. **The helper is statically linked** (D29): at the open the
driver tests the bytes it then seals and executes, and refuses `config.exec-gate-invalid` unless
they are an ELF image of its own class, byte order and machine, with a header the kernel's
loader takes (the type `ET_EXEC` or `ET_DYN`, the class's program header size, and at most one
4 KiB page of program headers), whose program header table was read whole and holds no
`PT_INTERP`
(`substrate_exec_gate::elf_linkage`, `check_linkage` in `crates/substrate-host/src/exec_gate.rs`);
a script is refused by the first of the four, and an image the kernel answers `ENOEXEC` for by
the second, because bubblewrap's `execvp` would run a shell of the view with the file as its
script. A loader would run before the helper's first instruction and read what a
profile can mount (`/etc/ld.so.preload`). A static-pie image passes; `PT_DYNAMIC` is not asked.
For the same reason a start with a list and a controlling terminal is refused
`session.pty-unserved` before dispatch: `setsid --ctty` would run before the helper. And on a
start with a list a declared mount at or beneath the helper's own, `/.substrate-exec-gate`, is
refused before dispatch by the code its kind has for a mount the driver owns
(`exec.read-only-root-invalid`, `exec.read-only-file-mount-reserved`,
`exec.writable-root-mount-reserved`; `listed_mounts_leave_the_helpers` in
`crates/substrate-host/src/process.rs`); a start without a list is judged by the wire crate's
rules alone, as before.
`scripts/delegated-lane.sh` builds the helper for the host's musl target
(`x86_64-unknown-linux-musl`, which `rust-toolchain.toml` lists and `release-lanes.yml` adds),
checks with `readelf` that it has no `PT_INTERP`, copies it to `substrate-exec-gate.lane` beside
cargo's output and exports that path as `SUBSTRATE_EXEC_GATE`: cargo's own file has a second hard
link under `deps/`, which the driver refuses too. Where the static build fails the lane ends by
name; it never falls back to a dynamic helper. It also names a copy of cargo's default,
dynamically linked build in `SUBSTRATE_EXEC_GATE_DYNAMIC`, for the one case that requires the
open refused. The lane then requires, by each case's own `executed` line and with
no `ABSENT` line, the `exec` cases of `ess_confinement_linux` and `ess_confinement_systemd`, the
adversary and security-review roots `adversary_exec_lists`, `security_exec_gate`,
`security2_exec_gate` and `security3_exec_gate`, and the helper package's `gate`,
`adversary_gate`, `security_gate` and `security2_gate`. **Eleven roots make their own mounts**
(D30: what is mounted beneath a location a run can write lies in it; every symbolic link a
followed entry reads and every directory it passes through is placed, and every entry where it
ends; a read-only root that shows a location a run can write is asked as one): the host crate's
`exec_lists_view_mounts`, `security5_view_mounts`, `adversary5_view_mounts`,
`adversary6_view_mounts`, `security6_view_mounts`, `adversary7_view_mounts` and
`security7_view_mounts`, and the helper package's `view_mounts_gate`,
`adversary5_view_mounts_gate`, `adversary6_links_gate` and `adversary7_walk_gate`. Each case mounts
in a user and mount
namespace of its own, with no privilege. The lane (`require_staged`) requires every case by
name: its `executed` line where `unshare --user --map-root-user --mount` can mount a tmpfs,
which a workstation grants. Where it cannot (a hosted runner, whose AppArmor gives a user
namespace no capability) the lane takes each case's own `ABSENT (` line instead and says in
one line which cases did not run: on `release-lanes.yml` and `gate.yml` those cases are
absent, and the evidence for them is a workstation's lane. Seven cases depend on something
else a host may lack (a mount this user cannot open, twice; `/usr/local`, twice;
`/usr/local/src`, twice; an
overlay in a user namespace); their `ABSENT (` line is taken on any host where it gives that
reason, and named in the same line. One case of those roots makes no mount, the trusted link
through the workspace of `security6_view_mounts`, and its `executed` line is required on every
host. What these cases stage is held on every host by cases on
mount tables written as text: `MountTree::reaches` and `reaches_as_written` in
`crates/substrate-exec-gate/src/place.rs`, the helper package's `adversary5_mount_tree` and
`adversary6_reach`, which the lane requires too, and `writable_source`, `class_together`,
`judge_link` and `judge_passed` in `crates/substrate-host/src/exec_gate.rs`, whose callers are
held on real files by the driver's unit cases with a mount table in which the files' own mount
has a type outside the list. The helper's walk of a followed path is held with no mount by
`security6_gate` and `security7_gate`, which the lane requires with no `ABSENT` line; the last
case of the first and the one case of the second run the helper under `/usr/bin/strace`, and
the second moves a directory under the helper's walk. The trace
case of `security3_exec_gate`
runs the driver's own start under `/usr/bin/strace`, which `release-lanes.yml` installs, and is
absent, so refused by the lane, without it. **An entry admits a file by where the file itself
lies**: a symbolic link in a listed directory that points outside it is not admitted
(`limit_a_link_in_a_listed_directory_that_points_outside_it_is_not_admitted` in the helper's
`gate` root). `gate.yml` runs on `ubuntu-latest` (Ubuntu 26.04 on 2026-11-21, where `ln`, `env`,
`cat`, `ls` and `mkdir` in `/usr/bin` are links into `/usr/lib/cargo/bin/coreutils`), and
`release-lanes.yml` pins `ubuntu-24.04`; the host crate's lane cases list the directories of
their tools through `base_directories()`, so they assume tools that are regular files in those
directories and would fail on a 26.04 runner. Those cases need Landlock ABI 2
(Linux 5.19) and the workspace root, `/usr`, `/bin`, `/lib`, `/lib64` and the temporary directory
on `ext4`, `overlay` or `tmpfs`; the preload cases and the stand-in helpers also need a C
compiler named `cc` that links statically. Without the
delegated root the host cases print `ABSENT`; the helper's own cases start cargo's dynamically
linked test build with no driver around it, need no delegated root and
run under the gate's test step wherever the kernel has Landlock, absent otherwise. On a start
with a list the run's variables are operands of the helper (`--env <name> <value>`) and bubblewrap
gets no `--setenv` (D28); a start without a list keeps its argv, which
`a_start_without_a_list_builds_the_argv_it_built_before` in `crates/substrate-host/src/process.rs`
holds.

**The same helper enforces the deny-list** (design 35 § 8). `ConfinementProfile.exec_denied_paths`
holds at most `MAX_EXEC_DENIED_PATHS` (64) entries, and a denied path wins. Landlock has no deny
rule, so `--deny` makes the helper grant each entry of a listed directory on its own where the
directory holds a denied path (`grant` in `crates/substrate-exec-gate/src/landlock.rs`); whether
one path holds another is decided by place, never by path text. The fact `exec_denied_paths`
stands directly after `exec_subpaths`, is proved by a second open-time probe
(`probe_exec_denied_paths`) only where the first held, and is not published: it has no wire
property (`PROVED_WITHOUT_A_WIRE_PROPERTY` in `crates/substrate-host/src/probe.rs`). A deny-list
beside an empty `exec_subpaths` is `confinement.exec-denied-paths-without-subpaths` (422) on
every adapter, before any fact is read. The lane requires the `exec deny` lines of
`ess_confinement_linux`: probes K2 and K8 through the driver, the acceptance, and the two cases
kept at the stakeholder's request, a program that is a link to a denied program and one directory
under two names. The base view's form of the second needs a host whose `/bin` is a link to
`/usr/bin` and where `/usr/bin/id` is a file of that directory; elsewhere it is absent, so
refused by the lane. **A deny entry holds across the view's mounts** (D30): what is mounted
beneath a denied directory is denied with it, and a listed directory with a denied place in a
mount beneath it is expanded. The helper's `step` and the driver's `judge_denied` ask
`MountTree::reaches` for it; a mount beneath a denied directory, or beneath a listed directory
of a start with a deny-list, that is covered or whose root was removed refuses the start
`exec.sandbox-place-unserved` (the helper: status 125), and so does a type outside the list
beneath a denied directory. The deny probe binds a directory no entry lists beneath a second
listed directory (`PROBE_HELD_MOUNT`), and
`ESS confinement linux exec deny probe held mount` holds that a helper which misses it proves
the allow-list alone. Both functions are held on every host by cases on mount tables written
as text (`landlock::tests`, `exec_gate::tests`). **Ten more roots are required through
`require_staged`**, by the rule of the eleven above: the helper package's `namespace_gate`
(a host bind mount, a tmpfs, an overlay, and the limit of an overlay listed beside its lower
directory), `adversary4_namespace_gate`, `adversary5_denied_reach_gate` and `security4_gate`,
and the host crate's `security4_exec_denied`, `security5_exec_denied`,
`security6_exec_denied`, `adversary4_exec_denied`, `adversary5_exec_denied_round2` and
`exec_denied_view_mounts`.
Their cases
that mount do so in a user and mount namespace of their own. Where that is granted no mount
they print `ABSENT (` and the lane names them in its one line: on `release-lanes.yml` and
`gate.yml` they are workstation evidence, never a passed case. The cases of those roots that
make no mount are required on every host, and so is `adversary4_gate`, with no `ABSENT` line.
One case is absent on every host, `adversary4 exec deny nested host mount`, which mounts in
the test process itself; `exec_denied_view_mounts` stages its shape. An absence a case
names for something else the host lacks is taken only with that reason: an overlay case with
`no overlay mount in a user namespace on this host`, never with `the stage failed`. **In a
listed directory a run can write, a deny entry holds for the files that exist at the start,
under the names they have then** (§ 8.2, § 8.5). A directory inside an expanded directory
that holds no denied path gets one rule, what the run creates in it executes, and the rule
moves with a rename, so a file the run creates or moves can run under any name, the denied
name included. A deny-list is exact only where the run cannot write. Two `limit_` cases hold
the figures: `limit_a_deny_entry_the_run_creates_by_a_link_or_a_rename_runs` in
`adversary4_exec_denied` (`copied 126`, `linked 0`, `renamed 0`) and
`limit_a_file_the_run_creates_or_moves_runs_under_a_denied_name` in `security5_exec_denied`
(`copied after a rename 0` and eight more). **A mount of a file-system type outside the list
that is an entry of a directory a start expands refuses the start before dispatch**,
`exec.sandbox-place-unserved` at `exec_subpaths` (`judge_denied` in
`crates/substrate-host/src/exec_gate.rs`). What decides is where the deny entry is: the
helper expands only the directories on the way to one, so the same mount one directory
further away refuses nothing (design 35 § 6, amended 2026-11-22). Nor does one whose mount
point is gone beneath a mount laid over a directory above: the helper never meets it. **The
driver also asks what the root of a mount of a listed type reaches where the helper would
step that mount as a directory**, and refuses the start the same way where that cannot be
established: by place the root holds the mounts attached in its file system under another
mount of it, which are not beneath the listed directory. The walk is by the mount tree
(`MountTree::listed_entries` in `crates/substrate-exec-gate/src/place.rs`), so a mount the
helper's walk would not meet is not asked. `security6_exec_denied` holds both directions in
fourteen starts of one staged case. A start with a deny-list
reads every directory it expands, and ends with status 125 where one cannot be read; its
standard error is then the one line
`substrate-exec-gate: cannot read the directory "<path>": <cause>`, a form consumers match,
held by `limit_a_directory_the_expansion_cannot_read_ends_the_run_125_and_names_it` in the
lane (`ESS confinement linux exec deny limit unreadable directory`).
`macos` renders the list as Seatbelt deny rules
after every allow (`render_exec_denied` in `crates/substrate-host/src/seatbelt.rs`); its cases
run only in the hosted macOS check, and how Seatbelt matches a deny entry in another letter
case on a case-insensitive volume is not verified.

**The runner has a third lane, the namespace lane.** It runs only when
`SUBSTRATE_VECTORS_KUBERNETES_CONFIG` names a namespace driver's kubeconfig. It starts the shipped
daemon with `--driver kubernetes` on its Unix socket, as deployment `dep_namespace_vectors`, and
runs `NAMESPACE_CASES` (64) once. These are selected by the facts a kind cluster publishes: a
probed snapshot with `confinement.no-egress` absent, the workspace and file routes, the promoted
route inventory, the eight root-page cases the wire alone sets up (ADR 0052; a FIFO, a symbolic
link and `.git` at the root are absent, because no route of this lane makes one),
`exec.sandbox-unavailable` (501) for an exec, and a destroy that reaches 404.
Unset, the lane prints that it is absent and that no case ran, and is **never reported as
passed**. Only `cargo xtask namespace-lane observe` sets the variable, as its step 5
([`xtask/src/namespace_lane_run.rs`](xtask/src/namespace_lane_run.rs)). Beside it, that verb runs
ADR 0047 decision 13's driver-specific cases once each and names every unreachable case absent with
its reason. The recorded run: `Namespace-lane: df7c6f57709cd03786ce271d5d5cd5e302022688 8
2026-10-29T10:40:20Z`, 1107 s on a workstation (§ *Releases*).

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
through `0.27.0`, so
a green gate *is* evidence that all twenty-seven still hold. Cutting a successor bundle therefore means
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
`.github/workflows/gate.yml` runs the same `bash scripts/gate.sh` on push and pull request, split
into its two phases as parallel jobs: `bash scripts/gate.sh tests` (workspace tests and Clippy) and
`bash scripts/gate.sh static` (formatting, the `cargo xtask` checks and the Python bundle
checkers). Every step belongs to exactly one phase, and no argument runs them all in order. The
required `Full gate` context is a third job that needs both and succeeds only when both did, or
when the run is a planning-only pull request (next paragraph). The
toolchain is pinned by `rust-toolchain.toml`, not by whatever `stable` is that day: bumping it is
**one commit** that moves `rust-toolchain.toml`, the `rust-version` in `Cargo.toml` and the
`Dockerfile` builder tag together, and `cargo xtask check-toolchain` fails the gate when the three
disagree. Read the gate's own exit status, never a pipeline's (`gate.sh 2>&1 | tail` reports
`tail`'s status, not the gate's).

**A change that adds or moves a file under `crates/substrate-daemon/src/` runs
`cargo test -p b10x-substrate-daemon --release --locked --test driver_port` before it is pushed.**
That target reads every source file of the crate and fails when one outside `src/runtime.rs` and
`src/app/tests.rs` names a `substrate_host` item (invariant 4). The crate's library tests do not
run it, so a run of the touched tests alone stays green: a new test module under `src/app/tests/`
named the host driver, passed its own 8 cases, and failed the forge's gate on 2026-11-23. A test
module takes its driver from `src/app/tests.rs` (`host_app`, `minimum_host_driver`).

**A change to `crates/substrate-wire/src/lib.rs` runs the library tests of the crates that read it
before it is pushed: `cargo test --release --locked --lib -p b10x-substrate-wire
-p b10x-substrate-daemon -p b10x-substrate-sdk`.** The daemon's
`app::health::tests::the_refusal_codes_are_the_wire_constants` reads the wire crate's source and
counts every `pub const NAME: &str` whose value is made of lower-case letters, digits, `.` and `-`
and holds a `.` as a refusal code. A new constant of that form that is no refusal code is named in
that test's `not_codes`. On 2026-11-23 `WORKSPACE_ROOT_DIRECTORY` (`"."`) passed the wire crate's
tests and the daemon's integration targets and failed the forge's gate in that library test.

**A planning-only pull request passes `Full gate` without a build or a test.**
[`.github/planning-only-paths`](.github/planning-only-paths) is the one path list: everything under
`.engineering/`, the Markdown under `adr/`, `architecture/` and `docs/`, and `ROADMAP.md`,
`SECURITY.md` and `glossary.md`, except three documents a test reads by name (ADR 0047
and designs 04 and 08). Every other path is code, Markdown included: `website/` is the built site,
`CHANGELOG.md` is compared with its release page, `STATUS.md` holds the release lines
`cargo xtask check-docs` compares with the workspace version, `contracts/` and `xtask/bundle-source/` are
released bundle bytes and their sources, `README.md`, `AGENTS.md` and the crate READMEs are read by
tests or compiled as doctests, and `spec/`, `deploy/` and `vendor/` are sources. The first job of
`gate.yml` and of `macos.yml` calls
[`.github/workflows/classify-change.yml`](.github/workflows/classify-change.yml), which builds
nothing: it reads the pull request's changed paths between the merge base and the head, both sides
of every rename, against the list as the base commit holds it, prints each path with its class and
answers `planning-only` or `code`. No changed path, one path outside the list, a commit the
checkout does not hold, a shallow history, a list it cannot read and a job that fails are all
`code`. For `planning-only`, `gate-tests`, `gate-static`, `pod-lane` and the macOS job are skipped
and `Full gate` succeeds, saying that no build and no test ran. For `code` nothing changes: a
skipped, failed or cancelled phase fails `Full gate`.
A push to `main` and a `workflow_dispatch` always run the full gate, so every commit on `main`
keeps the gate record `release.yml` reads. Accepted hazard: a planning-only pull request runs no
prose check (`check-links`, `check-adrs`), so a broken link or a wrong ADR index row in a planning
file, a design document or an ADR is first caught by the push run on `main`. It runs no
`cargo xtask check-secrets` either; the shared `common / Security and privacy` context is not
part of this rule and still runs on every pull request.
`xtask/tests/planning_only_classification.rs` executes both steps offline against throwaway
repositories, and fails when Rust source names a listed Markdown file.

The monorepo-era cross-component suite (`scripts/check-local.sh` at the predecessor's root) no longer
applies here.

## Releases

Maintain `CHANGELOG.md` in Keep a Changelog form and cut it under a version heading at release. The
tag is the bare version — `0.2.0`, the version and nothing else (atlas § *Naming*) — annotated, at a
fully gated `main` commit. The full gate comes first; component steps alone are not enough.

A wave that ends in a release carries its release-prepare commit on the wave's integration branch,
before the full gate: every workspace version and exact internal edge moved, `Cargo.lock`
(`cargo update --workspace --offline`), `THIRD_PARTY_LICENSES.html` (cargo-about 0.9.1), the
`CHANGELOG.md` version section, `website/docs/releases.md` (`cargo xtask docs-releases`) and the
site lines `cargo xtask check-docs` then prints as stale (§ *Public website*).
`release.yml` refuses a tag whose version is not the workspace version, and the four lane records
name the tagged commit, so the lanes can run only once that commit is on `main`; a separate
release-prepare pull request costs a second CI round and a second lane slot.

**No Substrate crate is published to crates.io.** Every workspace package sets `publish = false`;
Rust consumers use a local path or an exact Git revision. `cargo xtask check-packages` fails closed
if any member becomes publishable and retains the runtime source boundary: fixed package names,
exact internal versions, inherited SPDX metadata, checked-in READMEs and public documentation
targets. Tagged releases publish GitHub and GHCR artifacts only. Reintroducing a registry is a new
distribution decision, not a release convenience (ADR 0030).

**`b10x-substrate-host` has exactly two features, `git` and `kubernetes`, both default**
([its README](crates/substrate-host/README.md)). A consumer that needs only
`substrate_host::confinement` sets `default-features = false` and links no Git stack, no
`openssl-sys` and no Kubernetes client. Without `git` no `workspace.git` fact is published and a Git
source is refused `workspace.git-source-unserved`; without `kubernetes` every open of `pod` is
`confinement.adapter-unserved` naming the feature. The public items are the same in every build, so
the features are additive. The daemon and `b10x-substrate-kubernetes` take the defaults;
`b10x-substrate-cli` takes neither and must keep it that way: the host crate dev-depends on the
CLI, so a CLI edge with the defaults turns both features back on in every host test build. The
gate's no-default lane sets `SUBSTRATE_HOST_NO_DEFAULT_FEATURES`, so `host_features` then fails
instead of running on the defaults. `cargo xtask check-packages` refuses any other host feature
and any default but both, and reads the host's normal tree without default features for every
target: a dependency that brings gix, git2, libgit2-sys, curl, curl-sys, openssl-sys or the
Kubernetes client back into that tree is refused by name.

**Pushing that tag runs [`.github/workflows/release.yml`](.github/workflows/release.yml)**, which
builds the distinct `daemon` and `mcp` runtime targets from one shared pinned builder in `Dockerfile`
and packages the explicitly pinned current development bundle with
`cargo xtask package-bundle`. It publishes
`ghcr.io/beyond10x/b10x-substrate-daemon:<version>`,
`ghcr.io/beyond10x/b10x-substrate-mcp:<version>` and
`ghcr.io/beyond10x/b10x-substrate-executor:<version>`, plus
`ghcr.io/beyond10x/b10x-substrate-wire:<bundle-version>`; keyless-signs all four digests and verifies
all four signatures **before** it announces anything. The executor image is the `executor` target,
the static musl `substrate-executor` on an empty base built from the same pinned `rust` image
(design 34 § 10.4); it is pushed only after its `version` handshake names `substrate-executor/1`,
the tag's version and the tagged commit, and its executable has no interpreter. A recovery of a tag
whose Dockerfile has no `executor` stage publishes none and says so; a tag push without it is
refused. It also exports `substrate-daemon` and the
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
which `gate.yml` has not concluded `success`, an existing daemon, MCP or executor version tag on an initial
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

**A release also publishes the Kubernetes chart, and requires a green chart lane for the tagged
commit** (ADR 0046, K7 and K9). The preflight waits for `.github/workflows/chart-lane.yml`'s own
recorded `success` for that exact commit, as it does for `gate.yml`, and refuses a tag without one.
Both steps count only a `push` run on `main` or a `workflow_dispatch` run of this repository: a
pull-request run tested a merge, not the tagged tree. The scenarios the chart lane reports absent
(always the four execution-enabled ones it cannot run on a hosted kind node) are read from that
run's annotations and printed under the chart lane in the release notes and both job summaries; the
local record below is what proves those four.
That workflow runs on every push to `main`, so every taggable commit has a run; on pull requests it
runs only when the chart, its xtask modules, the daemon and the crates it links, the toolchain, the
`Dockerfile` or a release workflow changes. An existing chart tag refuses a tag push before any
artifact is published. After the daemon image is pushed, `cargo xtask package-chart` packages the
chart with that digest as its default image, byte for byte reproducibly, before the registry login.
The release job pushes it once to
`oci://ghcr.io/beyond10x/charts/b10x-substrate:<version>` (write-once; a recovery dispatch reuses it
only when its chart layer is byte for byte the local package), keyless-signs and `cosign verify`-es it, attests its
build provenance with `actions/attest-build-provenance` and checks that with `gh attestation
verify`, then reads it back anonymously, all before announcing. `attestations: write` is granted to
the release job only. A recovery dispatch of a tag whose source has no chart publishes none and says
so; a tag push without a chart is refused. A first push can create the chart package private, so
the anonymous read-back fails before anything is announced; make the package public and dispatch a
recovery.

**Two of those images ship for adopters outside this collection, and that is deliberate.** No
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
cgroup v2 subtree, which `gate.yml` does not set up, so that lane is reported **absent — not
executed, not passed** (§ *The gate*). `release-lanes.yml` runs it on a hosted runner and records
it (*The hosted lane records* below). Without that run, run `bash scripts/delegated-lane.sh` at the
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
GitHub release under *Confinement lane*, with its carrier, and into both job summaries. A
protected-`main` recovery dispatch may pass the same value as the workflow's `confinement_evidence`
input instead, which is the only way to release a tag that was annotated before this condition
existed and has no hosted run. `release.yml` itself never runs the lane, and its release notes name
the carrier, so an operator's record is never described as if CI had run it.

**A release also requires a recorded project-quota-lane run for the tagged commit**, in the same
shape. `release-lanes.yml` runs it on a hosted runner; without that run, run
`cargo xtask quota-fixture run` at that commit with a clean checkout (§ *The real project-quota
lane*) and put the line it prints in the annotated tag's message:

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

**A release that publishes the chart also requires a recorded local chart-lane run for the tagged
commit** (ADR 0046, K9 C), in the same shape. `chart-lane.yml` reports the four
execution-enabled scenarios absent, because its kind nodes load no Localhost seccomp or AppArmor
profile; `release-lanes.yml` runs `--node-profiles` on a hosted runner, whose kernel enables
AppArmor. Without that run, build the daemon image from that commit with
`--build-arg SOURCE_SHA=<sha>` (the lane refuses an image whose `org.opencontainers.image.revision`
is not `HEAD`), run `cargo xtask chart-lane cluster --node-profiles --image <ref> --node-image <ref>`
with a clean checkout on a host whose kernel enables AppArmor, and put the line it prints in the
annotated tag's message:

```
Chart-lane: <source-sha> <scenario-count> <RFC 3339 UTC>
```

The verb builds its kind node image from the committed
[`fixtures/chart-lab/Dockerfile`](fixtures/chart-lab/Dockerfile): the hosted lane's
`kindest/node` digest, which `--node-image` must name, plus the pinned `apparmor` package from the
node's own Debian snapshot, its boot unit masked so the node never loads its distribution's
profiles. The image is tagged `b10x-substrate-chart-lab:<key>` per checkout and never pushed. The
cluster has one worker, because every kind node shares the host kernel and a second node's
installer would find the first one's profile loaded without its own source. The worker gets the
host's securityfs. Each run names its own AppArmor profile `substrate-host-exec-v2-lane-<ulid>`,
a fresh lowercased ULID
([design 33](docs/design/33-host-profile-name-parameter.md) § 7),
so it never loads, selects or removes a profile it does not own; a `substrate-host-exec-v2` another
workload holds in the shared kernel does not stop it. The chart is installed with one pod,
`execution.enabled`, that name as `execution.appArmorProfile` and `nodeProfiles.enabled` in
`privileged` namespaces, and the verb runs every scenario in the literal
`SCENARIOS` array of [`xtask/src/chart_cluster.rs`](xtask/src/chart_cluster.rs): exactly the four
the hosted lane reports absent, namely the bootstrap's drop to uid 65532 with a confined exec
through it, the Localhost seccomp profile, the run's own enforced AppArmor label,
and the 0400 key served after the drop. The six hosted scenarios stay the hosted lane's. A missing
prerequisite is a refusal that names it and every scenario that did not run, and prints no record;
the run's own name already loaded in the kernel is one, because the installer refuses a loaded
profile whose source its fresh node does not hold and the run did not load it. After the scenarios
the verb uninstalls the release, waits until none of its pods remain, and removes its own profile,
by name, with `apparmor_parser --remove` on the node, which holds the source; a failed removal is
no record and names the profile left loaded. A run that dies before teardown leaves only its own
uniquely named profile loaded, which blocks no later run.
`release.yml` refuses to publish without the line, refuses a record naming any other commit,
refuses a count other than the number of entries in that array at the tagged commit (one quoted
name per line, matching its declared length), and applies the same calendar, one-line and
three-field rules; the line is written into the GitHub release under *Chart lane* and into both job
summaries, and a recovery dispatch may pass it as the `chart_evidence` input instead. A tagged
source without `deploy/helm/substrate` needs no record. The step's refusals are executed offline by
`xtask/tests/release_workflow.rs`.

**A release also requires a recorded namespace-lane run for the tagged commit**
(`decision-blocker:namespace-lane-runs-on-a-workstation`, D182), in the same shape:
`namespace-lane.yml` runs only the static checks, and `release-lanes.yml` runs the kind
observations on a hosted runner. Without that run, run `cargo xtask namespace-lane observe` at that
commit with a clean checkout and put the line it prints in the annotated tag's message:

```
Namespace-lane: <source-sha> <observation-count> <RFC 3339 UTC>
```

`release.yml` refuses to publish without it, refuses a record naming any other commit, refuses a
count other than the number of entries in the literal `OBSERVATIONS` array of
[`xtask/src/namespace_lane.rs`](xtask/src/namespace_lane.rs) at the tagged commit (the length the
verb records; the declared length must equal the entries), and applies the same calendar, one-line
and three-field rules; another lane's line does not satisfy it. The line is written into the GitHub
release under *Namespace lane* and into both job summaries, and a recovery dispatch may pass it as
the `namespace_evidence` input instead. A tag push whose source has no `xtask/src/namespace_lane.rs`
is refused; only a recovery of an older tag needs no record, and its notes say so. The preflight
also waits for `.github/workflows/namespace-lane.yml`'s own recorded `success` for that exact
commit, as it does for `gate.yml` and counting the same runs, and refuses a tag without one; a tag
push whose source has no `namespace-lane.yml` is refused, and only a recovery of such a tag needs
no run, which its notes say. Both steps' refusals are executed offline by
`xtask/tests/release_workflow.rs`.

**The hosted lane records.** `.github/workflows/release-lanes.yml` runs the four lanes, each in its
own job on a GitHub-hosted `ubuntu-24.04` runner, on every push to `main`, on `workflow_dispatch`,
and on pull requests that change what they build. Each job runs the lane's own command unchanged
after root setup steps give the VM what a workstation has (the workflow's header says which and
why: bubblewrap, socat, an AppArmor `userns` grant for `/usr/bin/bwrap` only and a lingering user
manager for the confinement lane; `quota_v2` for the quota lane), and records the lane's line as a
notice annotation titled with its trailer. The preflight step *Read the hosted lane records for this
commit* waits for that workflow's own recorded `success` for the exact tagged commit, counting only a
`push` to `main` or a `workflow_dispatch` of this repository, and hands each annotation to its
record step, which applies every refusal above to it. Per lane, a recovery input comes first, then
the tag's trailer, then the hosted annotation; when the tag or the dispatch records all four, no run
is read, and a tagged source without the workflow has only those carriers. The step reads the jobs
of every attempt of that run and takes each lane's record from the newest attempt in which a job
that concluded `success` annotated it; an annotation of a job that did not succeed is not a record.
A failed hosted run, or two annotations of one lane in one attempt, is refused. No self-hosted
runner is involved (Timo, 2026-11-19).

**A failed lane job of `release-lanes.yml` may be run again alone.** Release 0.26.0, 2026-11-23:
the confinement lane failed once at `d0bd8be`, its job alone was run again, and the tag-push
release run was refused "no recorded quota lane run", because the preflight of that tag's source
read the latest attempt only and a carried-over job has a new check run without its annotation.
The release was published by a recovery dispatch with the three records of the first attempt as
inputs. From the release after 0.26.0 the preflight reads every attempt (above). A tag whose
source is 0.26.0 or older still needs a whole new run (`workflow_dispatch` on `main`, or re-run all
jobs). The chart-lane step reads the `chart-lane absent` notices of `chart-lane.yml`'s latest
attempt in the old way: after a partial rerun there the release notes print fewer absent
scenarios than ran absent, and nothing is refused.

`packages: write` and `id-token: write` exist on the release job and nowhere else; that job holds
`contents: write` only to create the GitHub release. Everything it does uses the run's own
`GITHUB_TOKEN`; **the release needs no repository secret at all** (§ *Bot identity*). The GitHub
release records all artifact digests and the exact `cosign verify` and `cosign verify-blob` commands. **`0.16.1` is the published
release** (2026-10-29, tagged commit `f2fd606`): the daemon, disposable MCP and executor images at
`0.16.1`, the development bundle `ghcr.io/beyond10x/b10x-substrate-wire:0.25.0`, the chart
`oci://ghcr.io/beyond10x/charts/b10x-substrate` `0.16.1` and the two binary archives under a signed
`SHA256SUMS`. It is the first release with the executor image, and its annotated tag carries all
four records `release.yml` requires: `Confinement-lane:`, `Quota-lane:`, `Chart-lane:` and
`Namespace-lane:`. Tag `0.16.0` was never published, because `main`'s gate on its commit failed
`cargo xtask check-advisories` and release tags are write-once. The previous release, `0.15.0`, was
published by a protected-`main` recovery dispatch under the workflow identity
`release.yml@refs/heads/main` and predates the executor image.
Do not maintain a second digest list here: the digests of every release are the GitHub release's own
([0.16.1](https://github.com/beyond10x/substrate/releases/tag/0.16.1)) and
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

The public site is repository-owned at `https://beyond10x.github.io/substrate/`. `website/` is a
Docusaurus site on the shared Docs System, pinned by commit in `website/package.json`; the site
build is the only Node here. `npm --prefix website ci && npm --prefix website run build` writes
`website/build`. The start page is the static `website/static/index.html`, copied as it is.
`cargo xtask check-docs` validates the explicit public page allowlist without Node: every page under
`website/docs/` is in `PAGES` and in `website/sidebars.ts` in the same order and grouping, and the
start page's links resolve. `cargo xtask docs-provenance --site website/build --commit <full-sha>`
checks the built site and writes `.well-known/b10x-site.json` provenance and
`.well-known/b10x-routes.json`, every published route with its rendered anchor IDs at the same
commit. The organization Website reads that inventory, as it does Metaharness's, to redirect the
former `/docs/substrate/` pages; keep page paths and heading IDs stable. `pages.yml` builds, binds
and uploads the exact successful main artifact; `b10x-docs-site.yml` uses the pinned standalone
project-site workflow. Do not add unified Website bundles, `b10x.docs.yaml` route manifests,
redirect façades, or global docs reconciliation. Publish no internal plans, ADRs or work logs.

`check-docs` also holds what the site derives. The adapter matrices, the family, fact and lane tables
and the CLI Commands table must match `spec/confinement/`, `spec/confinement-suite.json` and
`spec/generated/cli/binding.json` (`xtask/src/site_drift.rs`). The *Release notes* page ends with
the newest released `CHANGELOG.md` section, generated by `cargo xtask docs-releases`; `check-docs`
fails when the two differ.

`check-docs` holds what the pages and `STATUS.md` call current to its source
(`xtask/src/site_release.rs`, UXF-009). `STATUS.md` holds four of the forms: its release line, the
bundle that release advertises, the latest annotated tag and the latest release of its *Source* row.
A page states the current release, its date, the advertised bundle or the newest bundle directory
only in a sentence form of `FORMS`. The values are `[workspace.package] version`, that version's
`CHANGELOG.md` date, `ADVERTISED_CONTRACT_BUNDLE` and the newest directory of
`contracts/substrate-wire/`. A source link pinned to a tag (`tree/<v>/`, `blob/<v>/`) names the
workspace version, and the fact list of `guides/confinement-profiles` is `FACT_ORDER`. After a
version bump `check-docs` prints each stale line with the text to write there, so the
release-prepare commit carries those lines. A sentence about an earlier release is history: write
it in another form ("Release `0.16.1` was the first to advertise …").

`check-docs` also holds three rules of the site's own styles (`xtask/src/site_style.rs`):
`website/src/css/custom.css` underlines the links of article text (UXF-008) and sets code blocks
and table headers to 14 px or more (UXF-015). The start page's one script is `START_THEME_SCRIPT`. It
reads the theme the docs pages store in `localStorage` under `theme-07d`: `future.v4` namespaces
the key with a hash of `url` and `baseUrl`, and `docs-provenance` fails when the built docs pages
read another key. The script stands
before its stylesheet, and `website/static/styles.css` sets every colour in three rules: `:root`
(dark), `:root[data-theme=light]` and the same declarations under `prefers-color-scheme: light`
(UXF-013). A colour literal in any other rule fails, and so does text under 4.5:1 on its surface.

A change under `xtask/src/` runs every test target of the `xtask` package but three before it is
pushed:

```console
SLOT_TIMEOUT=30m cargo nextest run -p xtask --release --locked -E 'not (binary(executor_tree_rule_adversary_u4_pass1) | binary(host_no_default_graph_security_review_u1) | binary(kubernetes_client_fake_fence_adversary))'
```

The three left out start `cargo` and ran into the 60 s bound in the shared pool on 2026-11-23. The
run is in the release profile, as the gate's is: in the debug profile the bundle targets reach
the 60 s bound, and that day the run was cancelled at 120 s with 81 of 712 tests run. The
`--bin xtask` tests alone are not enough. `docs_site_adversary` runs `check-docs` and
`docs-provenance` on copied inputs and a hand-built site: a rule that reads a new file adds it to
`input_root` there. `planning_only_classification` refuses a Markdown file that Rust source names
while `.github/planning-only-paths` lists it: a rule that reads one takes it off that list. PR 264
went red on the forge three times, each time for a target the local run had skipped.

Site changes follow the UI iteration loop in `website/ux/`
(`xtask/src/site_ux.rs`):

- `baseline.json` holds lower-is-better ratchets: the reading load per page, which `check-docs`
  holds, and the clicks per task in `tasks.json`, which `docs-provenance` holds on the built site.
  A value that improves is written back with `cargo xtask docs-baseline --site website/build` in
  the same change. A worse value needs `--decision <id>`.
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

**A fix for a defect of a released version states the rule, everywhere that is pushed.** This
repository is public. Until the fix is on `main`, a commit message, a pull request, a story, a
changelog entry, a test name and a code comment say what holds after the change and how it was
proved: the rule, the cases, their counts. They do not say what the released version does, which
input shows it, or that a case "was red before" with the input named. The detail belongs to the
operator's private record until the release that carries the fix, whose changelog *Security*
entry names it (`approval-record:operator-decides-how-security-review-fixes-travel-2026-11-22`).
A pushed commit message cannot be taken back without a force-push, which no session does: read
the message for this before the commit. It was broken twice, on 2026-11-22 and 2026-11-23, each
time by a commit body whose sentence began with what the code did before; the second branch was
closed and replaced on the operator's decision.

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

**On the operator's workstation, bot writes go through `b10x-gates`, not through these scripts,
until `story:bot-token-mints-without-cargo` is done.** `scripts/bot-token.sh` checks its two
credential files with `cargo xtask check-bot-files`, and since 2026-11-24 the workstation's
build pool sends `cargo xtask` to a build node, where those files are not: the mint fails. A
commit, a merge and a push are `b10x-gates bot -- <git arguments>`, a `gh` write is
`b10x-gates --repository beyond10x/substrate --policy <policy file> gh -- <gh arguments>`, and a
pull-request merge is `b10x-gates … api --method PUT --path /repos/beyond10x/substrate/pulls/<n>/merge`.
Where this file writes `scripts/as-bot.sh` or `scripts/bot-gh.sh` below, read that form.
Both scripts end non-zero and run no program where the mint fails or prints no token
(`xtask/tests/bot_scripts.rs`): `gh` without a `GH_TOKEN` would act under the login of the
caller's keyring, which no session may publish under.

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

**Bot writes from the operator's workstation go through `b10x-gates`.** The rule has no end date:
it is the owner's standing rule for every beyond10x repository (the operator's session,
2026-11-24), whether or not `scripts/as-bot.sh` and `scripts/bot-gh.sh` can mint there. A commit,
a merge and a push are `b10x-gates bot -- <git arguments>`, a `gh` write is
`b10x-gates --repository beyond10x/substrate --policy <policy file> gh -- <gh arguments>`, and a
pull-request merge is `b10x-gates … api --method PUT --path /repos/beyond10x/substrate/pulls/<n>/merge`.
Where this file writes one of the two scripts for a write from that workstation, read that form.
A refused `b10x-gates bot -- push` waits and is tried again, at most once in 10 minutes, and its
refusal line goes to the operator's session; it is not replaced by API writes that make the same
result another way (`git/refs`, `git/trees`, `git/commits`, `git/tags`, `contents/…`,
`pulls/<n>/update-branch`). Merging a pull request through the API after green checks, and
reads, are not such a replacement.

**A pull request's branch must hold `main` before it merges.** A third ruleset on `main`
(id 23511358, read on 2026-11-23 with `gh api repos/beyond10x/substrate/rules/branches/main`)
requires the check `common / Security and privacy` with the strict policy: a branch behind `main`
is `BEHIND`, and the merge call answers 405 "Required status check … is expected" although every
check of the head is green. So every merge to `main` puts each other open pull request one forge
round back. Merge `main` into the branch as the bot (`scripts/as-bot.sh merge --no-ff origin/main`)
and merge after that head's checks. Two branches that both add entries under `## [Unreleased]` of
`CHANGELOG.md` conflict there; keep both sides.

**Never bring a branch up to date with `PUT …/pulls/<n>/update-branch`.** The forge commits that
merge as GitHub, not as the bot, and `b10x-gates` then refuses every bot push of a branch that
holds the commit outside `main`: "merge is not on the authenticated published default branch"
(2026-11-23: `2754b62a7` on `story/replay-grant` stopped the push of release 0.25.0's
integration branch, which was built again from the bot's own head before it).

**Several finished pull requests reach `main` through one integration branch**, `release/<version>`
when a release follows: each branch merged by the bot at its own head, the release-prepare commit
on top, one pull request and one forge round. The runs of the replaced pull requests are cancelled
when the integration branch is pushed (2026-11-23: 15 queued runs, with the organization's hosted
queue at 45).

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

**The stakeholder of a request decides on the operator's behalf** (operator decision, 2026-11-21,
`approval-record:stakeholder-decides-for-the-operator-2026-11-21`). A decision an agent would put
to the operator (accepting an epic or a story, its order, a design answer, a default) goes to the
stakeholder of that work instead: the session or person that submitted the request the artifact
came from. An artifact names its stakeholder under *Where the requirement comes from*, and the
answer is recorded as an `approval-record` that quotes it. A stakeholder decides for the work it
requested, not for another stakeholder's. Merges, pushes to `main`, releases, tags and version
numbers stay the operator's, or the session `host-operator`'s acting for him (next paragraph).

**The session `host-operator` decides for the operator** (operator decision, 2026-11-21,
`approval-record:host-operator-decides-for-the-operator-2026-11-21`). On the operator's
workstation, an order, decision or approval of the session named `host-operator` counts as the
operator's own: merges, pushes to `main`, releases, tags, version numbers, other publications and
the choice between options an agent put to the operator. It counts whether it arrives as a
cross-session message or typed into the session's prompt, and no session asks the operator to
repeat it. It lifts no rule of this file: a release still goes through § *Releases*, and a change
reaches `main` through a pull request with a green `Full gate`. A session of that name anywhere
but the operator's workstation has no such authority.

**Two kinds of pull request are merged without a question.** The session that works this
repository merges, as the bot and on green checks, a pull request whose changed paths are all
planning-only (§ *The gate*), and a pull request that only moves a pin when no contract moves:
the ESS pin with its `gate.yml` version and digest and the generated headers, or the commit
`shared-gates.yml` calls. It reports the merge afterwards to the operator's session. Every other
pull request goes to that session with a recommendation, lettered options and a default. Both
are standing decisions of the operator's session acting for the operator; the second is of
2026-11-24: "These pin bumps are routine from now on: when the checks are green and no contract
moves, merge them yourself and report afterwards."

A story that changes a contract or a capability still owes its ADR or design document **before
code** (invariant 8); the story body names which. `ROADMAP.md` keeps the phase order and
`STATUS.md` the observed state; the store holds the work items and their status, and nothing else
restates it.

## Standalone documentation authority

Operator decision, 2026-10-02: Substrate owns its project site, built on the shared Docs System and published as a static project-site artifact. This supersedes the former unified docs manifest, source-bundle and redirect-façade instructions for this repository. Do not reconcile this repository back into the global documentation pipeline. App credentials never belong in this public repository.

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
