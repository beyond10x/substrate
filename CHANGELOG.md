# Changelog

All notable changes to Substrate are documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project uses
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.4] — 2026-08-31

### Changed

- **Substrate is Apache-2.0.** Atlas ADR 0010 grants Apache-2.0 across all beyond10x-owned
  Substrate history without rewriting a frozen contract byte. The repository, public site and
  daemon image carry the licence and public security-reporting path; third-party material retains
  its own licence.
- **The public distribution is fail-closed.** GitHub secret and dependency protections, private
  vulnerability reporting, action SHA pinning, locked audit-clean website dependencies and
  deterministic third-party notices join the existing full-history and RustSec gates.

### Added

- **Contract bundle [`contracts/substrate-wire/0.9.0`](contracts/substrate-wire/0.9.0)**, an
  additive successor to `0.8.0`: `adds_routes: 5`, `preserves_routes: 26`. One operation registry
  now declares every served API major, the five existing v2 workspace byte-plane routes carry v2
  schemas and envelopes, and the preserved v1 file routes declare the catch-all path the daemon
  actually serves. Accepted as [ADR 0018](adr/0018-one-registry-declares-every-served-api-major.md).
- **Confinement and observation hardening.** Declared host roots cannot expose host IPC; delegated
  context is authenticated before replay lookup; reads stay bounded even when metadata is stale;
  aperture setup preserves its failing stage and errno; seccomp denies Unix sockets and
  `io_uring_setup`; aperture state is reconciled fail-closed; and live pipe-output backpressure is
  terminal rather than silently lossy. Accepted as [ADRs 0015–0017](adr/README.md).
- **Repository and release gates are fail-closed.** The gate now scans every reachable Git object
  for secrets, checks RustSec advisories and forbidden HTTP/2 dependencies, keeps bot credential
  validation in Rust, and checks all nine contract bundles. The release workflow tests the exact
  daemon binary extracted from the image before the one permitted push, refuses an existing tag
  image or release, and publishes its GitHub release only after signature verification and an
  anonymous image pull. Docker ignores local build and website dependency trees, reducing the
  measured review build context from 6.49 GB to 17 MB.
- **An egress aperture can carry a declared byte ceiling, and crossing it refuses the run by name.**
  `--egress-aperture <name>=<host>:<port>/tcp[/max=<size>]` bounds one run's
  `to_destination + from_destination` over that aperture; `<size>` is a decimal byte count with an
  optional binary suffix (`1048576`, `512KiB`, `64MiB`, `2GiB`) and never a decimal-power unit, and
  an unrecognised term is a startup error rather than an ignored one. The relay stops relaying at
  the ceiling, so the overshoot is at most one 16 KiB relay buffer per live relay; the parent's
  existing 1 ms supervision loop reads the same counters, ends the run and names the refusal.
  Accepted as [ADR 0014](adr/0014-apertures-carry-a-declared-byte-ceiling.md).
- **An exec observation can name the bound that ended it.** One optional `refusal` member carrying
  `class`, `code` and `message` beside the state that is already `cancelled`. The declared aperture
  byte ceiling — `exhausted` / `exec.aperture-byte-limit` — is its only user; a timeout and a CPU
  budget are unchanged.
- **A ceiling is deployment vocabulary, never request data.** A request carrying one is refused
  `exec.aperture-ceiling-in-request` at `sandbox.network.aperture`, told apart from
  `exec.aperture-destination-in-request` so a rejected escalation does not read as a schema typo.
- **Published and observed.** The `exec.egress-apertures` capability fact and the applied-aperture
  observation each gain an optional `max_bytes`, so `/v1/machine` answers how much this daemon could
  ever pass and a run states the ceiling it actually ran under beside the bytes that crossed.
- **Contract bundle [`contracts/substrate-wire/0.8.0`](contracts/substrate-wire/0.8.0)**, 228 files,
  an additive successor to `0.7.0`: `adds_routes: 0`, `preserves_routes: 26`. Every earlier bundle
  directory keeps its bytes, and `cargo xtask check-bundle 0.8.0` is in
  [`scripts/gate.sh`](scripts/gate.sh).

