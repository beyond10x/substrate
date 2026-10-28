# Design 31: a clean-room runner certifies a remote endpoint

Status: proposed, 2026-10-28. Owned by `story:remote-clean-room-conformance`
(`epic:remote-serving`). It changes no wire byte and cuts no successor bundle (§ 8). No code before
acceptance (invariant 8). § 11 lists the questions left to the operator, each with a recommended
answer.

The story asks for a Rust runner that certifies a deployed endpoint from outside its process,
container and checkout, using only the public contract and a scoped test authority. This note
decides D1 to D8: where the runner lives, how it selects cases, what it can prove about lost
responses and restarts, how it receives credentials, what it reports and who signs it, how this
repository exercises it, its wire impact and what it leaves out.

## 1. What exists

| piece | what it gives the runner | why it is not the runner |
|---|---|---|
| `crates/substrate-daemon/tests/runtime_vectors.rs` | the clean-room shape: asserts only on the wire, pins the advertised contract independently (`ADVERTISED_CONTRACT`), pins case totals (`PORTABLE_CASES`, `DELEGATED_CASES`), absent cases are neither run nor counted | a test, not a binary; Unix socket only; it owns the daemon it checks (spawns, signals, reads its ledger rows), and its route probes are a hand-written list |
| SDK remote transport ([ADR 0029](../../adr/0029-the-remote-sdk-shares-one-verified-https-wss-transport.md), designs 20 and 23) | TLS 1.3 with explicit roots and server identity, `AccessTokenProvider`, contract-header checks, every route, WSS attachment authority | it recovers a lost mutation by itself (`recover_mutation`, `crates/b10x-substrate-sdk/src/lib.rs`) and refreshes after a 401, so it hides the server side of exactly the cases the story names |
| the bundle's runner protocol (`contracts/substrate-wire/0.24.0/runner.json`, `schemas/runner-result.json`) | the exit-code vocabulary (0 pass, 1 conformance failure, 2 invalid input, 3 harness failure) | per vector, with runner-owned `setup` (crash points, retention windows) a remote endpoint cannot accept; `pass` and `fail` only, no `absent`; executed in-process by `crates/substrate-daemon/tests/contract_vectors.rs` |
| the operation registry (`contracts/substrate-wire/0.24.0/operations.json`, source `xtask/bundle-source/0.24.0/routes.json`) | each operation's `capability_predicates` | its `required_scope` is the Unix listener's vocabulary (`session`, `process`), not what hosted admission enforces |
| [ADR 0026](../../adr/0026-hosted-admission-resolves-opaque-identity-authority.md), `hosted-admission.json` | the credential format `identity_access_v1_<43 base64url>`, the three hosted scopes and the `route_families` each admits (`exec` admits `execs` and `sessions`), the four `auth.*` refusals | — |
| `refusals.json`, `vectors/http/` | named refusals for the session family; unserved responses per vector (for example `sandbox-unavailable.json`) | no route-to-unserved-refusal map: each vector's answer is bound to its own runner-owned machine setup (§ 3) |
| [ADR 0045](../../adr/0045-operational-health-and-metrics-use-a-separate-listener.md) | `/livez`, `/readyz`, `/metrics` on a separate plaintext listener | — |
| [ADR 0046](../../adr/0046-kubernetes-serves-one-node-bound-daemon-per-stable-pod-address.md) | per-pod addressing (K1 A), no SAN for the headless Service (K2), the chart lane (K9 B) | the chart, `chart-lane.yml` and `cargo xtask chart-lane` are on PR 182, not on `main` (`ba75c00`) |
| `.github/workflows/release.yml` | keyless `cosign sign-blob --bundle` over `SHA256SUMS`, checked by `cosign verify-blob` | `id-token: write` exists on the release job and nowhere else (AGENTS.md § *Releases*) |

## 2. D1: where the runner lives

**Choice: A.** A new workspace crate `crates/b10x-substrate-conformance`, package and binary
`b10x-substrate-conformance`, `publish = false` (ADR 0030). Its only workspace dependencies are
`b10x-substrate-wire` and `b10x-substrate-sdk`, the SDK edge without features. Every third-party
dependency is already in `Cargo.lock`:

- the workspace dependencies `clap`, `rustls`, `tokio`, `tokio-rustls`, `x509-parser`, `sha2`,
  `zeroize` and `serde_json`;
