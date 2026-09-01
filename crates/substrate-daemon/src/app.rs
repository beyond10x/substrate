#![allow(clippy::result_large_err)] // Axum responses are the natural typed rejection at this seam.

pub const CONTRACT_BUNDLE: &str = substrate_wire::ADVERTISED_CONTRACT_BUNDLE;
pub const CONTRACT_BUNDLE_SHA256: &str = substrate_wire::ADVERTISED_CONTRACT_BUNDLE_SHA256;

const BODY_LIMIT: usize = 2_097_152;
const WORKSPACE_LOCK_STRIPES: usize = 256;
const LEASE_CLEANUP_BATCH: usize = 32;
const WORKSPACE_CLEANUP_BATCH: usize = 32;
const RESTART_RECONCILE_BATCH: usize = 64;
const PROVISIONAL_RECOVERY_BATCH: usize = 16;
const REQUEST_BODY_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
const PIPE_MAX_INPUT_BYTES: u64 = 16 * 1_024 * 1_024;
const PIPE_MAX_FRAME_BYTES: u64 = 64 * 1_024;
const PIPE_MAX_QUEUED_FRAMES: u32 = 16;
const MAINTENANCE_DRIVER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

mod events;
mod execs;
mod metrics;
mod operations;
mod responses;
mod routes;
mod service;
mod sessions;
mod workspaces;

#[cfg(test)]
mod tests;

pub use self::routes::router;
pub(crate) use self::routes::{development_router, hosted_router};
pub(crate) use self::service::SessionTransport;
pub use self::service::{App, Authority, Identity, SystemAuthority};
