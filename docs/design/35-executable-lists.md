# Design 35: an executable allow-list and deny-list on the confinement adapters

Status: draft under review, 2026-11-21. Serves `story:linux-executable-allow-list` and
`story:executable-deny-list` (`epic:runs-are-governed-and-limit-hits-are-events`). It changes no
wire byte and no contract bundle: the lists are substrate-host's confinement API
([ADR 0044](../../adr/0044-confinement-is-a-contract-family-served-by-declared-adapters.md)
decisions 7 and 8). No code before acceptance (invariant 8). § 17 lists the decisions, § 18 every
claim that is inferred and not observed, § 19 the questions for the operator.

Revised 2026-11-21 after the first review. Session `sdc` decided the helper (D5, D19) under
`approval-record:sdc-decides-plan-and-design-2026-11-21`. Three security defaults (D1, the memfd
limit of D9, the hard-link limit of § 8.5) are the operator's to decide and are written here as
provisional, pending the operator (§ 19).

Two asks, from a consumer that runs every process through substrate-host:

1. A run on the `linux` adapter executes only its program and the programs beneath listed paths.
   Today only `macos` enforces `exec_subpaths`.
2. A profile names programs that never run, even beneath a listed path. Today no adapter has such
   a list.

**What the lists are for.** The lists are for an honest tool chain: they keep an agent's tools to
the ones a step declares, and they make a wrong tool call fail loudly. Containment of hostile code
stays with the namespaces, the seccomp program and the read-only view. The lists are not a
boundary, and four probed limits say why (§ 3.1, § 7):

- an allowed interpreter runs any script it can read (A7);
- the dynamic loader, started directly, runs any program file the run can read (A8);
- a memfd copy of a program runs (A9);
- mapping a file as code is not execution, so the list does not see it (A10).

## 1. Evidence: what the tree does at `5d5b192db`

| what | where |
|---|---|
| `ConfinementProfile.exec_subpaths: Vec<String>`, "subpaths exec is admitted beneath, beyond the literal program"; a non-empty list implies the fact `exec_subpaths` | `crates/substrate-host/src/confinement/profile.rs:72`, `:216` |
| the `Fact` order has 13 members; `exec_subpaths` is the eighth, before `private_tmpdir` | `crates/substrate-host/src/confinement/facts.rs:28-42` |
| admission refuses a reserved field first, then names the first required fact the adapter did not prove | `crates/substrate-host/src/confinement/mod.rs:588`, `:631-637` |
| `linux` does not prove `exec_subpaths`; its proof is a function of the driver's capability snapshot alone | `crates/substrate-host/src/confinement/linux.rs:29-31`, `:166-214` |
| `macos` allows `process-exec` for the literal program, and for each exec subpath `file-read*`, `process-exec` and `file-map-executable` by `subpath`. With no entry only the program executes | `crates/substrate-host/src/seatbelt.rs:262-277`, `crates/substrate-host/src/confinement/macos.rs:3-5`, `:574` |
| on `macos` a relative entry is beneath the workspace, need not exist, and is refused when a part of it is a symbolic link; an absolute entry is resolved and must lie in the workspace, a declared root or a system or developer directory | `macos.rs:696-760` |
| `macos` requires an absolute program | `macos.rs:613-621` |
| Seatbelt applies the last matching rule, so the sealed-path denies are rendered after every allow | `seatbelt.rs:13-14`, `:291-313` |
| `macos` proves `exec_subpaths` by a probe: under a policy with `/usr/bin`, `/usr/bin/true` runs and `/bin/echo` does not | `seatbelt.rs:485-489` |
| the `linux` run's view is built by bubblewrap: `/usr`, `/bin`, `/lib`, `/lib64`, `/etc/alternatives`, `/proc`, `/dev`, a tmpfs `/tmp`, declared roots and files at their mounts, the workspace at `/workspace` | `crates/substrate-host/src/process.rs:2723-2807` |
| inside the view the program is started by `/usr/bin/env -u PWD --`, after `/usr/bin/setsid --ctty` for a terminal | `process.rs:2844-2854`, `crates/substrate-host/src/pty.rs:44` |
| the driver's `pre_exec` runs before bubblewrap: it places descriptors and sets `no_new_privs` | `process.rs:1213-1223` |
| the seccomp program refuses a foreign architecture, x32, `io_uring_setup` and argument checks on `socket`, `socketpair` and `ioctl`, and allows everything else | `crates/substrate-host/src/seccomp.rs:160-192` |
| adapter-only start parameters travel in `AdapterStart`, beside the wire's `ExecStartInput` | `process.rs:218-232`, `crates/substrate-host/src/lib.rs:1234-1255` |
| a probe proves a mechanism in a throwaway sandbox with the floor's argv, and its fact is absent unless the run's script succeeded | `crates/substrate-host/src/probe.rs:323-399` |
| the bubblewrap binary's path, hash, device and inode are bound into the snapshot and rechecked at every start | `probe.rs:25-34`, `process.rs:2657-2669` |
| `publish_confinement` sets `confinement_exec_subpaths` to `None`; a test holds the published set equal to `linux::proof` | `probe.rs:269-317`, `:1149-1220` |
| the published daemon image builds bubblewrap 0.11.2 and installs Ubuntu 24.04's `util-linux` | `Dockerfile` (`sandbox-builder`, `execution-prerequisites`) |
| the confinement model has two commands, `OpenAdapter` and `AdmitProfile`; it has no command that runs a program | `spec/confinement/confinement.yaml:227`, `:305` |
| the delegated lane runs on hosted `ubuntu-24.04` runners | `.github/workflows/release-lanes.yml:65` |

## 2. Settled before this document

These are given. This document states them and does not reopen them.

- **Invariant 3.** Where a probe did not prove the mechanism, the fact is absent and a profile that
  needs it is refused `confinement.fact-unserved`. Nothing runs with a weaker list.
- **The daemon does not publish `confinement.exec-subpaths` in the first story.** `linux::proof`
  proves the fact; `publish_confinement` withholds it, and the probe test states that one
  exception by name. A wire start has no field for the list, so a client that required the
  property would be admitted and its run would execute anything.
  `story:exec-start-carries-executable-lists` adds the field and lifts the exception.
- **A deny-list never stands alone.** A profile with a deny-list and an empty `exec_subpaths` is
  refused by name on every adapter (§ 8.4).
- **The new fact sits directly after `exec_subpaths` in the `Fact` order.**
- **No story promises a record of a denied execution.** § 10 states what the adapter observes,
  and § 11 what could observe one; neither designs a record.
- **`systemd` around `linux` takes `linux`'s set.** `systemd` around `local`, `local`, `oci` and
  `pod` prove neither fact in these two stories.

## 3. Probes

Run on 2026-11-21 on a workstation: kernel `6.18.55-1-MANJARO`, `/sys/kernel/security/lsm` =
`capability,landlock,lockdown,yama,bpf,apparmor`, bubblewrap 0.13.0, util-linux 2.42.4. The
kernel reports Landlock ABI 7 (`landlock_create_ruleset(NULL, 0,
LANDLOCK_CREATE_RULESET_VERSION)`).

Small C programs did the work, compiled with `slot cc` and kept outside the repository. Two of
them serve § 3.1 to § 3.5; § 3.6 describes a third:

- a **helper**: creates a ruleset that handles `LANDLOCK_ACCESS_FS_EXECUTE` (and, with a flag,
  `LANDLOCK_ACCESS_FS_REFER`, granted on `/`), adds one `LANDLOCK_RULE_PATH_BENEATH` rule per
  allowed path through an `O_PATH` descriptor, sets `no_new_privs`, calls
  `landlock_restrict_self` and executes its command. Given deny paths, it expands an allowed
  directory that holds one, as § 8.2 describes;
- a **static tool** that makes one system call and prints its `errno`: `execve`, `execveat` with
  `AT_EMPTY_PATH`, `fexecve` of a memfd copy, `rename`, `link`, `mmap` with `PROT_EXEC`, `mount`.

