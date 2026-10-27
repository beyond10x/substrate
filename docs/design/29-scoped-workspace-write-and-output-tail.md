# Scoped workspace write on `linux`, and output that keeps its tail

Status: accepted for implementation, 2026-10-27. Owned by
`story:linux-adapter-serves-scoped-workspace-write` and by `story:output-keeps-tail-within-bound`,
which ships in the same release. Neither
changes a wire byte or a contract bundle: both are substrate-host's confinement API
([ADR 0044](../../adr/0044-confinement-is-a-contract-family-served-by-declared-adapters.md)
decision 7).

Invariant 8 asks for a design before a capability change. No ADR is needed, for three reasons.
[ADR 0023](../../adr/0023-workspace-write-access-is-explicit.md) already decides the three
workspace access modes, the lexical rules, the descriptor-pinned mounts and the probe that publishes
the fact; this note routes the same request through the `linux` adapter and adds a refusal, at the
confinement surface only, for a writable `.git`. Atlas ADR 0109 decision 1
(`architecture/adr/0109-confinement-is-one-substrate-contract-with-adapters.md` in the atlas
repository) already names "named writable subtrees" among the profile's fields. Output retention
moves no bound, and no ADR decides which bytes inside the bound are kept. § 9 lists the decisions
this note leaves alone.

## Evidence: what the tree does at `c2b95f28` (release 0.13.0)

| what | where |
|---|---|
| `linux` sets `workspace_access: WorkspaceAccess::ReadWrite` for every profile | `crates/substrate-host/src/confinement/linux.rs`, `exec_input` |
| `linux` lists `sealed_paths` as not proved; a profile requiring it is `confinement.fact-unserved` | `linux.rs` module header; scenario `spec/confinement/scenarios/linux-sealed-paths-unserved-on-delegated.yaml` |
| the host driver serves `ReadOnly` and `Scoped`: shape, then the fact, then each subtree opened component by component with `openat2` beneath, no symlinks, no cross-device; then pinned with `O_NOFOLLOW` and bound by descriptor (`--ro-bind-fd` root, then `--bind-fd` per subtree) | `crates/substrate-host/src/process.rs`, `admit_workspace_access`, `admit_writable_subtrees`, `prepare_workspace_access`, `PreparedWorkspaceAccess::bind_args`; resolve flags at `crates/substrate-host/src/fs.rs:27` |
| `exec.workspace-scoped-write` is published only after a throwaway bubblewrap proves `allowed/` writable and its sibling and the root not | `crates/substrate-host/src/probe.rs`, `probe_workspace_scoped_write` |
| `render` already admits `workspace_access` | `process.rs`, the render admission beside `RENDER_UNSERVED` |
| every adapter keeps the **head** of each stream: `local`, `macos`, `systemd` around `local` and `oci` each copy `room = LIMIT - kept.len()`; `linux` drains through the host driver's `drain_capped`, which ends a truncated record in the 31-byte marker `\n[substrate: output truncated]\n`; `pod` asks the log API for `limitBytes` = bound + 1 | `local.rs` `drain`, `macos.rs` `drain`, `systemd.rs` `drain`, `oci.rs` `drain`, `process.rs` `drain_capped` and `TRUNCATION_MARKER`, `pod.rs` `read_log` |
| the bounds: `LOCAL_OUTPUT_LIMIT_BYTES`, `MACOS_OUTPUT_LIMIT_BYTES`, `SYSTEMD_OUTPUT_LIMIT_BYTES`, `oci::OUTPUT_LIMIT_BYTES`, `POD_OUTPUT_LIMIT_BYTES`, and for `linux` `io_bounds.output_bytes` capped by `HostConfig.output_limit_bytes` (default `substrate_wire::MAX_IO_BYTES`); every one is 1 MiB | the same files |
| `ConfinementProfile`, `RunOutput` and the `Fact` enum are public and not `#[non_exhaustive]`; `Fact` is generated from the ESS domain | `crates/substrate-host/tests/confinement-public-items.txt`, `crates/substrate-host/src/generated/confinement.rs` |

