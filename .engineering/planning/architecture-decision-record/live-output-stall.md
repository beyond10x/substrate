---
format: aep.planning-md/3
id: architecture-decision-record:live-output-stall
kind: architecture-decision-record
status: accepted
title: Bound terminal queue stalls instead of rejecting transient fullness
revision: 2
transitions:
- {from: "proposed", to: "accepted", at: "2026-10-02T22:00:42Z", actor: "human:timo", revision: 2}
---
## Decision
Accept ADR0031 for issue115, superseding only ADR0016's immediate-full termination behavior. A live output producer reserves its existing bounded queue slot asynchronously. A fixed one-second maximum no-drain stall per blocked reservation permits normal backpressure; expiry retains the existing session.output-backpressure refusal and whole-tree cancellation. A closed receiver wakes immediately under existing recorded/unrecorded semantics. Queue/frame/output limits and at most one8192-byte in-flight read stay bounded. Copy payload only after capacity is reserved, preserving unrecorded queue accounting and no-capture invariants. No wire fields, frozen bundle edits, dependency changes or new domain entity.

## Evidence and authority
Mantle realPTY continuously-drained attach repeatedly fails before authentication on Substrate0.7.8. A READY/ACK startup correction remains redafter77822bytes because attached replay still fills the queue. Current main2f81c434 (0.7.9) retains this immediate-full rule in forward_frame. Issue https://github.com/beyond10x/substrate/issues/115 records the diagnosis. Operator standing approval covers all corrective implementation waves toward Codex parity; this bounded transport repair serves that goal and keeps confinement.

## Verification
Active-consumer burst larger than queue capacity is delivered byte-for-byte without cancellation. A truly stalled consumer reaches its deadline and the named refusal; disconnect/signal/timeout promptly release reservation; recorded and unrecorded output accounting, capture ceilings and cleanup remain intact. Preserve all existing gates and released bundle bytes.