- **A pty is a second session mode, not a second session resource.** `POST /v1/pipe-sessions` takes
  `mode: "pty"` beside the `pipes` it already served, with a required initial `window` of 1–1000
  cells on each axis. The route family, the operation ids, the `ses_…` identity, the lease, the
  single attachment and the whole-tree cleanup are the ones a raw-pipe session already had; a
  `resize` frame joins the closed client vocabulary, bounded by the same cell rule and rated on the
  control window that already existed. There is no `close-input` on a terminal — a client ends
  input by sending the terminal's own end-of-file character as ordinary input bytes — and no
  `truncated`: reaching the declared output bound ends the session and names
  `session.output-limit` through the refusal field [ADR 0014](adr/0014-apertures-carry-a-declared-byte-ceiling.md)
  added. Decided in [ADR 0019](adr/0019-pty-is-a-second-session-mode.md).
- **`sessions.pty` is published only after a verified allocation.** A startup probe allocates a
  pair, makes it controlling inside a throwaway sandbox *after* bubblewrap's own `setsid`, and
  round-trips a window through the child's `TIOCGWINSZ` before and after a resize. Absent, every
  `mode: "pty"` request is refused `session.pty-unserved` (501, `unserved`) and **never** served as
  pipes; an allocation failure at start is `session.pty-exhausted` (429, `exhausted`, retriable).
  `--new-session` is not dropped to make a terminal work: that would weaken the confinement floor of
  every non-pty exec to serve one feature.
- **Session refusals are closed and actionable.** The bundle register names all 32 refusal codes,
  their arrival surface, class, status and retry fact; protocol-error frames use a closed typed
  vocabulary. The attachment-capacity path now reads the same per-code retry table as the register,
  so its HTTP response cannot contradict the published `retriable: false` decision.
- **Contract bundle [`contracts/substrate-wire/0.10.0`](contracts/substrate-wire/0.10.0)**, 251 files, an
  additive successor to `0.9.0`: `adds_routes: 0`, `preserves_routes: 31`. It adds the `mode` and
  `window` start fields, the `sessions.pty` fact, the served-modes capability document and
  `schemas/pty-channel-frame.json`. Every earlier bundle directory keeps its bytes, and
  `cargo xtask check-bundle 0.10.0` is in [`scripts/gate.sh`](scripts/gate.sh).

### Unchanged

- An aperture declared without the term behaves byte for byte as it did: no ceiling in the relay,
  no `max_bytes` on the fact or the observation, and no `refusal` on the run.
- A session start that names no `mode` is a raw-pipe session, byte for byte as before: the field
  defaults to `pipes`, so no existing client can be handed a terminal, and a `0.4.0` daemon refuses
  `mode: "pty"` as schema-invalid rather than quietly serving pipes.

## [0.2.3] — 2026-08-30

Image: `ghcr.io/beyond10x/b10x-substrate-daemon:0.2.3` at `sha256:ab10158266b579d705ce8422c7d2a6e783cde950d30e100f61ca6befc4d0beda`, keyless-signed; verify with `cosign verify`.

Three accepted ADRs implemented, and the gate's last Python removed. Additive throughout: no route
is removed or changed, and every earlier contract bundle keeps its bytes.

### Added

- **A tagged `main` publishes a signed, digest-pinned daemon image.**
  [`.github/workflows/release.yml`](.github/workflows/release.yml) triggers only on an annotated
  bare-version tag, refuses to build unless [`.github/workflows/gate.yml`](.github/workflows/gate.yml)
  has concluded `success` for that exact commit, builds the `Dockerfile` with `SOURCE_SHA` set to the
  tag's commit, pushes `ghcr.io/beyond10x/b10x-substrate-daemon:<version>`, signs it keylessly and
  runs `cosign verify` against the workflow identity **before** anything is announced. Only then does
  it create the GitHub release and write the digest under that version's heading here. A pre-release
  tag such as `0.2.2-rc.0` publishes nothing. `permissions` are `contents: read` at workflow level,
  with `packages: write` and `id-token: write` on the release job alone; every action is SHA-pinned;
  the release and the changelog commit are made by the b10x-bot App. No image is published yet — the
  workflow has never run. It needs no repository secret: the image push, the GitHub release and the
  changelog commit all use the run's own `GITHUB_TOKEN`, which cannot leave this repository.
  A signed daemon image does not make any wire contract bundle stable; the bundles under
  `contracts/substrate-wire/` remain development bundles.

