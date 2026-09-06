---
format: aep.planning-md/1
id: review-result:container-runtime-profile-security-review
kind: review-result
status: active
title: Independent container runtime profile security review
relations:
- reviews: story:container-private-exec-bootstrap
revision: 1
---
needs-revision

Independent read-only review of the expanded container runtime sources, policy files, schema and gate registration over base `1ceb11bbaa38221ecf3e48a62d41271d9ae57d0f`. One new actionable bootstrap finding. The two findings resolved in `bootstrap-independent-review-revision.md` remain resolved. No source, planning, image, node, service or workspace mutation was performed during this review.

**Validate the proc mount's propagation before replacing it.** `crates/substrate-daemon/src/container_bootstrap.rs:242` mounts a new proc filesystem over `/proc`, but preflight at line 77 validates only the `/sys/fs/cgroup` mount. A private cgroup mount and cgroup namespace can coexist with a shared `/proc` mount. PID 1 and the enforced AppArmor label do not establish private mount propagation. A mount onto a shared mount can propagate to its peers, including peers in another mount namespace, so this accepted startup state violates the promised container-private mutation boundary. Require an unambiguous private proc mount rooted and typed as expected before any mutation; refuse shared, propagated, missing or ambiguous proc mount observations. Preserve the existing masked child mounts until the controlled overlay. Add pure negative fixtures for an otherwise valid cgroup view with unsafe `/proc` propagation, and include this refusal in the disposable runtime validation. This is a conditional admission defect, not an observation that the current local or deployed container uses shared proc propagation. [Kernel shared-subtree semantics](https://www.kernel.org/doc/html/latest/filesystems/sharedsubtree.html)

The reviewed bootstrap otherwise retains the earlier executable/ceiling ownership protections and post-drop write-open denial. Its new exact enforced AppArmor-label check precedes cgroup and proc mutations, and the fresh proc mount uses fixed source, target and filesystem with nosuid/nodev/noexec. The final daemon exec remains fixed, with supplementary groups cleared, UID/GID 65532 verified, ordinary capabilities empty and only the existing SYS_ADMIN quota bounding allowance retained. The fresh proc mount is a deliberate removal of runtime mask submounts; it consequently depends on the reviewed policy and all existing worker confinement checks remaining effective.

The AppArmor policy has one inherited label and no relaxing exec transition or profile-change permission. It admits the transient tmpfs/proc/devpts, bind/remount, propagation setup and root-switch operations needed by the existing launcher, with explicit denials for sensitive proc/sys paths. New cgroup, block and security filesystem mount types have no allow rule. Its `file`, `network`, mount and same-profile ptrace permissions are intentionally broader than a default container policy: worker confinement therefore depends on the existing private namespaces, capability clearing, non-nestable user namespace, inner seccomp and cgroup bounds. This review does not treat the AppArmor label by itself as the complete sandbox. The outer capability list remains separately constrained by deployment and bootstrap; AppArmor capability allowances for namespace-local setup do not grant those capabilities to the daemon. No changes to `crates/substrate-host/src` were present.

The seccomp file retains default EPERM and an explicit amd64 architecture list. Independent comparison against the pinned Moby source matched every baseline rule after the documented architecture/kernel/capability-condition resolution. Socket and personality argument conditions remain; clone3 still returns ENOSYS. The final explicit rule supplies the documented namespace, mount/unmount, root-switch, hostname and quota calls, including the intentionally unrestricted clone setup path. There is no allow rule for the forbidden module, BPF, performance, file-handle-open, kernel-log or explicit clock-setting operations checked by the new test. Baseline clock-query/adjustment syscalls remain subject to kernel capability checks; SYS_TIME is not granted. The profile is an outer setup policy, with the existing inner filter still required for each worker. [Pinned Moby profile](https://github.com/moby/profiles/blob/61eaf32614c7c71b60bd8927d3e6a4ffc8ff1f31/seccomp/default.json)

`xtask/src/container_profiles.rs` validates the local schema against Draft 2020-12, validates the profile against that schema, and checks selected privileged syscalls remain absent from allow rules. `xtask/src/main.rs` registers this module, so its test participates in the workspace gate. The schema fixes the default action, errno, architecture and supported rule structure. This is useful structural and negative coverage; it is not runtime evidence that a node loaded the selected profile or that the complete confinement journey passed.

The node installer uses compiled-in policy bytes and fixed filenames under a protected root-owned directory. Existing files must be regular, protected, root-owned and byte-identical. New files are written through exclusive temporary files, synced and published with a same-directory hard link that cannot replace an existing name; a race winner must still pass exact verification. A previously loaded AppArmor name requires its matching retained source before replacement. The parser path is fixed and its environment cleared. Readiness checks compare both retained files and the enforced profile name; the hold loop repeats those checks and handles normal termination. Those checks rely on a trusted node administration boundary and parser/include files from the immutable image; they do not cryptographically attest the kernel's compiled policy from the profile name alone. The dedicated host mount, MAC_ADMIN authority, scheduling and application/installer separation require review of the deployment inputs and the pending real-node test.

Packaging keeps the ordinary non-root daemon entrypoint, durable state ownership, separate quota executable and separate MCP image. The runtime now includes the shell/tool libraries and profile installer. Bubblewrap is built without setuid support from a checksum-pinned archive; its license is copied, alongside distribution package notices and existing project notices. The source build remains locked and the existing release workflow is amd64-only, consistent with the explicit seccomp scope. A full image startup, quota file-capability preservation and release smoke pass cannot be inferred from Dockerfile inspection alone.

The retained `container-proc-machine-response.txt` is an HTTP 200 machine response advertising actual exec namespaces, no-egress, cgroup limits/kill and `sessions.pty=true`. It supports successful capability probing in that run; it does not establish an interactive PTY input/resize/output/cancel journey, whole-tree cleanup, node installer correctness, exact policy negative checks or a published image binding. `container-tools-clippy.log` records successful targeted Clippy completion. The earlier three bootstrap tests remain the inspected targeted test evidence. This reviewer ran no builds or privileged tests. Full repository gates, installer validation, ordinary/quota final-image verification, policy negative cases, wire PTY acceptance and deployment review remain separate pending evidence.

Reviewed file SHA-256 identities:

```text
f5b33660450aa7ea6c70a439aaa377aef0b4ccfdb5c1aa9d14b73ea0ad4f834d  Dockerfile
1aef1b30d504be1a376acb57a635d205bd26655aa5a7985ad8bd6a2688928113  docs/design/25-container-private-host-execution.md
7c41e6b61c851bc6f0bca2e451778763059f7a44dfdfe66ecc888b859ea5e711  crates/substrate-daemon/src/container_bootstrap.rs
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
[
  {
    "file": "crates/substrate-daemon/src/container_bootstrap.rs",
    "line": 242,
    "category": "security",
    "severity": "blocker",
    "message": "The fresh proc mount is performed after validating propagation only for the cgroup mount. An otherwise accepted shared /proc mount can propagate this mutation outside the container's mount namespace. Require an unambiguous private proc mount before mutation and add shared/propagated/missing/ambiguous proc refusal fixtures."
  }
]
```
