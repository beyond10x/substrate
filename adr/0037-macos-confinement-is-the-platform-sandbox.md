---
status: accepted
date: 2026-10-21
---

# ADR 0037: macOS confinement is the platform sandbox, with its own capability facts

## Acceptance

The operator accepted this decision on 2026-10-21, answering morning-queue row 35 with option A:
"yes A -> seatbelt" (17:07Z). Earlier that day the operator had said "we do want to support
sandboxing, *some* sandbox exists for macOS i guess, even if not bwrap".

## Context

Since 0.7.18, `b10x-substrate-host` compiles on macOS and refuses there by name.
`HostDriver::open` returns `host.platform-unserved` (`crates/substrate-host/src/unsupported.rs`,
constant `PLATFORM_UNSERVED` in `crates/substrate-host/src/lib.rs`), and
`HostProcessSupervisor::open` returns `process.host-unserved`
(`crates/substrate-host/src/host_process/unsupported.rs`). No unconfined macOS path exists, and
this decision keeps it that way.

Bubblewrap, namespaces, cgroup v2, seccomp and `openat2` are Linux interfaces. The macOS platform
sandbox is Seatbelt: a profile in the Sandbox Profile Language (SBPL), applied to a process by
`/usr/bin/sandbox-exec -p <profile>` or by the `sandbox_init` call. Apple marks both deprecated and
documents neither profile language, but ships them, and its own system services run under SBPL
profiles.

## Decision

1. Confinement on macOS is a Seatbelt profile that substrate renders per exec from the start
   request, with `(deny default)` as its first rule. Everything the exec may touch is an explicit
   allow rule naming a resolved absolute path.
2. The profile is applied by `/usr/bin/sandbox-exec`. Substrate does not call `sandbox_init`
   between `fork` and `exec`, where only async-signal-safe calls are sound.
3. A path that SBPL cannot carry as a plain string literal (a quote, a backslash, a control
   character, a relative path) is refused before anything runs. Substrate does not escape such
   paths into a profile.
4. Fail closed. A missing `sandbox-exec`, a profile the sandbox rejects, or a failed self-probe
   refuses with the existing named refusal `exec.sandbox-unavailable`. Nothing runs unconfined.
5. The Seatbelt backend publishes **its own capability facts** and claims no Linux floor fact it
   cannot enforce (invariant 3). Clients read facts and do not branch on which driver answered
   (invariant 4). The new fact names are a wire change and arrive in a successor bundle, which is
   additive and leaves every released bundle untouched (invariant 6).
6. Order of work. First, a confinement primitive in `b10x-substrate-host` and an adversary suite
   that runs it on GitHub's `macos-15` runner. `HostDriver::open` keeps refusing on macOS during
   that step. Second, the macOS `HostDriver` on top of the primitive, with the successor bundle
   and its facts. Each step is its own story.

## What Seatbelt confines

"Tested" names the case in `crates/substrate-host/tests/seatbelt_adversary.rs` that proves the
row on the `macos-15` runner. "Untested" means no case proves it yet.

| Linux floor item | Seatbelt rule | State |
|---|---|---|
| Writes confined to the workspace | `(deny default)` plus `file-write*` on the workspace subpath only | untested |
| Read-only roots | `file-read*` on each declared subpath, no `file-write*` there | untested |
| No read of undeclared host paths | `(deny default)`; no `file-read-data` outside declared and system subpaths | untested |
| No network | no `network*` allow rule | untested |
| Exec of allowed binaries only | `process-exec` on each allowed literal path | untested |
| No signal to a process outside the sandbox | `signal` allowed for `(target same-sandbox)` only | untested |
| Cleared and shaped environment | the spawner clears the environment; not a sandbox rule | untested |
| Fail closed | refusal when `sandbox-exec` is missing or a path is not representable | untested |

## What Seatbelt cannot confine

| Linux floor item | macOS gap | State |
|---|---|---|
| pids bound | no cgroups; `RLIMIT_NPROC` counts per user, not per tree | untested; refused by name |
| memory+swap bound | no cgroups; `RLIMIT_AS` is not a resident-memory bound | untested; refused by name |
| Cumulative CPU per tree | no cgroup accounting | untested; absent |
| Whole-tree kill | no cgroup; a child can leave the process group with `setsid` | untested; refused by name |
| `openat2` beneath/no-link/no-mount | no `RESOLVE_BENEATH`; profile rules match resolved paths instead | untested |
| No nested user namespace | macOS has no user namespaces | not applicable |
| Syscall filter | no seccomp counterpart | untested; absent |
| Writable-storage quotas | no project quotas | untested; refused by name |
| Metadata of undeclared paths | `file-read-metadata` is allowed so `stat` and path resolution work; existence of a path is observable | untested |
| Destination-bound apertures (ADR 0013) | SBPL matches IP and port, not a hostname | untested; refused by name |
| Mach service lookup | lookups the dynamic loader and libSystem need are allowed by name | untested |

## Consequences

- macOS gets confined exec with a smaller, published fact set. A client that needs resource bounds
  stays on Linux.
- Apple can change or remove `sandbox-exec`. The fail-closed refusal and the CI adversary suite are
  what detect that, and the suite runs on every pull request.
- The release confinement-lane record (`Confinement-lane:`) stays a Linux delegated-lane record. A
  macOS lane record is a separate decision.

## Alternatives

| Option | Why not |
|---|---|
| Stay refused on macOS | The operator chose sandboxing |
| A Linux VM (Virtualization.framework, Lima) running the Linux driver | Full floor, but a VM lifecycle inside substrate; it needs its own decision |
| App Sandbox entitlements | They apply to a signed application bundle, not to per-exec children with per-request paths |
| `sandbox_init` after `fork` | It allocates, and only async-signal-safe calls are sound there |
