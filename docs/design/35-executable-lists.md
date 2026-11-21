# Design 35: an executable allow-list and deny-list on the confinement adapters

Status: the fourth revision, `8c7318642`, was accepted by the stakeholder, session `sdc`, on
2026-11-21, and the fifth, `edac77173`, wrote the one change it decided with that acceptance
(D27). The sixth revision, `767fbea57`, closed a hole that two reviews of story 1's
implementation found in the design (D28). A second pass of both reviews on that commit found
the route D28 had marked as not checked, and this seventh revision closes it (D29): the helper
is statically linked, and on a start with a list no program runs in the view before it. The
coordinator of the request decided D29 on 2026-11-21 and gave it to the implementor. The
stakeholder, session `sdc`, read the decision and has no objection; it has not read this text.
Serves `story:linux-executable-allow-list` and
`story:executable-deny-list` (`epic:runs-are-governed-and-limit-hits-are-events`). It changes no
wire byte and no contract bundle: the lists are substrate-host's confinement API
([ADR 0044](../../adr/0044-confinement-is-a-contract-family-served-by-declared-adapters.md)
decisions 7 and 8). No code before acceptance (invariant 8). § 17 lists the decisions, § 18 every
claim that is inferred and not observed, § 19 the three defaults the stakeholder decided.

Revised seven times on 2026-11-21.

- After the first review: session `sdc` decided the helper (D5, D19) under
  `approval-record:sdc-decides-plan-and-design-2026-11-21`.
- After the stakeholder's independent review: D20 to D24 are new, D5, D6, D10, D11 and D19 changed,
  and § 3.7 holds the probes behind them. The operator ruled on 2026-11-21 that the stakeholder of
  a request decides on his behalf
  (`approval-record:stakeholder-decides-for-the-operator-2026-11-21`). Under it the stakeholder,
  session `sdc`, decided the three security defaults: D1, the memfd limit of D9 and the hard-link
  limit of § 8.5 (§ 19).
- Third revision. The stakeholder accepted the document at `ab22126ff` on 2026-11-21, and its
  confirming read asked for five corrections. They are in D20 to D24, and § 3.8 holds their
  probes. D25 is new: one definition, the *place* of a path, replaces the `--alias` option and
  carries the checks of D21, D23 and D24. The review proposed to compare the device and inode
  of a deny entry's view ancestors instead; a probe shows where that rule lets a denied file
  run (K2), so D25 is what the document specifies.
- Fourth revision. The stakeholder accepted D25 on 2026-11-21 with two conditions, and the
  third revision is `d6787264e`. Condition 1: where a place cannot be established, the answer
  is a named refusal and never a comparison of path text. D26 is that rule, with a closed list
  of file-system types. Condition 2: the place rule is probed on overlayfs before the deny
  story merges; the coordinator added tmpfs. § 3.9 holds those probes. Writing the refusal
  causes changed D25 in one point: a path's mount is now the row the kernel names by mount ID,
  not the row with the longest matching mount point, because a probe shows a table where the
  longest match is a hidden mount (S2).
- Fifth revision. The stakeholder accepted D26 and the fourth revision, the mount-ID change
  included, and decided the deployment question that D26 left as a consequence: "B now, C
  before the wire story". B: with a helper configured and a path on a file-system type outside
  the closed list, the adapter opens, the two facts stay absent, and the cause is visible in
  the open's result, in the adapter's proof and in the refusal of a profile with a list. C:
  `xfs` is probed in the quota fixture's container before the wire story is planned. D27 is
  that decision. It changes D19, D21 and D26 where they said such an open is refused. No probe
  was run for this revision; the code it cites was read at `8c7318642`.
- Sixth revision. Story 1 was implemented at `4b5208922`, and an adversary pass and a security
  review ran against it. Both found one hole that the first five revisions do not cover: the
  run's variables reached the programs that run before the ruleset exists, so a preload object
  named by the run's environment executed programs outside the list (§ 3.10, E1 to E3). D28
  closes it: on a start with a list, no variable of the run reaches a process that runs before
  `landlock_restrict_self` returned. D4, D5, D19 and D27 said the run cannot remove the ruleset
  and that nothing runs with a list that is not enforced; each now says that this holds with
  D28. The same reviews gave four corrections:
  - an entry whose view path is longer than the kernel takes is refused before dispatch (§ 5,
    E4);
  - the place of an absent path never joins `..` or `.` as text (D25, E5);
  - D26 names the cause of a mount table that is not UTF-8;
  - the probe of § 9 gains a half for D28.

  This revision ran no probe and built nothing: the implementor was changing code in the same
  tree. The code it cites was read at `4b5208922` with `git show`, and what the reviews
  measured was read from their cases and their logs.
- Seventh revision. The sixth is `767fbea57`, committed with the implementor's code for D28.
  The coordinator settled the five points that revision had left to it: the helper refuses a
  variable in its own environment; a start with a list gets no `--setenv` at all; a root
  spelled through `..` is `path-unopenable` and the open answers 501, and the review's
  assertion moved; the probe of § 9 uses no compiled object; a mount table that is not UTF-8 is
  `mount-table-unparsable`. A second adversary pass and a second security look then ran on
  that commit. Both found the route § 18 had listed as not checked (§ 3.11, E6 and E7): a
  profile with a list declares a read-only file at `/etc/ld.so.preload`, the file names an
  object in the workspace, and the loader of the dynamically linked helper maps that object
  before the helper's first instruction. D28 is about variables and does not see a file. D29
  is the answer, and it is about the helper, not about that one path:
  - the helper is statically linked, and the driver refuses at the open a helper that names a
    loader (`PT_INTERP`);
  - on a start with a list the helper is the first program bubblewrap starts in the view. A
    start with a list and a controlling terminal is refused before dispatch until the helper
    takes the terminal itself;
  - the lane builds the helper static, and refuses by name where it cannot.

  The stakeholder's words on the decision: "No objection to D29: static helper, the driver
  refuses a helper that needs a dynamic loader, and a listed start with a controlling terminal
  is refused before dispatch until the helper takes the terminal itself." It added two points,
  both accepted and written here: what exactly the open tests (D29, rule 1) and two permanent
  cases (§ 16). This revision ran nothing and built nothing; the code was read at `767fbea57`
  with `git show`, and the second reviews' cases and logs were read.

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

A fifth limit is stated since the sixth revision. Inside the domain, a preload object is loaded
into every allowed dynamic program and can run code, as an allowed interpreter can; `execve` of
an unlisted path still fails (§ 7). What the reviews found was not this limit: there the object
ran before the domain existed, and what it executed was held to no list (D28).

Since the seventh revision the limit names both of a loader's inputs: inside the domain the
program's own loader reads `/etc/ld.so.preload` and the environment, as an allowed interpreter
reads a script. It is observed now (E8). The second reviews found the same object running
before the domain a second time, through the file and not the variable; D29 closes that.

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

### 3.8 Probes for the confirming read

The probe helper gained three ways to decide whether a listed path holds a denied one:

- **text**: the two resolved view paths are compared as text;
- **ancestors**: the listed path has the device and inode of one of the deny entry's ancestors in
  the view, the rule the confirming read proposed;
- **place**: D25.

It also gained the `follow` check and the `openat2` open of D24, and a mode that prints places.
The probe host refuses a bind mount by an ordinary user (`mount: … must be superuser to use
mount`). The host bind mounts were therefore made inside `unshare -Urm`, and bubblewrap ran
inside that namespace with the floor's flags, `--disable-userns` among them.

Two views were used. In the base view `/bin` is a mount of `/usr/bin`, as in § 3.7. In the
host-bind view one directory is bound at a second host path and the two paths are mounted as
`/opt/a` and `/opt/b`; a second directory is mounted as `/opt/t`, and its subdirectory `bin`,
bound at a second host path, as `/opt/x`. "Runs" for the static tool means that it started.

| # | probe | observed |
|---|---|---|
| K1 | ancestors, one directory under two names: `/usr/bin` listed with `/bin/id` denied; `/bin` listed with `/usr/bin/id` denied; `/opt/a` listed with `/opt/b/tool` denied | in each the file is `Permission denied` (126) under both names and a neighbour runs |
| K2 | ancestors, a part of a listed directory under a second name: `/usr` and the loader listed with `/bin/id` denied; `/opt/t` listed with `/opt/x/tool` denied | `/bin/id` is denied and `/usr/bin/id` **runs**, with 3 rules written; `/opt/x/tool` is denied and `/opt/t/bin/tool` **runs** |
| K3 | one read-only descriptor of the helper's file given to two bubblewrap starts, each with `--perms 0555 --ro-bind-data` | first start: `/gate` has 17424 bytes and runs. Second start: `/gate` has 0 bytes |
| K4 | a file in a workspace with a hard link outside the workspace | `realpath` of the outside name prints the outside name; `stat` prints 2 links |
| K5 | place, the five lists of K1 and K2, and `/usr` listed with `/usr/bin/id` denied; text, the two lists of the host-bind view | place: the file is denied under every name in all six, and a neighbour runs; `/usr` was expanded into 5963 rules. Text: with `/opt/a` listed and `/opt/b/tool` denied the tool runs under both names; with `/opt/t` listed and `/opt/x/tool` denied it runs as `/opt/t/bin/tool` |
| K6 | places printed outside any sandbox, in the namespace with the bind mounts: a workspace directory bound at a second host path, and a directory inside it bound at a third | `realpath` prints the second and third paths unchanged. Their places are the places of the first: same device, same root |
| K7 | no deny entry; `/usr` and the loader listed | `/usr/bin/id` runs and `/bin/id` is `Permission denied`; the program `/bin/sh` is `EACCES`; a script that starts with `#!/bin/sh` is `EACCES`. With `/usr/bin` listed in place of `/usr`, `/bin/sh` and the script run. With `/usr` listed and no loader rule, `/usr/bin/sh` is `EACCES`: its loader is named through `/lib64` |
| K8 | `/usr/bin` and a workspace directory listed, `/usr` denied; the static tool executes `/usr/bin/id` | place: `EACCES`. Text: `id` runs |
| K9 | the `follow` check with `/workspace` as the writable mount, decided by ancestors and by place. (a) an entry in a read-only root that is a link into `/workspace`; (b) an entry in a read-only root whose host directory holds the workspace; (c) an entry in a read-only root whose host directory is a directory inside the workspace; (d) `/usr/bin` | (a) and (b): both refuse with exit 125 and one line; for (a) it is `/opt/ro/link resolves to /workspace/tools/bin, in the writable mount /workspace`. (c): place refuses; ancestors does not, and the entry is granted. (d): both pass |
| K10 | a read-only root whose host directory lies in the workspace, holding `link`, a symbolic link to `/usr/bin`, as an entry. Opened following links; opened with `openat2`, `RESOLVE_BENEATH` and `RESOLVE_NO_SYMLINKS` | following links: `/usr/bin/id` runs. `openat2`: `ELOOP` (40), exit 125, nothing runs. A directory beneath `/workspace` opens and its program runs. `tools/../../usr/bin` beneath `/workspace` is `EXDEV` (18) |

B6 and B7 of § 3.7 gave the helper both names of the denied file by hand. K5 gives it one.

### 3.9 Probes for the two conditions on D25

The probe helper was rewritten for D25 as the fourth revision states it. For every path it
opens the path, or the nearest ancestor that exists; asks `statx` for `STATX_MNT_ID`; takes the
one row of `/proc/self/mountinfo` with that mount ID; and joins the row's root with the
descriptor's path below the row's mount point. Each way this can fail prints
`cannot establish the place of <path>: <cause>` and exits 125 (D26). The helper has no other
way to compare two paths.

**Three file systems, one set of lists.** The lists of K5, K8 and K9 ran with every listed and
denied path on one file system, three times:

- **ext4**: a directory of the scratch directory.
- **overlay**: mounted inside `unshare -Urm` with its lower, upper and work directories in the
  scratch directory. The kernel gave that mount the options `redirect_dir=nofollow`,
  `index=off`, `uuid=null` and `metacopy=off`.
- **tmpfs**: mounted inside the same namespace at a path of the scratch directory.

Bubblewrap built the view with the floor's flags from that file system: one directory as
`/opt/a` and, through a host bind mount, as `/opt/b`; a directory as `/opt/t` and its `bin`,
through a host bind mount, as `/opt/x`; a workspace at `/workspace` and its `tools/bin` a second
time at `/second`. On the overlay the tree holds three kinds of file: `lowonly` and `cup` exist
only in the lower directory at the start, `uponly` was written through the mount, and the run
appends to `cup`, which copies it up.

What the view's table and `stat` said, per file system:

| file system | device and root of the row of `/opt/x` | `st_dev` of the files | type field |
|---|---|---|---|
| ext4 | `259:2`, the host path of the directory, `…/ext4/tree/bin` | `259:2` | `ext4` |
| overlay | `0:80`, `/tree/bin`: the path inside the overlay | `0:80` for the lower-only, the upper-only and the copied-up file alike; the inode of `cup` is the same before and after the copy-up | `overlay` |
| tmpfs | `0:298`, `/tree/bin` | `0:298` | `tmpfs` |

On all three the device of the row equals `st_dev` of the files, and two mounts of one directory
have rows with one device and one root.

| # | lists | right answer | ext4 | overlay | tmpfs |
|---|---|---|---|---|---|
| F1 | K5: `/opt/a` listed, `/opt/b/tool` denied; and the reverse | `tool` denied under both names, `other` runs | right | right | right |
| F2 | K5: `/opt/t` listed, `/opt/x/tool` denied | `/opt/t/bin/tool` and `/opt/x/tool` denied, `other` runs | right | right | right |
| F3 | K5: `/opt/t` listed, `/opt/t/bin/tool` denied | the same | right | right | right |
| F4 | `/workspace/tools` listed; `lowonly`, `uponly` and `cup` denied through `/second` | each denied under both names; `cupok` and `other` run | right | right | right |
| F5 | F4, then the run appends to `cup` and to `cupok` | `cup` still denied under both names, `cupok` still runs | right | right | right |
| F6 | K8: `/opt/t/bin` and a workspace directory listed, `/opt/t` denied | `execve` of `/opt/t/bin/other` is `EACCES`; without the deny entry it runs | right | right | right |
| F7 | K9: the `follow` check with `/workspace` writable, cases (a) to (d) | (a), (b) and (c) refused with exit 125, (d) passes | right | right | right |

On the overlay the upper directory held `cup` and `cupok` after F5 and not before: the copy-up
happened during the run. The four base-view lists of K1, K2 and K5 (`/usr/bin` with `/bin/id`,
`/bin` with `/usr/bin/id`, `/usr` with `/bin/id`, `/usr` with `/usr/bin/id`) ran again with this
helper and gave K5's answers, with 5963 rules for `/usr`.

| # | probe | observed |
|---|---|---|
| F8 | the lists of F1 to F3, F6 and F7 in an `ubuntu:24.04` container on Docker 29.8.2, on the container's own root: an `overlay` row that Docker mounted, with `index=off` and neither `metacopy` nor `redirect_dir` named. No bubblewrap is in that image; the second mounts are bind mounts made inside `unshare -Urm` in the container, and the container ran under `deploy/seccomp/host-exec-v1-amd64.json`, with no AppArmor profile and no network | every answer right. Row of `/`: device `0:80`, root `/`; `st_dev` `0:80` |
| F9 | in that container: `/usr` listed, a file of the image denied through a bind mount of `/usr/bin` at a second path; then the run appends to a denied and to an allowed image file | `id` is denied under both names and `true` runs, with 282 rules. After the copy-up the denied file is still denied under both names, the allowed one still runs, and `st_dev` and inode of both are unchanged |
| L1 | an overlay at `/opt/m` and its lower directory at `/opt/low`, both listed; `/opt/m/tools/tool` denied | denied as `/opt/m/tools/tool`; **runs** as `/opt/low/tools/tool`. The two have different places: `0:80 /tools/tool` and `259:2 …/lower/tools/tool`. They share an inode number and differ in `st_dev` |
| S1 | two mounts stacked on one mount point; the place of a file there | two rows with that mount point. The kernel names the upper mount's ID, and the place is in the upper mount's root. The file reads as the upper mount's |
| S2 | a mount at `hold/sub`, then a second mount over `hold`; the place of `hold/sub/z` | the kernel names the row of `hold`. The row with the longest matching mount point is `hold/sub`, the hidden mount, whose root is another directory. The file reads as the second mount's |
| S3 | a directory bound at a second path and then removed; the place of the mount point | the row's root is `…/gone//deleted`; `cannot establish the place of …/at: mount-root-deleted` |
| U1 | a deny entry on `/proc` in the view | `cannot establish the place of /proc/version: file-system-unlisted proc`, exit 125, nothing runs |
| U2 | places in the view: `/dev/null`, `/proc/version`, `/tmp`, `/dev`, `/` | `/dev/null` is `file-system-unlisted devtmpfs`, `/proc/version` is `file-system-unlisted proc`; `/tmp`, `/dev` and `/` are `tmpfs` rows and have places |
| U3 | the host's `/proc/sys` bound read-only at `/opt/p`, `/opt/p` listed and a file in it denied | `cannot establish the place of /opt/p/kernel/ostype: file-system-unlisted proc`, exit 125 |
| C1 | 10 starts with `/usr` expanded around `/bin/id` (5963 rules), the probe helper establishing every place anew | 92 ms per start. W12 measured 20 ms for about as many rules with the text comparison |

