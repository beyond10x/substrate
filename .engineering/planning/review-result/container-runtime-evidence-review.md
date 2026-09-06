---
format: aep.planning-md/1
id: review-result:container-runtime-evidence-review
kind: review-result
status: active
title: Container PTY and confinement evidence review
relations:
- reviews: story:container-private-exec-bootstrap
revision: 1
---
approve

Independent read-only review of the retained local runtime evidence following the approved Tini change. No actionable findings in the inspected evidence or the bounded source changes. This report supplements the immutable source reviews; it does not certify a published image or a deployed terminal service.

Both `container-pty-journey-tini.log` and `container-pty-journey-tini-quota.log` contain the checker's final PASS result. In the reviewed `crates/b10x-substrate-sdk/examples/container-pty-check.rs`, that result follows successful public-SDK workspace byte round-trip, PTY input and resize, zero effective/permitted/inheritable/ambient/bounding worker capabilities with no_new_privs, and resource usage for the exact execution with positive memory and at least two processes. Before Kill, the checker matches the acknowledged inner descendant PID to an actual sleep member of the execution cgroup. PASS requires that cgroup and every captured process record to disappear, followed by successful destruction of the owned workspace. This specifically checks the orphan zombie symptom that prompted Tini, beyond merely observing cgroup removal.

The current checker differs from its previously approved bytes only by three explanatory timeout error mappings. Removing those mappings in memory reproduces the prior SHA-256 `d98eeed85115966d6bc720d897cbf22b442adcc8153ed465644b3a81bd0c110b` exactly. The checks, five-second deadlines, separate failure-path cleanup attempts and final PASS placement remain unchanged.

`container-final-processes-quota.txt` shows Tini and the daemon with all four UID and GID fields equal to 65532. Tini has zero effective, permitted, inheritable and ambient capabilities, with only SYS_ADMIN in its bounding set. The daemon's effective, permitted and bounding sets contain only SYS_ADMIN; its inheritable and ambient sets are empty. Their no_new_privs value of zero is consistent with the intentional quota file-capability path. The daemon's parent PID is 1, consistent with the fixed Tini handoff. The separate observer shell has zero active capability sets and the broader bootstrap bounding allowance; the coordinator identifies it as the transient docker-exec observer, not a daemon child or execution worker. It is not evidence that sandbox workers retain that allowance. This snapshot contains no zombie state; the two checker results provide the stronger captured-descendant absence check. No separate ordinary-mode Tini/daemon process snapshot was supplied, so that live observation is not inferred from the quota snapshot.

`apparmor-undeclared-mount-audit.txt` contains kernel audit denials attributed explicitly to the enforced `substrate-host-exec-v1` profile, operation mount, filesystem cgroup2, target `/tmp/undeclared/`, error -13, including the read-only retry. This independently identifies AppArmor as the cause for the failed operation whose stderr appears in `apparmor-undeclared-mount.log`. It is evidence for that negative case, not an exhaustive confinement or syscall test.

`container-init-termination.txt` and `profile-installer-termination.txt` each record running=false, exit=0 and oom=false. The coordinator reports these were captured after normal stop with a five-second timeout. The retained snapshots support successful termination without OOM or forced-kill exit status; they do not themselves retain command timing or independently prove every signal-forwarding step. The earlier source review covers the fixed Tini forwarding behavior.

The retained build logs finish exporting local daemon image ID `sha256:8536e5855cc5a52fd562c528d8fa156e9ba8026cc4984ce079e08b43afe8b545` and checker image ID `sha256:01b3b35f381d120f213ed0ba4eb496c08f2a169de6a6ca9c4e644dcd19bc4c0b`. These are local image IDs, not published OCI manifest digests. The runtime logs contain the final assertion result but not invocation/image metadata; association of those runs with the built images is coordinator-provided provenance. Release publication must supply the immutable source/image binding separately.

The bootstrap, Dockerfile, design and AppArmor bytes still match the reviewed identities below. The subsequent Cargo changes are exclusively 0.7.5 to 0.7.6 substitutions: reversing that version string in each changed manifest and Cargo.lock reproduces HEAD exactly. The lock advances eight local packages without external dependency changes. README documents the opt-in prerequisites, separate profile installation and the limits of portable CI; the changelog describes the added runtime packaging without claiming publication. No existing host driver, process implementation or wire contract change was found in this follow-up.

Recommendation: accept this retained local ordinary/quota runtime evidence and the bounded version/documentation follow-up. The full repository gate was still running at review time. Protected source CI, immutable publication, concrete downstream deployment inputs and real deployed browser terminal acceptance remain separate evidence requirements. This review performed no builds or live mutations and makes no claim about model credentials or the preserved user workspace.

Reviewed source SHA-256 identities:

```text
13ab2abe2714b32752d1e0073d1c90f1153ad8851e077f0de12db491452f0dc2  crates/substrate-daemon/src/container_bootstrap.rs
5fed0081e8457a226cdad4378e4cee78befa5748ddb43e2351381b8643e6cc4e  Dockerfile
81dc058887fc7615a8fe52df351f7645f96aaad6b2a6ee3e8199a9798b547c6e  docs/design/25-container-private-host-execution.md
e3ff9b10eb633585354cedbcbe1dafb4ec01c4296ea1085677221adca3b6843a  deploy/apparmor/substrate-host-execution
3014f6eec60d90e7ade0f581aeefb7a2451dc46b02fb4dff4d0e3d7625a05f6e  crates/b10x-substrate-sdk/examples/container-pty-check.rs
5a2a939efc81c5290af535b06bd5bcf00baa36119115769aea62a386df9b7676  README.md
d32ab6253b90b41d4b78960e01e20402d1e722fd684322a4c2ce452cabab9c1a  CHANGELOG.md
```

Inspected evidence SHA-256 identities (private retained basenames):

```text
5186eb141bbf7c93ddd01121fba577f0d6fe58880367b7492da381e854e1f4b4  container-pty-journey-tini.log
5186eb141bbf7c93ddd01121fba577f0d6fe58880367b7492da381e854e1f4b4  container-pty-journey-tini-quota.log
2939047dc482cf4c033d336e511eb9924192047e9df522a0e5ca89c5ab73f396  container-final-processes-quota.txt
e1ad101e75d10c798e1ebdcd1a337d861bb0c05d2fc33de776324f459c00881a  apparmor-undeclared-mount.log
04907b4a3101dde5bb6aeb8c04a7a3f25d7a578a6e241c2408729050ac2f5ec2  apparmor-undeclared-mount-audit.txt
1e42ecb7fdb411cde96f6b359f81668f25cc3b0c23f3fcb8b2a06cdf63779b72  container-init-termination.txt
1e42ecb7fdb411cde96f6b359f81668f25cc3b0c23f3fcb8b2a06cdf63779b72  profile-installer-termination.txt
604877065b10e8428c5c34876fe1b0dd1e2951657530e77a3477ccf0b05df5b9  daemon-image-build-tini.log
a7e1b50ed5cb780c28bcfae205010634a1fa48c357f93af5080b2cdc312212ba  container-checker-build-tini.log
```

```findings
[]
```