- **An operation's ledger row carries the declared grant it ran under.** A start may present a
  signed delegated-context document; substrate verifies it and records `grant_ref` and
  `platform_principal` on the operation's ledger row and its `operation.*` events, so a reader can
  answer *which declared grant authorised this, on behalf of which platform principal* from
  substrate's own durable record. Accepted as
  [ADR 0011](adr/0011-delegated-context-and-grant-attribution.md).
- **Caller-written identity is never attribution.** Identity-shaped strings a caller may legitimately
  write — workspace labels are free-form and echoed verbatim — reach the resource and never the two
  attribution columns. Writing one into the request envelope is refused `request.schema-invalid`,
  not ignored quietly: the request union stays closed around `op`, `input` and the one signed
  member. Substrate verifies signature, issuer, exact audience, time window and binding to the
  authenticated subject, and it never evaluates the grant — connectors decides, substrate records.
- **The field is optional everywhere and costs a `0.6.0` client nothing.** `delegated_context` is a
  sibling of `op` and `input`, never a member of `input`, so it stays outside the canonical request
  hash: replaying the same `op` with a *fresh* context is the same operation and returns the
  original outcome. Absent, the serialized bytes are exactly what a `0.6.0` client sent.
- **Which service signs is configuration, not code.** A deployment declares
  `--delegated-context-key <kid>=<issuer>=<base64url>`; substrate holds a verifying key and never a
  signing key. Identity and connectors are both named relying parties (ADR 0011).
- **Contract bundle `contracts/substrate-wire/0.7.0`**, an additive successor to `0.6.0` carrying
  the optional `delegated_context` request member, the `grant_ref` and `platform_principal`
  ledger/event fields and the eight named refusals — `delegated-context.absent`, `.malformed`,
  `.unknown-key`, `.signature-invalid`, `.audience-mismatch`, `.subject-mismatch`, `.expired` and
  `.grant-conflict`. 224 files; every earlier bundle directory unchanged.

- **Destination-bound egress apertures.** A confined run can now reach exactly one
  operator-declared destination and nothing else. `--egress-aperture <name>=<host>:<port>/tcp`
  (repeatable) declares one; a start selects it **by name** — `sandbox.network: "aperture"` plus
  `sandbox.aperture: "<name>"` — and may never carry a destination, at any depth, in any field.
  Accepted as [ADR 0013](adr/0013-egress-apertures-are-declared-by-the-operator.md); the mechanism
  is the one [design 10a](docs/design/10a-egress-mechanism-spike.md) proved.
- **The mechanism keeps the kernel floor literally intact.** The sandbox still runs under
  `--unshare-net` with loopback and no other interface, no route and no resolver. What an aperture
  adds is one listening socket **inside that namespace**, created by a short-lived helper that joins
  the namespace bubblewrap created (through `ioctl(netns_fd, NS_GET_USERNS)`, and against the
  `child-pid` bubblewrap reports on `--info-fd` — not the bubblewrap pid, which lives in the host
  namespace) and handed back over `SCM_RIGHTS`. A per-run forwarder in the run's own cgroup dials
  the pinned address from the host namespace, one relay process per connection, and dies with the
  run. Everything else is still `ENETUNREACH`.
- **The capability fact `exec.egress-apertures`**, the declared names and their pinned destinations,
  published only after the mechanism was exercised in a throwaway sandbox at startup — never after
  reading configuration, and never by dialling a declared destination, which would make readiness
  somebody else's uptime. Absent otherwise: a start naming an aperture is then `unserved`.
