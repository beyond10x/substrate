---
title: Choose a confinement profile
description: Open the adapters a host offers, read the families and facts each proved, and pick one for a profile without naming an adapter or an operating system.
---

# Choose a confinement profile

`substrate_host::confinement`, in the `b10x-substrate-host` crate, is the in-process API an
embedder uses to run a command under confinement without a daemon. It is the host crate's API, not
the wire contract: no contract bundle changes with it, and it is a development surface.

A run takes three steps. Open an adapter, which proves what it can do. Describe what the run needs
as a `ConfinementProfile`. Have the adapter admit the profile, then start the command under it.
Anything the adapter did not prove is refused by name. Nothing falls back to another adapter, and
nothing is quietly dropped.

## Open what the host offers

`confinement::open` takes one `AdapterRequest`: `Local`, `Linux`, `Macos`, `Systemd`, `Oci` or
`Pod`, each carrying its own settings. An adapter that cannot open on this host answers
`confinement.adapter-unserved`. Open every adapter your configuration offers and keep the ones that
opened; which one opened is not something to branch on.

| Adapter | What it is |
|---|---|
| `local` | an unconfined host process, always declared insecure |
| `linux` | the host driver: bubblewrap, namespaces, cgroup v2, seccomp |
| `macos` | a Seatbelt profile |
| `systemd` | a transient systemd scope around `local` or `linux` |
| `oci` | one Docker or podman container per run, the image named by digest |
| `pod` | one Kubernetes Pod per run, the image named by digest |

[Confinement adapters](../concepts/adapters.md) explains what each one enforces, declares and
refuses.

## Read the facts

`adapter.facts()` returns `AdapterFacts`, read once at open and never raised afterwards:

- `served_families`: the contract families the adapter serves, in this order: `process_run`,
  `process_watch`, `output_read`, `wait`, `workspace`, `filesystem_view`, `network_egress`,
  `network_ingress`, `limits`, `audit`. `serves(&family)` asks for one.
- `facts`: what the adapter proved, in this order: `declared_insecure`, `paths_are_host_paths`,
  `whole_tree_kill`, `local_permitted`, `ipc_socket_allowlist`, `ipc_signals_contained`,
  `sealed_paths`, `exec_subpaths`, `private_tmpdir`, `host_network`, `image_digest_pinned`.
  `proves(&fact)` asks for one.

Two facts change how you write the profile and the command:

- `declared_insecure`: the run is not confined. The profile must carry a `local_reason` that is not
  blank, and every observation of the run carries that reason.
- `paths_are_host_paths`: the run sees host paths. Without it, the run sees the paths the profile
  mounts: under `linux` the workspace is `/workspace` and each root is at its `mount`.

What each adapter serves depends on the host. `linux` serves a family only when the driver's probe
proved the mechanism behind it at open, so `limits` is absent without a delegated cgroup v2 root,
and `filesystem_view` is absent without proven read-only roots and files. `local` serves
`process_run`, `process_watch`, `output_read`, `wait`, `workspace` and `audit`, and nothing that
would claim confinement. No adapter serves `network_ingress`.

## Describe the run

Each profile field needs the family or the fact beside it.

| Field | Needs |
|---|---|
| `families`, `facts` | themselves, beyond what the fields imply |
| `workspace`, `workdir` | `workspace` |
| `writable_roots`, `read_only_roots`, `read_only_files` | `filesystem_view` |
| `sealed_paths` | the fact `sealed_paths` |
| `exec_subpaths` | the fact `exec_subpaths` |
| `private_tmpdir` | the fact `private_tmpdir` |
| `network: Egress` | `network_egress` |
| `network: Isolated` | `network_egress`, on an adapter that proves `host_network` |
| `ipc.socket_allowlist` | the fact `ipc_socket_allowlist` |
| `ipc.signals_contained` | the fact `ipc_signals_contained` |
| `process_bounds`, `io_bounds` | `limits` |
| `audit` | `audit`; admission, refusal and start records go to the sink |
| `image` | on an adapter that proves `image_digest_pinned`, `name@sha256:` and 64 lowercase hex digits |

`ingress_routes` and `rate_bounds` are reserved: any value is
`confinement.reserved-field-unserved`. `profile.required_facts()` lists the facts a profile
requires, in the order above. `profile.required_families()` lists the families its `families` and
its fields imply, in the order above, except two that depend on more than the profile's fields:
`audit` for an `audit` sink, and `network_egress` for `network: Isolated` on an adapter that proves
`host_network`. Admission requires both, so add them yourself (see `needed` below).

