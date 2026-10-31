---
title: Release notes
description: What changed in the newest Substrate release, generated from CHANGELOG.md.
---

# What changed in the newest release

This page ends with the newest released section of `CHANGELOG.md`, and `cargo xtask check-docs`
fails when the two differ. Earlier releases are in the
[full changelog](https://github.com/beyond10x/substrate/blob/main/CHANGELOG.md). The
[confinement adapters](./concepts/adapters.md) page explains each adapter the notes name.

What 0.16.1 first publishes is listed under 0.16.0 in the full changelog and summarized on the
[status page](./status.md#new-in-0161), including the namespace driver's limits.

<!-- Generated from CHANGELOG.md by `cargo xtask docs-releases`. Edit CHANGELOG.md, not the text below. -->

## 0.18.0 (2026-10-31)

The wire states what a confinement adapter proves, never its name (design 30, ADR 0048), and the
Kubernetes namespace driver admits an exec start on the confinement its probe observed. Exec is
admitted only on a cluster whose network plugin stops the probe's egress attempts inside the Pod
(`EPERM`, `EACCES`, `ENETUNREACH` or `EHOSTUNREACH`). A plugin that drops denied packets silently,
kind's among them, leaves `confinement.no-egress` absent, and exec stays refused
`exec.sandbox-unavailable` as in 0.17.0.

### Added in 0.18.0

- Development contract bundle `substrate-wire/0.26.0` (design 30, ADR 0048), now advertised by the
  daemon, the Rust SDK and the conformance runner. It adds no route. A capability snapshot may
  state one optional fact per confinement family (`confinement.family.process-run` to
  `confinement.family.audit`) and per mechanism-neutral property (`confinement.whole-tree-kill`
  and six more), each `true` or absent. Both starts accept an optional closed `sandbox.requires`
  that names families and properties, and two refusals are published at 501 with a vector each:
  `confinement.family-unserved` and `confinement.fact-unserved`. `substrate-wire` exports
  `ConfinementFamily`, `ConfinementProperty`, `ConfinementRequires`,
  `validate_confinement_requires` and `confinement_requires_refusal`.
- The host daemon publishes the `linux` adapter's family and property facts, each derived from a
  proof it already makes: `audit` always, `workspace` with guarded workspace I/O, and with the exec
  floor `process-run`, `process-watch`, `output-read`, `wait`, `limits`, `filesystem-view` (with
  read-only roots and files), `network-egress` (with a declared aperture), `whole-tree-kill`,
  `ipc-socket-allowlist` and `ipc-signals-contained`. It never publishes `network-ingress`. The
  capability snapshot digest now binds the adapter, so the host's snapshot id changes once.
- Exec and session starts check `sandbox.requires`. A requirement outside the schema (an empty
  object or list, a repeated or unknown name, a `null` list) is `request.schema-invalid` (422)
  with the other request-shape checks, before `exec.capability-stale`. After the stale check, the
  first unserved family is `confinement.family-unserved` (501) and the first unproved property
  `confinement.fact-unserved` (501), each recorded as refused before dispatch and before any
  field-specific check. The host driver checks `requires` at the same two points, so it names
  the same refusal the daemon does; for that its stale check now also precedes read-only roots,
  read-only files, workspace access and secret slots, as the daemon's does. An admitted run's
  applied record echoes `requires` on both drivers. A start without `requires` is unchanged.
- `Machine::served_families` and `Machine::proved_properties` in the Rust SDK, which also
  re-exports `ConfinementFamily`, `ConfinementProperty` and `ConfinementRequires`.
- `/metrics` on the operational listener names the two new refusal codes and, on a host daemon
  only, `substrate_adapter_info{adapter="linux"} 1` (ADR 0048 D7): the operator sees the adapter
  name, the API never does. A `--driver kubernetes` daemon opens no adapter and has no such series.
  Each scrape holds at most 585 samples.
- The runtime vectors check the published facts and the `requires` refusals: the portable lane
  counts 124 cases and the delegated lane 189.
- The namespace driver's probe observes confinement (design 34 § 6.2). Its probe Pod now mounts
  the probe's volume writable and carries one environment variable, `SUBSTRATE_PROBE_TARGETS`.
  `probe-child` forks a grandchild that rewrites `substrate-probe-heartbeat` in the volume every
  100 ms, and makes three egress attempts of at most 3 s each: TCP to the Kubernetes Service host
  the keeper's environment states, port 443; a DNS query to `/etc/resolv.conf`'s first
  nameserver; TCP to `egress_probe_address`. Each probe run takes about 5 s longer. The snapshot
  then states the seven families `confinement.family.process-run`, `.process-watch`,
  `.output-read`, `.wait`, `.workspace`, `.limits` and `.audit` when the Pod ended with its
  frames, its applied bounds and an exit record that agrees with the API;
  `confinement.whole-tree-kill` when the keeper sees the heartbeat unchanged for 2 s after the
  container shows `terminated`; and `confinement.no-egress` when all three attempts were stopped
  inside the Pod (denied, or no route) and no NetworkPolicy in the bound set other than the
  default-deny one has an egress rule. A timed-out attempt never counts: on a kind cluster whose
  egress was observed open, one probe timed out all three attempts while another reached them.
  Any egress-allowing policy withholds the fact, whichever Pods it selects. Each lapses with the
  proof. It never states the four Linux `exec.*` facts.

### Changed in 0.18.0

- A namespace-driver daemon admits an exec start when its snapshot states
  `confinement.family.process-run`, `.wait`, `.output-read` and `.limits`,
  `confinement.whole-tree-kill` and `confinement.no-egress` (design 34 § 6.4). Only a network
  plugin that stops the probe's attempts inside the Pod yields `confinement.no-egress`. On a plugin
  that does not enforce the NetworkPolicy, or drops denied packets silently (kind's default plugin
  either way), the fact stays absent and every exec start is still `exec.sandbox-unavailable`
  (501), as in 0.17.0. Inferred, not verified: kind's plugin may enforce NetworkPolicy only after
  a Pod's first moments. The host driver never states `confinement.no-egress` and admits as
  before.
- `AppliedConfinement.cgroup` is `Option<String>` and is optional in the bundle, present whenever
  the adapter applied a cgroup; the host driver applies one to every run, so its records keep their
  bytes. The applied record gains an optional `requires` echo. A crate that constructs or
  exhaustively destructures the Rust type `AppliedConfinement` (loom does, in
  `loom-intake-slice/src/confinement.rs`) must adapt when its substrate pin moves, even though it
  uses the host API in process.