- **The applied aperture is an observation**, not an echo: `applied.network` becomes
  `{mode, name, destination, mechanism, bytes}` on the aperture branch, where `destination` is the
  address the forwarder dialled and `bytes` is counted in the forwarder, the only thing that sees
  them. A run with no aperture still reports `"none"`.
- **DNS stays outside the aperture.** The daemon resolves each declared host once, at declaration,
  and pins the address for the process's lifetime. A run with an aperture is given a generated
  read-only `/etc/hosts` holding exactly the declared name mapped to loopback — the whole of `/etc`
  it ever gets — so a child uses the URL the operator declared and the forwarder is what answers.
- **`--ca-bundle <path>`**, optional. TLS crosses the aperture byte for byte, but a sandbox has no
  trust anchor because it has no `/etc`; where one is configured, each run gets a private read-only
  **snapshot** of it, so rotating the source cannot change what a running child already trusts.
  Absent and unverifiable, never present and unverified.
- **Contract bundle `contracts/substrate-wire/0.6.0`**, an additive successor to `0.5.0`
  (`preserves_routes: 26`, `adds_routes: 0`) carrying the `aperture` start field on exec and
  pipe-session start, the `exec.egress-apertures` fact, the applied-network object and seven
  conformance vectors. `cargo xtask check-bundle 0.6.0` is in the gate.

### Changed

- `network: "aperture"` **with a declared name** is now served where the mechanism verified. Without
  a name it answers exactly as before — `exec.network-unserved`, the refusal
  `contracts/substrate-wire/0.4.0/vectors/http/egress-unserved.json` froze — so no earlier vector
  changes shape.
- `AppliedNetwork` gains a variant, which breaks exhaustive matches in Rust consumers.
- The capability snapshot's configuration generation now covers declared apertures **whole**, name
  and destination. Unlike a secret slot, what is behind an aperture is exactly what a client is
  told, so changing it must invalidate every snapshot.

- **Sealed secret slots.** A confined process can now be handed an operator-declared credential
  without the value existing in any byte substrate can emit. `--secret-slot <name>=<path>` declares
  one; a start names `secret_slots: [{"slot": …, "fd": …}]` and the driver copies the declared bytes
  into an anonymous `memfd`, seals it with exactly `F_SEAL_WRITE|F_SEAL_SHRINK|F_SEAL_GROW|
  F_SEAL_SEAL`, verifies the read-back, places it at the declared descriptor with `dup2` in
  `pre_exec`, and closes its own copy the moment the child exists. The child finds the mapping — names
  and descriptors only — in `SUBSTRATE_SECRET_SLOTS`. Accepted as
  [ADR 0012](adr/0012-secret-slots-are-sealed-memfds.md).
- **The capability fact `secrets.slots`**, the sorted list of declared slot names, published only
  from a probe that proved sealing in-process and descriptor pass-through through a real bubblewrap
  child. Absent otherwise: a start naming a slot is then `unserved`, never delivered by weaker means.
- **The delegated clean-room lane proves the slot guarantee on the wire**, against the shipped
  binary rather than in process. A confined child reads its slot from the declared descriptor and
  returns a SHA-256 of the bytes; it reports `F_GET_SEALS` as the declared set, a write refused with
  `EPERM`, and a descriptor table of exactly `{0,1,2}` plus its slot; the value is found in no
  captured argv, shaped environment, stdout, stderr, event page, ledger row, applied record, refusal
  body or daemon diagnostic; and `/proc/<daemon>/fd` holds no slot `memfd` while the child is still
  running and has not yet read. `bash scripts/delegated-lane.sh` now counts 48 cases and the
  portable lane 31, where it asserts `exec.secret-slots-unserved` and
  `exec.secret-slot-descriptor-invalid`.