## 1. Consumer surface: writable subtrees

`ConfinementProfile` gains one field, and `confinement` re-exports the wire type it holds, as it
already does for `ReadOnlyRoot`, `ReadOnlyFile`, `WritableHostRoot` and `SecretSlotRequest`:

```rust
pub use substrate_wire::WorkspaceAccess; // in confinement/mod.rs

pub struct ConfinementProfile {
    // ... every existing field unchanged ...
    /// What the run may write inside `workspace` (ADR 0023). `ReadWrite`, the default, is the
    /// whole workspace, as before.
    pub workspace_access: WorkspaceAccess,
}
```

`WorkspaceAccess` is `ReadWrite` (default), `ReadOnly`, or `Scoped { writable_subtrees }`. A second
type for the same request would need a second validator and a mapping that could drift, so the
profile holds the wire type itself.

A value other than `ReadWrite` implies the `workspace` family in `required_families`, beside
`workspace` and `workdir`.

**Validation, on every adapter, in its `check`, in this order.** A shared
`pub(crate) fn check_workspace_access(&ConfinementProfile)` in `confinement/mod.rs` runs steps 1 and
2, so the caller's mistake gets the same answer on every adapter. The adapter then runs step 3.

1. `substrate_wire::validate_workspace_access`, unchanged: 1 to `MAX_WORKSPACE_WRITABLE_SUBTREES`
   (64) entries; each passes `validate_relative_path` (not empty, not absolute, no empty, `.` or
   `..` component, no NUL, no backslash, at most `MAX_PATH_DEPTH` (64) components); strictly
   ascending byte order; none at or beneath another. A failure is `exec.workspace-access-invalid`
   (422).
2. No component of any subtree is `.git`, compared ASCII case-insensitively, so a case-folding
   directory cannot let `.GIT` reach the same entry. A failure is the new
   `confinement::WRITABLE_SUBTREE_GIT` = `confinement.writable-subtree-git` (422, class `Refused`),
   naming the subtree.
3. The adapter serves the mode, or refuses it by name (§ 4).

Two refusals come from the host at start, not at admission. They are the existing ADR 0023 refusals,
because only a start opens the workspace:

- `exec.workspace-subtree-absent`, `-symlink`, `-not-directory` (422) from
  `admit_writable_subtrees`;
- `exec.workspace-subtree-changed` (422) from `prepare_workspace_access`.

Each is audited by `audit_refusal`, as every start refusal is. A subtree must exist before the run.
A consumer that wants `target/` writable creates it first, because the read-only root lets the run
create nothing at the top of the workspace.

## 2. `linux` enforcement

`LinuxAdapter::exec_input` passes `profile.workspace_access.clone()` instead of the constant. The
mechanism is the host driver's, unchanged:

1. The workspace root is pinned beneath the driver's workspace root without following a link
   (`crate::fs::pin_workspace_directory`).
2. Admission walks each subtree from that descriptor one component at a time with `openat2`
   (`RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS | RESOLVE_NO_MAGICLINKS | RESOLVE_NO_XDEV`). A symlink at
   any component, a non-directory, an absent component, or a crossing onto another mount is refused
   before anything is spawned.
3. The start reopens each component from a duplicate of the root with `O_PATH | O_DIRECTORY |
   O_NOFOLLOW` and moves every descriptor above the secret-slot range. A component that changed
   since admission is `exec.workspace-subtree-changed`.
4. Bubblewrap binds `--ro-bind-fd <root> /workspace` first, then `--bind-fd <subtree>
   /workspace/<subtree>` for each subtree, in the validated ascending order. Every bind is the
   pinned object; no path is resolved again. Bubblewrap closes the descriptors before the program
   starts.

