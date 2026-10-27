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

"Tested" names the case that proves the row and the `macos-15` run it passed in: an adversary case
in `crates/substrate-host/tests/seatbelt_adversary.rs`, or a unit test in
`crates/substrate-host/src/seatbelt.rs`. Each adversary case pairs the attack with a control that
succeeds where the profile allows it. "Untested" means no case proves the row yet.

| Linux floor item | Seatbelt rule | State |
|---|---|---|
| Writes confined to the workspace | `(deny default)` plus `file-write*` on the workspace subpath only | tested: `a_write_outside_the_workspace_is_refused`, run 39652219079 |
| Read-only roots | `file-read*` on each declared subpath, no `file-write*` there | tested: `a_read_only_root_is_readable_and_not_writable`, run 39652219079 |
| No read of undeclared host paths | `(deny default)`; no `file-read-data` outside declared and system subpaths | tested: `an_undeclared_host_file_is_unreadable`, run 39652219079 |
| No network | no `network*` allow rule | tested: `an_outbound_connection_is_refused` (TCP to 127.0.0.1), run 39652219079 |
| Exec of allowed binaries only | `process-exec` on each allowed literal path | tested: `an_unlisted_executable_is_refused`, run 39652219079 |
| No signal to a process outside the sandbox | `signal` allowed for `(target same-sandbox)` only | tested: `a_signal_to_a_process_outside_the_sandbox_is_refused`, run 39652219079 |
| Cleared and shaped environment | the spawner clears the environment; not a sandbox rule | tested: `the_environment_is_cleared` (`TMPDIR`), run 39652219079 |
| Fail closed | refusal when `sandbox-exec` is missing or a path is not representable | tested: unit tests `a_missing_launcher_refuses_before_resolving_anything`, `unrepresentable_paths_are_refused_by_name` and `the_platform_sandbox_enforces_a_profile`, run 39652219079 |

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
| Listing of `/` | `file-read-data` on the literal `/` is allowed: without it the dynamic loader aborts the process (SIGABRT, run 39643245714); the names of top-level entries are observable | untested |
| Kernel state read through `sysctl` | `(allow sysctl-read)` is unscoped: libSystem and the dynamic loader read `sysctl` values (`hw.*`, `kern.*`) while a process starts, and the profile allows every read rather than naming those values; every readable `sysctl` value, such as the kernel version and hardware facts, is observable | untested; which values libSystem needs is not measured |

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
