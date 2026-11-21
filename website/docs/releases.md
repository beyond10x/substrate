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

## 0.23.0 (2026-11-21)

### Added in 0.23.0

- The `linux` confinement adapter, and `systemd` around it, can hold a run to an executable
  allow-list (design 35). A profile with a non-empty `exec_subpaths` executes only its literal
  program, the host's ELF loader and what the entries name; any other `execve` fails inside the
  run with `EACCES`. The wire, the bundle and the daemon are unchanged: no wire start carries a
  list, and the daemon does not publish `confinement.exec-subpaths`.
  - The fact `exec_subpaths` is proved only where the embedder names the new helper
    `substrate-exec-gate` (package `b10x-substrate-exec-gate`) in the new
    `HostConfig.exec_gate`, and the open-time probe held a run to a list with it. It needs
    Landlock ABI 2 (Linux 5.19). Cargo does not build a dependency's binary: build the helper
    from the revision you pin, and install a copy, because cargo's own output has a second hard
    link and is refused.
  - **The helper must be statically linked**, for example
    `cargo build --release -p b10x-substrate-exec-gate --target x86_64-unknown-linux-musl`. A
    helper whose program headers hold `PT_INTERP`, which cargo's default build has, and a file
    that is not an ELF image of the driver's own machine with a readable program header table,
    are refused `config.exec-gate-invalid` at the open: a loader would
    run before the helper and read files a profile can mount, `/etc/ld.so.preload` among them.
    So is an image the kernel does not start (a type other than `ET_EXEC` and `ET_DYN`, a
    program header size that is not its class's, no program header or more than one 4 KiB
    page of them), which bubblewrap would hand to a shell.
    A start with a list and a controlling terminal is refused `session.pty-unserved`, and one
    with a declared mount at or beneath `/.substrate-exec-gate`, where the helper is bound, by
    the code its kind has for a mount the driver owns.
  - An entry admits a file by where the file itself lies: a symbolic link in a listed
    directory that points outside it is not admitted. On Ubuntu 26.04 `ln`, `env`, `cat`, `ls`
    and `mkdir` in `/usr/bin` are such links, into `/usr/lib/cargo/bin/coreutils`.
  - On a start with a list the run's variables reach the run's program and no program before
    it: the driver gives them to the helper as `--env <name> <value>` operands and gives
    bubblewrap no `--setenv`, and the helper refuses to run when a variable other than `PWD`
    is in its own environment. No variable is refused by name. An object that `LD_PRELOAD`
    names is still loaded into every allowed dynamic program; an `execve` it makes of an
    unlisted path fails.
  - **An empty list differs by adapter.** On `linux` it runs every program of the run's view, as
    before; on `macos` it runs only the program.
  - An entry is a path of the run's view, not a host path: relative beneath `/workspace`,
    absolute beneath `/usr`, `/bin`, `/lib`, `/lib64`, `/workspace` or the mount of a declared
    root or file. It must exist at the start. The program is an absolute path when the list is
    not empty.
  - Limits: an allowed interpreter runs any script it can read, the loader run as a program
    loads any file, and a listed directory does not reach through a second mount of a part of
    it, so a profile lists `/usr/bin` and not `/usr`.
  - New refusals: `config.exec-gate-invalid` (422) and `config.exec-gate-unserved` (501) at the
    open, and `exec.sandbox-place-unserved` (501) at the start of a profile with a list.
    `exec.sandbox-path-unrepresentable` and `exec.sandbox-exec-subpath-undeclared` are new on
    `linux`; the first also answers a program, a loader, an entry or a writable mount whose
    path in the run's view is longer than 4095 bytes.
  - Write `workspace_root` and every `writable_roots` path without `..` and `.`: with
    `exec_gate` set, such a path that does not exist yet is refused at the open,
    `config.exec-gate-unserved` with the cause `path-unopenable`.
  - Whether one path lies in another is decided from the mount table, on `ext4`, `overlay` and
    `tmpfs`. With `exec_gate` set and a path of the open on another file-system type, the
    adapter opens without the fact; `AdapterFacts::withheld` and the `confinement.fact-unserved`
    refusal name the cause, the path and the type.
  - When a base directory (`/usr`, `/bin`, `/lib`, `/lib64`) shows a workspace, the `linux`
    adapter opens without the fact `exec_subpaths` and its proof names no cause; every start
    with a list is still refused `confinement.fact-unserved` before dispatch. Naming the cause
    is planned as `story:linux-adapter-names-why-the-list-fact-is-absent`.

### Changed in 0.23.0

- `confinement::linux::proof` takes a second argument, `&ExecListProof`.
  `ConfinementError::FactUnserved` gains the field `withheld`, and `AdapterFacts` the public
  field `withheld`, serialized only when it is not empty; both hold the new
  `confinement::FactWithheld`. An embedder that builds the variant, or names both of its fields
  in a pattern, changes one line.