**`.git` needs no re-bind.** The workspace's own `.git` (a directory, or a worktree's `gitdir:`
file) lies in the read-only root. Step 2 of § 1 refuses every subtree that is `.git` or lies
beneath it. A subtree cannot contain the root's `.git`, because "." is not a valid subtree. So no
writable bind reaches it. The run cannot write, rename, remove or create `/workspace/.git`. It cannot
rename a writable subtree's mount point (`EBUSY`) or unmount it (no `CAP_SYS_ADMIN`, and no nested
user namespace under the safety envelope's floor). It cannot hard-link a `.git` file into a writable
subtree, because `linkat` across two mounts is `EXDEV`. Nothing else is mounted at or beneath
`/workspace`: `RESERVED_MOUNTS` and `WRITABLE_ROOT_RESERVED_MOUNTS` keep read-only and writable roots
out of it.

**A subtree that holds a registered submodule is refused.** A submodule's `.git` is a `gitdir:`
file in its checkout, pointing into the root's `.git/modules/`. A run that may write the checkout
can replace that file with a repository whose configuration it chose, and a host `git status` in
the workspace root runs Git in every submodule, so it runs the `core.fsmonitor` the run chose. The
security review observed this. That is the root's `.git` reached through a file the run may write,
so the start refuses it with the same `confinement.writable-subtree-git` (422) as a subtree at or
beneath `.git`: no new name and no wire change. When the start has pinned the workspace and shown
every subtree to exist, it reads the root's `.gitmodules` through the pinned descriptor (`openat2`
beneath, no symlinks), at most 1 MiB, and only its `path` keys. Any registered path that equals or
lies beneath a writable subtree is refused, compared component by component and ASCII
case-insensitively, as the `.git` rule is. A registered submodule that contains a writable subtree
has its own `.gitmodules` read the same way. Each submodule is descended into once, and at most 64
registries are read per start. A `.gitmodules` that is present but cannot be read that way (a
symlink, not a regular file, over the bound) refuses the start, and so does a walk past 64
registries. Neither is taken for an empty registry. Only the `linux` adapter's start applies the rule. The daemon's exec route does
not, so its answers are unchanged (§ 7).

**The threat model is the confined run.** A host process with the workspace owner's uid can rename
entries before admission. It is outside this boundary, as it is for ADR 0023 on the daemon route.

**What `Scoped` protects, and the limits the note states rather than closes.** `Scoped` protects
the workspace root's `.git` from writes in the run, and it refuses subtrees holding a registered
submodule. It does not close the following; each is pinned by a delegated case in
`crates/substrate-host/tests/ess_confinement_linux.rs`:

- A nested `.git` inside a writable subtree stays writable if no `.gitmodules` registers it (a
  vendored repository, an unregistered checkout). Finding one needs a walk of the subtree, which is
  unbounded. A consumer that must keep one closed names a narrower subtree. `sealed_paths` would be
  the field for it, and § 3 keeps it refused on `linux`. Git's own registry is the index, not
  `.gitmodules`: a gitlink with no `.gitmodules` entry (an embedded repository added with
  `git add`, or a `.gitmodules` since removed) is not refused, and a root `git status` still runs
  Git in it. That was observed with host Git alone, outside a run, by removing `.gitmodules` from
  the worktree and, separately, from the index and `HEAD`. Reading the index is a design of its own.
- `Scoped` does not protect host Git run inside a writable subtree from a repository the run
  creates there. Host Git in `out/` finds `out/.git` before the root's, so it reads the
  configuration the run wrote, `core.fsmonitor` included. A `ReadWrite` workspace has the same
  exposure. An operator runs Git on the host in the workspace root, not inside a writable subtree.
- A hard link made on the host before the run stays writable through a subtree when it links a file
  there to a file elsewhere in the workspace (`.git` included). A read-only bind restricts the mount,
  not the inode. This was observed: the host's `.git/config` changed through `out/alias`, while a
  direct write to `/workspace/.git/config` was refused. The run itself cannot create such a link:
  `ln .git/config out/link` fails with `EXDEV` ("Invalid cross-device link").

