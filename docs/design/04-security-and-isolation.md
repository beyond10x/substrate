# Design 04: security and isolation

**Status:** accepted v1 design · **Date:** 2026-08-13

Substrate deliberately exposes high-impact machine authority. Its safety contract is therefore about
precise guarantees, observable enforcement, and named refusal—not about claiming all drivers are
equally isolated.

## 1. Threat model

Assume callers can submit hostile paths, repositories, arguments, environment requests, process
trees, images, output volume, network targets, and races. Assume executed code attempts filesystem
escape, secret discovery, child-process survival, resource exhaustion, and daemon credential theft.

Assume callers also attempt SSRF, DNS rebinding, redirect/proxy escape, cross-subject resource-id
enumeration, operation-id collision, and pairing a stored credential with an attacker destination.

Do not assume Docker socket access is a security boundary: a Docker-backed deployment is
root-equivalent to its host unless separately isolated by its environment. Kubernetes authority is
bounded only by the handed-over credentials and namespace enforcement.

## 2. Host filesystem

- Workspace roots are created and owned by substrate under an explicit configured root.
- Every path is relative, normalized, and walked component-by-component.
- Lexical traversal, absolute paths, symlink escape, dangling-link ambiguity, device files, and mount
  escape are refused according to the operation's policy.
- File replacement is atomic within the workspace and bounded by declared limits.
- Snapshot input and output cannot silently include paths outside the workspace.

The implementation belongs behind substrate-owned ports. Flux behavior may supply adversarial test
cases but no code or type dependency.

## 3. Process execution

- Commands are argv arrays; no contract field accepts a shell string.
- Working directory is a substrate-owned workspace address.
- Child environments start empty apart from a closed non-secret baseline, then apply explicitly
  admitted values.
- Daemon credentials, inherited file descriptors, control sockets, and host environment values are
  unavailable to children by default.
- Process groups, descendants, timeout, cancellation, output caps, and cleanup are part of the
  applied observation.
- A requested execution capsule is independently digest-verified, materialized as regular files in
  a private per-execution directory, and mounted read-only at `/runtime`; the mutable workspace
  remains a separate `/workspace` mount.
- Capsule directories remain owned through whole-tree terminal observation. After a daemon crash,
  startup reconciles orphan cgroups first and then removes only bounded, well-formed private capsule
  directories; an unexpected entry or symlink fails closed.
- The inline development capsule attests only its application/configuration/hook bytes. It does not
  attest the host kernel, interpreter, libraries, or read-only system tree and carries neither
  secrets nor network authority.

## 4. Sandbox and network

Sandbox requests name guarantees such as workspace-only write access, system read access, network
mode, process limits, and required enforcement. The machine capability document identifies which
backend was actually probed. If `require` cannot be honored, execution is refused.

Network defaults are explicit per posture and request. DNS, loopback, private destinations, public
egress, listening sockets, and exposed endpoints are separate capabilities rather than one boolean.
Ordinary execution defaults to no egress. An aperture is deployment/operator authority, is matched
after resolution and on connect, and cannot be widened by request fields. Loopback, link-local,
metadata, private, and public ranges are separate policy classes. Redirects and proxy behavior are
subject to the same final-destination check.

Named source, registry, and push credentials are inseparable from their configured scheme,
authority, port, path, and destination aperture. A request can select a configured binding but
cannot supply a new destination for its credential.

## 5. Resource bounds

Input bodies, path depth, file ranges, captured streams, event retention, process count, memory,
CPU, duration, artifact size, concurrent sessions, and queued mutations require declared bounds.
Truncation is an observation and must not stop draining a child pipe.

## 6. Audit-safe diagnostics

Errors may name a path, capability, limit, or configured secret slot, but never secret material.
Logs and events exclude bearer tokens, request authorization, injected secret values, raw child
environment, and session authorities by type.

## 7. Minimum host guarantee

The first host driver is Linux-only and requires all of the following before it advertises `exec`:

- a dedicated unprivileged daemon/worker identity;
- `openat2` path resolution rooted at a pre-opened workspace directory with `RESOLVE_BENEATH`,
  `RESOLVE_NO_MAGICLINKS`, and no-follow behavior for guarded file operations;
- unprivileged user, mount, PID, IPC, UTS, and network namespaces through a probed bubblewrap
  backend; the workspace is the only writable bind and system inputs are explicit read-only binds;
- a **non-nestable** user namespace: the backend accepts `--disable-userns`, and the probe proves it
  with `--assert-userns-disabled` rather than trusting that the option took effect. A confined
  process holds a full capability set inside its own user namespace, so without this it can create
  a second one and hold `CAP_SYS_ADMIN` there — the entry point of most unprivileged kernel
  privilege escalations. A backend that cannot honour either option leaves every fact gated on
  `exec` absent and every exec refused `exec.sandbox-unavailable`, never a sandbox without it;
- a delegated cgroup v2 subtree with process, memory, and CPU bounds plus whole-cgroup termination;
- cleared environment, closed inherited descriptors, `no_new_privs`, a private temporary directory,
  and no ambient daemon/control credential;
- a new network namespace with no usable interface for the minimum slice.

The minimum slice does not claim protection from kernel compromise or syscall-level seccomp
containment. If `openat2`, namespace isolation, bubblewrap, a non-nestable user namespace, cgroup
delegation/kill, or no-egress probing is unavailable, workspace file operations or exec are absent
from capabilities as appropriate; execution never falls back to an unconfined process. Non-Linux
hosts are unserved.

Cancellation signals the cgroup, waits the configured grace interval, kills the entire cgroup, and
observes emptiness. A process group alone is not accepted proof that descendants are gone.

## 8. Git destination behavior

Git is deferred to stack-adoption phase 6, but its security shape is fixed:

- anonymous sources permit HTTPS only and still pass the deployment aperture; authenticated HTTPS
  or SSH uses a named source/remote whose scheme, host, port, path prefix, credential, and host-key
  policy are inseparable;
- `file`, `git`, `ext`, scp-like unparsed addresses, URL userinfo, caller proxy settings, credential
  helpers, hooks, and protocol-command overrides are refused;
- every DNS answer must be inside the configured class, the connection is pinned to a validated
  address while TLS/SSH verifies the configured name, and each reconnect/redirect resolves and
  checks again; mixed allowed/forbidden answers fail closed;
- redirects are disabled by default; a named source may allow at most three and every hop must stay
  inside the same configured aperture. Proxies are disabled unless the named binding fixes the
  proxy and separately admits its destination;
- submodules and Git LFS are disabled by default. Enabling either requires named destination and
  credential bindings for every secondary fetch. No inherited parent credential crosses authority;
- checkout resolves to and records an immutable commit. Mutable refs are input convenience, never
  the observed source identity.

## 9. Required threat vectors

| Attack | Required outcome |
|---|---|
| lexical, symlink, magic-link, mount, or absolute path escape | `refused` with no outside access |
| sandbox/backend disappears after admission | `refused` before dispatch against stale snapshot |
| child forks, daemonizes, ignores signal, or fills output pipe | bounded observation, cgroup kill, no surviving process |
| request asks for egress in minimum slice | `unserved`; no weaker execution |
| cross-subject resource or operation id | indistinguishable not-found refusal |
| daemon environment/fd/credential discovery | value absent; violation fails the conformance test |
| Git rebinding, redirect, proxy, helper, hook, LFS, or submodule escape | `refused` before credential release/connect |
| input/output/resource limit exceeded | `exhausted` or typed truncation as specified, never unbounded use |

These vectors are the conformance inventory; implementation supplies fixtures before the minimum
slice exits.