- `nix`, a workspace dependency, with two features added: `process` for `prctl` (the SDK already
  enables it) and `resource` for `setrlimit` (no crate enables it yet). That is a feature of a
  locked crate, not a new package;
- for tests only, `jsonschema` (workspace) and `rcgen`. `rcgen` is not a workspace dependency. The
  runner declares it per crate, exactly as the SDK does (`version = "0.14"`,
  `default-features = false`, `features = ["aws_lc_rs", "pem"]`).

The SDK drives the positive journey: workspaces, files, execs, sessions over WSS, events,
metrics and operation reads. A small **raw probe** module, built on `rustls` directly and never
retrying, covers what the SDK deliberately cannot do: read the raw `/v1/machine` response, send a
request without a credential or with a forged one, speak plaintext or TLS 1.2, offer only `h2`,
verify against a name the certificate must not carry, and drop or discard a mutation's response.
Every assertion is made on wire values. Neither half reads SDK internals.

| option | rejected because |
|---|---|
| **B** a subcommand of the `b10x-substrate` CLI | the CLI depends on `b10x-substrate-host` (`crates/b10x-substrate-cli/Cargo.toml`), so the runner would link the host driver, which the story forbids |
| **C** an `xtask` verb | `xtask` is checkout tooling that links the bundle renderers. The story certifies from outside a checkout |
| **D** extend `runtime_vectors.rs` | a test, Unix-socket only, and it owns the daemon. A remote endpoint is neither |
| **E** the wire crate only, with its own HTTPS/WSS client | a second implementation of TLS configuration, token refresh and the session-authority transcript to keep in step with the SDK. The raw probe already covers what the SDK hides |

**The boundary is held on the resolved graph, not on the manifest.** A manifest rule is not
enough. `crates/substrate-mcp/Cargo.toml:16` and the CLI enable the SDK's `linked-daemon`, and
resolver 2 unifies features across every package one command selects. So the runner built by
`cargo test --workspace` links `b10x-substrate-daemon`, and through it the host driver and the store.
`cargo tree --workspace -e normal -i b10x-substrate-daemon` lists the SDK under the MCP adapter and
the CLI, while `cargo tree -p b10x-substrate-sdk -e normal` lists none of the three. `cargo xtask
check-packages`, which the gate runs, therefore:

- adds the crate to `SOURCE_PACKAGES` (`xtask/src/packages.rs`), refuses any other workspace path
  dependency in any of its dependency tables, and refuses a `features` entry on its SDK edge;
- runs `cargo tree --locked -p b10x-substrate-conformance -e normal,dev --prefix none`, which
  resolves features for that package alone, and refuses any line naming `b10x-substrate-daemon`,
  `b10x-substrate-host` or `b10x-substrate-store`. Reading that output is a pure function with its
  own unit cases.

"Links no daemon" is a claim about the binary built with `-p b10x-substrate-conformance`, and every
binary that certifies anything is built that way: the delegated lane's run (§ 7), the chart lane's
build (§ 7) and an operator's `cargo build --locked -p b10x-substrate-conformance`. The binary that
gate step 1 builds under `--workspace` is feature-unified. Its run is evidence of behaviour, never a
certification. The tests find the daemon as an executable (`SUBSTRATE_TEST_DAEMON`, or the sibling
of the test executable, as `daemon_binary()` in `crates/b10x-substrate-sdk/tests/remote.rs`
does), so no dev-dependency links it.

## 3. D2: case selection and absence

**Choice: A, selection derived from the contract.** One run:

1. **Handshake.** The raw probe sends `GET /v1/machine` and decodes the raw body into the wire
   crate's closed envelope and `CapabilitySnapshot`, so the runner judges the bytes the server sent
   rather than the SDK's view of them. The `x-b10x-contract*` pair must equal a pair the runner pins
   itself, independently of `substrate_wire::ADVERTISED_CONTRACT_BUNDLE`, as `runtime_vectors.rs`
   does. The first pin is `substrate-wire/0.24.0`. An unknown pair stops the run with exit 2. The
   report names the advertised pair, and no case runs on a guess.
