---
format: aep.planning-md/3
id: story:exec-start-carries-executable-lists
kind: story
status: draft
title: An exec start carries the executable lists on the wire
summary: 'A successor bundle at the next free version: start fields for the allow-list and deny-list, the applied record and named refusals, so a run keeps its lists on a remote daemon.'
owner: substrate
tags:
- confinement
- sdc
- wire
relations:
- decomposes: epic:runs-are-governed-and-limit-hits-are-events
- depends_on: story:executable-deny-list
- depends_on: story:process-count-limit-hit-is-observed
- depends_on: story:exec-file-changes-are-recorded
scope:
- confidence: inferred
  path: AGENTS.md
- confidence: inferred
  path: CHANGELOG.md
- confidence: inferred
  path: README.md
- confidence: inferred
  path: contracts/substrate-wire
- confidence: inferred
  path: crates/b10x-substrate-sdk/src
- confidence: inferred
  path: crates/substrate-daemon/src/app/operations.rs
- confidence: inferred
  path: crates/substrate-daemon/tests/runtime_vectors.rs
- confidence: cited
  path: crates/substrate-host/src/probe.rs
- confidence: inferred
  path: crates/substrate-host/src/process.rs
- confidence: inferred
  path: crates/substrate-kubernetes
- confidence: cited
  path: crates/substrate-wire/src/lib.rs
- confidence: inferred
  path: docs/design/37-executable-lists-on-the-wire.md
- confidence: inferred
  path: scripts/gate.sh
- confidence: inferred
  path: website/docs
- confidence: inferred
  path: xtask/bundle-source
- confidence: inferred
  path: xtask/src/bundle
- confidence: inferred
  path: xtask/src/bundle.rs
revision: 11
---
# Story: An exec start carries the executable lists on the wire

## Outcome

A client of the daemon names an executable allow-list and deny-list on an exec start and on a
session start. A daemon whose host proved the facts applies the lists, echoes them in the applied
record and publishes both properties. A daemon whose host did not refuses the start by name before
dispatch.

## Why

- The wire publishes `confinement.exec-subpaths` as a property a start may require
  (`crates/substrate-wire/src/lib.rs:3612-3613`, `:3696`, `:3723`). No start field carries the
  list: `subpath` appears in `crates/substrate-wire/src/lib.rs` only at those property lines.
- After `story:linux-executable-allow-list`, `publish_confinement` withholds
  `confinement.exec-subpaths` although the `linux` adapter proves it, because the start has no
  field (`crates/substrate-host/src/probe.rs:269`).
- The relayed requirement of 2026-11-21 says runs move remote
  (`epic:runs-are-governed-and-limit-hits-are-events`). A run that moves from the in-process
  `linux` adapter to a daemon loses its executable lists without this story.

## What changes

1. **A design document first** (`AGENTS.md` invariant 8),
   `docs/design/37-executable-lists-on-the-wire.md`: the fields on both starts and their bounds,
   the applied record, each named refusal with its status, the deny-list property in
   `ConfinementProperty`, and consumer compatibility. A start without the fields serializes and
   hashes as on the predecessor bundle.
2. A successor bundle under `contracts/substrate-wire`, at the next free version when the story
   starts (`main` advertises `substrate-wire/0.27.0` at `638a7d70c`,
   `crates/substrate-wire/src/lib.rs:18`, so `0.28.0` or later). It is cut with
   `cargo xtask render-bundle` from its `xtask/bundle-source` directory, and its check is added to
   `scripts/gate.sh` (invariant 6). Each new constant is bound from `xtask/src/bundle.rs`, not
   from `xtask/src/render.rs`.
3. The host driver applies the lists through the mechanism `story:linux-executable-allow-list` and
   `story:executable-deny-list` built.
4. `publish_confinement` publishes `confinement.exec-subpaths` and the deny-list property from
   `linux::proof`, and the exception in `the_published_set_is_the_linux_adapters_proof`
   (`crates/substrate-host/src/probe.rs:1146-1218`) is removed.
5. The daemon and the Rust SDK advertise the new bundle.

## Acceptance

In the delegated lane of `crates/substrate-daemon/tests/runtime_vectors.rs`, an exec start and a
session start that each name an allow-list and a deny-list run, a denied execution fails inside
each, and each applied record echoes both lists; on `0.26.0` neither start has the fields.

Checks:

- The design document is merged before any code of this story.
- Delegated lane: the machine publishes both properties, and
  `the_published_set_is_the_linux_adapters_proof` holds the published set equal to the adapter's
  proof with no exception.
- Portable lane: both properties are absent, and an exec start and a session start naming either
  list are refused by the published refusal before dispatch and recorded as refused.
- A unit test on the Kubernetes namespace driver's snapshot holds both properties absent. The
  refusal of the fields there is the daemon's driver-independent one the portable lane asserts.
- `cargo xtask check-bundles` from `0.5.0` through the new version and `cargo xtask check-json`
  pass in `scripts/gate.sh`; every earlier bundle is byte-unchanged.
- `PORTABLE_CASES` and `DELEGATED_CASES` and the counts `AGENTS.md` states are updated in the same
  change.

## Boundaries

- No Rust SDK builder setter and no CLI flag for the lists; each is its own story once the bundle
  exists.
- No enforcement on the Kubernetes namespace driver.

## Order

After `story:executable-deny-list`: it carries that story's field and fact. After
`story:process-count-limit-hit-is-observed` only because both change
`crates/substrate-host/src/process.rs`. After `story:exec-file-changes-are-recorded` only because
both change `crates/substrate-host/src/process.rs` and `crates/substrate-host/src/probe.rs`.
Independent of `story:executable-lists-on-oci-and-pod`:
the two share no file and may be worked at the same time.
`story:exec-observation-reports-process-count-hits` follows this story and cuts the bundle after
this one's.

## Scope

- `docs/design/37-executable-lists-on-the-wire.md` (new). Inferred.
- `crates/substrate-wire/src/lib.rs`. Cited: `:3612-3613`, `:3690-3723`.
- `crates/substrate-host/src/probe.rs`. Cited: `:269`, `:1146-1218`.
- `xtask/bundle-source`, `contracts/substrate-wire` (one new version directory in each),
  `xtask/src/bundle.rs`, `xtask/src/bundle` (a new module), `scripts/gate.sh`. Inferred from
  `AGENTS.md` § The gate.
- `crates/substrate-daemon/src/app/operations.rs`,
  `crates/substrate-daemon/tests/runtime_vectors.rs`, `crates/substrate-kubernetes`,
  `crates/b10x-substrate-sdk/src`. Inferred.
- `crates/substrate-host/src/process.rs`. Inferred.
- `website/docs`. Inferred: `cargo xtask check-docs` holds the site's contract tables to the spec.
- `AGENTS.md`, `README.md`, `CHANGELOG.md`. Inferred.
