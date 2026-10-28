//! Loopback ports for listeners a test's daemon binds later, shared by the test binaries that
//! start daemons on TCP addresses.
//!
//! Binding port 0 and dropping the listener frees the port before the daemon binds it, and the
//! kernel gives a freed ephemeral port to the next `bind(0)` or `connect` of any thread or
//! process. Concurrent tests were handed one port: a daemon refused to start, or a readiness
//! request reached another process's listener, which reset it. These ports lie below the
//! ephemeral range, which the kernel never picks from.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};

/// The lowest port handed out; the pool runs from here to the ephemeral range.
pub const TEST_PORT_FLOOR: u16 = 16_384;

/// The test binaries that draw from the pool, each from its own slice of it, so binaries running
/// at the same time never draw the same port: a process-wide counter does not reach across
/// processes. A binary that is not listed refuses to allocate.
const BINARIES: [&str; 2] = ["tls_listener", "hosted_pool_adversary_pass1"];

/// The slices the pool is cut into; more than `BINARIES` has, so a binary can be added.
const SLICES: usize = 8;

/// A loopback address for a listener a daemon binds later: free now, given to no other caller in
/// this process, below the kernel's ephemeral range, and inside this binary's slice of the pool.
pub fn unused_address() -> SocketAddr {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let ephemeral_low = ephemeral_port_low();
    assert!(
        ephemeral_low > TEST_PORT_FLOOR + 1_024,
        "the ephemeral port range starts at {ephemeral_low}, leaving no test ports above {TEST_PORT_FLOOR}"
    );
    let (slice_start, slice_len) = slice(ephemeral_low);
    // Spread concurrent runs of one binary over its slice; the bind below catches any overlap.
    let start = usize::try_from(std::process::id()).unwrap_or(0) * 7_919;
    for _ in 0..slice_len {
        let index = slice_start + (start + NEXT.fetch_add(1, Ordering::Relaxed)) % slice_len;
        let port = TEST_PORT_FLOOR + u16::try_from(index).expect("pool index fits a port");
        let address = SocketAddr::from(([127, 0, 0, 1], port));
        if std::net::TcpListener::bind(address).is_ok() {
            return address;
        }
    }
    panic!("no free loopback port in this binary's slice of {TEST_PORT_FLOOR}..{ephemeral_low}");
}

/// This binary's slice of the pool, as an offset from `TEST_PORT_FLOOR` and a length.
fn slice(ephemeral_low: u16) -> (usize, usize) {
    let binary = env!("CARGO_CRATE_NAME");
    let position = BINARIES
        .iter()
        .position(|name| *name == binary)
        .unwrap_or_else(|| panic!("{binary} is not listed in tests/support/ports.rs BINARIES"));
    let len = usize::from(ephemeral_low - TEST_PORT_FLOOR) / SLICES;
    (position * len, len)
}

/// The first port of the kernel's ephemeral range (`bind(0)` and `connect` pick from it).
pub fn ephemeral_port_low() -> u16 {
    std::fs::read_to_string("/proc/sys/net/ipv4/ip_local_port_range")
        .ok()
        .and_then(|range| range.split_whitespace().next()?.parse().ok())
        .unwrap_or(32_768)
}

/// Every caller gets its own port, below the ephemeral range and inside this binary's slice: no
/// concurrent test, other test binary, `bind(0)` or outgoing connection can be given it before
/// the daemon binds it.
#[test]
fn unused_addresses_are_distinct_and_outside_the_ephemeral_range() {
    let callers: Vec<_> = (0..8)
        .map(|_| {
            std::thread::spawn(|| (0..16).map(|_| unused_address().port()).collect::<Vec<_>>())
        })
        .collect();
    let ports: Vec<u16> = callers
        .into_iter()
        .flat_map(|caller| caller.join().expect("allocating thread"))
        .collect();
    let distinct = ports.iter().collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        distinct.len(),
        ports.len(),
        "a port was handed out twice: {ports:?}"
    );
    let ephemeral_low = ephemeral_port_low();
    let (slice_start, slice_len) = slice(ephemeral_low);
    let first = usize::from(TEST_PORT_FLOOR) + slice_start;
    assert!(
        ports
            .iter()
            .all(|port| (first..first + slice_len).contains(&usize::from(*port))),
        "a port outside this binary's slice {first}..{} (the kernel hands out {ephemeral_low} and up): {ports:?}",
        first + slice_len
    );
}
