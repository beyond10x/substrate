---
format: aep.planning-md/3
id: review-result:linux-executable-allow-list-security
kind: review-result
status: active
title: 'Security review, three rounds: the executable allow-list on linux'
tags:
- security-review
relations:
- reviews: story:linux-executable-allow-list
revision: 1
---
# Security review of `story:linux-executable-allow-list`, three rounds

Reviewer: the `aep:security-reviewer` role, run three times on 2026-11-21, each on the commit
before it. Verdicts: round 1 NEEDS-CHANGE, round 2 NEEDS-CHANGE, round 3 CONFIRMED.

## Rounds 1 and 2, summarized by the coordinating session

Round 1, on `4b5208922`:

- Finding 1, blocker: the run's environment is set before `env` and the helper start, so an
  `LD_PRELOAD` the command names ran an object before `landlock_restrict_self` (three red cases).
  A static helper alone did not close it. Fixed in `767fbea57` by design 35's D28.
- Finding 2: the helper's trust check placed `workspace_root` before the driver created it and
  joined missing names as text, `..` included. Fixed in `767fbea57`: `path-unopenable`, the open
  refused `config.exec-gate-unserved`.
- Finding 3: `HostDriver::exec_list_proof` was public outside the public-items snapshot. Fixed in
  `767fbea57`: `pub(crate)`.
- Held by execution: every Landlock call failing ends the run 125 with nothing executed; the
  floor's argv and posture are unchanged; the helper's bytes cannot be written, replaced or
  executed from the run; a host without Landlock proves no fact; D27; the daemon publishes
  nothing new.

Round 2, on `767fbea57`:

- D28 holds for variables on every start path reached.
- Finding, blocker: a read-only file declared at `/etc/ld.so.preload` lifted the list through the
  dynamic helper's loader; green with a static helper. Fixed in `1d961b30f` by D29.

Round 3's three notes below were taken in `a84b27edf`: the byte test refuses an image the kernel
would not start, a unit case holds the start-site terminal refusal, and the doc comment is back
on its function.

## Round 3, the reviewer's report

Verbatim, except that machine-local paths are replaced by `<scratch>` and `<worktree>`.

```
unit: story:linux-executable-allow-list, third look at 1d961b30f (on 767fbea57)
verdict: CONFIRMED
cases: executed 689→692, red 0
origin: introduced 3 notes, pre-existing 0, undecided 0
wrote-outside-worktree: yes, scratch only (section 6)
needs-coordinator: yes (three notes, each a decision rather than a defect)
```

D29 holds on this host for everything I could execute. One part of item 1 is held by a different step than the byte test, and one of the two terminal refusals is reached by no case.

## 1. `git --no-pager diff --stat`

```
 crates/substrate-exec-gate/README.md               |  7 +++
 crates/substrate-exec-gate/tests/gate.rs           | 58 +++++++++++++++++--
 .../substrate-host/tests/adversary_exec_lists.rs   | 66 ++++++++++++++++++++++
 scripts/delegated-lane.sh                          |  1 +
 4 files changed, 128 insertions(+), 4 deletions(-)
```

None of those four is mine; they are the implementor's uncommitted work, untouched. Mine is one untracked file:

```
?? crates/substrate-host/tests/security3_exec_gate.rs      (634 lines, 3 cases)
```

It passes `cargo clippy --release --locked -- -D warnings` and `rustfmt --check --edition 2024`. Nothing is committed and the lease is released.

## 2. Cases added, each run alone

Each ran as `target/release/deps/security3_exec_gate-a0450d83dd363ed1 --exact <case> --nocapture --test-threads=1` in a delegated scope, with a static-pie musl helper built from `1d961b30f` by the lane's own cargo command. Log: `third/final.log`.

