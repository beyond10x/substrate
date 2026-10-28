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
| `crates/substrate-daemon/tests/runtime_vectors.rs` | the clean-room shape: asserts only on the wire, pins the advertised contract independently (`ADVERTISED_CONTRACT`), pins case totals (`PORTABLE_CASES`, `DELEGATED_CASES`), absent cases are neither run nor counted | a test, not a binary; Unix socket only; it owns the daemon it checks (spawns, signals, reads its ledger rows) |
| SDK remote transport ([ADR 0029](../../adr/0029-the-remote-sdk-shares-one-verified-https-wss-transport.md), designs 20 and 23) | TLS 1.3 with explicit roots and server identity, `AccessTokenProvider`, contract-header checks, every route, WSS attachment authority | it recovers a lost mutation by itself (`recover_mutation`, `crates/b10x-substrate-sdk/src/lib.rs`) and refreshes after a 401, so it hides the server side of exactly the cases the story names |
| the bundle's runner protocol (`contracts/substrate-wire/0.24.0/runner.json`, `schemas/runner-result.json`) | the exit-code vocabulary (0 pass, 1 conformance failure, 2 invalid input, 3 harness failure) | per vector, with runner-owned `setup` (crash points, retention windows) a remote endpoint cannot accept; `pass` and `fail` only, no `absent`; executed in-process by `crates/substrate-daemon/tests/contract_vectors.rs` |
| the operation registry (`contracts/substrate-wire/0.24.0/operations.json`, source `xtask/bundle-source/0.24.0/routes.json`) | each operation's `capability_predicates` and `required_scope` | — |
| [ADR 0026](../../adr/0026-hosted-admission-resolves-opaque-identity-authority.md), `hosted-admission.json` | the credential format `identity_access_v1_<43 base64url>`, the scopes `exec`, `observe`, `workspaces`, the four `auth.*` refusals | — |
| [ADR 0045](../../adr/0045-operational-health-and-metrics-use-a-separate-listener.md) | `/livez`, `/readyz`, `/metrics` on a separate plaintext listener | — |
| [ADR 0046](../../adr/0046-kubernetes-serves-one-node-bound-daemon-per-stable-pod-address.md) | per-pod addressing (K1 A), no SAN for the headless Service (K2), the chart lane (K9 B) | the chart and `chart-lane.yml` are not on `main` yet (`.github/workflows/` at `ba75c00`) |
| `.github/workflows/release.yml` | keyless `cosign sign-blob --bundle` over `SHA256SUMS`, checked by `cosign verify-blob` | `id-token: write` exists on the release job and nowhere else (AGENTS.md § *Releases*) |

## 2. D1: where the runner lives

**Choice: A.** A new workspace crate `crates/b10x-substrate-conformance`, package and binary
`b10x-substrate-conformance`, `publish = false` (ADR 0030). Its only workspace dependencies, in
every dependency table, are `b10x-substrate-wire` and `b10x-substrate-sdk` with no features. Its
third-party dependencies are already workspace dependencies (`clap`, `rustls`, `tokio-rustls`,
`x509-parser`, `sha2`, `nix`, `zeroize`, `serde_json`; `jsonschema` and `rcgen` for tests only).

The SDK drives the positive journey: workspaces, files, execs, sessions over WSS, events,
metrics and operation reads. A small **raw probe** module, built on `rustls` directly and never
retrying, covers what the SDK deliberately cannot do: send a request without a credential or with
a forged one, speak plaintext or TLS 1.2, offer only `h2`, verify against a name the certificate
must not carry, and drop or discard a mutation's response. Every assertion is made on wire values.
Neither half reads SDK internals.

| option | rejected because |
|---|---|
| **B** a subcommand of the `b10x-substrate` CLI | the CLI depends on `b10x-substrate-host` (`crates/b10x-substrate-cli/Cargo.toml`), so the runner would link the host driver, which the story forbids |
| **C** an `xtask` verb | `xtask` is checkout tooling that links the bundle renderers. The story certifies from outside a checkout |
| **D** extend `runtime_vectors.rs` | a test, Unix-socket only, and it owns the daemon. A remote endpoint is neither |
| **E** the wire crate only, with its own HTTPS/WSS client | a second implementation of TLS configuration, token refresh and the session-authority transcript to keep in step with the SDK. The raw probe already covers what the SDK hides |

`cargo xtask check-packages` holds the boundary. It adds the crate to `SOURCE_PACKAGES`
(`xtask/src/packages.rs`) and refuses any other workspace path dependency in any of the crate's
dependency tables. It also refuses a `features` entry on its SDK edge, so `linked-daemon`, which
pulls in `b10x-substrate-daemon`, cannot be switched on. The tests find the daemon as a sibling
executable, as `daemon_binary()` in `crates/b10x-substrate-sdk/tests/remote.rs` does, so no
dev-dependency links it either.

