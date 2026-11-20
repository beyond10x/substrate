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

## Amendment, 2026-11-18: the deployment bound and the run bound differ

A host that serves boxes for instances on two forges declares the model host, the control-plane
dial-in and two hosts per forge (git and API): five apertures, while each run selects at most four
of them. The daemon refused that deployment at start, and every box on the host went down with it
(substrate#225).

The bound on one run stays `MAX_EGRESS_APERTURES` (four), in the wire and its bundles. The bound on
one deployment's declarations becomes six, `MAX_DEPLOYMENT_EGRESS_APERTURES` in the daemon. Every
aperture is still operator-declared, named and reviewed; six keeps that list short enough to read.
(Timo, 2026-11-18: "raise the limit to 6".)

## Amendment, 2026-11-20: eight per run, twelve per deployment, both in the wire

A box serving a GitHub instance already selects four apertures — two forge hosts (git and API), the
model host and the control-plane dial-in — which is the whole run bound, so it cannot add the
package-registry egress its build needs (substrate#236). A host serving two forges then declares
those plus a registry or two per forge: more than six.

- The bound on one run, `MAX_EGRESS_APERTURES`, becomes **eight**. A run's `apertures` list holds
  two to eight names; the n-th listens on `127.0.0.n`, so eight addresses `127.0.0.1` to
  `127.0.0.8`, and the `listen` pattern of the applied list admits exactly those.
- The bound on one deployment's declarations becomes **twelve** and moves from the daemon into the
  wire as `MAX_DEPLOYMENT_EGRESS_APERTURES`, because the contract states it: the
  `exec.egress-apertures` fact lists every *declared* aperture. Bundles `0.6.0` to `0.26.0` state
  that list's `maxItems` as the run bound (four), so a daemon declaring five or six under the
  2026-11-18 amendment published a fact its own contract refused. From bundle `0.27.0` the fact
  list is bounded by the deployment bound and the run's list by the run bound.
- Each aperture is still operator-declared, named, resolved once and reviewed; twelve keeps the
  declaration short enough to read, and a run still reaches only what it names.

Bundle `0.27.0` publishes both bounds and adds no route. The released bundles keep their bytes and
their bounds; a client pinned to `0.26.0` reads a list of at most four and must move to `0.27.0`
to select more. (Decided 2026-11-20, issue #236.)