"Inside the sandbox" means under the floor's argv (`--unshare-user --disable-userns
--unshare-ipc --unshare-pid --unshare-net --unshare-uts --new-session --die-with-parent
--clearenv`, the base binds, `--proc`, `--dev`, `--tmpfs /tmp`), a scratch directory bound at
`/workspace`, and the helper bound read-only at a path of its own.

### 3.1 What the Landlock execute right covers

| # | probe | observed |
|---|---|---|
| A1 | `execve` of a dynamically linked program in an allowed directory, no rule for its loader | `EACCES` |
| A2 | the same with one file rule for `/usr/lib/ld-linux-x86-64.so.2` | runs; `libc.so.6` needs no rule |
| A3 | a static program in an allowed directory, no loader rule | runs |
| A4 | `execve`, and `execveat(AT_EMPTY_PATH)`, of a program outside the list | `EACCES` (13) |
| A5 | a `#!` script inside the list whose interpreter is outside it | `EACCES` |
| A6 | a `#!` script outside the list whose interpreter is allowed | `EACCES` |
| A7 | allowed `sh` given a script outside the list as an argument | the script runs |
| A8 | the allowed loader given a program outside the list as an argument | the program runs |
| A9 | `fexecve` of a memfd holding a copy of a program outside the list | the program runs |
| A10 | `mmap(PROT_READ \| PROT_EXEC)` of a file outside the list | succeeds |
| A11 | a symbolic link inside the list to a program outside it | `EACCES` |
| A12 | a symbolic link outside the list to a program inside it | runs |
| A13 | a hard link inside the list to a program outside it | runs |
| A14 | a rule on one file; its sibling has none | the file runs, the sibling is `EACCES` |
| A15 | a shell starts a denied program | `Permission denied`, status 126 |
| A16 | the run's program lists its descriptors after the helper executed it | no ruleset descriptor: it is close-on-exec |

### 3.2 Rename and link under the ruleset

| # | probe | observed |
|---|---|---|
| R1 | ruleset handles execute only: `rename` within one directory | succeeds |
| R2 | the same ruleset: `rename`, and `link`, between two directories, neither allowed | `EXDEV` (18) |
| R3 | ruleset handles execute and refer, refer granted on `/`: the calls of R2 | succeed |
| R4 | R3's ruleset: `rename`, and `link`, from a directory outside the list into an allowed one | `EXDEV` |
| R5 | R3's ruleset: `rename` from an allowed directory to one outside the list | succeeds |
| R6 | R3's ruleset: `mv` from outside the list into an allowed directory | succeeds: coreutils copies |

R2 is what `landlock.h` states: the refer right "is always denied by default, even when its bit
is not set in `handled_access_fs`" (`/usr/include/linux/landlock.h:25-28`).

### 3.3 Where the ruleset can be applied

| # | probe | observed |
|---|---|---|
| W1 | ruleset applied first, every path under `/` executable, then bubblewrap with the floor's argv | `bwrap: Failed to make / slave: Operation not permitted`, exit 1 |
| W2 | the same bubblewrap argv without a ruleset | exit 0 |
| W3 | inside the sandbox, after `/usr/bin/env -u PWD --`: helper with `/usr/bin` and the loader, then `sh` | `/usr/bin/true` runs; a workspace program, the static tool and the helper itself are `Permission denied`, 126 |
| W4 | inside the sandbox: `/workspace/bin` allowed | `bin/tool` runs, `other/tool` is denied; a file copied into `bin` runs |
| W5 | inside the sandbox: only `/usr/bin/bash` and the loader allowed | `sh` runs its builtins; `/usr/bin/true` is denied |
| W6 | `mount(2)` inside the sandbox with no ruleset | `EPERM`: the run could not mount before either |
| W7 | helper bound through `--ro-bind-fd`, and copied in through `--perms 0555 --ro-bind-data` | both run |
| W8 | a descriptor opened by the caller, written by a program inside the sandbox | arrives: bubblewrap passes it through |
| W9 | helper given an entry that does not exist | prints the error, exits 3, executes nothing |
| W10 | copies of the tool in `/tmp` and `/dev/shm` | `Permission denied`: not in the list |
| W11 | an allowed directory removed and created again by the run | a program in the new directory is `Permission denied` |
| W12 | 20 starts each: no helper; helper with 3 rules; helper with `/usr/bin` expanded around one denied program (5953 rules) | 3 ms, 3 ms and 20 ms per start |

### 3.4 Denying one program beneath an allowed directory

| # | probe | observed |
|---|---|---|
| N1 | `/usr/bin` allowed, `/usr/bin/id` denied by expansion | `id` is denied, `true` runs, a workspace symbolic link to `id` is denied, `cat /usr/bin/id` still reads 51512 bytes |
| N2 | a workspace directory allowed, one program in it denied | the program and its hard link in the same directory are denied; a sibling runs; a file created there later is denied |
| N3 | the denied program linked by the run into another allowed directory | `link` is `EXDEV`; a copy made there runs |
| N4 | a hard link to the denied program that existed in another allowed directory before the start | runs |
| N5 | a directory denied beneath an allowed one | programs beneath it are denied |
| N6 | a deny entry that does not exist at the start, created by the run | denied |
| N7 | deny entry `/usr/bin/sh`, a symbolic link to `bash` | `sh` and `bash` are both denied |
| N8 | the same path allowed and denied | denied |
| N9 | no Landlock: an empty file of mode 0444 bound over `/usr/bin/id` (`--ro-bind`, and `--perms 0444 --ro-bind-data`) | `id` is `EACCES`; the file reads as 0 bytes |
| N10 | N9 over a path that does not exist beneath read-only `/usr` | `bwrap: Can't create file …: Read-only file system`, exit 1 |

### 3.5 Other tools and versions

| # | probe | observed |
|---|---|---|
| V1 | `bwrap --help` of 0.13.0, and the option table of the 0.11.2 source (the tarball the `Dockerfile` pins, same SHA-256) | no option mounts `noexec`. 0.11.2 has `--ro-bind-fd`, `--bind-fd`, `--perms`, `--ro-bind-data`. A bind adds `MS_NOSUID`, `MS_NODEV` and `MS_RDONLY` only (`bind-mount.c:450`) |
| V2 | util-linux 2.42.4 `setpriv --landlock-access fs:execute,refer --landlock-rule path-beneath:…` inside the sandbox | applies the same ruleset; an entry it cannot open is exit 1 with a message |
| V3 | `setpriv --help` in a cached `ubuntu:24.04` image (util-linux 2.39.3) and `ubuntu:26.04` (2.41.3) | 24.04 has no Landlock option; 26.04 has |
| V4 | `/usr/bin` on the probe host | 6987 names; 106 regular files have more than one link |

### 3.6 Seeing a denial from outside the run

Added for § 11. A third C program was the observer: it either tries to join the kernel audit
log's read group, or runs a command under a seccomp user-notification filter on `execve` and
`execveat`, reads each path from the caller's memory and lets the call continue.

| # | probe | observed |
|---|---|---|
| O1 | helper with `LANDLOCK_RESTRICT_SELF_LOG_NEW_EXEC_ON`, one denied execution in the sandbox, then `journalctl -k`, `journalctl _TRANSPORT=audit` and `dmesg` as the same user | the denial happened (status 126). No journal line since the run names `landlock` or `audit`; `dmesg` is `Operation not permitted`. `auditd` and `systemd-journald-audit.socket` are inactive on the host |
| O2 | `socket(AF_NETLINK, SOCK_RAW, NETLINK_AUDIT)` and `bind` to the audit read group, as an ordinary user | `bind`: `EPERM` |
| O3 | `auditctl -s` as an ordinary user | "You must be root to run this program" |
| O4 | notification supervisor outside; under it bubblewrap, the helper with `/usr/bin` allowed, and `sh` running one allowed and one denied program | five notifications, with the paths `/usr/bin/bwrap`, `/helper`, `/usr/bin/sh`, `/usr/bin/true` and `/workspace/other/tool`. The last was denied inside (status 126); the supervisor was not told |
| O5 | `sh` executing `/usr/bin/true` 200 times and one program that fails, no observer | 90 ms |
| O6 | O5 under the notification supervisor | 202 notifications, 88 ms |
| O7 | O5 under `strace -f -Z -e trace=execve,execveat` | 224 ms |
| O8 | `strace -f -Z -e trace=execve,execveat` outside; under it bubblewrap and O4's run | it recorded `execve("/workspace/other/tool", ["/workspace/other/tool", "mount"], …) = -1 EACCES (Permission denied)` |
| O9 | `/proc/sys/kernel`: `yama/ptrace_scope`, `unprivileged_bpf_disabled`, `perf_event_paranoid`, `dmesg_restrict` | 1, 2, 2, 1 |

