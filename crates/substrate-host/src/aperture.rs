//! The operator's egress aperture declaration, which configuration names on every platform.

use std::net::SocketAddr;

/// One operator-declared egress aperture, resolved once at declaration (ADR 0013).
///
/// `host` and `port` are what the operator wrote; `pinned` is what it resolved to, once, at
/// startup. Both are kept because they can differ and only one of them is what the kernel is asked
/// for. The sandbox never resolves anything: it gets no resolver at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EgressAperture {
    pub name: String,
    pub host: String,
    pub port: u16,
    pub pinned: SocketAddr,
    /// The declared byte ceiling over both directions summed, per run (ADR 0014).
    ///
    /// `None` installs exactly what installed before the term existed: no comparison in the relay,
    /// nothing for the parent to read, and no field on the observation.
    pub max_bytes: Option<u64>,
}