- **Contract bundle `contracts/substrate-wire/0.5.0`**, an additive successor to `0.4.0`
  (`preserves_routes: 26`, `adds_routes: 0`) carrying `secret_slots` on exec and pipe-session start,
  the `secrets.slots` fact, the applied-confinement slot record and six conformance vectors.
- **`cargo xtask check-bundle <version>`**, in the gate. It verifies a released bundle as the fixed
  point of its authored source, its manifest, its compatibility counts against both route
  inventories, and every JSON document's schema classification.

### Changed

- The `pre_exec` descriptor closure is one `close_range` per gap in the retained set rather than two
  fixed windows. With no slot declared the retained set is `{0,1,2,barrier}` and the gaps are exactly
  the two former windows, so behaviour without slots is unchanged.
- The capability snapshot's configuration generation now covers declared slot **names**. Adding or
  removing a slot moves the snapshot; rotating the material behind one moves nothing observable, so
  an operator can rotate a credential without invalidating an admitted operation.
- **The `secrets.slots` probe now observes the two properties it publishes a fact about.** Design 11
  § 5 requires a bubblewrap child reporting the probe descriptor at its declared number *with the
  same seals and nothing else above 2*; the probe checked neither, accepting any child whose stdout
  was the sentinel followed by `sealed` — so a descriptor sealed `F_SEAL_WRITE` alone, or one handed
  over beside leaked descriptors, published the capability. The child now reports the inode behind
  its declared number, the memfd's link, and every descriptor it holds; the parent compares the
  inode against the memory it staged, the seal word `F_GET_SEALS` reads off that inode against
  `F_SEAL_WRITE|F_SEAL_SHRINK|F_SEAL_GROW|F_SEAL_SEAL`, and the descriptor set against `{0,1,2}`
  plus the declared number. Any disagreement withholds `secrets.slots` (invariant 3), and nothing is
  compared against a substring of the child's output any more.

## [0.2.2] — 2026-08-30

One behavioural change, in what a workspace root may be called.

### Changed

- **A workspace root may be a directory the operator already owns.** A root name no longer has to
  carry the `ws_` prefix, only to be a single path component; containment was always `openat2`
  beneath the pinned root descriptor with symlinks refused, never the prefix. A harness can now run
  against your actual checkout — `harness`, `engineering-protocols` — instead of only a `ws_`-named
  scratch copy. Workspaces the server creates are still minted as `ws_…`.

## [0.2.1] — 2026-08-29

Documentation and distribution release. This release changes no runtime behavior, route, schema,
wire identifier, capability, or contract-bundle byte.

### Added

- A self-contained public Docusaurus site with a project-specific landing page, eight reader-facing
  documentation pages, responsive themes, a Substrate mark, and strict broken-link and
  broken-anchor gates.
- A GitHub Pages workflow that installs from the npm lockfile, type-checks, builds, uploads only
  `website/build`, and deploys the resulting artifact from `main`.
- A public-website working agreement that requires the Atlas `website-docs` skill and keeps
  internal designs, ADRs, plans, reviews, work logs, contributor status material, and source links
  out of the reader-facing site.

### Changed

- The repository is public under Atlas ADR 0003 after a full-history and working-tree credential
  scan found no leaks. Public visibility does not change the proprietary licence and does not make
  the development contract bundles stable.
- The workspace crate version is `0.2.1`.

## [0.2.0] — 2026-08-24

First release from the standalone repository. Substrate was extracted from the b10x monorepo
at `e01ea676` with full history on 2026-08-23; everything before the extraction is recorded in the
monorepo's own ledgers, and the version continues from the manifest it arrived with.

### Added

- **Declared host roots are mounted read-only** (ADR 0010). A root the operator names is bound
  `--ro-bind` into the confined tree, so a run can read a declared host tree and can never write
  through it.

### Changed

- The shared cargo cache mounts are `sharing=locked`, so parallel confined builds cannot corrupt
  one cache.
- This repository is the canonical Substrate home, with its own gate (`bash scripts/gate.sh`); the
  surface speaks as b10x and a fence keeps it that way, and links that escape the repository are
  pinned to the extraction baseline.