## 3. D2: case selection and absence

**Choice: A, selection derived from the contract.** One run:

1. **Handshake.** `GET /v1/machine`. The `x-b10x-contract*` pair must equal a pair the runner pins
   itself, independently of `substrate_wire::ADVERTISED_CONTRACT_BUNDLE`, as `runtime_vectors.rs`
   does. The first pin is `substrate-wire/0.24.0`. An unknown pair stops the run with exit 2. The
   report names the advertised pair, and no case runs on a guess.
2. **Requirements.** Each case declares the registry operations it calls and any fact its
   assertion needs beyond them (`sessions.pty`, `exec.resource-usage`). The predicates come from
   the pinned bundle's `operations.json`, embedded at build time from the frozen directory, and are
   never restated in runner code.
3. **Run or absent.** A case runs when every predicate holds in the snapshot, its scope is in
   `--scopes` (default: all three hosted scopes), the inputs it needs are declared (a restart hook,
   an ops origin, a forbidden name), and its runner bound holds. Otherwise it is `absent` with one
   reason from a closed set: `fact-absent`, `fact-unsatisfied`, `scope-not-declared`,
   `input-not-declared` or `bound-exceeded`, plus the facts or inputs concerned.
4. **Absence is still checked.** For every route whose predicates fail, a `refusal-when-unserved`
   case runs. It asserts the published unserved refusal before dispatch (for example
   `exec.sandbox-unavailable`, 501), and asserts that nothing ran. That case passes or fails. This
   is the portable lane's shape: invariant 3 is checked where the capability is missing.

Statuses are `passed`, `failed` and `absent`. Totals count each separately, and an absent case is
never in `passed`. **Core cases** cannot be satisfied by absence: machine and contract, the TLS and
auth negatives, the workspace and file lifecycle, operation recovery, and cleanup. The verdict is
`failed` if any case failed, `passed` if none failed and every core case passed, and otherwise
`incomplete`. Exit codes: 0 passed, 1 failed, 2 invalid input, 3 harness failure, 4 incomplete.

| option | rejected because |
|---|---|
| **B** an operator case list or `--skip` | a deployment could certify itself by omission. `--only <group>` exists for debugging, and it marks the report `partial`, whose verdict is never `passed` |
| **C** run everything and read 501 as absent | it confuses a missing capability with a deployment that advertises a fact and then refuses it, which invariants 3 and 4 forbid |
| **D** a hand-written fact list per case | it drifts from the registry, which already states each route's predicates |

### Inventory

| group | cases (one scenario each) | needs | core |
|---|---|---|---|
| machine | snapshot decodes into the wire type; contract pair pinned; `valid_until` not passed; snapshot digest equal at start and end unless a restart was declared | — | yes |
| TLS | plaintext HTTP/1.1 gets no HTTP answer; a TLS 1.2-only hello fails; an `h2`-only ALPN offer fails; a presented client certificate without a bearer is still `auth.credential-absent` | — | yes |
| TLS name | verification fails for each `--forbidden-name` (the headless Service, ADR 0046 K2) | `--forbidden-name` | no |
| auth | no bearer: `auth.credential-absent` (401); a well-formed unknown credential: `auth.authority-invalid` (401); a mutation refused 401 under id Y leaves `GET /v1/ops/Y` `resource.not-found` | — | yes |
| auth scope | a narrowed credential gets `auth.scope-denied` (403) on a route outside its scopes | provider serves `narrowed` (§ 5) | no |
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
300 s).

## 4. D3: lost responses and restarts at an endpoint the runner does not control

| case | how | proves | cannot prove | hook |
|---|---|---|---|---|
| response read and discarded | the raw probe sends `PUT …/files/…` with id X, reads the response and discards it. The SDK then reads `GET /v1/ops/X` and replays the identical bytes under X | the replay returns the original outcome, the file's bytes and version are unchanged, and there is exactly one write event. The same X with a different body is a replay conflict (vector `replay-conflict.json`) | — | none |
| response dropped before read | the raw probe flushes the request and closes the TLS connection. The report records which branch the server took: not admitted (`resource.not-found`, then one replay succeeds) or admitted (the outcome as above) | no fabricated state and no second execution on either branch | which branch occurs, or where the server was when the connection closed | none |
| crash before or after dispatch | — | — | needs fault injection inside the daemon. It stays with vectors `crash-before-dispatch`, `crash-after-dispatch` and `restart-no-redispatch` in `contract_vectors.rs`, and the report lists it under `not_covered` | — |
| restart to unknown | see below | see below | that the restart was a crash rather than a clean stop: the hook decides how the process ends | `--restart-hook` |