2. **Requirements.** Each case declares the registry operations it calls and any fact its
   assertion needs beyond them (`sessions.pty`, `exec.resource-usage`). The predicates come from
   the pinned bundle's `operations.json`. The scope comes from its `hosted-admission.json`. An
   operation's route family is the first path segment after the API major (`/v1/ops/…` is
   `operations`), and its scope is the one whose `route_families` lists that family. Sessions are
   therefore `exec`, as the daemon enforces (`required_scope`,
   `crates/substrate-daemon/src/hosted.rs:488`, and its test at `:646`). The runner never reads
   `operations.json` `required_scope`. Both documents are embedded at build time from the frozen
   directory and are never restated in runner code. A unit test maps every operation of the
   pinned registry: each lands in exactly one listed family or in none, and every listed family is
   reached.
3. **Run or absent.** A case runs when every predicate holds in the snapshot, its scope is in
   `--scopes` (default: all three hosted scopes), the inputs it needs are declared (a restart hook,
   an ops origin, a forbidden name), and its runner bound holds. Otherwise it is `absent` with one
   reason from a closed set: `fact-absent`, `fact-unsatisfied`, `scope-not-declared`,
   `input-not-declared` or `bound-exceeded`, plus the facts or inputs concerned. Selection reads
   `facts` only, never `driver` or `driver_version` (invariant 4). Those two are recorded in the
   report and used nowhere else.
4. **Absence is still checked, without a route-to-refusal map.** The bundle publishes no unserved
   refusal per route (§ 1), so the runner asserts what it does publish for any such answer. Take
   each operation in a hosted family whose fact-only predicates (those without `input_pointer`)
   fail in the snapshot and for which the inventory holds a request (the one its positive case
   would send). A `refusal-when-unserved` case sends that request once and asserts an error
   envelope under `schemas/error.json` with `class: "unserved"`, the contract headers and no
   resource. It records the observed status and code without judging them. Input predicates are
   satisfied by construction, because the runner sizes every request from the facts it reads.
   "Nothing ran" is claimed only as a remote client can observe it. For a mutation under operation
   id X, `GET /v1/ops/X` reads `resource.not-found` or a record in state `refused` with no
   resource, and a replay under X returns the identical error. Where `events.pull` holds, the
   events after a cursor taken just before the request name no resource the run did not already
   hold. Whether a host process was spawned is not observable from outside. That proof stays with
   `runtime_vectors.rs` and the delegated lane.
5. **Outside the hosted families.** The eight `process.*` operations belong to no hosted family,
   and no hosted credential can carry their scope `process` (`ALLOWED_SCOPES`, `hosted.rs:49`).
   One request per operation with the full credential must be refused `auth.scope-denied` (403),
   which admission answers before any handler (`require_hosted_authority`, `hosted.rs:528`). Their
   predicates are never evaluated, and they are never `absent`.

Statuses are `passed`, `failed` and `absent`. Totals count each separately, and an absent case is
never in `passed`. **Core cases** cannot be satisfied by absence: machine and contract, the TLS and
auth negatives, the workspace and file lifecycle, operation recovery, and cleanup. The verdict is
`failed` if any case failed, `passed` if none failed and every core case passed, and otherwise
`incomplete`. Exit codes: 0 passed, 1 failed, 2 invalid input, 3 harness failure, 4 incomplete.

| option | rejected because |
|---|---|
| **B** an operator case list or `--skip` | a deployment could certify itself by omission. `--only <group>` exists for debugging, and it marks the report `partial`, whose verdict is never `passed` |
| **C** run everything and read 501 as absent | it confuses a missing capability with a deployment that advertises a fact and then refuses it, which invariants 3 and 4 forbid |
| **D** a hand-written fact list, or route-to-refusal map, per case | it drifts from the registry, which already states each route's predicates. The bundle publishes no refusal map to hold such a list to |

### Inventory

