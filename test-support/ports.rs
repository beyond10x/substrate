//! Loopback ports for listeners a test's daemon binds later, shared by every test binary that
//! starts a daemon on a TCP address.
//!
//! A port is reserved across processes: it is handed out only to a process that holds an
//! exclusive lock on the port's file, `<target>/tmp/test-ports/<port>.lock`, and that process
//! keeps the lock until it ends. Every test binary that includes this file locks in that one
//! directory, so no two live processes are handed one port, whichever binaries they belong to. A
//! port whose file another process has locked is skipped. The lock ends with the process, however
//! it ends: nothing is cleaned up, and a killed test leaves no port reserved.
//!
//! A port is handed out only when a bind of it succeeds after the lock is taken, so a port that
//! something outside this scheme holds is skipped too. The pool lies below the kernel's ephemeral
//! range, from which `bind(0)` and outgoing connections are given ports, so the kernel gives none
//! of these ports to anyone between the draw and the daemon's bind.

use std::fs::{File, OpenOptions, TryLockError};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

/// The lowest port handed out; the pool runs from here to the ephemeral range.
pub const TEST_PORT_FLOOR: u16 = 16_384;

/// The lock files of the ports this process was handed, open until the process ends.
static HELD: Mutex<Vec<File>> = Mutex::new(Vec::new());

/// A loopback address for a listener a daemon binds later: free now, below the kernel's ephemeral
/// range, and reserved for this process until it ends.
pub fn unused_address() -> SocketAddr {
    draw_from(scan_start(), ephemeral_port_low())
}

/// Where this process's scan of the pool starts, before the pool's size is taken off. The lock
/// keeps processes that lock in one directory apart. The multiplier keeps apart the starts of
/// processes that do not: two checkouts with a target directory each share the loopback
/// interface and no lock, and process ids one apart then start 7919 ports apart.
fn scan_start() -> usize {
    usize::try_from(std::process::id())
        .unwrap_or(0)
        .wrapping_mul(7_919)
}

/// How many ports a scan from offset `start` passes until it has passed every port of `held`,
/// in a pool of `pool` ports. The order the ports were handed out in does not count: a port
/// another process released between two draws is handed out by the later draw.
#[allow(
    dead_code,
    reason = "only the cases below call it, and a test binary without the test harness compiles none of them"
)]
fn stretch(start: usize, held: &[u16], pool: usize) -> usize {
    held.iter()
        .map(|port| (usize::from(port - TEST_PORT_FLOOR) + pool - start % pool) % pool + 1)
        .max()
        .unwrap_or(0)
}

/// The first port of the pool from offset `start` on, wrapping around, that this process can
/// reserve and bind; its lock is kept until the process ends.
fn draw_from(start: usize, ephemeral_low: u16) -> SocketAddr {
    assert!(
        ephemeral_low > TEST_PORT_FLOOR + 1_024,
        "the ephemeral port range starts at {ephemeral_low}, leaving no test ports above {TEST_PORT_FLOOR}"
    );
    let pool = usize::from(ephemeral_low - TEST_PORT_FLOOR);
    for step in 0..pool {
        let index = (start + step) % pool;
        let port = TEST_PORT_FLOOR + u16::try_from(index).expect("a pool index fits a port");
        let Some(lock) = reserve(port) else {
            continue;
        };
        let address = SocketAddr::from(([127, 0, 0, 1], port));
        if std::net::TcpListener::bind(address).is_ok() {
            HELD.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(lock);
            return address;
        }
    }
    panic!("no loopback port in {TEST_PORT_FLOOR}..{ephemeral_low} could be reserved and bound");
}

/// The open lock file of `port` when this open holds its exclusive lock, `None` when another open
/// holds it.
fn reserve(port: u16) -> Option<File> {
    let path = lock_path(port);
    let file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .unwrap_or_else(|error| panic!("open the port lock {}: {error}", path.display()));
    match file.try_lock() {
        Ok(()) => Some(file),
        Err(TryLockError::WouldBlock) => None,
        Err(TryLockError::Error(error)) => {
            panic!("lock the port lock {}: {error}", path.display())
        }
    }
}

/// The lock file of `port`, in the one directory every test binary of the target directory uses.
fn lock_path(port: u16) -> PathBuf {
    let directory = Path::new(env!("CARGO_TARGET_TMPDIR")).join("test-ports");
    std::fs::create_dir_all(&directory)
        .unwrap_or_else(|error| panic!("create {}: {error}", directory.display()));
    directory.join(format!("{port}.lock"))
}

/// The first port of the kernel's ephemeral range (`bind(0)` and `connect` pick from it).
pub fn ephemeral_port_low() -> u16 {
    std::fs::read_to_string("/proc/sys/net/ipv4/ip_local_port_range")
        .ok()
        .and_then(|range| range.split_whitespace().next()?.parse().ok())
        .unwrap_or(32_768)
}

/// Every caller gets its own port, below the ephemeral range.
#[test]
fn unused_addresses_are_distinct_and_below_the_ephemeral_range() {
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
    assert!(
        ports
            .iter()
            .all(|port| (TEST_PORT_FLOOR..ephemeral_low).contains(port)),
        "a port outside {TEST_PORT_FLOOR}..{ephemeral_low}: {ports:?}"
    );
}

