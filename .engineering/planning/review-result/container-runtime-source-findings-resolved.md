---
format: aep.planning-md/1
id: review-result:container-runtime-source-findings-resolved
kind: review-result
status: active
title: Final runtime and checker source review
relations:
- reviews: story:container-private-exec-bootstrap
revision: 1
---
approve

Independent read-only final source re-review over base `1ceb11bbaa38221ecf3e48a62d41271d9ae57d0f`. All concrete findings retained in the initial bootstrap, expanded runtime and checker reports are resolved in the reviewed source. No remaining actionable findings within this bounded review. This approves the source correction; it does not claim that the pending final-image PTY journey or deployment has passed.

The checker starts its background sleep, verifies it exists and writes its namespace PID before reading input or emitting the acknowledgment (`crates/b10x-substrate-sdk/examples/container-pty-check.rs:36`). Before killing the session, it requires at least two cgroup members and matches the acknowledged PID to an actual member's innermost NSpid and `sleep` comm (`crates/b10x-substrate-sdk/examples/container-pty-check.rs:83`). The subsequent bounded wait requires the cgroup and every captured member to disappear. This closes the earlier race where the test could kill the shell before creating its intended descendant.

The checker now uses returned validation errors rather than assertions. A cloned session handle remains owned across failures, allowing explicit termination before workspace destruction. The final correction places independent five-second deadlines around both cleanup requests (`crates/b10x-substrate-sdk/examples/container-pty-check.rs:103`): a Kill error or timeout does not prevent the workspace cleanup attempt, and errors from both attempts are logged. The success message occurs only after the main result, session cleanup and affirmative workspace destruction succeed. These are bounded cleanup attempts; a lost server response remains an error rather than invented proof of remote cleanup.

Metrics must refer to the exact requested execution and contain `ExecUsage::Observed`, with positive live memory and at least two current processes (`crates/b10x-substrate-sdk/examples/container-pty-check.rs:65`). Pending usage is polled only within a five-second deadline; unavailable usage and a mismatched resource fail. Worker capability and no_new_privs checks remain, and the status now names the shell itself. Direct proc/cgroup observations still require the checker to run in the tested daemon's corresponding namespace views.

The accepted bootstrap corrections are unchanged: preflight validates private, unambiguous cgroup and proc mounts from the same mountinfo read before any mutation; the exact AppArmor enforcement check precedes the fresh proc mount; enclosing control files and selected executables have protected root-owned regular-file metadata; all four ceiling files must deny write-only opens after the verified UID/capability drop. Optional io activation follows the available-controller list and supplies child observations for the unchanged host measurement implementation without rewriting parent ceilings or widening ownership delegation.

The previously inspected AppArmor/seccomp/installer and Docker packaging bytes remain as bound below. The policy is an explicit outer setup profile, with the unchanged host namespace, no-egress, non-nestable-userns, inner-seccomp and cgroup enforcement still required. The installer uses fixed compiled-in versioned bytes and protected atomic file publication. Docker's separate checker target remains a local conformance helper, separate from the published daemon/MCP targets. This review did not rerun builds or mutate any image, node, service, workspace, repository or planning record.

`container-pty-check-clippy-final.log` records successful Clippy for the final checker revision. Retained bootstrap evidence records four passing tests and successful targeted binary Clippy. `installer-first-install.log` records successful installation/enforcement, and `installer-unrecorded-profile-refusal.log` records refusal of an already-loaded profile lacking its exact protected source. The coordinator additionally reports a successful read-only installer check; that report is distinguished from the two logs independently inspected here.

The latest actual PTY journey is still under investigation after a five-second timeout. No completed final-image PTY, metrics or whole-tree cleanup pass is asserted by this review. Complete repository gates, ordinary/quota image checks, real-node profile verification and negative cases, successful wire PTY acceptance, image publication binding, downstream deployment review and preserved-workspace acceptance remain required evidence before release/deployment completion. Earlier review reports remain immutable.

Reviewed file SHA-256 identities:

```text
d98eeed85115966d6bc720d897cbf22b442adcc8153ed465644b3a81bd0c110b  crates/b10x-substrate-sdk/examples/container-pty-check.rs
f5b33660450aa7ea6c70a439aaa377aef0b4ccfdb5c1aa9d14b73ea0ad4f834d  Dockerfile
daf89cb35b753041e08e5e46b0190992e7e5236be9195d636a7e7e0392bde64c  docs/design/25-container-private-host-execution.md
0482fa14ef3803ef39dd702f79d0aa034ccd1b18533f90776692a951db2e68f9  crates/substrate-daemon/src/container_bootstrap.rs
10f656ee4512de004d6b3b67babacc400cf53e9f30a7573d8edf76ecfd4286a1  crates/substrate-daemon/src/bin/substrate-container-exec.rs
9704c0716993556e07948089c5a5ca141df1a169ec31c7c731d0a32226f60806  crates/substrate-daemon/src/container_profiles.rs
eb37dc3f027961098ee87545eb55007ef2473c6142dd0f75ce1baf5bbdd8db3d  crates/substrate-daemon/src/bin/substrate-container-profiles.rs
e3ff9b10eb633585354cedbcbe1dafb4ec01c4296ea1085677221adca3b6843a  deploy/apparmor/substrate-host-execution
1ee2d9917369913565607cb93c99a38728756d9e7adc70762786983d064ee8b5  deploy/seccomp/host-exec-v1-amd64.json
7a5552479c10015907fa6035ec35f62b0d6cbf1099b7e32e3ee47c4739bca387  deploy/seccomp/schemas/host-execution.schema.json
30c45da4eb77f3be3dc5f33220207bcdccaf385d9c6aaf7852dd5bc59068564e  deploy/seccomp/README.md
4a1b271a92f30d4aafe1fbf34483924d81f2170337852fad86afbade1b30436a  xtask/src/container_profiles.rs
551446cb6f2339d8f9e1880fabc186ac86947c78d83260871d04b52a5e163593  xtask/src/main.rs
```

```findings
[]
```
