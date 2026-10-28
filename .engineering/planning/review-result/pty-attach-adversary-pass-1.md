---
format: aep.planning-md/3
id: review-result:pty-attach-adversary-pass-1
kind: review-result
status: active
title: Adversary pass on the pty attach refusal, pass 1 (CONFIRMED, one pre-existing race)
relations:
- reviews: story:pty-session-refuses-first-attach-under-load
revision: 1
---
unit: commit d548a2e1 on story/pty-attach-names-condition, working tree substrate-wave-pty-attach plus my two test edits (uncommitted)
verdict: CONFIRMED (one red case; the defect predates this commit)
cases: executed 45→46, red 1
origin: introduced 1 / pre-existing 2 / undecided 0
wrote-outside-worktree: 12 paths, all in the assigned scratch rev-u2
needs-coordinator: whether this story or a new story owns F1

1. `git diff --stat`: `crates/substrate-daemon/tests/pipe_session.rs | 3 +++`. I also added the untracked file `crates/substrate-daemon/tests/pipe_session/adversary_pass1.rs`. Both are test files; I mutated no implementation file.

2. **Case added:** `adversary_pass1::adversary_a_child_that_exits_before_the_start_commits_is_not_left_ready`, red now.
   - **Setup:** a wrapper around `PipeFixtureDriver` forgets an exact terminal observation once the daemon acknowledges it, and then answers `observe_exec` with not-found. The real host driver does the same (`substrate-host/src/process.rs:1680`, `:1652`). The wrapper runs one `App::sweep_expired` inside `set_exec_lease`, the driver call the start makes between dispatch and `complete_pipe_session_start`. A precondition asserts that the sweep persisted the exit and the daemon acknowledged it.
   - **Order disclosure:** the first run of this case was green, because my wrapper still let `observe_exec` see the retired exec, and a GET healed the state. I fixed the fixture to behave like the host driver, and the reruns are red. The tail below is from the third run of the case alone:
   ```
   assertion `left == right` failed: the driver reported this exec exited and the daemon acknowledged that observation; the session must not read ready and attachable, nor the exec running. ...
     left: (String("ready"), String("available"), String("running"))
    right: (String("exited"), String("consumed"), String("exited"))
   test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 45 filtered out; finished in 0.19s
   EXIT=101
   ```

3. **Suite runs, after the case existed:**
   - `cargo test -p b10x-substrate-daemon --release --locked --test pipe_session`: `test result: FAILED. 45 passed; 1 failed; ... finished in 5.61s` and `EXIT=101`
   - The same run with `-- --skip adversary_pass1`: `test result: ok. 45 passed; 0 failed; ... 1 filtered out` and `EXIT=0`
   - `cargo xtask check-bundles 0.23.0`: `contract bundle 0.23.0 verified: 361 files, fixed point of xtask/bundle-source/0.23.0` and `EXIT=0`
   - `--test runtime_vectors`, portable lane: `test result: ok. 11 passed; 0 failed` and `EXIT=0`
   - `cargo clippy -p b10x-substrate-daemon --all-targets --release --locked -- -D warnings`: `EXIT=0`
   - `cargo fmt --all --check` first flagged only my import list. After `rustfmt` on my file it gives `fmt EXIT=0`.

4. **Findings**

| id | sev | file:line | test | verdict / origin | fix |
|---|---|---|---|---|---|
| F1 | warning | crates/substrate-store/src/sessions.rs:499 (called from operations.rs:1284) | adversary_a_child_that_exits_before_the_start_commits_is_not_left_ready (red) | CONFIRMED / pre-existing | In `complete_pipe_session_start`, load the stored exec inside the transaction. If it is already terminal, keep it and project the session from it, as `complete_pipe_session_observation` does, instead of writing the stale `running` exec and a `ready` session. |
| F2 | note | crates/substrate-daemon/src/app/sessions.rs:966 | none (hosted transport is `pub(crate)`) | CONFIRMED / pre-existing | Under the hosted posture the first `session.not-attachable` comes from the attachment-authority mint, and it still says "The session is not ready to mint an attachment authority." without naming the failed condition. Reuse `not_attachable_message` there. |
| F3 | note | .engineering/planning/story/pty-session-refuses-first-attach-under-load.md | none | CONFIRMED / introduced | The story's acceptance also asks that start return an attachable pty session (or that the SDK wait for readiness), held by a delegated case. This commit only adds the diagnosis, so the story must not close on it. |

**What reaches F1:** the background lease sweeper (`runtime.rs:1730`) and every GET or attach call `sweep_expired`, and nothing stops them running while a start is in flight. After the host driver retires the exec, nothing reports the exit again. The session then reads `ready` and `available` over a dead child until its lease expires. I built the interleaving with the wrapper. I have no runtime observation of it on a real host.

5. **Attacked, could not break:** the pre-check message is built from the same snapshot it tests; the claim path's second read is scoped to the caller's subject, carries only the mode and three enum words, and falls back to the generic sentence with 409 if the session vanished or the store fails (no test runs that branch; no mutant run); code, status, class and `retriable` are unchanged and the longest message is about 190 characters against `maxLength` 1024; the SDK read-back cannot panic before the main message.

```findings
[
  {"file": "crates/substrate-store/src/sessions.rs", "line": 499, "category": "concurrency", "severity": "warning", "verdict": "CONFIRMED", "origin": "pre-existing", "message": "complete_pipe_session_start writes the start's stale running exec and a ready session over an exit a sweep already persisted and the host driver retired, so the session stays ready and attachable over a dead child"},
  {"file": "crates/substrate-daemon/src/app/sessions.rs", "line": 966, "category": "judgement", "severity": "note", "verdict": "CONFIRMED", "origin": "pre-existing", "message": "the hosted attachment-authority mint still answers session.not-attachable without naming the failed condition, so under the hosted posture the first refusal stays undiagnosable"},
  {"file": ".engineering/planning/story/pty-session-refuses-first-attach-under-load.md", "category": "acceptance", "severity": "note", "verdict": "CONFIRMED", "origin": "introduced", "message": "the commit delivers only the diagnosis; the acceptance's attachable-on-start or SDK-waits clause and its delegated case are not met"}
]
```