| group | cases (one scenario each) | needs | core |
|---|---|---|---|
| machine | the raw response decodes into the wire types; contract pair pinned; `valid_until` not passed; the wire `snapshot` equal at the first and last read unless the restart group ran | — | yes |
| TLS | plaintext HTTP/1.1 gets no HTTP answer; a TLS 1.2-only hello fails; an `h2`-only ALPN offer fails | — | yes |
| TLS name | verification fails for each `--forbidden-name` (the headless Service, ADR 0046 K2) | `--forbidden-name` | no |
| auth | no bearer: `auth.credential-absent` (401); a well-formed unknown credential: `auth.authority-invalid` (401); a mutation refused 401 under id Y leaves `GET /v1/ops/Y` `resource.not-found`; every `process.*` operation: `auth.scope-denied` (403) under the full credential (step 5) | — | yes |
| auth scope | a credential narrowed to `observe` gets `auth.scope-denied` (403) on a `sessions` route and on a `workspaces` route | provider serves `narrowed` (§ 5) | no |
| unserved | one `refusal-when-unserved` case per hosted operation whose fact-only predicates fail (step 4) | the failing facts | no |
| workspace | create, write, read v1 and v2, tree, replace, edit, delete, destroy, each read back; out-of-root path refused | workspace facts | yes |
| recovery | the two lost-response cases and replay conflict (§ 4) | workspace facts | yes |
| exec | argv-only start, output read, wait, signal, retire; no egress (an outbound connect fails inside the child) | exec facts | no |
| resources | `measurements: ["resource-usage"]` gives `usage` in state `observed` after exit; `/v1/metrics` names the exec | `exec.resource-usage` | no |
| sessions | raw-pipe attach over WSS with fresh authority; a second attach is refused (ADR 0041); PTY attach and `resize` | session facts; `sessions.pty` | no |
| events | own events in commit order with strictly increasing sequence; no event is visible before its state is readable (ADR 0005); stream equals pull | `events.pull`; `events.stream` | no |
| retention gap | the runner writes `events.retention-events` + 1 events in its own scope, and its first cursor then gets `event.retention-gap` | retention ≤ `--max-generated-events` (default 256) | no |
| restart | § 4 | `--restart-hook` | no |
| ops | `/livez` and `/readyz` answer 200; `/metrics` parses and carries none of the run's credential, resource ids or operation ids | `--ops-origin` | no |
| cleanup | every resource the run created is destroyed or retired and then reads as absent | — | yes |

Every resource the run creates carries a random run label, so an operator can find leftovers
after a harness crash. Cases run sequentially in a fixed order under one `--deadline` (default
300 s, at most 3600 s). Every connect, TLS handshake, read, write and WSS frame wait, on the raw
probe and on the SDK (each call wrapped in `tokio::time::timeout`), is bounded by `--io-timeout`
(default 10 s, at most 60 s). An expired bound fails its case with diagnostic `io-timeout`. An
expired deadline stops the run, marks the cases not yet run `failed` with `deadline-exceeded`, and
still writes the report.

## 4. D3: lost responses and restarts at an endpoint the runner does not control

| case | how | proves | cannot prove | hook |
|---|---|---|---|---|
| response read and discarded | the raw probe sends `PUT …/files/…` with id X, reads the response and discards it. The SDK then reads `GET /v1/ops/X` and replays the identical bytes under X | the replay returns the original outcome, the file's bytes and version are unchanged, and there is exactly one write event. The same X with a different body is a replay conflict (vector `replay-conflict.json`) | — | none |
| response dropped before read | the raw probe flushes the request and closes the TLS connection. The report records which branch the server took: not admitted (`resource.not-found`, then one replay succeeds) or admitted (the outcome as above) | no fabricated state and no second execution on either branch | which branch occurs, or where the server was when the connection closed | none |
| crash before or after dispatch | — | — | needs fault injection inside the daemon. It stays with vectors `crash-before-dispatch`, `crash-after-dispatch` and `restart-no-redispatch` in `contract_vectors.rs`, and the report lists it under `not_covered` | — |
| restart to unknown | see below | see below | that the restart was a crash rather than a clean stop: the hook decides how the process ends | `--restart-hook` |

**Restart sequence.** Create workspace W and write a file. If exec facts hold, start exec E, a
fixed program run through `--probe-shell` (default `/bin/sh`) that writes one marker into
`/workspace` and then blocks. Take an event cursor C and note the wire `probed_at` as P0. Run the
hook, then poll `/v1/machine` until `probed_at` is later than P0. The hook and the poll together
are bounded by `--restart-timeout` (default 120 s, at most 600 s), and `/readyz` is also polled when
`--ops-origin` is declared.

The poll assumes what the host driver does. It probes once, at construction (`probe::probe` in
`crates/substrate-host/src/lib.rs:1139`), so `probed_at` moves only when a new process starts, and
the wall clock does not step back across the restart. A driver that re-probed while running would
let the poll end without a restart. Where exec facts hold, E reading `unknown` corroborates the
restart. Without them, the report states that `probed_at` alone detected it. Then assert:

- E reads `unknown`, which is what the store defines. At startup `reconcile_after_restart` marks
  every exec row in `accepted` or `running` as `unknown` (`mark_exec_unknown`,
  `crates/substrate-store/src/recovery.rs`). Its operation-side rule is pinned by
  `restart_moves_only_accepted_operations_to_unknown` (`crates/substrate-store/src/tests.rs:1758`).
  `reconcile_provisional_execs` (`crates/substrate-daemon/src/app/service.rs:532`) replaces that row
  only with an observation the driver still holds. The host driver's process table is in memory,
  so after a crash the driver answers not-found and the row stays `unknown`
  (`mark_exec_physically_absent`). E's start operation was already `terminal` and stays so. A
  driver that re-observes execs across a restart would be a contract change with its own design,
  and this case would move with it.
- The marker count is one, so nothing was re-dispatched. Replaying E's start under its operation id
  returns the original outcome, and the count is still one.
- W's bytes are readable.
- C answers either a continuous page or a named refusal (`event.source-mismatch` or
  `event.retention-gap`), never a silent jump in sequence.

Without exec facts, only the workspace, replay and cursor assertions run. The exec assertions are
`absent` (`fact-absent`).

**The hook is declared, not discovered.** `--restart-hook <absolute path>` takes repeatable
`--restart-hook-arg` values and is run argv-only, never as a shell string. Its environment is
cleared except for names passed with `--hook-env`. Its stdin, stdout and stderr are `/dev/null`,
and it inherits no other descriptor: the token fd, the token command's pipes and every endpoint
connection are close-on-exec (§ 5). Only its exit status is recorded. A non-zero exit or a timeout
fails the restart group with diagnostic `hook-failed`.

| option | rejected because |
|---|---|
| an interactive pause | cannot run unattended in a lane |
| the runner calls the Kubernetes API | provisioning and RBAC in a client, which the story excludes |
| passive detection of restarts | cannot be scheduled against the in-flight state the case needs |
| a daemon restart route | a wire change, and a remote kill switch |

## 5. D4: the token provider, and proof that nothing persists

**Choice: exactly one of `--token-command` or `--token-fd`, with one line protocol for both.** The
runner writes one JSON line, `{"reason":"request"|"refresh","purpose":"primary"|"narrowed","scopes":[…]}`,
and reads one line of at most 128 bytes that matches the bundle's credential format. The deadline
is 10 s.

- `--token-command <absolute path>` with repeatable `--token-command-arg`: argv-only, exit 0. Its
  environment is cleared except names passed with `--token-env`, a list separate from
  `--hook-env`, so neither child receives a variable meant for the other. Its stdin and stdout are
  its own pipes and its stderr is `/dev/null`. Exit 3 or the line `unsupported` means the purpose is
  not served, and the scope case is then `absent` (`input-not-declared`).
- `--token-fd <n>`: an inherited Unix stream socket (checked with `fstat`), so a supervising
  process can answer many requests and tell a refresh from a request. At start the runner sets
  `FD_CLOEXEC` on it, so neither the hook nor a later child inherits it.

Both are wrapped as the SDK's `AccessTokenProvider`, and the raw probe asks the same provider.
`refresh` is sent once after a named hosted 401 (design 20). Every descriptor the runner opens is
close-on-exec (the Rust default). The token command therefore holds no descriptor but its own
stdio, and the hook holds none at all.

| option | rejected because |
|---|---|
| a credential file | a credential at rest, which the story excludes |
| an environment variable | inherited by every child including the hook, readable in `/proc/<pid>/environ`, and cannot refresh |
| a read-only pipe | cannot carry the request, and a pre-filled pipe is a credential buffer |
| the runner mints from client credentials | makes the runner an Identity client holding a long-lived secret |

**What the runner does, and how its test proves it:**

1. At start it sets `PR_SET_DUMPABLE` to 0 and `RLIMIT_CORE` to 0 (`nix`, no `unsafe`), so there is
   no core file and no same-uid ptrace or `/proc/<pid>/mem` read.
2. A credential lives in a `Zeroizing` buffer for one request, as the SDK's `AccessToken` already
   does, and never enters error text. Diagnostics are closed codes and wire refusal codes.
3. It writes only into `--out`, which must not exist and is created with mode 0700, and only
   `report.json` and `report.json.sha256`.
