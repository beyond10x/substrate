//! S6: listening sockets held by a cgroup's members.
//!
//! A socket belongs to a host process when a member of its cgroup holds a descriptor on it: the
//! `socket:[inode]` links under `/proc/<pid>/fd` give the inodes, and the kernel's own socket
//! tables under `/proc/<pid>/net/` give each inode's state and local address. Read at call time;
//! nothing is sampled in the background (wave point 13). Ownership is by descriptor, not by who
//! created the socket.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::io::Read as _;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::os::fd::AsFd as _;
use std::path::{Path, PathBuf};

/// The transport of a listening socket.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SocketProtocol {
    Tcp,
    Udp,
}

/// One socket in a listening state: TCP `LISTEN`, or a bound and unconnected UDP socket.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ListeningSocket {
    pub protocol: SocketProtocol,
    pub address: IpAddr,
    pub port: u16,
}

/// TCP `LISTEN` in `/proc/net/tcp{,6}`.
const TCP_LISTEN: &str = "0A";
/// The state an unconnected UDP socket reports in `/proc/net/udp{,6}`.
const UDP_UNCONNECTED: &str = "07";

/// The listening rows of one `/proc/net/{tcp,tcp6,udp,udp6}` table as `(inode, address, port)`.
/// A row that does not parse is skipped, never guessed at.
pub(crate) fn parse_proc_net(text: &str, protocol: SocketProtocol) -> Vec<(u64, IpAddr, u16)> {
    text.lines()
        .skip(1)
        .filter_map(|line| parse_row(line, protocol))
        .collect()
}

fn parse_row(line: &str, protocol: SocketProtocol) -> Option<(u64, IpAddr, u16)> {
    let fields = line.split_whitespace().collect::<Vec<_>>();
    // sl local rem st tx:rx tr:when retrnsmt uid timeout inode ...
    if fields.len() < 10 {
        return None;
    }
    let (local, remote, state, inode) = (fields[1], fields[2], fields[3], fields[9]);
    let listening = match protocol {
        SocketProtocol::Tcp => state == TCP_LISTEN,
        SocketProtocol::Udp => {
            let (remote_address, remote_port) = parse_endpoint(remote)?;
            state == UDP_UNCONNECTED && remote_port == 0 && remote_address.is_unspecified()
        }
    };
    if !listening {
        return None;
    }
    let (address, port) = parse_endpoint(local)?;
    let inode = inode.parse::<u64>().ok().filter(|inode| *inode != 0)?;
    Some((inode, address, port))
}

/// `ADDRESS:PORT` as the kernel prints it: the address as native-endian 32-bit words in hex, the
/// port as a hex number.
fn parse_endpoint(endpoint: &str) -> Option<(IpAddr, u16)> {
    let (address, port) = endpoint.split_once(':')?;
    let port = u16::from_str_radix(port, 16).ok()?;
    let word = |hex: &str| u32::from_str_radix(hex, 16).ok().map(u32::to_ne_bytes);
    let address = match address.len() {
        8 => IpAddr::V4(Ipv4Addr::from(word(address)?)),
        32 => {
            let mut bytes = [0_u8; 16];
            for (index, chunk) in bytes.chunks_exact_mut(4).enumerate() {
                chunk.copy_from_slice(&word(address.get(index * 8..index * 8 + 8)?)?);
            }
            IpAddr::V6(Ipv6Addr::from(bytes))
        }
        _ => return None,
    };
    Some((address, port))
}

/// The inode a `/proc/<pid>/fd/<n>` link names when it is a socket.
pub(crate) fn socket_inode(link: &str) -> Option<u64> {
    let digits = link.strip_prefix("socket:[")?.strip_suffix(']')?;
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// The listening sockets among `tables` whose inode is in `held`, sorted and without repeats.
pub(crate) fn sockets_for_inodes(
    held: &HashSet<u64>,
    tables: &[(&str, SocketProtocol)],
) -> Vec<ListeningSocket> {
    let mut found = BTreeSet::new();
    for (text, protocol) in tables {
        for (inode, address, port) in parse_proc_net(text, *protocol) {
            if held.contains(&inode) {
                found.insert(ListeningSocket {
                    protocol: *protocol,
                    address,
                    port,
                });
            }
        }
    }
    found.into_iter().collect()
}

/// The most members one observation reads; a record with more has no port observation.
pub(crate) const MAX_OBSERVED_MEMBERS: usize = 4_096;
/// The most descriptors one member's `/proc/<pid>/fd` is read for.
pub(crate) const MAX_OBSERVED_FDS: usize = 65_536;
/// The most bytes one `/proc/<pid>/net/*` table is read for.
pub(crate) const MAX_SOCKET_TABLE_BYTES: u64 = 16 * 1_048_576;

/// What a read of `/proc` decided for one member.
enum Read<T> {
    Done(T),
    /// The process exited during the read; it has nothing to report.
    Gone,
    /// The read failed or passed its bound: the whole observation is absent.
    Absent,
}

fn read_bounded(path: &str) -> Read<String> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Read::Gone,
        Err(_) => return Read::Absent,
    };
    let mut text = String::new();
    match file
        .take(MAX_SOCKET_TABLE_BYTES + 1)
        .read_to_string(&mut text)
    {
        Ok(read) if u64::try_from(read).is_ok_and(|read| read <= MAX_SOCKET_TABLE_BYTES) => {
            Read::Done(text)
        }
        _ => Read::Absent,
    }
}

