---
format: aep.planning-md/3
id: decision-blocker:chart-lane-host-profile
kind: decision-blocker
status: cleared
title: The local chart lane cannot run beside a host kernel that already holds the execution profile
relations:
- blocks: story:chart-lane-local-execution-record
withholds: test_result
revision: 3
transitions:
- {from: "open", to: "cleared", at: "2026-10-28T19:46:32Z", actor: "human:timo", revision: 3}
---
# The local chart lane cannot run beside a host kernel that already holds the execution profile

## Finding

`cargo xtask chart-lane cluster --node-profiles` (PR 190) runs on a kind node, which shares the
host kernel. On the workstation that records releases, the kernel already holds
`substrate-host-exec-v1 (enforce)`: first loaded 2026-09-09 shortly after boot, reloaded with the
same bytes on 2026-10-06 and 2026-10-07, loader not identified. The lane did not load it, so it
refuses up front rather than replace or remove a profile another workload may run under.

A lane-unique profile name is not a chart value: the name is fixed in the policy source
(`deploy/apparmor/substrate-host-execution`), compiled into the installer for its exact-source
check, fixed in `crates/substrate-daemon/src/container_profiles.rs` and checked by the bootstrap
in `crates/substrate-daemon/src/container_bootstrap.rs`. Changing it is a confinement contract
change (invariant 8).

## Options

| | what it does | cost |
|---|---|---|
| A | The profile name becomes a deployment parameter: the policy is rendered from one template, the installer compares against the rendered bytes, the bootstrap checks the configured name; unset by default. | A design document first, then a chart, installer and bootstrap change. |
| B | The lane adopts a loaded profile only when the node's parser compiles the repository source to the kernel's exact hash, and never replaces or removes it. | Parser output must match across versions (unverified); a wrong comparison could replace a profile another workload runs under. |
| C | The local lane runs on its own kernel, a throwaway VM. | The heaviest option; nothing shared. |

Recommendation: A. It is the only option that keeps the installer's exact-source check and
needs no shared-kernel assumption.

## What clears it

A chosen option, and for A an accepted design. Until then PR 190 stays a draft, because its
release step requires a `Chart-lane:` record nobody can produce here.


## Decided, 2026-10-28: A, design first

Taken by the coordinating session as its D167 under the operator's standing grant.

- The profile name becomes a deployment parameter whose default is `substrate-host-exec-v1`, so
  no current deployment changes. The policy source, the installer's exact-source check and the
  bootstrap's label check all take the name from it.
- The chart lane uses a lane-unique name; a loaded profile it does not own stays untouched.
- B is rejected: adopting a profile whose loader is unknown weakens the exact-source guarantee.
  C is rejected: it adds a VM to every release.
- Limit: if the design finds the change needs an ADR, or alters what a deployed profile
  enforces, it stops and is filed for the operator.
- Order: after the open pull requests and the `story:adapter-stdin-pipe` wave. PR 190 stays a
  draft until then. The work is `story:host-profile-name-is-a-deployment-parameter`.
