---
format: aep.planning-md/3
id: review-result:proc-events-security-pass-1
kind: review-result
status: active
title: 'Security review of the host-process event stream, pass 1 (needs change: cursor names no record)'
relations:
- reviews: story:host-process-event-stream
revision: 1
---
unit: story:host-process-event-stream (ADR 0036 S12), uncommitted working tree on cff3f219
verdict: NEEDS-CHANGE
cases: executed 72→76 on the processes binary (portable and delegated), red 3
origin: introduced 5 / pre-existing 0 / undecided 0
needs-coordinator: none

1. **Paths:** `crates/substrate-daemon/tests/processes.rs` (one `#[path]`/`mod` line) and new `crates/substrate-daemon/tests/processes/security_pass1.rs`. No implementation file changed; `git diff --stat -- contracts/` is empty and the only new bundle path is `0.24.0/`.

2. **Cases** (`security_pass1.rs`):

| case | asserts | now |
|---|---|---|
| `security_pass1_a_cursor_of_another_record_does_not_skip_this_records_lifecycle` | record B opened with record A's cursor is refused 409, or still delivers B's `process.started` | red |
| `security_pass1_a_subject_holds_at_most_512_process_event_streams` | with 64 live records × 8 streams open, a 513th (on an ended record) is refused 429 | red |
| `security_pass1_a_cursor_outside_the_published_pattern_is_cursor_invalid` | `pe1.scope_.<gen>.<seq>.0` is `event.cursor-invalid` | red |
| `security_pass1_program_output_is_only_ever_a_line_item` (delegated) | a printed fake `ended` JSON line, CR, NUL and 0xFF each arrive as `line` items; the stream ends only on the real exit | green, 0.78 s |

```
record proc_01M60XX580BB0MA52YTNDEXHA4's stream, opened with the cursor pe1.scope_61d7….6538522917832550.6.0 that record proc_01M60XX59Z4M16V1Z6W9CZ4RB1's stream carried, was upgraded and silently omits proc_01M60XX580BB0MA52YTNDEXHA4's own process.started: ["event:process.stopped", "ended"]
pe1.scope_.8374207354191120.3.0 does not match the published process-event-cursor pattern: {..."code":"event.source-mismatch",...}
with 512 process event streams open, a 513th on the ended record proc_01M60XX75DVVN8YD590DR9AKB3 was upgraded: no per-subject (or deployment) bound counts process event streams
test result: FAILED. 1 passed; 3 failed; 0 ignored; 0 measured; 73 filtered out; finished in 7.31s
```

3. **Suite** (`cargo test -p b10x-substrate-daemon --release --locked --test processes`): without this module `ok. 72 passed; 1 ignored` on both lanes; with it portable `FAILED. 73 passed; 3 failed`. One delegated run also failed the existing `host_process_lease_renewal_extends_renew_by` (409 `lease.expired`) under the CPU load of the 512-stream case; it passed on rerun. clippy and rustfmt on the file: 0. `cargo xtask check-bundle 0.24.0`: `verified: 367 files`. `check-json 0.24.0`: `367`.

4. **Findings**
   - **F1, blocker** (`crates/substrate-daemon/src/app/process_events.rs:137` `parse_cursor`, `:303`). The cursor names no record, so record B accepts A's cursor and silently skips B's events up to that seq, and its lines below the offset when the offset lands on one of B's line boundaries. ADR 0036 S12 says the cursor is per process and lines are never silently skipped. Reached by a watcher resuming with its last cursor after the service it follows restarts under a new record id. Fix: bind the record (id or digest) into the cursor and refuse a mismatch `event.cursor-invalid`.
   - **F2, warning** (`process_events.rs:62-90`; `spec/host-processes/README.md:116`). Streams are counted per record only. The stated 512-per-subject bound ignores streams on ended records (up to 256 retained × 8), and there is no deployment cap, although `runtime.rs:111-115` says every `StreamSlot` has both. Only the daemon's own uid reaches the route (`processes.rs:1077`). Fix: a per-subject and deployment cap, or correct the claim.
   - **F3, note** (`process_events.rs:143`). Any `scope_…` source is accepted while `0.24.0/schemas/common.json:143` publishes `scope_[A-Za-z0-9_-]+`; a malformed cursor gets `event.source-mismatch` (retriable) instead of `event.cursor-invalid`.
   - **F4, note, judgement** (`process_events.rs:59`, `:490-516`; read from code, not measured). Each stream makes two store reads under the single connection mutex plus a log read every 100 ms regardless of wakeups: about 10k store reads per second at 512 streams.
   - **F5, note, judgement** (`crates/substrate-daemon/tests/runtime_vectors.rs:2837`). No runner executes `vectors/http/process-events-beyond-the-readers-bound-is-exhausted.json`; xtask checks only its shape. The in-crate `host_process_stream_readers_bound_is_refused` covers the behaviour.

5. **Checked and cleared:** admission refuses non-Unix transports and any subject but the daemon's uid before any read; lookup is scoped by subject and the reader key is `(Scope, id)`; hosted maps `/v1/processes/*` to the `process` scope and answers 403 `process.host-posture-refused` over TCP or TLS; `read_from` refuses an offset past the end or not preceded by `\n`; offset below retention gives a truthful `gap`; seq beyond end is `event.cursor-invalid`, below retention `event.retention-gap`, another source or generation `event.source-mismatch`; output cannot forge an item (case 4); lines are cut at 16 KiB on a character boundary, also after lossy growth; 5 s send deadline, 120 control frames per minute then 1008, data frame 1003, 1 KiB input frames, 1 h lifetime then 1011; the reader slot returns on every exit path and the transport slot is handed over; 0.1.0 to 0.23.0 untouched; bounds held to their `substrate_wire` constants; the new refusal has a vector.

```findings
[{"file":"crates/substrate-daemon/src/app/process_events.rs","line":137,"category":"integrity","severity":"blocker","verdict":"NEEDS-CHANGE","origin":"introduced","message":"the cursor carries no record binding, so another record's cursor opens this record's stream and silently skips its process.started and earlier lines, against ADR 0036 S12's per-process cursor and never-skip rule"},
{"file":"crates/substrate-daemon/src/app/process_events.rs","line":62,"category":"acceptance","severity":"warning","verdict":"CONFIRMED","origin":"introduced","message":"streams are counted per record only, so a subject's 513th stream (on an ended record) is upgraded, contrary to the stated 512-per-subject bound, and no deployment-wide cap exists"},
{"file":"crates/substrate-daemon/src/app/process_events.rs","line":143,"category":"contract-drift","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"parse_cursor admits sources outside the published process-event-cursor pattern and answers them event.source-mismatch instead of event.cursor-invalid"},
{"file":"crates/substrate-daemon/src/app/process_events.rs","line":59,"category":"judgement","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"every stream polls two store reads under the single connection mutex plus a log read every 100 ms regardless of wakeups, about 10k store reads per second at the stated 512 streams (read from code, not measured)"},
{"file":"crates/substrate-daemon/tests/runtime_vectors.rs","line":2837,"category":"judgement","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"the process.events-readers-exceeded vector is shape-checked by xtask but executed by no runner"}]
```