/// A port this process was handed stays reserved: a second open of its lock file, which is
/// what another process's draw makes, cannot take the lock, and the draw's own step refuses it.
#[test]
fn a_port_handed_out_cannot_be_reserved_a_second_time() {
    let port = unused_address().port();
    let second: File = OpenOptions::new()
        .write(true)
        .open(lock_path(port))
        .expect("a second open of the port's lock file");
    assert!(
        matches!(second.try_lock(), Err(TryLockError::WouldBlock)),
        "the lock of port {port} was taken through a second open while this process holds it"
    );
    assert!(
        reserve(port).is_none(),
        "port {port} was reserved a second time"
    );
}

/// The stretch a child scans covers every held port, in whatever order the ports were handed
/// out and across the end of the pool.
#[test]
fn the_stretch_of_a_scan_covers_every_held_port_in_any_order() {
    let port = |offset: u16| TEST_PORT_FLOOR + offset;
    assert_eq!(
        stretch(10, &[port(14), port(11), port(12), port(13)], 100),
        5
    );
    assert_eq!(
        stretch(10, &[port(11), port(12), port(13), port(14)], 100),
        5
    );
    assert_eq!(stretch(98, &[port(99), port(0), port(1)], 100), 4);
    assert_eq!(stretch(98, &[port(1), port(99), port(0)], 100), 4);
    assert_eq!(stretch(210, &[port(10)], 100), 1);
    assert_eq!(stretch(10, &[], 100), 0);
}

/// Process ids one apart start their scans far apart, so two processes that share no lock
/// directory do not start on neighbouring ports.
#[test]
fn neighbouring_process_ids_start_their_scans_far_apart() {
    let pool = usize::from(ephemeral_port_low() - TEST_PORT_FLOOR);
    let own = scan_start() % pool;
    let next = (scan_start() + 7_919) % pool;
    let apart = (next + pool - own) % pool;
    assert!(
        apart.min(pool - apart) >= 1_024,
        "scans of neighbouring process ids start {apart} ports apart in a pool of {pool}"
    );
}

/// The locks lie in `tmp` of the target directory this test executable was built in, the one
/// directory every test binary of that target directory uses, and in no package's own.
#[test]
fn the_lock_directory_is_the_target_directorys_own() {
    let executable = std::env::current_exe().expect("this test executable");
    let tmp = Path::new(env!("CARGO_TARGET_TMPDIR"));
    assert!(
        executable
            .ancestors()
            .any(|directory| directory.join("tmp") == tmp),
        "{} is `tmp` of no directory above {}",
        tmp.display(),
        executable.display()
    );
}

/// A child process that draws from where this process's scan starts, as many ports as that
/// scan passes up to the last port this process holds, so it passes every one of them, is
/// handed none of them.
#[test]
fn a_child_process_is_handed_none_of_the_ports_this_process_holds() {
    // Set in the child: the offset in the pool it draws from, and how many ports it draws.
    const CHILD_FIRST: &str = "SUBSTRATE_TEST_PORTS_CHILD_FIRST";
    const CHILD_COUNT: &str = "SUBSTRATE_TEST_PORTS_CHILD_COUNT";
    // What precedes each port the child reports; libtest may print its own text before the first.
    const CHILD_LINE: &str = "test-port-handed ";
    let ephemeral_low = ephemeral_port_low();
    if let Ok(start) = std::env::var(CHILD_FIRST) {
        let start: usize = start.parse().expect("the child's first offset");
        let count: usize = std::env::var(CHILD_COUNT)
            .expect("the child's count")
            .parse()
            .expect("the child's count");
        for _ in 0..count {
            println!("{CHILD_LINE}{}", draw_from(start, ephemeral_low).port());
        }
        return;
    }
    let held: Vec<u16> = (0..4).map(|_| unused_address().port()).collect();
    let pool = usize::from(ephemeral_low - TEST_PORT_FLOOR);
    let first = scan_start() % pool;
    let count = stretch(first, &held, pool);
    // The child draws `count` ports and each draw scans from `first`: a count near the pool's
    // size would run past the test budget and take the pool from the tests beside it.
    assert!(
        count <= 1_024,
        "the ports {held:?} lie {count} ports into the scan from offset {first}: the pool is crowded"
    );
    let name = format!(
        "{}::a_child_process_is_handed_none_of_the_ports_this_process_holds",
        module_path!()
            .split_once("::")
            .map_or(module_path!(), |(_, path)| path)
    );
    let output = std::process::Command::new(std::env::current_exe().expect("this test executable"))
        .args([
            name.as_str(),
            "--exact",
            "--nocapture",
            "--test-threads",
            "1",
        ])
        .env(CHILD_FIRST, first.to_string())
        .env(CHILD_COUNT, count.to_string())
        .output()
        .expect("the child test process");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "the child failed: {stdout}{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let handed: Vec<u16> = stdout
        .lines()
        .filter_map(|line| line.split_once(CHILD_LINE).map(|(_, port)| port))
        .map(|port| port.parse().expect("a port the child reported"))
        .collect();
    assert_eq!(
        handed.len(),
        count,
        "the child reported {handed:?}, not {count} ports: {stdout}"
    );
    let shared: Vec<&u16> = handed.iter().filter(|port| held.contains(port)).collect();
    assert!(
        shared.is_empty(),
        "the child was handed {shared:?}, which this process holds ({held:?})"
    );
}
