---
format: aep.planning-md/3
id: story:substrate-host-features-let-confinement-consumers-skip-git-and-kubernetes
kind: story
status: active
title: substrate-host features let confinement-only consumers skip Git and Kubernetes
tags:
- host
- packaging
revision: 3
transitions:
- {from: "draft", to: "proposed", at: "2026-11-05T06:18:55Z", actor: "human:timo", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-05T06:18:55Z", actor: "human:timo", revision: 3}
---
# Story: substrate-host features let confinement-only consumers skip Git and Kubernetes

## Outcome

A consumer that needs only `substrate_host::confinement` can depend on `b10x-substrate-host` with
`default-features = false`. Its lockfile then holds none of gix, git2, curl, openssl or the
Kubernetes client, and its confinement adapters behave as they do today. Every existing consumer,
the daemon included, keeps today's dependency set and behaviour, because the default features are
today's set.

## Observation

A downstream consumer pinned `b10x-substrate-host` 0.19.2 for confinement only. Its lockfile gained
175 packages, among them gix, git2, curl, openssl, aws-lc and the Kubernetes client, because the
crate declares no features. This was reported by the orchestrator and has not been re-measured here.

Read at `aeab475f`:

- gix, git2 and curl are used only in `crates/substrate-host/src/git.rs` and
  `src/git/network.rs`. Their callers are the workspace Git roots in `lib.rs` and the
  `workspace.git` fact in `probe.rs:100`.
- `b10x-substrate-kubernetes-client`, which brings hyper, rustls with aws-lc and tungstenite, is
  used only by `src/confinement/pod.rs`.

## Design

- **Features:** `git` makes gix, git2 and curl optional dependencies; `kubernetes` makes
  `b10x-substrate-kubernetes-client` optional. `default = ["git", "kubernetes"]`.
- **Without `git`:**
  - `git::mechanism_is_provable` is false, so the probe publishes no `workspace.git` fact.
  - A Git source is refused with the existing unserved refusal.
  - Every other `git::` entry point the driver calls returns that same refusal, or does nothing
    where nothing can exist, such as reconciling baselines that no Git root ever wrote.
- **Without `kubernetes`:** `pod::open` and `pod::open_pod` refuse with a named confinement error.
- **Public API:** every public type, function and enum variant stays under every feature set, so
  the features are additive and no exhaustive match breaks.
- **Gate:** it gains Clippy for `-p b10x-substrate-host --no-default-features --all-targets`, and a
  no-default-features run of the confinement tests that need neither Git nor Kubernetes.
- **`cargo xtask check-packages`:** admits these two features and refuses any other host feature.

## Acceptance

- `cargo tree -p b10x-substrate-host --no-default-features -e normal` names none of gix, git2,
  curl, openssl-sys or b10x-substrate-kubernetes-client. The default tree is unchanged.
- With no default features, the probe reports no `workspace.git`, a Git source is refused unserved,
  and opening the pod adapter is refused by name.
- The default-feature workspace tests and the `confinement_public_items` freeze pass unchanged.
- CHANGELOG, the host README and AGENTS.md name the two features, and say which one a
  confinement-only consumer takes.