The rule failed nowhere on the three types. L1 is not a failure of the rule: a layer directory
is another file system to the kernel, and Landlock treats it so (`landlock(7)`, *Bind mounts
and OverlayFS*: "A policy restricting an OverlayFS layer will not restrict the resulted merged
hierarchy, and vice versa"). D26 states it as a limit.

### 3.10 What the two reviews of story 1's implementation measured

Not probes of this document. An adversary pass and a security review ran against story 1's
implementation at `4b5208922` on 2026-11-21, on the probe host, in the delegated lane. Each
finding is a case the review wrote, and each case failed there. This revision read the cases
and their logs and ran none of them. The files are
`crates/substrate-host/tests/adversary_exec_lists.rs`,
`crates/substrate-host/tests/security_exec_gate.rs` and
`crates/substrate-exec-gate/tests/security_gate.rs`.

| | case | what the run was | observed | expected |
|---|---|---|---|---|
| E1 | `adversary_a_preloaded_object_in_the_runs_environment_executes_outside_the_list` | the profile lists `bin`. The run's environment holds `LD_PRELOAD=/workspace/pre.so`, an object in the workspace whose constructor executes the run's script in the first process that loads it. The script runs `bin/tool`, `other/tool` and a base tool by path and prints each status | `0`, `0`, `0`, exit 0 | `0`, `126`, `126`: what the same start prints without the variable |
| E2 | `security_a_preload_named_by_the_commands_environment_does_not_lift_the_list` | the same list and variable. The constructor acts only in a process named `substrate-exec-gate`, and executes what follows `--` in its arguments | `listed:0`, `unlisted:0`: the constructor ran in the helper's process before `landlock_restrict_self` | `listed:0`, `unlisted:126` |
| E3 | `security_a_preload_that_acts_before_the_helper_starts_does_not_lift_the_list` | the same. The constructor acts only in a process that is not the helper and whose arguments name the helper's mount: `/usr/bin/env` | `listed:0`, `unlisted:0`: the constructor started the program itself, and the helper never ran | `listed:0`, `unlisted:126` |
| E4 | `adversary_an_entry_at_the_path_limit_is_refused_before_dispatch_or_runs` | a read-only root whose mount path is longer than its host path, and an entry beneath it whose host path is at most 4095 bytes and whose view path is longer | the start was dispatched, and the helper ended the run with status 125: `cannot open … File name too long (os error 36)` | a named refusal before dispatch, or a run |
| E5 | `security_a_helper_in_a_workspace_is_refused_however_the_workspace_root_is_spelled` | `workspace_root` is `<ws>/new/..`, `new` does not exist, and `exec_gate` names a file in a workspace beneath `<ws>` | the open accepted the helper | the refusal the open gives for the root spelled `<ws>` |

- **E1 to E3 are one cause.** The run's variables are set by bubblewrap, so every program
  between bubblewrap and the run's own starts with them (§ 6, D28). The driver's rule for a
  variable admits `LD_PRELOAD`, `LD_LIBRARY_PATH` and `LD_AUDIT`: an upper-case name of at most
  128 bytes that names no credential (`crates/substrate-guarded/src/environment.rs:19-41`).
- **E3 is why a statically linked helper is not the answer.** A static helper has no loader
  that reads the variable, which closes E2. In E3 the object runs in `/usr/bin/env`, which is
  the host's and dynamically linked. Since the seventh revision: it is not the whole answer.
  It is one half of D29; the other half is that no program runs before the helper (§ 3.11).
- **E4** is in the helper's own open of the path's text
  (`crates/substrate-exec-gate/src/landlock.rs:238`). The driver had opened the same entry
  beneath the descriptor of its mount, where the path is shorter.
- **E5** is in the place of a path that does not exist
  (`crates/substrate-exec-gate/src/place.rs:430-437`): the names after the nearest existing
  ancestor were joined as text, `..` among them. The driver places `workspace_root` before it
  creates it (`crates/substrate-host/src/lib.rs:1159-1160`). It then created `<ws>/new`, and
  from there on its root was `<ws>`.

### 3.11 What the second pass of both reviews measured

Not probes of this document either. A second adversary pass and a second security look ran
against `767fbea57`, the commit that holds D28, on 2026-11-21, on the probe host, in the
delegated lane. This revision read their cases and logs and ran none. The cases are in
`crates/substrate-host/tests/adversary_exec_lists.rs`,
`crates/substrate-host/tests/security2_exec_gate.rs` and
`crates/substrate-exec-gate/tests/security2_gate.rs`; they were not committed when this was
written.

| | case | what the run was | observed | expected |
|---|---|---|---|---|
| E6 | `adversary2_a_read_only_ld_so_preload_runs_before_the_ruleset` | the profile lists `bin` and declares a read-only file mounted at `/etc/ld.so.preload`. The file names `/workspace/evil.so`. The object, on its first load, executes an unlisted base tool: `echo LEAKED`. No variable is set. The lane's helper is a default build, with the loader `/lib64/ld-linux-x86-64.so.2` | `LEAKED`, exit 0: the run never reached its own script | `0`, `126`, `126`, or a named refusal before dispatch |
| E7 | `security2_a_declared_file_at_the_loaders_preload_path_does_not_lift_the_list` | the same shape. The object appends the `argv[0]` of every process that loads it to a file, and in the helper's process it executes what follows the last `--` | `listed:0`, `unlisted:0`. Loaded in `/.substrate-exec-gate`, `/usr/bin/sh`, `bin/tool` and `other/tool` | `listed:0`, `unlisted:126`, and the object loaded in no process named for the helper |
| E8 | E7 again, with a helper built from the same commit for `x86_64-unknown-linux-musl`. `file` reports it `static-pie linked` | the same profile and object | `listed:0`, `unlisted:126`: the list held. Loaded in `/usr/bin/sh` and `bin/tool`, and not in the helper | as observed |
| E9 | `security2_an_object_the_run_names_is_loaded_in_no_process_before_the_program` | D28's own case: the object is named by `LD_PRELOAD` in the run's environment | passed with the default helper and with the static one: the object is loaded in the run's program and what it starts, and in no process before | as observed |
| E10 | `security2_a_declared_file_at_the_helpers_mount_does_not_stand_in_for_the_helper` | the profile declares a read-only file at the helper's own mount, `/.substrate-exec-gate` | the helper's bytes stood over the declared file, and the list held | as observed, or a named refusal |
| E11 | `security2_a_value_of_a_listed_run_is_readable_where_an_unlisted_run_shows_it` | where the run can read one of its own values in its view | the arguments of bubblewrap and the environment of the shell, on a start with a list and on one without | the same places on both |
| E12 | `security2_an_unlisted_start_keeps_its_argv_and_a_listed_one_has_no_stage_before_the_helper`, `security2_a_listed_run_with_piped_input_is_held_with_a_preload_in_its_environment`, `security2_no_refusal_of_a_variable_repeats_its_value` | rule 7 of D28, a start with piped input, and the helper's diagnostic lines | each passed | as observed |

The same pass ran the suites of the first reviews on `767fbea57`: the cases of E1 to E5 pass
there (`adversary_exec_lists`, 8 cases; `security_exec_gate`, 7 cases), and
`ess_confinement_linux` runs 74 cases. D28 with its rule in place is therefore observed.

- **E6 and E7 are one cause, and it is not D28's.** `/etc/ld.so.preload` is a file a loader
  reads beside its variables. `validate_read_only_files` refuses a mount that collides with a
  mount the driver owns and admits every other path
  (`crates/substrate-wire/src/lib.rs:4782-4797`, `:4822`; the reserved mounts are `/`,
  `/usr`, `/bin`, `/lib`, `/lib64`, `/proc`, `/dev`, `/tmp` and `/workspace`, `:4723-4733`).
  The open admitted a helper that names a loader: it read the helper's bytes and sealed them
  without looking at them (`crates/substrate-host/src/exec_gate.rs:650`, `:683-686`).
- **E8 shows what decides it: the helper's linking.** The same commit, the same profile and
  the same object; only the helper differs. E8 also shows the limit of § 7 as an observation:
  inside the domain the object is loaded into the allowed shell and the listed tool, and the
  unlisted tool still ends with status 126.
- **E10** is D20's order: the helper's bytes are bound after every other mount.
- **A judgement of the reviews, relayed by the coordinator and not a run:** what bubblewrap
  takes from the caller before it starts the helper is a cleared environment, an argv the
  driver built, the driver's descriptors, and the directory of `--chdir`, which becomes `PWD`.
- **Read, not run:** `CONTROLLING_TERMINAL_ARGV`, which is `/usr/bin/setsid --ctty`, is
  placed after bubblewrap's `--` and before the helper
  (`crates/substrate-host/src/process.rs:3077-3079`, `crates/substrate-host/src/pty.rs:44`). A
  terminal start with a list would run the host's `setsid`, dynamically linked, before the
  helper. No such start exists today: a terminal belongs to a session start, and the three
  wire paths carry no list (`process.rs:931`, `:967`, `:1078`).

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
| symbolic link in a mount that shows something a run can write: the workspace, a writable root, and a read-only mount whose host directory lies in or holds a writable location (D24) | no part may be one; refused `exec.sandbox-path-unrepresentable` | relative entries: the same |
| symbolic link in any other mount | followed; the rule is on the file or directory it resolves to (probe N7). Refused `exec.sandbox-path-unrepresentable` when it resolves into a writable mount (D24) | absolute entries are canonicalized |
| does not exist at the start | refused `exec.sandbox-path-unrepresentable` | a relative entry need not exist; an absolute one must |
| its path in the view, `<root>/<rel>` of D24, is longer than 4095 bytes | refused `exec.sandbox-path-unrepresentable` before dispatch (sixth revision, E4) | unchanged |
| the program | an absolute view path when the list is not empty; else `exec.sandbox-path-unrepresentable`, address `argv`. The length rule of the row above holds for it too | absolute, always |

Reasons:

- **One lexical grammar.** A profile that `macos` can carry is one `linux` accepts, and the
  reverse.
- **No symbolic link beneath a writable location.** An earlier run could leave `bin` as a link to
  `/usr/bin`; the entry `bin` would then admit all of `/usr/bin`. The driver checks through the
  pinned workspace descriptor before dispatch, as it does for a writable subtree (design 29 § 2),
  and the helper opens the entry again with `openat2`, `RESOLVE_BENEATH` and
  `RESOLVE_NO_SYMLINKS`, so a link swapped in between is refused by the kernel: `ELOOP` in the
  probe helper (K10). Which mounts are opened this way is D24's.
- **No object named by the caller's environment before the ruleset.** The planted link has a
  twin. An earlier run, or a checkout, could leave a shared object in the workspace, and the
  caller's environment could name it: `LD_PRELOAD=/workspace/pre.so`. The caller need not be
  hostile for that. A tool chain sets such variables for its own reasons, and the driver's rule
  for a variable does not refuse them (§ 3.10). The object's constructor then runs in every
  dynamically linked program that starts with the variable. Before the sixth revision that
  included the programs that run before the ruleset exists, and what the constructor executed
  there was held to no list (E1 to E3). D28 keeps the run's variables from every such program.
  Inside the domain the object still loads; § 7 states that as a limit.
- **No object named by a file the profile declares, either.** The seventh revision's twin of
  the twin. A loader reads files beside its variables, `/etc/ld.so.preload` first among them,
  and a profile may declare a read-only file at that path. The file is the caller's, as the
  variable was, and it too can name an object an earlier run left in the workspace. With a
  dynamically linked helper the object ran in the helper's own process, before the ruleset
  (E6, E7). D29 gives the helper no loader and puts no program before it. Inside the domain
  the file is read by every allowed dynamic program; that is the same limit of § 7.
- **A view path the kernel takes.** A path given to a system call is at most 4095 bytes
  (`PATH_MAX` with its terminator). The helper opens the text of `<root>/<rel>` for a `follow`
  path and executes the program by that text. The driver checks an entry through the
  descriptor of its mount, where the path is shorter, so a mount whose view path is longer than
  its host path gave an entry the driver could open and the helper could not (E4). The driver
  now measures what the helper will be given: the text of `<root>/<rel>` for the program and
  for every entry of either list, and the path of the loader and of every `--writable` root.
  One above 4095 bytes refuses the start, with the field the path came from as the address
  (`argv`, `exec_subpaths`, `exec_denied_paths`, or the field of the declared mount) and its
  audit record. A bound below the kernel's is not set: a path the kernel takes is a path the
  helper opens. The check is the start's, not
  admission's: the mount of a declared root is known only there.
- **An entry must exist on `linux`.** A Landlock rule is tied to an open file, not to a name. A
  fresh clone has no `target` and no `node_modules`, so a profile that lists one is refused at
  the start (T14 shows the helper's side). The client creates the directory in the workspace
  before the start; § 6, *Build housekeeping*, says what happens to it afterwards.
- **`/tmp` is not eligible.** It is a fresh tmpfs per run and writable. `macos` does not admit its
  private `TMPDIR` either.
- **A rule is on a directory, not on a name.** Where the view mounts one host directory twice, a
  rule on either mount admits both: listing `/bin` admits `/usr/bin` on a host whose `/bin` is a
  link into `/usr` (B5). D23 covers what that means for a deny entry.
- **A rule reaches a file only along the mounts of the path the run uses.** The kernel walks from
  the file up to the root of its mount and continues at the mount point. Listing `/usr` therefore
  does not admit `/bin/sh` where the view's `/bin` is a mount of `/usr/bin`: that walk never
  passes `/usr` (K7). Listing `/usr/bin` admits the program under both names. A client lists the
  directory that holds the programs, not a directory above it, when its tools name programs
  through `/bin`, `/lib` or `/lib64`. This is a limit of the allow-list as it stands; the
  deny-list does not depend on it (D25).

### Reference text

The difference of D1 and the rules of this section are stated where a client reads the field:

- the doc comment of `ConfinementProfile.exec_subpaths` and its row in the field table
  (`profile.rs:17-36`, `:71-72`): an empty list runs every program of the view on `linux` and only
  the program on `macos`; on `linux` an entry is a view path and must exist;
- the module documentation of `linux.rs` (`:17-31`) and `macos.rs` (`:28-35`);
- the nine causes and the three file-system types of D26, in the doc comment where
  `exec.sandbox-place-unserved` and `config.exec-gate-unserved` are raised, and in the doc
  comment of `FactWithheld` and of `AdapterFacts.withheld` (D27);
- the doc comment of `HostConfig.exec_gate` (`crates/substrate-host/src/lib.rs:384-406` at
  `4b5208922`), which ends "Nothing runs with a list that is not enforced." Since the sixth
  revision it says what that rests on: on a start with a list the run's variables reach the
  program and nothing before it (D28), and inside the domain an object those variables name
  still loads into every allowed dynamic program (§ 7). Since the seventh revision it also
  says that the helper is a statically linked program, that one that names a loader is
  refused `config.exec-gate-invalid`, and that a start with a list has no controlling
  terminal (D29);
- the doc comment of `ConfinementProfile.exec_subpaths` and the module documentation of
  `linux.rs` name that limit beside the four of § 7;
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

**That sentence holds only with D28.** It is about what happens after the helper applied the
ruleset. Until the sixth revision nothing in this document said what the run can do to the
programs that execute before that point, and the run could reach them: its variables were set
for each of them, and an object a variable named ran there and started the run's program with
no ruleset (E1 to E3). The run did not remove the domain; it kept the domain from being
created. D28 is the rule that the run supplies nothing to a process before the ruleset exists.

**Since the seventh revision: with D28 and D29.** D28 keeps the run's variables from those
programs. A profile reached them a second way, with a file: a declared read-only file at the
path a loader reads (E6, E7). D29 removes what that route needs, a loader, and with it every
program before the helper. § 6, *Before the ruleset*, states what the two decisions give
together and what that still rests on.

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
  `execve` after it is the program's. Since the sixth revision that sentence carries a
  condition it did not state: those programs start with none of the run's variables, and the
  `env` stage is not needed on such a start (D28). The line numbers here are the first
  revision's; D28 cites the argv as implemented. Since the seventh revision the helper is the
  first program after bubblewrap's `--` on such a start, with no `env` and no `setsid` before
  it (D29): `… -- <gate> <option>… -- <program> <argument>…`.
- **Statically linked (D29).** "No dependency beyond `libc`" is about the crates it is built
  from. Since the seventh revision the built file has a rule too: it names no loader.
- **Only with a list.** A start whose profile has an empty `exec_subpaths` gets no helper and no
  bind. Its argv is today's, byte for byte.

**D20. The driver reads the helper once, at open, through one descriptor, and binds those bytes
into every view.** The first draft bound the file by descriptor and compared it by name before
each start, as `backend_unchanged` does for bubblewrap (`probe.rs:510-528`). That leaves a gap
between the comparison and the bind. This closes it:

1. At open the driver opens the configured path once. Every later fact comes from that
   descriptor: `fstat` (a regular file, at most `MAX_BACKEND_BINARY_BYTES`, `probe.rs:22`), the
   path the descriptor has (`/proc/self/fd/<n>`, for D21), and the bytes.
2. It copies the bytes into a memfd, seals it with `F_SEAL_WRITE`, `F_SEAL_GROW` and
   `F_SEAL_SHRINK`, adds `F_SEAL_SEAL` as hygiene so that the set of seals cannot change, and
   takes the SHA-256 of the sealed bytes. The driver already seals memfds for secret slots and
   proves that one crosses bubblewrap (`probe.rs:839-845`).
3. `BackendBinding` (`probe.rs:25-34`) gains that SHA-256, so the snapshot digest binds the bytes.
   The file's device, inode and name are not bound: they are not read again.
4. The probes of § 9 and every start hand bubblewrap a fresh read-only descriptor of the sealed
   memfd with `--perms 0555 --ro-bind-data <fd> <gate>`. Bubblewrap copies the bytes into a
   temporary file outside the view's root, binds it read-only at `<gate>` and unlinks the
   temporary name (`bubblewrap.c:1531-1572` of 0.11.2; B10 on 0.13.0). `<gate>` is one fixed path
   outside every location § 5 makes eligible.
5. Both options exist in 0.11.2 (`--ro-bind-data` at `bubblewrap.c:2274`, `--perms` at `:2673`),
   and a bind adds `nosuid`, `nodev` and read-only and never `noexec` (`bind-mount.c:450`), so
   the copy executes. Read in the tarball the `Dockerfile` pins; the stakeholder's review read
   the same lines.
6. Each start gets a descriptor of its own, opened anew on the sealed memfd. Bubblewrap reads
   the descriptor to its end, so a second start on the same open file copies nothing (K3).
7. Seventh revision. Between step 1 and step 2 the driver looks at the bytes it read: they are
   an ELF program with no `PT_INTERP`, or the open is refused (D29, rule 1). The bytes that are
   tested are the bytes that are sealed and bound: one read through the one descriptor, and no
   second look at the file. A file swapped after the read changes nothing, as before.

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
- **The compiler stops the adapter from calling `ExecLists::wire()`.** The type is defined in
  `crates/substrate-host/src/process.rs`, and `wire` is private to that module: it has no `pub`.
  The three wire sites are methods of `ProcessRuntime` in that file (`start` at `:810`,
  `start_with_git_roots` at `:835`, `start_pipe_with_git_roots` at `:865`, whose call is `:958`).
  The adapter's path is outside it: `crate::confinement::linux` (`linux.rs:467`) calls
  `HostDriver::start_exec_for_adapter` in the crate root (`lib.rs:1234`). Neither module can
  name a private function of `crate::process`, so a call from either does not compile. `of` is
  `pub(crate)`. `pub(super)` would be wrong: it opens `wire` to the crate root, where the
  adapter's entry point is. No code moves.
- One caller the compiler does not stop: `start_for_adapter` (`process.rs:823`) is in the same
  module. It receives the lists as an argument and constructs none. A source test counts the
  uses of `wire` in the file (§ 16), so a fourth is seen. Moving the three wire methods into a
  child module of their own would let the compiler hold that too; it is not needed for the
  adapter and is not done here.
- A unit test holds that `ExecLists::of` is not empty for any admitted profile with a list, and
  the lane cases of § 16 fail when a denied program runs.

`ExecStartInput` is a wire type and does not change.

### The place of a path

**D25. Whether one path lies in another is decided by their places, not by their text.** Three
checks of this document ask it: a denied path inside a listed directory (D23), an entry inside a
location a run can write (D24), and the helper's own file inside one (D21). The text of two paths
does not answer it:

| what the host or the view does | what the text says | what is true |
|---|---|---|
| mounts one directory at two paths: the view's `/bin` and `/usr/bin` (B4), or a directory that has two host paths (K5) | neither path is beneath the other | they are one directory |
| mounts a part of a directory at a second path (K2) | the second path is outside the directory | it is inside |

- **The path's mount** is the one the kernel names. The path is opened, and `statx` with
  `STATX_MNT_ID` (Linux 5.8; D8 already needs 5.19) returns the mount ID of the descriptor. The
  mount is the one row of `/proc/self/mountinfo` whose first field is that ID.
- **The place** is that row's device field, and its root field joined with the rest of the
  descriptor's path (`/proc/self/fd/<n>`) below the row's mount point.
- **`X` lies in `Y`** when both places have the same device and the place of `X` is the place of
  `Y` or lies beneath it.
- A path that does not exist has the place of its nearest existing ancestor, joined with the
  remaining names.
- **A remaining name of `..` or `.` is never joined.** The kernel resolves those two names; it
  has not resolved them in the part of a path that does not exist, and text joined in their
  place names another directory than the path will once it exists. Where one of the remaining
  names is `..` or `.`, there is no place and the cause is `path-unopenable` (D26). Sixth
  revision, from E5: with `workspace_root` written `<ws>/new/..` and `new` absent, the joined
  text was `<ws>/new/..`, a helper in a workspace beneath `<ws>` did not lie in it as text, and
  the driver then created the root, which is `<ws>`.
  - The rule is about the names after the nearest existing ancestor only. A path that exists
    is opened, and the descriptor's path has neither name in it.
  - It refuses and does not resolve. Removing `new/..` as text would be right here and wrong
    where `new` is later created as a symbolic link.
  - An entry of a list cannot hold either name: the lexical rule of § 5 refuses it at
    admission. The paths this reaches are the embedder's: `workspace_root` and the
    `writable_roots` paths.
- The helper reads the table of the view once, before its first rule. Bubblewrap mounts `/proc`
  in every view (`process.rs:2750`). The driver reads its own table, for host paths.
- Where any step fails, there is no place, and D26 says what happens. Nothing compares the text
  of two paths in its stead.

**Not the longest matching mount point.** The third revision chose the row whose mount point is
the longest prefix of the path, the last such row. That is a guess at what the kernel knows.
Two tables show it:

- two mounts stacked on one mount point give two rows with that mount point, and only their
  order in the table tells them apart (S1);
- a mount at `hold/sub` that a later mount over `hold` hides still has its row, and it is the
  longest match for `hold/sub/z`, which is a file of the later mount. There the guess is wrong
  (S2).

With the mount ID no two rows can both be the path's mount: the kernel reports one ID, and the
reader refuses a table in which that ID has no row or more than one (D26).

On the probe host the view's table has `/usr` with the root `/usr` and `/bin` with the root
`/usr/bin`, on one device. `/bin/id` therefore has the place `/usr/bin/id`, which lies in the
place of `/usr/bin` and in the place of `/usr`.

**Not the device and inode of the view's ancestors.** The stakeholder's confirming read proposed
that rule: a denied path lies beneath `A` when `A` has the device and inode of one of the deny
entry's ancestors in the view. It needs no table. It holds where one directory has two names
(K1). It does not hold where the second mount shows a part of the listed directory:

- `/usr` listed and `/bin/id` denied. The ancestors of `/bin/id` in the view are `/bin` and `/`.
  `/bin` is the directory `/usr/bin`, and `/usr` is not among them. `/usr` gets one rule, and
  `/usr/bin/id` runs (K2).
- The same with a subdirectory of a declared root under a second host path (K2).

A walk up from a path leaves a mount at its root and continues at the mount point. It never
passes the directories above that root in their own file system. The place does.

Device and inode stay in use for one thing: another name of a denied file inside an expanded
directory (D11 step 1, N2). The place does not find a hard link elsewhere; § 8.5 states that
limit.

**D26. Where a place cannot be established, the answer is a named refusal. Neither the driver
nor the helper falls back to the text of a path.** This is the stakeholder's first condition on
D25 (2026-11-21). At the open, two of the causes below withhold the two facts where the fourth
revision refused the open (D27); the profile with a list is then the thing refused, by name and
with the cause. One function establishes a place, from three inputs: the mount ID the kernel
reported, the descriptor's path, and the text of the mount table. The helper and the driver
compile the same source file for it, so the two cannot disagree, and the helper keeps no
dependency beyond `libc` (D5). Getting the inputs and running the function ends in a place or in
one of these causes, and in nothing else:

| cause | when |
|---|---|
| `mount-table-unreadable` | `/proc/self/mountinfo` cannot be opened or read |
| `mount-table-unparsable` | the table was read and is not UTF-8 text; or a row lacks its ID, device, root or mount point, has no ` - ` separator or no type after it, or holds an escape that is not three octal digits |
| `path-unopenable` | the path cannot be opened for a reason other than not existing, or no ancestor of it exists; or the path does not exist and one of the names after its nearest existing ancestor is `..` or `.` (D25) |
| `mount-id-unreported` | `statx` fails, or its result does not have `STATX_MNT_ID` set |
| `mount-row-missing` | no row has the reported ID |
| `mount-row-repeated` | more than one row has it |
| `mount-root-deleted` | the row's root ends in `//deleted`: the directory the mount shows was removed (S3) |
| `mount-point-mismatch` | the descriptor's path cannot be read, or the row's mount point is not that path or a prefix of it |
| `file-system-unlisted` | the row's file-system type is not in the closed list below |

The causes are checked in this order, and the first that holds is the one named.

**A mount point that is not UTF-8 is `mount-table-unparsable`, not `mount-table-unreadable`.**
The kernel writes a mount point's bytes into the table as they are and escapes only the space,
the tab, the newline and the backslash (`show_mountinfo` in `fs/proc_namespace.c`; not read
again or run for this revision, § 18), so one mount with such a name makes the whole file
something other than text. The fifth revision's two definitions were:
`mount-table-unreadable`, the table "cannot be opened or read", and `mount-table-unparsable`, a
row that lacks a field or holds a bad escape. The first is a failure of the read. A table whose
bytes arrived was read, so it is not that cause; and the second did not name the case. The
sixth revision names it in the row above. The coordinator asked for the answer after the two
reviews, because the implementation at `4b5208922` reads the table into a string in one call
(`crates/substrate-exec-gate/src/place.rs:261`), and maps every error of that call to
`mount-table-unreadable`, the error for bytes that are not UTF-8 among them. That is read from
the code; no such table was staged. Both causes refuse (D27), so the answer to the caller does
not change, only the cause it names. That one mount with such a name refuses every open and
every start with a list on that host is a consequence; § 20 has it.

**How "two rows could both be its mount" is handled.** It is not detected; it cannot arise. The
row is chosen by the ID the kernel reported for the opened path, never by matching mount points.
Mounts stacked on one mount point are two rows with two IDs, and the kernel reports the upper
one (S1). A mount hidden by a later mount keeps its row and is never reported for a path that
can be opened (S2). `mount-row-missing` and `mount-row-repeated` refuse the two tables in which
the ID does not name exactly one row.

**The closed list of file-system types** is `ext4`, `overlay` and `tmpfs`. These are the types
the rule was observed to hold on, with the same lists on each (§ 3.9). The list is a constant
beside the place function. Adding a type is a change to this document, backed by the lists of
§ 3.9 run on that type.

- **What is compared** is the type field of the path's row: the first field after the ` - `
  separator, as exact text. For a bind mount it is the type of the file system the mount shows.
- **A type outside the list** is `file-system-unlisted`, whatever else is true of the path. The
  refusal names the path, the type and the three listed types.
- **`overlay`** was probed twice: mounted in a user namespace, and as the root of a container
  that Docker mounted (F1 to F9). In both, every layer was on one ext4 file system.
- **`btrfs`** is outside the list. Its subvolumes are the case where `st_dev` of a file and the
  device of its row differ. Not probed.
- **`xfs`** is outside the list. That it behaves as ext4 does here is inferred, and nothing was
  run on it: no xfs is mounted on the probe host, and mounting one needs privilege the probes
  did not have.
- **Network and user-space file systems** are outside the list: `nfs`, `nfs4`, `cifs`, `smb3`,
  `ceph`, `9p`, `virtiofs`, `fuse` and every `fuse.<name>`. Not probed.
- **Kernel file systems** (`proc`, `sysfs`, `devtmpfs`, `devpts`, `cgroup2`, `mqueue`) are
  outside it. No entry may lie on them (§ 5), and U1 and U2 show the refusal.

**The view's own tmpfs mounts.** The view's root, `/dev` and `/tmp` are tmpfs that bubblewrap
creates (U2). No entry may lie on them: an absolute entry lies beneath `/usr`, `/bin`, `/lib`,
`/lib64`, `/workspace` or a declared mount, and `/tmp`, `/proc`, `/dev` and `/etc` are refused
`exec.sandbox-exec-subpath-undeclared` (§ 5). `tmpfs` is in the list for two other reasons.
A host directory can be on tmpfs: the repository's tests create their workspace root with
`tempfile::tempdir()` (`crates/substrate-host/tests/ess_confinement_linux.rs:98`), and `/tmp` is
tmpfs on the probe host. And the helper establishes the place of every `--writable` root, `/tmp`
and `/dev` among them, for the check of D24.

**Who refuses, and with what.**

| where | what cannot be placed | answer |
|---|---|---|
| the open of `linux`, and of `systemd` around it, with `exec_gate` set: the cause is `file-system-unlisted` or `mount-id-unreported` | the helper's file, `workspace_root`, a `writable_roots` path, a base directory (`/usr`, `/bin`, `/lib`, `/lib64`), or the temporary directory the probes of § 9 build their workspace in (`tempfile::tempdir()`, as `probe.rs:324` does today) | **the adapter opens and proves neither fact (D27).** The cause, the path and the file-system type are in the open's result and in the adapter's proof. A profile with a list is refused `confinement.fact-unserved`, 501, at admission, and the refusal repeats them |
| the same open: one of the other seven causes | the same paths | the open is refused `config.exec-gate-unserved`, 501: the existing code of D19, with the cause and the path in its message. Address: `config.exec_gate`, `config.workspace_root` or `config.writable_roots`, by whose path it was; `config.exec_gate` for a base directory and for the temporary directory |
| a start whose profile has a list, before dispatch | the workspace, the host source of a declared mount (for its class, D24), the program, or an entry of either list | `exec.sandbox-place-unserved`, 501: **new**. A `DriverError` of class `Unserved`, the form of `exec.sandbox-unavailable` (`read_only_roots.rs:85-90`). Address: `workspace`, `argv`, `exec_subpaths`, `exec_denied_paths`, `read_only_roots`, `read_only_files` or `writable_roots`. Its audit record is written as for every refused start. It arises only on an adapter that proved the facts (D27) |
| the helper, inside the sandbox | any path of its arguments, or a `--writable` root | exit 125 and one line, `substrate-exec-gate: cannot establish the place of <path>: <cause>`, with the type after `file-system-unlisted`. Nothing is executed |
| admission | nothing | admission reads no path. An entry is resolved at the start (§ 12), so the refusal is the start's |

- **One new code, and one existing code with new causes.** At a start no existing code fits.
  `exec.sandbox-path-unrepresentable` and `exec.sandbox-exec-subpath-undeclared` are 422 and
  say the client wrote a path this policy cannot carry; here the path is well formed and the
  host cannot serve it, which is 501 (invariant 3). `exec.sandbox-unavailable` is 501, and it
  is what a host without the sandbox answers; a client that branches on the code could not
  tell "no sandbox" from "move this root to another file system". At the open the existing
  code already means what is needed: the helper is configured and this host cannot use it.
- **A file-system type outside the list does not refuse the open.** The fourth revision
  refused it, so that the cause could not hide behind an absent fact. The stakeholder decided
  otherwise on 2026-11-21: the adapter opens without the two facts, and the cause is made
  visible (D27). The other seven causes still refuse the open.
- **The probes' temporary directory is placed before a probe runs.** Otherwise a temporary
  directory on a type outside the list would make the helper exit 125 inside the probe, the
  fact would be absent, and the cause would read as "this kernel has no Landlock". With the
  place established first, the absence carries its own cause (D27).
- **The reference text of both codes** names the nine causes and the three types: the doc
  comment where each code is raised, and the confinement pages under `website/docs` (§ 5,
  *Reference text*). The doc comment of `FactWithheld` names the two causes that withhold.
- **The helper is the second line again.** The driver has placed every path before dispatch, so
  an exit 125 with this line means the view's table differs from what the driver expected.
- `exec.sandbox-place-unserved` cannot occur on a wire start: a wire start has no list (D22),
  and nothing is placed for a start without one. Whether the wire story publishes the code is
  that story's.

**D27. With a helper configured and a path on a file-system type outside the list, the adapter
opens, the two facts stay absent, and the absence says why.** Decided by the stakeholder
(agent:sdc) on 2026-11-21, with its acceptance of the fourth revision: "B now, C before the wire
story". Its words for B: "With a helper configured and a workspace volume on a file system
outside the closed list, the open succeeds, the two facts stay absent, and a profile that
carries a list is refused `confinement.fact-unserved`; nothing ever runs with a list that is not
enforced." It set two conditions: the absence is visible and not silent, and C.

**Which causes withhold the facts, and which still refuse the open.** The stakeholder's sentence
names the workspace volume and one cause. The split below goes further in two places, and each
is a reading, not the stakeholder's words:

| cause at the open, on any path the open places | answer | whose reading, and why |
|---|---|---|
| `file-system-unlisted` | both facts are withheld; the adapter opens | the coordinator's reading: every path the open places, not the workspace volume alone. A host with an `xfs` root has the helper's own file and `/usr` on `xfs`, so a rule for the volume alone would still refuse that open |
| `mount-id-unreported` | both facts are withheld; the adapter opens | this revision's. `STATX_MNT_ID` exists since Linux 5.8 (`statx(2)`), Landlock since 5.13 (`landlock(7)`, *VERSIONS*). A kernel that reports no mount ID has no Landlock, and for that host D19 already says the adapter opens and the facts are absent. Refusing the open there would make a configured helper on an old kernel an error, against that row |
| `mount-table-unreadable`, `mount-table-unparsable`, `path-unopenable`, `mount-row-missing`, `mount-row-repeated`, `mount-root-deleted`, `mount-point-mismatch` | the open is refused `config.exec-gate-unserved` (D26) | unchanged. Each says the driver could not read the host's table or a configured path. None says what the host offers, and the stakeholder's decision does not name them |

- **The split holds on its own terms.** The causes are checked in D26's order and the first
  that holds decides, so `file-system-unlisted`, the last, is reached only by a path that has a
  mount ID, one row, a live root and a matching mount point. U2 and U3 show that for two types
  outside the list: with every type admitted, `/proc/version` and `/dev/null` had a mount ID
  and a row. That a mount ID is reported on `xfs`, `btrfs` and the network file systems is
  inferred: `statx(2)` marks other fields "support varies by filesystem" and not this one.
- **Every path is placed before the answer is given.** The order is the helper's file,
  `workspace_root`, each `writable_roots` path as declared, `/usr`, `/bin`, `/lib`, `/lib64`,
  and the probes' temporary directory. One refusing cause on any of them refuses the open. With
  none, every path that has a withholding cause is recorded, not only the first.
- **One writable root on a type outside the list withholds the lists for every start**, also
  for starts that do not mount it. D24 classes every mount against every writable location,
  and a location without a place cannot be compared.

**What the open does, in order.** This replaces the order § 6, *Embedder notes*, gave:

1. `exec_gate` is not absolute: `config.exec-gate-invalid` (D21).
2. The file is opened once (D20). Missing, not a regular file, unreadable or over the size
   bound: `config.exec-gate-unserved` (D19).
3. The places, as above. A refusing cause: `config.exec-gate-unserved`. A withholding cause:
   both facts are withheld and the open goes on **without steps 4 to 6**. The helper is not
   sealed, not bound and never executed, and no probe of § 9 runs.
4. The file lies in a writable location, or has more than one link:
   `config.exec-gate-invalid` (D21).
5. The bytes are read once. Seventh revision: bytes that are not an ELF program without
   `PT_INTERP` are `config.exec-gate-invalid` (D29). The bytes are then sealed and hashed, and
   `--version` is read in a throwaway sandbox (D20): a wrong line is
   `config.exec-gate-unserved`.
6. The two probes of § 9. A probe that does not hold leaves its fact absent (D19).

A helper with two links on a host whose facts are withheld is therefore not refused there. It
is refused on the first host where the facts could be proved. The same holds for a helper
that names a loader: step 5 is not reached there, and the helper is never executed there.

**Nothing runs with a list that is not enforced.** A non-empty list implies its fact
(`profile.rs:213-229`), and admission refuses the first required fact the proof lacks
(`mod.rs:631-637`). A deny-only profile is D13's refusal. A profile without a list is admitted
and runs as today, with no helper in its argv (D5).

Since the sixth revision: this paragraph argues from admission, and admission was never where
the sentence failed. A profile with a list was admitted on an adapter that proved the fact, was
started, and ran programs outside its list (E1 to E3). The stakeholder's sentence is true of a
start only with D28. Since the seventh revision: only with D28 and D29 (E6, E7).

**Where the cause is carried.** The stakeholder's first condition: "the open's result and the
adapter's capability report carry the named cause (file-system type and path), and the refusal
of a profile with a list repeats that cause, so the host's operator learns why lists are not
served without reading a log at debug level". Each carrier, with the item that exists today:

| carrier | what exists | what is added |
|---|---|---|
| the driver's open | `HostDriver::open` returns the driver (`crates/substrate-host/src/lib.rs:1111`). D14's `ExecListProof` is the driver's own record of what its probes proved | `ExecListProof` gains `withheld`, the entries below. The driver writes one `tracing::warn!` line per entry, as the floor's probe does when it withholds the floor (`probe.rs:614-617`) |
| the open's result, in the confinement API | `open` returns the adapter (`mod.rs:159-170`), and `ConfinementAdapter::facts` returns its proof (`mod.rs:725`) | the proof carries the entries. A caller reads them from the value `open` returned, before it admits anything |
| the adapter's capability report | the proof is `AdapterFacts`, with public fields and `Serialize` (`facts.rs:71-81`), built by `linux::proof` (`linux.rs:164-214`) and, for `systemd` around `linux`, from the inner proof (`systemd.rs:576-599`) | `AdapterFacts` gains `pub withheld: Vec<FactWithheld>`, serialized only when it is not empty. `linux::proof` fills it from `ExecListProof`; `systemd`'s proof copies the inner adapter's |
| the refusal of a profile with a list | `decide` returns `ConfinementError::FactUnserved { adapter, fact }` (`mod.rs:631-637`), whose text is "did not prove" and the fact (`mod.rs:225-226`) | the variant gains `withheld: Option<FactWithheld>`. `decide` fills it with the proof's first entry for that fact, and the text ends with the cause, the path, its address and the type |
| the audit record | `ProfileRefused` carries the code and the fact (`mod.rs:292-304`, built at `:327-345`) | nothing |
| the daemon's capability snapshot | `publish_confinement` writes `Some(true)` for a proved fact and nothing for another (`probe.rs:269-317`) | nothing in these stories; see below |

```rust
/// Why an opened adapter does not prove a fact its probe could have proved.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FactWithheld {
    pub fact: Fact,
    /// `file-system-unlisted` or `mount-id-unreported`.
    pub cause: &'static str,
    /// `config.exec_gate`, `config.workspace_root` or `config.writable_roots`.
    pub address: &'static str,
    pub path: String,
    /// The type field of the path's mount row; absent for `mount-id-unreported`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_system: Option<String>,
}
```

The refusal then reads, for a workspace volume on `xfs`:

```text
confinement.fact-unserved: Linux did not prove ExecSubpaths: file-system-unlisted:
/var/lib/b10x-substrate/workspaces (config.workspace_root) is on xfs; the lists are served
on ext4, overlay and tmpfs
```

It is one line; it is broken here for the page. Its code and status do not change
(`mod.rs:242-275`).

- **The public items this adds** are one struct with five fields, one field of `AdapterFacts`,
  one field of the `FactUnserved` variant and one field of D14's `ExecListProof`.
  `AdapterFacts::new` keeps its signature (`facts.rs:86`); the struct has a private field
  (`facts.rs:78-80`), so no caller outside the crate builds one by naming its fields, and a new
  public field breaks none. `crates/substrate-host/tests/confinement-public-items.txt` records
  each line, and § 16 regenerates it in both stories already.
- **The field on `FactUnserved` changes an existing item.** Four places in the host crate's
  source build the variant (`mod.rs:191`, `:633`, `pod.rs:989`, `:997`), and the CLI builds it
  once (`crates/b10x-substrate-cli/src/confine.rs:284`). Each gains `withheld: None`, except
  `decide` at `:633`. A pattern that names both fields gains `..`: the scenario runner
  (`crates/substrate-host/tests/support/mod.rs:149`) and two tests in `pod.rs` (`:1906`,
  `:1942`). A pattern that already ends in `..` is untouched (`mod.rs:246`, `:262`, `:334`,
  `confine.rs:625`, `tests/ess_confinement_local.rs:477`). The snapshot test's own cases quote
  the variant's text (`tests/confinement_public_items.rs:1438-1448`) and change with it. An
  embedder that builds the variant, or names both of its fields in a pattern, has to change
  one line.
- **Not a second variant.** A new variant with the code `confinement.fact-unserved` would
  change no existing item. It would also give one code two variants, where today each variant
  has its own (`mod.rs:242-253`), and the scenario runner maps the variant to the
  specification's error by name (`support/mod.rs:149-151`). A consumer that matches
  `FactUnserved` would miss a withheld fact and not be told.
- **The audit record stays as it is.** `ProfileRefused` is a serialized record with a fixed
  shape, and the stakeholder's three carriers do not include it. It still names the fact. An
  embedder that wants the cause in its audit trail reads the proof once at open.
- **The specification does not change for this.** Its error `FactUnserved` has the fields
  `adapter` and `fact` (`spec/confinement/confinement.yaml:161-165`), and the model has no file
  system (§ 14). The cause is the host's word beside them, as an error's text is today.
- **Visible by default.** The proof and the refusal are values the embedder already handles.
  The `warn` line is the third way: the daemon's default filter is `info`
  (`crates/substrate-daemon/src/main.rs:602-605`), and the driver writes no line below `warn`
  for this.

- **`withheld` is empty in D19's other rows.** With `exec_gate` unset, and where a probe did not
  hold, the facts are absent with no entry. The stakeholder's decision is about a place, and
  those two rows are as they were.

**The daemon and the wire.** The daemon's capability snapshot cannot carry the cause without a
bundle change: a confinement fact on the wire is `const true` or absent (`AGENTS.md`, invariant
6, on bundle `0.26.0`), and `publish_confinement` writes exactly that. So in these two stories
nothing of the cause reaches the wire. The daemon also does not set `exec_gate` in them: no
wire start carries a list (§ 15). When the daemon does set it, the driver's `warn` line is what
its operator sees at startup, once per withheld path. A published reason on the wire, and the
daemon's own startup line if it wants one of its own, belong to
`story:exec-start-carries-executable-lists`.

**The start is unchanged.** With the facts absent no profile with a list is admitted, so none
reaches a start. `exec.sandbox-place-unserved` can therefore arise only on an adapter that
proved the facts at open, when a path of the start itself has no place: a workspace or a
declared root on another file system, or an entry that resolves onto one. Its causes, its
addresses and its status are D26's.

**C, a recorded obligation.** The stakeholder's second condition: "probe xfs in the quota
fixture's container before the wire story is planned, and add it to the closed list only if
every probed case of the rule holds there; btrfs stays refused by name."

- **What is owed:** the lists F1 to F7 of § 3.9 and the table of row device, root and `st_dev`,
  run on a mounted `xfs`, before `story:exec-start-carries-executable-lists` is planned.
- **Where:** the container `cargo xtask quota-fixture run` builds from
  `fixtures/quota-lab/Dockerfile`, which already has `SYS_ADMIN` and loop access for a file
  system image (`AGENTS.md`, § *The real project-quota lane*). The recipe installs `e2fsprogs`
  and `util-linux` and checks for `mkfs.ext4` (`fixtures/quota-lab/Dockerfile:3-6`). It has no
  `mkfs.xfs` and no bubblewrap, so the probe needs both added to a recipe. That is read from
  the file; nothing was built or run.
- **The rule for the list:** `xfs` enters D26's list only if every case gives the right answer.
  One wrong answer keeps it out and is recorded with the case.
- **`btrfs`** stays refused by name. Nothing here obliges a probe of it.
- This revision ran no probe.

**What this means for the daemon image and the lanes.** Read from the tree, with what a file
cannot say marked as inferred. In these two stories no wire start carries a list (§ 15), so
what follows reaches the daemon when the wire story lands and it sets `exec_gate`. It reaches an
in-process embedder that runs in a container now.

- **The daemon image.** Its base is `ubuntu:24.04` (`Dockerfile:60`), and it declares
  `/var/lib/b10x-substrate` a volume (`Dockerfile:82`). The chart starts the daemon with
  `--workspaces /var/lib/b10x-substrate/workspaces` and mounts the claim `ledger` there
  (`deploy/helm/substrate/templates/statefulset.yaml:159-160`, `:286-287`, `:324-326`), with the
  cluster's default storage class unless `persistence.storageClassName` is set
  (`deploy/helm/substrate/values.yaml:16-17`). So the base directories of every view lie on the
  container's root file system, and the workspaces lie on the volume.
- **The container's root.** No file of the repository names its type: the container runtime's
  storage driver decides it, and neither the `Dockerfile` nor a workflow file states one. On the
  probe host Docker reports `overlay2`, the root row has the type `overlay`, and the rule holds
  on it (F8, F9). That the hosted runners' Docker and the kind nodes' containerd use an overlay
  too is inferred. A runtime with another driver gives another type; the adapter then opens
  without the two facts and names that type (D27).
- **The volume.** Its type is the storage class's, or the Docker host's for an anonymous
  volume. On `ext4` the lists are served. **On `xfs` or `btrfs` the adapter opens, serves no
  list, and says why**: the proof names the path and the type, and every profile with a list
  is refused `confinement.fact-unserved` with them (D27). `xfs` is probed before the wire story
  is planned (C); `btrfs` stays outside the list.
- **The daemon's `/tmp`** is an `emptyDir` without a medium in the chart
  (`statefulset.yaml:294-295`, `:321-323`): a directory of the node's own file system, whose
  type no file states. The probes' temporary directory lies there, so the same holds for it.
- **The hosted delegated lane** runs on the runner VM itself, not in a container
  (`.github/workflows/release-lanes.yml:65`, `:118-122`). The job prints `df -h / /mnt`
  (`:111`), which shows sizes and not types, so the workflow does not say what `/` or the
  tests' temporary directory is on. That both are ext4 there is inferred. If one is not listed,
  the adapter opens without the facts, the open step of every `linux` scenario that expects
  them fails, and the failure prints the proof with the type. The lane does not pass silently.

**The chart's reader is told.** The stakeholder asked for one sentence. It belongs in
`website/docs/guides/deployment.md`, § *Before you install*, after the table whose row
`persistence.storageClassName` asks for "a class that binds `WaitForFirstConsumer`"
(`website/docs/guides/deployment.md:205-213`):

> The executable lists are served only on ext4, overlay and tmpfs volumes today.

- Not under `deploy/helm/substrate`: `values.yaml` has no comment line today, the directory has
  no README, and `templates/NOTES.txt` prints what an install did. The guide is where the
  chart's values are explained.
- The page is public and describes the daemon. In these two stories the daemon serves no list
  (§ 15), so this document places the sentence with
  `story:exec-start-carries-executable-lists`, when the chart's daemon first serves one. That
  timing is this document's reading; the stakeholder's words give none. Until then the embedder
  notes carry the same statement.
- `cargo xtask check-docs` holds that page's reading load to `website/ux/baseline.json`; a
  sentence raises it, and the change that adds it moves the baseline with a decision
  (`AGENTS.md`, § *Public website*).

**A limit the place does not close: the layers of an overlay.** A directory that is a lower or
upper layer of an overlay, mounted in the view beside the overlay, shows the same bytes as
another file system. A deny entry on the overlay does not deny the layer's file (L1). In a
container the layer directories lie on the host, outside the container's mounts. On a
workstation an embedder would have to declare both the overlay and one of its layers as roots.
This is stated beside the hard-link limit of § 8.5, and no refusal is specified for it (§ 20).
The stakeholder decided on 2026-11-21 that the embedder notes say it as a rule, in these words:
"never list a lower directory of an overlay that is also in the view".

### The helper's arguments

**D24. The driver states for every path which mount it lies in and how it is to be opened.** The
helper cannot know which locations a run can write; the driver's mount table does.

```text
substrate-exec-gate --version
substrate-exec-gate <option>... -- <program> [<argument>...]

<option>  = --program  <class> <root> <rel>
          | --loader   <path>
          | --allow    <class> <root> <rel>
          | --deny     <class> <root> <rel>
          | --writable <root>
          | --env      <name> <value>
<class>   = follow | beneath
<root>    = the absolute view path of a mount: /usr, /bin, /lib, /lib64, /workspace, or the
            mount of a declared root or file
<rel>     = a normal relative path beneath <root>, or "." for <root> itself
<path>    = an absolute view path
<program> = the text of <root>/<rel> of --program
```

- **`follow`** is for a mount that shows nothing a run can write. The helper opens
  `<root>/<rel>` with `open(O_PATH | O_CLOEXEC)` and follows symbolic links.
- **`beneath`** is for a mount that shows something a run can write. The helper opens `<root>`
  with `O_PATH | O_DIRECTORY | O_NOFOLLOW` and `<rel>` beneath it with `openat2`,
  `RESOLVE_BENEATH` and `RESOLVE_NO_SYMLINKS`. A symbolic link in any part is an error, and so
  is a `<rel>` that leaves the root (K10).
- **`--program`** occurs exactly once and `--loader` at most once. `--allow` carries an entry of
  `exec_subpaths`, `--deny` an entry of `exec_denied_paths`. `--loader` is opened as a `follow`
  path.
- **`--writable`** names a mount of the view the run can write. The driver passes one for
  `/workspace`, for each writable root, for each mount it classed `beneath`, and for `/tmp`,
  `/dev` and `/scratch` where the view has them.
- Every operand is its own argument. A `<root>` is absolute and `<class>` is one of two words,
  so no operand reads as an option. The helper takes options only before `--`.
- An unknown option, a missing operand, a `<rel>` that is absolute or not normal, a second
  `--program`, or a `<program>` that differs from its option: the helper exits 125 and executes
  nothing.
- An `--allow` or `--program` path that does not open is an error. A `--deny` path that does
  not exist is kept as a name (§ 8.2).
- **`--env`** carries one variable of the program's environment (sixth revision, D28). Its
  form is a proposal, and the implementor fixes it in code. Seventh revision: it is the form
  `767fbea57` implements.
- **These arguments are everything the helper is given** (seventh revision, D29). It is the
  first program in the view, it is statically linked, and its environment holds `PWD` alone,
  so it reads its arguments, the view's mount table and the paths the arguments name, and no
  other input. `--loader` is the loader of the run's programs (D7). The helper itself has
  none.

The first draft's `--allow <path>` could not carry the rule of § 5: one absolute path does not
say where the writable mount begins. The second revision's `--alias` is gone: the helper reads
the view's mount table itself (D25).

**The class comes from the mount's host source, not from the mount's mode in this run.** A
read-only root is read-only for this run. Its host directory may still lie in a workspace that
another run writes, and then a link in it is a link a run planted. Opened as `follow`, such a
link to `/usr/bin` admits all of `/usr/bin` (K10). Nothing refuses such a root today: admission
checks the form of its two paths and opens the host path as a directory without following a
link (`crates/substrate-wire/src/lib.rs:4762-4775`,
`crates/substrate-host/src/read_only_roots.rs:109-124`).

The driver decides the class of every mount at each start:

| mount | class |
|---|---|
| `/workspace`, whatever its access mode | `beneath` |
| a writable root | `beneath` |
| `/usr`, `/bin`, `/lib`, `/lib64`, a read-only root, a read-only file | `beneath` when its host source lies in a writable location, or a writable location lies in it; else `follow` |

- **The writable locations** are `HostConfig.workspace_root`, the path of every
  `HostConfig.writable_roots` entry, and the host directories this start binds writable. The
  first two are in the list whether or not this start mounts them: another run can write them.
- **The host source** of a declared mount is the path of the descriptor the driver pinned and
  will bind (`read_only_roots.rs:30-33`), read from `/proc/self/fd/<n>`. For a base directory it
  is the canonical path of `/usr`, `/bin`, `/lib` or `/lib64`, which is what bubblewrap binds
  when it resolves the name.
- **"Lies in" is D25's**, on the driver's own mount table, in both directions. A source that
  holds a writable location shows that location through the mount (K9). A source inside one is
  writable as a whole.
- A source whose place cannot be established has no class. The start is refused
  `exec.sandbox-place-unserved` (D26); the mount is never classed `follow` for want of an
  answer.
- A mount classed `beneath` refuses an entry with a symbolic link in it,
  `exec.sandbox-path-unrepresentable` (§ 5). On a host whose `/usr` holds the workspace root,
  that includes `/usr/bin/sh` where it is a link.

**A `follow` path that resolves into a writable mount is refused.** A link in a read-only root
can point into `/workspace`. What it names is then a file the run can replace. The rule holds for
every option that carries a `follow` path: `--program`, `--loader`, `--allow` and `--deny`.

- **The driver, before dispatch.** It resolves each `follow` entry as the view will, which it
  already does to refuse an entry that does not exist (§ 5). When the result lies in a mount it
  passes as `--writable`, the start is refused `exec.sandbox-path-unrepresentable` (422), with
  the entry's field as its address and its audit record.
- **The helper, at the point of use.** After it opened a `follow` path it reads the path of the
  descriptor (`/proc/self/fd/<n>`). When that path lies in a `--writable` root, by place (D25),
  it executes nothing, exits 125 and prints one line that names the option, the path as
  written, what it resolved to and the mount:

  ```text
  substrate-exec-gate: --allow /opt/ro/link resolves to /workspace/tools/bin,
  in the writable mount /workspace
  ```

  The message is one line; it is broken here for the page.
- **By place, so that the helper does not rest on the driver's classes.** With every class
  right, the text of the resolved path would do: each mount that shows something writable is a
  `--writable` root. The helper compares places all the same. A read-only mount the driver
  wrongly passed as `follow`, whose host directory holds the workspace or lies in it, shows
  workspace files under a path that is not beneath `/workspace` as text. The place finds both;
  the view's ancestors find only the first (K9).
- The loader goes through the same check in the probes of § 9. On a host where it fails, the
  probe fails and the fact is absent.

### The run's variables

**D28. On a start with a list, no variable of the run reaches a process that runs before
`landlock_restrict_self` returned.** New in the sixth revision. The coordinator of the request
decided it on 2026-11-21, after the adversary pass and the security review of story 1's
implementation, and gave it to the implementor. The stakeholder has not read it.

**Seventh revision: implemented at `767fbea57`, and its open points are settled.** The
coordinator decided the three points this section marked as proposals: no `--setenv` at all on
a start with a list (rule 1), a helper that refuses and does not clear (rule 5), and the
operand form `--env <name> <value>`. The commit has no `env` stage on such a start. The words
"proposes", "proposal" and "the implementor's" below are the sixth revision's and are kept as
its record. D29 turns rule 2 into a stronger one: no stage runs before the helper at all.

**The argv at `4b5208922`**, in `crates/substrate-host/src/process.rs`:

| part | what it does | where |
|---|---|---|
| the driver's own environment | cleared before bubblewrap is spawned | `:2887` |
| `--clearenv` | bubblewrap clears what it inherited | `:2918` |
| `--setenv <name> <value>` for each baseline name the start allows (`LANG`, `LC_ALL`, `PATH`, `TERM`, `TZ`) | a fixed value per name | `:3011-3013`, `crates/substrate-guarded/src/environment.rs:60` |
| `--setenv <name> <value>` for each variable of the run | the caller's names and values (`ExecEnvironment.set`) | `:3014-3016` |
| `--setenv` of the secret-slot mapping | names and descriptor numbers, when the start names a slot | `:3017-3021` |
| `--setenv SSL_CERT_FILE <path>` | when an egress anchor was generated | `:3022-3031` |
| `--` | the end of bubblewrap's options | `:3032` |
| `/usr/bin/setsid --ctty` | on a terminal start only: takes the terminal as the controlling one | `:3037-3039`, `crates/substrate-host/src/pty.rs:44` |
| `/usr/bin/env -u PWD --` | bubblewrap sets `PWD` itself after `--clearenv`. This stage removes it, so the program does not see a variable its request did not set | `:3040-3042` |
| `/usr/bin/env -- PWD=<value>` | only when the run set `PWD`: the stage before removed that one too, and this puts the run's value back | `:3043-3046` |
| the helper and its options | only with a list (D5) | `:3050-3054` |
| the run's program and arguments | | `:3055` |

Every variable bubblewrap sets is in the environment of every program after `--`. So `setsid`,
both `env` stages and the helper started with the run's variables. `setsid` and `env` are the
host's programs, and `env` is dynamically linked on the probe host (E3). The lane's helper was
a default build, dynamically linked too (E2). A loader acts on `LD_PRELOAD`, `LD_LIBRARY_PATH`
and `LD_AUDIT` before the program's first instruction, and the object those name can be a file
of the workspace (E1). That is the hole: E2 is the helper's own process, E3 is `/usr/bin/env`.

**The rule, on a start whose profile has a list:**

1. **Bubblewrap is given no `--setenv` for a variable of the run.** This document proposes one
   rule for all four groups of the table: no `--setenv` at all. The baseline values, the
   secret-slot mapping and `SSL_CERT_FILE` are the driver's own text and name no object, so the
   decision does not need them gone. One rule is simpler to hold: the test is that the argv of
   such a start has no `--setenv`, and the program's environment then has one source.
2. **No stage before the helper starts with a variable of the run.** With rule 1 that follows:
   a stage has what bubblewrap left, which is `PWD` alone. Bubblewrap sets `PWD` to the
   directory it changed to. Its value is the run's working directory, a view path the driver
   has checked (design 26), and no loader acts on that name (§ 18).
3. **The driver hands every variable to the helper as operands**, after the lists and before
   `--`.
4. **The helper gives the variables to the `execve` of the program and to nothing else.** It
   builds the program's environment from the operands, and executes the program with it only
   after `landlock_restrict_self` returned 0. Nothing of the helper's own environment is
   passed on.
5. **The helper does not rest on its caller.** Started with a variable in its own environment
   other than `PWD`, it executes nothing: exit 125 and one line that names the variable.
6. **No list of refused names.** `substrate_guarded::environment::check` does not change.
   `LD_PRELOAD` stays an admitted variable of a run and reaches the run's program.
7. **A start without a list keeps its argv, byte for byte** (D5). Its stages start with the
   run's variables as they do today. There is no ruleset on such a start for anything to come
   before.

**Rule 5 is stricter than the decision as given, and this is why.** The decision's words are
that the helper "clears its own if it is not empty, so the property does not rest on the
caller". Clearing meets the second half for the program's environment. It cannot meet D28's
own sentence:

- A variable in the helper's environment has already reached a process that runs before the
  ruleset: the helper. In a dynamically linked helper the loader has acted on it before the
  helper's first instruction, and no instruction of the helper undoes that (E2).
- The same variable was in the environment of every stage before the helper, because a stage
  passes its environment on. Clearing in the helper says nothing about E3.

Refusing turns the helper into the witness for the whole chain. Neither `setsid` nor
`env -u PWD` removes a variable other than `PWD`, so a helper that starts with none shows that
no stage before it started with one. The refusal does not stop E3 itself: where a caller leaks
and an object acts in a stage before the helper, the helper never runs. What it does is make
the leak fail every ordinary start that has a variable, with exit 125 and a line that names
it, so a driver that leaks passes no lane case with a list. The probe of § 9 checks that the
configured helper has this refusal. The implementor's change, in progress while this was
written, takes the same form; either form keeps rule 4, and which one ships is the
implementor's and the coordinator's.

**The operand form is a proposal. The implementor fixes it in code.**

```text
--env <name> <value>
```

- One option per variable, with the name and the value each an argument of its own. No byte of
  a value then reads as an option, as `--` or as the `=` of another variable.
- The helper refuses, with exit 125, an empty name, a name that holds `=`, and a name given
  twice: it does not choose between two values.
- The program's environment is the operands, each name once. Where two groups of the table
  name one variable, the later `--setenv` wins today: a variable of the run over a baseline
  value, and the driver's `SSL_CERT_FILE` over the run's. The driver resolves that the same way
  before it writes the operands.
- `PWD` is a variable like any other: present when the run set it, absent otherwise. Neither
  `env` stage is needed for that on a start with a list. Whether the first one stays is the
  implementor's. Started with `PWD` alone it changes nothing the program sees.
- The protocol line stays `substrate-exec-gate/1`. No helper of the first form was released,
  so no helper exists that prints the line and lacks the option.

**What does not change.**

- **The size of a start.** The variables were arguments of bubblewrap before, as `--setenv`
  operands, and are arguments of bubblewrap now. The kernel counts argument and environment
  strings against one limit (`execve(2)`), and the driver's rule bounds them at 64 variables of
  at most 128 and 4096 bytes. Read, not run (§ 18).
- **What a reader of the process table sees.** A value was in bubblewrap's arguments before
  and is there now. No secret is a variable: the rule refuses a name that names a credential,
  and a secret slot is a descriptor.
- **D7.** The loader is read at open from the `PT_INTERP` of the file `/usr/bin/env`. That
  does not depend on a start executing that file.
- **`render`.** It shows the argv of a start with a list with its `--env` operands and no
  `--setenv`, as the start builds it (D22).

**What remains a limit after D28.** Inside the domain, a preload object is loaded into every
allowed dynamic program and can run code, as an allowed interpreter can; `execve` of an
unlisted path still fails. § 7 states it with the other limits. The first half follows from
A10: mapping a file as code is not execution. The cases of E1 to E3 hold the second half: with
the object named and D28 in place, the unlisted program ends with status 126. Neither half had
been observed with D28 in place when the sixth revision was written. Both are observed since:
the cases of E1 to E3 pass on `767fbea57`, and E9 lists the processes the object was loaded in
(§ 3.11).

**What D28 does not cover.** A loader has inputs that are not variables, such as
`/etc/ld.so.preload`. The base view holds no `/etc` beyond `/etc/alternatives`
(`process.rs:2931-2936`), and a run cannot mount (W6). The sixth revision wrote here that
whether a declared read-only file may be mounted at that path was not checked. It may, and
the second reviews measured what follows (E6, E7): the helper's own loader read the file. D29
is the answer.

### Before the ruleset

**D29. On a start with a list, the helper is statically linked and is the first process
bubblewrap starts in the view.** New in the seventh revision. The coordinator of the request
decided it on 2026-11-21, after the second adversary pass and the second security look, and
gave it to the implementor. The stakeholder, session `sdc`: "No objection to D29: static
helper, the driver refuses a helper that needs a dynamic loader, and a listed start with a
controlling terminal is refused before dispatch until the helper takes the terminal itself."
It added what rule 1 tests and two cases of § 16.

**What was found.** A dynamically linked program does not start at its own first instruction.
The kernel starts the loader the program names in `PT_INTERP`, and the loader decides what
else is mapped before the program runs. It decides from variables, which D28 removed, and from
files of the view. A profile may declare a read-only file at any path the driver does not own,
`/etc/ld.so.preload` among them (§ 3.11). With one there, the helper's loader mapped an object
of the workspace into the helper's process, and the object started the run's program with no
ruleset (E6, E7). With a static helper the same start held (E8).

**The rule:**

1. **The helper is statically linked, and the open refuses one that is not.** The driver reads
   the helper's bytes once, through the one descriptor of D20, and tests those bytes before it
   seals them:
   - they are an ELF image whose class, byte order and machine are the driver's own;
   - the whole program header table lies in the bytes and was read, and its count is not the
     escape value for a table too large for the header;
   - no program header has the type `PT_INTERP`.

   Anything else is refused `config.exec-gate-invalid` (422), address `config.exec_gate`, and
   the message says which of the three failed. The test is "no `PT_INTERP`", not "no
   `PT_DYNAMIC`": a static position-independent program has a dynamic section and names no
   loader, and it passes. E8's helper is one. The bytes tested are the bytes sealed, bound and
   executed, so there is nothing to swap between the test and the `execve` (D20, step 7). The
   first two points are this document's reading of "tests the program headers": a file that is
   not an ELF image, or whose headers cannot be read, has no `PT_INTERP` to find, and passing
   it would let a `#!` script through, whose interpreter the kernel starts before it.
2. **No list of loader files is kept.** The driver does not refuse a declared mount at
   `/etc/ld.so.preload`, and `validate_read_only_files` does not change.
3. **On a start with a list no program runs in the view before the helper.** After
   bubblewrap's `--` the next argument is the helper's mount. No `env` stage, which
   `767fbea57` already has, and no `setsid`.
4. **A start with a list and a controlling terminal is refused before dispatch**, by name,
   until the helper takes the terminal itself. Today the terminal is taken by
   `/usr/bin/setsid --ctty`, a program of the host that would run before the helper (§ 3.11).
5. **The lane builds the helper static, and refuses by name where it cannot.** It never falls
   back to a default build (§ 16).
6. **A start without a list is unchanged.** It has no helper, and its stages run as before.

**Why not the narrower correction.** Refusing a declared mount at `/etc/ld.so.preload` on a
start with a list would turn E6 and E7 green. It is not taken, because that path is one member
of a class and the class has no end the driver could write down:

- `/etc/ld.so.cache` tells a loader which file a library name means. A declared file there
  points the helper's `libc` at a file of the workspace.
- A declared file or root mounted over a library the helper links, or over a directory its
  loader searches, replaces the library itself. The reserved mounts refuse `/usr`, `/lib` and
  `/lib64` and what lies beneath them (`crates/substrate-wire/src/lib.rs:4782-4797`), so this
  route needs a search path outside those. A run path in the helper, the cache above or a path
  file below gives one.
- Another `libc` reads other files. musl's loader reads a path file, `/etc/ld-musl-<arch>.path`,
  and each loader version may add one.
- A loader named by a relative `PT_INTERP` is resolved from the directory the start changed
  to, which is the run's.

A list of paths would be right for the loaders it was written from and silent about the next.
A helper with no loader reads none of them. The existing reader for D7 shows how easily the
narrow form goes wrong: `elf_interpreter` answers "none" for a `PT_INTERP` it cannot use as a
rule's path, such as one that is relative or not normal
(`crates/substrate-exec-gate/src/lib.rs:359-386`). That is right for D7 and would be wrong
here. Rule 1 therefore asks whether the header exists, not what it holds, and needs its own
function.

**The refusal of rule 4: a proposal, and the implementor's to confirm.** No existing code was
written for this case. Read from § 12 and from the wire crate:

| code | what it says today | fit |
|---|---|---|
| `session.pty-unserved` (501) | "This deployment never proved it can give a confined process a controlling terminal" (`crates/substrate-wire/src/lib.rs:434-435`); raised before dispatch for a `pty` session on a driver without the fact, with the address `mode` (`crates/substrate-host/src/process.rs:1019-1025`) | **proposed.** A start with a list and a terminal is a start this driver cannot give a terminal. The class is `Unserved` and the status 501, which is invariant 3's answer for a missing guarantee. The message says why: the profile has a list, and the helper does not take the terminal yet |
| `exec.sandbox-unavailable` (501) | a host without the sandbox | no: D26 already declined it, because a client could not tell "no sandbox" from this |
| `confinement.fact-unserved` (501) | an adapter that did not prove a fact | no: it is admission's code and names a fact, and no fact says "a terminal with a list" |
| `exec.sandbox-path-unrepresentable` (422) | the client wrote something this policy cannot carry | no: the request is well formed, and the host does not serve it |

- **It changes the code's meaning in one point.** Today the code is about the deployment: the
  fact `sessions.pty` is absent. Here a driver that proved the fact answers it for one kind of
  start. No wire client can see that today: a terminal belongs to a session start, the wire
  paths carry no list (D22), and no adapter profile reaches a terminal start (§ 3.11). So the
  refusal is a guard in the function that builds the argv, for a caller that does not exist
  yet. When `story:exec-start-carries-executable-lists` gives a session start a list, that
  story decides between this code and a new one. A new code is a wire change and needs its
  bundle.
- **The alternative is a new code of the host crate**, in the family of
  `exec.sandbox-place-unserved`. It would be exact, and it would be a second new refusal for a
  start nobody can make. That is the reason for the proposal, and it is a judgement.
- **Until the helper takes the terminal itself.** `setsid --ctty` makes a session and takes the
  inherited terminal as its controlling one. The helper could make the same two calls before
  it applies the ruleset, with `libc` alone. That is not designed here (§ 20).

**What D28 and D29 give together.** On a start with a list, before `landlock_restrict_self`
returns, the processes in the view are bubblewrap's own and the static helper; the helper's
environment holds `PWD` and nothing else, and its arguments are the driver's; nothing the run
or the profile names is loaded as code.

The basis, part by part:

| part of the sentence | basis |
|---|---|
| the processes are bubblewrap's own and the helper | the argv has nothing between `--` and the helper's mount (rule 3; a test of the argv, § 16). E12's case holds that for a quiet start and a start with piped input at `767fbea57` |
| the helper is static | rule 1, on the bytes that are bound. E8 shows a static helper is not reached by the file |
| its environment holds `PWD` alone | the driver clears its own environment before it spawns bubblewrap (`process.rs:2920`), passes `--clearenv` (`:2951`) and no `--setenv` (`:3054`), and the helper refuses any other variable (D28, rule 5) |
| its arguments are the driver's | `--` is followed by the helper's mount and the options the driver wrote (`:3080-3089`). The run's values are in them as operands and are read as data |
| the helper is not replaced by a declared mount | its bytes are bound after every other mount (D20). E10 |
| nothing the run or the profile names is loaded as code | a static program has no loader, so no file and no variable chooses what is mapped. What the helper opens are its arguments' paths and the view's mount table, and it reads them as data |

**What that sentence still rests on, and what was not run.** Each line is a claim about a part
this repository does not own or did not probe:

- **Bubblewrap between its own start and the `execve` of the helper.** Bubblewrap is the
  host's program and is dynamically linked on the probe host (`file`). Its loader ran on the host,
  before the view existed, with the environment the driver cleared, so the view's files did
  not reach it. That it maps nothing more once it is inside the view (no library opened late,
  no name service, no locale file) is not read from its source and not run. Its process stays
  in the view as the parent of the run.
- **The helper's mount is the file that is executed.** Bubblewrap executes the first argument
  after `--`. The driver writes an absolute path there, so no `PATH` is searched; that
  bubblewrap would search one for a name without a slash is not read.
- **The kernel starts a static ELF program with no other program.** That is the ELF loader of
  the kernel. A host can register a handler (`binfmt_misc`) that claims a file by its first
  bytes and starts an interpreter of the host's choosing. The probe host has none registered:
  `/proc/sys/fs/binfmt_misc` holds `register` and `status` and nothing else. Other hosts were
  not read, and the driver does not check it. Rule 1's test of class, byte order and machine
  is there so that a helper for another machine, which only such a handler could start, is
  refused.
- **A static `libc` that loads nothing.** A program can have no `PT_INTERP` and still map a
  library later, by calling the `libc` function for it. glibc's static form does that for
  name-service and character-set lookups. The helper calls neither, and E8's helper is
  musl's. That no call of the helper or of the Rust runtime's start reaches such a function
  is read from the crate having no dependency beyond `libc` (D5), not from a trace.
- **The working directory.** Bubblewrap changes to the run's directory before it executes the
  helper (`--chdir`, `process.rs:3037-3038`). The helper's own path is absolute and its `--env`,
  `--allow` and `--program` operands are checked forms, so nothing is resolved from that
  directory; a relative `PT_INTERP` would have been (rule 1 refuses it with every other).
- **The descriptors.** The helper inherits what the run will: the three standard streams, the
  secret-slot descriptors and the start's own descriptors (`--block-fd`, `--info-fd`,
  `process.rs:3039-3042`). The helper reads none of them (read from its source, not traced).
- **The seccomp program.** Bubblewrap installs the run's program before it executes the helper
  (`--seccomp`, `process.rs:2981`), so the helper runs under it. That it lets the helper's
  calls through is observed (the lane's cases run); it is a filter and maps nothing.
- **`systemd` around `linux`.** The stages `systemd` adds run on the host, outside the view,
  and the profile's mounts exist only in the view. Read from
  `crates/substrate-host/src/confinement/systemd.rs:704-712`; the one lane case with a list
  runs through it (§ 16).
- **The probes of § 9 and the `--version` run** build their own bubblewrap arguments and
  declare no file, so the route does not exist there.

**The limit that remains, unchanged in class.** Inside the domain the program's own loader
reads `/etc/ld.so.preload` and the environment, as an allowed interpreter reads a script. An
object named there is loaded into every allowed dynamic program and runs its code inside the
domain; `execve` of an unlisted path still fails. E8 is that observation: the object was
loaded in `/usr/bin/sh` and `bin/tool`, and `other/tool` ended with status 126. § 7 states it.

### Embedder notes

An embedder of the in-process adapter links `b10x-substrate-host` as a library. Cargo does not
build a dependency's binary, so the helper does not arrive with the crate. Loom is the first such
embedder. It must:

1. **Build `substrate-exec-gate` from the revision it pins the crate at**, and install it beside
   its own binary. **The build is static. That is the rule since the seventh revision (D29);
   it was a recommendation before.** The target is `x86_64-unknown-linux-musl`, which
   `rust-toolchain.toml` pins and the `Dockerfile` builds `substrate-executor` for:

   ```console
   cargo build --release --locked --target x86_64-unknown-linux-musl \
     -p b10x-substrate-exec-gate --bin substrate-exec-gate
   ```

   The driver refuses a helper that names a loader, `config.exec-gate-invalid`, so a default
   build no longer opens. A static helper also starts in any view, whatever `libc` the host
   has. The check a build can make itself is the one the `Dockerfile` makes for the executor
   (`Dockerfile:124`): the program headers hold no `INTERP`.
2. **Name it in `HostConfig.exec_gate`** as an absolute path. The driver searches nowhere: not
   `PATH`, not the directory of the running executable.

What the file must be:

| requirement | why |
|---|---|
| named by an absolute path | D21 |
| outside the workspace root and every declared writable root | D21: a run could have written it |
| a file with one link | D21: a second name could lie in a workspace. Install a copy, not a hard link |
| a regular file the driver's uid can read | the driver reads its bytes (D20). It needs no execute bit: bubblewrap sets the mode of the copy |
| at most 64 MiB | `MAX_BACKEND_BINARY_BYTES` (`probe.rs:22`) |
| statically linked: an ELF program for the driver's machine with no `PT_INTERP` | D29: a loader reads files and variables a profile can supply, before the helper's first instruction. A static position-independent build passes |
| able to start inside the view | a static helper needs nothing of the view. Before the seventh revision this row said that only `/usr`, `/bin`, `/lib` and `/lib64` of the host are there (`process.rs:2732-2743`) |
| of the driver's protocol | it answers `--version` with the line `substrate-exec-gate/1` and takes the arguments of D24 |

What the driver does with it at open is D27's list of six steps: the path's form, the file,
the places, the trust check of D21, the test of the bytes for a loader (D29), the seal, hash
and `--version` of D20, and the two probes of § 9, with the SHA-256 into `BackendBinding` at
the end. On a host whose file system is outside
D26's list the open stops after the places and the helper is never used. The driver never
executes the file outside a sandbox. Who may write the file and its directory is the
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
- **A toolchain root lies outside the workspace root and every writable root.** A read-only root
  inside one is classed `beneath` (D24), and an entry or program with a symbolic link in it is
  then refused. On the probe host `npm`, `sh` and the rustup proxies are links (T5, T10).
- **A missing entry shows as `Permission denied`** naming the program the tool could not start
  (T2, T3, T7), or as `bad interpreter` for a script (T12).
- **The file systems the lists are served on are `ext4`, `overlay` and `tmpfs`** (D26). The
  helper's file, the workspace root, every declared writable root, the base directories, the
  temporary directory and every root a listed start declares must lie on one of them.
  - With `exec_gate` set and one of the first five elsewhere, for example a workspace volume on
    `xfs` or `btrfs`, the adapter opens and serves no list (D27). Read `facts().withheld` on the
    adapter `open` returned: each entry names the fact, the cause, the path and the type. A
    profile with a list is refused `confinement.fact-unserved`, and the refusal's text repeats
    the first entry. The driver also writes one `warn` line per entry.
  - A start that declares a root elsewhere is refused `exec.sandbox-place-unserved`.
  - A temporary directory elsewhere is moved with `TMPDIR`, which `tempfile::tempdir()` reads.
- **Every entry must exist at the start** (§ 5). The client creates `target` or `node_modules`
  in a fresh clone before it starts the run.
- **The run's variables are not filtered, and they reach the program.** A `Command.env` that
  names `LD_PRELOAD`, `LD_LIBRARY_PATH` or `LD_AUDIT` is admitted. On a start with a list the
  variables reach the run's program and no program before it (D28), so an object they name
  cannot start the run without its list. Inside the run it is loaded into every allowed
  dynamic program and runs its code there (§ 7). An embedder that does not want that leaves
  those names out of the environment it builds; the driver does not do it for the embedder.
- **A declared file at a path a loader reads is not refused either.** A profile may declare a
  read-only file at `/etc/ld.so.preload` or `/etc/ld.so.cache`. The helper is static and reads
  neither (D29), so such a file cannot start the run without its list. Inside the run the
  loader of every allowed dynamic program reads it, as it reads the variables above (§ 7, E8).
  An embedder that takes `read_only_files` from a source it does not trust checks the mount
  paths itself.
- **A start with a list has no controlling terminal** (D29). No adapter profile asks for one
  today.
- **Write `workspace_root` and every `writable_roots` path without `..` and `.`.** With
  `exec_gate` set, such a path that does not exist yet is refused at open,
  `config.exec-gate-unserved` with the cause `path-unopenable` (D25).

list `/usr/bin`, not `/usr`

A rule on `/usr` does not reach a program that is named through `/bin`, `/lib` or `/lib64`
where those are links into `/usr`, so with `/usr` listed `/bin/sh` and every script that starts
with `#!/bin/sh` are `Permission denied`, and with `/usr/bin` listed they run (§ 5, K7).

never list a lower directory of an overlay that is also in the view

A deny entry on the overlay does not deny the same file reached through the lower directory,
because the kernel treats the two as different files (L1, D26).

### Without a usable helper

**D19. A profile that carries a list never runs without it. Which refusal answers depends on what
the embedder configured.** Decided with D5.

**The first sentence holds only with D28.** The table below is about the open and admission:
it shows that no profile with a list is admitted where the list cannot be applied. It says
nothing about a start that was admitted. Such a start ran without its list where a variable of
the run named an object that executed before the helper applied the ruleset (E1 to E3). D28
is the rule for the start; this table is unchanged by it.

**Since the seventh revision: only with D28 and D29.** With D28 in place a start still ran
without its list where the profile declared a file that the helper's loader read (E6, E7).
D29 changes this table in one row: the second, which now also refuses a helper that names a
loader.

| situation | answer | code and status |
|---|---|---|
| `exec_gate` is not set | the adapter opens; neither fact is proved; a profile with a list is refused at admission | `confinement.fact-unserved` naming `exec_subpaths`, 501 |
| `exec_gate` is set and is not absolute, or the file lies beneath the workspace root or a declared writable root, or has more than one link (D21); or, seventh revision, its bytes are not an ELF program for the driver's machine without `PT_INTERP` (D29) | the open is refused: before the file is read for the first three, on the bytes read for the last | `config.exec-gate-invalid`, 422, address `config.exec_gate`; new |
| `exec_gate` is set and the file is missing, not a regular file, unreadable, over the size bound, or does not print the protocol line in the throwaway sandbox | the open is refused; no adapter exists, so nothing is admitted | `config.exec-gate-unserved`, 501, address `config.exec_gate`; new |
| `exec_gate` is set and the helper's file, the workspace root, a declared writable root, a base directory or the probes' temporary directory lies on a file-system type outside D26's list, or the kernel reports no mount ID (D27) | the adapter opens; neither fact is proved; the proof names the cause, the path and the type; a profile with a list is refused at admission, and the refusal repeats them | `confinement.fact-unserved` naming `exec_subpaths`, 501, with the cause |
| `exec_gate` is set and the place of one of those paths cannot be established for one of the other seven causes (D26) | the open is refused | `config.exec-gate-unserved`, 501, address the field whose path it was; the message names the path and the cause |
| `exec_gate` is set and usable, and a probe of § 9 did not hold (no Landlock, ABI 1) | the adapter opens; the fact that probe proves is absent; a profile that needs it is refused at admission | `confinement.fact-unserved` naming the fact, 501 |
| the file changed after the open | nothing: the bytes read at open run (D20) | none |

The rows apply in the order of D27's six steps. On a host that withholds the facts, the trust
check of D21, the protocol line and the probes are not reached: the helper is never used there.

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
- **A file system outside the list is not a configuration error either** (D27, decided by the
  stakeholder on 2026-11-21). The reason above for refusing was that an absent fact hides its
  cause. Here the cause travels with the absence: in the proof, in the refusal and in one `warn`
  line. The same helper on an `ext4` volume proves the facts.
- **A probe that did not hold is not a configuration error.** The same helper on another kernel
  proves the fact. The fact is absent, as for every probed fact.
- **Without the floor nothing is probed.** The protocol line is read in a sandbox, so a host
  without the exec floor reads none; both facts are absent there, with every other exec fact.

**D21. A helper a run could have written is refused.** The helper decides what a run may execute.
A file beneath `HostConfig.workspace_root` or a `HostConfig.writable_roots` path could have been
written by a run, so it cannot be that authority. The driver refuses the open
`config.exec-gate-invalid` when one of these holds:

| check | how | why |
|---|---|---|
| the configured path is not absolute | lexical | a relative path depends on the driver's working directory |
| the file lies in the workspace root or a declared writable root | the path of the opened descriptor, and the canonical form of `workspace_root` and of each `writable_roots` path; then their places are compared (D25). A place that cannot be established does not pass the check. For two causes the facts are withheld and the helper is never used (D27); for the other seven the open is refused `config.exec-gate-unserved` (D26) | both sides are resolved. `HostConfig` holds the paths as the embedder wrote them: nothing in `lib.rs` canonicalizes them, and the adapter resolves the root where it needs it (`linux.rs:353`). Unresolved text would miss a workspace root named through a symbolic link. Canonical text would still miss a workspace root that the host mounts at a second path (K6); the place does not |
| the file has more than one link | `st_nlink` of the opened descriptor is above 1 | a hard link inside a workspace is another name of the same file, and the path check cannot see it (K4). Whether one of the other names lies in a workspace cannot be answered without walking every workspace. The link count is one `fstat` |

The code is a `DriverError` of class `Refused` (status 422), the form of
`config.writable-root-invalid` and `config.tmp-bytes-invalid` (`lib.rs:385`, `:475`). Its message
says which check failed. A helper that a package manager installed with a second hard link is
refused too; the embedder installs a copy. The same checks are not made for
`HostConfig.bubblewrap` today; this document does not change that.

**A helper that needs a loader is refused with the same code (seventh revision, D29).** The
three checks above ask whether a run could have written the helper's file. This fourth asks
whether a run or a profile could supply code to the helper's process, and a loader is how:

| check | how | why |
|---|---|---|
| the bytes are an ELF program for the driver's machine, with a program header table that was read whole and holds no `PT_INTERP` | on the bytes of the one read through the one descriptor, before they are sealed (D20, step 7). The answer has three values and only the first passes: no `PT_INTERP`; a `PT_INTERP`, whatever it holds; not established, for bytes that are not such an image | a loader maps what variables and files of the view name, before the helper's first instruction (E2, E6, E7). `PT_DYNAMIC` is not asked: a static position-independent program has one and names no loader (E8) |

- **`config.exec-gate-invalid`, not `-unserved`.** The file is not a helper this driver takes,
  on any host; a host that cannot use a good helper is the other code. The message says
  "names the loader `<path>`", "is not an ELF program", or which part of the header could not
  be read.
- **The order.** After the three checks above and before the seal (D27, step 5), so a helper
  in a workspace is still refused for that, and the protocol line is read only from a helper
  that passed.
- **It is a check of the bytes, not a probe.** Nothing is executed for it, so it needs no
  sandbox and is made wherever the open reaches step 5. § 9 gains no half.

**A root spelled through a name that does not exist yet is not compared; the open is refused.**
E5's configuration, `workspace_root` written `<ws>/new/..` with `new` absent, has no place
(D25): the cause is `path-unopenable`, one of the seven that refuse the open, so the answer is
`config.exec-gate-unserved` (501) with the address `config.workspace_root`, and the message
names the path and the cause. It is not `config.exec-gate-invalid`:

- that code says the helper lies in a writable location, and the driver cannot say so of a
  location it could not place. The security review's case asserts
  `config.exec-gate-invalid`, the answer for the same root spelled `<ws>`. Under this rule the
  open is refused with the other code, whether or not the helper lies beneath `<ws>`; the case
  or this paragraph has to move, and that is the coordinator's;
- the driver could create the root first and place it as a directory that exists, which gives
  `config.exec-gate-invalid` here. That is not taken: the driver would write beneath a path it
  has not checked, and a `writable_roots` path that does not exist is not the driver's to
  create.

A deny-only profile is refused by D13 before any of these. In no row does a run start without its
list. Since the sixth revision: no row of the open or of admission. For a start that was
admitted, D28 is what makes it true.

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
failure (no Landlock, an argument it does not accept, a variable in its own environment (D28),
an entry it cannot open, a place it cannot establish (D26), a `follow` path into a writable
mount, a rule the kernel refuses) ends the run
before the program starts: one diagnostic line on standard error that
begins `substrate-exec-gate:`, and exit status 125. When `execve` of the program fails it exits
126, or 127 for `ENOENT`, as `/usr/bin/env` does in that position today. The driver has checked
every entry before dispatch (§ 5), so a helper failure means the kernel changed its answer after
the probe or a path changed after the check. Nothing runs unrestricted (invariant 3).

**The status is ambiguous.** A program that itself exits 125, 126 or 127 looks the same to the
adapter. Only the diagnostic line tells them apart, and a program can print that line too. A
named failure needs the status descriptor of § 20.

## 7. What the list does not stop

**D9. The list restricts which files `execve` and `execveat` accept. It does not restrict what
code an allowed program runs.** The fact's documentation states these limits. The first four are
probes; the fifth, added in the sixth revision, is inferred from the fourth.

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
- **Inside the domain, a preload object is loaded into every allowed dynamic program and can
  run code, as an allowed interpreter can; `execve` of an unlisted path still fails.** Stated
  since the sixth revision. The run's variables reach the run's program (D28), and the driver
  refuses no variable by name, so `LD_PRELOAD`, `LD_LIBRARY_PATH` and `LD_AUDIT` do what they
  do in any process: the loader of each allowed dynamic program maps the object, which is A10,
  and runs its constructor. A statically linked program loads nothing. The object's code is
  inside the domain like the program's own: it can do what the four limits above let any
  allowed program do, and an `execve` it makes of an unlisted path fails with `EACCES`. That
  the constructor runs in an allowed program is inferred from A10; the lane cases of § 16 hold
  both halves.
  - **Seventh revision: the limit is the loader's, whatever its input.** Inside the domain the
    program's own loader reads `/etc/ld.so.preload` and the environment, as an allowed
    interpreter reads a script. A profile may declare a read-only file at that path (§ 3.11),
    and no variable is needed then. The class is unchanged: code an allowed program maps.
  - **It is observed now.** With a static helper and such a file, the object was loaded in the
    allowed shell and the listed tool, and the unlisted tool ended with status 126 (E8). E9
    shows the same for an object the environment names.
  - **What is not this limit:** the same object in the helper's process, before the domain
    (E6, E7). D29 closes that.
- **On `macos` the last four do not apply in the same way**: Seatbelt also confines
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
  the measured cost per rule (§ 18). W12 used the text comparison. The fourth revision's probe
  helper, which opens every entry of an expanded directory and looks its row up anew for each
  deny entry, took 92 ms for 5963 rules (C1). The helper does less: one `statx` per entry,
  which the walk makes anyway, and an entry whose mount ID is its directory's has the
  directory's place and its own name. That cost is not measured (§ 18). `exec_subpaths` has no
  bound today and gets none here; the wire story bounds both when it gives them a schema.
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

1. `A` lies in a denied path, or is a file with the device and inode of a denied file: no rule.
2. No denied path lies in `A`: one rule on `A`.
3. `A` is a directory and a denied path lies in it: no rule on `A`; the same three steps for
   each entry of `A`. A symbolic link gets no rule: what it points at has its own place in the
   walk, or is outside the list.

"Lies in" is D25's: it compares places, and a path lies in itself. Step 1 covers a listed
directory inside a denied one: `/usr/bin` listed beside a denied `/usr` gets no rule (K8).

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
| a second mount | `/usr/bin` listed and `/bin/id` denied: the entry is not beneath `/usr/bin` as written, `/usr/bin` gets one rule for the whole directory, and `id` runs under both names (B4) | steps 1 to 3 compare places (D25): `/bin/id` has the place of `/usr/bin/id`, the directory is expanded, and `id` is denied under both names (K5) |

**A denied program is refused before dispatch.** D10 says a denied path wins, so the start cannot
succeed. The driver resolves the program and every deny entry through the descriptors it pins the
mounts with, as § 5 resolves an entry. It refuses the start `exec.sandbox-program-unlisted` (422)
when the program lies in a denied path, or is the same file, by device and inode, as a denied
file. "Lies in" is D25's, on the driver's own mount table. `/usr/bin/sh` beside a denied
`/usr/bin/bash` is that case on the probe host. The helper is the second line: should the driver
miss it, the program has no rule and nothing runs (B2). The driver does not refuse a denied
loader. A static program still runs, and the client wrote the entry.

**One directory under more than one path.** Three things put one directory, or a part of it, at
a second path of the view:

- the base view, where the host links `/bin`, `/lib` or `/lib64` into `/usr`;
- two declared mounts: admission compares the mount points of read-only roots and never their
  host paths, so one host directory can be declared twice, and one root's host directory can lie
  inside another's (`crates/substrate-wire/src/lib.rs:4762-4775`);
- the host itself, which can mount one directory at two host paths.

A Landlock rule is on the directory itself, so a rule on one mount admits the directory under
the other (B5). What holds a deny entry against that:

- Steps 1 to 3 compare places (D25). The helper reads the view's own mount table, so the driver
  passes nothing for this, and all three cases look the same to the helper: two mounts with one
  device, and roots that are equal or nest.
- The second revision had the driver pass `--alias` for each pair of mounts whose host sources
  nest by path. A directory with two host paths has two sources that do not nest as text, so
  that rule would not have paired them (K6 shows such paths); the text of the view paths misses
  the case as well (K5). `--alias` was never run, and it is gone.
- Observed with the place rule (K5): one directory under two names (`/usr/bin` listed with
  `/bin/id` denied, and the reverse); a part of a listed directory under a second name (`/usr`
  listed with `/bin/id` denied); and both shapes built from a host bind mount. In each the file
  is denied under every name and its neighbour runs.
- A place may be hidden in the view by a later mount. Then a directory is expanded that did not
  need it, which fails closed.

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

A deny entry denies a path, and every mount of that path (D23, D25). It does not find every hard
link to a file. Decided by the stakeholder (agent:sdc), 2026-11-21, revisable before the wire
story publishes the property (§ 19 Q4): the limit is stated, and a deny entry whose file has more
than one link is not refused.

It does not find the same bytes in a layer directory of an overlay that the view mounts beside
the overlay either (L1, D26). That limit is new in the fourth revision and is not decided: the
document states it and specifies no refusal (§ 20).

## 9. Facts, admission and the probe

`linux::proof` reads only the capability snapshot today (`linux.rs:166`). The snapshot's
`confinement_exec_subpaths` is the published wire fact and stays `None` (§ 2), so the proof needs
its own input.

**D14. The driver keeps what its probe proved in a value of its own, `ExecListProof { subpaths,
denied_paths }`, and `linux::proof` takes it as a second argument.** The value is serialized into
`SnapshotMaterial` (`probe.rs:36-48`), so the snapshot digest binds it; no published fact changes.
`proof` is a public item, so `crates/substrate-host/tests/confinement-public-items.txt` is
regenerated in the first story too. Since the fifth revision the value has a third member,
`withheld`: the entries of D27, empty unless the open withheld the facts for a named cause.
`linux::proof` copies them into `AdapterFacts.withheld`.

**D15. Each fact is proved at open by one run in a throwaway sandbox**, with
`USER_NAMESPACE_ARGV` and the argv of `probe_workspace_scoped_write` (`probe.rs:358-391`), gated on
the floor (`exec`) and on a configured helper. The probe copies the host's `/usr/bin/true` and
`/usr/bin/ln` into a temporary workspace and runs `/bin/sh -c` through the helper, bound from the
sealed bytes of D20 and given the arguments of D24, as a start does.

| fact | list | the script, as the run's program | proof |
|---|---|---|---|
| `exec_subpaths` | `allowed` | `allowed/true` succeeds; `outside/true` fails; `/usr/bin/true` fails; `allowed/ln outside/true elsewhere/linked` succeeds | exit 0, and `elsewhere/linked` exists on the host |
| `exec_denied_paths` | `allowed`; the probe binds `allowed/sub` a second time at `/second` and denies `/second/true` | `allowed/true` succeeds; `/second/true` fails; `allowed/sub/true`, the same file under the listed name, fails; `allowed/sub/again`, a hard link to it, fails | exit 0, and `exec_subpaths` was proved |

The deny entry names the file only through the second mount, and that mount shows a part of the
listed directory. This is the shape the text of a path and the view's ancestors both get wrong
(K2, K5), so the probe proves D25's place handling and does not assume it.

Each half must hold: what the list allows happened, and what it refuses did not, as
`seatbelt::probe_build` requires (`seatbelt.rs:429-431`). The `ln` step proves the refer rule, so
an ABI 1 kernel fails it. The probe proves the kernel and the helper together; it never reads a
kernel version or an ABI number to decide.

**The preload half (sixth revision, D28).** `exec_subpaths` is proved only where a preload
named in the environment did not lift the list. The probe of that fact gains two observations,
and `exec_denied_paths` inherits them, because it is proved only where `exec_subpaths` was:

| run | how the variable is given | proof |
|---|---|---|
| the row's own run | `LD_PRELOAD`, naming a path in the probe's workspace at which no file exists, as a variable of the run: an `--env` operand of the helper, the way a start gives it (D28) | the row's proof, and the script finds the variable set to that value: what the run's environment names reached the program, and the list held |
| a second run, whose script links to a second name | the same variable set for the helper's own start, with bubblewrap's `--setenv`: what the implementation did before D28 | the helper executed nothing: exit status 125, and the second name does not exist on the host |

- **What this proves** is the helper's part of D28, with the helper binary this host was
  configured with: it takes the program's environment from its operands, and it refuses to
  start a program when a variable reached its own process. An embedder builds the helper
  itself (§ 6, *Embedder notes*), so this is not known before the open.
- **What it does not prove** is a constructor. The driver has no compiler and no shared object
  at open, and the path it names holds nothing: a loader that is given it reports that it
  cannot load it and goes on, so the probe discards the run's standard error. A proof with a
  real object needs one built for the host's loader and `libc`, which would be a second
  product of this repository beside the helper. That is not designed here (§ 20). The lane
  cases of § 16 use a compiled object.
- **It does not prove the driver's part**, that a start gives bubblewrap no `--setenv`. The
  probe builds its own bubblewrap arguments
  (`probe_exec_subpaths`, `crates/substrate-host/src/probe.rs:487` at `4b5208922`), not a
  start's. A unit test holds the start's argv (§ 16).
- The second run is one more sandbox per open. Its cost was not measured.
- This form of the half is a proposal. It follows from rule 5 of D28: with a helper that
  clears its environment and does not refuse, the second run would have nothing to observe.
- Seventh revision. The coordinator settled the form: the probe uses no compiled object, and
  `767fbea57` implements both observations.
- **D29 adds no half and no run.** That the helper names no loader is read from its bytes at
  the open (D21), before any probe; a helper that names one never reaches a probe. The probes
  build their own bubblewrap arguments with the helper directly after `--`, declare no file
  and take no terminal, so rule 3 of D29 holds in them as written. The second observation
  above stays as it is: a static helper reads its own environment as a dynamic one does.

`systemd` around `linux` copies the inner adapter's proved facts
(`crates/substrate-host/src/confinement/systemd.rs:576-599`), so it needs no change for the two
facts. It builds its own proof with `AdapterFacts::new`, which carries no withheld entry, so
since the fifth revision it copies the inner proof's `withheld` as well: one line (D27).

**No probe runs where the facts are withheld.** D27's third step ends the helper's part of the
open, so neither row above is run on a host whose file system is outside the list, and the
probes' temporary directory is placed before that decision.

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
| `config.exec-gate-invalid` | 422 | `HostConfig.exec_gate` is not absolute, or the file's place is beneath the workspace root or a declared writable root, or the file has more than one link (D21). Seventh revision: or its bytes are not an ELF program for the driver's machine without `PT_INTERP` (D29) | the open of `linux`, and of `systemd` around it: before the file is read, and for the last case on the bytes read; new |
| `session.pty-unserved` | 501 | seventh revision, proposed: a start with a list and a controlling terminal (D29). Unreachable today: no start has both | the start, before dispatch; existing code, new use, the implementor's to confirm |
| `config.exec-gate-unserved` | 501 | `HostConfig.exec_gate` is set and the file is missing, not a regular file, unreadable, over the size bound, or does not print the protocol line (D19); or the place of the helper's file, of the workspace root, of a declared writable root, of a base directory or of the probes' temporary directory cannot be established for one of seven causes: every cause of D26 but `file-system-unlisted` and `mount-id-unreported` (D27). The address is the field whose path it was | the open of `linux`, and of `systemd` around it; new |
| `exec.sandbox-place-unserved` | 501 | the place of the workspace, of the program, of an entry of either list, or of the host source of a declared mount cannot be established: one of the nine causes of D26, among them a file-system type outside `ext4`, `overlay` and `tmpfs` | the start of a profile with a list, before dispatch, on an adapter that proved the facts; never admission, never a start without a list; new |
| `confinement.fact-unserved` naming `exec_subpaths` | 501 | a non-empty `exec_subpaths` on an adapter that did not prove it: `linux` without `exec_gate`, `linux` whose probe did not hold, `linux` that withheld the facts at open (D27), and every adapter of § 13 that proves neither fact. In the withheld case the refusal carries the cause, the path, its address and the file-system type | admission; existing code, with one new field on its error (D27) |
| `confinement.fact-unserved` naming `exec_denied_paths` | 501 | a deny-list on an adapter that proved `exec_subpaths` and not this fact | admission; existing code, new fact |
| `exec.sandbox-path-unrepresentable` | 422 | an entry of either list that is not normal, holds a refused character, has a symbolic link in a mount classed `beneath`, resolves through a link into a writable mount (D24), or (allow entry on `linux`) does not exist; a program that is not absolute when a list is set, or that fails one of the two link rules. Sixth revision: on `linux`, a program, a loader, an entry of either list or a `--writable` root whose path in the view is longer than 4095 bytes (§ 5, E4) | admission for the lexical rule, the start for the rest, before dispatch; existing code, new on `linux` |
| `exec.sandbox-exec-subpath-undeclared` | 422 | an absolute entry of either list outside the eligible locations (§ 5) | the start; existing code, new on `linux` |
| `exec.sandbox-program-unlisted` | 422 | the program lies in a denied path, by place (D25), or is the same file as a denied file (D23) | the start, before dispatch; existing code, new use |

Every refusal of an admission or a start writes its audit record, as it does today
(`linux.rs:55-56`). A refused open has no profile and no sink, so it writes none; the embedder
gets the error from `open`. After dispatch there is no refusal: a helper failure is exit status
125, which a program can also return (§ 6). No row lets a profile with a list run without it
(D19). `exec.capability-stale` is not in the table: the helper's bytes are read once (D20).

**D28 adds no row.** No variable is refused by name, and `exec.environment-refused` keeps its
rule. A helper that finds a variable in its own environment exits 125 after dispatch; that is
a defect of its caller, not an answer to a client. The row of `config.exec-gate-unserved`
gains one case through its cause `path-unopenable`: a configured path that does not exist yet
and is written through `..` or `.` (D25, E5).

**D29 adds no code.** Seventh revision.

- The row of `config.exec-gate-invalid` gains one case: a helper whose bytes are not an ELF
  program for the driver's machine without `PT_INTERP` (D21).
- A start with a list and a controlling terminal is refused before dispatch with an existing
  code. This document proposes `session.pty-unserved` (501), address `mode`, a `DriverError` of
  class `Unserved`, with a message that names the list; the implementor confirms it or takes a
  new host code (§ 6, *Before the ruleset*). No client can make such a start today, so no
  client sees the code for it.
- No declared mount is refused for its path. A file at `/etc/ld.so.preload` on a start with a
  list is admitted, and the list holds (E8).

No row compares the text of two paths where a place could not be established (D26). The two
rows that name a place refuse, and where the open withheld the facts (D27) the row of
`confinement.fact-unserved` refuses every profile with a list.

A refused open writes no record, and an open that withholds the facts writes no audit record
either: there is no profile yet. What it leaves is the proof's `withheld` entries and the
driver's `warn` lines (D27).

## 13. Per adapter

| adapter | `exec_subpaths` | `exec_denied_paths` |
|---|---|---|
| `linux` | proved where the open did not withhold it (D27) and the probe held (story 1) | proved where the open did not withhold it and the probe held (story 2) |
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

D26's two refusals are raised at the open and at the start, on a mount table. The model has no
file system and no start, so no scenario states them; they are unit tests and one lane case
(§ 16). D27 adds nothing to the model either: an adapter that withheld the facts is, to the
model, an adapter that did not prove them, and `fact-unserved-exec-subpaths` is its outcome.
The cause is not a field of the specification's `FactUnserved`
(`spec/confinement/confinement.yaml:161-165`).

D28 adds nothing to the model either. The model has no environment and no start; the rule is
held by the lane cases and the argv tests of § 16. Nor does D29: the model has no helper file,
no declared file's content and no terminal.

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

D28 changes no wire start either. It applies to a start with a list, a wire start has none, and
a start without a list keeps its argv byte for byte. `exec.environment-refused` keeps its rule,
so no variable a wire client may set today is refused.

D29 changes no wire start. `validate_read_only_files` is untouched, so a wire client declares
the files it could declare before. A `pty` session is a wire start and has no list, so it
keeps its `setsid --ctty` stage and is not refused; `session.pty-unserved` is answered on the
wire exactly where it was.

None of the three defaults of § 19 (the empty-list difference, no `memfd_create` refusal, the
hard-link limit) reaches a wire property or release notes without being decided again or
confirmed when `story:exec-start-carries-executable-lists` is planned.

## 16. Tests and lanes

Each case runs its scenario once. A case named `limit_…` asserts a limit of § 5, § 6, § 7 or
§ 8.5 as it is, so a kernel or tool change that moves it is seen.

**Delegated lane** (`bash scripts/delegated-lane.sh`, `crates/substrate-host/tests/ess_confinement_linux.rs`).
The script builds `substrate-exec-gate` and names it in the lane's `HostConfig`; without it the
open step of every `linux` scenario fails on the missing fact and does not pass silently. On a
host that withholds the facts (D27) the same step fails the same way, and the scenario runner
prints the proof's `withheld` entries with the failure, so the lane's log names the path and the
file-system type.

**The lane's helper is static (seventh revision, D29).** At `767fbea57` the script builds the
helper for the default target (`scripts/delegated-lane.sh:55`), which is dynamically linked;
E6 and E7 ran with that helper. From this revision on:

- the script builds it with `--target x86_64-unknown-linux-musl`, the target
  `rust-toolchain.toml` pins, and names a copy of that file, as it names a copy today
  (`scripts/delegated-lane.sh:64-68`);
- before it exports `SUBSTRATE_EXEC_GATE` it checks the file it will name: the program
  headers hold no `INTERP`, the check `Dockerfile:124` makes for the executor;
- where the build fails, the target is not installed or the check finds a loader, the script
  refuses: it names what failed and every suite that therefore did not run, and exits 1, as
  it does for a missing prerequisite today. It never builds or names a default helper in its
  place. A lane that ran with a dynamic helper would be refused by the driver at the first
  open anyway (`config.exec-gate-invalid`); the script's own refusal says why before a suite
  starts;
- the hosted lane runs the same script (`.github/workflows/release-lanes.yml:122`). That its
  runner installs the pinned target with the toolchain is read from `rust-toolchain.toml`, not
  from a run (§ 18);
- the helper's own suites in `crates/substrate-exec-gate/tests` start the helper cargo built
  for them, with no driver around it. They may keep a default build: no profile and no view
  is involved there. The cases among them that state D28 or D29 through a driver are the
  host crate's.

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
- `limit_a_listed_directory_does_not_reach_through_a_second_mount_of_a_part_of_it`: a directory
  declared as the read-only root `/opt/t` and its subdirectory `bin` as `/opt/x`, `/opt/t`
  listed. `/opt/t/bin/tool` runs and `/opt/x/tool` fails with status 126 (K7's shape, built
  from declared roots so that it runs on every host);
- refused at the start, each with its audit record and nothing run: an entry with a symbolic link
  in the workspace, an absent entry, an entry under `/tmp`, a relative program;
- D24 and § 5: the workspace entry `bin`, replaced by a symbolic link to `/usr/bin` after the
  driver's check and before the helper opens it, ends the run with status 125, and nothing of
  `/usr/bin` runs. The case makes the swap through a test hook between the two steps;
- D24, the class comes from the host source. A read-only root whose host directory lies in the
  workspace root holds `link`, a symbolic link to `/usr/bin`, and the profile lists it: the start
  is refused `exec.sandbox-path-unrepresentable`, and nothing runs (K10);
- D24, a followed path into a writable mount. A read-only root outside every writable location
  holds a symbolic link into `/workspace`, and the profile lists it: the start is refused
  `exec.sandbox-path-unrepresentable`. The helper given the same arguments in a sandbox, without
  the driver's check, exits 125 with the line of D24 and executes nothing (K9);
- D20: the run cannot write or execute the helper in its view; the helper's file replaced on
  disk after the open changes no later start; two starts of one opened adapter both run under
  the helper (K3);
- D19 and D21, each with nothing run:
  - an adapter opened without `exec_gate`: a profile with a list is `confinement.fact-unserved`
    naming `exec_subpaths`, with its `profile_refused` record, and a profile without a list runs;
  - `exec_gate` naming a file in a workspace, a file beneath a declared writable root, a symbolic
    link outside both that points into a workspace, and a relative path: each open is
    `config.exec-gate-invalid`, status 422;
  - `workspace_root` configured through a symbolic link and `exec_gate` naming a file in it by
    its resolved path: the same refusal;
  - `exec_gate` naming a file outside every workspace that has a second hard link inside one:
    the same refusal, and the message names the link count (K4);
  - `exec_gate` naming a path that does not exist, a directory, and an unreadable file: each open
    is `config.exec-gate-unserved`, status 501, and the message names the path;
  - `exec_gate` naming a program that does not print the protocol line: the same refusal;
- **D27, through the driver, with no privilege.** The case opens an adapter of its own. Its
  `HostConfig` is the lane's with one more declared writable root, whose path is `/proc/sys`
  and which no start names; the driver checks only a root's name and that its path is absolute
  when it opens (`lib.rs:470-500`). With `exec_gate` set:
  - the adapter opens;
  - `facts()` proves neither `exec_subpaths` nor `exec_denied_paths`, and its `withheld` names
    `file-system-unlisted`, `config.writable_roots`, `/proc/sys` and `proc` for each;
  - a profile with a list is refused `confinement.fact-unserved`, its text holds `/proc/sys`
    and `proc`, its `profile_refused` record is written, and nothing runs;
  - a profile without a list is admitted and runs, with no helper in its argv;
  - `systemd` around that adapter reports the same entries (`ess_confinement_systemd`);
- **D28, through the driver. The cases the two reviews wrote** (§ 3.10), each started through
  `ConfinementAdapter` with a profile that lists `bin`. Each builds its object with the host's
  `cc` and prints that it is absent, and why, where there is none:
  - E1: `LD_PRELOAD=/workspace/pre.so` in the run's environment, the object's constructor
    executing the run's script in the first process that loads it. The run prints `0`, `126`,
    `126`, as the same start does without the variable;
  - E2: the constructor acts only in the helper's process. The unlisted program ends with
    status 126;
  - E3: the constructor acts only in a program before the helper. The same result;
  - the same three with `LD_LIBRARY_PATH` and `LD_AUDIT` are not separate cases: no name is
    special to the driver or the helper, and one name holds the rule;
  - the variables arrive: a run with a list and the variables `A=b`, `EMPTY=`, a value with a
    space, with `=` and one that is `--`, and with `PWD` set, prints each as given; a run with
    a list and no `PWD` in its environment has no `PWD`; a baseline name the start allows has
    its fixed value, and the run's value where the run set the same name;
  - the driver's bound: a run with a list and 64 variables of 4096 bytes each starts, and its
    program has all of them;
  - a terminal start with a list: the same E1 result, with `setsid` before the helper.
    Seventh revision: this case is gone. Such a start is refused before dispatch (D29), and
    the argv case below holds that;
  - a start without a list and with `LD_PRELOAD` set runs as it did before the story;
  - `limit_a_preload_object_runs_its_code_in_every_allowed_dynamic_program`: with D28 in
    place and the variable naming the object, the object's constructor writes a file from
    inside the allowed shell, and the unlisted program still ends with status 126;
- **D29, through the driver. Permanent cases of the lane**, each run once, each started
  through `ConfinementAdapter` with the lane's static helper:
  - **the measured case, as the stakeholder asked for it.** The profile lists `bin` and
    declares a read-only file mounted at `/etc/ld.so.preload` that names an object in the
    workspace. The object, on its first load, executes an unlisted base tool. The run prints
    what the control prints, the same start without the declared file: `0`, `126`, `126`. This
    is E6's case, `adversary2_a_read_only_ld_so_preload_runs_before_the_ruleset`. A refusal
    before dispatch is no longer one of its two passing answers: the start is admitted and
    the list holds;
  - **where the object was loaded.** E7's case,
    `security2_a_declared_file_at_the_loaders_preload_path_does_not_lift_the_list`: the object
    records every process that loads it. The list holds, and no recorded process is the
    helper;
  - `limit_a_declared_preload_file_is_read_by_every_allowed_dynamic_program`: the same
    profile. The record holds the allowed shell and the listed tool, and the unlisted tool
    ends with status 126 (E8). This is the limit of § 7, held as it is;
  - the same shape with the file declared at `/etc/ld.so.cache` is not a second case: the
    rule is about the helper's linking, not about a path, and one path holds it;
  - **a dynamically linked helper is refused at the open, as the stakeholder asked.** The
    case hands the driver a dynamically linked ELF program as `exec_gate`: a copy of the
    host's `/usr/bin/env`, in a directory outside every workspace, so that it has one link and
    passes the three checks of D21. The open is refused `config.exec-gate-invalid`, status
    422, address `config.exec_gate`, and the message names the loader. Portable: nothing is
    executed for the check. On a host whose `env` names no loader the case prints that it is
    absent and why. **It is held by a mutant that removes the check**: with the test of the
    bytes removed, the open accepts that helper and this case fails. The measured case above
    does not fail under that mutant, because the lane's helper is static; it fails when the
    lane is given a default helper, which is E6. The mutant is run once by the implementor or
    the adversary pass and recorded with the story's evidence. It is not a test, a script or
    a step of the gate (`AGENTS.md`, § *The gate*);
  - E10's case, a declared file at the helper's own mount: the list holds;
  - each of these builds its object with the host's `cc` and prints that it is absent, and
    why, where there is none. The dynamically linked helper case needs no compiler;
- **The path limit (§ 5, E4).** A read-only root whose mount path is longer than its host
  path, and an entry beneath it that the driver can open through the mount's descriptor and
  whose view path is longer than 4095 bytes: the start is refused
  `exec.sandbox-path-unrepresentable`, address `exec_subpaths`, with its audit record, and
  nothing runs. The same root with an entry whose view path is exactly 4095 bytes runs;
- **The root's spelling (D25, E5).** Portable: the check precedes every probe. `<ws>` holds a
  workspace with a file in it, `exec_gate` names that file, `workspace_root` is `<ws>/new/..`
  and `new` does not exist. The open is refused `config.exec-gate-unserved`, status 501,
  address `config.workspace_root`, the message names `path-unopenable`, and `<ws>/new` was not
  created. The same with `<ws>/new/.`. The control, the root written `<ws>`, is
  `config.exec-gate-invalid`. The security review's case expects that code for both
  spellings (§ 6, D21);
- `systemd` around `linux`: one run with a list (`ess_confinement_systemd`), and E1 once
  through it.

Story 2:

**The story's first failing test is K2 and K8 through the driver.** The stakeholder asked for
this on 2026-11-21: not only through the helper. The story therefore begins with two cases of
this lane that start a run through `ConfinementAdapter` and fail before any code of the story
exists:

- K2: a directory declared as the read-only root `/opt/t` and its subdirectory `bin` as
  `/opt/x`, the profile listing `/opt/t` and denying `/opt/x/tool`. The run's `execve` of
  `/opt/t/bin/tool` and of `/opt/x/tool` fails with status 126, and `/opt/t/bin/other` runs.
- K8: the same roots, the profile listing `/opt/t/bin` and denying `/opt/t`. With a program
  beneath `/opt/t` the start is refused `exec.sandbox-program-unlisted`. With a program
  elsewhere the run starts and its `execve` of `/opt/t/bin/other` fails with status 126.

The helper's own cases for the two shapes come after these.

- `/usr/bin` allowed, one program beneath it denied: another program in `/usr/bin` runs, the
  denied one fails inside the run (the story's acceptance);
- **D23, a program that is a link to a denied program. A permanent case of the lane**, not only a
  probe of this document. A read-only root holds a program `real` and a symbolic link `sh` to
  it, as `/usr/bin/sh` links to `bash` on the probe host. The profile lists the root and denies
  `real`.
  - With the program `sh`, the start is refused `exec.sandbox-program-unlisted` before dispatch,
    with its audit record, and nothing runs.
  - With another program of the root, the run starts, and its `execve` of `real` and of `sh`
    both fail with status 126.
  - A unit test gives the helper the same arguments without the driver's check: it writes no
    rule for the program and exits 126 (B2);
- D23, the loader: a deny entry that is the host's loader. A dynamically linked program exits 126
  and a static one runs (B3);
- **D23 and D25, a directory reachable under two names with the program denied under one. A
  permanent case of the lane**, in three forms, each run once:
  - the base view: `/usr/bin` listed and `/bin/<program>` denied, on a host whose `/bin` is a
    link into `/usr`. The program fails under both names and another program of `/usr/bin` runs
    (K5). On another host this form prints that it is absent and why;
  - two declared roots, on every host: one directory declared as the read-only roots `/opt/a`
    and `/opt/b`, `/opt/a` listed and `/opt/b/tool` denied. `tool` fails under both names and
    its neighbour runs;
  - a part of a listed directory: a directory declared as `/opt/t` and its subdirectory `bin` as
    `/opt/x`, `/opt/t` listed and `/opt/x/tool` denied. `/opt/t/bin/tool` and `/opt/x/tool` both
    fail and `/opt/t/bin/other` runs. This is the form the view's ancestors get wrong (K2);
- **D25, a host bind mount.** The lane has no privilege, so the case makes the mount in a user
  and mount namespace of its own (`unshare --user --map-root-user --mount`). Inside it one
  directory is bound at a second host path, bubblewrap builds a view from both paths, and
  `substrate-exec-gate` gets the first listed and the program denied under the second. The
  program fails under both names and its neighbour runs (K5). The case drives the helper, not
  the driver: a driver opened inside that namespace was not probed (§ 18). Where `unshare`
  cannot create the namespace, the case prints that it is absent and why;
- D11 step 1: a listed directory inside a denied one gets no rule (K8);
- D26, through the driver: the host's `/proc/sys` declared as a read-only root and listed. The
  start is refused `exec.sandbox-place-unserved` before dispatch, the message names `proc` and
  the three listed types, the audit record is written and nothing runs (U3 shows the helper's
  side);
- D26, the file systems: the lists of F1 to F7 run once with the workspace root on a tmpfs.
  The case mounts one inside `unshare --user --map-root-user --mount` and drives the helper, as
  the host bind mount case does. The overlay form of the same lists runs the same way;
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
    the program, after `env` where the implementor keeps that stage (D28), with one `--allow`
    per entry, and bind it with `--ro-bind-data`;
  - a start without a list renders the argv it rendered before;
  - the wire constructor is named at exactly the three wire sites: a source test in the form of
    `every_probe_sandbox_carries_the_user_namespace_posture` (`probe.rs:1660`) counts them, so a
    fourth site fails until this design is revisited. The test covers `process.rs` only. Outside
    that module the compiler refuses the call (D22), and no test stands in for it: a
    `compile_fail` documentation test cannot name a crate-private item for any reason, so it
    would pass without proving this.
- `process.rs` and `linux.rs`, D28, on the argv alone and with no sandbox:
  - `command` and `render` of a start with a list hold no `--setenv`, for a start with baseline
    names, variables of the run, a secret slot and a generated egress anchor;
  - every variable of those four groups is an `--env` operand of the helper, each name once,
    with the run's value where the run and the baseline name one variable;
  - between bubblewrap's `--` and the helper there is nothing, or `setsid --ctty` on a
    terminal start, or the `env -u PWD --` stage if the implementor keeps it; never an
    argument that holds a variable of the run. Seventh revision, D29: there is nothing, on
    every start with a list. The argument after `--` is the helper's mount, for a quiet
    start, a start with piped input and a start with live output;
  - **the terminal start (D29).** `command` given a list and the terminal channel returns the
    refusal and builds no argv: the proposed `session.pty-unserved`, class `Unserved`, address
    `mode`, with a message that names the list. Nothing is spawned. The same input without a
    list builds the argv it built before, with `setsid --ctty` after `--`. No lane case can
    make this start: no public start carries both a list and a terminal;
  - a run that set `PWD` has it as an operand and has no `env -- PWD=…` stage;
  - a start without a list renders, byte for byte, the argv it rendered before, `--setenv`
    groups and both `env` stages included. This is the case of D22 above, now with variables
    in its input;
- `lib.rs` and `process.rs`, the path limit: the check takes the text the helper will be
  given, so it needs no file. `<root>/<rel>` of 4095 bytes passes and of 4096 bytes is
  `exec.sandbox-path-unrepresentable`, for the program (address `argv`), an allow entry, a deny
  entry, the loader and a `--writable` root;
- the helper's own tests, D28:
  - the parse: `--env` with one operand, with an empty name, with a name that holds `=`, and
    the same name twice each exit 125 and execute nothing; a value that is `--`, that is
    `--env`, that is empty, and that holds `=` or a space is that value;
  - the helper started with an empty environment and two variables as operands, `A` and
    `LD_PRELOAD`, executes a program whose environment is exactly those two;
  - the helper started with `PWD` in its own environment and no operand executes a program
    with an empty environment;
  - the helper started with any other variable in its own environment executes nothing, exits
    125 and names the variable (rule 5 of D28);
  - the security review's case in `crates/substrate-exec-gate/tests/security_gate.rs`: the
    helper started as the driver starts it, `LD_PRELOAD` naming a compiled object among the
    operands, holds the list: the unlisted program ends with status 126;
- the place function, D25 and D26, sixth revision, each with no mount and no privilege:
  - a path whose nearest existing ancestor is followed by `new/..`: `path-unopenable`. The
    same for `new/.`, for `..` as the only remaining name, and for `a/../b`;
  - the control: the same ancestor followed by `new` and by `new/deeper` has the place of the
    ancestor joined with those names;
  - a path that exists and is written through `..` is opened, and its place is the place of
    what it resolves to;
  - a table file that holds one row whose mount point has the byte `0xFF`:
    `mount-table-unparsable`, and never `mount-table-unreadable`. The reader is given the file
    by path, as in the two cases outside the function below;
- the helper's own tests, D24 and D25: each malformed argument list exits 125 and executes
  nothing; `--alias` is an unknown option; a `beneath` entry with a symbolic link in it, or one
  that leaves its root, is refused; the place of a path is computed from a mount table given as
  text, for one directory at two mount points, for a part of a directory at a second one, for a
  path that does not exist, and for a mount point with an escaped space.
- **D26, one unit test per cause, each on a mount table written in the test.** The place
  function takes the reported mount ID, the descriptor's path and the table's text, so no test
  needs a sandbox, a mount or a privilege. The function is the one source file the helper and
  the driver both compile (D26), so each test covers both:
  - `mount-table-unparsable`: a row without the ` - ` separator; a row with three fields; a
    mount point with the escape `\04`;
  - `mount-id-unreported`: no mount ID given;
  - `mount-row-missing`: an ID no row has;
  - `mount-row-repeated`: two rows with the ID;
  - `mount-point-mismatch`: the row's mount point is `/opt/a` and the descriptor's path is
    `/opt/b/tool`; and a descriptor path that could not be read;
  - `mount-root-deleted`: a root of `/tree//deleted`;
  - `file-system-unlisted`: one row each of type `btrfs`, `xfs`, `nfs4`, `fuse.sshfs`, `proc`
    and `devtmpfs`, and the error carries the type; a row each of `ext4`, `overlay` and `tmpfs`
    has a place;
  - `mount-table-unreadable` and `path-unopenable` are the two causes outside the function:
    the reader is given a path that does not exist as its table, and a path beneath a
    directory of mode 000 as its subject;
  - stacked mounts: two rows with one mount point and two IDs; the place is the root of the
    row whose ID was given, for each of the two IDs (S1);
  - a hidden mount: the rows of S2; with the ID of `hold` the place lies in the second mount's
    root, and the row of `hold/sub` is never chosen;
  - no fallback: for each cause above, the comparison "lies in" returns the cause and never a
    boolean, so a caller cannot read a failure as "outside".
- `lib.rs` and `process.rs`, D26: a start whose declared root has a row of type `xfs` is
  `exec.sandbox-place-unserved`, status 501, address `read_only_roots`, on a table given as
  text; the same open check with a table in which the workspace root's row has a root ending
  in `//deleted` is `config.exec-gate-unserved`, status 501, address `config.workspace_root`,
  and so is each of the other six refusing causes.
- **D27, the open that withholds, each on a table given as text and with no sandbox:**
  - the workspace root's row has the type `xfs`: the open's decision is "withheld", not an
    error. Both facts are absent from `ExecListProof`, and its `withheld` has two entries, one
    per fact, each with the cause `file-system-unlisted`, the address `config.workspace_root`,
    the root's path and the type `xfs`;
  - the same for the helper's own file, for a `writable_roots` path, for `/usr`, and for the
    probes' temporary directory, each with its address; and for `btrfs`, `nfs4` and
    `fuse.sshfs` in place of `xfs`;
  - no mount ID reported: withheld with the cause `mount-id-unreported` and no type;
  - two paths on types outside the list: four entries, in the open's order;
  - one path with a withholding cause and another with a refusing one: the open is refused;
  - a withheld open never reaches the trust check: the decision function is given a helper
    with two links and a table that withholds, and answers "withheld" and not
    `config.exec-gate-invalid`;
  - `linux::proof` given that `ExecListProof`: `facts` lacks `exec_subpaths` and
    `exec_denied_paths`, and `withheld` holds the entries. Serialized, a proof with no entry
    has no `withheld` member, so every existing serialized proof is unchanged;
  - `systemd`'s proof around such an inner proof carries the same entries.
- `mod.rs`, D27, the refusal repeats the cause:
  - a profile with `exec_subpaths` admitted against that proof is `FactUnserved` naming
    `exec_subpaths`, with `withheld` set to the proof's first entry for that fact. Its code is
    `confinement.fact-unserved` and its status 501, and its text holds the cause, the path,
    the address and `xfs`;
  - a profile with both lists names `exec_subpaths` first, as the `Fact` order has it;
  - a profile without a list is admitted against the same proof, and renders the argv it
    rendered before;
  - the `profile_refused` record of the first case has its code and its fact and no new
    member;
  - against a proof with no entry, `FactUnserved` has `withheld: None` and the text it has
    today, for `declared_insecure` at open (`mod.rs:191`) and for a fact at admission.
- `probe.rs`: `the_published_set_is_the_linux_adapters_proof` with its named exception;
  `every_probe_sandbox_carries_the_user_namespace_posture` counts the new probes;
  an unset helper, and one that exits without restricting, each withhold both facts. Sixth
  revision, the preload half of D15: a helper that does not hand the operand's variable to the
  program, and one that starts the program although a variable reached its own process, each
  leave `exec_subpaths` absent. Each needs a helper that restricts and has that one defect;
  how the test gets one is the implementor's.
- `lib.rs`: the open checks of D19 and D21 decide on a path, a mount table, metadata (the link
  count among it) and a protocol line they are given, as `tmp_size_served_by` decides on help
  text (`lib.rs:409-427`), so they need no sandbox and run on every Linux host. The class of a
  mount (D24) is decided the same way, from host sources and a mount table given as text: a
  source inside the workspace root, a source that holds it, and a source beside it.
- **The `PT_INTERP` refusal (D29), on bytes written in the test, with no file and no
  sandbox.** The function takes the helper's bytes and answers one of three values:
  - no loader: a 64-bit little-endian image for the host's machine with a `PT_LOAD` header
    only; the same with a `PT_DYNAMIC` header and no `PT_INTERP`, the static
    position-independent form, which passes;
  - a loader: an image with a `PT_INTERP` header, for a path that is absolute, for one that
    is relative, for one that is not normal, for one that is not UTF-8 and for an empty one.
    Each is refused. These are the forms for which the reader of D7 answers "none";
  - not established: bytes that begin `#!/bin/sh`; an empty file; an ELF header cut short; a
    program header table that ends beyond the bytes; a header count that is the escape value
    `0xffff`; an image of the other class, of the other byte order and for another machine.
    Each is refused;
  - the open's decision function given each of the three answers:
    `config.exec-gate-invalid`, status 422, address `config.exec_gate`, for the second and
    the third, with a message that tells them apart; no error for the first;
  - the order: a helper with a loader that also lies in a workspace is refused for the
    workspace, and one on a host that withholds the facts is not refused at all (D27).
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
| D3 | on `linux` entries are view paths: relative beneath `/workspace`, absolute in the base system directories and declared mounts; they must exist; no symbolic link beneath a writable location; the program is absolute. Third revision: which mounts count as writable is D24's, a link elsewhere must not resolve into one, and a listed directory does not reach through a second mount of a part of it (K7). Sixth revision: a path the helper is given that is longer than 4095 bytes in the view is `exec.sandbox-path-unrepresentable` (422) before dispatch (E4) |
| D4 | Landlock, applied inside the sandbox by a helper that is the last program before the run's. Sixth revision: "the run cannot remove it" holds only with D28. Seventh revision: only with D28 and D29 |
| D5 | the helper is `substrate-exec-gate`, a binary of this repository, named by `HostConfig.exec_gate`, absent from a start without a list. Decided by session `sdc`, 2026-11-21, `approval-record:sdc-decides-plan-and-design-2026-11-21`. Changed in the second revision: how it is bound is D20, its arguments are D24. Sixth revision: the programs before it start with none of the run's variables (D28). Seventh revision: it is statically linked, and no program runs before it on a start with a list (D29) |
| D6 | the ruleset handles execute and refer only; refer is granted on `/`. Changed: the program's and the loader's rules go through D23 |
| D7 | the host's loader, read at open from `/usr/bin/env`, is granted; never a loader read from the run's program |
| D8 | the fact needs Landlock ABI 2; the probe decides, never a version number |
| D9 | the list restricts `execve` targets, not the code an allowed program runs: interpreters, the loader, a memfd and mapped code are stated limits. No `memfd_create` refusal: decided by the stakeholder (agent:sdc), 2026-11-21, revisable before the wire story publishes the property (Q3). Sixth revision: a fifth stated limit, a preload object inside the domain. Seventh revision: the limit is the loader's, for `/etc/ld.so.preload` as for the environment, and it is observed (E8) |
| D10 | `exec_denied_paths`, fact `exec_denied_paths` after `exec_subpaths`, at most 64 entries, a denied path wins. Changed: a denied program is refused by what it resolves to, not only as written (D23); the bound limits the profile, not the start's cost |
| D11 | `linux` denies by expanding a listed directory into per-entry rules; the expansion fails closed, and a program replaced in an expanded directory loses the right. Another hard link elsewhere beneath a listed path still runs: decided by the stakeholder (agent:sdc), 2026-11-21, revisable before the wire story publishes the property (Q4). Third revision: the three steps decide "lies in" by place (D25), and a listed path inside a denied one gets no rule (K8) |
| D12 | `macos` denies with `process-exec` and `file-map-executable` deny rules after every allow |
| D13 | a deny-list without an allow-list is `confinement.exec-denied-paths-without-subpaths`, 422, on every adapter, before any fact is read |
| D14 | the probe's result is a host-private `ExecListProof`, a second argument of `linux::proof`. Fifth revision: it also holds the `withheld` entries of D27 |
| D15 | each fact is proved by one throwaway run in which the allowed half happened and the refused half did not. Changed: the deny probe includes a second mount. Third revision: the deny entry names the file only through that mount, which shows a part of the listed directory. Sixth revision: `exec_subpaths` is proved only where a variable given as an operand reached the program and a helper started with that variable in its own environment executed nothing (D28); a proposal, with no compiled object at open |
| D16 | `confinement.exec-subpaths` stays unpublished, and the probe test names that exception |
| D17 | a denied execution is `EACCES` inside the run |
| D18 | no adapter observes or records a denied execution; § 11 lists what could, and designs none of it |
| D19 | a profile with a list never runs without it: no `exec_gate` is `confinement.fact-unserved` (501) at admission; a set `exec_gate` that cannot be used is `config.exec-gate-unserved` (501) at open. Decided with D5. Changed: a file changed after the open has no effect (D20), so `exec.capability-stale` is gone from this row. Fifth revision: a file system outside D26's list is not such an open error; the adapter opens with the facts withheld and the cause carried (D27). Sixth revision: the first clause is about the open and admission; for an admitted start it holds only with D28. Seventh revision: only with D28 and D29; a helper that names a loader is `config.exec-gate-invalid` at open |
| D20 | the driver reads the helper once at open through one descriptor, seals and hashes the bytes, and binds those bytes into every view with `--perms 0555 --ro-bind-data`; no name is resolved at a start. Third revision: `F_SEAL_SEAL` is added; each start opens the sealed memfd anew (K3); the two options are cited in the 0.11.2 source. Seventh revision: the bytes read are tested for `PT_INTERP` before they are sealed, so what is tested is what runs (D29) |
| D21 | an `exec_gate` that is not absolute, whose place lies in the workspace root or a declared writable root, or that has more than one link, is `config.exec-gate-invalid` (422) at open. Third revision: both sides are resolved and compared by place (D25), and the link count is new (K4). Fifth revision: where the facts are withheld (D27) the trust check is not reached, because the helper is never used. Seventh revision: a fourth check with the same code, a helper whose bytes are not an ELF program for the driver's machine without `PT_INTERP` (D29) |
| D22 | the lists are a required argument of a type without `Default`, not a field of `AdapterStart`; the three wire sites write `ExecLists::wire()`. Third revision: `wire` is private to the module `crate::process`, so the adapter's modules cannot call it |
| D23 | a denied file gets no execute rule from any source: an entry, the program, the loader, or a second mount of its directory; a denied program is `exec.sandbox-program-unlisted` (422) before dispatch. Third revision: the second mount is found by place (D25), and `--alias` is gone |
| D24 | the helper's arguments: each path is a class (`follow` or `beneath`), a mount root and a relative path. Third revision: the class comes from the mount's host source, not its mode in this run; `--writable` names the mounts the run can write; a `follow` path that resolves into one is `exec.sandbox-path-unrepresentable` (422) before dispatch and exit 125 in the helper. Sixth revision: `--env <name> <value>` carries the program's environment (D28); the form is a proposal |
| D25 | new in the third revision. Whether one path lies in another is decided by place: the device and root of its mount in `/proc/self/mountinfo`, joined with the rest of the path. The helper reads the view's table, the driver its own. The view-ancestor rule the review proposed is not taken (K2). Accepted by the stakeholder on 2026-11-21 with two conditions (D26, § 3.9). Fourth revision: the mount is the row with the mount ID the kernel reports for the opened path (`statx`, `STATX_MNT_ID`), not the longest matching mount point (S2). Sixth revision: a path that does not exist has no place where a name after its nearest existing ancestor is `..` or `.`; the cause is `path-unopenable` (E5) |
| D26 | new in the fourth revision, the stakeholder's first condition. Where a place cannot be established, nothing compares path text: nine named causes, one of them a file-system type outside the closed list `ext4`, `overlay`, `tmpfs`. A start is refused `exec.sandbox-place-unserved` (501, new) before dispatch; the open is refused `config.exec-gate-unserved` (501, existing) with the cause; the helper exits 125 with one line. `btrfs`, `xfs` and every network file system are outside the list and refused by name. Accepted by the stakeholder on 2026-11-21, the mount-ID change included. Fifth revision: at the open, `file-system-unlisted` and `mount-id-unreported` withhold the facts and no longer refuse the open (D27); the other seven causes refuse it as before. Sixth revision: a table that was read and is not UTF-8 is `mount-table-unparsable`, and `path-unopenable` gains the case of D25 |
| D27 | new in the fifth revision, decided by the stakeholder (agent:sdc) on 2026-11-21: "B now, C before the wire story". With a helper configured and a path of the open on a file-system type outside the list, the adapter opens, both facts stay absent, and a profile with a list is refused `confinement.fact-unserved` (501). The cause, path, address and type are in `ExecListProof`, in `AdapterFacts.withheld`, in the `FactUnserved` error and in one `warn` line; the audit record and the wire do not change. Which paths and causes withhold is a reading: every path the open places (the coordinator's), and `mount-id-unreported` beside `file-system-unlisted` (this document's). C: `xfs` is probed in the quota fixture's container before the wire story is planned and enters the list only if every case holds; `btrfs` stays refused by name. Sixth revision: its sentence "nothing ever runs with a list that is not enforced" holds for an admitted start only with D28 |
| D28 | new in the sixth revision, decided by the coordinator of the request on 2026-11-21 after the adversary pass and the security review of story 1's implementation (E1 to E3); not yet read by the stakeholder. On a start with a list, no variable of the run reaches a process that runs before `landlock_restrict_self` returned: bubblewrap gets no `--setenv` of the run's variables, no stage before the helper starts with one, the driver hands the variables to the helper as operands, and the helper gives them only to the `execve` of the program. No variable is refused by name; a start without a list keeps its argv. This document's additions, each a proposal: no `--setenv` at all on such a start; the helper refuses to run when a variable other than bubblewrap's `PWD` reached its own process, where the decision said it clears them; the operand form `--env <name> <value>`. What remains is a stated limit (§ 7): inside the domain a preload object loads into every allowed dynamic program and can run code, and `execve` of an unlisted path still fails. Seventh revision: implemented at `767fbea57`; the coordinator settled the three additions as written (no `--setenv`, the helper refuses, `--env`), and the cases of E1 to E3 pass there |
| D29 | new in the seventh revision, decided by the coordinator of the request on 2026-11-21 after the second adversary pass and the second security look (E6, E7). The stakeholder (agent:sdc): "No objection to D29: static helper, the driver refuses a helper that needs a dynamic loader, and a listed start with a controlling terminal is refused before dispatch until the helper takes the terminal itself." The helper is statically linked: at the open the driver tests the bytes it read through the one descriptor, the bytes it then seals and binds, and refuses `config.exec-gate-invalid` (422) unless they are an ELF program with no `PT_INTERP`. The rule is "no `PT_INTERP`", not "no `PT_DYNAMIC`", so a static position-independent helper passes (the stakeholder's first point). No list of loader files is kept and no declared mount is refused for its path. On a start with a list the helper is the first program bubblewrap starts in the view; a start with a list and a controlling terminal is refused before dispatch, with `session.pty-unserved` (501) proposed and the implementor's to confirm. The lane builds the helper static and refuses by name where it cannot. This document's additions, each a reading: bytes that are not an ELF image for the driver's machine, or whose headers cannot be read, are refused too. With D28: before `landlock_restrict_self` returns, the processes in the view are bubblewrap's own and the static helper, and nothing the run or the profile names is loaded as code. The limit inside the domain is unchanged (§ 7) |

## 18. Inferred, not observed

- Everything about `macos` in D2, D12, § 8.5 and § 10: no macOS host ran a probe. The `macos-15`
  lane cases of § 16 decide each.
- The helper's `openat2` with `RESOLVE_BENEATH` and `RESOLVE_NO_SYMLINKS` (§ 5, D24) ran in the
  probe helper for three paths (K10). The swap between the driver's check and the helper's open
  was not staged; § 16 has that case.
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
- D25 and D26: the place rule ran in the probe helper on ext4, on tmpfs, and on overlayfs
  mounted two ways (§ 3.9), all on one host and one kernel. `btrfs`, `xfs` and the network
  file systems were not run; D26 refuses them by name, so nothing rests on how they behave.
- D26, what was not run: an overlay whose layers lie on more than one file system, where
  `st_dev` of a lower file can differ from the device of the row; an overlay with `index=on`;
  a container runtime other than Docker 29.8.2 with `overlay2`. That the hosted runners'
  Docker, and containerd on the kind nodes, give the daemon an `overlay` root is inferred from
  those being the usual defaults; no file of the repository says it. That the runner VM's `/`
  and temporary directory are ext4 is inferred the same way.
- D26, the causes: `mount-table-unreadable`, `mount-table-unparsable`, `path-unopenable`,
  `mount-id-unreported`, `mount-row-missing`, `mount-row-repeated` and `mount-point-mismatch`
  are specified and were not provoked. The probe helper implements them; only
  `mount-root-deleted` (S3) and `file-system-unlisted` (U1 to U3) were observed.
- D27 is read from the code and not built. The fields of `FactWithheld`, the field on
  `AdapterFacts`, the field on `FactUnserved` and the count of places that change are a
  proposal from the files cited; nothing was compiled, and the public-items snapshot was not
  regenerated.
- D27, the split of causes. That only `file-system-unlisted` and `mount-id-unreported` should
  withhold, and that every path of the open counts, are readings of the stakeholder's sentence,
  which names the workspace volume. That a kernel without `STATX_MNT_ID` has no Landlock is
  from the two manual pages; no such kernel was run. That `xfs`, `btrfs` and the network file
  systems report a mount ID is inferred from `statx(2)`; U2 and U3 observed it for `proc` and
  `devtmpfs`.
- D27, the open's `warn` line: that a `warn` line of the driver reaches an in-process
  embedder's operator depends on the embedder's subscriber. The daemon's default filter was
  read (`main.rs:602-605`); no line was written.
- D27, the lane case: that the driver places `/proc/sys` as type `proc` when it is a declared
  writable root was not run through the driver. U3 ran the helper on a bind of `/proc/sys`.
- D27, `TMPDIR`: that `tempfile::tempdir()` follows `TMPDIR` is that crate's and the standard
  library's documented behaviour, not a run.
- D27, C: that the quota fixture's container can mount an `xfs` image once `xfsprogs` is in a
  recipe. The recipe was read; nothing was built.
- D27, the chart sentence: where it belongs was read from the guide and the chart's files. That
  it lands with the wire story is this document's reading.
- D26, one source file for the helper and the driver: how the file is shared between the two
  crates is the story's. Nothing was built.
- D10: the cost of a start with places. C1 measured an implementation that does more work than
  the one specified; one `statx` per entry is expected to cost about what W12 measured.
- That nothing but a hard link, or a layer directory of an overlay (L1), gives a file's bytes a
  second place is an argument, not an observation.
- D25, the driver's side: reading its own mount table, resolving a `follow` entry as the view
  will, and comparing the program with a deny entry before dispatch are not prototyped. K6 read
  host places with the probe helper, not with driver code.
- D24: the class of a mount from its host source, the `--writable` option and the driver's
  refusal of a `follow` path into a writable mount are proposals. K9 and K10 ran the checks in
  the probe helper. That `/tmp`, `/dev` and `/scratch` need a `--writable` each is caution, not
  a probed gap.
- D21: the three checks are proposals, not driver code. K4 shows the hard link, its count, and
  that its path is outside the workspace; K6 shows a second host path of one directory.
- § 16, the host bind mount case: K5 ran bubblewrap and the probe helper inside
  `unshare -Urm`, with `--disable-userns`. A driver opened inside that namespace, with its
  cgroup and uid handling, was not run, so the lane case drives the helper only.
- D22: `ExecLists` and its two constructors are a proposal; the three sites and the adapter's
  literal were read (`process.rs:816`, `:848`, `:958`, `lib.rs:1247-1252`). That a private
  function of `crate::process` cannot be named from `crate::confinement::linux` or the crate
  root is Rust's privacy rule, not a compiled check: no code was written.
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
- The sixth revision as a whole: nothing was run or built for it. E1 to E5 are the two
  reviews' cases and logs, read; this revision did not run them. The code was read at
  `4b5208922` with `git show`, because the working tree was changing.
- D28, `PWD`: that bubblewrap sets `PWD` after `--clearenv` is from the comment at
  `process.rs:3040-3041` and the stage it explains; bubblewrap's source was not read for it.
  That no loader acts on `PWD` is from `ld.so(8)` as remembered, not read in this revision.
  That bubblewrap sets no other variable was not checked.
- D28, the size of a start: that argument and environment strings count against one limit is
  from `execve(2)`. No start with 64 variables of 4096 bytes was run through the helper.
- D28, `systemd` around `linux`: that its starts go through the same argv is read from
  `systemd.rs:704-712`, where the adapter calls the inner adapter's `start`. Not run.
- D15, the preload half: that a loader given a preload path at which nothing
  exists reports it and goes on is glibc's documented behaviour, not run here; musl's was not
  looked up. The cost of the second sandbox per open was not measured.
- § 5, the path limit: 4095 bytes is `PATH_MAX` less its terminator. E4 shows a path above it
  refused by the kernel (`ENAMETOOLONG`). That a path of exactly 4095 bytes opens and executes
  through bubblewrap and the helper was not staged; § 16 has the case.
- D25, `..` and `.`: the rule is read from E5 and from `place.rs:402-437`. That
  `create_dir_all` on `<ws>/new/..` creates `<ws>/new` is from E5's case. A dangling symbolic
  link among the remaining names is still joined as text. No route from that to a wrong
  answer was found: the driver's own `create_dir_all` fails on such a root, and nothing can
  exist beneath a target that does not. That is an argument, not a run.
- D26, a table that is not UTF-8: that the kernel writes such a byte of a mount point
  unescaped is from `fs/proc_namespace.c` as remembered. No such mount was made, and the
  implementation's answer was read from `place.rs:261`, not provoked.
- The seventh revision as a whole: nothing was run or built for it, except two reads of the
  probe host: `file` on its `bwrap` and `setsid`, and a listing of
  `/proc/sys/fs/binfmt_misc`. E6 to E12 are the second reviews' cases and logs, read. The code
  was read at `767fbea57` with `git show`. Four items of the sixth revision left this list:
  they are measured (E6 to E9, § 3.11) or settled by the coordinator.
- D29 is not implemented. That a helper with `PT_INTERP` is refused at the open, that the
  lane builds a static helper, and that a start with a list and a terminal is refused are
  this document's text. E8 ran a static helper against the driver of `767fbea57`, which has
  none of the three.
- D29, rule 1: that a static position-independent helper has `PT_DYNAMIC` and no `PT_INTERP`
  is from `file` calling E8's helper `static-pie linked`; its program headers were not
  printed. That the reader of D7 answers "none" for a `PT_INTERP` it cannot use is read from
  `crates/substrate-exec-gate/src/lib.rs:359-386`, not run.
- D29, rule 1, the three tests beyond "no `PT_INTERP`" (an ELF image, for the driver's
  machine, with a table read whole): this document's reading of the stakeholder's point, not
  its words. The reason given for the machine test, a `binfmt_misc` handler, is an argument.
- D29, the sentence of § 6, *Before the ruleset*. Each of these is read or argued, and none
  was run:
  - that bubblewrap maps nothing once it is inside the view and before it executes the
    helper. Its source was not read for this;
  - that bubblewrap executes an absolute first argument without a search;
  - that the kernel starts a static ELF program with no other program on a host with a
    `binfmt_misc` handler registered. The probe host has none;
  - that neither the helper's code nor the Rust runtime's start calls a `libc` function that
    maps a library. No trace of the helper was taken;
  - that the helper reads none of the descriptors it inherits, read from its source;
  - that the stages `systemd` adds run outside the view and are not reached by a declared
    file;
  - that the kernel resolves a relative `PT_INTERP` from the working directory.
- D29, why not the narrower correction: that `/etc/ld.so.cache` and a file mounted over a
  library reach a dynamically linked helper the way `/etc/ld.so.preload` did was not run.
  Only the preload file was (E6, E7). That musl's loader reads a path file under `/etc` is
  from its documentation as remembered.
- D29, rule 4: `session.pty-unserved` is a proposal from reading § 12 and the wire crate.
  That no start has both a list and a terminal is read from the three wire sites
  (`process.rs:931`, `:967`, `:1078`) and relayed from the reviews. Nothing was built.
- D29, the lane: that the hosted runner has the `x86_64-unknown-linux-musl` target because
  `rust-toolchain.toml` pins it, and that the helper links for it there without a C tool
  chain for musl. On the probe host that build succeeded (E8). The `Dockerfile` installs
  `musl-tools` for the executor's build (`Dockerfile:113`); whether the helper needs them was
  not checked.
- § 16, the dynamically linked helper case: that a copy of the host's `/usr/bin/env` passes
  the three checks of D21 and reaches the test of the bytes was not run.

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
- An entry that admits its place under every mount of the view. Today a listed directory above
  the root of a second mount does not reach through that mount (K7); the helper could add a rule
  for each such mount, through the deny check, with the table it already reads. The stories do
  not ask for it, and the limit is stated (§ 5).
- Whether `xfs` enters D26's list. That is no longer open-ended: D27's C obliges the probe
  before `story:exec-start-carries-executable-lists` is planned, and says what its result
  decides. Until then a workspace volume on `xfs` opens without the lists and says why.
  `btrfs` stays refused by name; a probe of it would start with the subvolume case.
- The cause of a withheld fact on the wire, and in the daemon's own startup output:
  `story:exec-start-carries-executable-lists` (D27).
- The cause in the audit record. `ProfileRefused` keeps its shape (D27).
- A refusal for a view that mounts an overlay and one of its layer directories (L1). The row
  of an overlay names its layer directories in its options; comparing them with the places of
  the other mounts is not designed.
- A refusal for a read-only root that is declared inside the workspace root. D24 classes it
  `beneath`; whether such a root should be admitted at all is a question for the roots' own
  design (ADR 0010).
- `oci` and `pod`, and the wire field: other stories of the epic.
- A bound on `exec_subpaths`.
- A refusal of loader variables by name, on a start with a list or on every start. D28 decides
  against a list of names for these stories. Whether `LD_PRELOAD` in a run's environment
  should be refused at all is a question for the environment rule's own design; it would
  change starts without a list too.
- A start without a list. Its stages before the program start with the run's variables, as
  before these stories. No ruleset exists there, so D28 has nothing to protect; whether the
  same order is wanted for other reasons is not asked here.
- A probe at open with a real preload object (§ 9). It needs an object built for the host's
  loader, a second product beside the helper.
- A mount table that is not UTF-8 refuses every open with `exec_gate` set and every start with
  a list, for one mount with such a name anywhere on the host. A reader that takes the table
  as bytes and fails only for the row of the path it places is not designed.
- D28 on `oci` and `pod`: their story, with the lists themselves. D29 goes with it.
- A helper that takes the controlling terminal itself, so that a start with a list can have
  one (D29, rule 4). It is a session and one `ioctl` before the ruleset, with `libc` alone.
  No start needs it today. The story that gives a session start a list designs it, and
  decides the refusal's code on the wire with it.
- A refusal of declared mounts at paths a loader reads (`/etc/ld.so.preload`,
  `/etc/ld.so.cache`), on a start with a list or on every start. D29 decides against it for
  these stories: the helper needs no such rule, and inside the domain it is the stated limit.
  Whether a profile should be able to declare such a file at all is a question for the
  read-only files' own design (ADR 0033); it would change starts without a list too.
- A static bubblewrap, or a check of how `HostConfig.bubblewrap` is linked. Bubblewrap's
  loader runs on the host, before the view exists and with a cleared environment, so the
  route of E6 does not reach it. What it does inside the view before it executes the helper
  is on the list of § 18.
- A check for a `binfmt_misc` handler on the host. Registering one needs root on the host.