## 4. The rule

**D1. On `linux` a profile with a non-empty `exec_subpaths` is held to its literal program and the
list. A profile with an empty list runs as today. `macos` does not change: with an empty list only
the program executes.**

Provisional, pending the operator (§ 19 Q2): the difference stays and is stated. The design is
built on that. The reasons:

- Holding an empty list to the program on `linux` would change every existing run. A run that
  starts `sh -c` today executes whatever its script names; every delegated-lane case does.
- Letting an empty list execute anything on `macos` would remove a rule that adapter enforces
  today.

What a client can rely on across both adapters:

| profile | `linux` | `macos` |
|---|---|---|
| empty `exec_subpaths` | every program in the run's view executes | only the program executes |
| non-empty `exec_subpaths` | `execve` succeeds only for the program and for files that are, or lie beneath, an entry and are not denied | the same |

A client that wants one rule on both sends a non-empty list. A client that wants `macos`'s
empty-list rule on `linux` lists the program itself (D2 makes that an entry; probe W5).

Requiring the fact through `ConfinementProfile.facts` with an empty list checks only that the
adapter proved it. It applies nothing on `linux`.

## 5. Entries

**D2. An entry names a directory or one file.** A directory admits every file beneath it. A file
admits itself. Landlock takes either: a rule's `parent_fd` "identifies the parent directory of a
file hierarchy, or just a file" (`landlock.h`, `struct landlock_path_beneath_attr`; probe A14).
`macos` renders an entry as `(subpath …)` (`seatbelt.rs:270-277`); that a `subpath` rule on a file
matches the file is inferred and gets a case on the `macos-15` lane (§ 16).

**D3. On `linux` an entry is written and resolved in the run's view, not in host paths.** `linux`
does not prove `paths_are_host_paths` (`linux.rs:29-31`): the program sees `/workspace` and the
declared mounts. The program's own path is already a view path, and the list uses the same
namespace.

| entry | `linux` | `macos` (unchanged) |
|---|---|---|
| relative | beneath `/workspace` | beneath the workspace |
| absolute | beneath `/usr`, `/bin`, `/lib`, `/lib64`, `/workspace`, the mount of a declared read-only root, read-only file or writable root | resolved; in the workspace, a declared root, or a system or developer directory |
| anywhere else (`/tmp`, `/proc`, `/dev`, `/etc`) | refused `exec.sandbox-exec-subpath-undeclared` | the same code |
| lexical form | `seatbelt::relative_literal` and `absolute_literal`: normal names, no `.` or `..`, no quote, backslash or control character; else `exec.sandbox-path-unrepresentable` | the same functions |
| symbolic link beneath a location the run can write (the workspace, a writable root) | no part may be one; refused `exec.sandbox-path-unrepresentable` | relative entries: the same |
| symbolic link beneath a read-only location | followed; the rule is on the file or directory it resolves to (probe N7) | absolute entries are canonicalized |
| does not exist at the start | refused `exec.sandbox-path-unrepresentable` | a relative entry need not exist; an absolute one must |
| the program | an absolute view path when the list is not empty; else `exec.sandbox-path-unrepresentable`, address `argv` | absolute, always |

Reasons:

- **One lexical grammar.** A profile that `macos` can carry is one `linux` accepts, and the
  reverse.
- **No symbolic link beneath a writable location.** An earlier run could leave `bin` as a link to
  `/usr/bin`; the entry `bin` would then admit all of `/usr/bin`. The driver checks through the
  pinned workspace descriptor before dispatch, as it does for a writable subtree (design 29 § 2),
  and the helper opens the entry again with `openat2`, `RESOLVE_BENEATH` and
  `RESOLVE_NO_SYMLINKS`, so a link swapped in between is refused by the kernel. The helper's
  `openat2` call was not probed (§ 18).
- **An entry must exist on `linux`.** A Landlock rule is tied to an open file, not to a name. A
  client that lists `target` creates it before the start.
- **`/tmp` is not eligible.** It is a fresh tmpfs per run and writable. `macos` does not admit its
  private `TMPDIR` either.

## 6. The mechanism on `linux`

### Options

| option | finding | verdict |
|---|---|---|
| Landlock applied in the driver's `pre_exec`, before bubblewrap | a thread under any filesystem ruleset cannot change the mount tree; bubblewrap fails at its first mount (W1) | not possible |
| mounts marked `noexec` by bubblewrap | neither 0.11.2 nor 0.13.0 has the option (V1). `noexec` could also not admit one file, or deny one beneath an allowed directory | not possible |
| seccomp on `execve` | a classic-BPF filter reads the argument registers, and the path is a pointer it cannot follow (`seccomp.rs:63-68`) | not possible |
| Landlock applied inside the sandbox by util-linux `setpriv`, placed before the program | works on util-linux 2.42.4 (V2). The daemon image and the hosted lane runner have 2.39.3, which has no Landlock option (V3). `setpriv` cannot expand a directory around a denied program, so the deny-list would be a bound empty file (N9, N10): files only, existing paths only | not chosen (D5) |
| Landlock applied inside the sandbox by a helper of this repository, placed before the program | works (W3 to W5, N1 to N8) | chosen |

**D4. A Landlock ruleset holds the run to the list. A helper applies it inside the sandbox, as the
last program before the run's own.** Every process of the run descends from the helper and inherits
the domain (`landlock(7)`, *Inheritance*). The run cannot remove it.

### The helper

**D5. `substrate-exec-gate` is a new binary of this repository with no dependency beyond `libc`.**
`HostConfig` gains `exec_gate: Option<PathBuf>`, as it names bubblewrap today (`lib.rs:317`).
Decided by session `sdc` on 2026-11-21 under
`approval-record:sdc-decides-plan-and-design-2026-11-21`, over the `setpriv` option of the table
above: the acceptance of both stories is a delegated-lane run, and that lane's hosted runner has a
`setpriv` without Landlock options (V3).

- **In the view.** The driver opens the helper once per start, checks it against the bound
  identity (below), and binds it read-only through its descriptor with `--ro-bind-fd`, the option
  that already binds declared files (ADR 0033; W7). The run can read the helper and cannot execute
  it: its path is not in the list (W3).
- **In the argv.** After `/usr/bin/env -u PWD --` and the optional `PWD` link
  (`process.rs:2849-2853`), directly before the program:
  `… /usr/bin/env -u PWD -- <gate> --allow <path>… --deny <path>… -- <program> <argument>…`.
  Everything bubblewrap, `setsid` and `env` execute happens before the ruleset exists. The only
  `execve` after it is the program's. Every path the driver passes is absolute, so none reads as
  an option.
- **In the snapshot.** The helper's path, SHA-256, device and inode join `BackendBinding`
  (`probe.rs:25-34`). A helper replaced after the probe makes the next start
  `exec.capability-stale`, as a replaced bubblewrap does (`process.rs:2657-2669`).
- **Only with a list.** A start whose profile has an empty `exec_subpaths` gets no helper and no
  bind. Its argv is today's, byte for byte.

The lists reach the driver in `AdapterStart` (`process.rs:218-232`), beside `retention`, `stdin`
and `live`. `ExecStartInput` is a wire type and does not change.