4. The test runs the binary with `HOME`, `TMPDIR`, each `XDG_*_HOME` and the working directory set
   to fresh empty directories, and with a token command that emits a sentinel credential.
   Afterwards those directories are still empty and `--out` holds exactly the two files. No byte of
   `--out`, stdout or stderr contains the sentinel, its 43-character body or `identity_access_v1_`.
   The test's restart hook and token command each record their environment and their open
   descriptors (`/proc/self/fd`). The hook's environment holds no sentinel and no `--token-env`
   name, and it has no descriptor beyond 0 to 2. The token command has no descriptor beyond 0 to 2,
   and in the `--token-fd` run the hook does not hold the socket.
5. The report schema is closed at every level (§ 6), so the report has no free-text field a
   credential could land in.

Not provable by the runner: what the provider does with the credential, and whether the kernel
swaps the page holding it (no `mlock`).

## 6. D5: the report, and who signs it

**Format `b10x.substrate-remote-conformance-report.v1`.** The schema is
`crates/b10x-substrate-conformance/schema/remote-conformance-report.v1.schema.json`, Draft
2020-12, with `additionalProperties: false` at every level.

**Invariant 7.** The schema is a new JSON authority and is not a bundle file. The crate's own test
classifies it: it meta-validates the schema with the pinned `jsonschema` and validates every report
the tests produce against it. This is the precedent of
`deploy/seccomp/schemas/host-execution.schema.json`, classified by `xtask/src/container_profiles.rs`.
`cargo xtask check-json` keeps its scope, which is released bundles.

| option | rejected because |
|---|---|
| **B** the schema in a successor bundle | a wire change (invariant 6) for a document the daemon neither serves nor accepts. It would tie the report's evolution to the bundle cadence |
| **C** extend `runner-result.json` | frozen, per-vector, and has no `absent` |

The report is written in the repository's deterministic form (sorted keys, two-space indent,
trailing newline; `xtask/src/json.rs`), atomically, so its digest follows from its content.

| member | content |
|---|---|
| `runner` | version, source revision |
| `run` | `started_at`, `finished_at`, run label, `partial` |
| `endpoint` | SHA-256 of the origin and of the verified server name, TLS version, ALPN, SHA-256 of the leaf certificate and of the declared trust roots, leaf `not_after` |
| `contract` | advertised name and SHA-256 |
| `capability_snapshot` | the wire `snapshot` and `probed_at` at the first and last read, `driver`, `driver_version`, `config_generation`; recorded only (§ 3, step 3) |
| `declared` | scopes; which inputs were declared (restart hook, ops origin, how many forbidden names); bounds |
| `cases[]` | `id`, `group`, `status`, `reason` when absent, `requires`, diagnostic codes, observed status and code for an unserved case, `duration_ms` |
| `not_covered[]` | case id and the in-repository reference that covers it (§ 4) |
| `totals`, `verdict` | `passed`, `failed`, `absent`; `passed`, `failed` or `incomplete` |

**Endpoint identity is hashed.** A per-pod name carries the Kubernetes namespace (ADR 0046 K1), and
a namespace or host name can name a tenant, which the story keeps out of the report. A certifier
who knows the name recomputes the digest. The leaf certificate's digest binds the same identity
without naming it.

The report never carries a credential or anything derived from one, a subject, tenant or
principal, a clear-text origin or server name, a resource or operation id, argv, output bytes, a
refusal message (codes and HTTP statuses only), a path, the hook's output, or the Identity origin.

**Signing. Recommended: C, a detached signature.**

| option | what | cost |
|---|---|---|
| **A** the runner signs with an Ed25519 key passed by fd | self-contained | the runner becomes a key holder, with its own signature format and verifier to maintain |
| **B** keyless sigstore inside the runner (a sigstore client crate) | one step | a large new dependency tree for the advisory and licence gates, and an OIDC flow inside a conformance tool |
| **C** the runner writes `report.json` and `report.json.sha256`. Whoever certifies runs `cosign sign-blob --bundle report.json.sigstore.json` and verifies with `cosign verify-blob` | the `SHA256SUMS` pattern `release.yml` already uses; the runner holds no key | the runner itself produces an unsigned report |

**C reinterprets the story.** Its acceptance says "The runner produces a machine-readable signed
report". Under C the runner produces the report and its digest file, and the certification run
signs them. The story's acceptance text is revised in the planning store to say so before
implementation starts. The coordinator makes that revision. This note does not write the store.

Under C, this repository's lanes do not sign their reports. A lane report is test evidence, not a
certification, and keyless signing needs `id-token: write`, which AGENTS.md § *Releases* allows on
the release job only. Which identity may certify a deployment is a policy choice (§ 11, Q1).

