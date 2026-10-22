---
format: aep.planning-md/3
id: review-result:wave-2026-10-22-writable-roots-adversary-pass-1
kind: review-result
status: active
title: Adversary pass 1, writable-roots wave
relations:
- reviews: story:writable-roots-wire-0-22-0
- reviews: story:writable-host-roots
- reviews: story:writable-git-roots
revision: 1
---
Adversary pass 1 on the writable-roots wave, against ed0d9975. Cases in `crates/substrate-daemon/tests/writable_roots_contract_adversary.rs` (1), `writable_roots_startup_adversary.rs` (5) and `writable_git_roots_adversary.rs` (3); 7 red, 2 green controls. Verdict: three introduced defects and one pre-existing, all fixed.

Held: lexical mount checks (`..`, trailing slash, reserved paths, the length bound); overlap with read-only roots and files; workdir beneath a root; sorted, deduplicated lock order; destroy and lease expiry hold a Git root; pipe-session parity; refusals in the ledger before dispatch; hash compatibility of a start without writable roots; the store migration; an entry mixing both kinds is `request.schema-invalid`; an adopted workspace is refused as a Git root.

- **A1, warning, fixed.** Bundle 0.22.0 did not publish six refusals the host and daemon return (`exec.writable-root-absent`, `-not-a-directory`, `-not-owned`, `-not-git`, `-own-workspace`, `-workspace-duplicate`). Published in the unreleased 0.22.0; no vector file changed.
- **A2, warning, fixed.** The startup protected-path check missed the daemon executable, the TLS private key and certificate chain, the hosted Identity CA bundle and the static-bearer file.
- **A3, warning, fixed.** The accepting transaction re-checked the lease of the own workspace only; a Git root whose lease fell due during admission was bound and dispatched. Now every bound Git root is re-checked.
- **A4, note, pre-existing, fixed.** A lease falling due during admission answered 500 `state.store-failed`, retriable; it now answers 409 `workspace.not-ready`.
- **A5, note, fixed.** A declared path that is not UTF-8 passed startup; it now stops startup.

Accepted as is: one host root name at two mounts (ADR 0038 does not forbid it); EACCES classified as `exec.writable-root-absent`. A start refused at acceptance leaves no ledger row, as before.

Full gate exit 0; delegated lane exit 0; 111 portable and 167 delegated runtime vectors.
