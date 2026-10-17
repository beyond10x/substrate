//! S6: listening sockets held by a cgroup's members.
//!
//! A socket belongs to a host process when a member of its cgroup holds a descriptor on it: the
//! `socket:[inode]` links under `/proc/<pid>/fd` give the inodes, and the kernel's own socket
//! tables under `/proc/<pid>/net/` give each inode's state and local address. Read at call time;
//! nothing is sampled in the background (wave point 13). Ownership is by descriptor, not by who
//! created the socket.

use std::collections::{BTreeSet, HashSet};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

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

/// Reads the listening sockets the given pids hold, now.
///
/// A pid that exits during the read is skipped. Any other failure to read makes the whole
/// observation absent (`None`), never an empty list that would read as "no ports".
pub(crate) fn observe(pids: &[u32]) -> Option<Vec<ListeningSocket>> {
    let mut held = HashSet::new();
    let mut tables = Vec::new();
    let mut namespaces = HashSet::new();
    for pid in pids {
        let directory = match std::fs::read_dir(format!("/proc/{pid}/fd")) {
            Ok(directory) => directory,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return None,
        };
        for entry in directory {
            let Ok(entry) = entry else { continue };
            let Ok(link) = std::fs::read_link(entry.path()) else {
                continue;
            };
            if let Some(inode) = link.to_str().and_then(socket_inode) {
                held.insert(inode);
            }
        }
        // One set of tables per network namespace the members live in.
        let namespace = match std::fs::read_link(format!("/proc/{pid}/ns/net")) {
            Ok(namespace) => namespace,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return None,
        };
        if !namespaces.insert(namespace) {
            continue;
        }
        for (name, protocol) in [
            ("tcp", SocketProtocol::Tcp),
            ("tcp6", SocketProtocol::Tcp),
            ("udp", SocketProtocol::Udp),
            ("udp6", SocketProtocol::Udp),
        ] {
            match std::fs::read_to_string(format!("/proc/{pid}/net/{name}")) {
                Ok(text) => tables.push((text, protocol)),
                // A kernel without IPv6 has no `tcp6`/`udp6`; there is nothing to list there.
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => return None,
            }
        }
    }
    let borrowed = tables
        .iter()
        .map(|(text, protocol)| (text.as_str(), *protocol))
        .collect::<Vec<_>>();
    Some(sockets_for_inodes(&held, &borrowed))
}
