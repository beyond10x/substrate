# substrate

The b10x execution data plane. It turns one Linux host into a governed service for confined
workspaces, bounded processes and sessions, durable operations, leases, and observed state.
Workloads, images, volumes and endpoints remain future resource families. A Kubernetes namespace
driver ships from 0.16.1 as a development profile; it serves workspaces and refuses every exec.

The problem it removes: anything that wants to run a command on behalf of somebody else has to
build confinement, quotas, durable lifecycle state and honest reporting for itself, and usually
builds the reporting optimistically. Substrate runs things and reports **what it observed**. Where
the machine cannot confine, it says so — an exec on a host with no delegated cgroup answers
`exec.sandbox-unavailable` rather than running unconfined.

It does not decide product policy, run agent loops, or understand connector vendors.

[Public handbook](https://beyond10x.github.io/substrate/docs/) ·
[Run one confined command](https://beyond10x.github.io/substrate/docs/getting-started) ·
[System model and derivation](https://beyond10x.github.io/substrate/docs/concepts/model/) ·
[Run a bounded command](https://beyond10x.github.io/substrate/docs/guides/run-a-command/)

Source: <https://github.com/beyond10x/substrate/> · Security reports:
<https://github.com/beyond10x/substrate/security/advisories/new>

Substrate is licensed under [Apache-2.0](LICENSE), including all beyond10x-owned material in its
reachable history. Third-party material retains its own licence. Public source does not make the
development wire contract stable.

## Where it sits

| direction | what |
|---|---|
| confines | [harness](https://github.com/beyond10x/harness) — over the daemon socket, directly or through the Rust SDK |
| may govern | [connectors](https://github.com/beyond10x/connectors) — as a first-party provider, and later to isolate an attested connector artifact |
| may execute for | [autodev](https://github.com/beyond10x/autodev) — over its `Executor` port |
| may adapt | [flux](https://github.com/codewandler/flux) — a remote execution adapter over the substrate API. The dependency never points back into Flux |

There is **no sibling-component implementation dependency**: a consumer embeds Substrate, and
Substrate embeds nothing of theirs (AGENTS.md invariant 2). Cross-component consumers use the
released native `substrate-daemon` artifact or `b10x-substrate-sdk` over the authenticated socket.
A consumer that wants no deployment at all instead embeds `b10x-substrate-host` and
`b10x-substrate-wire` in-process from an exact Git revision — that is the seam
[harness](https://github.com/beyond10x/harness) pins today in
`crates/harness-substrate/Cargo.toml`, and it is a consumer surface of this repository like the
other two. An embedder that only needs to run a command under confinement uses that crate's
`substrate_host::confinement` module: it opens the adapters the host offers, reads the families and
facts each proved, and admits a `ConfinementProfile` against them, with a named refusal for
anything not proved ([choosing a profile](website/docs/guides/confinement-profiles.md)).
The SDK's opt-in linked mode may package the daemon solely to re-execute it as a
separate child; resource operations still cross the authenticated Unix socket.

The product and binary name are `substrate`. Rust source packages use the `b10x-substrate-*`
prefix and are consumed from a local path or exact Git revision; every workspace package is
non-publishable.

## Status

**Release [0.16.1](https://github.com/beyond10x/substrate/releases/tag/0.16.1) (2026-10-29)**
ships keyless-signed daemon, disposable MCP and executor images, the signed Helm chart, the signed `0.25.0`
development contract bundle, and signed x86_64 Linux binaries of the daemon and the command-line
client. It is the first release of the Kubernetes namespace
driver ([below](#kubernetes-namespace-driver)), a development profile; tag `0.16.0` was not published. Signed distribution does not make the contract stable. Why bytes annotated
`development` are signed, digest-pinned and published under a write-once tag at all is recorded in
[ADR 0006](adr/0006-substrate-publishes-its-own-contract-bundle.md) § *Addendum*, which also records
that no accepted text states the write-once-tag motive.

| Area | Current behavior |
|---|---|
| Workspaces | Empty or authorized configured HTTPS Git source; guarded file operations, exact-commit baseline and bounded change reads |
| Exec and sessions | Confined argv execution, raw pipes and probe-gated PTY; explicit bounds, leases, output and optional exact usage |
| Recovery | Durable operation reservation before dispatch, stored answers, bounded events and reconciliation snapshots |
| Transport | Personal Unix peer identity or explicit-root TLS 1.3 HTTPS/WSS with online hosted Identity admission |
| Rust SDK and MCP | Typed clients and bounded tools over the daemon's authenticated service contract |
| Drivers | The Linux host driver; from 0.16.1, the Kubernetes namespace driver, a development profile, which serves empty workspaces and refuses every exec start `exec.sandbox-unavailable` |
| Capability limits | Execution, PTY, Git, storage quotas and metrics depend on the running daemon's verified facts |
| Model and derivation | Explicit wire types and frozen contract bundles; authored CLI, handlers and transitions, with an initial [ESS accepted-operation projection](spec/README.md), not whole-system derivation |

Read the [public status page](https://beyond10x.github.io/substrate/docs/status/) for availability
and trust limits. Inspect `GET /v1/machine` for the facts of the daemon you will actually use.

Per-area state with the exact next proof each is waiting for is [`STATUS.md`](STATUS.md); ordered
exit criteria are [`ROADMAP.md`](ROADMAP.md).

## Build, test, run

The gate is **`bash scripts/gate.sh`**. It is the full component gate; green here is the bar for
main.

The table is the gate's own order (`scripts/gate.sh`).

| step | command |
|---|---|
| tests | `cargo test --workspace --release --locked` |
| format | `cargo fmt --all --check` |
| lint | `cargo clippy --workspace --all-targets --release --locked -- -D warnings` |
| links | `cargo xtask check-links` — rejects machine-local and broken repository-relative links |
| ADRs | `cargo xtask check-adrs` |
| secrets | `cargo xtask check-secrets` — scans every reachable commit, including root trees |
| dependencies | `cargo xtask check-advisories` — rejects RustSec findings and HTTP/2 |
| licences | `cargo xtask check-licenses` — verifies Apache-2.0 workspace metadata and deterministic third-party notices |
| packages | `cargo xtask check-packages` — refuses every publishable workspace package, verifies the ten runtime source packages' fixed names, exact internal versions, SPDX metadata, READMEs and public documentation targets, and refuses a Flux dependency or a `beyond10x` Git dependency in any dependency table |
| MCP boundary | `cargo xtask check-mcp-boundary` — refuses a publishable MCP crate, a Substrate implementation dependency, an unpinned or featured MCP stack, and a built-in unbounded or remote transport |
| contract bundle 0.1.0 | `python3 scripts/check-contract-bundle.py` |
| contract bundle 0.2.0 | `python3 scripts/check-contract-bundle-0.2.0.py` |
| contract bundle 0.3.0 | `python3 scripts/check-contract-bundle-0.3.0.py` |
| contract bundle 0.4.0 | `python3 scripts/check-contract-bundle-0.4.0.py` |
| contract bundles 0.5.0–0.25.0 | `cargo xtask check-bundles 0.5.0 … 0.25.0` — bounded parallel fixed-point, compatibility, classification and version-addition checks; `check-bundle <version>` remains the focused form |
| contract JSON | `cargo xtask check-json` — every JSON under `contracts/` is classified by exactly one bundled schema, or it fails closed |
| toolchain | `cargo xtask check-toolchain` |

Rust 1.97, edition 2024 — the toolchain is pinned by `rust-toolchain.toml`, and
`cargo xtask check-toolchain` fails when it, `Cargo.toml`'s `rust-version` and the `Dockerfile`
builder tag disagree. `.github/workflows/gate.yml` runs the same gate on push and pull request.

Every check the gate runs is a `cargo xtask` verb — anything that runs in a b10x foundation
repository is Rust. Two verbs are not gate steps because `cargo test --workspace --release --locked`, the
gate's first step, already covers them:

| verb | what it does |
|---|---|
| `cargo xtask package-bundle <version> --out <dir>` | packages a released bundle as a deterministic OCI image layout, so a consumer can pin one manifest digest |
| `cargo xtask render-bundle <version> --out <dir>` | renders a bundle tree from `substrate-wire` and the authored source at `xtask/bundle-source/<version>/`; this is how a successor bundle is cut, and it refuses to write anywhere under `contracts/` |

`cargo xtask check-bundles <version>...` runs the 0.5.0-and-later gate checks concurrently with a
bounded worker count and deterministic reporting. Its focused `check-bundle <version>` form runs
the same check for one bundle. Re-rendering and comparing bytes catches a released tree that has
stopped being the fixed point of its own source, which a hand-written checker cannot see.

The four `scripts/render-contract-bundle*.py` and their `check-contract-bundle*.py` partners are
**not tooling** — they are the reproducibility proof of the frozen `0.1.0`–`0.4.0` bundles, which
are immutable, and `0.4.0`'s own `generator.name` points at one of them. They stay in Python and
are not ported. `render-bundle` dispatches to the frozen renderer for `0.5.0`–`0.8.0` and the
versioned multi-major renderer for `0.9.0` and later; a test asserts the original renderer still reproduces
the frozen `0.4.0` byte for byte.

`crates/substrate-daemon/tests/runtime_vectors.rs` is the clean-room runner — an independent
Unix-socket HTTP lane that spawns the shipped `substrate-daemon` binary and asserts only on the
wire, linking no implementation. It has no gate step of its own because the gate's first step,
`cargo test --workspace --release --locked`, runs it. **`bash scripts/delegated-lane.sh` runs that lane**, and needs no privilege: it asks systemd for a
delegated scope (`systemd-run --user -p Delegate=yes --scope`), moves itself into a child group so
the delegation root stays process-free, and sets the variable. A user session's own scope is
root-owned, so trying to `mkdir` in it fails and the lane reports itself **absent, not passed**
(invariant 3) — which reads exactly like a green run if you only look at `cargo test`.

Set `SUBSTRATE_VECTORS_CGROUP_ROOT=<delegated-root>` by hand instead,
**while the test process itself is inside that delegation**, to add the real no-egress,
shaped-environment, pids/memory, timeout, truncation and whole-tree cancellation cases:

```console
cargo test --workspace --release --locked -- --nocapture   # the runner prints its case inventory
```

The runner prints its current portable or delegated case inventory from each fresh execution;
this document deliberately pins no counts that drift as adversarial coverage grows.

**Neither lane covers the real project-quota cases.** The seven `real_quota_*` cases in
`crates/substrate-host/src/git/quota_tests.rs` are `#[ignore]`d on an explicitly delegated
project-quota fixture, and no script passes `--ignored`. `cargo xtask quota-lane` runs them:

```console
SUBSTRATE_TEST_QUOTA_ROOT=/quota SUBSTRATE_TEST_PROJECT_QUOTA_IDS=200000-200511 \
  cargo xtask quota-lane
```

The root must sit on a filesystem mounted with project quotas (`prjquota`, or XFS `pquota`) and the
range must be an exclusive inclusive `START-END` of at least 128 identities. **Absent either, the
verb refuses by name and exits 1**, listing each missing prerequisite and every case that did not
run, rather than reporting nothing. `cargo xtask quota-lane --inventory` prints that statement
without running anything, and `scripts/delegated-lane.sh` ends with it so a green delegated lane
cannot be read as covering these cases.

`cargo xtask check-json` fails closed on unclassified or schema-invalid contract JSON and
meta-validates every Draft 2020-12 schema offline, across all twenty-five released bundles. Classification
used to live in a Python module the four checkers imported — shared live machinery, not any one
bundle's reproducibility proof — so it moved with the rest of the tooling, and the four checkers no
longer do it. They verify everything else about the bundles they froze.

`cargo xtask package-bundle <version> --out <dir>` packages a released contract bundle as a
deterministic OCI image layout — `0.4.0` reproduces manifest
`sha256:3758e80bc39f1eb03b15c69410608c9ef1d2ba8095c7e707c6988dbb5894ab00`. It reads
`contracts/` read-only and is not a gate step of its own: its cases run in `cargo test`. On an
eligible release tag, the release workflow copies that exact layout to GHCR with ORAS, confirms the
remote digest, and signs and verifies the digest before announcement. The workflow's offline
write-once and ordering assertions run in `xtask/tests/release_workflow.rs`.

### Release binaries

From 0.14.0 on, each GitHub release also carries the daemon and the command-line
client as prebuilt x86_64 Linux binaries:

| asset | holds |
|---|---|
| `substrate-daemon-<version>-x86_64-unknown-linux-gnu.tar.gz` | `substrate-daemon`, `LICENSE`, `THIRD_PARTY_LICENSES.html`, `README.md` |
| `b10x-substrate-<version>-x86_64-unknown-linux-gnu.tar.gz` | `b10x-substrate`, the same three files |
| `SHA256SUMS` | the SHA-256 of both archives |
| `SHA256SUMS.sigstore.json` | the keyless `cosign sign-blob` bundle for `SHA256SUMS` |

The release workflow exports both binaries from the Dockerfile's pinned `builder` stage, the stage
the images copy from, so they share the image toolchain. The daemon archive holds the daemon of the
published daemon image byte for byte: a first release run refuses any difference, and a recovery
run that reuses an already published image archives that image's daemon. The binaries run on
glibc 2.35 or newer (Ubuntu 22.04, Debian 12 and later) and load two libraries beyond glibc and
`libgcc_s`: OpenSSL 3 (`libssl3`, `libssl3t64` on Ubuntu 24.04) and zlib (`zlib1g`), both present
on a stock Ubuntu 22.04 image. Before any image tag moves, the workflow refuses a binary that
needs a newer glibc or any other shared library, and starts both in a digest-pinned Ubuntu 22.04
image. A recovery run of a tag whose source predates the `release-binaries` stage publishes no
archives, no `SHA256SUMS` and no signature, and its release notes say so.

Verify the signature, then the archives. The certificate identity is the release workflow at the
ref that ran it, which the release notes print:
`https://github.com/beyond10x/substrate/.github/workflows/release.yml@refs/tags/<version>` for a
release started by pushing its tag, and
`https://github.com/beyond10x/substrate/.github/workflows/release.yml@refs/heads/main` for one
published by a recovery dispatch from `main`. The image and contract-bundle signatures of a
release carry the same identity, so `cosign verify` of an image takes the same value.

```bash
cosign verify-blob \
  --bundle SHA256SUMS.sigstore.json \
  --certificate-identity "https://github.com/beyond10x/substrate/.github/workflows/release.yml@refs/tags/<version>" \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com \
  SHA256SUMS
sha256sum --check --ignore-missing SHA256SUMS
```

The daemon binary alone does not confine anything: execution still needs bubblewrap and a delegated
cgroup v2 subtree, and without them exec answers `exec.sandbox-unavailable`.

### Running the daemon

By default `substrate-daemon` serves an owner-permissioned Unix socket. Startup requires at least
one explicit `--allow-uid`; the daemon derives `local:<uid>` from **kernel peer credentials** and
never accepts a subject from HTTP data.

```console
cargo build --workspace --locked
install -d -m 700 ./run
target/debug/substrate-daemon \
  --socket ./run/substrate.sock \
  --state ./run/state.db \
  --workspaces ./run/workspaces \
  --deployment personal \
  --event-retention 10000 \
  --allow-uid "$(id -u)"
```

Rust applications can instead follow the [public Rust SDK guide](https://beyond10x.github.io/substrate/docs/guides/rust-sdk/)
to connect to that socket or supervise the daemon as a separate child.

From a shell, the `b10x-substrate` client drives that socket through the same SDK. `--socket`
selects the daemon and falls back to `SUBSTRATE_SOCKET`; `--output json` prints a
`{"ok":…}` envelope, and a refusal prints the daemon's named code and exits non-zero. `exec run`
passes the command's output through and exits with its status. The
[command-line guide](https://beyond10x.github.io/substrate/docs/guides/cli/) lists every command.

```bash
cargo install --path crates/b10x-substrate-cli --locked
export SUBSTRATE_SOCKET=./run/substrate.sock
b10x-substrate machine
ws=$(b10x-substrate --output json workspace create | jq -r .result.workspace)
b10x-substrate exec run --workspace "$ws" -- /usr/bin/printf 'hello\n'
b10x-substrate workspace destroy "$ws"
```

The daemon image also provides `/usr/local/bin/substrate-daemon-quota` for explicit project-quota
deployments. It is a root-owned, byte-identical copy carrying only `cap_sys_admin=ep`; the default
executable and entrypoint have no file capabilities. Select the quota executable together with
`--project-quota-ids START-END`, a filesystem that passes the hard byte/inode quota probe, UID/GID
65532, and only `SYS_ADMIN` in the container capability bounding set. Startup must allow file
capability acquisition (`no_new_privs` disabled). Keep inheritable and ambient sets empty and never
grant `SYS_RESOURCE`, which can bypass quotas. Deploy and roll back the image and selected
executable together.

Release checks inspect the final image's bytes, ownership and file capabilities, then verify the
daemon and Tokio worker masks under both startup profiles. They do not prove a served terminal
backend or filesystem quotas: the image still requires a separately verified execution profile,
and quota facts remain absent until the host probe succeeds. The process launcher continues to
set `PR_SET_NO_NEW_PRIVS` before executing its trusted backend; this prevents gaining capabilities
at exec, while an ordinary non-root exec with empty inheritable/ambient sets clears existing
permitted/effective capabilities. `PR_SET_NO_NEW_PRIVS` alone does not clear capabilities at fork.

### Container execution

The daemon image includes `/usr/local/bin/substrate-container-exec` for an explicit container
execution profile. It requires Linux amd64, private PID/mount/cgroup namespaces, finite enclosing
CPU and memory limits, the enforced `substrate-host-exec-v2` AppArmor profile and the matching
versioned [seccomp profile](deploy/seccomp/README.md). The application receives no host paths or
host namespaces. Start the bootstrap as container PID 1 and UID/GID 0 with exactly CHOWN, SETGID,
SETUID, SETPCAP and SYS_ADMIN after dropping all other capabilities. Supply a writable temporary
directory and the existing private state/workspace volumes.

Pass ordinary daemon arguments after `--`; prepend `--project-quotas` when using the existing
quota executable. The bootstrap verifies its enclosing resource controls, prepares only the
container's private delegation, drops to UID/GID 65532 and starts the daemon beneath an init
process that reaps orphaned children. Ordinary mode drops every capability; quota mode retains
only the existing SYS_ADMIN file-capability path. The daemon's existing probes decide whether
execution and PTY facts are present. Missing prerequisites remain startup or capability refusals.

Provision node profiles separately with the image's `substrate-container-profiles` executable.
The installer needs only MAC_ADMIN, securityfs and a dedicated root-owned kubelet seccomp
directory mounted at `/node-profiles`. It installs fixed versioned bytes and refuses conflicting
files or an already-loaded profile without its exact retained source. `--check` verifies without
writing; `--hold` supports deployment readiness and normal termination. Application pods never
receive these mounts or installation authority.

The profile name is a deployment parameter
([design 33](docs/design/33-host-profile-name-parameter.md)). `--apparmor-profile <name>` on both
executables, and `execution.appArmorProfile` in the chart, name `substrate-host-exec-v2` (the
default), `substrate-host-exec-v1`, or either followed by `-<suffix>`. The installer renders that
version's policy under the name, loads it from `<name>.apparmor` and holds it to those exact
bytes; the bootstrap requires `<name> (enforce)`. `v2` is `v1` with `noatime` and `nodiratime`
admitted on a remount, which bubblewrap needs on a node whose filesystems are mounted `noatime`;
`v1` is unchanged since `0.10.0`.

The `container-checker` Docker build target contains a public-SDK example for final-image
acceptance. Run its `container-pty-check SOCKET` executable as the admitted non-root UID inside
the tested container's PID, mount and cgroup namespaces. It verifies file bytes, generated PTY
input, resize, worker capability removal, live resource observations and whole-tree cleanup.
Run both ordinary and quota variants, the existing delegated lane and image-startup checks before
admitting terminals. The portable CI lane alone does not prove container execution.

### Kubernetes

[`deploy/helm/substrate/`](deploy/helm/substrate/) is the node-bound serving profile of
[ADR 0046](adr/0046-kubernetes-serves-one-node-bound-daemon-per-stable-pod-address.md): one daemon pod per dedicated node,
each with its own ledger claim, TLS certificate and Identity admission, addressed by its own host
name. With `execution.enabled` the pod starts the bootstrap above, and `nodeProfiles.enabled`
installs the node profiles. Each release publishes the chart, signed and with build provenance, at
`oci://ghcr.io/beyond10x/charts/b10x-substrate`. The
[deployment guide](website/docs/guides/deployment.md#kubernetes-one-daemon-per-dedicated-node)
lists what to supply, what the chart refuses and how to rotate certificates. `cargo xtask
chart-lane static` checks the chart offline with Helm; `chart-lane cluster` runs it on kind
(`.github/workflows/chart-lane.yml`).

### Kubernetes namespace driver

`substrate-daemon --driver kubernetes --kubernetes-config <file>` serves the wire from one
namespace ([ADR 0047](adr/0047-a-kubernetes-namespace-driver-serves-workspaces-on-pvcs-and-execs-as-pods.md),
[design 34](docs/design/34-kubernetes-namespace-driver.md)). It is a development profile,
first released in 0.16.1. A workspace is a PersistentVolumeClaim with a keeper Pod that runs the static
`substrate-executor`; every file route reaches it through `pods/exec`. The driver claims the
namespace with the Lease `b10x-substrate-driver`, holds one namespaced Role and refuses to open on
more, and publishes `driver: kubernetes` with only the facts its own probe proved.

Its limits, stated plainly:

- **Exec is refused.** The probe proves neither the four Linux confinement facts nor
  `confinement.no-egress`, so every exec start is `exec.sandbox-unavailable` (501). Exec Pods are
  implemented but unverified on a real cluster: on kind no enforced egress denial was observed,
  so `confinement.no-egress` stays absent.
- **Shared clean-room conformance follows in the next release.** `cargo xtask namespace-lane
  observe` runs design 34's observations and steps 3 and 5 to 7 on a kind cluster of its own, and
  reports step 4 absent until `story:remote-clean-room-conformance` lands.

From 0.16.1, the first published release of the driver (tag `0.16.0` was not published), each
release publishes the executor image
`ghcr.io/beyond10x/b10x-substrate-executor:<version>` and requires a `Namespace-lane:` record for
the tagged commit ([AGENTS.md](AGENTS.md) § *Releases*). The
[deployment guide](website/docs/guides/deployment.md#kubernetes-namespace-driver) lists the
settings, the Role, the startup refusals and the answers.

### Git workspace sources

`--git-source <name>=<https-prefix>/` (repeatable) declares a segment-bounded Connector byte-plane
source. A Git workspace request names that source and a broker-issued locator below the prefix,
plus the provider's branch, exact 40-character commit and a depth from 1 through 50. The source
authority arrives only in `X-B10X-Workspace-Source-Authorization`; it is never JSON, ledger state,
argv, environment or an error. The configured prefix must end in `/` so an adjacent path cannot be
mistaken for a child path.

The SDK's `Workspace::read_git_file` reads a complete baseline blob under a caller-declared byte
bound, while `Workspace::git_changes` returns a path-sorted item/patch-byte-bounded comparison with
the exact materialization commit. Current bytes still use `read_file_v2`; `.git` is deliberately
absent from every file/tree API and remains available only inside the confined terminal.

### Secret slots

`--secret-slot <name>=<path>` (repeatable) declares a secret the daemon holds for a run. The value
reaches a child only as a **sealed memfd at a declared descriptor** — never in argv, never in the
environment, never in an event, the ledger or an error body. The child gets the mapping and nothing
else, through `SUBSTRATE_SECRET_SLOTS=<name>=<fd>,…`; the daemon closes its own copy immediately
after spawn, and the seal set is `F_SEAL_WRITE|F_SEAL_SHRINK|F_SEAL_GROW|F_SEAL_SEAL`, which a child
can confirm with `fcntl(fd, F_GET_SEALS)`.

```console
target/debug/substrate-daemon \
  --socket ./run/substrate.sock \
  --state ./run/state.db \
  --workspaces ./run/workspaces \
  --deployment personal \
  --allow-uid "$(id -u)" \
  --secret-slot model_key=/etc/substrate/model-key
```

A slot **name** is lowercase ASCII, digits and `_`, first character a letter, at most 64 bytes
(`valid_secret_slot_name` in `crates/substrate-wire/src/lib.rs`) — a hyphen is refused. The **path** never leaves the
daemon process — it is not a capability fact, not an event field and
not an error message. An error may name a slot; it never names a value. Rotating the file behind a
declared name needs no restart and invalidates no admitted operation. The ledger request hash covers
slot **names** only, so two requests differing only in a slot's value hash identically. Where sealing
is unavailable the capability fact `secrets.slots` is **absent and the operation is refused by
name** — it never degrades to passing the value some other way (invariant 3). ADR 0012.

### Egress apertures

Ordinary execution has no egress and that does not move: every run is under `--unshare-net` in a
namespace with loopback and nothing else. An **aperture** is a separate, operator-declared authority
to reach exactly one destination — `--egress-aperture <name>=<host>:<port>/tcp[/max=<size>]`,
repeatable. A request selects one **by name** and can never carry a destination or a ceiling:

```console
target/debug/substrate-daemon \
  --socket ./run/substrate.sock \
  --state ./run/state.db \
  --workspaces ./run/workspaces \
  --deployment personal \
  --allow-uid "$(id -u)" \
  --cgroup-root /sys/fs/cgroup/…/substrate \
  --egress-aperture model=api.example.com:443/tcp \
  --ca-bundle /etc/ssl/certs/ca-certificates.crt
```

```json
{ "sandbox": { "network": "aperture", "aperture": "model", "profile": "workspace", "…": "…" } }
```

The host is resolved **once**, at declaration, and pinned; the sandbox gets no resolver and performs
no lookup. Inside the run, the declared name maps to loopback through a generated read-only
`/etc/hosts` and the forwarder listens on the declared port, so `https://api.example.com/…` is the
URL a child uses unchanged — and, where `--ca-bundle` is configured, verifies against a private
per-run snapshot of that anchor. The pinned address itself is **not** reachable directly: the
aperture is the only peer in the namespace.

What was installed is reported rather than inferred — `applied.network` becomes
`{mode, name, destination, mechanism, bytes, max_bytes}`, with the address the forwarder actually
dialled and the bytes counted where they crossed. An aperture nobody declared is `unserved` **with
the name in the message**; where the mechanism did not verify in a throwaway sandbox at startup, the
capability fact `exec.egress-apertures` is absent and every aperture request is `unserved` — never a
run that quietly got no network instead (invariant 3). ADR 0013.

A run that needs two to four destinations names them as a list, `"apertures": ["model", "audit"]`,
in place of `aperture`. Each keeps its own forwarder and ceiling; the n-th listens on `127.0.0.n`,
and `applied.network` becomes an array whose elements add that `listen` address. Two listed
apertures that declare one host name, or `localhost` listed anywhere but first, are refused
`exec.aperture-host-listed-twice` before anything is installed, because the child could reach only
one of them by that name. Design 26.

The optional `/max=<size>` term bounds **how much** may cross, over both directions summed, for one
run — `1048576`, `512KiB`, `64MiB`, `2GiB`, and never a decimal-power unit such as `MB`. An
unrecognised term is a startup error, not an ignored one, and an aperture declared without the term
passes exactly what it passed before. The relay stops relaying at the ceiling, so the total may
exceed it by at most one 16 KiB relay buffer per live connection; the run is then ended and the
observation carries `refusal: {class: "exhausted", code: "exec.aperture-byte-limit", …}` beside a
state of `cancelled`. The child is told nothing — its socket closes mid-stream and the tree is
killed. A ceiling in a request is refused `exec.aperture-ceiling-in-request`. ADR 0014.

### Grant attribution

`--delegated-context-key <kid>=<issuer>=<base64url>` (repeatable) declares a key substrate will
**verify** delegated-context documents against. Substrate holds a verifying key and never a signing
key: which service signs is a configuration of the trusted key and changes no substrate code.

A start may then carry `delegated_context`, a compact JWS, alongside `op` and `input` — never inside
`input`, so it stays outside the canonical request hash. Replaying the same `op` with a *fresh*
context is the same operation and returns the original outcome, and a request without one serializes
exactly as a `0.6.0` client's did.

```console
substrate-daemon \
  --delegated-context-key k1=https://identity.example.com=g5Iv…A0
```

What a verified document contributes is two columns and nothing else: `grant_ref` and
`platform_principal`, on the ledger row and on the `operation.*` events. Substrate verifies
signature, issuer, exact audience, time window and binding to the authenticated subject; it never
evaluates the grant — connectors decides, substrate records. Identity-shaped strings a caller may
legitimately write reach the resource and never the attribution; writing one into the envelope is
`request.schema-invalid`, not a quiet drop. Every failure is a named refusal, never a weaker run:
`delegated-context.absent`, `.malformed`, `.unknown-key`, `.signature-invalid`,
`.audience-mismatch`, `.subject-mismatch`, `.expired`, `.grant-conflict`
(`crates/substrate-daemon/src/delegation.rs:209-256`). ADR 0011.

### Serving exec

Without a delegated cgroup root, workspace operations are still served, exec confinement facts are
absent, and exec admission answers `exec.sandbox-unavailable`. A Linux deployment that serves exec
must:

1. place the daemon in a delegated cgroup subtree carrying `cpu`, `memory` and `pids`;
2. keep the delegation root itself **process-free** — for example systemd `Delegate=yes` plus
   `DelegateSubgroup=daemon`;
3. provide the configured bubblewrap binary and `/usr/bin/socat`, which the runtime probe uses to
   prove that the seccomp profile denies host Unix-socket access. **That bubblewrap must accept
   `--disable-userns` and `--assert-userns-disabled`**, because the probe asks for a non-nestable
   user namespace and then makes bubblewrap assert it rather than trusting the option took. A
   backend without those options fails the probe, so exec confinement facts are absent and every
   exec is refused `exec.sandbox-unavailable`, exactly as with a missing delegated cgroup. Check
   with `bwrap --help`; 0.11.2 is the version this was measured against, and no lower bound has
   been established here;
4. pass that root through `--cgroup-root`.

The published daemon image carries the two files item 3 names. Its `daemon` stage
(`Dockerfile:59-72`) builds `FROM execution-prerequisites` (`Dockerfile:48-57`), which installs
the Ubuntu `socat` package as `/usr/bin/socat` (`Dockerfile:51-54`) and copies bubblewrap to
`/usr/bin/bwrap` (`Dockerfile:55`), the default of `--bubblewrap`. That bubblewrap is 0.11.2,
built non-setuid from a checksum-pinned release tarball in the `sandbox-builder` stage
(`Dockerfile:33-46`): the version item 3 was measured against, which accepts the
`--disable-userns` the probe passes (`crates/substrate-host/src/process.rs:64`) and the
`--assert-userns-disabled` it asserts with (`crates/substrate-host/src/probe.rs:630`). The image
supplies nothing else the probe needs, so before the probe advertises exec a deployment of it must
still supply:

1. a delegated cgroup v2 subtree carrying `cpu`, `memory` and `pids`, with the daemon inside it and
   its `cgroup.procs` writable by the image's UID 65532 (`USER 65532:65532`, `Dockerfile:71`);
2. a process-free delegation root: the probe refuses a root whose own `cgroup.procs` is not empty;
3. `--cgroup-root` (or `SUBSTRATE_CGROUP_ROOT`) naming that root, which the image's entrypoint
   (`Dockerfile:72`) does not pass;
4. a container runtime that permits bubblewrap's namespaces: a user namespace, and IPC, PID,
   network and UTS namespaces inside it. A runtime policy that refuses them fails the probe, and
   every exec is refused `exec.sandbox-unavailable`.

The [container execution](#container-execution) bootstrap supplies items 1 to 3 inside its own
private cgroup namespace and refuses a caller's `--cgroup-root`; item 4 remains the deployment's,
through the AppArmor and seccomp profiles that section names. The disposable MCP image's `mcp`
stage (`Dockerfile:74-82`) is distroless `cc-debian12` and carries neither bubblewrap nor socat,
so the daemon child it starts serves no exec: every exec is refused `exec.sandbox-unavailable`.

Each exec's CPU bandwidth is derived from its declared CPU time over its timeout and capped at
`--exec-cpu-cores N` whole cores (`SUBSTRATE_EXEC_CPU_CORES`, default 1). Startup refuses 0, or
more cores than the daemon process may run on, as `config.exec-cpu-cores-invalid`; it never clamps
the value. The ceiling is not published on any capability fact.

The runtime probe enables and tests the controllers, bubblewrap namespaces, cgroup kill and the
swap-inclusive memory bound **before** it advertises exec.

### The TCP transport is development-only

The static-bearer TCP transport is enabled only as an explicitly acknowledged development profile
(`--tcp-development-only --tcp-private-overlay`) and now refuses every non-loopback bind. It requires
a bounded bearer file plus deployment-owned `--tcp-subject` and `--tcp-actor` bindings. The daemon
opens that file once, bounds it to 512 bytes, and admits either an owner-private workload file or a
root-owned, group-readable projected Secret with no group write/execute and no world access.

**This static bearer does not satisfy the accepted scoped, expiring, rotating hosted trust-envelope
profile, and must not be published through external or shared ingress.** A hosted container without
a delegated cgroup or bubblewrap environment continues to report execution sandbox unavailability
rather than weakening confinement.

### Production TLS transport

Current source can bind a distinct TLS 1.3 HTTPS/WSS listener:

```console
substrate-daemon \
  --socket /run/substrate/local.sock \
  --state /var/lib/substrate/state.sqlite \
  --workspaces /var/lib/substrate/workspaces \
  --deployment edge-01 \
  --tls-listen 0.0.0.0:8443 \
  --tls-certificate-chain /run/substrate-tls/chain.pem \
  --tls-private-key /run/substrate-tls/key.pem \
  --hosted-identity-origin https://identity.example.com \
  --hosted-identity-ca-bundle /run/substrate-identity/ca.pem
```

The certificate and key paths must be non-empty regular files rather than symlinks. The key must
belong to the daemon's effective user and have no group or other permission bits. The daemon checks
certificate validity and certificate/key agreement before binding, negotiates only TLS 1.3 and
HTTP/1.1, and never trusts `Forwarded`, `X-Forwarded-*`, or caller-written identity headers.

Replace both files completely and send SIGHUP to rotate them. A complete valid pair becomes the
snapshot for new connections; existing connections retain their admitted snapshot. An invalid
replacement is logged only as `tls.reload-invalid`, and the last valid identity keeps serving.

The production listener also authenticates every caller by resolving an opaque five-minute
`identity_access_v1_…` bearer at Identity's `GET /v1/access-authority` endpoint. Resolution uses the
exact `urn:b10x:substrate` audience, the explicit CA roots above, direct HTTPS with no redirects or
proxy, a five-second deadline and a 64 KiB response bound. `observe`, `workspaces` and `exec` are
checked against the addressed route before any handler can reserve a durable operation. Missing,
invalid, under-scoped and temporarily unresolvable authority answers `auth.credential-absent`,
`auth.authority-invalid`, `auth.scope-denied` or `auth.authority-unavailable`; there is no cached or
caller-written fallback. There is no production plaintext fallback and no verification-disable
flag. Every request resolves online, at the cost of one HTTP round trip on a reused keep-alive
connection: at most 8 idle connections, each closed after 30 seconds idle or after any error,
refusal or close-requesting response (ADR 0040).

The Rust SDK addresses this listener only when the caller supplies the exact HTTPS origin, PEM
trust roots, expected DNS identity and an asynchronous Identity access-token provider. It uses the
same TLS 1.3 configuration for HTTP and WSS, refreshes once only after a named authentication 401,
and mints a fresh one-use attachment authority for every hosted session connection. See the
[public Rust SDK guide](https://beyond10x.github.io/substrate/docs/guides/rust-sdk/#connect-to-a-remote-daemon)
for a complete builder example.

## What is enforced

| area | enforced |
|---|---|
| filesystem | `openat2` beneath / no-link / no-mount I/O, atomic replacement, symlink escape refusal |
| process | cleared and shaped environment, namespace no-egress, `pids` and memory-plus-swap bounds, cumulatively observed CPU bounds, timeout, whole-tree kill |
| capsules | exact capsule-byte verification, read-only `/runtime`, separate writable `/workspace`, bounded normal and restart cleanup |
| output | both stdout and stderr drained continuously while a process runs, bounded captures retained, persisted when the exec is observed, ranged reads exposed |
| durability | terminal observations and output stay in memory until the durable store acknowledges them; maintenance cannot regress a durable terminal state |
| concurrency | blocking filesystem and SQLite work runs in separate bounded lanes, so saturation backpressures callers without starving asynchronous service |

Substrate reports the applied capsule identity. It does **not** claim the host interpreter,
libraries or base system as part of that closure.

Git materialization is conditional on a configured HTTPS source and transient Connector authority.
It checks out the exact provider commit and exposes bounded baseline and change reads. General
workspace backup/restore snapshots remain absent.

## Layout

| crate | owns |
|---|---|
| `crates/substrate-wire` | the closed Rust representation of the wire; **subordinate to the contract bundle**, never the other way round |
| `crates/substrate-store` | durable operation and resource state |
| `crates/substrate-host` | the Linux host driver |
| `crates/substrate-guarded` | guarded workspace file access (`openat2` beneath a pinned root), the driver error type and the exec environment rule, shared by every driver and by a binary that must not link the host driver |
| `crates/substrate-kubernetes-client` | the bounded Kubernetes API client the `pod` confinement adapter and the namespace driver share: kubeconfig loading, HTTPS requests, watches, `pods/exec` and the gone-Pod rule |
| `crates/substrate-kubernetes` | the Kubernetes namespace driver (ADR 0047): its settings, object names and labels, the ownership rule, the namespace `Lease`, the startup refusals and the snapshot binding |
| `crates/substrate-daemon` | the standalone HTTP daemon: `DaemonConfig` plus the async `serve` entrypoint |
| `crates/substrate-contract-check` | the offline contract checker |
| `crates/b10x-substrate-sdk` | the typed Rust client over the daemon's authenticated service contract, with an opt-in linked-daemon mode |
| `crates/substrate-mcp` | the private SDK-only stdio MCP adapter: bounded JSONL over a closed tool and resource vocabulary |

| path | holds |
|---|---|
| [`contracts/substrate-wire/`](contracts/substrate-wire/) | the canonical wire bundles, one directory per version; earlier bundles are immutable |
| [`architecture/`](architecture/) | the accepted system boundary and dependency direction |
| [`docs/design/`](docs/design/) | wire, driver, lifecycle, security, session and trust design; each document states whether it is accepted or under review |
| [`docs/plan/`](docs/plan/) | design turned into review gates and implementation slices, without implementation |
| [`.engineering/planning/`](.engineering/planning/) | the plan: epics and stories as governed artifacts, read with `aep plan artifact list` / `board` |
| [`adr/`](adr/) | accepted component decisions, with YAML frontmatter |
| [`scripts/`](scripts/) | `gate.sh` and the checks it runs |
| [`xtask/`](xtask/) | the workspace member holding the gate's own `cargo xtask` verbs and the bundle renderer |

## Read more

Start here, in order:

1. [Vision](docs/VISION.md)
2. [Architecture overview](architecture/overview.md)
3. [Domain model](architecture/domain-model.md)
4. [Stack integration](architecture/stack-integration.md)
5. [API contract](docs/design/01-contract.md)
6. [Specification bundle and minimum wire](docs/design/07-specification-and-conformance.md)
7. [Roadmap](ROADMAP.md)

Also: [`glossary.md`](glossary.md), [`STATUS.md`](STATUS.md), [`CHANGELOG.md`](CHANGELOG.md), and
[`AGENTS.md`](AGENTS.md) for the working agreements and invariants.

## Documentation

[Substrate project site](https://beyond10x.github.io/substrate/) · [Public handbook](https://beyond10x.github.io/substrate/docs/) · [Executable specification](spec/README.md)
