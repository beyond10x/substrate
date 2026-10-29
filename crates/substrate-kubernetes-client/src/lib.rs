//! The bounded Kubernetes API client that the `pod` confinement adapter
//! (`docs/design/28-pod-confinement-adapter.md`) and the Kubernetes namespace driver
//! (`docs/design/34-kubernetes-namespace-driver.md` § 2) share.
//!
//! [`kubeconfig`] loads one operator-written JSON kubeconfig and refuses every member design 28
//! names. [`Api`] speaks to the API server itself over HTTP/1.1 and TLS with the workspace's own
//! `hyper` and `rustls`: one request per connection, each bounded in time and in body size; paged
//! metadata lists; watch streams; deletes with `preconditions.uid`; `dryRun=All`; `pods/log`;
//! `pods/exec` over WebSocket `v5.channel.k8s.io`; `Lease` reads and updates; and
//! `SelfSubjectRulesReview`. [`gone`] holds the rule that decides whether the container of a Pod a
//! read found gone is known to have stopped. The crate admits no Kubernetes client crate and runs
//! no `kubectl`.
//!
//! No error this crate returns carries the credential: [`Api`]'s `Debug` prints none of it, and
//! every message the API server sends back is cleared of the bearer token before it is quoted.
//!
//! The `fake` module, compiled for this crate's own tests and under the `fake` feature, is an
//! in-process API server. The feature is legal only on a `[dev-dependencies]` edge
//! (`cargo xtask check-packages`).

mod api;
mod exec;
pub mod gone;
pub mod kubeconfig;

#[cfg(any(test, feature = "fake"))]
pub mod fake;

#[cfg(test)]
mod client_tests;

pub use api::{
    Api, ApiError, ListForm, Listed, Pacing, Watch, WatchError, WatchEvent, continuation,
    list_items,
};
pub use exec::{EXEC_PROTOCOL, ExecOutput};
pub use gone::{PodEnd, PodTrace, container_state, never_bound, pod_end, watch_event};
pub use hyper::Method;

/// What a request asks the API server for.
pub const JSON: &str = "application/json";
/// What a metadata list asks for: a `PartialObjectMetadataList`, whose items carry each object's
/// `metadata` alone.
pub const METADATA_LIST: &str = "application/json;as=PartialObjectMetadataList;g=meta.k8s.io;v=v1";
/// The bound on one answer's body, and on the bytes one watch may read.
pub const MAX_API_BODY_BYTES: usize = 4 * 1_048_576;
/// The bound on one call to the API when the caller states none.
pub const API_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
/// The container whose end the gone-Pod evidence reads. The `pod` adapter and the namespace
/// driver both name a run's container so.
pub const RUN_CONTAINER: &str = "run";
/// The user agent a client sends when its owner names none.
pub const USER_AGENT: &str = "b10x-substrate";

/// Whether `name` is a DNS-1123 label: what a namespace or container name must be.
pub fn is_dns_label(name: &str) -> bool {
    name.len() <= 63 && is_label_form(name)
}

/// Whether `name` is a DNS-1123 subdomain: what a Pod name must be. At most 253 characters of
/// dot-separated label forms, so it holds no `/`, `?`, `&`, `%`, `,` or `=` and cannot reach
/// another path, query member or field selector term.
pub fn is_dns_subdomain(name: &str) -> bool {
    name.len() <= 253 && name.split('.').all(is_label_form)
}

/// Lowercase letters, digits and `-`, at least one, beginning and ending with a letter or digit.
fn is_label_form(name: &str) -> bool {
    let bytes = name.as_bytes();
    !bytes.is_empty()
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
        && bytes.first().is_some_and(u8::is_ascii_alphanumeric)
        && bytes.last().is_some_and(u8::is_ascii_alphanumeric)
}
