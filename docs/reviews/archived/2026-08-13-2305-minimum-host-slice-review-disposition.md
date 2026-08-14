# Disposition: minimum host substrate slice review

- **Date:** 2026-08-13
- **Review:** `2026-08-13-2305-minimum-host-slice-review.md`
- **Disposition:** addressed and archived
- **Scope:** all 15 ranked findings, all five verified cut findings, snapshot GC, and renewal replay ordering

This disposition records evidence, not a waiver. The immutable `substrate-wire/0.1.0` bytes were
not edited. Three unsafe expectations are explicitly corrected in the reproducible 0.2.0 bundle
and exercised through manifest-selected runtime tests.

This is not a claim of full 0.1 conformance. The named 0.1 development-vector errata are
`vectors/http/machinery-failure.json` (`retriable:true` on a terminally persisted answer) and
`vectors/driver/crash-before-dispatch.json` (remaining `accepted` after daemon restart), and
`vectors/http/input-body-limit.json` (a 1 MiB envelope that cannot represent an exactly 1 MiB
decoded file). Pretending
the implementation satisfies those bytes would preserve unsafe semantics. Version 0.2 therefore
carries the corrected expectations, and its machine-readable `compatibility.json` makes all three
errata visible to 0.1 development consumers.

## Ranked findings

| # | Disposition | Evidence |
|---:|---|---|
| 1 | Closed | Terminal exec observations remain tracked until `put_exec` or `complete_exec_leased` commits and an exact terminal `acknowledge_exec` follows. A deterministic stale-Running ACK race cannot remove a newer terminal observation. The delegated lane completes 129 waited and 129 abandoned execs, then starts another exec. |
| 2 | Closed | `Unknown` is not treated as provably active and therefore cannot permanently block destroy. Signalling an unknown or already-terminal durable exec returns its stored observation rather than fabricating a 404. Store recovery tests cover the destroy condition. |
| 3 | Closed | `wait_terminal` registers and enables the `Notify` future before the state recheck and loops after timeout boundaries. Fast-exit delegated runs exercise the race repeatedly. |
| 4 | Closed | `observe` is non-destructive. Driver state is removed only by explicit acknowledgement after durable persistence. Periodic maintenance persists before acknowledgement. |
| 5 | Closed | Bubblewrap environment construction clears injected `PWD` and restores an explicitly requested value with valid argv ordering. The delegated lane executes a `PWD=/workspace` case successfully. |
| 6 | Closed | Transient `accept` and peer-credential failures warn and continue. Socket cleanup is RAII-owned, so every return path removes the owned socket. |
| 7 | Closed | Mutation `preflight` inspects the subject-scoped idempotency ledger before resource existence or dispatch. Destroy and file-write replays after resource absence pass in the portable lane. Renewal additionally preflights before its synchronous expiry sweep. |
| 8 | Closed | A failure committed as a terminal replay answer is emitted with `retriable=false`; callers are never told the same operation id can be retried. The corrected 0.2 machinery vector executes exactly. |
| 9 | Closed | The envelope bound is 2 MiB. The portable lane writes the advertised exact 1,048,576-byte file successfully and still rejects an envelope above 2 MiB. |
| 10 | Closed | Destruction traverses raw directory-entry bytes, owns `DIR*` by RAII, and flattens nested directories descriptor-relatively in 4,096-item batches. It has no total depth/item cap, proves progress for every incomplete batch, and resumes from durable `destroying`. Tests remove FIFO/non-UTF-8 content plus depth 1,100 and 100,001 direct entries. |
| 11 | Closed | Signal delivery no longer sets cancellation unconditionally. The application process set is signalled without killing the bubblewrap supervisor; wrapper `128+signal` status is normalized. A TERM-trapping command exits zero and is recorded `exited`; forced grace expiry records `KILL`. |
| 12 | Closed | Pre-dispatch mutation refusals use one durable path across workspace creation, file mutations, and exec start. A changed body under a consumed operation id conflicts consistently. |
| 13 | Closed | `contract_vectors.rs` selects disputed cases through bundle manifests and executes them at the declared version. Path-depth, cross-subject hiding, and TERM remain exact 0.1 behavior. The 0.2 bundle explicitly corrects machinery retryability, restart reconciliation, and the input-body fixture through an exact three-record errata inventory. Both bundle checkers and exact hash fixtures pass. |
| 14 | Closed | Workspace observation uses conditional update and cannot upsert a row deleted by concurrent destroy. Store tests prove no resurrection. |
| 15 | Closed | Exec start and workspace destroy share a fixed 256-stripe async lock keyed by deployment, subject, and workspace, avoiding an attacker-grown map and cross-scope key aliasing. Destroy first commits `destroying`; start cannot pass the boundary concurrently or after restart. A crash-after-destroying test reconciles to `unknown`, resumes cleanup, terminalizes the original operation/tombstone, and proves no exec dispatch. |

