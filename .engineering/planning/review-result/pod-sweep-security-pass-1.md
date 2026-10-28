---
format: aep.planning-md/3
id: review-result:pod-sweep-security-pass-1
kind: review-result
status: active
title: 'Security review of the pod Unknown cleanup and open sweep, pass 1 (needs change: sweep races start)'
relations:
- reviews: story:pod-unknown-cleanup-outlives-adapter
revision: 1
---
unit: commit 88be48c6 (open sweep plus A4 retry) in the working tree of story/pod-unknown-cleanup-a4
verdict: NEEDS-CHANGE
cases: executed 12→20, red 3 (1 this pass, 2 the adversary's)
origin: introduced 3 / pre-existing 1 / undecided 0
needs-coordinator: none

1. **Paths:** `crates/substrate-host/tests/adversary_pod.rs` (mod line) and new `crates/substrate-host/tests/adversary_pod/security_pass1.rs`. No implementation file changed.

2. **Cases added** (each with its own loopback fake):
   - `security_a_pod_whose_start_succeeds_keeps_its_policy_through_another_adapters_open`, **red.** A start's policy is stamped 29 s before its answer and its Pod create is held 2 s, inside its own 30 s bound; a second adapter opens meanwhile:
     ```
     the run started and its Pod pods/substrate-run-01m60skwjaxr424jyq21s4qxqg is running without its default-deny NetworkPolicy, which the second adapter's open sweep deleted (second open: Ok("opened"))
     test result: FAILED. 2 passed; 1 failed; 0 ignored; 0 measured; 12 filtered out; finished in 2.06s
     ```
   - `security_a_sweep_without_a_readable_server_date_refuses_open_and_deletes_nothing`, green: missing or garbled `Date` is `confinement.adapter-unserved`, nothing deleted.
   - `security_a_future_or_unnamed_policy_is_never_swept`, green: future `creationTimestamp`, empty and invalid run labels are kept.

3. **Suite** `cargo test -p b10x-substrate-host --release --locked --test adversary_pod`: without this module `ok. 12 passed`; with every module `FAILED. 17 passed; 3 failed; ... finished in 122.07s`. `rustfmt --check` on the file: 0. `cargo clippy -p b10x-substrate-host --all-targets --release --locked -- -D warnings`: 0.

4. **Findings**
   - **S1, blocker, introduced** (`crates/substrate-host/src/confinement/pod.rs:763`, test red). The 30 s age floor covers only the Pod create. `start_run` (`pod.rs:1199-1205`) makes two sequential creates, each bounded by 30 s, and the API server stamps `creationTimestamp` before admission and storage, so the Pod may be stored about 60 s after its policy. Another adapter's open in that window deletes the policy and the start still returns Ok with an unfenced Pod. Skew between API server members (the `Date` header and the timestamp may come from different members) widens it. Fix: after the Pod create, `start` reads its policy back and, if gone, removes the Pod and refuses; a floor of 2×API timeout plus margin alone still depends on clocks.
   - **S2, warning, introduced** (`pod.rs:1469-1470` with `pod.rs:1171`, judgement). An Unknown run is evicted from tracking while its removal task now loops forever; with a NotReady node whose Pods stay Terminating, every run sent there adds a permanent task (2 requests per 30 s), growing with runs started, not with the 128 tracked. Fix: count Unknown runs with cleanup pending against `POD_MAX_TRACKED_RUNS`, or drain them through one bounded queue.
   - **S3, note, introduced** (`pod.rs:736-738`). The policy list has no `limit`/`continue` paging. Past 4 MiB of owned policies the sweep fails closed and open is refused, consistent with invariant 3, but orphans then pile up while every open is refused; open time is linear in candidates. Fix: page with `limit`/`continue`.
   - **S4, warning, pre-existing** (`pod.rs:1205-1216`; same code at `cff3f219`:1080). If the Pod create times out and the server stores the Pod later, the DELETE of the not-yet-stored Pod answers 404, `start` reads it as gone and deletes the policy; the Pod then runs unfenced and untracked until `activeDeadlineSeconds`. Fix: an `ownerReference` from the Pod to its policy, so the garbage collector removes a Pod whose policy is gone.

5. **Checked and cleared:** scope is the configured namespace only and the owner label is re-checked client-side (`pod.rs:784`); the Pod is read by GET on the run-label name, so any phase including terminating keeps the policy; Pod name equals the run label (`pod.rs:954`, `pod.rs:1030`); list and GET carry no resourceVersion, so reads are consistent; a future timestamp counts as young; RBAC is unchanged and sufficient (`gate.yml:236-237`, design 28:107).

```findings
[{"file":"crates/substrate-host/src/confinement/pod.rs","line":763,"category":"concurrency","severity":"blocker","verdict":"NEEDS-CHANGE","origin":"introduced","message":"The 30 s age floor covers only the Pod create, so another adapter's open sweep can delete the default-deny policy of a run whose two sequential 30 s-bounded creates then succeed, leaving a running unfenced Pod."},
{"file":"crates/substrate-host/src/confinement/pod.rs","line":1469,"category":"judgement","severity":"warning","verdict":"CONFIRMED","origin":"introduced","message":"Unknown runs are evicted from tracking while their removal tasks loop forever, so tasks and API request rate grow with every run sent to a node that stays away."},
{"file":"crates/substrate-host/src/confinement/pod.rs","line":736,"category":"boundary","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"The unpaged 4 MiB policy list fails closed once owned policies exceed it, and orphans then pile up because every open is refused."},
{"file":"crates/substrate-host/src/confinement/pod.rs","line":1205,"category":"concurrency","severity":"warning","verdict":"CONFIRMED","origin":"pre-existing","message":"A Pod stored after the client's create timeout reads as gone on DELETE 404, so start deletes its policy and the Pod later runs unfenced and untracked."}]
```
