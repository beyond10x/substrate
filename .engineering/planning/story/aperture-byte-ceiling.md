---
format: aep.planning-md/1
id: story:aperture-byte-ceiling
kind: story
status: implemented
title: A declared aperture carries a byte ceiling that refuses mid-run
summary: Design 10 § 5 row 5 names exec.aperture-byte-limit; the counter exists, the ceiling does not.
relations:
- decomposes: epic:byte-plane-completion
- depends_on: story:destination-bound-egress
revision: 9
---
# Story: A declared aperture carries a byte ceiling that refuses mid-run

## Outcome

An operator who declares an aperture can bound what crosses it, and a run that exceeds the bound is
stopped with a named refusal rather than left to drain the link. Today the bytes are counted and
reported but nothing reads them, so a confined run's egress volume is observable after the fact and
unbounded during it.

## Context

`docs/design/10-destination-bound-egress.md:126` names the condition and its answer:
*Declared byte ceiling exceeded mid-run* → class `exhausted`, code `exec.aperture-byte-limit`, no
address. `:284-287` records why it did not ship with ADR 0013: *"There is no declared byte ceiling
in the configuration surface, so there is nothing to exceed."*

Verified in the tree, not inferred — the half that exists:

- `ApertureBytes { to_destination, from_destination }`, `crates/substrate-wire/src/lib.rs:790-795`.
- The forwarder holds both counters as `AtomicU64` and the parent reads them without synchronising,
  `crates/substrate-host/src/egress.rs:621-622` and `:252-256`.
- Every applied observation already carries them, `crates/substrate-host/src/egress.rs:288-297`.

The half that does not:

- `EgressAperture { name, host, port, pinned }`, `crates/substrate-host/src/egress.rs:83-88` — four
  fields, no ceiling.
- `--egress-aperture name=host:port/tcp`, `crates/substrate-daemon/src/main.rs:108`, parsed at
  `:34-56` — the grammar has nowhere to put one.
- Nothing reads `counters` for a comparison; the only readers are `applied()` and the tests.

The refusal class already exists: `Exhausted`, `crates/substrate-wire/src/lib.rs:132`.

## Acceptance

An aperture declared with a ceiling refuses the run by name when the ceiling is crossed, and the
same aperture declared without one behaves exactly as it does today.

Evidence that satisfies it, in order:

1. An ADR fixing the declaration grammar, which direction(s) the ceiling counts, whether it is
   per-run or per-aperture-lifetime, and what the child observes at the moment of refusal
   (invariant 8: design before code).
2. A successor bundle carrying the ceiling on the aperture capability fact and the
   `exec.aperture-byte-limit` refusal class; earlier bundle bytes unchanged (invariant 6).
3. A delegated-lane vector: a child that reads more than the declared ceiling from the pinned
   destination ends `exhausted` with code `exec.aperture-byte-limit`, and the applied observation
   reports at least the ceiling in `bytes`.
4. A delegated-lane vector proving the negative: an aperture declared with no ceiling passes the
   same traffic to completion, so the existing `declared-aperture-is-reachable` case
   (`docs/design/10-destination-bound-egress.md:161`) is unchanged.
5. A vector proving the ceiling is a *deployment* declaration and not request data — a request that
   carries a ceiling is refused, the same way a raw destination is
   (`exec.aperture-destination-in-request`, `docs/design/10-destination-bound-egress.md:124`).

## Out of Scope

Rate limits, time ceilings, and any ceiling on the `none` network mode — there is no forwarder there
to count in.

## Open

Whether the ceiling is enforced in the forwarder (it holds the counters, and can stop reading) or by
the parent watching the atomics (it can end the operation with the right class but cannot stop the
bytes already in flight). The ADR decides; both are consistent with the counters as they stand.

## Design draft — 2026-08-30

`docs/design/12-aperture-byte-ceiling.md`, **proposed**. No ADR number is claimed: `adr/` admits
`accepted` and `superseded` only (`xtask/src/adrs.rs:12`), so the number is assigned at acceptance.
Design 10's deferred note now points at it instead of describing an open gap.

Decisions it fixes, each with what the alternative would have cost:

| Decision | Chosen | Alternative's cost |
|---|---|---|
| Grammar | optional `/max=<size>` term on `--egress-aperture name=host:port/tcp`; absent = today byte for byte | a separate flag lets a ceiling name an aperture that does not exist; comma is unavailable either way (`crates/substrate-daemon/src/main.rs:116`, `value_delimiter = ','`) |
| Direction | one ceiling over `to_destination + from_destination` summed | a per-direction pair is two numbers to get wrong and a bound a child evades by picking a direction |
| Scope | per run | per-lifetime needs durable cross-run accounting and makes a refusal unreproducible from its own request |
| Enforcement | in the relay, which stops at the ceiling; overshoot bounded by `RELAY_BUFFER = 16_384` (`crates/substrate-host/src/egress.rs:68`) | parent-only stops no byte: overshoot is bounded by the destination's throughput, not by substrate |

