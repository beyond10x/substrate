---
format: aep.planning-md/1
id: review-result:container-runtime-checker-security-review
kind: review-result
status: active
title: Runtime admission fixes and PTY checker review
relations:
- reviews: story:container-private-exec-bootstrap
revision: 1
---
needs-revision

Independent read-only re-review of the bootstrap/design correction and the new public-SDK PTY checker over base `1ceb11bbaa38221ecf3e48a62d41271d9ae57d0f`. The proc propagation correction is accepted, and optional io delegation is consistent with the unchanged host observation path. Three checker findings remain. This report binds the source inspected before the follow-up checker edits; earlier reports remain unchanged.

1. **Synchronize the background descendant before the cleanup proof.** `crates/b10x-substrate-sdk/examples/container-pty-check.rs:32` prints the acknowledged sentinel before executing `sleep 600 & wait`. The reader can observe that sentinel, complete its other requests and kill the shell before the background process has been created. The cgroup-member assertion at line 64 requires only a nonempty group, so shell/launcher membership satisfies it. This permits a run to claim whole-tree cleanup without exercising the intended surviving background descendant. Create and verify the background process before emitting the readiness acknowledgment, and establish that this known descendant is alive before the kill and absent afterward. The existing resize/input sentinel alone is not that synchronization.

2. **Keep cleanup reachable when a checker assertion fails.** Assertions after workspace creation, including the file equality check at `crates/b10x-substrate-sdk/examples/container-pty-check.rs:23` and worker capability/metrics assertions at lines 57–64, panic rather than return the `Result` stored at line 21. A panic bypasses the later `workspace.destroy()` at line 75. It can also bypass the session kill at line 65, so a failed conformance assertion leaves its owned execution/workspace to timeout or external teardown instead of performing the checker's explicit cleanup. Convert post-create assertions into returned errors and retain sufficient ownership to attempt bounded session termination and workspace destruction on every ordinary failure path. Preserve cleanup failures in the reported result. This concerns only test-owned resources; no user workspace was accessed by this review.

3. **Require an observed metrics sample.** `crates/b10x-substrate-sdk/examples/container-pty-check.rs:60` accepts any `MetricsObservation::Exec { .. }`. Its `usage` field is an `ExecUsage` union that also permits `Pending` and `Unavailable` (`crates/substrate-wire/src/lib.rs:1633`; exposed by `crates/b10x-substrate-sdk/src/model.rs:569`). Therefore this check can claim metrics passed without receiving a resource measurement. Require the requested exec identity and `ExecUsage::Observed`; poll pending state only within a bounded deadline and reject unavailable usage. This is especially relevant because the checker is the deciding proof for the newly enabled io controller.

The bootstrap now reads mountinfo once and checks both mount targets before any mutation (`crates/substrate-daemon/src/container_bootstrap.rs:77`). The new proc validator requires exactly one `/proc` entry, filesystem root `/`, proc filesystem type, nosuid/nodev/noexec, and exactly six pre-separator fields. Shared/master/unbindable metadata, duplicate or absent entries, subtree roots, missing flags and the wrong filesystem are rejected. Its focused negative fixtures cover these cases (`crates/substrate-daemon/src/container_bootstrap.rs:406`). Runtime mask submounts remain untouched until the controlled overlay. This resolves the prior expanded review's propagation blocker.

`make_delegation` enables io only when it appears in the available controller list, together with cpu/memory/pids after the root becomes process-free (`crates/substrate-daemon/src/container_bootstrap.rs:260`). This makes child io observations available on the admitted host without changing the existing host probe or measurement implementation. Controller activation does not rewrite parent resource ceilings or extend the explicit ownership-transfer list. The absence of io continues through the existing capability/probe behavior. The earlier ownership, executable-mode, post-drop write-denial and capability checks remain intact.

The Docker checker target contains the standalone public-SDK example, runs as UID/GID 65532 and is separate from the published daemon/MCP targets. The shared builder compiles it with locked dependencies. Its direct cgroup and proc observations require it to run in the tested daemon's corresponding namespace views; launching a separate checker container with only the Unix socket is not sufficient evidence for those observations. The checker does not link the optional daemon implementation feature. No additional concrete Docker packaging blocker was found in this bounded re-review.

Retained `bootstrap-tests-proc-revision.log` records all four bootstrap tests passing, and `container-tools-clippy-revision.log` records successful targeted binary Clippy completion. These logs do not constitute a completed run of the revised final image or PTY checker. Real-image rebuild, the interactive PTY/metrics/cleanup journey, installer validation, complete gates and deployment acceptance remain pending evidence. The unchanged AppArmor/seccomp/installer findings and limitations from the expanded review still apply; no fresh live test, build or mutation was performed here.

Reviewed file SHA-256 identities:

```text
f5b33660450aa7ea6c70a439aaa377aef0b4ccfdb5c1aa9d14b73ea0ad4f834d  Dockerfile
daf89cb35b753041e08e5e46b0190992e7e5236be9195d636a7e7e0392bde64c  docs/design/25-container-private-host-execution.md
0482fa14ef3803ef39dd702f79d0aa034ccd1b18533f90776692a951db2e68f9  crates/substrate-daemon/src/container_bootstrap.rs
319f27aa4489395086008ccf65fbed7720a3d6cf22d050129e54de53c72baa81  crates/b10x-substrate-sdk/examples/container-pty-check.rs
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
[
  {
    "file": "crates/b10x-substrate-sdk/examples/container-pty-check.rs",
    "line": 32,
    "category": "testing",
    "severity": "blocker",
    "message": "Readiness output precedes creation of the intended background descendant, and the later membership assertion requires only a nonempty cgroup. Synchronize and verify a known live descendant before killing the session so the whole-tree cleanup proof cannot skip that case."
  },
  {
    "file": "crates/b10x-substrate-sdk/examples/container-pty-check.rs",
    "line": 23,
    "category": "correctness",
    "severity": "warning",
    "message": "Post-create assertions panic past explicit session/workspace cleanup. Return validation errors through Result and attempt bounded termination and destruction for test-owned resources on failure, retaining cleanup errors."
  },
  {
    "file": "crates/b10x-substrate-sdk/examples/container-pty-check.rs",
    "line": 60,
    "category": "testing",
    "severity": "blocker",
    "message": "Accepting any Exec metrics variant also accepts Pending or Unavailable usage. Require matching exec identity and an Observed resource sample before claiming the io/metrics check passed."
  }
]
```
