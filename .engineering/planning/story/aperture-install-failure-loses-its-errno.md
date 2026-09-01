---
format: aep.planning-md/1
id: story:aperture-install-failure-loses-its-errno
kind: story
status: implemented
title: An aperture install failure names its stage and loses its errno
summary: The sandbox helper reports through _exit(stage), so a stage-6 bind failure carries no errno; the delegated lane fails on it about one run in ten.
tags:
- wave/remote-foundation-01
relations:
- decomposes: epic:release-hardening
revision: 6
---
# Story: An aperture install failure names its stage and loses its errno

## Outcome

When `exec.aperture-install-failed` fires, the operator learns *why* and not only *where*, and the
intermittent stage-6 failure in the delegated lane is either fixed or explained.

## What was observed

**Reported by the pty implementor on 2026-08-30, not reproduced by the coordinator.**
`egress::tests::declared_aperture_is_reachable` (`crates/substrate-host/src/egress.rs:1537`) fails
intermittently inside a delegated systemd scope:

```
install the aperture: DriverError { class: Failed, code: "exec.aperture-install-failed",
message: "The egress aperture could not be installed exactly as declared (stage 6)." }
```

Its numbers, as given: **1 failure in 10** serial delegated runs; 0 in 3 isolated runs of `egress::`
alone; 0 in the portable `cargo test --workspace`. None of these were re-run independently.

It became visible only because `scripts/delegated-lane.sh` stopped selecting host cases by the
substring `pty` — that filter had been running 8 of 58 host lib cases, so this test had never
executed in a delegated scope. The fix is on `wt/pty-sessions`.

## Blast radius, verified

`scripts/gate.sh` contains no reference to the delegated lane or to
`SUBSTRATE_VECTORS_CGROUP_ROOT` (`grep`, 0 hits). **The whole gate is unaffected.** What is affected
is `bash scripts/delegated-lane.sh`, which is run by hand and is the only thing that executes the
delegated cases at all — so a lane that is red one run in ten is a lane whose red gets discounted.

## What the code says, verified

- **Stage 6 is `bind`.** The sandbox helper returns a stage number as its exit status; `return 6`
  follows `libc::bind(listener, …)` on `loopback_sockaddr(port)`, inside the sandbox's network
  namespace after `setns(CLONE_NEWUSER)` and `setns(CLONE_NEWNET)`.
- **`SO_REUSEADDR` is already set on that socket**, immediately before the bind. So the first
  hypothesis anybody reaches for — a previous run's socket in `TIME_WAIT` — is ruled out, and the
  next person should not spend the afternoon on it.
- **The errno cannot escape, and `install_failed` is not the reason.** The helper's only channel to
  its parent is `_exit(stage)` (`egress.rs:456-464` turns that exit status into the message), and an
  exit code carries a stage, not an errno. The helper is a fork precisely because
  `setns` with `CLONE_NEWUSER` refuses a threaded caller, so it cannot allocate or return a
  `DriverError`. The `install_failed` helper at `egress.rs:1092` *does* interpolate `{error}`; it is
  simply not on this path.
- The design states the trade deliberately: "The return value is the stage that failed, so an
  install failure names *where* rather than leaving an operator to guess between a namespace, a bind
  and a handback."

**Not established:** where `port` is chosen, whether two runs in one delegated scope can choose the
same one, and what the bind's actual errno is. Everything about the cause is open.

## Acceptance

An aperture install failure carries the failing stage **and** the errno, and the stage-6 failure in
the delegated lane is either eliminated or has a named cause recorded against this story.

Evidence that satisfies it, in order:

1. The helper's stage channel widens to carry an errno — the exit status is one byte and already
   spent, so this needs a second channel (the existing handback socket, a pipe, or an `AtomicI32` in
   the shared mapping the forwarder already maps). Written failing-first against a forced bind
   failure.
2. The message names both, and a test asserts the errno reaches the caller.
3. `bash scripts/delegated-lane.sh` run at least 20 times serially, with the failure count recorded
   here — before and after. One failure in ten is a claim a 10-run sample cannot separate from two
   in ten.
4. If the cause is a port collision, the port's selection is the fix and the errno is the evidence
   that found it; if it is not, this story records what it was.

## Provenance

Found while running `story:pty-sessions` in the wave of 2026-08-30, in the lane that story repaired.
Not caused by it: the test and the helper both predate the wave. The coordinator verified the four
code claims above against `wave/2026-08-30-byte-plane`; the failure rate and the failing run are the
implementor's, quoted.

## Correction — 2026-08-30, the flake is wider than this story says

The observation above names **one** test at **1 failure in 10**. A second reporter — the adversary
on `story:pty-sessions`' fifth pass — puts it at **module-wide, roughly 1 run in 2**, with two
further tests failing that this story does not name:

| test | `file:line` | observed |
|---|---|---|
| `declared_aperture_is_reachable` | `crates/substrate-host/src/egress.rs:1537` | the original report |
| `applied_aperture_is_observed` | `crates/substrate-host/src/egress.rs:1657` | `Unreachable` where `Served` was expected |
| `a_declared_ceiling_stops_the_relay` | `crates/substrate-host/src/egress.rs` | named, no assertion text captured |

Both rates are **reported, not reproduced by the coordinator**, and they disagree by a factor of
five. Two reporters, two runs of different lengths, on one machine — the honest reading is that the
rate is unmeasured, not that it is 1 in 2 rather than 1 in 10. Acceptance item 3 already asks for a
20-run sample; that sample now has to cover the **module**, not the one test.

What this changes about the story: the stage-6 `bind` in `declared_aperture_is_reachable` is no
longer necessarily the whole subject. `applied_aperture_is_observed` failing with `Unreachable` is a
different symptom — a relay that installed and did not carry — so a single cause is a hypothesis and
not a finding. The errno the helper cannot report (`_exit(stage)`) is still the first thing to fix
either way: it is what would tell these two symptoms apart.

**Still verified, unchanged:** `scripts/gate.sh` has no delegated-lane reference, so the whole gate
is unaffected; stage 6 is `libc::bind` with `SO_REUSEADDR` already set; the errno is lost at the
`_exit(stage)` boundary and not in `install_failed`.

## Implementation evidence — 2026-09-01

The inherited implementation had begun sending `[stage, errno]` over the descriptor handback, but
the production pair was still `SOCK_STREAM`, the receiver assumed one read returned the complete
record, and coverage injected a payload rather than forcing the helper to fail. The failing-first
test `helper_handback_preserves_record_boundaries` observed `SO_TYPE=1` (`SOCK_STREAM`) where a
record-preserving channel was required.

The completed path uses `SOCK_SEQPACKET | SOCK_CLOEXEC`; failure records and `SCM_RIGHTS` success
handbacks are distinct packets. Sends suppress `SIGPIPE`, receive and reap retry `EINTR`, truncation
and unexpected control messages fail closed, and the exit status remains the fallback when no
record arrives. `a_forced_bind_failure_names_its_stage_and_errno` opens one listener inside a held
sandbox, forces a second bind on the same tuple, and asserts the caller receives class `failed`,
code `exec.aperture-install-failed`, stage 6, numeric `EADDRINUSE`, and its decoded OS error.

### Named cause and fix

The first local `bash scripts/delegated-lane.sh` reproduction failed in
`an_aperture_without_a_ceiling_passes_the_same_traffic` at the in-namespace connect. The widened
helper channel then caught the original bind symptom as stage 6, errno 99
(`EADDRNOTAVAIL`, `Cannot assign requested address`). Together with the reported
`Reach::Unreachable` (`ENETUNREACH`), this identifies one readiness race: bubblewrap publishes the
child pid while its concurrent child has not necessarily raised `lo` yet. The helper now reads
`IFF_UP` and retries only `EADDRNOTAVAIL`, using one bounded one-second budget. It does not configure
the interface or widen the loopback-only network floor; every other bind errno is immediate.

The module harness also detached every `AppServer` and `Firehose` thread: each `Drop` discarded its
`JoinHandle` while the thread retained its listener and blocked in `accept`. The affected test was
0/100 failures in isolation but failed once in a 20-run module sample with the prior server threads
present. Both fixtures now stop, wake and join their thread, so the module sample does not accumulate
listeners or scheduling pressure.

### Evidence

- Failing first: `cargo test -p b10x-substrate-host --locked
  egress::tests::helper_handback_preserves_record_boundaries -- --exact --nocapture` ran one test and
  failed with `left: 1`, `right: 5` before the production pair changed.
- Focused final: `cargo fmt --all --check`, `cargo clippy -p b10x-substrate-host --all-targets
  --locked -- -D warnings`, and `cargo test -p b10x-substrate-host --locked --lib egress::tests:: --
  --test-threads=1 --nocapture` passed; the module ran 17 tests.
- Required delegated module sample: 20 serial executions inside one `Delegate=yes` systemd scope,
  after moving the runner into a child cgroup and enabling `cpu`, `memory` and `pids`, passed
  **20/20 with 0 failures**. Earlier intermediate samples that failed were discarded rather than
  counted green.
- Integrated delegated lane: `bash scripts/delegated-lane.sh` passed 66 host unit tests, all host
  integration tests, and 7/7 clean-room daemon tests. The lane printed its delegated cgroup root and
  controllers, so this was executed rather than absent.
- Full repository gate: `bash scripts/gate.sh` passed, including release tests, fmt, workspace
  clippy, full-history secrets, advisories, licences, package assembly, all twelve bundle fixed
  points, 2,557 classified contract JSON documents, and toolchain pins.

This closes the accepted outcome in code and evidence. The story remains `active` until the operator
chooses the lifecycle move.