**Restart sequence.** Create workspace W and write a file. If exec facts hold, start exec E, a
fixed program run through `--probe-shell` (default `/bin/sh`) that writes one marker into
`/workspace` and then blocks. Take an event cursor C and note `probed_at` as P0. Run the hook, then
poll `/v1/machine` until `probed_at` is later than P0, which shows a new process probed. The poll
is bounded by `--restart-timeout` (default 120 s, at most 600 s), and `/readyz` is also polled when
`--ops-origin` is declared. Then assert:

- E reads `unknown` ([ADR 0005](../../adr/0005-operations-are-durable-before-driver-dispatch.md):
  terminal only with proof). The implementing story confirms against the host driver's restart
  reconciliation whether any other state is provable for a child the daemon never saw exit; the
  case asserts exactly that set.
- The marker count is one, so nothing was re-dispatched. Replaying E's start under its operation id
  returns the original outcome, and the count is still one.
- W's bytes are readable.
- C answers either a continuous page or a named refusal (`event.source-mismatch` or
  `event.retention-gap`), never a silent jump in sequence.

Without exec facts, only the workspace, replay and cursor assertions run. The exec assertions are
`absent` (`fact-absent`).

**The hook is declared, not discovered.** `--restart-hook <absolute path>` takes repeatable
`--restart-hook-arg` values and is run argv-only, never as a shell string. Its environment is
cleared except for names passed with `--hook-env`, its stdin is `/dev/null`, and its stdout and
stderr are discarded, never reported. Only its exit status is recorded. A non-zero exit or a
timeout fails the restart group with diagnostic `hook-failed`.

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

- `--token-command <absolute path>` with repeatable `--token-command-arg`: argv-only, environment
  cleared except `--hook-env` names, stderr to `/dev/null`, exit 0. Exit 3 or the line
  `unsupported` means the purpose is not served, and the scope case is then `absent`
  (`input-not-declared`).
- `--token-fd <n>`: an inherited Unix stream socket (checked with `fstat`), so a supervising
  process can answer many requests and tell a refresh from a request.

