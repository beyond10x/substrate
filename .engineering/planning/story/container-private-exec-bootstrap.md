---
format: aep.planning-md/1
id: story:container-private-exec-bootstrap
kind: story
status: implemented
title: Serve confined execution beneath a private container delegation root
relations:
- decomposes: epic:kubernetes-deployment-and-driver
- informed_by: story:node-bound-kubernetes-serving-profile
scope:
- confidence: cited
  path: .github/workflows/release.yml
- confidence: cited
  path: CHANGELOG.md
- confidence: cited
  path: Cargo.lock
- confidence: cited
  path: Cargo.toml
- confidence: cited
  path: Dockerfile
- confidence: cited
  path: README.md
- confidence: cited
  path: THIRD_PARTY_LICENSES.html
- confidence: cited
  path: crates/b10x-substrate-sdk/Cargo.toml
- confidence: cited
  path: crates/b10x-substrate-sdk/examples/container-pty-check.rs
- confidence: cited
  path: crates/substrate-daemon/Cargo.toml
- confidence: inferred
  path: crates/substrate-daemon/src/bin/substrate-container-exec.rs
- confidence: cited
  path: crates/substrate-daemon/src/bin/substrate-container-profiles.rs
- confidence: inferred
  path: crates/substrate-daemon/src/container_bootstrap.rs
- confidence: cited
  path: crates/substrate-daemon/src/container_profiles.rs
- confidence: cited
  path: crates/substrate-host/Cargo.toml
- confidence: cited
  path: crates/substrate-mcp/Cargo.toml
- confidence: cited
  path: crates/substrate-store/Cargo.toml
- confidence: inferred
  path: deploy/apparmor/substrate-host-execution
- confidence: cited
  path: deploy/seccomp
- confidence: inferred
  path: docs/design/25-container-private-host-execution.md
- confidence: cited
  path: xtask/Cargo.toml
- confidence: cited
  path: xtask/src/container_profiles.rs
- confidence: inferred
  path: xtask/src/image_startup.rs
- confidence: cited
  path: xtask/tests/release_workflow.rs
revision: 15
---
## Outcome

A deployment can opt into the existing host execution contract inside a container without granting the daemon a host cgroup mount or changing wire semantics. The released runtime includes the required shell, socat and compatible non-setuid bubblewrap. A bounded Rust bootstrap verifies the container-private namespace and resource ceiling, prepares only its own delegation root, drops bootstrap authority and starts the non-root daemon. Existing host probes decide whether execution and PTY facts are served.

## Acceptance

A real container retains its enclosing CPU and memory ceilings, hides ancestor/sibling cgroups, places the daemon below a process-free delegated root and enforces per-exec CPU, memory, swap, process, timeout and whole-tree cleanup. Startup refuses unsupported namespace, mount, ownership, kernel or security-profile postures before launching the daemon. The quota variant retains only the existing SYS_ADMIN authority after bootstrap; launched sandboxes inherit no permitted/effective/inheritable/ambient capabilities. A real PTY echoes generated input, supports resize and leaves no process/cgroup after closure. Negative probes prove absent setup remains refused. Default startup continues to work with its existing non-root posture.

## Design and scope

The design is recorded at docs/design/25-container-private-host-execution.md. Each setup change was documented before its implementation and checked in disposable containers. The initial default AppArmor probe refused the cgroup mount; a separate disposable diagnostic distinguishes LSM policy from kernel delegation. Deployment must carry an explicit reviewed security profile, with no global policy disablement. No host paths or host namespaces belong to the application pod.

Cited surfaces: Dockerfile, crates/substrate-host/src/probe.rs, crates/substrate-host/src/process.rs, crates/substrate-daemon/src/main.rs, README.md and the existing image-startup/release gate. Inferred surfaces: a new daemon bootstrap binary, its Linux/container conformance and the explicit security profile. The existing host probes and frozen contracts are not relaxed. Downstream generic chart and private deployment composition are owned by the active Devcenter Projects recovery story.

This is one bounded runtime story, not completion of the broader standalone Kubernetes chart or namespace driver. No multi-story decomposition is created in this slice, so a comparison panel is not applicable. Independent security review and the complete repository plus real delegated/image gates remain required before publication.

## Runtime observations and remaining proof

The initial independent review identified unsafe enclosing-control metadata and setuid/setgid executable admission; both were fixed and the original review retained. A bounded re-review approved those corrections. Subsequent real sandbox tests exposed three further deployment prerequisites: explicit AppArmor namespace/mount permissions, an outer seccomp allowlist admitting pivot_root, and a private proc mount which permits nested procfs. The enforced AppArmor policy protects the sensitive proc paths removed from runtime mount masking. Kernel user-namespace ceiling writes additionally require namespace-local SYS_RESOURCE; this capability is never added to the bootstrap or daemon bounding set.

