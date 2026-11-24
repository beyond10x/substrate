---
format: aep.planning-md/3
id: review-result:test-ports-adversary
kind: review-result
status: active
title: 'Adversary pass on the test port reservation: no S1, no S2, two S3, both fixed'
relations:
- reviews: story:daemon-test-cannot-lose-its-tcp-port-to-another-test
revision: 1
---
Adversary pass on the port reservation of `story:daemon-test-cannot-lose-its-tcp-port-to-another-test`,
commit `e479d8c58` (`test-support/ports.rs` and its five including test binaries), 2026-11-24.
Verdict: needs change. No S1, no S2, two S3. Nothing the pass tried took a port from the lock.

## Held

- Run: one lock directory across packages. `CARGO_TARGET_TMPDIR` is `tmp` of the target directory
  in the daemon's and the SDK's test binaries; the conformance binary is inferred the same.
- Run: a second open in the same process cannot take the lock of a port handed out.
- Run: the child case's name with `--exact` selects one case in the SDK's `remote` and in the
  daemon's three binaries, and the child returns before it spawns.
- Mutant, the lock not taken in `reserve`: 3 of 3 `ports::` cases fail. Mutant, the lock dropped
  after the draw: 3 of 3 fail.
- Read: `File::try_lock` is `flock` with `LOCK_EX | LOCK_NB`; std opens files `O_CLOEXEC`, so a
  daemon a test starts inherits no lock; between the lock and the push into the held list only a
  bind runs, which does not panic; nothing deletes `tmp/test-ports`; none of the 18 sites under
  `crates/*/tests` and `xtask/tests` that bind port 0 drops its listener before a later bind; a
  test draws at most 2 ports (the helper's own case 128) from a pool of 16384;
  `local_endpoint` (`harness = false`) keeps no unused item.
- Suite, release profile, build pool: the daemon's three binaries 47 of 47 with the pass's case,
  the SDK's `remote` and the conformance `local_endpoint` 9 of 10 (the red one is finding 1).

## Findings and what was done

1. S3, introduced. The child case took the first and the fourth port this process was handed as
   the ends of the scan. A port another process releases between two draws is handed out by the
   later draw, so the fourth can lie before the first, and the count then wraps to about the
   pool's size. Seen red twice in four runs of the pass's own case, which made the release with
   opens of its own. Done: the count is `stretch(start, held, pool)`, the farthest held port from
   the scan's start, whatever the order; the case refuses a count over 1024 before it starts the
   child; `the_stretch_of_a_scan_covers_every_held_port_in_any_order` holds the function on
   numbers, the order reversed and the end of the pool crossed. The pass's case asserted the old
   premise and is replaced by that one.
2. S3, introduced, inferred and not run. Locks are shared per target directory. Two checkouts
   that test on one host share the loopback interface and no lock, and with a scan that starts
   at the process id, ids one apart started one port apart. Done: the scan starts at the process
   id times 7919, so ids one apart start 7919 ports apart in the default pool;
   `neighbouring_process_ids_start_their_scans_far_apart` holds 1024 ports or more. Between two
   checkouts the bind check at the draw stays the only guard, as before the change; the forge
   runs one checkout.

The pass's second case, that the lock directory is the target directory's own, is kept as
`the_lock_directory_is_the_target_directorys_own` in the helper, so every including binary with a
test harness runs it.

## Not established

- Whether jobs of the build pool run in a network namespace each (decides whether finding 2 can
  occur there).
- Whether anything on a hosted runner or in the kind lanes binds a port in 16384..32767; none
  was found.
- The run time of the wrapped child of finding 1; it was not run to its end.