**Gap the draft found, verified not inferred.** At HEAD a mid-run bound has no code to report. Both
existing mid-run bounds fold into one state: `forced_cancellation = timed_out || cpu_exhausted ||
cancellation_requested` (`crates/substrate-host/src/process.rs:1318-1319`) sets
`ExecState::Cancelled` (`:1364`). The supervision loop that would notice a ceiling already exists
and already polls at 1 ms (`:1392`, `tokio::time::interval(Duration::from_millis(1))`), so the
mechanism is there and the vocabulary is not — `exhausted` / `exec.aperture-byte-limit`
(`docs/design/10-destination-bound-egress.md:126`) has nowhere to live in today's observation. The
draft therefore also proposes an optional class/code/message field on the exec observation, which is
what makes this a contract change rather than a host-local one.

## Blocked on

Operator acceptance of design 12. Invariant 8: an ADR before code. Nothing here is implementable
until that decision is made.

## Accepted — 2026-08-30

The operator accepted design 12 as
[`adr/0014-apertures-carry-a-declared-byte-ceiling.md`](../../../adr/0014-apertures-carry-a-declared-byte-ceiling.md).
Evidence item 1 is satisfied; invariant 8 no longer blocks implementation.

One number moved while the design waited: it was drafted against successor bundle `0.7.0`, and that
number went to ADR 0011's grant attribution. The successor is **`0.8.0`**, predecessor `0.7.0`,
`adds_routes: 0`, `preserves_routes: 26` (`contracts/substrate-wire/0.7.0/bundle.json:5-10`), with
`cargo xtask check-bundle 0.8.0` added to `scripts/gate.sh`.

Remaining evidence: items 2-5 — the successor bundle, the two delegated-lane vectors (the ceiling
refuses; an aperture without one is unchanged) and the request-side refusal
`exec.aperture-ceiling-in-request`.

## Scope

Derived 2026-08-30 by `story-scoper`, verified against HEAD `34b219a`. Every line is **cited** or
**inferred**.

- **Primary surface:** `crates/substrate-host` — the aperture byte path — cited (ADR 0014
  § *Decision*, "Enforced in the relay, classified by the parent").
- **Files, cited and exact at HEAD:** `crates/substrate-host/src/egress.rs:83-88`
  (`EgressAperture`, gains `Option<u64>`), `:68` (`RELAY_BUFFER`, the stated overshoot bound),
  `:713-818` (`relay_body`/`forward_once`, where the comparison lands), `:252-256`
  (`SharedCounters::read`), `:322` (`install`, per-run counter lifetime), `:92-93`
  (`EgressAperture::fact`).
- **Files, cited:** `crates/substrate-host/src/process.rs:1318-1319` (`forced_cancellation`),
  `:1366` (`ExecState::Cancelled`), `:1392-1414` (1 ms supervision poll and `cgroup.kill_all`),
  `:919` (driver-side aperture refusal mapping).
- **Files, cited with positions corrected:** `crates/substrate-wire/src/lib.rs:783`
  (`AppliedAperture`), `:813` (`ApertureBytes`), `:1874` (`EgressApertureFact`), `:983-990`
  (`Exec`, gains the optional class/code/message field), `:1577` and `:1627`
  (`WireValidationError`), `:1909` (`validate_aperture_request`), `:155` (`Exhausted`).
- **Files, cited with positions corrected:** `crates/substrate-daemon/src/main.rs:34-60`
  (`parse_egress_aperture`, gains `/max=<size>`), `:144-155` (the clap arg; `value_delimiter = ','`
  at `:152` is why the separator cannot be a comma).
- **Contract surface, cited:** new `contracts/substrate-wire/0.8.0/` (predecessor `0.7.0`,
  `adds_routes: 0`, `preserves_routes: 26`, mirroring
  `contracts/substrate-wire/0.7.0/bundle.json:5-10`), new `xtask/bundle-source/0.8.0/`, and a
  `check-bundle 0.8.0` line after `scripts/gate.sh:29`.
- **Also likely:** `xtask/src/bundle.rs:384` (per-version branch) and its census functions at
  `:681-682`, `:704`, `:865` — inferred; no document names the file.
- **Also likely:** `crates/substrate-daemon/tests/runtime_vectors.rs` and `tests/contract_vectors.rs`
  for evidence items 3-5 — inferred by analogy with `vectors/http/aperture-destination-in-request-refused.json`,
  the only aperture vector that exists.