`ReadOnly` is served by the same path with no subtree: `--ro-bind-fd <root> /workspace` alone.

## 3. `sealed_paths` stays refused on `linux`

Decision: `linux` keeps refusing a profile that requires `sealed_paths` with
`confinement.fact-unserved` naming `sealed_paths`. The scenario
`linux-sealed-paths-unserved-on-delegated` holds unchanged, so this story edits no scenario and no
suite (ADR 0044 decision 6).

Why not serve it now:

1. **The fact would mean two things.** On `macos` a sealed path stays unwritable whether or not it
   exists, and so does every directory between the workspace and it (`macos.rs` module header). A
   bind mount needs an existing object. An absent sealed path would make bubblewrap create the
   mount point, which is a host-visible directory when its parent is writable. Keeping a parent from
   being renamed means binding each parent onto itself. A `linux` fact that refuses an absent path,
   or holds only for existing ones, is a weaker promise under the same name, and a client would
   have to branch on the adapter (invariant 4).
2. **Scoped write covers what the consumer asked for.** The acceptance needs a write outside the
   named subtrees refused and a write over `.git` refused. With `Scoped`, everything not named is
   read-only, `.git` included, and a subtree holding a registered submodule is refused (§ 2). A
   deny-list inside a writable tree is then only needed for the limits § 2 states: an unregistered
   nested `.git`, a repository the run creates for host Git run inside the subtree, and a hard
   link made on the host before the run. They have no consumer yet.
3. **ADR 0023's rule would apply.** A fact is published only after a throwaway sandbox proves it.
   A sealed-path probe, its fact source in `CapabilityFacts` and its mount order are a design of
   their own. When a consumer needs a nested seal, that is a later story with its own note.

## 4. Every adapter: served, or refused by name

| adapter | `ReadOnly`, `Scoped` | refusal when not served |
|---|---|---|
| `linux` | served when the snapshot has `exec.workspace-scoped-write: true` | `exec.workspace-scoped-write-unserved` (501) on a host whose probe did not prove it |
| `systemd` around `linux` | served: its `check` and `start` are `linux`'s (`systemd.rs`, `self.inner.check`) | as `linux` |
| `local` | refused: an unconfined process cannot be kept from a path | `exec.workspace-scoped-write-unserved` (501) |
| `systemd` around `local` | refused, as `local` | `exec.workspace-scoped-write-unserved` (501) |
| `macos` | refused in this story. Seatbelt can express it, but no `seatbelt::probe_build` case proves it, and ADR 0023 publishes nothing unproved. A later story that adds the probe serves it. | `exec.workspace-scoped-write-unserved` (501) |
| `oci` | refused. The runtime resolves a `--mount` source path in its own process, so it cannot take a pinned descriptor. A subtree swapped for a symlink between the check and the runtime's mount would redirect a writable bind, which ADR 0023 forbids. | `exec.workspace-scoped-write-unserved` (501) |
| `pod` | refused. Its workspace is an `emptyDir` the adapter creates, and it already refuses a host `workspace` (`check_profile`). | `exec.workspace-scoped-write-unserved` (501) |

The refusal reuses the wire's constant `substrate_wire::EXEC_WORKSPACE_SCOPED_WRITE_UNSERVED`. That
follows the `linux` adapter's existing pattern for `exec.workdir-unserved` and
`exec.secret-slots-unserved`: a field that needs more than its family is refused by the host's own
name in `check`.

**Which fact advertises it: none in `AdapterFacts`, in this story.** `Fact` is a generated,
exhaustive public enum. A new variant breaks every consumer that matches on it exhaustively. It
also changes the ESS domain, `FACT_ORDER`'s array length and the site's fact tables. On `linux` the
capability is the host snapshot fact `exec.workspace-scoped-write`. A consumer learns it from
admission, which spawns nothing, and the refusal names the code. § 10 records this as an open point.