### Fixed in 0.23.0

- The executable allow-list's checks of what a run can write follow the mounts (design 35
  D30). The defect is on `main` since pull request 241 (2026-11-21), with the allow-list
  itself, and **no release contains it**. The `linux` adapter asked whether something lies in
  a location a run can write by that location's own place alone, so a file system mounted
  beneath `workspace_root`, a declared writable root or a workspace counted as outside it,
  although a run that is given the location writes what is mounted there. Four checks had the
  gap:
  - a read-only root declared on such a mount, or one that holds a mount on which a declared
    writable root lies, was classed as safe to follow. A symbolic link a run had left there,
    listed by a later profile, admitted what it named: all of `/usr/bin` for a link to it;
  - the helper `substrate-exec-gate` installed on such a mount passed the open;
  - an entry that resolved onto such a mount was not refused by the driver, nor by the helper
    for a mount beneath a writable mount of the run's view.

  Each now asks the places the location reaches: its own, and the root of every mount that
  is mounted beneath it, read from the parent IDs of the mount table. Where that cannot be
  established for a location a run can write, the answer is the one a place has, with the
  mount point in its text, `cannot establish where <mount point> is attached: <cause>`:
  `exec.sandbox-place-unserved` (501) at a start with a list, `config.exec-gate-unserved`
  (501) at the open, and status 125 from the helper. A tenth cause, `mount-covered`, names a
  mount beneath such a location that a later mount hides or that lies under a stack; it
  refuses the open. The host directory of a read-only root, and `/usr`, `/bin`, `/lib` and
  `/lib64`, are never refused for a mount beneath them while they show nothing a run can
  write: such a mount is counted by what the mount table says it shows, which can only make
  the directory count as one a run can write. A directory that does count as one is asked
  like every other location a run can write, and a mount beneath it that cannot be compared
  refuses the start `exec.sandbox-place-unserved` (501) with the cause and the field the
  mount was declared in, before anything is dispatched.
  A start without a list reads no mount table, as before.
  The cases that stage a mount ran on a workstation only: a GitHub-hosted runner grants a user
  namespace no mount, and its confinement lane lists them as absent, not passed; the cases on
  synthetic mount tables and the cases without a mount ran there.
- **A symbolic link on a file system of a type other than `ext4`, `overlay` and `tmpfs` is
  no longer followed.** Also on `main` since pull request 241, and in no release. A mount of
  such a type is compared with nothing, so a link a run had left on one, mounted beneath a
  workspace and again beneath a read-only root, was followed out of that root: a profile
  that listed the link ran what it named. Every symbolic link that a listed path, the
  program or the loader resolves through is now placed like the path's end, by the driver
  before dispatch and by the helper in the run's view. A link that has no place refuses the
  start `exec.sandbox-place-unserved` (501) with the cause, `file-system-unlisted` and the
  type for such a mount, and the helper exits 125; a link in a place a run can write is
  refused as before, and now by the helper as well, which checked only where a path ends.
  The same file system of such a type mounted beneath a workspace and beneath a read-only
  root also makes that root count as one a run can write.
- **A listed path that passes through a directory a run can write is refused**, also where
  it ends outside every such directory: a link in a read-only root to `/workspace/../usr/bin`
  is such a path. Also on `main` since pull request 241, and in no release. A run that
  shares the directory could rename what it holds while the helper resolved the path, and
  move the path's end to a directory nothing had judged. The driver refuses the start
  `exec.sandbox-path-unrepresentable` (422) and the helper exits 125; a directory on the way
  that has no place refuses like a link that has none. The helper no longer asks the kernel
  for `..`: it keeps the directories of its walk open and steps back to the one before, as
  the driver resolves the same text.
- **Every entry and the program are placed where they end, whatever the mount they lie
  in.** One in a workspace or another mount a run can write was opened and not placed, so an
  entry that ended on a file system of a type other than `ext4`, `overlay` and `tmpfs`
  mounted there was admitted. It is refused `exec.sandbox-place-unserved` (501) now, and by
  the helper with status 125.
- `substrate_exec_gate::place`: `Unestablished` gains the public field `asked` (the new
  `Asked`) and `Cause` the variant `MountCovered`. New and public, for the host driver:
  `MountTree` (`table`, `reaches`, `reaches_as_written`), `Reach` (`holds`, `overlaps`),
  `Unlisted` and `MountTable::into_tree`. `Reach::places` and
  `MountTable::into_tree_as_written` are public for test code and hidden from the
  documentation. `Relation`, `Place::relation`, `Place::lies_in`, `MountTable::lies_in` and
  the free function `place`, which pull request 241 made public and no release contains,
  have no user outside the module and are private to the crate again.