## 7. D6: how this repository exercises the runner

**Portable, inside gate step 1.** `crates/b10x-substrate-conformance/tests/local_endpoint.rs`
copies the fixture of `crates/b10x-substrate-sdk/tests/remote.rs` and extends it. A copy, because
an integration test's helpers cannot be shared across crates without a test-support package,
which would be one more package to fence. The fixture starts the shipped `substrate-daemon` on the
production TLS listener with an `rcgen` private CA and an in-process Identity stub. The extension
adds `--event-retention 32`, a loopback `--ops-listen`, and a supervisor that restarts the daemon.
The test finds the daemon as `daemon_binary()` there does, and fails, rather than passes, when it is
missing. It runs the **binary** (`CARGO_BIN_EXE_b10x-substrate-conformance`) once, with:

- `--token-command`, a fixed argv that prints the stub's sentinel credential and serves `narrowed`;
- `--restart-hook`, `kill -KILL` of the daemon (a crash: SIGTERM would take the graceful path
  through `shutdown_signal`, `crates/substrate-daemon/src/runtime.rs:1636`). The supervisor
  restarts it on the same state, certificate and port;
- one `--forbidden-name`;
- `--deadline 60`, `--io-timeout 5` and `--restart-timeout 30`. The test kills the runner and fails
  if the runner has not exited after 90 s, so a hang cannot hold gate step 1.

The test asserts the verdict `passed`, zero failures, pinned `passed` and `absent` totals, an
absent set equal to the exec, resource, session and restart-exec cases with reason `fact-absent`,
the report valid against its schema, and every check in § 5. A second, partial run
(`--only machine,auth`) proves `--token-fd` over a socketpair. Negative cases each run once: exit 2
for an unknown advertised contract (a TLS stub answering another pair) and for an existing `--out`,
and exit 3 for a provider that prints a malformed credential. Each scenario runs once. The budget is
seconds: two daemon starts and one restart. The implementing story records the measured time, and
there is no repetition or soak (AGENTS.md § *The gate*). Under `--workspace` this binary is the
feature-unified one (§ 2).

**Delegated.** `scripts/delegated-lane.sh` gains one call after the remote SDK journey (`:215-216`).
At that point `daemon_bin` exists, and no other daemon holds the delegated root:

```bash
SUBSTRATE_TEST_DAEMON="${daemon_bin}" require_executed -p b10x-substrate-conformance local_endpoint \
  'conformance delegated exec: executed' \
  'conformance delegated resources: executed' \
  'conformance delegated sessions: executed' \
  'conformance delegated restart exec: executed'
```

`require_executed` (`:89-113`) runs the test binary with `-p`, so the runner it builds is the one
§ 2's check describes. It fails on any `ABSENT` in the output and on any line it does not find. The
binary's delegated test therefore prints `conformance delegated: ABSENT, SUBSTRATE_VECTORS_CGROUP_ROOT is not set`
and returns when the root is unset. Otherwise it hands the root to the daemon and prints each line
above only when every case of that group is `passed`. It pins its own totals and never prints the
report. The portable test in the same binary also runs there and stays green, because it never
hands the daemon a cgroup root. Neither test prints `ABSENT` otherwise. The `Confinement-lane:`
count stays `DELEGATED_CASES` of `runtime_vectors.rs`.

**The chart lane** (ADR 0046 K9 B, PR 182). This step belongs to `story:remote-clean-room-conformance`
and lands after PR 182 merges, as a change to `chart-lane.yml` and `xtask/src/chart_cluster.rs`.
After install, the lane runs the runner once against ordinal 0:

- `kubectl port-forward` to the pod, with `--endpoint https://127.0.0.1:<port>` and `--server-name`
  set to the per-pod DNS name. The SDK keeps the TCP destination apart from the verified name
  (design 20).
- `--forbidden-name` set to the headless Service name.
- `--token-fd` served by the verb with the credential its Identity stub admits. The stub runs in
  the verb's own process.
- No `--restart-hook`, so the restart group is `absent` (`input-not-declared`). The lane's own
  upgrade scenario already proves that a pod restart keeps the workspace and the ledger. The lane
  grants kind's nodes no delegated execution and reports its execution-enabled scenarios absent
  (`EXECUTION_ABSENT` in that verb), so the restart-exec assertions would be absent anyway.
  A pod deletion and readiness wait (up to 120 s) would exceed the budget by itself.