A workspace is the directory the run starts in and may write, so a build that writes `target/`
inside it needs no more than `workspace`. A directory outside it is a declared root:
`read_only_roots` for a toolchain, `writable_roots` for an install root the operator declared by
name.

## Choose an adapter by its facts

Pick the adapter whose facts cover the profile, and prefer one that does not prove
`declared_insecure`:

```rust
use substrate_host::confinement::{
    self, AdapterFacts, ConfinementProfile, ContractFamily, FAMILY_ORDER, Fact, Network,
};

/// The families admission requires of this adapter, in the contract's order.
fn needed(profile: &ConfinementProfile, facts: &AdapterFacts) -> Vec<ContractFamily> {
    let implied = profile.required_families();
    FAMILY_ORDER
        .into_iter()
        .filter(|family| {
            implied.contains(family)
                || (*family == ContractFamily::Audit && profile.audit.is_some())
                || (*family == ContractFamily::NetworkEgress
                    && profile.network == Some(Network::Isolated)
                    && facts.proves(&Fact::HostNetwork))
        })
        .collect()
}

fn covers(profile: &ConfinementProfile, facts: &AdapterFacts) -> bool {
    needed(profile, facts).iter().all(|family| facts.serves(family))
        && profile.required_facts().iter().all(|fact| facts.proves(fact))
}

let offered: Vec<_> = requests
    .into_iter()
    .filter_map(|request| confinement::open(request).ok())
    .collect();
let chosen = offered
    .iter()
    .filter(|adapter| covers(&profile, adapter.facts()))
    .min_by_key(|adapter| adapter.facts().declared_insecure());
```

`covers` is a preselection; admission is the answer. An adapter can refuse what its families do
not show: `linux` refuses a `workdir` its host has not proved with `exec.workdir-unserved`, and a
writable root the operator did not declare with `exec.writable-roots-host-unserved`. When an
adapter does not cover a profile, admitting it there is refused in the order of the refusals table
below. Past its first five rows, that is `confinement.family-unserved` naming the first family of
`needed` the adapter does not serve, or, when it serves them all,
`confinement.fact-unserved` naming the first fact of `required_facts()` it did not prove. Either
can be read from the facts before the call.

## Admit, start and observe

```rust
let admitted = adapter.admit(&profile)?;
let run = adapter.start(&admitted, &command).await?;
let ended = adapter.wait(&run).await?;
let output = adapter.output(&run).await?;
```

`Command` holds the `argv` and the whole environment; nothing is inherited from the embedding
process. A profile admitted by one adapter starts only on that adapter, and a start decides the
profile again against the adapter's own facts. `observe`, `signal` and `render` (the program and
arguments a start would run, without running it) take the same run or admitted profile.

## Refusals

Admission refuses in this order and names the first refusal it meets:

| Order | Code | Status | When |
|---|---|---|---|
| 1 | `confinement.reserved-field-unserved` | 501 | a reserved field is set |
| 2 | `confinement.adapter-unserved` | 501 | the adapter did not open, or the profile's `inner` is not the adapter it wraps |
| 3 | `confinement.local-reason-missing` | 422 | a declared-insecure adapter and no reason |
| 4 | `confinement.image-unpinned` | 422 | a digest-pinning adapter and an image not named by digest |
| 5 | `confinement.local-refused` | 403 | a declared-insecure adapter the host's policy forbids |
| 6 | `confinement.family-unserved` | 501 | the first family of `needed` not served |
| 7 | `confinement.fact-unserved` | 501 | the first required fact not proved |

`error.code()` and `error.status()` return the code and status. A refusal from the host itself,
met while starting, keeps its own code, such as `exec.workdir-missing`.

A start or a render whose profile names no `workspace` is `confinement.workspace-required` (422)
on every adapter that runs the program in a host directory. `pod` runs it in its own `emptyDir` at
`/workspace` instead.

The host's operator can forbid `local` by setting `SUBSTRATE_CONFINEMENT_LOCAL` to any value other
than `permit`; an embedder can only tighten that, with `LocalPolicy::Refuse`.

Read [confinement and refusal](../concepts/confinement.md) for the guarantees the `linux` adapter's
host driver enforces.