## 5. Consumer surface: output retention

New public types live in `crates/substrate-host/src/output_retention.rs`, a crate-level module,
because the host driver and the confinement adapters both use them. `confinement` re-exports them,
as it re-exports `HostConfig`:

```rust
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Retention {
    /// The first bytes, as every adapter keeps today.
    #[default]
    Head,
    /// The last bytes.
    Tail,
    /// The first half of the bound and the last half.
    HeadAndTail,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OutputRetention {
    pub stdout: Retention,
    pub stderr: Retention,
}

/// Which bytes of one stream `RunOutput` holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct KeptOutput {
    pub retention: Retention,
    /// Every byte the stream produced. Present only when the adapter read it to end of file.
    /// Absent, never guessed, under `pod` and after a read error (invariant 3).
    pub observed_bytes: Option<u64>,
    /// The stream's first bytes at the start of the kept bytes.
    pub head_bytes: u64,
    /// The stream's last bytes, directly after the head.
    pub tail_bytes: u64,
}
```

`ConfinementProfile` gains `pub output_retention: OutputRetention`. `RunOutput` gains
`pub stdout_kept: KeptOutput` and `pub stderr_kept: KeptOutput`. The existing `stdout`, `stderr`,
`stdout_truncated` and `stderr_truncated` keep their meaning: `*_truncated` is true when bytes were
dropped or the read failed.

A non-default `output_retention` implies the `output_read` family. Every adapter serves it, so the
implication only orders the refusal.

**What a consumer reads.** For one stream:

- the head is `stdout[..head_bytes]`;
- the tail is `stdout[head_bytes..head_bytes + tail_bytes]`;
- the omitted count is `observed_bytes - head_bytes - tail_bytes`, and the gap sits between head and
  tail;
- anything in `stdout` after the tail is the `linux` truncation marker. That holds only under `Head`
  on `linux`, as today, and nowhere else.

A cut can split a UTF-8 sequence at either side of the gap. The record is bytes.

## 6. Retention algorithm, one buffer for every adapter

`output_retention.rs` holds `pub(crate) struct RetentionBuffer`:

```rust
pub(crate) struct RetentionBuffer {
    retention: Retention,
    head_cap: usize,
    tail_cap: usize,
    head: Vec<u8>,          // grows to head_cap, never beyond
    tail: VecDeque<u8>,     // grows to tail_cap, then drops from the front
    observed: u64,          // saturating
    read_failed: bool,
}
```

The caps come from the stream's existing bound `B`:

| retention | `head_cap` | `tail_cap` |
|---|---|---|
| `Head` | `B` | 0 |
| `Tail` | 0 | `B` |
| `HeadAndTail` | `B / 2` | `B - B / 2` |

`push(chunk)` adds `chunk.len()` to `observed`. It copies into `head` until `head_cap` is reached.
Of what is left, it keeps the last `tail_cap` bytes: when the rest of the chunk alone is at least
`tail_cap` bytes, the ring is cleared and refilled from the chunk's last `tail_cap` bytes. Otherwise
it first pops from the front what the new bytes would push past `tail_cap`, then appends, so the
ring never holds more than `tail_cap` bytes, even for a moment. Work is linear in the bytes read.
Memory is at most `B` per stream, as today, and it is allocated as output arrives, so a quiet
stream costs nothing. Each buffer grows by doubling its allocation, capped at its share of `B`, so
neither the head nor the ring is ever allocated beyond it.

