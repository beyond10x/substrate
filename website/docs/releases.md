---
title: Release notes
description: What changed in the newest Substrate release, generated from CHANGELOG.md.
---

# What changed in the newest release

This page ends with the newest released section of `CHANGELOG.md`, and `cargo xtask check-docs`
fails when the two differ. Earlier releases are in the
[full changelog](https://github.com/beyond10x/substrate/blob/main/CHANGELOG.md). The
[confinement adapters](./concepts/adapters.md) page explains each adapter the notes name.

What 0.16.1 first publishes is listed under 0.16.0 in the full changelog and summarized on the
[status page](./status.md#new-in-0161), including the namespace driver's limits.

<!-- Generated from CHANGELOG.md by `cargo xtask docs-releases`. Edit CHANGELOG.md, not the text below. -->

## 0.18.1 (2026-11-01)

Fixes only; the wire, the contract bundle (`substrate-wire/0.26.0`) and every published route are
unchanged. The `pod` adapter now sweeps a `NetworkPolicy` whose Pod was never created, and the
Kubernetes namespace driver's probe says why each fact it withholds was not proved.

### Fixed in 0.18.1

- Every fact the Kubernetes namespace driver's probe withholds is logged at `warn` as
  "`<fact>` was not proved: `<reason>`", and a failed call keeps its own reason code: a store
  write that fails while the probe creates its PVC or keeper refuses the probe with
  `state.store-failed`, not `kubernetes.api-failed`, a refused `pods get` while the probe waits
  ends its run at once with `kubernetes.api-failed` and the API's answer, and the probe's
  removal warnings carry the API's answer (`cannot <verb> resource "<resource>"`).
- An exec end the Kubernetes namespace driver decides after an API call no longer panics, or
  overwrites the end, when a concurrent observation already ended the exec; the end recorded
  first stands.
- The `pod` adapter's open and deferred sweeps remove an old owned `NetworkPolicy` whose run's
  Pod was never created, once a watch replay from the policy's `resourceVersion` ends in a
  `BOOKMARK` before any event of the run's Pod; a replay that does not end in one (410, an
  `ERROR` event, a stream that ends or a silent one) still keeps the policy.
- A Role-verb case of `cargo xtask namespace-lane observe` passes on the daemon's log only once
  the first probe has finished and a `WARN` line carries the API's answer refusing that verb
  (`cannot <verb> resource "<resource>"`); a reason code or a "was not proved" line alone,
  or another resource's refusal, no longer passes it.
- A `pods/exec` upgrade the Kubernetes API refuses keeps the API's `Status.message` when the
  refusal's body arrives after its head (by `Content-Length` or chunked), and the namespace
  driver's executor handshake ends at once on a 401 or 403 with
  `kubernetes.executor-handshake-failed` and the API's answer (`cannot create resource
  "pods/exec"`) instead of retrying until its 60 s bound; every other answer is still retried.
  `cargo xtask namespace-lane observe` keeps the daemon logs of a failed case under
  `target/namespace-lane/daemon-logs/` and names them in the case's failure.
- The `cargo xtask namespace-lane observe` case "Role without pods/exec create" also passes on
  the API's bare refusal of the exec: a `kubernetes.executor-handshake-failed` line whose
  `exec answered 403` names the probe's keeper Pod and gives no reason, which is how the API
  refuses that verb; every other Role-verb case still needs the API's `cannot <verb> resource`
  answer.
- `cargo test -p xtask` no longer fails a chart-lane test with `Text file busy (os error 26)`: a
  child process writes each stand-in script, so no child another test forks meanwhile inherits it
  open for writing.
