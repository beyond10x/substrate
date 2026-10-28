---
format: aep.planning-md/3
id: review-result:ops-listener-security-pass-1
kind: review-result
status: active
title: Security review of the operational listener, pass 1 (NEEDS-CHANGE)
relations:
- reviews: story:operational-health-and-telemetry
revision: 1
---
unit: commit cd5f2b48 (operational listener, ADR 0045) in the substrate-ops-telemetry tree, plus my untracked `tests/ops_listener/security.rs`
verdict: NEEDS-CHANGE
cases: executed 7→12, red 2
origin: introduced 3 / pre-existing 0 / undecided 0
wrote-outside-worktree: 2 (`<scratch>/rev-sec/run.sh` and its `tmp/` directory)
needs-coordinator: whether ADR 0045 should add a per-source cap on the ops listener (finding S3)

**1. Diff.** `git diff --stat` shows only two changed files, and neither is mine: `crates/substrate-daemon/tests/tls_listener.rs` (+5, the module wiring that was already there) and `.engineering/planning/story/unrecorded-terminal-streaming.md` (+6/−2, another session's work; there is also an untracked evidence JSON from that session). I wrote only `crates/substrate-daemon/tests/ops_listener/security.rs` (untracked, 225 lines). I did not touch any implementation file.

**2. Cases added**, each run alone first:
- `ops_security_a_store_stamped_higher_is_not_silently_restamped`: **RED**. Tail: `opened a store stamped 2 and rewrote the stamp to 1` / `test result: FAILED. 0 passed; 1 failed` / `EXIT=101`
- `ops_security_an_unstamped_store_opens_and_is_stamped` (a store as 0.14.0 left it, `user_version` 0; the 0.14.0 `schema.rs` never sets it): green. `1 passed` / `EXIT=0`
- `ops_security_address_rules_refuse_every_spelling_of_the_control_address`: **RED**. Port 0, `0.0.0.0:p` and `[::]:p` are refused correctly. `[::ffff:127.0.0.1]:p` is not. Tail: `[::ffff:127.0.0.1]:41679: Error: tls.listener-config-invalid: bind production TLS listener / Caused by: Address already in use (os error 98)` / `EXIT=101`
- `ops_security_a_unix_socket_path_is_not_an_operational_address`: green. clap refuses the path and nothing is bound. `1 passed` / `EXIT=0`
- `ops_security_connection_bounds_hold_on_the_shipped_daemon`: green. With 8 connections held, a 9th gets no answer. The 8 close by the 2 s header deadline. Pipelined requests get one answer (keep-alive is off). 40 headers or a 20 KiB head is not served. `1 passed; ... finished in 2.72s` / `EXIT=0`

**3. Suite.** `cargo test -p b10x-substrate-daemon --release --locked --test tls_listener`:
`test result: FAILED. 10 passed; 2 failed; 0 ignored; 0 measured; 0 filtered out; finished in 5.75s` / `EXIT=101`. The two failures are the red cases above.
The same run with `--skip security_ops_listener`: `test result: ok. 7 passed; 0 failed; ... 5 filtered out` / `EXIT=0`
`cargo xtask check-bundles 0.23.0`: `contract bundle 0.23.0 verified: 361 files, fixed point of xtask/bundle-source/0.23.0`. The commit does not touch `contracts/`, `xtask/` or `substrate-wire`.

**4. Findings**

| id | sev | file:line | test | verdict / origin | fix |
|---|---|---|---|---|---|
| S1 | warning | crates/substrate-store/src/schema.rs:286 | `..._stamped_higher_is_not_silently_restamped` (red) | NEEDS-CHANGE / introduced | Refuse to open a store when `user_version > SCHEMA_VERSION` (a named refusal), and never lower the stamp |
| S2 | note | crates/substrate-daemon/src/runtime.rs:1828 | `..._address_rules_refuse_every_spelling...` (red) | CONFIRMED / introduced | Compare the two addresses after `to_canonical()` (unmapping IPv4-mapped IPv6), so the refusal is `ops.listener-config-invalid` |
| S3 | warning | crates/substrate-daemon/src/runtime.rs:1895 | `..._connection_bounds_hold_on_the_shipped_daemon` (green, shows the mechanism) | CONFIRMED / introduced | Add a small per-source cap, or reserve slots, so a single peer cannot hold all 8 |
| S4 | note | crates/substrate-daemon/src/app/health.rs:934 | none (judgement) | INFEASIBLE / introduced | Document that the 1 s bound covers waiting for the lock, not the statement; or give the probe its own short `busy_timeout` |

- **S1, what it means.** Reached by any rollback to an older daemon. The old binary runs its migrations over the newer schema, rewrites the stamp to 1, and `/readyz` reports `schema ok`. The schema check can then fail only if something else writes the stamp while the daemon runs. So it does not detect incompatibility (acceptance: "schema compatibility"), and it erases the evidence of the newer binary. Opening a 0.14.0 store still works.
- **S2, what it means.** It fails closed, but under the wrong name. The ops listener binds first and, for a few milliseconds, holds the control port in plaintext. Reached only by an operator who spells the control address in the mapped form.
- **S3, what it means.** The bound works as the ADR decided, so this is a design gap rather than a bug. Any peer that can reach the port and reconnects every 2 s keeps all 8 slots full. Kubelet `/livez` then goes unanswered and the daemon is restarted, which drops live sessions, the cost ADR D5/§5 says liveness must avoid. NetworkPolicy is the only mitigation, and ADR 0046 is still proposed.
- **S4, what it means.** `try_lock_for(1s)` (`schema.rs:303`) bounds only the wait for the lock. The PRAGMA can wait up to the 5 s `busy_timeout`. The verdict stays correct (the elapsed time is over 1 s, so `store failed`), but the answer can arrive after kubelet's 1 s timeout. Reaching it needs a foreign process with the daemon's uid to lock the WAL store; I found no in-daemon path.

**5. Cleared (code read and tests):**
- Only `/livez`, `/readyz` and `/metrics` are served, by a separate `service_fn`, not `router_for`. Every registry route returns 404, and so do other methods, bodies and upgrades (existing test).
- No `Authorization` header is read. Identity accepts stay at 0.
- The `hosted.rs` edits: `required_scope` already ignored its method argument, so dropping it changes nothing; the `ErrorDetail` it now inserts is a response extension, which is never serialized to the client; the admission decision itself is unchanged.
- Labels are all `&'static str` from enums or wire constants. I recomputed the series count: 1+4+90+210+107+30+70+9+1+2+28+2+5+14+5 = 578.
- The listener is off by default (`Option`, no default value).
- The `block_in_place` probe runs only on the multi-thread `#[tokio::main]`. The SDK's linked child never sets `ops`. The probe holds the store lock for one PRAGMA only.

```findings
[
  {"file": "crates/substrate-store/src/schema.rs", "line": 286, "category": "integrity", "severity": "warning", "verdict": "NEEDS-CHANGE", "origin": "introduced", "message": "Store::open overwrites a higher user_version with 1, so a rollback reports schema ok and the readiness schema check cannot detect a newer store"},
  {"file": "crates/substrate-daemon/src/runtime.rs", "line": 1828, "category": "boundary", "severity": "note", "verdict": "CONFIRMED", "origin": "introduced", "message": "the control address in IPv4-mapped IPv6 form passes check_ops_listener and fails later as tls.listener-config-invalid after the ops listener briefly bound the control port"},
  {"file": "crates/substrate-daemon/src/runtime.rs", "line": 1895, "category": "judgement", "severity": "warning", "verdict": "CONFIRMED", "origin": "introduced", "message": "the 8-slot pool has no per-source cap, so one peer reconnecting every 2 s starves kubelet liveness and gets the daemon restarted"},
  {"file": "crates/substrate-daemon/src/app/health.rs", "line": 934, "category": "judgement", "severity": "note", "verdict": "INFEASIBLE", "origin": "introduced", "message": "the 1 s store bound covers only the lock wait, while the PRAGMA can wait up to the 5 s busy_timeout"}
]
```
