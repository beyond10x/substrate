---
format: aep.planning-md/3
id: review-result:pod-sweep-adversary-pass-1
kind: review-result
status: active
title: 'Adversary pass on the pod Unknown cleanup and open sweep, pass 1 (needs change: age floor)'
relations:
- reviews: story:pod-unknown-cleanup-outlives-adapter
revision: 1
---
unit: commit 88be48c6 on story/pod-unknown-cleanup-a4, plus the reviewer's uncommitted test files
verdict: NEEDS-CHANGE
cases: executed 40→46, red 2
origin: introduced 2 / pre-existing 0 / undecided 0
needs-coordinator: none

1. **Diff stat:** test files only — `src/confinement/pod_cleanup_tests.rs` (mod line), `tests/adversary_pod.rs` (mod lines), new `src/confinement/pod_cleanup_adversary.rs` and `tests/adversary_pod/adversary_pass1.rs`.

2. **Cases added** (`cargo test -p b10x-substrate-host --release --locked --test adversary_pod -- adversary_pass1`):
   - **ADV1-1, red:** `adv1_open_keeps_an_owned_policy_whose_run_may_still_be_starting` (`crates/substrate-host/src/confinement/pod.rs:763`, warning). The open sweep deletes owned policies aged 30 s and 45 s whose Pod does not exist yet. `start` creates the policy first (up to 30 s for that answer) and the Pod only then (up to another 30 s); both timestamps are whole seconds. A policy up to about 61 s old can therefore still be the only fence of a run another adapter is starting, and if that Pod create succeeds the run lives with no default-deny policy, against D124's "never removes". Reached with two adapters on one namespace (design 28 assumes this) and a slow Pod create, for example admission webhooks. Fix: an age floor of at least `2 * API_TIMEOUT + 1 s`. Output: `left: [] right: ["networkpolicies/substrate-run-fortyfive", "networkpolicies/substrate-run-thirty"]`.
   - **ADV1-2, red:** `adv1_open_never_removes_the_policy_of_a_pod_that_exists` (`pod.rs:790`, note, infeasible). The sweep checks the Pod named by the run label but deletes the policy by its own name, so a policy named `substrate-run-live` labelled `run=substrate-run-gone` is deleted while Pod `substrate-run-live` runs. Nothing in the adapter creates such a policy; it needs a third party with patch rights on policies. Fix: `owned()` requires the name to equal the run label.
   - **Green:** age is read by the server `Date` header in any RFC 3339 form (catches a host-clock mutant); a 404 on the policy delete still opens; a 500 on the Pod read after the list is refused `confinement.adapter-unserved`; the pause between removal tries stops doubling at its ceiling (10 deletes in under 2.5 s; catches dropping `.min(limit)`).

3. **Suite after the cases existed:**
   - `--test adversary_pod`: `test result: FAILED. 17 passed; 3 failed; ... finished in 122.07s`. The third red is the security reviewer's `security_a_pod_whose_start_succeeds_keeps_its_policy_through_another_adapters_open`, the same defect as ADV1-1.
   - `--lib -- confinement::pod`: `test result: ok. 5 passed; 0 failed`.
   - `--test ess_confinement_pod`: `ok. 14 passed`. `--test security_review_pod`: `ok. 7 passed`.

4. **Mutants the suite did not catch before this pass** (by arithmetic, not run): M1 drop `.min(limit)` at `pod.rs:576` (now caught); M5 bounding `remove_until_gone` at 3 or more rounds (no finite fake rules it out); M6 age by the host clock (now caught). `security_review_pod.rs:522` counts three open requests without pinning the third as the sweep's list; `ess_confinement_pod` pins it.

5. **Attacked, could not break:** the A4 loop ends on the first 404 from delete or read; 409, 410 and 5xx retry without overflow with the pause capped at 30 s; a run that keeps reading Unknown keeps retrying the policy removal; production cannot build a non-default `Pacing`; a missing labels map, extra fields, fractional or offset `creationTimestamp` and a nameless item are handled; a 403 on the list is refused.

```findings
[{"file":"crates/substrate-host/src/confinement/pod.rs","line":763,"category":"acceptance","severity":"warning","verdict":"NEEDS-CHANGE","origin":"introduced","message":"the sweep's 30 s age floor equals one API timeout, but start's policy create plus Pod create can take 60 s and timestamps are whole seconds, so open removes the fence of a run another adapter is still starting"},
 {"file":"crates/substrate-host/src/confinement/pod.rs","line":790,"category":"boundary","severity":"note","verdict":"INFEASIBLE","origin":"introduced","message":"the sweep checks the Pod named by the run label but deletes the policy by its own name, so a policy whose name and label differ is removed while its namesake Pod runs; nothing in the adapter creates such a policy"}]
```
