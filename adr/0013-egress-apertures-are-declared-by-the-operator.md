---
status: accepted
date: 2026-08-29
---

# ADR 0013: egress apertures are declared by the operator and referenced by name

## Context

Ordinary execution has no egress: every exec and pipe session runs under bubblewrap's
`--unshare-net`, and the namespace has loopback and nothing else. That floor is enforced and is the
reason a process can be trusted with a workspace. It also makes a sealed secret slot worthless — a
confined vendor harness holding a model credential cannot spend it, so the run is not confined, it
is dead. The vendor case and the b10x case are one shape: one process reaching one model endpoint.

Design 04 already fixes the vocabulary — egress, listening sockets and exposed endpoints are
separate capabilities, an aperture is deployment authority, and a request cannot widen it. Missing
are the mechanism, the declaration surface and the refusal.

## Decision

An **egress aperture** is a named operator declaration in daemon configuration: one destination
tuple of host, port and `tcp`. A request may reference an aperture **by name** and may never carry
a destination, at any depth, in any field. Configuration owns reach; a request selects among what
configuration already permitted.

The default does not move. Without an aperture the sandbox keeps `--unshare-net` and no interface.
An aperture is a separately probed capability fact, `exec.egress-apertures`, published only after
the mechanism verified in a throwaway sandbox — never after reading configuration; an unverified
mechanism leaves the fact absent with a diagnostic. DNS stays outside the aperture: the daemon
resolves the declared host once at declaration, pins the address, and gives the sandbox no resolver.

Refusals are typed and named: an undeclared aperture is `unserved` with the aperture named, an
absent fact is `unserved`, a raw destination where a name belongs is `refused`, and an aperture that
cannot be installed exactly as declared refuses the dispatch with nothing partial installed. The
applied aperture — name, pinned destination, mechanism — is an observation in the run's record and
rides the existing `exec.*` and `session.*` events. A successor bundle carries the request field,
the capability fact and the applied branch; earlier bundle bytes are unchanged.

## Consequences

Substrate gains its first outbound authority, and with it the first deployment-held decision about
where a confined process may reach. That is the cost, bounded by being operator-declared,
name-referenced, exactly matched, probed and observed. A hosted runner cannot prove the positive
half: reachability of a declared destination and unreachability of an undeclared one need a
delegated lane on a self-hosted runner. CI proves the typed refusals and the schema shape and
reports the rest absent rather than passed.
