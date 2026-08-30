---
format: aep.planning-md/1
id: story:destination-bound-egress
kind: story
status: active
title: Destination-bound egress apertures are declared, verified and refused by name
summary: 'Design 04 section 6: ordinary execution has no egress; an aperture is operator authority matched to a destination. With sealed secret slots this is what unlocks a confined vendor harness.'
owner: substrate
tags:
- daemon
- host
- wire
relations:
- decomposes: epic:byte-plane-completion
- depends_on: story:sealed-secret-slots
revision: 7
---
# Story: Destination-bound egress apertures are declared, verified and refused by name

## Outcome

A confined run reaches exactly the network destinations an operator declared for it — and, with
`story:sealed-secret-slots`, a vendor harness (`codex`, `claude`) can run under substrate
confinement with its model credential and its model endpoint, which design 05 § *Progress* says is
refused until both capabilities exist.

## Context

`docs/design/04-security-and-isolation.md` § 6: egress, listening sockets and exposed endpoints
are separate capabilities; ordinary execution defaults to **no egress**; "an aperture is
deployment/operator authority" matched to a destination. The delegated lane today proves
namespace no-egress and nothing else. Atlas O3/O4 want vendor harnesses driven under the same
governor as the native loop; `autodev` dispatches only `codex` and `claude`
(`atlas/ROADMAP.md`, arrow *autodev → the fleet*). A vendor harness that cannot reach its model
is not confined, it is dead — so secrets and egress are one capability pair, and the review that
filed this story found the backlog held only the first half.

## Acceptance

A run started with a declared egress aperture reaches the declared destination and nothing else,
the applied aperture is an observation in the run's record, and a request for an aperture the
deployment did not declare is `unserved` with the aperture named — proven by the delegated lane
with a real listener inside and outside the aperture.

Evidence that satisfies it, in order:

1. **Before code**: an ADR fixing the aperture shape (host:port or name:port, protocol, whether
   DNS resolution is inside the aperture), the operator declaration surface, and the refusal.
2. A successor bundle adds the request field, the capability fact `network.egress.apertures`, the
   observation, and the refusal; earlier bundle bytes unchanged.
3. Failing-first tests: `egress_defaults_to_none`, `declared_aperture_is_reachable`,
   `undeclared_destination_is_unreachable_and_named`,
   `aperture_outside_operator_declaration_is_unserved`, `applied_aperture_is_observed`.
4. The delegated lane runs the model-free fake app-server against a loopback endpoint inside the
   aperture and proves a second endpoint outside it is unreachable.

## Out of Scope

Public ingress, DNS, TLS termination and tunnels (design 05 § 4). Brokered connector artifacts.

## Open Questions

Enforcement mechanism inside the existing namespace no-egress posture: a veth pair plus nftables
in the sandbox namespace, or a userspace proxy at a declared descriptor. Decides: the ADR. Default
if nobody answers: **namespace + nftables**, because it keeps bytes out of the daemon.

## Design draft — 2026-08-30

`docs/design/10-destination-bound-egress.md` (proposed). Finding that overturns this story's
default: option (a), namespace + nftables, needs root, and the daemon withholds `exec` when it
runs as root (`crates/substrate-host/src/probe.rs:49-51`); bubblewrap 0.11.2 has no `--netns`, so
a prepared namespace cannot be handed in. The draft recommends (c), a per-run forwarder inside the
sandbox netns, owing a `setns` spike on a delegated host before the ADR. `NetworkMode::Aperture`
already exists and is `unserved` twice (`substrate-wire/src/lib.rs:595-599`,
`operations.rs:290-300`, `process.rs:810-815`); the only executed no-egress proof today is one
`connect` to `1.1.1.1:53` (`crates/substrate-daemon/tests/runtime_vectors.rs:620-640`, ported from the retired
`scripts/check-runtime-vectors.py:348-367` on 2026-08-30).

## Progress — 2026-08-30, steps 2-4