The final-image daemon has now observed and advertised actual namespace/no-egress/cgroup-limit/whole-tree-kill and PTY capability facts. The exact policy bytes, separate Rust node installer, public-SDK PTY lifecycle, quota variant and real-node rollout still require independent review and retained conformance before publication. The installer carries only MAC_ADMIN with the exact securityfs and dedicated kubelet profile directory, outside the application pod. These are runtime packaging/deployment configuration changes; no host probe or frozen wire contract has been relaxed.

## Release candidate verification

Both ordinary and quota final-image SDK journeys pass file write/read, generated PTY input and resize, empty worker capabilities and no_new_privs, exact live measurements, an acknowledged background descendant and complete process/cgroup removal. A PID-1 reaper was added after the first real cleanup run found orphaned zombie records; the corrected runtime leaves none. Quota-mode observations show a non-root init with empty permitted/effective capabilities and a non-root daemon with only the existing SYS_ADMIN quota authority. Normal container termination and installer hold termination both return exit zero.

The final-image startup checker passes all four cases: executable metadata/xattrs, ordinary startup with empty bounding set, ordinary startup with SYS_ADMIN bounding only, and explicit quota startup, including Tokio worker masks. Real negative containers refuse absent AppArmor enforcement and unlimited enclosing memory. Kernel audit proves the explicit AppArmor profile denies an undeclared cgroup2 mount. First installation, repeated installation, read-only verification and refusal of an unrecorded already-loaded profile were exercised. Earlier independent findings and their source/evidence approvals remain recorded unchanged.

The complete scripts/gate.sh passes for version 0.7.6, including all sixteen immutable bundle fixed points, 3633 classified JSON documents, formatting, Clippy with warnings denied, license fixed point, package boundaries, secret history and advisory checks. The complete scripts/delegated-lane.sh also passes host confinement, PTY, public SDK, remote WSS, disposable MCP, hosted attachment and the delegated wire inventory. The version bump changes only the eight workspace versions and exact internal edges; regenerated third-party notices change those eight version labels only.

The pinned Docs System collector accepts this repository. The full organization documentation audit separately refuses existing aep/docs manifest drift; no unrelated manifest or catalog state was modified. Protected-main CI, immutable publication and exact published-image readback remain outstanding. The downstream deployment separately owes real-node profile verification, preserved-workspace checks and actual browser terminal acceptance before enabling the product profile.

## Immutable release and published-image acceptance

PR95 merged after exact-source Full gate34051053302 succeeded. Its reviewed source68b4e21115b4a57cd8c273287225e00a7f8efe97 is an ancestor of main and has the same tree as merge96e4117bfbc1da59332b72dc9bca827f7747317c; the main Full gate34051612003 also succeeded. The annotated0.7.6 tag names that exact reviewed source. Release34051681656 completed successfully, including all three keyless signatures, exact workflow-identity verification, final-image smoke tests and anonymous artifact retrieval.

The published daemon digest is sha256:2ffe9021c9f498cda8d08e5b7438f0e3ca2bc371bdb1bf467e18ab7403073170, the disposable MCP digest is sha256:61b08d32b1c2e365c7466113c87334ae99b269716f3a469626134abf1ea580a2, and the unchanged development wire0.16.0 bundle is sha256:4c4e57a1b2427cb004a05cb475c1193e979777c5c79d9a9505ba5facbe10daf7. Release metadata and CHANGELOG.md retain all three.

An independent anonymous pull confirms the daemon digest and exact source-revision label. Local verification with the same cosign3.0.6 used by the release verifies the claims, transparency-log inclusion and signing certificate under the exact release workflow identity. A previously installed cosign2.4.3 did not find the new signature format; its unsuccessful result is not counted as verification.

Both ordinary and quota journeys pass against the published immutable image, using the public-SDK checker built from the exact released source: workspace bytes, generated PTY input, resize, all empty worker capability sets, no_new_privs, observed resource usage, acknowledged descendant identity, and complete process/cgroup cleanup. Both containers terminate normally with exit zero and are removed. This establishes the bounded runtime story's released behavior. Downstream node-profile installation, actual hosted browser terminal admission, preserved-user-workspace checks and Agent credential acceptance remain the downstream product's separate delivery obligations; this result does not claim them complete.
