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
  `sealed_paths`, `exec_subpaths`, `exec_denied_paths`, `private_tmpdir`, `host_network`,
  `image_digest_pinned`.
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
| `workspace_access` other than `ReadWrite` | `workspace` |
| `output_retention` other than `Head` on both streams | `output_read` |
| `writable_roots`, `read_only_roots`, `read_only_files` | `filesystem_view` |
| `sealed_paths` | the fact `sealed_paths` |
| `exec_subpaths` | the fact `exec_subpaths` |
| `exec_denied_paths` | the fact `exec_denied_paths`, and an `exec_subpaths` that is not empty |
| `private_tmpdir` | the fact `private_tmpdir` |
| `network: Egress` | `network_egress` |
| `network: Isolated` | `network_egress`, on an adapter that proves `host_network` |
| `ipc.socket_allowlist` | the fact `ipc_socket_allowlist` |
| `ipc.signals_contained` | the fact `ipc_signals_contained` |
| `process_bounds`, `io_bounds` | `limits` |
| `stdin: Piped` | `process_run` and the fact `stdin_pipe` |
| `output_delivery: Live` | `output_read` and the fact `output_live`; `Head` retention on both streams |
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

`workspace_access: Scoped` names the subtrees a run may write; the rest of the workspace is
read-only. `linux` and `systemd` around it serve it. The run cannot write the root's `.git`, and a
subtree that is `.git`, lies beneath it or holds a submodule the root's `.gitmodules` registers is
refused (`confinement.writable-subtree-git`). Limits: an unregistered nested `.git` stays writable,
and host Git run inside a subtree reads a repository the run created there, so run it in the root.
A hard link made on the host before the run stays writable through a subtree; the run cannot
create one (`EXDEV`).

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
below. Past its first seven rows, that is `confinement.family-unserved` naming the first family of
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

## Executable allow-lists on linux

`exec_subpaths` names what a run may execute. The two adapters that prove the fact differ on an
empty list, and the difference is deliberate:

| `exec_subpaths` | `linux` | `macos` |
|---|---|---|
| empty | every program of the run's view runs | only the program runs |
| not empty | the program and what the entries name | the program and what the entries name |

On `linux` the fact needs a helper. Build `substrate-exec-gate` from the revision you pin the
host crate at, install a copy, and name it in `HostConfig.exec_gate` as an absolute path. Without
it the adapter opens and a profile with a list is `confinement.fact-unserved` (501).

An entry names a directory, which admits every file beneath it, or one file:

| Entry | On `linux` |
|---|---|
| relative | beneath `/workspace` |
| absolute | beneath `/usr`, `/bin`, `/lib`, `/lib64`, `/workspace`, or the mount of a declared root or file |
| anywhere else, such as `/tmp` | `exec.sandbox-exec-subpath-undeclared` (422) |
| absent when the run starts | `exec.sandbox-path-unrepresentable` (422) |
| a symbolic link where a run can write | `exec.sandbox-path-unrepresentable` (422) |
| a link that resolves into a writable mount | `exec.sandbox-path-unrepresentable` (422) |
| a link whose target passes through a writable mount | `exec.sandbox-path-unrepresentable` (422) |

- Entries are paths the run sees, not host paths. A toolchain in a home directory is declared as
  a read-only root, and the entry names its mount.
- The program is an absolute path when the list is not empty.
- List the directory that holds the programs, not one above it. With `/usr` listed, `/bin/sh` is
  denied on a host whose `/bin` is a link into `/usr`; with `/usr/bin` listed both names run.
- An entry admits a file by where the file itself lies. A symbolic link in a listed directory
  that points outside it is not admitted. On Ubuntu 26.04, `ln`, `env`, `cat`, `ls` and `mkdir`
  in `/usr/bin` are links into `/usr/lib/cargo/bin/coreutils` and are denied with only
  `/usr/bin` listed; `mv`, `cp`, `rm` and `true` are links to `gnu*` names in `/usr/bin` and
  run. List the directory the tools really lie in as well, or the tool itself.
- A declared root or file mounted at or beneath `/.substrate-exec-gate` is refused when the
  profile has a list: the helper is bound there.
- Create `target` or `node_modules` before the start: an entry must exist.
- The list limits which files are executed. An allowed interpreter still runs any script it can
  read, and the loader run as a program loads any file.
- The run's variables are not filtered. An object that `LD_PRELOAD` names is loaded into every
  allowed dynamic program and runs its code there; an execution it makes of an unlisted path
  still fails. Leave such names out of `Command.env` where that is not wanted.
- An entry, or the program, whose path in the run's view is longer than 4095 bytes is refused
  `exec.sandbox-path-unrepresentable` (422).

