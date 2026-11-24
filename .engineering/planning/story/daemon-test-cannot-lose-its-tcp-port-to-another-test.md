---
format: aep.planning-md/3
id: story:daemon-test-cannot-lose-its-tcp-port-to-another-test
kind: story
status: draft
title: A test that starts the daemon on a TCP port cannot lose the port to another test
summary: One hosted gate run failed on a daemon that could not bind the port its test chose
tags:
- defect
- flaky
revision: 2
---
# Story: a test that starts the daemon on a TCP port cannot lose the port to another test

## Outcome

No daemon test fails because the port it chose was taken between the choice and the daemon's
bind. Seen once: pull request 254, hosted gate, 1 of 3068 tests,
`adversary_ops_listener::adversary_a_method_not_allowed_on_the_unix_listener_is_counted_among_the_refusals`
in `crates/substrate-daemon/tests/tls_listener.rs`, with `ops.listener-config-invalid: bind the
operational listener`. The rerun of the same commit passed. The cause is inferred, not shown.

## Where the requirement comes from

The collection's rule "The local gate equals the forge": a forge run that goes red gets the
change that makes it not recur. Since pull request 250 the tests run as parallel processes under
cargo-nextest. Stakeholder: the operator.

## What changes

1. Find every test that picks a free port by binding port 0, closing the socket and passing the
   number to a daemon it starts; count them in this story.
2. The daemon gets its listener from the test without a gap: an inherited descriptor, a port the
   daemon reports after binding port 0 itself, or one nextest test group for the tests that
   cannot do either. Which one is decided after step 1; a test group is the last choice.
3. No retry and no rerun rule in the gate.

## Acceptance

A case that holds the mechanism chosen in step 2, and the count of step 1 at zero outside the
named test group.

## Seen again, 2026-11-24

Hosted gate of `main` at `f9421f58f` (run 45293748554), 1 of 3406 tests:
`only_a_kubernetes_daemon_serves_the_collision_series` in
`crates/substrate-daemon/tests/kubernetes_driver_selection.rs`, "the operational listener never
answered" after 10 s. The case takes the address of its operational listener from
`unused_address()`, as two more cases of that file do. The cause is inferred, not shown. No
rerun was started: the next commit on `main` gets its own run.