`snapshot()` returns `head ++ tail` (one copy, as today's `clone`) with its `KeptOutput`. On the
adapters that read their own pipes (`local`, `macos`, `systemd` around `local`, and `oci`),
`output()` while the run is running returns the current snapshot, with `observed_bytes` absent
until the stream ended. `linux`, and `systemd` around it, report output when the run ends: the host
driver publishes a run's bytes once its drains ended, so `output()` while the run is running holds
none of them, whatever the retention, as before this note. `pod` reads the log API and is not
covered by either statement. `local` and `linux` hold delegated or portable cases for their
statement; `macos`, `systemd` around `local` and `oci` share `local`'s `Captured` read path and
are stated from it, not observed.

Properties the unit tests hold:

- head + tail never exceeds `B`;
- when `observed <= B`, the kept bytes are the whole stream in order, in all three modes, with
  nothing omitted and `*_truncated` false;
- when `observed > B`, `Tail` keeps exactly the last `B` bytes, and `HeadAndTail` the first
  `B / 2` and the last `B - B / 2`;
- `B = 1` under `HeadAndTail` is a one-byte tail.

**Who uses it.**

- `local`, `macos`, `systemd` around `local` and `oci` replace their four copies of the head-only
  `drain` arithmetic with one `RetentionBuffer` per stream inside their `Captured`.
- The host driver's `drain_capped` gains a `retention` parameter. It stores into a `RetentionBuffer`
  when it has no live sender and the output is recorded. It writes `TRUNCATION_MARKER` only under
  `Head`; under the other modes the marker would overwrite kept bytes, and the counts already say
  what is missing. `ExecObservation` gains `stdout_kept` and `stderr_kept`.
- `HostDriver` gains `pub(crate) async fn start_exec_for_adapter(&self, id, root_name, input,
  OutputRetention)`, which also applies the registered-submodule rule of § 2. Only the `linux`
  adapter calls it, with the profile's value; `Driver::start_exec` keeps the default retention and
  no submodule rule. The `Driver` trait, `start_pipe_session` and every frame forwarded
  live are untouched. A session is always `Head`, because only a `pub(crate)` method takes a
  retention, and the daemon crate cannot name it.

`pod` keeps `Head` only. The log API's one byte bound is `limitBytes`, which keeps the first bytes.
`tailLines` counts lines, not bytes. Reading the whole log to keep its tail transfers an amount the
adapter does not bound, because the kubelet's log rotation is node configuration the adapter cannot
prove. A profile asking `Tail` or `HeadAndTail` on either stream is refused, in `check_profile`,
with the new `confinement::OUTPUT_RETENTION_UNSERVED` = `confinement.output-retention-unserved`
(501, class `Unserved`). Its `KeptOutput` reports `observed_bytes: None`.

The file-backed host-process log of
[ADR 0036](../../adr/0036-host-processes-are-a-supervised-resource-family.md) § S7
(`crates/substrate-host/src/host_process/log.rs`) also discards the oldest bytes and counts them.
It is not reused: it frames whole records into a file under the daemon's state directory, and these
runs keep bytes in memory.

**The bound and the default.** No bound constant changes, and no bound is raised. `io_bounds`
keeps lowering the bound where `limits` is served. A profile that does not set `output_retention`
gets `Head` on both streams. Its `stdout`, `stderr` and `*_truncated` are byte for byte what they are
today, the `linux` marker included.

## 7. The wire is untouched

- `crates/substrate-wire/src/lib.rs` gains no item. `ExecStartInput` is unchanged, and so its
  request hash is unchanged.
- `WorkspaceAccess::Scoped` has been published since bundle `0.12.0` (ADR 0023) and on the session
  start since `0.23.0` (design 27). The `linux` adapter now sends a value that already exists.
- Retention reaches the host driver only through `start_exec_retaining`, which is `pub(crate)`.
- `cargo xtask check-bundles` and `check-json` see no changed byte, and no `contracts/` directory is
  written.

Offering retention on `POST /v1/execs` would be a successor bundle, and it is out of scope. The exec
output route reads by `offset` from the stream's first byte (`ProcessRuntime::output`,
`ExecOutputQuery.offset`), and a tail record changes what an offset names. That is a contract
decision with its own consumers to notify, not a side effect of this story.