| Case | Line | Own output |
|---|---|---|
| `security3_before_the_ruleset_a_listed_start_executes_bubblewrap_and_the_helper_only` | `:375` | `executed before the ruleset [("/usr/bin/bwrap", true), ("/.substrate-exec-gate", true)]`, `1 passed` |
| `security3_a_run_cannot_register_a_binary_format_handler` | `:547` | `every try failed`, same four results unlisted and listed: `register 2, mount 1, mount-proc 1, namespace 28`, `1 passed` |
| `security3_an_image_the_kernel_does_not_start_is_refused_at_the_open` | `:590` | `the byte test called the image Static and the open refused it config.exec-gate-unserved (501)`, `1 passed` |

The first case sets `HostConfig.bubblewrap` to bubblewrap under `strace -f`, so the trace is of the driver's own start. The profile declares `/etc/ld.so.preload` and the command sets `LD_PRELOAD`, both naming a workspace object. Up to the first successful `landlock_restrict_self` the trace shows:

- **Programs executed:** bubblewrap and the helper's mount, once each, no failed attempt.
- **The helper's environment:** one variable.
- **The helper's opens:** `/proc/self/mountinfo` for its contents; eight more, all `O_PATH` (program, `/workspace`, `/tmp`, `/dev`, `bin`, the host loader's path, `/`).
- **The helper's reads:** nine, all from the mount table; none from an inherited descriptor.
- **The helper's mappings:** eight, all anonymous, none executable.
- **Bubblewrap after its last `pivot_root`:** five opens, all under `/proc`; no mapping.

After the ruleset the program opened `/etc/ld.so.preload` and the object, so the trap was armed, and the list held (`listed:0`, `unlisted:126`). The trace is kept at `third/run.trace`.

## 3. Suite

`bash third/suite.sh` ran thirteen binaries, each `--nocapture --test-threads=1`, in one delegated scope with the static helper (`third/suite.log`). It was built from the worktree as it stood, so `gate` and `adversary_exec_lists` include the implementor's uncommitted cases.

| Binary | Result |
|---|---|
| `substrate_exec_gate` (lib) | 36 passed |
| `gate` | 16 passed |
| `adversary_gate` | 2 passed |
| `security_gate` | 5 passed |
| `security2_gate` | 1 passed |
| `substrate_host` (lib) | 490 passed, 20 ignored |
| `confinement_public_items` | 18 passed |
| `ess_confinement_linux` | 76 passed |
| `ess_confinement_systemd` | 22 passed |
| `adversary_exec_lists` | 10 passed |
| `security_exec_gate` | 7 passed |
| `security2_exec_gate` | 6 passed |
| `security3_exec_gate` | 3 passed |

There were 0 `ABSENT` lines and the suite exited 0. After the last edit of my file I re-ran its three cases alone and its binary once, not the whole suite.

## 4. Per item

### Your assertion change

`security2_a_declared_file_at_the_loaders_preload_path_does_not_lift_the_list` keeps what it was for and is stricter. A refusal now fails, the list must hold, and it adds that the object is not loaded in the helper. Green: `executed, the list held; loaded in ["/usr/bin/sh", "bin/tool"]`.

### Item 1, the open's byte test

| Claim | Verdict | Evidence |
|---|---|---|
| Runs on the bytes that are sealed and executed, no second read | holds, by reading | `crates/substrate-host/src/exec_gate.rs:691-699`: one `read_to_end` from the open's descriptor, `check_linkage(path, &bytes)` at `:698`, `ExecGate::seal(&bytes, …)` at `:699`. |
| Refuses all but a native ELF image with a wholly read table and no `PT_INTERP` | holds | `crates/substrate-exec-gate/src/lib.rs:429-480`; committed unit cases green (lib 36). Reading: whenever the kernel accepts an image, it reads the same headers the test reads, so a loader the kernel would start is always seen. |
| A static-pie passes | holds, executed | The helper used is `static-pie linked`, with `DYNAMIC` and no `INTERP` (`readelf`); every lane case opened with it and proved `exec_subpaths`. |
| A dynamic helper is refused | holds, executed | Committed `ESS confinement linux exec helper dynamic: executed`. |
| An image the kernel refuses is not taken | holds, by another step | Note 1 below. |

