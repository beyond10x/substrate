# b10x-substrate-kubernetes-client

The bounded Kubernetes API client the Substrate `pod` confinement adapter and the Kubernetes
namespace driver share. It loads one operator-written JSON kubeconfig and refuses every member that
changes who the client is or whom it trusts, then speaks to the API server itself over HTTP/1.1 and
TLS: bounded bodies, paged metadata lists, watch streams, deletes with `preconditions.uid`,
`dryRun=All`, `pods/log`, `pods/exec` over WebSocket `v5.channel.k8s.io`, `Lease` reads and
updates and `SelfSubjectRulesReview`. It also holds the rule that decides whether a gone Pod's
container is known to have stopped. It admits no Kubernetes client crate and runs no `kubectl`.

The `fake` feature compiles an in-process API server for tests. It is legal only on a
`[dev-dependencies]` edge; `cargo xtask check-packages` refuses it anywhere else.

This package is under development. See the
[public Substrate documentation](https://beyond10x.github.io/substrate/) for its current status.