The command line `b10x-substrate confine run` gains no flag here. Its output is unchanged, because
it uses the default.

## 8. Tests

Each case runs its scenario once and finishes in seconds. Retention cases on `linux` lower
`io_bounds.output_bytes` to 4096 so they move little data. Each case goes into the adapter's
existing lane test file, and no new test binary is added. Each delegated case prints
`<name>: executed`, and `scripts/delegated-lane.sh` requires that line with `require_executed`, so
an absent case cannot read as green.

| # | case | lane | file |
|---|---|---|---|
| A1 | `linux`, `Scoped(["out"])`: the run writes `out/f`, and its writes to `src/f` and to `/workspace/f` fail; on the host `out/f` exists and the other two do not | delegated | `crates/substrate-host/tests/ess_confinement_linux.rs` |
| A2 | `linux`, `Scoped(["out"])`: the run's append to `.git/config`, `mkdir .git/hooks/x`, `mv .git g` and `rm -rf .git` all fail; the host's `.git` hashes the same before and after | delegated | `ess_confinement_linux.rs` |
| A3 | a subtree `.git`, `a/.git/b` or `.GIT` is `confinement.writable-subtree-git` (422) at admission, with its refusal record | portable | `crates/substrate-host/src/confinement/mod.rs` unit test |
| A4 | an unsorted, overlapping or `..` list is `exec.workspace-access-invalid` (422) at admission | portable | `mod.rs` unit test |
| A5 | `linux` on a snapshot without `exec.workspace-scoped-write` refuses `Scoped` `exec.workspace-scoped-write-unserved` (501). The check is a free function over `&CapabilityFacts`, so no driver opens. | portable (Linux build) | `linux.rs` unit test |
| A6 | `linux`, a subtree that is a symlink, is `exec.workspace-subtree-symlink` (422) at start, audited | delegated | `ess_confinement_linux.rs` |
| A7 | `local` refuses `Scoped` `exec.workspace-scoped-write-unserved` (501) | portable | `crates/substrate-host/tests/ess_confinement_local.rs` |
| A8 | `oci` and `pod` refuse it the same way, through their pure profile checks | portable | unit tests in `oci.rs` and `pod.rs` |
| A9 | `macos` refuses it the same way | macos | `crates/substrate-host/tests/ess_confinement_macos.rs` |
| A10 | `systemd` around `local` refuses it `exec.workspace-scoped-write-unserved` (501); around `linux` it serves `Scoped` with A1's assertions | delegated | `crates/substrate-host/tests/ess_confinement_systemd.rs` |
| B1 | `RetentionBuffer`: the properties of § 6, each mode over and at the bound, a chunk larger than the ring, `B = 1` | portable | `output_retention.rs` unit tests |
| B2, B3, B4 | `local`, one case per mode: a run prints `seq 1 300000` (about 2 MiB) to stdout; `Head` holds line 1 and not the last line, `Tail` the last line and not line 1, `HeadAndTail` both; `head_bytes`, `tail_bytes` and `observed_bytes` match the bytes the run wrote; nothing kept exceeds the bound | portable | `ess_confinement_local.rs` |
| B5, B6, B7 | `linux`, one case per mode, the bound at 4096. B7 sets stdout `HeadAndTail` and stderr `Tail` in one run, so each stream is shown to take its own mode. `Head` still ends in the marker. | delegated | `ess_confinement_linux.rs` |
| B8 | `systemd` around `local`, `Tail` | delegated | `ess_confinement_systemd.rs` |
| B9 | `macos`, `Tail` | macos | `ess_confinement_macos.rs` |
| B10 | `oci`, `Tail` | oci | `crates/substrate-host/tests/ess_confinement_oci.rs` |
| B11 | `pod` refuses `Tail` on stdout `confinement.output-retention-unserved` (501) through `check_profile` | portable | `pod.rs` unit test |

