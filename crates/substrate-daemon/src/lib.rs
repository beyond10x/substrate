#![forbid(unsafe_code)]

mod app;
mod delegation;
mod hosted;
mod runtime;
mod tls;

pub use app::{
    App, Authority, CONTRACT_BUNDLE, CONTRACT_BUNDLE_SHA256, Identity, SystemAuthority, router,
};
pub use delegation::{ContextRefusal, DelegatedContextPolicy, TrustedKey, VerifiedContext};
pub use hosted::HostedIdentityConfig;
pub use runtime::{
    DaemonConfig, DelegatedContextKey, EgressAperture, GitSourceConfig, OpsListenerConfig,
    SecretSlot, TcpDaemonConfig, WritableRootConfig, serve,
};
pub use tls::TlsDaemonConfig;