### Item 2, the lines the design lists as not run

| Design line | Verdict | Evidence |
|---|---|---|
| Bubblewrap maps nothing once inside the view | holds on this host (0.13.0) | Trace above. |
| Bubblewrap executes an absolute first argument without a search | holds, with one addition | Trace: one `execve` of the path. Addition in note 1. |
| Helper and Rust runtime map no library before the ruleset | holds for the musl build | Trace: anonymous mappings only. A static glibc build was not traced. |
| Helper reads none of the descriptors it inherits | holds | Trace: reads from the mount table only. |
| No `binfmt_misc` handler can be registered from inside the view | holds | `register` is absent (ENOENT), both mounts are EPERM, a new user namespace is ENOSPC; same in a listed and an unlisted start. |
| Kernel starts a static ELF directly on a host with a registered handler | could not be verified | This host has none and I did not register one. |
| `systemd` stages run outside the view | could not be verified beyond the committed case | `ESS confinement systemd linux exec preload: executed`; not traced. |
| Kernel resolves a relative `PT_INTERP` from the working directory | not run | Moot: every `PT_INTERP` is refused. |

### Item 3, terminal with a list

| Site | Verdict | Evidence |
|---|---|---|
| Argv builder, `crates/substrate-host/src/process.rs:2957` | holds, executed | Committed `exec gate terminal: executed`: `session.pty-unserved`, class unserved, address `mode`; the unlisted terminal argv is unchanged. |
| Start, `process.rs:1177-1181` | holds by reading; executed by no case | It sits before `admit` at `:1183`, so nothing is pinned. Note 2 below. |

### Item 4, `scripts/delegated-lane.sh`

Holds, executed. I cut the committed helper section out of the script and ran it with stand-ins for `cargo`, `rustc` and `readelf` (`third/lane-gate.log`).

| Condition | Result |
|---|---|
| Static build fails | exit 1, `could not be built statically … does not fall back to a dynamic helper` |
| Build reports nothing, or two executables | exit 1, same message |
| Build for the static target has `PT_INTERP` | exit 1, `is not statically linked` |
| Build for the static target is a script | exit 1, with `readelf`'s own error, not the lane's wording |
| Host with no static target | exit 1, `no static target is known` |
| No `readelf` | exit 1, `readelf is required` |
| Default build is not dynamic | exit 1, by name |
| Control | reaches the export; the `.lane` copy is byte-identical to the build it checked |

No path exports a helper after a failure. Every host test reads `SUBSTRATE_EXEC_GATE` and none locates a build of its own. The helper crate's own tests use cargo's dynamic test build with no driver, as the script states.

### Item 5, unlisted floor

Holds. Reading: the `process.rs` diff `767fbea57..1d961b30f` adds one function and two refusals, guarded by `!lists.is_empty()` and `gate.is_some()`. Executed: my rule-7 golden-tail case, both first-look floor cases and the committed unlisted-terminal assertion are green.

### Notes