### Embedder notes

An embedder of the in-process adapter links `b10x-substrate-host` as a library. Cargo does not
build a dependency's binary, so the helper does not arrive with the crate. Loom is the first such
embedder. It must:

1. **Build `substrate-exec-gate` from the revision it pins the crate at**, and install it beside
   its own binary.
2. **Name it in `HostConfig.exec_gate`** as an absolute path. The driver searches nowhere: not
   `PATH`, not the directory of the running executable.

What the file must be:

| requirement | why |
|---|---|
| a regular file, reached through no symbolic link the embedder does not control | the driver resolves the path once at open and binds the resolved file |
| executable by the uid the driver runs as | bubblewrap executes it inside the view as that uid |
| at most 64 MiB | the bound the driver reads a backend binary under (`MAX_BACKEND_BINARY_BYTES`, `probe.rs:22`) |
| on a local filesystem | the stamp the driver rechecks relies on a kernel-maintained change time (`probe.rs:441-446`) |
| able to start inside the view | only `/usr`, `/bin`, `/lib` and `/lib64` of the host are there (`process.rs:2732-2743`). A default build needs the host's loader and `libc`, both beneath them; a static build needs nothing |
| of the driver's protocol | the helper answers `--version` with the line `substrate-exec-gate/1`; the driver passes the argv of D5 and reads the exit statuses of *When the helper fails* |

What the driver does with it at open, in order:

1. resolves the path and reads the file's metadata and SHA-256, as it does for bubblewrap
   (`probe.rs:493-507`);
2. runs it once outside any sandbox with `--version`, a cleared environment and no arguments
   from a request, and requires the protocol line;
3. runs the two probes of § 9 with it;
4. binds its resolved path, SHA-256, device and inode into `BackendBinding` (`probe.rs:25-34`),
   and so into the snapshot digest.

At every start `backend_unchanged` compares the file again (`probe.rs:510-523`). A helper that
was replaced, removed or made unreadable since the open refuses the start
`exec.capability-stale`, before dispatch (`process.rs:2657-2669`). The embedder reopens the
adapter after it upgrades the helper.

### Without a usable helper

**D19. A profile that carries a list never runs without it. Which refusal answers depends on what
the embedder configured.** Decided with D5.

| situation | answer | code and status |
|---|---|---|
| `exec_gate` is not set | the adapter opens; neither fact is proved; a profile with a list is refused at admission | `confinement.fact-unserved` naming `exec_subpaths`, 501 |
| `exec_gate` is set and the file is missing, not a regular file, not executable by the driver's uid, over the size bound, or does not answer the protocol line | the open is refused; no adapter exists, so nothing is admitted | `config.exec-gate-unserved`, 501, address `config.exec_gate`; new |
| `exec_gate` is set and usable, and a probe of § 9 did not hold (no Landlock, ABI 1, a view the helper cannot start in) | the adapter opens; the fact that probe proves is absent; a profile that needs it is refused at admission | `confinement.fact-unserved` naming the fact, 501 |
| the helper changed after the open | the start is refused before dispatch | `exec.capability-stale`, 422 |

Reasons:

- **An unset helper is a host that does not offer the lists.** That is invariant 3's case: the
  fact is absent and the refusal names it. A profile without a list is admitted and runs as today.
- **A set helper that cannot be used is a configuration error, refused when it is made.** The
  embedder asked for the lists. An adapter that opened anyway would answer every listed profile
  `confinement.fact-unserved`, which names the fact and hides the cause. The precedent is the
  `/tmp` size: a size bubblewrap cannot apply refuses startup `config.tmp-bytes-unserved` and
  "Never drops the size and starts runs without it" (`lib.rs:393-427`).
- **`config.exec-gate-unserved` has the form of that code**: a `DriverError` of class `Unserved`,
  which `ConfinementError::Driver` reports with status 501 (`mod.rs:264-271`). Its message names
  the path and which requirement failed. `linux::open` returns it as it returns every other driver
  error (`linux.rs:144-153`).
- **A probe that did not hold is not a configuration error.** The same helper on another kernel
  proves the fact. The fact is absent, as for every probed fact.

A deny-only profile is refused by D13 before any of these. In no row does a run start without its
list.

### The ruleset

**D6. The ruleset handles two rights, `LANDLOCK_ACCESS_FS_EXECUTE` and `LANDLOCK_ACCESS_FS_REFER`,
and nothing else.** The helper passes the first ABI's attribute size, so `handled_access_net` and
`scoped` stay zero. Read, write and network behave as before.

| rule | right | why |
|---|---|---|
| `/` | refer | Any ruleset denies a rename or link into another directory unless it grants refer (R2). Without this rule `git` and every build tool that renames across directories would fail. R3 restores it |
| the program | execute | the literal program, as a file rule |
| the host's ELF loader | execute | the kernel opens a dynamically linked program's loader with the execute right (A1, A2). Shared libraries need none (A2, A10) |
| each entry of `exec_subpaths` | execute | D1; expanded around a denied path (§ 8.2) |

**D7. The loader is the host's own, read once at open from the `PT_INTERP` of `/usr/bin/env`.** The
driver already executes `/usr/bin/env` in every sandbox (`process.rs:2849`), so its loader is the
one the base view's programs use. It is `/lib64/ld-linux-x86-64.so.2` on the probe host. The
loader is never read from the run's program: a program in the workspace is a file an earlier run
may have written, and its `PT_INTERP` would then choose a path that receives the execute right. A
program with another loader (musl, 32-bit) lists that loader as a file entry. A `#!` program lists
its interpreter; so does `macos`, which allows only the literal program.

**D8. The fact needs Landlock ABI 2 (Linux 5.19).** ABI 1 has the execute right and no refer right,
so every cross-directory rename in the run would fail (`landlock(7)`, *VERSIONS*; `landlock.h`
"when using the first Landlock ABI version, Landlock will always deny attempts to reparent files").
The probe tests a cross-directory link (§ 9), so an ABI 1 kernel leaves the fact absent. A kernel
without Landlock answers `ENOSYS`, one with Landlock disabled at boot `EOPNOTSUPP`
(`landlock_create_ruleset(2)`); the helper exits, the probe fails, the fact is absent. The
repository states no minimum kernel. The container seccomp profile already allows the three
Landlock system calls (`deploy/seccomp/host-exec-v1-amd64.json:145-147`), and the run's own
seccomp program allows every call it does not name (`seccomp.rs:192`).

### What else the ruleset changes for the run

- **A rename or link from a directory outside the list into a listed directory fails `EXDEV`**
  (R4). Landlock refuses a move that would give a file a right it did not have. `mv` and other
  tools that fall back to a copy still work (R6). The opposite direction works (R5).
- **A listed directory that the run removes and creates again is no longer listed** (W11). The
  rule belonged to the removed directory. `cargo clean` followed by a build in one run fails; the
  run removes the directory's contents instead, or the client lists the parent.
- **Mounting stays impossible.** It already was (W6).
- **`ptrace` is unchanged within the run.** Landlock limits tracing to processes of the same or a
  nested domain (`landlock(7)`, *Ptrace restrictions*); every process of the run is in the one
  domain.

### When the helper fails

The helper executes the program only after `landlock_restrict_self` returned 0. Any earlier
failure (no Landlock, an entry it cannot open, a rule the kernel refuses) ends the run before the
program starts: one diagnostic line on standard error and exit status 125. When `execve` of the
program fails it exits 126, or 127 for `ENOENT`, as `/usr/bin/env` does in that position today.
The driver has checked every entry before dispatch (§ 5), so a helper failure means the kernel
changed its answer after the probe or a path changed after the check. Nothing runs unrestricted
(invariant 3).

## 7. What the list does not stop

**D9. The list restricts which files `execve` and `execveat` accept. It does not restrict what
code an allowed program runs.** The fact's documentation states these limits; each is a probe.

- **An allowed interpreter runs any script it can read** (A7). `sh script`, `python script` and
  `sh -c` do not execute the script file. A `#!` script outside the list is refused (A6).