Gate green (`bash scripts/gate.sh`, exit 0). `contracts/substrate-wire/0.6.0/` (213 files) is the
successor; `0.4.0` and `0.5.0` both still reproduce byte for byte (`diff -r` empty each), and
`git status --short contracts/` shows only the new directory.

**Mechanism (c), and it really ran here.** `strace -f -e trace=execve` counts **5 `execve` of
`/usr/bin/bwrap`** across the 9 `egress::tests::*` — these are not vacuous passes. 40 bytes were
counted through the forwarder. `Sandbox::open` deliberately refuses to skip: a sandbox that reports
no `child-pid` is `expect`-ed, not returned as absent, because silently skipping is exactly how
these cases would pass without running.

**A third silent-failure trap, in the same class as the spike's two but not named by it.**
`std::io::pipe()` is `O_CLOEXEC`, so the first version of the mechanism tests passed in 0.00 s
having opened no sandbox at all. Fixed with `pipe2(…, 0)` and a *written* barrier byte — closing the
write end only releases bwrap once every forked copy of it is gone. Recorded in
`docs/design/10a-egress-mechanism-spike.md` § 8.

**Trap 2 bites, and a test now catches it.** Injecting `child.id()` in place of the `--info-fd`
`child-pid` fails `the_mechanism_is_proven_in_a_throwaway_sandbox`. Trap 1 did **not** reproduce for
the throwaway argv — bwrap nested no second user namespace there — so `NS_GET_USERNS` is kept as the
superset, not as a demonstrated fix. Say so rather than claim a fix that was never exercised.

**`xtask/src/render.rs` can never be edited again without breaking `0.5.0`.** A bundle's
`generator.digest` is the sha256 of that file (`xtask/src/render.rs:308-312`), so a successor cannot
add a `{"$wire": …}` binding there. `MAX_EGRESS_APERTURES` is bound from `xtask/src/bundle.rs`
instead, which no bundle hashes. Recorded in `AGENTS.md` § *The gate*.

**The story and the ADR disagree on the fact name; the ADR won.** This story said
`network.egress.apertures`; ADR 0013 § *Decision* and design 10 § 6 say `exec.egress-apertures`, and
no fact name in `contracts/…/schemas/capability.json` carries two dots. Shipped as
`exec.egress-apertures`.

**Test brittleness found and fixed by the orchestrator, not the implementer.**
`egress_defaults_to_none` pinned the exact errno — `Refused` for a host listener, `Unreachable` for a
public address. Under `strace -f` this host answers `Unreachable` where it otherwise answers
`Refused`, and the test went red. Both outcomes prove the same guarantee, so the assertions now hold
*did not reach* and report the observed variant. A test that pins the variant goes red on a
different kernel — including CI's — for something that is not a regression.

**Defaults taken.**

- Per-run read-only CA snapshot behind `--ca-bundle`. Unset means no anchor, so verification fails
  inside the sandbox: **absent, not unverified** (invariant 3).
- The forwarder listens on the *declared* port, so the generated `/etc/hosts` makes the operator's
  own URL work unchanged.
- `network: "aperture"` with no name keeps the frozen `exec.network-unserved`.
- `exec.aperture-byte-limit` (design 10 § 5 row 5) **deferred**: bytes are counted and observed, the
  ceiling is not enforced. That is the one row of design 10 § 5 this change does not deliver.

**Not executed on this host: the delegated lane.**
`/sys/fs/cgroup/user.slice/user-1000.slice/session-3.scope` is root-owned and `mkdir` gives
`Permission denied`, so `check_confined_apertures` and `DELEGATED_CASES = 42` are **absent, never
reported as passed** (invariant 3). Portable lane 29/29. Step 4 of the acceptance — the fake
app-server inside the aperture with a second endpoint outside it — is written and switched on by
`SUBSTRATE_VECTORS_CGROUP_ROOT`, and has not run anywhere yet.