The default-unchanged property needs no new case. Every existing output case runs with the default,
and the `linux` marker test in `process.rs` keeps passing.

`tests/confinement-public-items.txt` gains these lines only: `WorkspaceAccess` re-exported, the two
profile fields, the two `RunOutput` fields, `Retention`, `OutputRetention`, `KeptOutput`, and the
constants `WRITABLE_SUBTREE_GIT` and `OUTPUT_RETENTION_UNSERVED`. No line is removed or reshaped.

Also changed, as documentation:

- the table at the top of `profile.rs`;
- the field table in `website/docs/guides/confinement-profiles.md`: `workspace_access` needs
  `workspace`, and `output_retention` needs `output_read`;
- the admission table in the `linux.rs` module header.

`cargo xtask check-docs` holds no profile-field table to the specification, and no family, fact or
lane table changes.

## 9. Decisions this note leaves alone

- ADR 0023: modes, lexical rules, descriptor binds, the probe. Reused, not changed.
- ADR 0044 and atlas ADR 0109: the families, the `Fact` set, the admission order and the suite.
  Nothing is added to the domain `substrate.confinement`.
- ADR 0016, ADR 0031 and design 23: live output, backpressure, terminal capture. Sessions keep
  `Head`.
- Invariant 6: no bundle is cut.

## 10. Open points

- **A confinement `Fact` for scoped write.** A consumer that chooses an adapter by its facts, as
  `crates/substrate-host/tests/consumer_surface.rs` does, cannot see scoped write before it admits.
  A `workspace_scoped_write` fact would let it. It is a breaking change to the exhaustive `Fact`
  enum, a domain change with its own scenario per adapter, and a site table change, so it is left to
  a later decision, not taken here.
- **ESS coverage.** Like `exec.workdir-unserved` and `exec.secret-slots-unserved`, the refusals here
  are raised by an adapter's `check`, which the domain's `AdmitProfile` does not model. They are
  held by the Rust cases of § 8, not by scenarios. Atlas ADR 0109 decision 7 asks for one scenario
  per promise and adapter. That gap predates this note.

## CHANGELOG (under `## [Unreleased]`, `### Added`)

- The confinement profile gains `workspace_access`, the wire's `WorkspaceAccess`, re-exported from
  `substrate_host::confinement`, default `ReadWrite` as before. `linux`, and `systemd` around it,
  serve `ReadOnly` and `Scoped` on a host whose probe proved `exec.workspace-scoped-write`: the
  workspace is bound read-only, and each named subtree is bound writable over it by descriptor, so a
  write elsewhere in the workspace and a write over `.git` fail inside the run. A subtree that is
  `.git` or lies beneath one is refused `confinement.writable-subtree-git` (422), and a malformed list
  `exec.workspace-access-invalid` (422). Every other adapter refuses a non-default access
  `exec.workspace-scoped-write-unserved` (501), and so does `linux` on a host without the fact.
  `sealed_paths` stays refused on `linux` (`confinement.fact-unserved`). The surface change is
  additive: a profile built with `..ConfinementProfile::default()` compiles unchanged, and only a
  struct literal that names every field must name the new one.
- The confinement profile gains `output_retention`, which sets `Head` (the default, unchanged),
  `Tail` or `HeadAndTail` for stdout and stderr separately, within the same byte bound per stream.
  `HeadAndTail` keeps the first half of the bound and the last half. `RunOutput` gains `stdout_kept`
  and `stderr_kept` (`KeptOutput`): the retention applied, the head and tail byte counts, and the
  bytes the stream produced when the adapter read it to its end. `local`, `linux`, `macos`,
  `systemd` and `oci` serve all three modes from one shared buffer. `pod` refuses `Tail` and
  `HeadAndTail` with `confinement.output-retention-unserved` (501), because the log API bounds a
  read only from its start. No bound changes, and a run that does not set it reports the same bytes
  as before.