- **The loader is an interpreter for programs.** A dynamically linked program cannot start unless
  the loader is executable (A1), and the loader started directly runs any program file the run can
  read (A8). No Landlock right separates the two uses.
- **A memfd is outside the list's reach.** A program that copies bytes into a memfd and executes
  it runs them (A9). Provisional, pending the operator (§ 19 Q3): these two stories add no
  `memfd_create` refusal to the seccomp program, and the limit is stated.
- **Mapping a file as code is not execution** (A10). An allowed program can load any shared object
  it can read.
- **On `macos` the last three do not apply in the same way**: Seatbelt also confines
  `file-map-executable` to the listed subpaths (`seatbelt.rs:273-276`). The first applies there
  too.

The allow-list therefore keeps a run that only starts programs by path to the listed ones. It is
not a boundary against a program that sets out to run other code. The floor's other rules (the
namespaces, the seccomp program, the read-only view) are unchanged and do not depend on it.

## 8. The deny-list

### 8.1 Surface

**D10. `ConfinementProfile` gains `exec_denied_paths: Vec<String>`.** An entry has the form of an
`exec_subpaths` entry (§ 5): a file or a directory, relative beneath the workspace or absolute in
the same locations, the same lexical rule. On `linux` a deny entry may be absent at the start
(§ 8.2).

- **Bound.** `MAX_EXEC_DENIED_PATHS = 64`, a constant of the confinement module. More entries are
  refused `confinement.exec-denied-paths-exceeded` (422) at admission on every adapter. One
  expanded directory of 5953 entries costs 17 ms per start (W12), so the bound keeps the worst
  start near one second. `exec_subpaths` has no bound today and gets none here; the wire story
  bounds both when it gives them a schema.
- **Fact.** `exec_denied_paths` (`Fact::ExecDeniedPaths`), directly after `exec_subpaths`.
  `FACT_ORDER` grows to 14. A non-empty `exec_denied_paths` implies it
  (`profile.rs:213-229`). The fact has no wire property in these stories, as `stdin_pipe` and
  `output_live` have none.
- **Precedence. A denied path wins.** It wins over a listed directory that holds it, over an entry
  that names the same path (N8), and over the literal program. A start whose program is, as
  written, a denied entry or beneath one is refused `exec.sandbox-program-unlisted` (422) before
  dispatch, the existing code for "not an executable of this policy" (`seatbelt.rs:38-39`). A
  program that reaches a denied file under another name fails inside the run (§ 10).
- **Outside the list.** A deny entry that no allowed path holds changes nothing and is not an
  error.

### 8.2 `linux`: the helper expands

Landlock has no deny rule. A layer grants a path when one rule on the way to it grants the access
(`landlock(7)`), so a rule on `/usr/bin` cannot except `/usr/bin/id`.

**D11. The helper grants per entry where a directory holds a denied path.** For each allowed path
`A`, resolved:

1. `A` is a denied path, or a file with the device and inode of a denied file: no rule.
2. No denied path lies beneath `A`: one rule on `A`.
3. `A` is a directory with a denied path beneath it: no rule on `A`; the same three steps for
   each entry of `A`. A symbolic link gets no rule: what it points at has its own place in the
   walk, or is outside the list.

What follows, each observed:

- The denied file stays readable and is not executable (N1). A symbolic link to it is denied
  wherever the link lies (N1, N7), because Landlock decides on the file reached.
- Another name of the same file in an expanded directory is denied (N2): step 1 compares inodes.
- A file created during the run in an expanded directory is not executable (N2), and so is a
  deny entry created during the run (N6). The expansion fails closed. A client that needs new
  programs to be executable in a directory does not put a deny entry inside it.
- The run cannot link the denied file into another listed directory (`EXDEV`, N3).
- A directory can be denied (N5).

The walk is bounded: the helper stops at 65536 rules and exits 125. The option the stories also
name, a non-executable file bound over the denied path, works (N9) and was not chosen: the denied
file would read as empty, a path that does not exist cannot be covered beneath a read-only mount
(N10), and a directory cannot be denied that way.

### 8.3 `macos`: deny rules after the allows

**D12. `SeatbeltPolicy` gains denied paths, rendered as
`(deny process-exec (subpath "<path>"))` and `(deny file-map-executable (subpath "<path>"))` after
every allow**, with the sealed-path denies (`seatbelt.rs:291-313`), because the last matching rule
decides (`seatbelt.rs:13-14`). A relative entry goes through `beneath_without_symlink`, an
absolute one through the location rule of `exec_subpath` (`macos.rs:696-760`); neither need exist,
so the path is resolved through its nearest existing ancestor as a sealed path is
(`seatbelt.rs:514-531`). Nothing of this was run: this document's probes had no macOS host (§ 18).

### 8.4 A deny-list without an allow-list

**D13. A profile with a non-empty `exec_denied_paths` and an empty `exec_subpaths` is refused
`confinement.exec-denied-paths-without-subpaths`, status 422, on every adapter.** It is a new
`ConfinementError` variant, `ExecDeniedPathsWithoutSubpaths { adapter }`, beside
`LocalReasonMissing` and `ImageUnpinned` (`mod.rs:259`). The check reads the two fields and no
adapter fact. It runs where `reserved` runs (`mod.rs:588`), directly after it, and in
`admit_unopened` (`mod.rs:201-210`), so it comes before `confinement.fact-unserved`: `local`,
which proves neither fact, answers a deny-only profile with this refusal, and a profile with both
lists with `confinement.fact-unserved` naming `exec_subpaths`.

The limit, in one sentence: a run that can write and execute files in a listed directory can copy
a denied program there and run the copy (W4, N3), so a deny-list means something only beside an
allow-list that admits no directory the run can write.

### 8.5 Links to a denied program

| reached through | `linux` | `macos` (inferred) |
|---|---|---|
| a symbolic link, anywhere | denied (N1, N7) | denied: Seatbelt matches the resolved path (`seatbelt.rs:180-181`) |
| a hard link in the same directory | denied (N2) | not denied: the rule names a path |
| a hard link the run creates in a listed directory | cannot be created (N3) | not known |
| a hard link that existed before the start elsewhere beneath a listed path | **runs** (N4) | runs: the rule names a path |
| a copy in a listed directory the run can write | runs (N3) | runs |

A deny entry denies a path. It does not find every name of a file. Provisional, pending the
operator (§ 19 Q4): the limit is stated, and a deny entry whose file has more than one link is not
refused.

## 9. Facts, admission and the probe

`linux::proof` reads only the capability snapshot today (`linux.rs:166`). The snapshot's
`confinement_exec_subpaths` is the published wire fact and stays `None` (§ 2), so the proof needs
its own input.

**D14. The driver keeps what its probe proved in a value of its own, `ExecListProof { subpaths,
denied_paths }`, and `linux::proof` takes it as a second argument.** The value is serialized into
`SnapshotMaterial` (`probe.rs:36-48`), so the snapshot digest binds it; no published fact changes.
`proof` is a public item, so `crates/substrate-host/tests/confinement-public-items.txt` is
regenerated in the first story too.

**D15. Each fact is proved at open by one run in a throwaway sandbox**, with
`USER_NAMESPACE_ARGV` and the argv of `probe_workspace_scoped_write` (`probe.rs:358-391`), gated on
the floor (`exec`) and on a configured helper. The probe copies the host's `/usr/bin/true` and
`/usr/bin/ln` into a temporary workspace and runs `/bin/sh -c` through the helper.

| fact | list | the script, as the run's program | proof |
|---|---|---|---|
| `exec_subpaths` | `allowed` | `allowed/true` succeeds; `outside/true` fails; `/usr/bin/true` fails; `allowed/ln outside/true elsewhere/linked` succeeds | exit 0, and `elsewhere/linked` exists on the host |
| `exec_denied_paths` | `allowed`, deny `allowed/sub/true` | `allowed/true` succeeds; `allowed/sub/true` fails; `allowed/sub/again`, a hard link to it, fails | exit 0, and `exec_subpaths` was proved |