| # | Finding | file:line | Verdict | Origin | Measured | What reaches it |
|---|---|---|---|---|---|---|
| 1 | The byte test calls an image `Static` that the kernel refuses with `ENOEXEC` (ELF type of a core file; by reading, also a header size above the kernel's or more than 73 headers). Bubblewrap starts programs with `execvp`, which then runs `/bin/sh <file>`: a dynamic host program in the view. The open still refuses, at the protocol step. | `crates/substrate-exec-gate/src/lib.rs:433-480`; refusal at `crates/substrate-host/src/exec_gate.rs:584` | CONFIRMED, holds by another step | introduced | `third/enoexec.log`: `execve(…) = -1 ENOEXEC`, then `execve("/bin/sh", ["/bin/sh", "<file>", "--version"]) = 0`. My third case: open refused `config.exec-gate-unserved`. | Only the operator's own `exec_gate` file. A file that passes both steps would have to be built for it, and that operator needs no trick. |
| 2 | The start-site terminal refusal is dead code today: `start_for_adapter` passes no pipe settings (`process.rs:962`) and the three wire paths pass no list. No case executes it. | `crates/substrate-host/src/process.rs:1179` | CONFIRMED by reading | introduced | Not executable from a test file; `start_inner` is private. | `story:exec-start-carries-executable-lists`, when a session start gets a list. |
| 3 | The doc comment of `listed_environment` now sits on `listed_terminal_unserved`, which was inserted between the comment and its function. Cosmetic. | `crates/substrate-host/src/process.rs:294-303` | CONFIRMED | introduced | Read. | — |

Decisions for you:
- **Note 1:** either make rule 1 also ask for an executable ELF type and the kernel's header size, or add one sentence to design § 6 that `execvp` falls back to the shell and step 6 is what refuses such a file.
- **Note 2:** a unit case in `process.rs` calling `start_inner` with a list and a window; that is the implementor's file.

## 5. Reviewed and could not fault

- `ElfTarget::NATIVE`: fails closed on an unnamed architecture and on big-endian.
- `check_linkage`'s three messages.
- The `HostConfig.exec_gate` documentation.
- The `rustup target add` line in `release-lanes.yml`.
- `listed_terminal_unserved`.

## 6. Paths written outside the worktree

All under `<scratch>/security1/third/`:
- scripts: `cases.sh`, `suite.sh`, `all.sh`, `final.sh`, `lane-gate.sh`, `enoexec.sh`
- logs: `cases-1.log`, `cases-2.log`, `all.log`, `final.log`, `suite.log`, thirteen `suite-<binary>.log`, `lane-gate.log`, `enoexec.log`
- evidence: `run.trace`, `impl.diff`, `committed-security2_exec_gate.rs`
- helpers: `substrate-exec-gate.static`, `substrate-exec-gate.dynamic`
- `suite.paths`, `security3_exec_gate.path`

Test temporaries went to `security1/tmp/`. I created no target directory: the static build reused `target/x86_64-unknown-linux-musl`, which was already there from the lane and is not mine to remove.

## Not run

- `bash scripts/gate.sh` and the full `bash scripts/delegated-lane.sh`; the daemon's runtime vectors
- a host with a registered `binfmt_misc` handler; a host whose `/bin/sh` is dash; another bubblewrap version
- a static glibc build of the helper
- a trace through `systemd` around `linux`
- the start-site terminal refusal
- the macOS lane

## 7. Findings

```findings
- file: crates/substrate-exec-gate/src/lib.rs
  line: 433
  category: boundary
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: >-
    elf_linkage answers Static for a native image the kernel refuses with ENOEXEC (e_type of a
    core file measured; e_phentsize above the kernel's and more than 73 headers by reading).
    Bubblewrap's execvp then runs /bin/sh with the file, a dynamic host program in the view.
    The open still refuses such a file at the protocol step (config.exec-gate-unserved, case
    security3_an_image_the_kernel_does_not_start_is_refused_at_the_open, green). Either add
    e_type and exact e_phentsize to rule 1 or state in design 35 section 6 that step 6 covers it.
- file: crates/substrate-host/src/process.rs
  line: 1179
  category: coverage
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: >-
    The start-site refusal of a listed start with a controlling terminal is before admit and
    correct by reading, and no case executes it: no caller passes both a list and a terminal
    (start_for_adapter passes no pipe settings, process.rs:962). Only the argv builder's refusal
    (process.rs:2957) is tested. A unit case calling start_inner would hold it.
- file: crates/substrate-host/src/process.rs
  line: 294
  category: documentation
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: >-
    The doc comment of listed_environment is now attached to listed_terminal_unserved, which
    was inserted between the comment and its function.
```
