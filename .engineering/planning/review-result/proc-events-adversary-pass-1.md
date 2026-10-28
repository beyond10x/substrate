---
format: aep.planning-md/3
id: review-result:proc-events-adversary-pass-1
kind: review-result
status: active
title: 'Adversary pass on the host-process event stream, pass 1 (needs change: retention refusal)'
relations:
- reviews: story:host-process-event-stream
revision: 1
---
unit: story:host-process-event-stream, uncommitted working tree on cff3f219
verdict: NEEDS-CHANGE
cases: executed 76→87, red 3
origin: introduced 4 / pre-existing 0 / undecided 0
needs-coordinator: whether ADR 0036 S12 or the README's retention-gap rule governs (finding 1)

1. **Paths:** `crates/substrate-daemon/tests/processes.rs` (one `#[path]`/`mod` line) and new `crates/substrate-daemon/tests/processes/adversary_pass1.rs`. No implementation file changed.

2. **Red cases** (`adversary_pass1.rs`, each run alone first):
   - `adv_ended_record_whose_start_left_event_retention_still_replays_and_closes` (:317, portable, retention 4; precondition checked: start event gone, end event retained): refused 409 `event.retention-gap`, `retriable: true`, `address: "cursor"`.
   - `adv_running_record_whose_start_left_event_retention_can_still_be_watched` (:355, portable): refused 409 `event.retention-gap`.
   - `adv_a_reader_attached_while_starting_receives_process_started_first` (:924, portable, gated driver): `left: ["line:early", "event:process.started"]`, `right: ["event:process.started", "line:early"]`.

   **Green cases**, each aimed at a mutant the unit's 13 cases miss (reasoned, no mutant built): cursor older than event retention refused before the upgrade; abandoned handshakes return their reader slots; a data frame closes 1003; a renewal between two lines is placed and resumed between them, two readers get identical items and every item decodes and passes `Event::validate_closed_shape` (delegated); stop and lease expiry come after the lines before them (delegated); lines of 16384, 16385 and one without a trailing newline (delegated); a cursor resumes across a daemon restart (delegated); a resume into discarded bytes states the bytes after the cursor (delegated).

3. **Suite** (`processes`, delegated lane, `--test-threads=1`): `FAILED. 81 passed; 6 failed; 1 ignored; ... finished in 94.98s` (these 3 and the security reviewer's 3); with `--skip adversary_pass1`: `73 passed; 3 failed; ... 11 filtered out`. `cargo xtask check-bundle 0.24.0`: `verified: 367 files`. `check-json 0.24.0`: `367 documents`.

4. **Findings**
   - **A1, blocker** (`crates/substrate-daemon/src/app/process_events.rs:286-294`). A stream without a cursor is refused `event.retention-gap` once the record's `process.started` (or end event) left event retention, for the rest of the record's retained life. Contradicts ADR 0036 S12 ("A stream opened on an ended record delivers what the retained log and the event log still hold, the terminal item, and closes"), `substrate-wire/src/lib.rs:2937` and the acceptance scenario `…-on-an-ended-record-replays-and-closes`. The refusal is retriable and names `address: "cursor"` on a request without one. Reached by the default `--event-retention` of 10,000 events per subject against one long-lived process in a busy subject.
   - **A2, warning, infeasible as observed** (`process_events.rs:542`). A reader attached while the record is `starting` receives a line before `process.started`, against `spec/host-processes/README.md:100`. The real driver copies the log from spawn (`host_process/mod.rs` `copy_lines`) before `mark_host_process_running`, and `GET /v1/processes?state=starting` lists the record; the window is one store write long. Built with a gate, not observed with the real driver.
   - **A3, warning** (`spec/host-processes/README.md:116`). The 512-stream subject claim is not enforced: `acquire` (`process_events.rs:76`) counts per record only, and 256 retained ended records also accept readers, so a subject can hold (64+256)×8 = 2560 streams, each polling every 100 ms. Overlaps security F2.
   - **A4, note** (`process_events.rs:206`). A line of exactly 16384 bytes with one non-UTF-8 byte is reported `truncated: true`, `full_bytes: 16384`, and loses its replacement character; the 0.24.0 schema reserves `truncated` for lines longer than the bound. The unit's own test asserts this behaviour.
   - **A5, note** (`process_events.rs:60`). No case catches `process.vanished` removed from `END_TRANSITIONS`, the mid-stream 1011 on removed history, the 1008 control flood, the lifetime limit, or `read_log` treating NotFound at offset 0 as an empty log.

5. **Attacked, could not break:** exit mid-stream, two readers and a restart resume each give one terminal item; cursor offset and seq hold across a gap and a resume; 16 KiB, 16 KiB + 1 and a final line without newline are right; an abandoned handshake frees its slot; bundle 0.24.0 is a fixed point and `ADVERTISED_CONTRACT_BUNDLE_SHA256` matches its inner `bundle.json`; the compatibility counts (44 preserved + 1 added) match `operations.json`.

```findings
[{"file":"crates/substrate-daemon/src/app/process_events.rs","line":286,"category":"acceptance","severity":"blocker","verdict":"NEEDS-CHANGE","origin":"introduced","message":"A stream without a cursor is refused event.retention-gap (retriable, address cursor) once the record's process.started left event retention, so an ended record never replays and a running one can never be watched again, against ADR 0036 S12 and ProcessEventsQuery's own doc."},
{"file":"crates/substrate-daemon/src/app/process_events.rs","line":542,"category":"concurrency","severity":"warning","verdict":"INFEASIBLE","origin":"introduced","message":"A reader attached while the record is starting receives the program's first line before process.started, breaking the unit's own rule that process.started precedes every line; shown with a gated driver only."},
{"file":"spec/host-processes/README.md","line":116,"category":"contract-drift","severity":"warning","verdict":"CONFIRMED","origin":"introduced","message":"The claimed 512-stream subject limit ignores the 256 retained ended records that also accept readers; acquire counts per record only, so a subject can hold 2560 streams each polling the store every 100 ms."},
{"file":"crates/substrate-daemon/src/app/process_events.rs","line":206,"category":"boundary","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"A line of exactly 16384 bytes containing a non-UTF-8 byte is reported truncated with full_bytes 16384 and loses its replacement character, though the schema reserves truncated for lines longer than the bound."},
{"file":"crates/substrate-daemon/src/app/process_events.rs","line":60,"category":"mutant","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"No case catches process.vanished removed from END_TRANSITIONS, the mid-stream 1011 on removed history, the 1008 control flood, the lifetime limit, or read_log's NotFound-as-empty-log at offset 0."}]
```
