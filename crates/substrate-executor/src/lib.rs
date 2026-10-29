//! The executor the Kubernetes namespace driver runs inside the cluster (design 34 § 7, ADR 0047
//! N3, N4 and N10), and the protocol types and frame codec the driver decodes it with.
//!
//! One static binary has the roles `version`, `keeper`, `fs`, `install` and `run`. It links
//! `substrate-wire` and `substrate-guarded` and nothing else of this workspace; `cargo xtask
//! check-packages` refuses a normal dependency path to the host driver, the store, the daemon, the
//! Kubernetes client, `rustls` or `gix` (design 34 § 2).

#![allow(unsafe_code)]
#![deny(unsafe_op_in_unsafe_fn)]

use serde::{Deserialize, Serialize};

pub mod frame;
#[cfg(target_os = "linux")]
pub mod keeper;
pub mod message;
#[cfg(target_os = "linux")]
pub mod run;
#[cfg(target_os = "linux")]
pub mod serve;
pub mod spec;

/// The protocol the driver and the executor speak; a change to any frame or message is a new one.
pub const PROTOCOL: &str = "substrate-executor/1";

/// This executor's release, which the driver requires to equal its own (N4 A).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The source revision the release build stamps in, or `unknown`.
pub const SOURCE: &str = match option_env!("SUBSTRATE_EXECUTOR_SOURCE") {
    Some(source) => source,
    None => "unknown",
};

/// The one line `substrate-executor version` writes (design 34 § 7.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Handshake {
    pub protocol: String,
    pub version: String,
    pub source: String,
}

impl Handshake {
    /// This build's handshake.
    pub fn current() -> Self {
        Self {
            protocol: PROTOCOL.to_owned(),
            version: VERSION.to_owned(),
            source: SOURCE.to_owned(),
        }
    }

    /// The handshake as one JSON line, without its newline.
    ///
    /// # Panics
    ///
    /// Never: three strings always serialize.
    pub fn line(&self) -> String {
        serde_json::to_string(self).expect("a handshake serializes")
    }

    /// Parses one handshake line.
    ///
    /// # Errors
    ///
    /// A line that is not exactly the closed handshake object.
    pub fn parse(line: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(line)
    }

    /// Whether an executor answering this handshake speaks this build's protocol at this build's
    /// release. The source revision is reported, not compared.
    pub fn is_current(&self) -> bool {
        self.protocol == PROTOCOL && self.version == VERSION
    }
}