/// The socket inodes `pid` holds, at most [`MAX_OBSERVED_FDS`] descriptors read.
fn held_inodes(pid: u32, held: &mut HashSet<u64>) -> Read<()> {
    let directory = match std::fs::read_dir(format!("/proc/{pid}/fd")) {
        Ok(directory) => directory,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Read::Gone,
        Err(_) => return Read::Absent,
    };
    for (index, entry) in directory.enumerate() {
        if index >= MAX_OBSERVED_FDS {
            return Read::Absent;
        }
        let Ok(entry) = entry else { continue };
        let Ok(link) = std::fs::read_link(entry.path()) else {
            continue;
        };
        if let Some(inode) = link.to_str().and_then(socket_inode) {
            held.insert(inode);
        }
    }
    Read::Done(())
}

/// The network namespace `pid` lives in.
fn namespace_of(pid: u32) -> Read<PathBuf> {
    match std::fs::read_link(format!("/proc/{pid}/ns/net")) {
        Ok(namespace) => Read::Done(namespace),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Read::Gone,
        Err(_) => Read::Absent,
    }
}

/// The four socket tables as `pid` sees them.
fn socket_tables(pid: u32) -> Read<Vec<(String, SocketProtocol)>> {
    let mut tables = Vec::new();
    for (name, protocol) in [
        ("tcp", SocketProtocol::Tcp),
        ("tcp6", SocketProtocol::Tcp),
        ("udp", SocketProtocol::Udp),
        ("udp6", SocketProtocol::Udp),
    ] {
        match read_bounded(&format!("/proc/{pid}/net/{name}")) {
            Read::Done(text) => tables.push((text, protocol)),
            // A kernel without IPv6 has no `tcp6`/`udp6`; there is nothing to list there.
            Read::Gone => {}
            Read::Absent => return Read::Absent,
        }
    }
    Read::Done(tables)
}

/// Reads the listening sockets the members of the cgroup subtree at `top` hold, now.
///
/// Each member is pinned by a pidfd before its `/proc` entries are read and counts only when that
/// pidfd still names a live process after the read and the subtree still lists its pid afterwards:
/// a live process keeps its number, so a recycled pid's descriptors are not attributed to the
/// record. A member that exits during the read is skipped. The tables are read once per network
/// namespace. A record with more than [`MAX_OBSERVED_MEMBERS`] members, a member with more than
/// [`MAX_OBSERVED_FDS`] descriptors, a table above [`MAX_SOCKET_TABLE_BYTES`], or any other failure
/// to read makes the whole observation absent (`None`), never a partial list that would read as
/// complete.
#[cfg(target_os = "linux")]
pub(crate) fn observe(top: &Path) -> Option<Vec<ListeningSocket>> {
    let pids = super::tree::members(top).ok()?;
    if pids.len() > MAX_OBSERVED_MEMBERS {
        return None;
    }
    let mut counted = Vec::new();
    let mut tables = HashMap::new();
    for pid in pids {
        let Some(pin) = super::pidfd::open(pid).ok()? else {
            continue;
        };
        let mut held = HashSet::new();
        match held_inodes(pid, &mut held) {
            Read::Done(()) => {}
            Read::Gone => continue,
            Read::Absent => return None,
        }
        let namespace = match namespace_of(pid) {
            Read::Done(namespace) => namespace,
            Read::Gone => continue,
            Read::Absent => return None,
        };
        let fresh = if tables.contains_key(&namespace) {
            None
        } else {
            match socket_tables(pid) {
                Read::Done(read) => Some(read),
                Read::Gone => continue,
                Read::Absent => return None,
            }
        };
        // Gone during the read: its number may already name another process, so nothing read
        // through it is kept.
        if !super::pidfd::send(pin.as_fd(), 0).ok()? {
            continue;
        }
        if let Some(read) = fresh {
            tables.insert(namespace.clone(), read);
        }
        counted.push((pid, held, namespace));
    }
    let still = super::tree::members(top).ok()?;
    let mut held = HashSet::new();
    let mut namespaces = HashSet::new();
    for (pid, own, namespace) in counted {
        if still.contains(&pid) {
            held.extend(own);
            namespaces.insert(namespace);
        }
    }
    let borrowed = namespaces
        .iter()
        .filter_map(|namespace| tables.get(namespace))
        .flatten()
        .map(|(text, protocol)| (text.as_str(), *protocol))
        .collect::<Vec<_>>();
    Some(sockets_for_inodes(&held, &borrowed))
}
