# Design 04: security and isolation

**Status:** draft for review · **Date:** 2026-08-13

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

## Security gates before implementation

1. Write host-driver attack cases and expected refusal classes.
2. Fix the minimum Linux isolation guarantee and explicitly state other operating-system support.
3. Decide process-tree containment and cleanup primitives.
4. Define network capability granularity and default egress.
5. Close the secret materialization design in Design 06.
6. Fix subject/resource/operation namespace isolation and not-found behavior.
7. Define Git redirect, proxy, submodule, LFS, helper, hook, and DNS-rebinding refusal cases.
