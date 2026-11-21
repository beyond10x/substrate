# Design 35: an executable allow-list and deny-list on the confinement adapters

Status: draft under review, 2026-11-21. Serves `story:linux-executable-allow-list` and
`story:executable-deny-list` (`epic:runs-are-governed-and-limit-hits-are-events`). It changes no
wire byte and no contract bundle: the lists are substrate-host's confinement API
([ADR 0044](../../adr/0044-confinement-is-a-contract-family-served-by-declared-adapters.md)
decisions 7 and 8). No code before acceptance (invariant 8). § 17 lists the decisions, § 18 every
claim that is inferred and not observed, § 19 the three defaults the stakeholder decided.

Revised twice on 2026-11-21.

- After the first review: session `sdc` decided the helper (D5, D19) under
  `approval-record:sdc-decides-plan-and-design-2026-11-21`.
- After the stakeholder's independent review: D20 to D24 are new, D5, D6, D10, D11 and D19 changed,
  and § 3.7 holds the probes behind them. The operator ruled on 2026-11-21 that the stakeholder of
  a request decides on his behalf
  (`approval-record:stakeholder-decides-for-the-operator-2026-11-21`). Under it the stakeholder,
  session `sdc`, decided the three security defaults: D1, the memfd limit of D9 and the hard-link
  limit of § 8.5 (§ 19).

Releases, tags, merges to `main` and version numbers are the operator's. This document touches
none of them.

Two asks, from a consumer that runs every process through substrate-host:

1. A run on the `linux` adapter executes only its program and the programs beneath listed paths.
   Today only `macos` enforces `exec_subpaths`.
2. A profile names paths whose `execve` fails, even beneath a listed path. Today no adapter has
   such a list.

**What the lists are for.** The lists are for an honest tool chain: they keep an agent's tools to
the ones a step declares, and they make a wrong tool call fail loudly. Containment of hostile code
stays with the namespaces, the seccomp program and the read-only view. The lists are not a
boundary, and four probed limits say why (§ 3.1, § 7):

- an allowed interpreter runs any script it can read (A7);
- the dynamic loader, started directly, runs any program file the run can read (A8);
- a memfd copy of a program runs (A9);
- mapping a file as code is not execution, so the list does not see it (A10).

The deny-list has the same limits. The loader given a denied program runs it, and `sh` given a
denied script runs it (B8).

## 1. Evidence: what the tree does at `5d5b192db`

Read at `5d5b192db`. The branch now sits on `638a7d70c`; between the two, of the files cited here,
only `process.rs` changed, in its tests after `:4841`, so every cited line holds.

| what | where |
|---|---|
| `AdapterStart` derives `Default` and `Copy`. The three wire paths (an exec start, a start with Git roots, a session start) pass `AdapterStart::default()`; the `linux` adapter builds it as a literal that names every field | `process.rs:217-232`, `:816`, `:848`, `:958`, `crates/substrate-host/src/lib.rs:1247-1252` |
| `backend_unchanged` resolves the bubblewrap path and reads its metadata by name at every start; the spawn that follows names the path again | `probe.rs:510-528`, `process.rs:2698` |
| `render` builds the argv from the wire input alone | `crates/substrate-host/src/confinement/linux.rs:625-640` |
| the daemon and SDK advertise `substrate-wire/0.27.0` | `crates/substrate-wire/src/lib.rs:18` |
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

### 3.7 Probes for the stakeholder's review

The probe helper gained a flag that adds a rule without the deny check, as the first draft's
"program" and "loader" rules were written. B10 used a fourth small program, which copies a file
into a sealed memfd and hands bubblewrap a read-only descriptor of it. On the probe host
`/usr/bin/sh` is a symbolic link to `bash`, and `/bin`, `/lib` and `/lib64` are symbolic links
into `/usr`, so the view's `/bin` is a second mount of the directory `/usr/bin`.

| # | probe | observed |
|---|---|---|
| B1 | unchecked rule on the program `/usr/bin/sh`, `/usr/bin` listed, `/usr/bin/bash` denied; `sh` then starts `/usr/bin/bash` | `sh` runs, and `bash` runs as its child: the denied file has a rule |
| B2 | the same lists, the program's rule put through the deny check | no rule; the helper's `execve` of `/usr/bin/sh` is `EACCES`, exit 126 |
| B3 | the loader denied, its rule put through the check | a dynamic program is `EACCES`, exit 126; a static program runs |
| B4 | `/usr/bin` listed, `/bin/id` denied | `/usr/bin/id` and `/bin/id` both run: the deny entry denies nothing |
| B5 | `/bin` listed, `/usr/bin/id` denied | both run again; and listing `/bin` admits `/usr/bin` |
| B6 | `/usr/bin` listed, the file denied under both names, `/bin/id` and `/usr/bin/id` | both are `Permission denied`; `/usr/bin/true` runs |
| B7 | `/usr` listed, the file denied under both names | the same |
| B8 | `/usr/bin/id` and a workspace script denied; the loader given `/usr/bin/id`, and `sh` given the script | direct execution of each is denied (126); `ld.so /usr/bin/id` prints the uid; `sh script` runs the script |
| B9 | a program in an expanded directory: overwritten in place with `cp`; removed and written again; replaced by `mv` of a new file | in place: still runs. Removed and written again: `Permission denied`. Renamed over: `Permission denied` |
| B10 | the helper's bytes in a sealed memfd, handed to bubblewrap with `--perms 0555 --ro-bind-data` | the helper runs from `/gate` and restricts; the run cannot execute `/gate` (126) or append to it; its mode is 555; no temporary file is left in the view's root |
| T1 | `git` with `/usr/bin` listed | `status`, `log`, `gc`, `bisect start` and a clone of a workspace repository over the `file` transport work |
| T2 | the same without and with `/usr/lib/git-core` | without it `ls-remote https://…`, `submodule`, `mergetool` and `difftool` fail: "cannot exec 'git-remote-https': Permission denied", and the same for `git-submodule` and `git-mergetool`. With it each gets past the exec |
| T3 | `cc -o out hello.c` with `/usr/bin` listed | `cannot execute '/usr/lib/gcc/x86_64-pc-linux-gnu/16/cc1': posix_spawn: Permission denied` |
| T4 | T3 with `/usr/lib/gcc/x86_64-pc-linux-gnu/16` | compiles and links |
| T5 | `rustc --version` through the rustup proxy `/usr/bin/rustc`, the rustup home bound read-only at `/opt/rustup`; `/usr/bin` listed | `command failed: 'rustc': Permission denied` |
| T6 | T5 with the toolchain's `bin` | prints the version |
| T7 | `rustc hello.rs` with the toolchain's `bin` and the gcc directory | `linking with cc failed`: `collect2: fatal error: posix_spawnp: Permission denied` (the toolchain's own linker lies beneath its `lib`) |
| T8 | T7 with the whole toolchain directory | compiles and links |
| T9 | `cargo about --version`, `cargo-about` in the cargo home's `bin` bound at `/opt/cargo/bin`, without and with that directory | `could not execute process … Permission denied` without it; prints the version with it |
| T10 | `npm --version` with `/usr/bin` listed | `EACCES`: `/usr/bin/npm` is a link to `/usr/lib/node_modules/npm/bin/npm-cli.js` |
| T11 | T10 with `/usr/lib/node_modules` | prints the version (`/usr/bin/env` and `/usr/bin/node` are beneath `/usr/bin`) |
| T12 | `npm run hello`, the script in the project's `node_modules/.bin`, without and with the project's `node_modules` listed | denied (126) without it, runs with it |
| T13 | a run with `target` listed runs `git clean -fdx`, creates `target` again and copies its program back | `target` is gone after the clean; the program in the new `target` is `Permission denied` |
| T14 | a list naming `node_modules` in a clone that has none | the probe helper cannot open the entry and executes nothing |

