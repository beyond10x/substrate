#![forbid(unsafe_code)]

mod app;
mod backend_ledger;
mod delegation;
mod hosted;
mod runtime;
#[cfg(test)]
#[path = "../../../test-support/unix_socket.rs"]
mod test_unix_socket;
mod tls;

pub use app::{
    App, Authority, CONTRACT_BUNDLE, CONTRACT_BUNDLE_SHA256, Identity, SystemAuthority, router,
};
pub use backend_ledger::{StoreBackendLedger, UNSETTLED_PAGE, operation_digest};
pub use delegation::{ContextRefusal, DelegatedContextPolicy, TrustedKey, VerifiedContext};
pub use hosted::HostedIdentityConfig;
pub use runtime::{
    DaemonConfig, DelegatedContextKey, DriverSelection, EgressAperture, GitSourceConfig,
    OpsListenerConfig, SecretSlot, TcpDaemonConfig, WritableRootConfig, serve,
};
pub use tls::TlsDaemonConfig;
