---
format: aep.planning-md/3
id: story:read-only-mount-sources-are-declared-by-the-operator
kind: story
status: draft
title: Read-only mount sources are declared by the operator and granted per caller
summary: A caller names a declared source; the daemon resolves it beneath operator-owned roots
tags:
- security
relations:
- depends_on: story:read-only-mount-sources-refuse-the-daemons-private-paths
revision: 2
---
# Story: read-only mount sources are declared by the operator and granted per caller

## Outcome

A caller names a source the operator declared, as a writable root is declared today (ADR 0038).
The daemon resolves the name beneath operator-owned roots and admits it only for a caller it is
granted to. A host path the operator did not declare is refused by name before dispatch.

## Design

Owed before code: an ADR that amends ADR 0010, which makes the source caller-selected, and
ADR 0033. It is a change of contract and capability: the start fields, the facts
`exec.read-only-roots` and `exec.read-only-files`, the refusals, a successor bundle, and what
consumers that pass host paths today do instead. The ADR names those consumers.

## Open

- Whether the local posture, where the caller is the daemon's own uid, keeps caller-selected
  paths. The ADR decides.
- This is a change of architecture and waits for the operator's word on the ADR.

## Acceptance

An approved toolchain source mounts for a caller it is granted to; an undeclared path and a
declared source not granted to the caller are each refused before dispatch, by name.


## Where the requirement comes from

The operator's private security review of `main` at `1af046902`, dated 2026-11-22,
finding 1 (high priority), the durable fix. The operator ordered its findings handled on 2026-11-22. By
`approval-record:operator-decides-how-security-review-fixes-travel-2026-11-22` this story
states the rule that must hold; the review's detail stays with the operator until the fix is
on `main`. Checked against the source at `1af046902` by reading; nothing was run. Release
0.23.0 has the same source. Stakeholder: the operator.

## What consumers name today

### Loom (its own statement, 2026-11-22, integration head `1ee45331`, read from code, nothing run)

1. Bound program binary: `at_host_path(argv[0])`, absolute and outside `/usr /bin /lib /lib64`, not
   `local` (`crates/loom-intake-slice/src/bindings.rs:441-446`). Source: the bindings document's author.
2. Retry feedback file (`crates/loom-bindings/src/retry.rs:206`): written by Loom in the run's store
   area; the directory was not verified by Loom's session.
3. `ConfinementSpec` roots, files and host paths (`crates/loom-intake-slice/src/confinement/spec.rs:63-73`):
   the embedder's value; no Loom caller builds it.
4. Rust toolchain root (`crates/loom-intake-slice/src/confinement.rs:3282-3298`): two levels above
   the `rustc` path `rustup` reports, accepted only under `$RUSTUP_HOME/toolchains`; mounted at
   `RUST_MOUNT`. Source: the process environment.
5. Cargo cache directories (`:3305-3335`): `$CARGO_HOME` joined with three names in `CACHE_DIRS`,
   skipped when absent, links rejected; mounted under `CACHE_MOUNT`. Source: the process environment.
6. Planned, not built (`per-run-program-path`): up to 4 directories the embedder names per run.

Loom's reading: 1, 3 and 6 are caller-selected from documents or settings; 4 and 5 are derived from
`RUSTUP_HOME`, `CARGO_HOME`, `HOME` and `rustup` and would need a declared form (a toolchain and a
cache root in Loom's planned engine configuration document, its wave 6); 2 is Loom's own file.
Not read by Loom: whether a `local` run skips 4 and 5.

### Runbook (its own statement, 2026-11-22, tree at `b406369`)

No call into Substrate today. Planned: through Loom's `program_path`, one tools directory per run
that the embedding daemon owns, for example `<state>/presets/<runbook>/<version>/tools`, outside
every workspace, no key, no run record, named by several runs at once. Nothing else.

### Harness (read by a Substrate agent, not confirmed by Harness)

Provider toolchain roots and a staged driver directory under the temporary directory
(`crates/harness-substrate/src/toolchain.rs:97-100`, `:197-200`).
