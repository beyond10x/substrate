---
format: aep.planning-md/1
id: review-result:container-init-security-review
kind: review-result
status: active
title: Fixed non-root init handoff review
relations:
- reviews: story:container-private-exec-bootstrap
revision: 1
---
approve

Independent read-only review of the Tini/PID-1 change only. No actionable findings in the inspected change. Reversing only the new init constant, executable validation loop, fixed Tini invocation/environment removals, Docker package addition and design paragraph reproduces the three previously reviewed file hashes exactly. Earlier bootstrap, profile and checker reports remain unchanged.

`crates/substrate-daemon/src/container_bootstrap.rs:27` applies the existing protected root-owned regular-file predicate to both `/usr/bin/tini` and the selected fixed daemon, rejecting symlinks, setuid/setgid and group/other write. All mount, ceiling, ownership, UID/GID and capability checks still complete before exec. Tini therefore replaces the already-unprivileged bootstrap as PID 1; the extra child daemon remains in the same delegated `daemon` cgroup, leaving the delegation root process-free and retaining enclosing limits. No capability, mount, namespace or path authority was added.

The invocation at `crates/substrate-daemon/src/container_bootstrap.rs:52` is fixed to Tini followed by `--`, the selected absolute daemon path, normal daemon arguments and the fixed cgroup root. The delimiter prevents daemon arguments from becoming Tini options. Removal of `TINI_SUBREAPER`, `TINI_KILL_PROCESS_GROUP` and `TINI_VERBOSITY` covers the runtime environment controls in the inspected upstream Tini version; normal daemon configuration remains inherited. Tini's fork/exec path preserves the non-root identities and ordinary no_new_privs state. In quota mode the retained SYS_ADMIN bounding allowance can still be acquired by the existing quota daemon file capability after the intermediate ordinary Tini executable. No UID restoration or capability acquisition is requested for Tini itself. [Tini v0.19.0 source](https://github.com/krallin/tini/blob/v0.19.0/src/tini.c)

Tini's PID-1 role supplies orphan adoption and reaping while forwarding termination to its immediate daemon child. It does not replace Substrate's per-execution cgroup kill or signal handling, and the disabled group-forwarding override prevents a deployment environment variable from changing that scope. The daemon retains responsibility for its own direct children; Tini reaps adopted orphans. This addresses the reported zombie process records that remained after cgroup removal. Tini exits with its child result rather than pretending a failed daemon started successfully. [Tini behavior and PID-1 requirements](https://github.com/krallin/tini/blob/v0.19.0/README.md)

Docker adds `tini` only to the existing execution-prerequisites package installation. The ordinary default image entrypoint and the separate MCP target are unchanged; the hosted bootstrap opt-in selects the new init handoff. The retained build log identifies Ubuntu's amd64 `tini` package as `0.19.0-1`. The package is installed at image build time, with distribution notices retained; no startup download was added. The immutable published image must bind the actual package bytes and runtime behavior.

`bootstrap-tests-tini.log` records four passing bootstrap validation tests, and `container-tools-clippy-tini.log` records successful targeted Clippy. These checks do not exercise the final PID-1 process tree. Fresh-image proof must still establish Tini's non-root identity and absent effective/permitted capabilities, ordinary/quota daemon behavior, container termination forwarding, acknowledged descendant removal and absence of zombie records. Tini makes no new graceful application-shutdown guarantee; that remains the daemon's behavior. The coordinator is running the fresh-image PTY proof, and no result from that run is assumed here. No build or live-service mutation was performed by this reviewer.

Reviewed file SHA-256 identities:

```text
13ab2abe2714b32752d1e0073d1c90f1153ad8851e077f0de12db491452f0dc2  crates/substrate-daemon/src/container_bootstrap.rs
5fed0081e8457a226cdad4378e4cee78befa5748ddb43e2351381b8643e6cc4e  Dockerfile
81dc058887fc7615a8fe52df351f7645f96aaad6b2a6ee3e8199a9798b547c6e  docs/design/25-container-private-host-execution.md
```

```findings
[]
```