- **Confidence:** **high** — ADR 0014 and design 12 name the declaration, enforcement, contract and
  bundle sites with `file:line`, and every host-side citation reproduces exactly at HEAD.
- **Would collide with:** any unit touching `crates/substrate-wire/src/lib.rs` (this story edits it
  in six places), the egress byte path, the supervision loop, the daemon CLI flag surface, or the
  bundle gate. **Hard exclusion:** any other unit cutting a successor bundle — `0.8.0` is one
  directory and one gate line, and two stories cannot both own it.

### Gap found while scoping, not present in either document

`crates/substrate-daemon/src/runtime.rs:335-339` declares a **second** `EgressAperture` — the
daemon's own three-field config vocabulary (`name`, `host`, `port`), deliberately not re-exported
from `substrate_host` (invariant 4), converted at the composition root (`:424`, `:451`). ADR 0014 and
`docs/design/12-aperture-byte-ceiling.md` mention `runtime.rs` **zero times**. The ceiling has to
land there too, or the declared value cannot reach the host type the ADR names. Verified:
`grep -c runtime.rs` over both documents returns `0`.

## Implemented — 2026-08-30, one-unit wave

Merged on `wave/aperture-byte-ceiling` at `433ec68`, unit commit `5802e8d`.

Evidence items 2-5 are met. `contracts/substrate-wire/0.8.0/`, 228 files, predecessor `0.7.0`,
`adds_routes: 0`, `preserves_routes: 26`; every `0.7.0` vector, fixture and schema byte-identical in
`0.8.0` except `$id` and one `contract` string. `PORTABLE_CASES` 33 → 34, `DELEGATED_CASES` 50 → 54.

Verified by the orchestrator rather than taken on report: `bash scripts/gate.sh` → `gate: passed`,
exit 0, run on the integration branch after the merge. `bash scripts/delegated-lane.sh` → exit 0,
`54 HTTP cases … (delegated lane)`. `cargo xtask check-json` → `1544 documents in 8 bundles`.
`git status --short contracts/` showed only the new `0.8.0`; `xtask/src/render.rs` untouched.

**The surface ADR 0014 never named.** The ADR and design 12 mention `runtime.rs` zero times, but
`crates/substrate-daemon/src/runtime.rs` declares the daemon's own three-field `EgressAperture`,
deliberately not re-exported from `substrate_host` (invariant 4). The ceiling landed in both types
and the conversion, and `configuration_generation` now formats `name=host:port/tcp[/max=N]` so a
changed ceiling moves the snapshot digest — a published fact that does not move the snapshot is a
stale fact. Found by `story-scoper` before dispatch, not at merge time.

### Two defects the adversary found, both verified by the orchestrator before being routed back

**A client-supplied aperture name could panic the handler.** `reads_as_ceiling` sliced `term[..4]`
on the raw request string and ran before `valid_aperture_name`, so any name whose multi-byte
character straddled byte index 4 panicked — `ab€cd`, `mo€del`, `ma€x`. Reproduced independently in
isolation. At `0.7.0` that request was `422 exec.aperture-name-invalid`; with the slice it became a
dropped connection carrying zero bytes, no status, no error class. Invariant 3 inverted: a refusal
turned into a crash. The comparison is now on bytes via `get(..4)`, total for every input.

**A run stopped at the ceiling could report no refusal at all.** `aperture_exhausted` was set only
inside the supervision tick, which shares a `select!` with `child.wait()`. The relay closes the
socket, the child exits cleanly on that EOF, `child.wait()` wins, and the flag was never read again
— the run recorded `state: exited, exit 0, refusal: null`, byte-identical to a run that finished on
its own. Measured at 4-6 of 20. The ceiling is now asked once more after the wait, while the
forwarder is still alive. The implementor reports 200 runs across 10 executions with `refusal: null`
zero times and the race window still hit 74 of 200, every one now `cancelled` with
`exec.aperture-byte-limit`; the orchestrator re-ran the case 15 consecutive times, 0 failures.

A third disagreement was fixed with them: the relay and the parent disagreed at
`max_bytes: Some(0)` — unreachable through the daemon, reachable through the public
`substrate_host::EgressAperture`.

### Recorded, deliberately not fixed

The same `select!` race can drop `cpu_exhausted`. It loses one state bit and has no refusal code
today (ADR 0014 defers naming timeout and CPU exhaustion), where the ceiling lost the only field
distinguishing a bounded run from an unbounded one. Widening the change was refused rather than
forgotten.

There is no bundle *vector* for the startup grammar refusal: the `vector.json` `startup` action
shape has no configuration member to carry a malformed aperture declaration. The shipped binary is
asserted instead, in `check_aperture_term_refusal`.
