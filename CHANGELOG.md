# Changelog

All notable changes to Substrate are documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project uses
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

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