Both are wrapped as the SDK's `AccessTokenProvider`, and the raw probe asks the same provider.
`refresh` is sent once after a named hosted 401 (design 20).

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
   The test's restart hook records its own environment, which must not contain the sentinel either.
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
| `endpoint` | origin, verified server name, TLS version, ALPN, SHA-256 of the leaf certificate and of the declared trust roots, leaf `not_after` |
| `contract` | advertised name and SHA-256 |
| `capability_snapshot` | SHA-256 of the canonical `/v1/machine` result (the wire crate's canonical hashing) at the first and last read, `driver`, `driver_version`, `config_generation` |
| `declared` | scopes; which inputs were declared (restart hook, ops origin, how many forbidden names); bounds |
| `cases[]` | `id`, `group`, `status`, `reason` when absent, `requires`, diagnostic codes, `duration_ms` |
| `not_covered[]` | case id and the in-repository reference that covers it (§ 4) |
| `totals`, `verdict` | `passed`, `failed`, `absent`; `passed`, `failed` or `incomplete` |

The report never carries a credential or anything derived from one, a subject, tenant or
principal, a resource or operation id, argv, output bytes, a refusal message (codes and HTTP
statuses only), a path, the hook's output, or the Identity origin.

**Signing. Recommended: C, a detached signature.**

| option | what | cost |
|---|---|---|
| **A** the runner signs with an Ed25519 key passed by fd | self-contained | the runner becomes a key holder, with its own signature format and verifier to maintain |
| **B** keyless sigstore inside the runner (a sigstore client crate) | one step | a large new dependency tree for the advisory and licence gates, and an OIDC flow inside a conformance tool |
| **C** the runner writes `report.json` and `report.json.sha256`. Whoever certifies runs `cosign sign-blob --bundle report.json.sigstore.json` and verifies with `cosign verify-blob` | the `SHA256SUMS` pattern `release.yml` already uses; the runner holds no key | "the runner produces a signed report" becomes "the certification run signs the runner's report" |

Under C, this repository's lanes do not sign their reports. A lane report is test evidence, not a
certification, and keyless signing needs `id-token: write`, which AGENTS.md § *Releases* allows on
the release job only. Which identity may certify a deployment is a policy choice (§ 11, Q1).

## 7. D6: how this repository exercises the runner

**Portable, inside gate step 1.** `crates/b10x-substrate-conformance/tests/local_endpoint.rs` uses
the fixture of `crates/b10x-substrate-sdk/tests/remote.rs`: it starts the shipped
`substrate-daemon` on the production TLS listener with an `rcgen` private CA and an in-process
Identity stub, adding `--event-retention 32` and a loopback `--ops-listen`. It then runs the
**binary** (`CARGO_BIN_EXE_b10x-substrate-conformance`) once, with:

- `--token-command`, a fixed argv that prints the stub's sentinel credential and serves `narrowed`;
- `--restart-hook`, `kill -TERM` of the daemon, which a supervisor task in the test restarts on the
  same state, certificate and port;
- one `--forbidden-name`.

The test asserts the verdict `passed`, zero failures, pinned `passed` and `absent` totals, an
absent set equal to the exec, resource, session and restart-exec cases with reason `fact-absent`,
the report valid against its schema, and every check in § 5. A second, partial run
(`--only machine,auth`) proves `--token-fd` over a socketpair. Negative cases each run once: exit 2
for an unknown advertised contract (a TLS stub answering another pair) and for an existing `--out`,
and exit 3 for a provider that prints a malformed credential. Each scenario runs once. The budget is
seconds: two daemon starts and one restart. The implementing story records the measured time, and
there is no repetition or soak (AGENTS.md § *The gate*).

**Delegated.** `scripts/delegated-lane.sh` gains
`require_executed -p b10x-substrate-conformance local_endpoint`, so the exec, resource, raw-pipe,
PTY and restart-to-unknown exec cases run under the delegated cgroup. Their totals are pinned
separately. The `Confinement-lane:` count stays `DELEGATED_CASES` of `runtime_vectors.rs`.

**The chart lane** (ADR 0046 K9 B, owned by `story:node-bound-kubernetes-serving-profile`). After
install, the lane runs the runner once against ordinal 0:

- `kubectl port-forward` to the pod, with `--endpoint https://127.0.0.1:<port>` and `--server-name`
  set to the per-pod DNS name. The SDK keeps the TCP destination apart from the verified name
  (design 20).
- `--forbidden-name` set to the headless Service name.
- `--token-fd` served by the lane's `xtask` verb from the in-cluster Identity stub.
- `--restart-hook` set to a subcommand of that verb, which deletes the pod, waits for readiness and
  re-binds the forward.

The lane requires the verdict `passed`. It prints the absent set into the job summary without
pinning it, because cluster facts vary (ADR 0046 K9 lists AppArmor and cgroup unknowns), and it
uploads the unsigned report as a workflow artifact. This step lands with whichever of the two
stories merges second.

Rejected: running against kind from `gate.yml` (the per-workflow budget), and any repetition count
as evidence.

## 8. D7: wire impact

None. No route, schema, refusal, fact, header or event is added or changed. The runner reads the
frozen `operations.json` of the bundle it pins and writes nothing beneath `contracts/`. The next
successor bundle moves the runner's pin in the same change, as it moves `ADVERTISED_CONTRACT` in
`runtime_vectors.rs`. A runner that meets a contract it does not pin refuses with exit 2. The only
option above that would have changed the wire, a daemon restart route, is rejected in § 4.

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
`Cargo.lock`, `xtask/src/packages.rs` (`SOURCE_PACKAGES` and the boundary rule), and
`scripts/delegated-lane.sh`. `SOURCE_PACKAGES` requires a public documentation target, so it also
touches a guide page under `website/docs/guides/` with its `PAGES` and `website/sidebars.ts` entries
(`cargo xtask check-docs`), and `AGENTS.md`, `STATUS.md` and `CHANGELOG.md`. No new third-party
dependency, so `THIRD_PARTY_LICENSES.html` is unchanged.

## 11. Questions for the operator

- **Q1. Which signature certifies a deployment?** Recommended: C. Keyless `cosign sign-blob`
  under the certifier's OIDC identity, with a key-based signature and no transparency-log upload
  admitted for private deployments, because a public log entry records the signer identity and the
  report digest (not its content). Lane reports in this repository stay unsigned, which keeps
  `id-token: write` on the release job only. Accepting C also reads the story's "produces a signed
  report" as "a report with a detached signature made at certification time".
- **Q2. Is `incomplete` a failed certification?** Recommended: yes for the exit code (4, non-zero),
  with the report kept, so a deployment without workspace facts cannot pass by absence.
- **Q3. Should the runner binary ship as a release artifact?** Recommended: not yet. Build it from
  source at a revision (ADR 0030), and decide release names separately, because they become a
  public interface (AGENTS.md § *Releases*).