Each half must hold: what the list allows happened, and what it refuses did not, as
`seatbelt::probe_build` requires (`seatbelt.rs:429-431`). The `ln` step proves the refer rule, so
an ABI 1 kernel fails it. The probe proves the kernel and the helper together; it never reads a
kernel version or an ABI number to decide.

`systemd` around `linux` copies the inner adapter's proved facts
(`crates/substrate-host/src/confinement/systemd.rs:576-599`) and needs no change.

**D16. `publish_confinement` keeps `confinement_exec_subpaths` at `None`** (`probe.rs:315`). The
test `the_published_set_is_the_linux_adapters_proof` keeps every other member equal and states
this one by name: proved by the adapter, absent from the published set, until
`story:exec-start-carries-executable-lists`. `exec_denied_paths` has no wire property to publish.

## 10. What the run sees, and what the adapter can observe

**D17. A denied execution fails with `EACCES` (13)** on `linux`, for `execve` and `execveat` (A4).
A shell reports `Permission denied` and status 126 (A15). The run continues. On `macos` Seatbelt
denies `process-exec`; the probe there relies on a non-zero status from the shell
(`seatbelt.rs:487`), and the exact `errno` is not recorded in the tree.

**D18. The adapter does not observe a denied execution, and records none.**

- On `linux` the denial is an error return inside the run. The helper has executed the program
  and is gone by then; it sees no later call. The helper sets no Landlock log flag.
- On `macos` the sandbox reports denials to the system log, which the adapter does not read
  (inferred).

The run's own exit status and standard error are what a consumer has from these two stories.
§ 11 lists what could show a denial to the driver afterwards.

## 11. Making a denied execution visible afterwards

The consumer needs to learn, after a run, that an execution was denied. `EACCES` inside the run
does not tell it: a tool may swallow the error, retry another program, or exit 0.