## 4. The rule

**D1. On `linux` a profile with a non-empty `exec_subpaths` is held to its literal program and the
list. A profile with an empty list runs as today. `macos` does not change: with an empty list only
the program executes.**

Decided by the stakeholder (agent:sdc), 2026-11-21, revisable before the wire story publishes the
property (§ 19 Q2): the difference stays and is stated, here and in the reference text of the
profile field (§ 5, *Reference text*). The reasons:

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
  fresh clone has no `target` and no `node_modules`, so a profile that lists one is refused at
  the start (T14 shows the helper's side). The client creates the directory in the workspace
  before the start; § 6, *Build housekeeping*, says what happens to it afterwards.
- **`/tmp` is not eligible.** It is a fresh tmpfs per run and writable. `macos` does not admit its
  private `TMPDIR` either.
- **A rule is on a directory, not on a name.** Where the view mounts one host directory twice, a
  rule on either mount admits both: listing `/bin` admits `/usr/bin` on a host whose `/bin` is a
  link into `/usr` (B5). D23 covers what that means for a deny entry.

### Reference text

The difference of D1 and the rules of this section are stated where a client reads the field:

- the doc comment of `ConfinementProfile.exec_subpaths` and its row in the field table
  (`profile.rs:17-36`, `:71-72`): an empty list runs every program of the view on `linux` and only
  the program on `macos`; on `linux` an entry is a view path and must exist;
- the module documentation of `linux.rs` (`:17-31`) and `macos.rs` (`:28-35`);
- the confinement pages under `website/docs`, which `cargo xtask check-docs` holds to the
  specification (§ 14).

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

- **In the view: its bytes, not its file (D20).**
- **In the argv.** After `/usr/bin/env -u PWD --` and the optional `PWD` link
  (`process.rs:2849-2853`), directly before the program:
  `… /usr/bin/env -u PWD -- <gate> <option>… -- <program> <argument>…`, with the options of D24.
  Everything bubblewrap, `setsid` and `env` execute happens before the ruleset exists. The only
  `execve` after it is the program's.
- **Only with a list.** A start whose profile has an empty `exec_subpaths` gets no helper and no
  bind. Its argv is today's, byte for byte.

**D20. The driver reads the helper once, at open, through one descriptor, and binds those bytes
into every view.** The first draft bound the file by descriptor and compared it by name before
each start, as `backend_unchanged` does for bubblewrap (`probe.rs:510-528`). That leaves a gap
between the comparison and the bind. This closes it:

1. At open the driver opens the configured path once. Every later fact comes from that
   descriptor: `fstat` (a regular file, at most `MAX_BACKEND_BINARY_BYTES`, `probe.rs:22`), the
   path the descriptor has (`/proc/self/fd/<n>`, for D21), and the bytes.
2. It copies the bytes into a memfd, seals it against write, grow and shrink, and takes the
   SHA-256 of the sealed bytes. The driver already seals memfds for secret slots and proves that
   one crosses bubblewrap (`probe.rs:839-845`).
3. `BackendBinding` (`probe.rs:25-34`) gains that SHA-256, so the snapshot digest binds the bytes.
   The file's device, inode and name are not bound: they are not read again.
4. The probes of § 9 and every start hand bubblewrap a fresh read-only descriptor of the sealed
   memfd with `--perms 0555 --ro-bind-data <fd> <gate>`. Bubblewrap copies the bytes into a
   temporary file outside the view's root, binds it read-only at `<gate>` and unlinks the
   temporary name (`bubblewrap.c:1531-1572` of 0.11.2; B10 on 0.13.0). `<gate>` is one fixed path
   outside every location § 5 makes eligible.

What follows:

- What was probed and hashed is what runs. No name is resolved at a start, so there is nothing to
  swap between a check and a bind.
- The run can read `<gate>` and cannot write or execute it (B10, W3).
- A helper replaced on disk changes nothing until the adapter is opened again. The embedder
  reopens the adapter after an upgrade. `exec.capability-stale` is not raised for the helper.
- Each start copies the helper's bytes once. The probe helper is 17 KiB; the size of the real
  one is not known yet (§ 18).

**D22. The lists do not travel in `AdapterStart`.** `AdapterStart` derives `Default`, and the three
wire paths pass `AdapterStart::default()` (`process.rs:816`, `:848`, `:958`): a wire exec start, a
start with Git roots, and a session start. A list added as a field there would be empty on those
paths without anyone having written so, and a fourth caller could forget it the same way. The
type also derives `Copy`, which a list cannot.

- The lists are a separate, required argument of `start_for_adapter`, `start_inner`, `command`
  and `render`, of a type `ExecLists` that implements neither `Default` nor `Copy`.
- `ExecLists` has two constructors and no public fields. `ExecLists::of(&AdmittedProfile)` is the
  adapter's: it carries every entry of both lists. `ExecLists::wire()` is what the three wire
  sites write out: no list, because a wire start has no field for one (§ 15).
- A call that omits the argument does not compile. `render` gets it too: today it builds the
  argv from the wire input alone (`linux.rs:625-640`) and would show an argv without the helper.
- The compiler cannot stop the adapter from passing `ExecLists::wire()`. A unit test holds that
  `ExecLists::of` is not empty for any admitted profile with a list, and the lane cases of § 16
  fail when a denied program runs.

`ExecStartInput` is a wire type and does not change.

### The helper's arguments

**D24. The driver states for every path which mount it lies in and how it is to be opened.** The
helper cannot know which locations a run can write; the driver's mount table does.

```text
substrate-exec-gate --version
substrate-exec-gate <option>... -- <program> [<argument>...]

<option>  = --program <class> <root> <rel>
          | --loader  <path>
          | --allow   <class> <root> <rel>
          | --deny    <class> <root> <rel>
          | --alias   <path> <path>
<class>   = follow | beneath
<root>    = the absolute view path of a mount: /usr, /bin, /lib, /lib64, /workspace, or the
            mount of a declared root or file
<rel>     = a normal relative path beneath <root>, or "." for <root> itself
<path>    = an absolute view path
<program> = the text of <root>/<rel> of --program
```

- **`follow`** is for a mount the run cannot write: the base directories, a read-only root, a
  read-only file. The helper opens `<root>/<rel>` with `open(O_PATH | O_CLOEXEC)` and follows
  symbolic links.
- **`beneath`** is for a mount the run can write, in this or an earlier start: `/workspace`,
  whatever its access mode, and a writable root. The helper opens `<root>` with
  `O_PATH | O_DIRECTORY | O_NOFOLLOW` and `<rel>` beneath it with `openat2`, `RESOLVE_BENEATH`
  and `RESOLVE_NO_SYMLINKS`. A symbolic link in any part is an error.
- **`--program`** occurs exactly once and `--loader` at most once. `--allow` carries an entry of
  `exec_subpaths`, `--deny` an entry of `exec_denied_paths`. `--alias` is D23's.
- Every operand is its own argument. A `<root>` is absolute and `<class>` is one of two words,
  so no operand reads as an option. The helper takes options only before `--`.
- An unknown option, a missing operand, a `<rel>` that is absolute or not normal, a second
  `--program`, or a `<program>` that differs from its option: the helper exits 125 and executes
  nothing.
- An `--allow` or `--program` path that does not open is an error. A `--deny` path that does
  not exist is kept as a name (§ 8.2).

The first draft's `--allow <path>` could not carry the rule of § 5: one absolute path does not
say where the writable mount begins.

### Embedder notes

An embedder of the in-process adapter links `b10x-substrate-host` as a library. Cargo does not
build a dependency's binary, so the helper does not arrive with the crate. Loom is the first such
embedder. It must:

1. **Build `substrate-exec-gate` from the revision it pins the crate at**, and install it beside
   its own binary. A static build is recommended (`x86_64-unknown-linux-musl`, the target the
   `Dockerfile` builds `substrate-executor` for): it starts in any view. A default build needs the
   build host's loader path and a compatible `libc` beneath `/usr`, `/lib` or `/lib64` of every
   host it runs on.
2. **Name it in `HostConfig.exec_gate`** as an absolute path. The driver searches nowhere: not
   `PATH`, not the directory of the running executable.

What the file must be:

| requirement | why |
|---|---|
| named by an absolute path | D21 |
| outside the workspace root and every declared writable root | D21: a run could have written it |
| a regular file the driver's uid can read | the driver reads its bytes (D20). It needs no execute bit: bubblewrap sets the mode of the copy |
| at most 64 MiB | `MAX_BACKEND_BINARY_BYTES` (`probe.rs:22`) |
| able to start inside the view | only `/usr`, `/bin`, `/lib` and `/lib64` of the host are there (`process.rs:2732-2743`) |
| of the driver's protocol | it answers `--version` with the line `substrate-exec-gate/1` and takes the arguments of D24 |

What the driver does with it at open, in order: the trust check of D21; the read, seal and hash
of D20; one run of `<gate> --version` in a throwaway sandbox, from the sealed bytes, which must
print the protocol line; the two probes of § 9; the SHA-256 into `BackendBinding`. The driver
never executes the file outside a sandbox. Who may write the file and its directory is the
embedder's to settle, as it is for `HostConfig.bubblewrap`.

**Deriving a list.** A list names what the step's tools execute, in view paths. Each line below
was run on the probe host (§ 3.7); the paths are that host's.

| tool chain | entries | found with |
|---|---|---|
| the base tools | `/usr/bin` | where `/bin` is a link into `/usr`, it is the same directory (B5) |
| `git`, local work | `/usr/bin` (T1) | |
| `git` over HTTPS, `submodule`, `mergetool`, `difftool` | also `/usr/lib/git-core` (T2) | `git --exec-path` |
| `cc` | `/usr/bin` and `/usr/lib/gcc/x86_64-pc-linux-gnu/16` (T3, T4) | the directory of `gcc -print-prog-name=cc1`: `/usr/lib/gcc/<triple>/<version>` |
| a rustup toolchain | the whole toolchain directory, not only its `bin` (T5 to T8), and the gcc directory for the link | `rustup which rustc`, two levels up |
| cargo subcommands | the cargo home's `bin` (T9) | |
| `npm` | `/usr/bin` (for `env`, `node`, `sh`) and `/usr/lib/node_modules` (T10, T11) | the target of the `npm` link |
| a project's own tools | its `node_modules`, or `target` for build scripts and tests (T12) | relative entries, beneath the workspace |

- **A toolchain in a home directory is not in the view.** The client declares it as a read-only
  root, and it appears at the `mount` that root names (`ReadOnlyRoot { host_path, mount }`, bound
  at `process.rs:2774-2776`). The entry names the mount path. In the probes the rustup home was
  mounted at `/opt/rustup` and the entry was `/opt/rustup/toolchains/<toolchain>`, with
  `RUSTUP_HOME`, `CARGO_HOME` and `RUSTUP_TOOLCHAIN` set in the command's environment.
- **A missing entry shows as `Permission denied`** naming the program the tool could not start
  (T2, T3, T7), or as `bad interpreter` for a script (T12).
- **Every entry must exist at the start** (§ 5). The client creates `target` or `node_modules`
  in a fresh clone before it starts the run.

### Without a usable helper

**D19. A profile that carries a list never runs without it. Which refusal answers depends on what
the embedder configured.** Decided with D5.

| situation | answer | code and status |
|---|---|---|
| `exec_gate` is not set | the adapter opens; neither fact is proved; a profile with a list is refused at admission | `confinement.fact-unserved` naming `exec_subpaths`, 501 |
| `exec_gate` is set and is not absolute, or the file lies beneath the workspace root or a declared writable root (D21) | the open is refused before the file is read | `config.exec-gate-invalid`, 422, address `config.exec_gate`; new |
| `exec_gate` is set and the file is missing, not a regular file, unreadable, over the size bound, or does not print the protocol line in the throwaway sandbox | the open is refused; no adapter exists, so nothing is admitted | `config.exec-gate-unserved`, 501, address `config.exec_gate`; new |
| `exec_gate` is set and usable, and a probe of § 9 did not hold (no Landlock, ABI 1) | the adapter opens; the fact that probe proves is absent; a profile that needs it is refused at admission | `confinement.fact-unserved` naming the fact, 501 |
| the file changed after the open | nothing: the bytes read at open run (D20) | none |

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
- **Without the floor nothing is probed.** The protocol line is read in a sandbox, so a host
  without the exec floor reads none; both facts are absent there, with every other exec fact.

**D21. A helper beneath a location a run can write is refused.** The helper decides what a run may
execute. A file beneath `HostConfig.workspace_root` or a `HostConfig.writable_roots` path could
have been written by a run, so it cannot be that authority. The check is made on the path of the
opened descriptor, so a symbolic link in the configured path does not hide the location. The code
is `config.exec-gate-invalid`, a `DriverError` of class `Refused` (status 422), the form of
`config.writable-root-invalid` and `config.tmp-bytes-invalid` (`lib.rs:385`, `:475`). The same
check is not made for `HostConfig.bubblewrap` today; this document does not change that.

A deny-only profile is refused by D13 before any of these. In no row does a run start without its
list.

### The ruleset

**D6. The ruleset handles two rights, `LANDLOCK_ACCESS_FS_EXECUTE` and `LANDLOCK_ACCESS_FS_REFER`,
and nothing else.** The helper passes the first ABI's attribute size, so `handled_access_net` and
`scoped` stay zero. Read, write and network behave as before.

| rule | right | why |
|---|---|---|
| `/` | refer | Any ruleset denies a rename or link into another directory unless it grants refer (R2). Without this rule `git` and every build tool that renames across directories would fail. R3 restores it |
| the program | execute | the literal program, as a file rule, unless D23 withholds it |
| the host's ELF loader | execute | the kernel opens a dynamically linked program's loader with the execute right (A1, A2). Shared libraries need none (A2, A10). Unless D23 withholds it |
| each entry of `exec_subpaths` | execute | D1; expanded around a denied path (§ 8.2) |

Every execute rule, whatever its source, goes through the deny check of D23 (§ 8.2). The first
draft exempted the program and the loader, and B1 shows what that allowed.

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
  rule belonged to the removed directory.
- **Mounting stays impossible.** It already was (W6).
- **`ptrace` is unchanged within the run.** Landlock limits tracing to processes of the same or a
  nested domain (`landlock(7)`, *Ptrace restrictions*); every process of the run is in the one
  domain.

### Build housekeeping

The two changes above meet ordinary build tools. A client that lists a build directory has to
plan for them.

- **`cargo clean`, `npm ci` and `git clean -fdx` remove a listed directory and the build creates
  it again.** The new directory has no rule, and what the build puts there does not execute (T13
  for `git clean -fdx`; that `cargo clean` removes `target` itself and `npm ci` removes
  `node_modules` first is those tools' documented behaviour, not probed).
- **Listing the parent is not the way out.** The parent of `target` or `node_modules` is the
  workspace. Listing it makes every file the run writes executable. A deny-list beside it then
  denies nothing the run cannot undo with a copy (§ 8.4), so the point of the deny-list is gone.
- **What the client does instead.** It runs the housekeeping step as a start of its own, creates
  the directory again from outside the run, and lists it in the next start. Or the step empties
  the directory and keeps the directory itself.
- **A hard link from a cache fails `EXDEV`.** pnpm and uv link packages from a store into
  `node_modules` or `.venv`. From an unlisted directory into a listed one that is R4. From a
  store on another mount of the view it fails without any list: a hard link never crosses two
  mounts. That both tools then copy is their documented fallback and was not run here.
- **A rename into a listed directory fails the same way** (R4). A tool that writes beside the
  listed directory and renames the result in gets `EXDEV`.

### When the helper fails

The helper executes the program only after `landlock_restrict_self` returned 0. Any earlier
failure (no Landlock, an argument it does not accept, an entry it cannot open, a rule the kernel
refuses) ends the run before the program starts: one diagnostic line on standard error that
begins `substrate-exec-gate:`, and exit status 125. When `execve` of the program fails it exits
126, or 127 for `ENOENT`, as `/usr/bin/env` does in that position today. The driver has checked
every entry before dispatch (§ 5), so a helper failure means the kernel changed its answer after
the probe or a path changed after the check. Nothing runs unrestricted (invariant 3).

**The status is ambiguous.** A program that itself exits 125, 126 or 127 looks the same to the
adapter. Only the diagnostic line tells them apart, and a program can print that line too. A
named failure needs the status descriptor of § 20.

## 7. What the list does not stop

**D9. The list restricts which files `execve` and `execveat` accept. It does not restrict what
code an allowed program runs.** The fact's documentation states these limits; each is a probe.

- **An allowed interpreter runs any script it can read** (A7). `sh script`, `python script` and
  `sh -c` do not execute the script file. A `#!` script outside the list is refused (A6).
- **The loader is an interpreter for programs.** A dynamically linked program cannot start unless
  the loader is executable (A1), and the loader started directly runs any program file the run can
  read (A8). No Landlock right separates the two uses.
- **A memfd is outside the list's reach.** A program that copies bytes into a memfd and executes
  it runs them (A9). Decided by the stakeholder (agent:sdc), 2026-11-21, revisable before the
  wire story publishes the property (§ 19 Q3): these two stories add no `memfd_create` refusal to
  the seccomp program, and the limit is stated. A refusal would not close the route: a copy into
  a writable listed directory still runs (W4, N3), and so does the loader given the file (A8).
- **Mapping a file as code is not execution** (A10). An allowed program can load any shared object
  it can read.
- **On `macos` the last three do not apply in the same way**: Seatbelt also confines
  `file-map-executable` to the listed subpaths (`seatbelt.rs:273-276`). The first applies there
  too.

The allow-list therefore keeps a run that only starts programs by path to the listed ones. It is
not a boundary against a program that sets out to run other code. The floor's other rules (the
namespaces, the seccomp program, the read-only view) are unchanged and do not depend on it.

## 8. The deny-list

**What a deny entry does: `execve` and `execveat` of that path fail.** It does not say the program
never runs. The limits of § 7 hold for a denied file as for an unlisted one: the loader given a
denied program runs it, and `sh` given a denied script runs it (B8). A copy in a listed directory
the run can write runs too (§ 8.4).

### 8.1 Surface

**D10. `ConfinementProfile` gains `exec_denied_paths: Vec<String>`.** An entry has the form of an
`exec_subpaths` entry (§ 5): a file or a directory, relative beneath the workspace or absolute in
the same locations, the same lexical rule. On `linux` a deny entry may be absent at the start
(§ 8.2).

- **Bound.** `MAX_EXEC_DENIED_PATHS = 64`, a constant of the confinement module. More entries are
  refused `confinement.exec-denied-paths-exceeded` (422) at admission on every adapter. The bound
  limits the size of a profile and of the helper's argument list. It does not bound a start's
  cost. The cost follows the number of rules the expansion writes, and that follows the size of
  the directories that hold a denied path, not the number of entries: one entry in `/usr/bin`
  wrote 5953 rules and cost 17 ms (W12), and 64 entries in that one directory would write about
  as many. What bounds a start is the helper's own limit of 65536 rules (§ 8.2), about 0.2 s at
  the measured cost per rule (§ 18). `exec_subpaths` has no bound today and gets none here; the
  wire story bounds both when it gives them a schema.
- **Fact.** `exec_denied_paths` (`Fact::ExecDeniedPaths`), directly after `exec_subpaths`.
  `FACT_ORDER` grows to 14. A non-empty `exec_denied_paths` implies it
  (`profile.rs:213-229`). The fact has no wire property in these stories, as `stdin_pipe` and
  `output_live` have none.
- **Precedence. A denied path wins.** It wins over a listed directory that holds it, over an entry
  that names the same path (N8), over the loader, and over the literal program (D23). A start
  whose program is denied is refused `exec.sandbox-program-unlisted` (422) before dispatch, the
  existing code for "not an executable of this policy" (`seatbelt.rs:38-39`).
- **Outside the list.** A deny entry that no allowed path holds changes nothing and is not an
  error.

### 8.2 `linux`: the helper expands

Landlock has no deny rule. A layer grants a path when one rule on the way to it grants the access
(`landlock(7)`), so a rule on `/usr/bin` cannot except `/usr/bin/id`.

**D11. The helper grants per entry where a directory holds a denied path.** For each path `A` that
is to receive the execute right (an entry, the program, the loader; D23), resolved:

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
- **A program rebuilt in an expanded directory loses the execute right.** A file overwritten in
  place keeps its rule; a file removed and written again, or replaced by a rename, is a new file
  without one (B9). Compilers and linkers replace their output. So a directory a build writes
  its programs into must not hold a deny entry.

The walk is bounded: the helper stops at 65536 rules and exits 125. The option the stories also
name, a non-executable file bound over the denied path, works (N9) and was not chosen: the denied
file would read as empty, a path that does not exist cannot be covered beneath a read-only mount
(N10), and a directory cannot be denied that way.

**D23. A denied file gets no execute rule from any source.** The first draft put only the entries
of `exec_subpaths` through step 1. The program's rule and the loader's rule were written without
it, and a second mount of a directory was not considered. Each is a way around a deny entry:

| source of the rule | the gap | what holds it now |
|---|---|---|
| the program | program `/usr/bin/sh`, a link to `bash`, with `/usr/bin/bash` denied: the rule lands on `bash`, and `bash` then runs under every name (B1) | the program's rule goes through step 1. No rule: the helper's `execve` is `EACCES` (B2) |
| the loader | a deny entry that is the loader, or holds it, was overridden by the loader's rule | the loader's rule goes through step 1. No rule: no dynamically linked program starts; a static one does (B3) |
| a second mount | `/usr/bin` listed and `/bin/id` denied: the entry is not beneath `/usr/bin` as written, `/usr/bin` gets one rule for the whole directory, and `id` runs under both names (B4) | `--alias`, below; with both names known `id` is denied under both (B6, B7) |

**A denied program is refused before dispatch.** D10 says a denied path wins, so the start cannot
succeed. The driver resolves the program and every deny entry through the descriptors it pins the
mounts with, as § 5 resolves an entry. It refuses the start `exec.sandbox-program-unlisted` (422)
when the program is a denied path, lies beneath a denied directory, or is the same file, by
device and inode, as a denied file. `/usr/bin/sh` beside a denied `/usr/bin/bash` is that case on
the probe host. The helper is the second line: should the driver miss it, the program has no rule
and nothing runs (B2). The driver does not refuse a denied loader. A static program still runs
under it, and the client wrote the entry.

**A second mount of one directory.** The view can show one host directory at two places: `/bin`
and `/usr/bin`, or `/lib`, `/lib64` and `/usr/lib`, where the host links them, and a declared root
whose host path lies inside another mount's. A Landlock rule is on the directory itself, so it
admits the directory under every mount (B5).

- The driver knows the host source of every mount of the view. For each pair of mounts where one
  source is, or lies beneath, the other, it passes `--alias <path> <path>`: the two view paths
  of that one directory or file. On the probe host these are `/usr/bin` and `/bin`, `/usr/lib`
  and `/lib`, `/usr/lib` and `/lib64`.
- The helper resolves each deny entry to a view path: the path of the descriptor it opened, or
  the path as written when the entry does not exist. It adds every path the aliases map that
  one to, and uses all of them in steps 1 to 3.
- This finds every case. A denied file lies inside an allowed directory only when their host
  paths nest. Then the sources of the two mounts nest as well, and an alias maps the deny entry
  to a path beneath the allowed directory as the client wrote it.
- A path an alias produces may be hidden by a later mount. Then a directory is expanded that did
  not need it, which fails closed.

B6 and B7 gave the helper both names by hand. The alias arguments and the helper's closure over
them are not probed (§ 18).

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

A deny entry denies a path, and every mount of that path (D23). It does not find every hard link
to a file. Decided by the stakeholder (agent:sdc), 2026-11-21, revisable before the wire story
publishes the property (§ 19 Q4): the limit is stated, and a deny entry whose file has more than
one link is not refused.

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
`/usr/bin/ln` into a temporary workspace and runs `/bin/sh -c` through the helper, bound from the
sealed bytes of D20 and given the arguments of D24, as a start does.

| fact | list | the script, as the run's program | proof |
|---|---|---|---|
| `exec_subpaths` | `allowed` | `allowed/true` succeeds; `outside/true` fails; `/usr/bin/true` fails; `allowed/ln outside/true elsewhere/linked` succeeds | exit 0, and `elsewhere/linked` exists on the host |
| `exec_denied_paths` | `allowed`, deny `allowed/sub/true`; the probe binds `allowed/sub` a second time at `/second` | `allowed/true` succeeds; `allowed/sub/true` fails; `allowed/sub/again`, a hard link to it, fails; `/second/true` fails | exit 0, and `exec_subpaths` was proved |

The second mount makes the deny probe prove D23's alias handling, not assume it.

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
| `config.exec-gate-invalid` | 422 | `HostConfig.exec_gate` is not absolute, or the file lies beneath the workspace root or a declared writable root (D21) | the open of `linux`, and of `systemd` around it, before the file is read; new |
| `config.exec-gate-unserved` | 501 | `HostConfig.exec_gate` is set and the file is missing, not a regular file, unreadable, over the size bound, or does not print the protocol line (D19) | the open of `linux`, and of `systemd` around it; new |
| `confinement.fact-unserved` naming `exec_subpaths` | 501 | a non-empty `exec_subpaths` on an adapter that did not prove it: `linux` without `exec_gate`, `linux` whose probe did not hold, and every adapter of § 13 that proves neither fact | admission; existing |
| `confinement.fact-unserved` naming `exec_denied_paths` | 501 | a deny-list on an adapter that proved `exec_subpaths` and not this fact | admission; existing code, new fact |
| `exec.sandbox-path-unrepresentable` | 422 | an entry of either list that is not normal, holds a refused character, has a symbolic link beneath a writable location, or (allow entry on `linux`) does not exist; a program that is not absolute when a list is set | admission for the lexical rule, the start for the rest; existing code, new on `linux` |
| `exec.sandbox-exec-subpath-undeclared` | 422 | an absolute entry of either list outside the eligible locations (§ 5) | the start; existing code, new on `linux` |
| `exec.sandbox-program-unlisted` | 422 | the program is a denied path, lies beneath a denied directory, or is the same file as a denied file (D23) | the start, before dispatch; existing code, new use |

Every refusal of an admission or a start writes its audit record, as it does today
(`linux.rs:55-56`). A refused open has no profile and no sink, so it writes none; the embedder
gets the error from `open`. After dispatch there is no refusal: a helper failure is exit status
125, which a program can also return (§ 6). No row lets a profile with a list run without it
(D19). `exec.capability-stale` is not in the table: the helper's bytes are read once (D20).

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
lane cases. The stakeholder (agent:sdc) accepted this change to the story's acceptance explicitly
in the review of 2026-11-21.

Story 1, `spec/confinement/confinement.yaml`:

- `:192-201`, the `linux` declared set: add `exec_subpaths` ("Landlock execute right, applied
  inside the sandbox, design 35"). The `UNMAPPED` sentence at `:199-201` keeps `private_tmpdir`
  only.
- No type, error or outcome changes: `exec_subpaths` and `fact-unserved-exec-subpaths`
  (`:560-567`) exist. The comment of `exec_subpaths` at `:41` gains what an empty list means on
  `linux` and on `macos` (D1).
- Scenarios: `linux-admits-exec-subpaths-on-delegated` and
  `systemd-linux-admits-exec-subpaths-on-delegated`, in the form of
  `spec/confinement/scenarios/macos-s2-admits-exec-subpaths-on-macos.yaml`. Every existing
  `linux-*-on-delegated` and `systemd-linux-*-on-delegated` scenario lists the adapter's facts in
  its open step; each gains `exec_subpaths` after `ipc_signals_contained`.
- `spec/confinement/README.md`: the lane table and the sentence at `:149` that lists
  `exec_subpaths` on `linux` among the facts no story declares.

Story 2:

- `:31-49`, the fact comments: add `exec_denied_paths: execve of these paths fails, even beneath
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
requires it is still `confinement.fact-unserved` (501). `substrate-wire/0.27.0`, the bundle `main`
advertises (`crates/substrate-wire/src/lib.rs:18`), and its digest stay what the daemon
advertises. The three wire paths pass `ExecLists::wire()` (D22): a wire start has no list.

None of the three defaults of § 19 (the empty-list difference, no `memfd_create` refusal, the
hard-link limit) reaches a wire property or release notes without being decided again or
confirmed when `story:exec-start-carries-executable-lists` is planned.

## 16. Tests and lanes

Each case runs its scenario once. A case named `limit_…` asserts a limit of § 6, § 7 or § 8.5 as
it is, so a kernel or tool change that moves it is seen.

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
- D24 and § 5: the workspace entry `bin`, replaced by a symbolic link to `/usr/bin` after the
  driver's check and before the helper opens it, ends the run with status 125, and nothing of
  `/usr/bin` runs. The case makes the swap through a test hook between the two steps;
- D20: the run cannot write or execute the helper in its view; the helper's file replaced on
  disk after the open changes no later start;
- D19 and D21, each with nothing run:
  - an adapter opened without `exec_gate`: a profile with a list is `confinement.fact-unserved`
    naming `exec_subpaths`, with its `profile_refused` record, and a profile without a list runs;
  - `exec_gate` naming a file in a workspace, a file beneath a declared writable root, a symbolic
    link outside both that points into a workspace, and a relative path: each open is
    `config.exec-gate-invalid`, status 422;
  - `exec_gate` naming a path that does not exist, a directory, and an unreadable file: each open
    is `config.exec-gate-unserved`, status 501, and the message names the path;
  - `exec_gate` naming a program that does not print the protocol line: the same refusal;
- `systemd` around `linux`: one run with a list (`ess_confinement_systemd`).

Story 2:

- `/usr/bin` allowed, one program beneath it denied: another program in `/usr/bin` runs, the
  denied one fails inside the run (the story's acceptance);
- **D23, the review's example.** A read-only root holds a program `real` and a symbolic link
  `sh` to it, as `/usr/bin/sh` links to `bash` on the probe host. The profile lists the root and
  denies `real`.
  - With the program `sh`, the start is refused `exec.sandbox-program-unlisted` before dispatch,
    with its audit record, and nothing runs.
  - With another program of the root, the run starts, and its `execve` of `real` and of `sh`
    both fail with status 126.
  - A unit test gives the helper the same arguments without the driver's check: it writes no
    rule for the program and exits 126 (B2);
- D23, the loader: a deny entry that is the host's loader. A dynamically linked program exits 126
  and a static one runs (B3);
- D23, a second mount: `/usr/bin` listed and `/bin/<program>` denied, on a host whose `/bin` is a
  link into `/usr`: the program fails under both names (B6). On another host the case prints
  that it is absent and why;
- a symbolic link to the denied program, its hard link in the same directory, a denied directory,
  a deny entry created during the run: each denied;
- `limit_a_hard_link_that_existed_in_another_listed_directory_runs`,
  `limit_a_copy_in_a_listed_writable_directory_runs`,
  `limit_the_loader_runs_a_denied_program`, `limit_sh_runs_a_denied_script`,
  `limit_a_program_replaced_in_an_expanded_directory_no_longer_runs`;
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
- `process.rs` and `linux.rs`, D22:
  - `ExecLists::of` carries every entry of both lists of an admitted profile, and is never the
    value `ExecLists::wire()` returns when either list has an entry;
  - `render` and `command` of an admitted profile with a list place the helper directly before
    the program and after `env`, with one `--allow` per entry, and bind it with
    `--ro-bind-data`;
  - a start without a list renders the argv it rendered before;
  - the wire constructor is named at exactly the three wire sites: a source test in the form of
    `every_probe_sandbox_carries_the_user_namespace_posture` (`probe.rs:1660`) counts them, so a
    fourth site fails until this design is revisited.
- the helper's own tests, D24: each malformed argument list exits 125 and executes nothing; a
  `beneath` entry with a symbolic link in it is refused; `--alias` maps a deny entry under one
  path to the other.
- `probe.rs`: `the_published_set_is_the_linux_adapters_proof` with its named exception;
  `every_probe_sandbox_carries_the_user_namespace_posture` counts the new probes;
  an unset helper, and one that exits without restricting, each withhold both facts.
- `lib.rs`: the open checks of D19 and D21 decide on a path, metadata and a protocol line they
  are given, as `tmp_size_served_by` decides on help text (`lib.rs:409-427`), so they need no
  sandbox and run on every Linux host.
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
| D1 | `linux`: a non-empty `exec_subpaths` holds the run to its program and the list; an empty one runs as today. `macos` is unchanged. The difference is stated, not removed. Decided by the stakeholder (agent:sdc), 2026-11-21, revisable before the wire story publishes the property (Q2) |
| D2 | an entry names a directory or one file |
| D3 | on `linux` entries are view paths: relative beneath `/workspace`, absolute in the base system directories and declared mounts; they must exist; no symbolic link beneath a writable location; the program is absolute |
| D4 | Landlock, applied inside the sandbox by a helper that is the last program before the run's |
| D5 | the helper is `substrate-exec-gate`, a binary of this repository, named by `HostConfig.exec_gate`, absent from a start without a list. Decided by session `sdc`, 2026-11-21, `approval-record:sdc-decides-plan-and-design-2026-11-21`. Changed in the second revision: how it is bound is D20, its arguments are D24 |
| D6 | the ruleset handles execute and refer only; refer is granted on `/`. Changed: the program's and the loader's rules go through D23 |
| D7 | the host's loader, read at open from `/usr/bin/env`, is granted; never a loader read from the run's program |
| D8 | the fact needs Landlock ABI 2; the probe decides, never a version number |
| D9 | the list restricts `execve` targets, not the code an allowed program runs: interpreters, the loader, a memfd and mapped code are stated limits. No `memfd_create` refusal: decided by the stakeholder (agent:sdc), 2026-11-21, revisable before the wire story publishes the property (Q3) |
| D10 | `exec_denied_paths`, fact `exec_denied_paths` after `exec_subpaths`, at most 64 entries, a denied path wins. Changed: a denied program is refused by what it resolves to, not only as written (D23); the bound limits the profile, not the start's cost |
| D11 | `linux` denies by expanding a listed directory into per-entry rules; the expansion fails closed, and a program replaced in an expanded directory loses the right. Another hard link elsewhere beneath a listed path still runs: decided by the stakeholder (agent:sdc), 2026-11-21, revisable before the wire story publishes the property (Q4) |
| D12 | `macos` denies with `process-exec` and `file-map-executable` deny rules after every allow |
| D13 | a deny-list without an allow-list is `confinement.exec-denied-paths-without-subpaths`, 422, on every adapter, before any fact is read |
| D14 | the probe's result is a host-private `ExecListProof`, a second argument of `linux::proof` |
| D15 | each fact is proved by one throwaway run in which the allowed half happened and the refused half did not. Changed: the deny probe includes a second mount |
| D16 | `confinement.exec-subpaths` stays unpublished, and the probe test names that exception |
| D17 | a denied execution is `EACCES` inside the run |
| D18 | no adapter observes or records a denied execution; § 11 lists what could, and designs none of it |
| D19 | a profile with a list never runs without it: no `exec_gate` is `confinement.fact-unserved` (501) at admission; a set `exec_gate` that cannot be used is `config.exec-gate-unserved` (501) at open. Decided with D5. Changed: a file changed after the open has no effect (D20), so `exec.capability-stale` is gone from this row |
| D20 | the driver reads the helper once at open through one descriptor, seals and hashes the bytes, and binds those bytes into every view with `--perms 0555 --ro-bind-data`; no name is resolved at a start |
| D21 | an `exec_gate` that is not absolute, or lies beneath the workspace root or a declared writable root, is `config.exec-gate-invalid` (422) at open |
| D22 | the lists are a required argument of a type without `Default`, not a field of `AdapterStart`; the three wire sites write `ExecLists::wire()` |
| D23 | a denied file gets no execute rule from any source: an entry, the program, the loader, or a second mount of its directory (`--alias`); a denied program is `exec.sandbox-program-unlisted` (422) before dispatch |
| D24 | the helper's arguments: each path is a class (`follow` or `beneath`), a mount root and a relative path |

## 18. Inferred, not observed

- Everything about `macos` in D2, D12, § 8.5 and § 10: no macOS host ran a probe. The `macos-15`
  lane cases of § 16 decide each.
- The helper's `openat2` with `RESOLVE_BENEATH` and `RESOLVE_NO_SYMLINKS` (§ 5, D24): the probe
  helper used `open` with `O_PATH`. The repository already relies on `openat2` for guarded I/O.
- The run's seccomp program lets the Landlock system calls through: read from `seccomp.rs:160-192`.
  The probes ran without that program installed.
- `ENOSYS` and `EOPNOTSUPP` on a kernel without Landlock: from `landlock_create_ruleset(2)`; every
  probe ran on a kernel that has it. Hosts on Linux 5.13 to 5.18 report ABI 1: from the table in
  `landlock(7)`.
- GitHub's `ubuntu-24.04` runner kernel enables Landlock at ABI 2 or later: not checked. If it
  does not, the delegated lane fails at the first `linux` open step there.
- ESS can guard an outcome on one empty and one non-empty list input (§ 14).
- The start cost of an expanded directory scales with its entries, and 65536 rules cost about
  0.2 s: one measurement, 5953 rules in 17 ms.
- The helper's `--version` protocol line and the driver's open checks (§ 6, D19, D21): proposed,
  modelled on `check_tmp_size_served`; nothing of them was run.
- D20: B10 ran on bubblewrap 0.13.0. For 0.11.2 the options and the code were read, not run. The
  real helper's size, and so the bytes copied per start, are not known. That the copy counts
  against the run's memory bound is not checked.
- D23: the `--alias` arguments, the helper's closure over them, and the argument that they find
  every case. B6 and B7 gave both names by hand. That the driver can compare a view path with a
  deny entry by device and inode before dispatch is not probed.
- D22: `ExecLists` and its two constructors are a proposal; the three sites and the adapter's
  literal were read (`process.rs:816`, `:848`, `:958`, `lib.rs:1247-1252`).
- § 6, *Build housekeeping*: that `cargo clean` removes `target` itself and `npm ci` removes
  `node_modules`, and that pnpm and uv copy when a hard link fails. `pnpm` and `uv` are installed
  on the probe host and were not run: both need a package index.
- § 6, *Deriving a list*: every row was run on one host (Manjaro, gcc 16, git with 186 programs
  in `/usr/lib/git-core`, rustup 1.100.0 toolchain, npm 12.2.0). Paths differ on other hosts.
  That rustup installs its proxies in the cargo home's `bin` where the distribution does not
  package them is not checked here: on the probe host they are `/usr/bin/cargo` and
  `/usr/bin/rustc`, links to `/usr/bin/rustup`.
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

## 19. The three defaults the stakeholder decided

Open questions: none open.

Q1 of the first draft, which program applies the ruleset, was decided first: the helper of this
repository (D5, D19), by session `sdc` on 2026-11-21 under
`approval-record:sdc-decides-plan-and-design-2026-11-21`.

The three questions below are security defaults. The operator ruled on 2026-11-21 that the
stakeholder of a request decides on his behalf
(`approval-record:stakeholder-decides-for-the-operator-2026-11-21`). Under that ruling the
stakeholder, session `sdc`, decided all three on 2026-11-21. This section records each decision
and the options not taken. None of the three reaches a wire property or release notes without
being decided again or confirmed when `story:exec-start-carries-executable-lists` is planned
(§ 15).

**Q2. What does an empty `exec_subpaths` mean on `linux`?**

- **A. As today: every program in the view executes**, and the difference from `macos` is stated
  (D1), in this document and in the field's reference text (§ 5). A client that wants `macos`'s
  rule lists the program itself.
- **B. The program only, as on `macos`.** One rule on both adapters. It changes every existing
  `linux` run. The change is not contained in the rule alone:
  - D19's first row goes: every profile would need the helper, so an adapter without `exec_gate`
    could start nothing;
  - the branch that leaves the helper out of a start without a list goes, and with it the test
    that such a start renders today's argv;
  - the three wire paths have no list (D22), so every wire start would be held to its program
    unless the wire were exempt, which is a second rule;
  - every existing delegated-lane case and runtime vector that starts a second program would
    need a list.
- **C. An absent list keeps today's behaviour, an explicitly empty list means the program only.**
  It needs the field to tell absent from empty. `exec_subpaths: Vec<String>` cannot: the field
  becomes `Option<Vec<String>>`, a breaking change for every embedder that writes the field. The
  specification changes with it: `AdmitProfile`'s input becomes an optional list, a present list
  requires the fact even when empty, and a scenario holds the empty case. On `macos` absent and
  empty mean the same. The branch that leaves the helper out keys on absent, not on empty.

Decided by the stakeholder (agent:sdc), 2026-11-21, revisable before the wire story publishes the
property: **A** for these two stories. B is rejected because it changes every existing `linux`
run. C is a follow-up, `story:explicitly-empty-exec-subpaths-means-the-program-only`, a draft
that is not in this slice.

**Q3. Should the floor refuse `memfd_create` so that A9 closes?**

- **A. Leave it** and state the limit (D9). `memfd_create` has ordinary uses.
- **B. Refuse `memfd_create` in the seccomp program** when a profile has a list. This changes the
  floor for those runs and is its own design (ADR 0039 owns the program).

A refusal would not close the route. A copy into a writable listed directory still runs (W4,
N3), and the loader given the file runs it (A8); no Landlock right closes that. B would gain one
route out of three.

Decided by the stakeholder (agent:sdc), 2026-11-21, revisable before the wire story publishes the
property: **A**. No `memfd_create` refusal.

**Q4. A denied program that has another hard link beneath a listed path (N4).**

- **A. State the limit** (§ 8.5). On the probe host 106 files in `/usr/bin` have more than one
  link.
- **B. Refuse a deny entry whose file has a link count above one** on `linux`, by a new named
  refusal. It closes N4 and refuses, for example, a deny entry for a `zsh` that is installed
  under two names. It would not hold on `macos`, where the rule names a path.

Decided by the stakeholder (agent:sdc), 2026-11-21, revisable before the wire story publishes the
property: **A**. The limit is stated, a `limit_` case holds it (§ 16), and no deny entry is
refused by link count.

## 20. Not decided here

- A way to make a denied execution visible afterwards. § 11 lists the options; none is designed,
  and no story of the epic promises the record.
- A named failure for a helper that exits 125 after dispatch. A status descriptor from the helper
  to the driver would make it one (W8 shows a descriptor reaches that place); the stories do not
  need it.
- An explicitly empty list that means the program only:
  `story:explicitly-empty-exec-subpaths-means-the-program-only`.
- The same trust check and read-once binding for `HostConfig.bubblewrap` (D20, D21).
- `oci` and `pod`, and the wire field: other stories of the epic.
- A bound on `exec_subpaths`.