Whether one path lies in another is decided from the host's mount table, on `ext4`, `overlay`
and `tmpfs`. Where that cannot be established the answer names one of ten causes:
`mount-table-unreadable`, `mount-table-unparsable`, `path-unopenable`, `mount-id-unreported`,
`mount-row-missing`, `mount-row-repeated`, `mount-root-deleted`, `mount-point-mismatch`,
`mount-covered` or `file-system-unlisted` with the type.

A file system mounted beneath a location a run can write (`workspace_root`, a declared writable
root, a workspace) lies in that location: a read-only root declared on it counts as one a run
can write, and a helper installed on it is refused. `mount-covered` names such a mount that a
later mount hides, or two mounts on one mount point: unmount one, or declare another directory.

A listed path is refused where it ends on another file-system type, reads a symbolic link
there or passes through a directory there, with `file-system-unlisted`.

A read-only root that shows a location a run can write is one itself from then on. A mount
beneath it that cannot be compared refuses the start, as beneath a workspace.

One limit: a file system that shows under two device numbers, such as a network file system
mounted twice with different options or two FUSE mounts of one directory. The comparison of
mounts cannot see that a read-only root and a writable location hold the same files. The only
guard is that nothing on a file system other than `ext4`, `overlay` and `tmpfs` is followed.

| Where | Cause | Answer |
|---|---|---|
| opening the adapter with `exec_gate` set | `file-system-unlisted` or `mount-id-unreported` | the adapter opens without the fact, and `facts().withheld` names the cause, the path and the type |
| opening the adapter with `exec_gate` set | any other | `config.exec-gate-unserved` (501) |
| starting a profile with a list | any | `exec.sandbox-place-unserved` (501) |

Where the fact is withheld, a profile with a list is `confinement.fact-unserved` (501) and its
text repeats the cause. A profile without a list asks for no place and runs as before.

## Executable deny-lists

`exec_denied_paths` names what a run never executes, even beneath an `exec_subpaths` entry.
`linux`, `systemd` around it and `macos` prove the fact `exec_denied_paths`. Every other adapter
refuses the profile `confinement.fact-unserved` (501).

- An entry has the form of an `exec_subpaths` entry: a file or a directory, relative beneath
  the workspace or absolute in the same locations. A profile holds at most 64. An entry may
  be absent when the run starts.
- A denied path wins over a listed directory that holds it, over an entry that names it and
  over the program. A start whose program is denied is `exec.sandbox-program-unlisted` (422).
- A deny-list needs an allow-list. Without `exec_subpaths` the profile is
  `confinement.exec-denied-paths-without-subpaths` (422) on every adapter.
- What fails is `execve` of the path. The loader given a denied program runs it, and `sh` given
  a denied script runs it.
- A run that can write a listed directory copies a denied program there and runs the copy. List
  no directory the run can write beside a deny-list.

On `linux` a denied program is found by where its file lies, not by the text of a path, and
across mounts. Where a mount beneath a denied directory, or beneath a listed directory of a
profile with a deny-list, cannot be compared, the start is `exec.sandbox-place-unserved` (501):
`mount-covered`, `mount-root-deleted`, and `file-system-unlisted` beneath a denied directory or
as an entry of a directory that holds a denied path. For that last case, what decides is where
the deny entry is, not the mount: the helper expands only the directories on the way to a deny
entry.

| A mount of a type other than `ext4`, `overlay` and `tmpfs` | The start |
|---|---|
| at `k/vol`, with `k` listed and `k/tool` denied: `k` is on the way to the deny entry, and its entries are walked one by one | refused |
| at `far/in/vol`, with `far` listed and `far/tool` denied: `far/in` is not on the way, and gets one rule | runs |
| beneath a listed directory of a profile without a deny-list | runs |

| A denied program reached through | Answer |
|---|---|
| a symbolic link, anywhere | denied |
| a second mount of its directory, such as `/bin/id` for `/usr/bin/id` | denied |
| a file system mounted beneath a listed directory, or a listed mount beneath a denied directory | denied |
| a hard link in the same directory | denied |
| a hard link that existed before the start in another listed directory | runs |
| a copy in a listed directory the run can write | runs |
| a new file the run writes into the listed directory that holds the deny entry itself: `copied 126` | denied |
| a file the run writes into a directory inside it that held no denied path at the start (`new file in a directory of the expanded one 0`), also once the run renamed that directory so that the file has the denied path (`copied after a rename 0`) | runs |
| the name of an entry that was absent at the start, given to a listed program by a hard link (`linked 0`), a rename (`renamed 0`) or a symbolic link | runs |

- A listed directory that holds a denied path is expanded: one rule per file in it, and one per
  directory in it that holds no denied path. A new file in the expanded directory itself has
  no rule, and a program removed and written again there no longer runs. Put no deny entry in
  a directory a build writes its programs into.
