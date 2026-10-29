//! Design 28's kubeconfig loader: one self-contained JSON kubeconfig, its current context naming
//! the namespace, and a refusal for every member that changes who the client is or whom it trusts.

use std::path::Path;
use std::sync::Arc;

use base64::Engine as _;
use rustls::ClientConfig;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use serde_json::Value;
use tokio_rustls::TlsConnector;
use zeroize::Zeroizing;

use crate::api::Api;
use crate::is_dns_label;

/// The largest kubeconfig the loader reads.
pub const MAX_KUBECONFIG_BYTES: u64 = 1_048_576;

/// The cluster members design 28 refuses.
pub const REFUSED_CLUSTER_MEMBERS: [&str; 2] = ["proxy-url", "certificate-authority"];
/// The user members design 28 refuses.
pub const REFUSED_USER_MEMBERS: [&str; 11] = [
    "exec",
    "auth-provider",
    "username",
    "password",
    "tokenFile",
    "client-certificate",
    "client-key",
    "as",
    "as-uid",
    "as-groups",
    "as-user-extra",
];

/// Reads the kubeconfig at `path`: one regular file of at most [`MAX_KUBECONFIG_BYTES`].
///
/// # Errors
///
/// Why the file cannot be opened, inspected or read, or that it is not one bounded regular file.
pub fn read(path: &Path) -> Result<Vec<u8>, String> {
    use std::io::Read as _;
    let file = std::fs::File::open(path)
        .map_err(|error| format!("the cluster configuration cannot be opened: {error}"))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("the cluster configuration cannot be inspected: {error}"))?;
    if !metadata.is_file() || metadata.len() > MAX_KUBECONFIG_BYTES {
        return Err("the cluster configuration must be one regular file of at most 1 MiB".into());
    }
    let mut bytes = Vec::new();
    file.take(MAX_KUBECONFIG_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("the cluster configuration cannot be read: {error}"))?;
    Ok(bytes)
}

/// Parses the JSON kubeconfig `bytes` into a client for its current context's namespace, and the
/// owner's own settings, which `settings` reads from the current context (its `extensions`) once
/// the namespace is known and before the cluster and user are: so a configuration with several
/// faults is refused for the same one whatever owns it.
///
/// # Errors
///
/// The first fault found, naming the member; never a credential's value.
pub fn parse<T>(
    bytes: &[u8],
    settings: impl FnOnce(&Value, String) -> Result<T, String>,
) -> Result<(Api, T), String> {
    let config: Value = serde_json::from_slice(bytes)
        .map_err(|_| "the cluster configuration is not the JSON form of a kubeconfig".to_owned())?;
    let current = config["current-context"]
        .as_str()
        .ok_or("the cluster configuration names no current context")?;
    let context = named(&config, "contexts", current, "context")?;
    let namespace = text(context, "namespace")?
        .ok_or("the current context names no namespace")?
        .to_owned();
    if !is_dns_label(&namespace) {
        return Err("the context namespace is not a DNS-1123 label".into());
    }
    let settings = settings(context, namespace.clone())?;
    let cluster_name = text(context, "cluster")?.ok_or("the context names no cluster")?;
    let user_name = text(context, "user")?.ok_or("the context names no user")?;
    let cluster = named(&config, "clusters", cluster_name, "cluster")?;
    let user = named(&config, "users", user_name, "user")?;
    let api = Api::new(cluster, user, &namespace)?;
    Ok((api, settings))
}

fn named<'a>(config: &'a Value, list: &str, name: &str, member: &str) -> Result<&'a Value, String> {
    config[list]
        .as_array()
        .and_then(|entries| entries.iter().find(|entry| entry["name"] == name))
        .map(|entry| &entry[member])
        .filter(|value| value.is_object())
        .ok_or_else(|| format!("the current context names no {member} {name:?}"))
}

pub(crate) fn text<'a>(object: &'a Value, member: &str) -> Result<Option<&'a str>, String> {
    match &object[member] {
        Value::Null => Ok(None),
        Value::String(value) => Ok(Some(value)),
        _ => Err(format!("{member} is not a string")),
    }
}

fn decode(object: &Value, member: &str) -> Result<Option<Zeroizing<Vec<u8>>>, String> {
    text(object, member)?
        .map(|data| {
            base64::engine::general_purpose::STANDARD
                .decode(data)
                .map(Zeroizing::new)
                .map_err(|_| format!("{member} is not base64"))
        })
        .transpose()
}

/// The refusals of [`REFUSED_CLUSTER_MEMBERS`], [`REFUSED_USER_MEMBERS`] and
/// `insecure-skip-tls-verify`.
pub(crate) fn refuse_members(cluster: &Value, user: &Value) -> Result<(), String> {
    if cluster["insecure-skip-tls-verify"] == Value::Bool(true) {
        return Err("insecure-skip-tls-verify is refused".into());
    }
    for member in REFUSED_CLUSTER_MEMBERS {
        if !cluster[member].is_null() {
            return Err(format!("the cluster member {member} is refused"));
        }
    }
    for member in REFUSED_USER_MEMBERS {
        if !user[member].is_null() {
            return Err(format!("the user member {member} is refused"));
        }
    }
    Ok(())
}

/// The TLS client the cluster's own CA data alone verifies, with the user's client certificate
/// when it carries one.
pub(crate) fn connector(
    cluster: &Value,
    user: &Value,
    bearer: bool,
) -> Result<TlsConnector, String> {
    let authority = decode(cluster, "certificate-authority-data")?
        .ok_or("the cluster names no certificate-authority-data")?;
    let mut roots = rustls::RootCertStore::empty();
    for certificate in rustls_pemfile::certs(&mut authority.as_slice()) {
        roots
            .add(certificate.map_err(|_| "certificate-authority-data is not PEM".to_owned())?)
            .map_err(|_| "certificate-authority-data holds an invalid certificate".to_owned())?;
    }
    if roots.is_empty() {
        return Err("certificate-authority-data holds no certificate".into());
    }
    let builder = ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|_| "no TLS protocol version is available".to_owned())?
    .with_root_certificates(roots);
    let certificate = decode(user, "client-certificate-data")?;
    let key = decode(user, "client-key-data")?;
    let mut config = match (certificate, key) {
        (Some(certificate), Some(key)) => {
            let chain: Vec<CertificateDer<'static>> =
                rustls_pemfile::certs(&mut certificate.as_slice())
                    .collect::<Result<_, _>>()
                    .map_err(|_| "client-certificate-data is not PEM".to_owned())?;
            let key: PrivateKeyDer<'static> = rustls_pemfile::private_key(&mut key.as_slice())
                .ok()
                .flatten()
                .ok_or("client-key-data holds no private key")?;
            builder
                .with_client_auth_cert(chain, key)
                .map_err(|_| "the client certificate and key do not agree".to_owned())?
        }
        (None, None) if bearer => builder.with_no_client_auth(),
        (None, None) => {
            return Err("the user carries neither a client certificate nor a token".into());
        }
        _ => return Err("a client certificate needs both its certificate and its key".into()),
    };
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(TlsConnector::from(Arc::new(config)))
}
