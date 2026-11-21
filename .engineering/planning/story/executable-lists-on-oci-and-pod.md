---
format: aep.planning-md/3
id: story:executable-lists-on-oci-and-pod
kind: story
status: draft
title: The oci and pod adapters' answer to the executable lists is observed and held by a scenario
summary: Design 35 records the mechanism tried on each container adapter's lane, the suite holds a refusal scenario per adapter, and a draft enforcement story is filed where a mechanism worked.
owner: substrate
tags:
- confinement
- sdc
relations:
- decomposes: epic:runs-are-governed-and-limit-hits-are-events
- depends_on: story:executable-deny-list
- depends_on: story:process-count-limit-hit-is-observed
scope:
- confidence: cited
  path: crates/substrate-host/tests/ess_confinement_oci.rs
- confidence: cited
  path: crates/substrate-host/tests/ess_confinement_pod.rs
- confidence: inferred
  path: docs/design/35-executable-lists.md
- confidence: inferred
  path: spec/confinement-suite.json
- confidence: cited
  path: spec/confinement/confinement.yaml
- confidence: inferred
  path: spec/confinement/scenarios
- confidence: inferred
  path: website/docs
revision: 9
---
# Story: The `oci` and `pod` adapters' answer to the executable lists is observed and held by a scenario

## Outcome

For `oci` and for `pod`, the design document records what happened when a candidate mechanism for
the executable lists was tried on that adapter's lane, and the committed confinement suite holds
one scenario per adapter for a profile that names both lists. Today no document states either
adapter's answer and no scenario holds it. Enforcement on an adapter where a mechanism was observed
to work is a follow-up story this one files.

## Why

- `oci` proves `image_digest_pinned`, `stdin_pipe` and `output_live` and no executable fact
  (`crates/substrate-host/src/confinement/oci.rs:419-421`). `pod` proves `image_digest_pinned`
  only (`crates/substrate-host/src/confinement/pod.rs:205`).
- `pod` is the in-process adapter that runs on another machine. A consumer that moves a run there
  needs to know whether the executable lists move with it.
- Inside a container the candidate mechanisms differ from the host's: a Landlock rule applied by
  the executor before the program starts, or a `noexec` mount. Which of them a pinned runtime and
  a Pod's security context admit is not known. Inferred, not verified.
- No scenario under `spec/confinement/scenarios` opens `oci` or `pod` with `exec_subpaths`; the
  refusal such a profile gets today is held by no test of those lanes.

## What changes

1. `docs/design/35-executable-lists.md` gains a section for `oci` and a section for `pod`. Each
   names the mechanism tried, the command that was run on the adapter's lane, and what was
   observed: the denied execution failed, or the mechanism was refused and by what.
2. `spec/confinement/confinement.yaml` gains one scenario per adapter: a profile with
   `exec_subpaths` and a deny-list is refused `confinement.fact-unserved` naming `exec_subpaths`.
   `cargo xtask check-ess` regenerates the suite.
3. For each adapter whose section records a mechanism that worked, a draft story for its
   enforcement is created under `epic:runs-are-governed-and-limit-hits-are-events`. No adapter
   code changes in this story.

## Acceptance

The `oci` lane (`crates/substrate-host/tests/ess_confinement_oci.rs` with `SUBSTRATE_OCI_LANE`)
and the `pod` lane (`crates/substrate-host/tests/ess_confinement_pod.rs` with
`SUBSTRATE_POD_LANE`) each run the new refusal scenario from the committed suite; before this
story the suite holds no scenario that opens either adapter with an executable list.

Checks:

- `docs/design/35-executable-lists.md` holds a section for `oci` and one for `pod`, and each
  records a mechanism, the command run and its observed result.
- The scenario's expected refusal names `exec_subpaths`, the first of the two facts in the `Fact`
  order `story:executable-deny-list` fixes.

- The planning store holds one draft enforcement story per adapter whose section records a
  working mechanism, and none for an adapter whose section records a refusal.
- No adapter admits a profile whose list it does not enforce (invariant 3).
- A lane that cannot run reports the scenario absent, never passed.

## Boundaries

- No enforcement code for `oci` or `pod`.
- The `linux` and `macos` adapters: `story:linux-executable-allow-list`,
  `story:executable-deny-list`.
- The Kubernetes namespace driver behind the wire (`--driver kubernetes`) is not the `pod` adapter;
  the wire is `story:exec-start-carries-executable-lists`.

## Order

After `story:executable-deny-list`: the scenario names the deny-list field that story adds. After
`story:process-count-limit-hit-is-observed` only because both change
`spec/confinement/confinement.yaml` and the regenerated suite.

## Scope

- `docs/design/35-executable-lists.md`. Inferred.
- `spec/confinement/confinement.yaml`, `spec/confinement/scenarios`, `spec/confinement-suite.json`.
  Cited: `spec/confinement/confinement.yaml:212-220`.
- `crates/substrate-host/tests/ess_confinement_oci.rs`,
  `crates/substrate-host/tests/ess_confinement_pod.rs`. Cited: `ess_confinement_oci.rs:35`,
  `ess_confinement_pod.rs:865`.