## Verified cut findings

| Finding | Disposition | Evidence |
|---|---|---|
| Signal-versus-exit false 404 | Closed | A driver `NotFound` triggers a durable re-read; stored terminal/unknown state completes the signal operation instead of recording 404. An injected signal/maintenance race proves it deterministically; TERM/exit classification is covered by delegated adversaries. |
| Dual-daemon socket steal | Closed | An adjacent nonblocking instance lock is acquired before stale-socket handling. The second daemon is refused while the first continues to answer `/v1/machine`. |
| Blocking filesystem/SQLite on Tokio workers | Closed | Guarded filesystem calls use a bounded 16-slot `spawn_blocking` lane. Runtime SQLite calls use a separate 16-slot lane; permits are acquired asynchronously before `block_in_place`. The saturation test proves unrelated async work remains schedulable. |
| Missing direct-driver upper bounds | Closed | Admission enforces explicit ceilings for process count, memory, and CPU time in addition to nonzero/lower checks. |
| `put_exec` fsync on every poll | Closed | Byte-identical observations return without a transaction. Terminal observations are acknowledged after commit, so polling cannot repeatedly write an unchanged row. |

## Additional closure conditions

- Snapshot GC physically deletes expired `snapshots`; the foreign-key cascade deletes all
  `snapshot_items`. Active snapshots are bounded to 64 and items to 4,096 per snapshot. A bounded
  set of 1,024 expiry markers per subject preserves `snapshot.expired` versus a never-created ID.
- Workspace and exec renewal preflight runs before expiry sweep and resource lookup. Portable and
  delegated cases replay the original successful renewal after expiry/deletion and reject a
  changed body with `operation.request-conflict`.
- Durable `exited`, `cancelled`, and `expired` exec states are immutable across competing
  maintenance writes. A concurrent store regression test covers all three states.
- Live process observations receive the accepted lease before persistence; later lease-less driver
  observations preserve it. Store and snapshot tests prove the active lease survives terminal
  observation and remains projected for reconciliation.
- Snapshot active/item exhaustion terminalizes the accepted operation with stable 429
  `snapshot.limit` in the same transaction. Tests cover the 65th active snapshot, 4,097th item,
  replay/conflict, zero partial rows, and the 1,024-marker retention bound.
- Every JSON document in both bundles has one exact declared or fixed schema classification. The
  pinned offline Rust `jsonschema` validator meta-validates all Draft 2020-12 schemas and validates
  every classified instance. Four negative tests prove unclassified JSON, an invalid fixed
  authority, an invalid declared payload, and an invalid schema each fail through the production
  gate. The 0.1 authority bytes remain unchanged.
- Blocking lanes apply bounded backpressure. Neither semaphore wait blocks a Tokio worker.

## Validation record

- `cargo test --workspace`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `python3 scripts/check-contract-bundle.py`
- `python3 scripts/check-contract-bundle-0.2.0.py`
- `python3 scripts/test_contract_json_gate.py -v`
- `python3 scripts/check-runtime-vectors.py` — 26 portable HTTP cases plus startup and dual-daemon refusal
- delegated systemd lane — 37 HTTP cases plus startup and dual-daemon refusal
- `git diff --exit-code -- contracts/substrate-wire/0.1.0`

## Deferred cleanup, not hidden

The review's cleanup tier remains non-blocking future work: deduplicate secret/output helpers,
reduce mirror observation structs, audit unused dependencies, share a router per listener, avoid
redundant base64/read-back work where proof is preserved, and centralize JCS tooling. None is used
as evidence for closing a correctness, isolation, durability, or availability finding above.