The lane requires the verdict `passed`. It prints the absent set into the job summary without
pinning it, because cluster facts vary, and it uploads the unsigned report as a workflow artifact.

**Budget.** ADR 0046 holds a workflow to growth of about a minute. The step costs one runner build
and one run. A cold release build of the SDK's dependency tree on a hosted runner (`tokio`,
`hyper`, `tokio-tungstenite`, `aws-lc-rs`) does not fit a minute, so the step never builds cold.
The `Dockerfile` gains a stage `conformance-runner`, `FROM builder`, whose own `cargo build --locked
--release -p b10x-substrate-conformance` runs in the same BuildKit session right after the cluster
job's daemon image build. It reuses that build's target cache mount and is exported with
`--output type=local`. The third-party artefacts are already compiled there. Only the SDK, without
`linked-daemon`, and the runner compile. It is a separate invocation, not a `-p` added to the image
build, because that build selects the MCP adapter and would unify `linked-daemon` back in (§ 2).
No published target depends on the stage. Without a restart, the run is the inventory over one
port-forward. The story measures build plus run on its pull request and records the time. If it is
above 60 s, the step moves to ADR 0046 K9 C's shape, a recorded local run, rather than growing the
workflow.

Rejected: running against kind from `gate.yml` (the per-workflow budget), and any repetition count
as evidence.

## 8. D7: wire impact

None. No route, schema, refusal, fact, header or event is added or changed. The runner reads the
frozen `operations.json` and `hosted-admission.json` of the bundle it pins and writes nothing
beneath `contracts/`. The next successor bundle moves the runner's pin in the same change, as it
moves `ADVERTISED_CONTRACT` in `runtime_vectors.rs`. A runner that meets a contract it does not pin
refuses with exit 2. The only option above that would have changed the wire, a daemon restart
route, is rejected in § 4.

## 9. D8: out of scope

- Provisioning clusters, daemons or Identity, and choosing a production fleet (the story's
  boundary).
- Fault injection inside the daemon. The crash-window vectors stay in `contract_vectors.rs`.
- Load, soak, latency targets and repeated runs.
- The Unix-socket and development static-bearer postures. The runner refuses an origin that is not
  `https://`.
- Certifying more than one endpoint per run, or a fleet.
- Running the bundle's per-vector `runner.json` protocol remotely.
- Schema-validating every response body against bundle schemas. Responses are decoded into the wire
  crate's closed types.
- A stable-contract claim. The bundle stays `development` (atlas ADR 0019).
- A release artifact for the runner binary (§ 11, Q3), and a conformance record in release tags.

## 10. What the implementing story touches

`crates/b10x-substrate-conformance/` (source, README, schema, tests), the workspace `members` and
`Cargo.lock`, `xtask/src/packages.rs` (`SOURCE_PACKAGES`, the manifest rule and the `cargo tree`
rule), and `scripts/delegated-lane.sh`. After PR 182 merges it also touches `Dockerfile` (the
`conformance-runner` stage), `.github/workflows/chart-lane.yml` and `xtask/src/chart_cluster.rs`.
`SOURCE_PACKAGES` requires a public documentation target, so it also touches a guide page under
`website/docs/guides/` with its `PAGES` and `website/sidebars.ts` entries (`cargo xtask
check-docs`), and `AGENTS.md`, `STATUS.md` and `CHANGELOG.md`. No new third-party package, so
`THIRD_PARTY_LICENSES.html` is unchanged.

## 11. Questions for the operator

- **Q1. Which signature certifies a deployment?** Recommended: C. Keyless `cosign sign-blob`
  under the certifier's OIDC identity, with a key-based signature and no transparency-log upload
  admitted for private deployments, because a public log entry records the signer identity and the
  report digest (not its content). Lane reports in this repository stay unsigned, which keeps
  `id-token: write` on the release job only. Accepting C accepts the revised story acceptance in
  § 6: the runner's report is signed by the certification run, not by the runner.
- **Q2. Is `incomplete` a failed certification?** Recommended: yes for the exit code (4, non-zero),
  with the report kept, so a deployment without workspace facts cannot pass by absence.
- **Q3. Should the runner binary ship as a release artifact?** Recommended: not yet. Build it from
  source at a revision with `-p` (ADR 0030, § 2), and decide release names separately, because they
  become a public interface (AGENTS.md § *Releases*).