- **In a listed directory a run can write, a deny entry holds for the files that exist at the
  start, under the names they have then.** A directory inside an expanded directory that holds
  no denied path gets one rule: what the run creates in it executes, and the rule moves with a
  rename. So a file the run creates or moves can run under any name, the denied name included.
  Nothing runs that the run could not already run by copying it into a listed directory it can
  write. **A deny-list is exact only where the run cannot write.** The figures in the table
  are measured statuses.
- A layer of an overlay is another file system to the kernel, and a program denied in the
  overlay runs from the layer. So never list a lower directory of an overlay that is also in the
  view.
- A start with a deny-list reads every directory it expands and writes at most 65536 rules.
  Where a directory cannot be read, or beyond that number, the run ends with status 125 and
  nothing runs. An earlier run causes the first by taking the read permission from a
  workspace directory. Standard error is then one line a consumer may match:
  `substrate-exec-gate: cannot read the directory "<path>": <cause>`, with the path in the
  run's view as a quoted string and the system's cause, such as
  `Permission denied (os error 13)`. A program can print that line and exit 125 itself: it
  explains an end and proves none.

On `macos` the entries are Seatbelt deny rules after every allow. A hard link to a denied
program under another path runs. **A limit on `macos`:** the deny rules were read and run only
in the hosted macOS check, never on a workstation, and how Seatbelt matches a deny entry
written in another letter case on a case-insensitive volume is not verified.

## Build with the macOS developer tools

Inside the Seatbelt policy, the `/usr/bin/cc` and `xcrun` shims cannot find the compiler or the
SDK: they ask `xcodebuild` through `/bin/sh`, which no exec subpath allows. So `macos` resolves the
selected developer tools once, when it opens. It runs `/usr/bin/xcrun --sdk macosx` outside the
sandbox for the SDK path, `clang` and `ar`, with a cleared environment (`PATH=/usr/bin:/bin` and
nothing else) and `/` as its working directory, so no `DEVELOPER_DIR` or `TOOLCHAINS` of the
embedding process chooses the answer. Each answer must be an absolute path that resolves beneath
the selected developer tools. On a host without developer tools, `xcrun` is not run.

A start then gets these variables, each only when its path lies beneath one of the profile's
`exec_subpaths`, and only when the command's environment does not name it:

| Variable | Value |
|---|---|
| `SDKROOT` | the macOS SDK |
| `CARGO_TARGET_AARCH64_APPLE_DARWIN_LINKER` | `clang` |
| `CARGO_TARGET_X86_64_APPLE_DARWIN_LINKER` | `clang` |
| `CC` | `clang` |
| `AR` | `ar` |

A confined `cargo build` or `cargo test`, and a build script that compiles C, then link without
reaching the shim. A linker variable outranks a linker a cargo configuration file names; to use
another, set the variable in the command's environment. Nothing is resolved again at a start, and
the Seatbelt policy gains no rule.

A start whose exec subpaths reach the developer tools is refused
`exec.sandbox-developer-tools-unresolved` (501), naming why, before anything runs, when:

- the open could not resolve the SDK, `clang` or `ar`;
- other developer tools are selected than at open, including when the open found none;
- the SDK the open resolved is no longer a directory beneath them;
- the exec subpaths hold the selected developer directory (an Xcode bundle's `Contents/Developer`,
  or the Command Line Tools directory) but not a value it needs.

## Refusals

Admission refuses in this order and names the first refusal it meets:

| Order | Code | Status | When |
|---|---|---|---|
| 1 | `confinement.reserved-field-unserved` | 501 | a reserved field is set |
| 2 | `confinement.exec-denied-paths-without-subpaths` | 422 | a deny-list and an empty `exec_subpaths` |
| 3 | `confinement.exec-denied-paths-exceeded` | 422 | more than 64 deny entries |
| 4 | `confinement.adapter-unserved` | 501 | the adapter did not open, or the profile's `inner` is not the adapter it wraps |
| 5 | `confinement.local-reason-missing` | 422 | a declared-insecure adapter and no reason |
| 6 | `confinement.image-unpinned` | 422 | a digest-pinning adapter and an image not named by digest |
| 7 | `confinement.local-refused` | 403 | a declared-insecure adapter the host's policy forbids |
| 8 | `confinement.family-unserved` | 501 | the first family of `needed` not served |
| 9 | `confinement.fact-unserved` | 501 | the first required fact not proved |

`error.code()` and `error.status()` return the code and status. A refusal from the host itself,
met while starting, keeps its own code, such as `exec.workdir-missing`.

A start or a render whose profile names no `workspace` is `confinement.workspace-required` (422)
on every adapter that runs the program in a host directory. `pod` runs it in its own `emptyDir` at
`/workspace` instead.

The host's operator can forbid `local` by setting `SUBSTRATE_CONFINEMENT_LOCAL` to any value other
than `permit`; an embedder can only tighten that, with `LocalPolicy::Refuse`.

Read [confinement and refusal](../concepts/confinement.md) for the guarantees the `linux` adapter's
host driver enforces.