**This document designs none of the options below, and no story of the epic promises the record**
(the epic's § Out of scope). The section records what each option needs and costs, so that the
story that takes this up starts from measurements.

| option | what it sees | what it needs | cost | status |
|---|---|---|---|---|
| **Landlock audit records.** The helper passes `LANDLOCK_RESTRICT_SELF_LOG_NEW_EXEC_ON` to `landlock_restrict_self`; the kernel writes each denial of the domain to the audit subsystem | the kernel's own decision: the denied path and the domain | Landlock ABI 7 (Linux 6.15; `landlock(7)`, *VERSIONS*; `landlock.h:137-168`). Audit enabled on the host. A reader with privilege: joining the audit read group without it is `EPERM` (O2), and `auditctl` needs root (O3). In a container the host's audit log is not there | none per execution in the run; one host-wide privileged reader, and a way to match a record's domain to a run | flag and refusals probed (O1 to O3). No record was seen: the probe host runs no audit daemon, and whether the kernel wrote one could not be read. The record format is inferred |
| **A seccomp user-notification filter** on `execve` and `execveat`, installed by the helper, supervised by the driver | every attempt, allowed or denied, with the path string read from the caller's memory (O4). Not the outcome | no privilege (O4 ran as an ordinary user, the filter installed before bubblewrap). A listener descriptor handed from inside the sandbox to the driver. The driver must decide itself whether an attempt was denied, by evaluating the list against the path | 200 executions took 88 ms supervised and 90 ms unsupervised (O5, O6): no measurable cost per execution. A stalled supervisor blocks every execution of the run | probed (O4 to O6). The path can be rewritten by the run while the call waits (`seccomp_unotify(2)`: "must not be used to make security policy decisions"); for a record about an honest tool chain that race matters less than for enforcement. The second filter beside ADR 0039's program is not designed |
| **A ptrace observer**: the driver traces the run's tree and reads the result of each `execve` | the outcome: `execve("/workspace/other/tool", …) = -1 EACCES`, read from outside the sandbox (O8) | no privilege where the tracer is an ancestor of the run and Yama's `ptrace_scope` is 0 or 1 (it is 1 on the probe host, O9). `CAP_SYS_PTRACE` above that (inferred). One tracer thread per run. The run's own tools can no longer trace each other: a task has one tracer | 200 executions took 224 ms under `strace -f` against 90 ms without (O7); `strace` stopped at every system call, and a tracer that stops only at `execve` would cost less (inferred) | probed with `strace` (O7, O8). A tracer inside the driver is not designed |
| **An eBPF observer** on the exec path (a tracepoint, or an LSM program) | the outcome, for every process on the host | `CAP_BPF` and `CAP_PERFMON`, or root. `kernel.unprivileged_bpf_disabled` is 2 on the probe host (O9). The container seccomp profile does not allow `bpf` (`deploy/seccomp/host-exec-v1-amd64.json`) | one privileged host component, outside the unprivileged driver | sysctl and profile read; no program was loaded. Inferred |
| **A wrapper convention**: the consumer's tools report their own failed executions, for example a launcher the step calls that records `EACCES` and status 126 in the step's result | what cooperating tools report | no kernel support and no change here | none in substrate | not probed; it is the consumer's design. It covers only tools that cooperate, which is the honest tool chain the lists are for |
| **The recorded output**: a shell that is denied writes `Permission denied` to standard error and returns 126 (A15), and the adapter already records both | only shells and tools that print the error | nothing new | none | observed (A15). A heuristic, not a record |

What the options have in common: the two that need no privilege (the seccomp notification and
ptrace) put the driver in the path of every execution of the run, and the two that do not
(Landlock audit, eBPF) need a privileged reader on the host. Only the wrapper convention and the
recorded output work for an in-process embedder today without either.

## 12. Refusals

| code | status | when | where |
|---|---|---|---|
| `confinement.exec-denied-paths-without-subpaths` | 422 | a deny-list beside an empty `exec_subpaths` | admission, every adapter, after the reserved fields (D13); new |
| `confinement.exec-denied-paths-exceeded` | 422 | more than `MAX_EXEC_DENIED_PATHS` entries | admission, every adapter; new |
| `config.exec-gate-unserved` | 501 | `HostConfig.exec_gate` is set and the file is missing, not a regular file, not executable, over the size bound, or does not answer the protocol line (D19) | the open of `linux`, and of `systemd` around it; new |
| `confinement.fact-unserved` naming `exec_subpaths` | 501 | a non-empty `exec_subpaths` on an adapter that did not prove it: `linux` without `exec_gate`, `linux` whose probe did not hold, and every adapter of § 13 that proves neither fact | admission; existing |
| `confinement.fact-unserved` naming `exec_denied_paths` | 501 | a deny-list on an adapter that proved `exec_subpaths` and not this fact | admission; existing code, new fact |
| `exec.sandbox-path-unrepresentable` | 422 | an entry of either list that is not normal, holds a refused character, has a symbolic link beneath a writable location, or (allow entry on `linux`) does not exist; a program that is not absolute when a list is set | admission for the lexical rule, the start for the rest; existing code, new on `linux` |
| `exec.sandbox-exec-subpath-undeclared` | 422 | an absolute entry of either list outside the eligible locations (§ 5) | the start; existing code, new on `linux` |
| `exec.sandbox-program-unlisted` | 422 | the program is, as written, a denied entry or beneath one | the start, before dispatch; existing code, new use |
| `exec.capability-stale` | 422 | the helper's identity changed after the probe | the start; existing |

Every refusal of an admission or a start writes its audit record, as it does today
(`linux.rs:55-56`). A refused open has no profile and no sink, so it writes none; the embedder
gets the error from `open`. After dispatch there is no refusal: a helper failure is exit status
125 (§ 6). No row lets a profile with a list run without it (D19).

## 13. Per adapter

| adapter | `exec_subpaths` | `exec_denied_paths` |
|---|---|---|
| `linux` | proved where the probe held (story 1) | proved where the probe held (story 2) |
| `systemd` around `linux` | `linux`'s | `linux`'s |
| `macos` | proved, unchanged | proved where `probe_build` held (story 2) |
| `local`, `systemd` around `local`, `oci`, `pod` | not proved | not proved |

`oci` and `pod` are `story:executable-lists-on-oci-and-pod`.

## 14. The specification

Described here; the stories edit the files and `cargo xtask check-ess` regenerates
`spec/confinement-suite.json` and the generated types.

The model admits profiles and does not run programs (§ 1). It can state which adapter proves and
requires which fact, and the new refusal. It cannot state an allowed and a denied execution; those
are cases of the adapter lanes (§ 16). `story:linux-executable-allow-list` asks for "a scenario for
an allowed and a denied execution": the scenario it gets is the admission, and the executions are
lane cases.

Story 1, `spec/confinement/confinement.yaml`:

- `:192-201`, the `linux` declared set: add `exec_subpaths` ("Landlock execute right, applied
  inside the sandbox, design 35"). The `UNMAPPED` sentence at `:199-201` keeps `private_tmpdir`
  only.
- No type, error or outcome changes: `exec_subpaths` and `fact-unserved-exec-subpaths`
  (`:560-567`) exist.
- Scenarios: `linux-admits-exec-subpaths-on-delegated` and
  `systemd-linux-admits-exec-subpaths-on-delegated`, in the form of
  `spec/confinement/scenarios/macos-s2-admits-exec-subpaths-on-macos.yaml`. Every existing
  `linux-*-on-delegated` and `systemd-linux-*-on-delegated` scenario lists the adapter's facts in
  its open step; each gains `exec_subpaths` after `ipc_signals_contained`.
- `spec/confinement/README.md`: the lane table and the sentence at `:149` that lists
  `exec_subpaths` on `linux` among the facts no story declares.

Story 2:

- `:31-49`, the fact comments: add `exec_denied_paths: listed paths never execute, even beneath
  an exec subpath`.
- `:50-66`, `Fact`: add `exec_denied_paths` after `exec_subpaths` (`:60`).
- `:150-181`, errors: add `substrate.confinement.ExecDeniedPathsWithoutSubpaths`,
  "`confinement.exec-denied-paths-without-subpaths`, 422", field `adapter`.
- `:186-222`: `linux` and `macos` declared sets gain `exec_denied_paths`.
- `:305-320`, `AdmitProfile` input: the model must see that a deny-list is set and an allow-list
  is not. Proposed: two inputs, `exec_subpaths` and `exec_denied_paths`, each `List<String>`, and
  an outcome `exec-denied-paths-without-subpaths` directly after `rate-bounds-reserved` (`:328`)
  whose guard is a non-empty second list and an empty first. Whether ESS states that guard over
  two lists is not checked here (§ 18); two `Boolean` inputs are the fallback.
- After `:567`: `fact-unserved-exec-denied-paths`, in the form of its neighbours.
- Scenarios: `linux-admits-exec-denied-paths-on-delegated`,
  `systemd-linux-admits-exec-denied-paths-on-delegated`,
  `macos-lists-admits-exec-denied-paths-on-macos`,
  `local-exec-denied-paths-without-subpaths-on-portable`,
  `linux-exec-denied-paths-without-subpaths-on-delegated`, and
  `local-exec-lists-unserved-on-portable` (both lists on `local`: `fact-unserved` naming
  `exec_subpaths`). The open steps of the `linux`, `systemd`-around-`linux` and `macos` scenarios
  gain `exec_denied_paths` after `exec_subpaths`.
- `cargo xtask check-docs` holds the site's adapter and fact tables to the specification
  (`xtask/src/site_drift.rs`); the pages under `website/docs` change with each story.

## 15. The wire is untouched

No route, schema, fact or refusal of `contracts/substrate-wire` changes. `ExecStartInput` gains no
field. `confinement.exec-subpaths` stays unpublished by the host driver, so a wire start that
requires it is still `confinement.fact-unserved` (501). `substrate-wire/0.26.0` and its digest stay
what the daemon advertises.

None of the three provisional defaults of § 19 (the empty-list difference, no `memfd_create`
refusal, the hard-link limit) may be published as a wire property or in release notes before the
operator has answered. `story:exec-start-carries-executable-lists` inherits that condition.

## 16. Tests and lanes

Each case runs its scenario once. A case named `limit_…` asserts a limit of § 7 or § 8.5 as it is,
so a kernel or tool change that moves it is seen.

**Delegated lane** (`bash scripts/delegated-lane.sh`, `crates/substrate-host/tests/ess_confinement_linux.rs`).
The script builds `substrate-exec-gate` and names it in the lane's `HostConfig`; without it the
open step of every `linux` scenario fails on the missing fact and does not pass silently.

Story 1:

- the new admission scenarios, through the existing scenario runner;
- a profile listing one directory: a program beneath it runs, a program outside it fails with
  status 126 inside the run (the story's acceptance);
- a file entry; a relative entry beneath the workspace; the program alone as the list;
- a `#!` script outside the list is denied;
- `limit_an_allowed_interpreter_runs_a_script_outside_the_list`,
  `limit_the_loader_runs_a_program_outside_the_list`,
  `limit_a_memfd_copy_runs_outside_the_list`;
- a rename between two unlisted directories succeeds; `limit_` a rename into a listed directory is
  `EXDEV`; `limit_` a listed directory removed and created again is no longer listed;
- refused at the start, each with its audit record and nothing run: an entry with a symbolic link
  in the workspace, an absent entry, an entry under `/tmp`, a relative program;
- D19, each with nothing run:
  - an adapter opened without `exec_gate`: a profile with a list is `confinement.fact-unserved`
    naming `exec_subpaths`, with its `profile_refused` record, and a profile without a list runs;
  - `exec_gate` naming a path that does not exist, a directory, and a file without an execute bit:
    each open is `config.exec-gate-unserved`, status 501, and the message names the path;
  - `exec_gate` naming a program that does not answer the protocol line: the same refusal;
  - the helper replaced after the open: the next start with a list is `exec.capability-stale`;
- `systemd` around `linux`: one run with a list (`ess_confinement_systemd`).

Story 2:

- `/usr/bin` allowed, one program beneath it denied: another program in `/usr/bin` runs, the
  denied one fails inside the run (the story's acceptance);
- a symbolic link to the denied program, its hard link in the same directory, a denied directory,
  a deny entry created during the run: each denied;
- `limit_a_hard_link_that_existed_in_another_listed_directory_runs`,
  `limit_a_copy_in_a_listed_writable_directory_runs`;
- a deny-only profile on `linux`: `confinement.exec-denied-paths-without-subpaths`, nothing runs.

The runtime vectors' delegated lane is unchanged: 189 cases, and it still lists `whole-tree-kill`,
`ipc-socket-allowlist` and `ipc-signals-contained` and no `exec-subpaths`.

**Unit tests, every platform** (`cargo test --workspace`):

- `mod.rs`: a deny-only profile is refused by D13's code on facts of `local` and of `linux`, with
  a `profile_refused` record carrying the code; both lists on `local`'s facts are
  `confinement.fact-unserved` naming `exec_subpaths`, with the record; both lists on facts that
  prove `exec_subpaths` and not `exec_denied_paths` name `exec_denied_paths`; 65 deny entries are
  refused; a profile without a deny-list is admitted as before.
- `profile.rs` and `facts.rs`: the deny-list implies its fact; the `Fact` order has it directly
  after `exec_subpaths`.
- `process.rs`: a start without a list renders the argv it rendered before; a start with lists
  places the helper directly before the program and after `env`, and binds it by descriptor.
- `probe.rs`: `the_published_set_is_the_linux_adapters_proof` with its named exception;
  `every_probe_sandbox_carries_the_user_namespace_posture` (`probe.rs:1660`) counts the new probe;
  an unset helper, and one that exits without restricting, each withhold both facts.
- `lib.rs`: the open check of D19 decides on a path, its metadata and a protocol line it is given,
  as `tmp_size_served_by` decides on help text (`lib.rs:409-427`), so the missing, non-regular,
  non-executable, oversized and wrong-protocol cases need no sandbox and run on every Linux host.
- `seatbelt.rs`: the deny rules are rendered after every allow, for a file and for a directory;
  an unrepresentable deny path is refused by name. Rendering compiles everywhere (`seatbelt.rs:16`).
- `confinement_public_items`: the list is regenerated in each story.

**`macos-15` lane** (`.github/workflows/macos.yml`, `ess_confinement_macos`), story 2:

- the new `macos` admission scenario; the lane's summary line counts its group;
- `/usr/bin` allowed and one program beneath it denied: another program runs, the denied one does
  not (the story's check);
- a file entry in `exec_subpaths` admits that file (D2); a symbolic link to a denied program is
  denied; a deny entry that does not exist yet denies the file once created;
- `the_platform_sandbox_proves_every_build_fact` (`seatbelt.rs:831`) includes the new field of
  `BuildProof`.

## 17. Decisions

| # | decision |
|---|---|
| D1 | `linux`: a non-empty `exec_subpaths` holds the run to its program and the list; an empty one runs as today. `macos` is unchanged. The difference is stated, not removed. Provisional, pending the operator (Q2) |
| D2 | an entry names a directory or one file |
| D3 | on `linux` entries are view paths: relative beneath `/workspace`, absolute in the base system directories and declared mounts; they must exist; no symbolic link beneath a writable location; the program is absolute |
| D4 | Landlock, applied inside the sandbox by a helper that is the last program before the run's |
| D5 | the helper is `substrate-exec-gate`, a binary of this repository, named by `HostConfig.exec_gate`, bound by descriptor, bound into the snapshot, absent from a start without a list. Decided by session `sdc`, 2026-11-21, `approval-record:sdc-decides-plan-and-design-2026-11-21` |
| D6 | the ruleset handles execute and refer only; refer is granted on `/` |
| D7 | the host's loader, read at open from `/usr/bin/env`, is granted; never a loader read from the run's program |
| D8 | the fact needs Landlock ABI 2; the probe decides, never a version number |
| D9 | the list restricts `execve` targets, not the code an allowed program runs: interpreters, the loader, a memfd and mapped code are stated limits. No `memfd_create` refusal: provisional, pending the operator (Q3) |
| D10 | `exec_denied_paths`, fact `exec_denied_paths` after `exec_subpaths`, at most 64 entries, a denied path wins |
| D11 | `linux` denies by expanding a listed directory into per-entry rules; the expansion fails closed. Another hard link elsewhere beneath a listed path still runs: provisional, pending the operator (Q4) |
| D12 | `macos` denies with `process-exec` and `file-map-executable` deny rules after every allow |
| D13 | a deny-list without an allow-list is `confinement.exec-denied-paths-without-subpaths`, 422, on every adapter, before any fact is read |
| D14 | the probe's result is a host-private `ExecListProof`, a second argument of `linux::proof` |
| D15 | each fact is proved by one throwaway run in which the allowed half happened and the refused half did not |
| D16 | `confinement.exec-subpaths` stays unpublished, and the probe test names that exception |
| D17 | a denied execution is `EACCES` inside the run |
| D18 | no adapter observes or records a denied execution; § 11 lists what could, and designs none of it |
| D19 | a profile with a list never runs without it: no `exec_gate` is `confinement.fact-unserved` (501) at admission; a set `exec_gate` that cannot be used is `config.exec-gate-unserved` (501) at open; a helper changed after the open is `exec.capability-stale` (422) at the start. Decided with D5 |

## 18. Inferred, not observed

- Everything about `macos` in D2, D12, § 8.5 and § 10: no macOS host ran a probe. The `macos-15`
  lane cases of § 16 decide each.
- The helper's `openat2` with `RESOLVE_BENEATH` and `RESOLVE_NO_SYMLINKS` (§ 5): the probe helper
  used `open` with `O_PATH`. The repository already relies on `openat2` for guarded I/O.
- The run's seccomp program lets the Landlock system calls through: read from `seccomp.rs:160-192`.
  The probes ran without that program installed.
- `ENOSYS` and `EOPNOTSUPP` on a kernel without Landlock: from `landlock_create_ruleset(2)`; every
  probe ran on a kernel that has it. Hosts on Linux 5.13 to 5.18 report ABI 1: from the table in
  `landlock(7)`.
- GitHub's `ubuntu-24.04` runner kernel enables Landlock at ABI 2 or later: not checked. If it
  does not, the delegated lane fails at the first `linux` open step there.
- ESS can guard an outcome on one empty and one non-empty list input (§ 14).
- The start cost of an expanded directory scales with its entries: one measurement, 5953 rules.
- The helper's `--version` protocol line and the driver's open check (§ 6, D19): proposed, modelled
  on `check_tmp_size_served`; nothing of it was run.
- § 11, Landlock audit records: that the kernel writes a record for a denial under
  `LANDLOCK_RESTRICT_SELF_LOG_NEW_EXEC_ON`, and what the record holds, is from `landlock.h`. No
  record was read (O1). That a privileged reader could match a record to a run is not checked.
- § 11, seccomp user-notification: that the run can rewrite the path while the call waits is from
  `seccomp_unotify(2)`; the probe did not attempt it. That the notification filter composes with
  ADR 0039's program, and that the helper can hand the listener to the driver from inside the
  sandbox, is not probed: in O4 the filter was installed outside, before bubblewrap.
- § 11, ptrace: the cost of a tracer that stops only at `execve`, and the `CAP_SYS_PTRACE`
  requirement above `ptrace_scope` 1. O7 and O8 used `strace`, not a tracer in the driver.
- § 11, eBPF: the capabilities it needs. No program was loaded.

## 19. Open questions for the operator

Q1 of the first draft, which program applies the ruleset, is decided: the helper of this
repository (D5, D19), by session `sdc` on 2026-11-21 under
`approval-record:sdc-decides-plan-and-design-2026-11-21`.

The three questions below are security defaults. They belong to the operator, who has not
answered. Each has a provisional answer, and the design is built on it. None of the three may
reach a wire property or release notes before the operator has answered (§ 15).

**Q2. What does an empty `exec_subpaths` mean on `linux`?**

- **As today: every program in the view executes**, and the difference from `macos` is stated
  (D1). A client that wants `macos`'s rule lists the program itself.
- **The program only, as on `macos`.** One rule on both adapters. Every existing `linux` run that
  starts a second program would fail until its client sends a list.

Provisional, pending the operator: as today. The design is built on it (§ 4): a start without a
list gets no helper and today's argv.

**Q3. Should the floor refuse `memfd_create` so that A9 closes?**

- **Leave it** and state the limit (D9). `memfd_create` has ordinary uses, and the loader limit
  (A8) stays open either way: no Landlock right closes it.
- **Refuse `memfd_create` in the seccomp program** when a profile has a list. This changes the
  floor for those runs and is its own design (ADR 0039 owns the program).

Provisional, pending the operator: no `memfd_create` refusal in these two stories. The design is
built on it (§ 7): the purpose statement and the fact's documentation name the limit.

**Q4. A denied program that has another hard link beneath a listed path (N4).**

- **State the limit** (§ 8.5). On the probe host 106 files in `/usr/bin` have more than one link.
- **Refuse a deny entry whose file has a link count above one** on `linux`, by a new named
  refusal. It closes N4 and refuses, for example, a deny entry for a `zsh` that is installed
  under two names. It would not hold on `macos`, where the rule names a path.

Provisional, pending the operator: the limit is stated. The design is built on it (§ 8.5), and a
`limit_` case holds it (§ 16).

## 20. Not decided here

- A way to make a denied execution visible afterwards. § 11 lists the options; none is designed,
  and no story of the epic promises the record.
- A named failure for a helper that exits 125 after dispatch. A status descriptor from the helper
  to the driver would make it one (W8 shows a descriptor reaches that place); the stories do not
  need it.
- `oci` and `pod`, and the wire field: other stories of the epic.
- A bound on `exec_subpaths`.
