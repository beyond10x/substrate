---
format: aep.planning-md/3
id: review-result:ops-listener-adversary-pass-1
kind: review-result
status: active
title: Adversary pass on the operational listener, pass 1 (NEEDS-CHANGE)
relations:
- reviews: story:operational-health-and-telemetry
revision: 1
---
unit: commit cd5f2b48 plus uncommitted test files, worktree substrate-ops-telemetry (story/operational-health-and-telemetry)
verdict: NEEDS-CHANGE
cases: executed 12→19, red 5 (the 12 counts the 2 red cases from security.rs; my module was left out with `--skip=adversary_ops_listener`)
origin: introduced 4 / pre-existing 0 / undecided 0
wrote-outside-worktree: 4 paths, all under `<scratch>/rev-adv/`
needs-coordinator: none

**1. Diff.** `git diff --stat` shows `tls_listener.rs | 5 +` (the mod lines, already there when I started) and `.engineering/planning/story/unrecorded-terminal-streaming.md | 8` (not mine: another agent's planning write). My only file is the untracked `crates/substrate-daemon/tests/ops_listener/adversary.rs` (431 lines). I changed no implementation line and ran no source mutant.

**2. Cases** (`adversary_ops_listener::`, each run alone first):
- `adversary_every_refused_request_is_counted_among_the_refusals`, RED: `left: 0 right: 1 … a refused answer is in substrate_http_requests_total but in no substrate_refusals_total series`. A development TCP 401 has no ErrorDetail.
- `adversary_a_method_not_allowed_on_the_unix_listener_is_counted_among_the_refusals`, RED: `left: 0 right: 1`. Production Unix posture: axum's default 405 has no ErrorDetail.
- `adversary_a_database_stamped_by_a_newer_binary_is_not_ready`, RED: `a database stamped 2 was re-stamped 1 at open and reported ready: "store ok\nschema ok\nmaintenance ok\ndriver ok\n"`.
- `adversary_a_required_fact_outside_the_capability_facts_is_refused_at_startup`, RED: `a misspelt fact was accepted; readiness is 503 "…driver failed\n" for the daemon's life`.
- `adversary_the_mapped_spelling_of_the_control_address_is_refused_by_name`, RED: `refused under another name: Error: bind authenticated TCP listener`. Before I tightened the assertion it passed once in the full suite, apparently through a port collision.
- `adversary_dispatch_counts_one_admission_and_no_replay`, GREEN. One workspace create plus its replay give `dispatch_total … 1` and `requests … ok 2`.
- `adversary_the_ninth_connection_is_closed_and_the_header_deadline_frees_slots`, GREEN (2.21 s).

**3. Suite** (`cargo test -p b10x-substrate-daemon --locked --test tls_listener`): `test result: FAILED. 12 passed; 7 failed; 0 ignored; 0 measured; 0 filtered out; finished in 5.80s` and `EXIT=101`. The 7 are my 5 plus security's `a_store_stamped_higher_is_not_silently_restamped` and `address_rules_refuse_every_spelling_of_the_control_address`. With my module skipped: `10 passed; 2 failed; … 7 filtered out` and `EXIT=101`.

**4. Findings**

| id | sev | file:line | verdict | what reaches it | fix |
|---|---|---|---|---|---|
| A1 | warning | `crates/substrate-daemon/src/app/health.rs:846` | NEEDS-CHANGE / introduced | any 405 on Unix or TLS, any dev-TCP 401 | count `other` when the outcome is a failure and no ErrorDetail is attached |
| A2 | warning | `crates/substrate-store/src/schema.rs:286` | NEEDS-CHANGE / introduced | rolling back onto a volume a newer binary migrated (no newer binary exists yet, but 1 is the first stamp, so the guard has to ship now) | read `user_version` before migrating and refuse when it is above `SCHEMA_VERSION` |
| A3 | note | `crates/substrate-daemon/src/runtime.rs:1844` | CONFIRMED / introduced | an operator typo in `--ops-require-fact`; the pod never becomes ready | check names against the `CapabilityFacts` key set |
| A4 | note | `crates/substrate-daemon/src/runtime.rs:1828` | CONFIRMED / introduced | `--ops-listen [::ffff:127.0.0.1]:p` next to `127.0.0.1:p`; still refused, but by the generic bind error | compare `ip().to_canonical()` |

Not caught by the suite before this pass (reasoned, not executed): removing `dispatch_admitted()` (no test asserted `substrate_dispatch_total` until my green case); changing `OPS_MAX_CONNECTIONS` or `OPS_HEADER_DEADLINE` (nothing tested them until my green case and security's).

Judgement only, no test: a render racing between `bucket++` and `count++` in `Histogram::observe` (`health.rs:377-379`) can publish a finite bucket above `+Inf`. It needs concurrency to show, so I left it untested.

**5. Held up under attack:** a strict OpenMetrics parser (`prometheus_client`) accepted 578 samples with observations in the histograms; HTTP/1.0, absolute-form and `?x` answer 200; `//livez`, `%6c`, OPTIONS `*`, CONNECT and lowercase `get` answer 404; HTTP/0.9 answers 400; pipelining and `Expect` give one answer; h2c is closed; replays and in-flight duplicates of an operation id are not counted as dispatches; an output stall is counted once per execution; Unix requests are counted once; label values come only from closed sets.

```findings
- file: crates/substrate-daemon/src/app/health.rs
  line: 846
  category: acceptance
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: a failure answer without ErrorDetail (Unix/TLS 405, dev-TCP 401) is counted as refused in substrate_http_requests_total but in no substrate_refusals_total series
- file: crates/substrate-store/src/schema.rs
  line: 286
  category: acceptance
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: open re-stamps user_version unconditionally, so a database a newer binary stamped reads back as SCHEMA_VERSION and the schema readiness check reports ok
- file: crates/substrate-daemon/src/runtime.rs
  line: 1844
  category: boundary
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: a well-shaped --ops-require-fact name outside the closed CapabilityFacts set is accepted and leaves the daemon unready for life
- file: crates/substrate-daemon/src/runtime.rs
  line: 1828
  category: boundary
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: the IPv4-mapped spelling of the control address passes the address rule and is refused only by a generic bind error, not ops.listener-config-invalid
```
