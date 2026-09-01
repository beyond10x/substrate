//! Production server identity loading for the HTTPS/WSS listener (ADR 0024).

use std::fs::File;
use std::io::{Cursor, Read as _};
use std::net::SocketAddr;
use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rustls::ServerConfig;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls_pemfile::Item;
use x509_parser::prelude::FromDer as _;
use zeroize::Zeroizing;

const MAX_CERTIFICATE_CHAIN_BYTES: u64 = 1024 * 1024;
const MAX_PRIVATE_KEY_BYTES: u64 = 64 * 1024;

/// Operator-provided production TLS listener identity (ADR 0024).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TlsDaemonConfig {
    pub listen: SocketAddr,
    pub certificate_chain: PathBuf,
    pub private_key: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TlsIdentityError {
    PrivateKeyUnsafe,
    IdentityInvalid,
}

impl TlsIdentityError {
    pub(crate) const fn startup_code(self) -> &'static str {
        match self {
            Self::PrivateKeyUnsafe => "tls.private-key-unsafe",
            Self::IdentityInvalid => "tls.identity-invalid",
        }
    }
}

/// Read and validate one complete identity snapshot before any listener is bound.
///
/// No parser error is returned because it may contain fragments derived from identity material.
/// Callers receive only the stable condition named by ADR 0024.
pub(crate) fn load_server_config(
    config: &TlsDaemonConfig,
) -> Result<Arc<ServerConfig>, TlsIdentityError> {
    let certificate_bytes = read_bounded_regular_file(
        &config.certificate_chain,
        MAX_CERTIFICATE_CHAIN_BYTES,
        false,
    )
    .map_err(|()| TlsIdentityError::IdentityInvalid)?;
    let private_key_bytes = Zeroizing::new(
        read_bounded_regular_file(&config.private_key, MAX_PRIVATE_KEY_BYTES, true)
            .map_err(|()| TlsIdentityError::PrivateKeyUnsafe)?,
    );

    let certificates = parse_certificate_chain(&certificate_bytes)?;
    if certificates.is_empty() || !certificates.iter().all(certificate_is_currently_valid) {
        return Err(TlsIdentityError::IdentityInvalid);
    }
    let private_key = parse_one_private_key(&private_key_bytes)?;
    let mut server = ServerConfig::builder_with_protocol_versions(&[&rustls::version::TLS13])
        .with_no_client_auth()
        .with_single_cert(certificates, private_key)
        .map_err(|_| TlsIdentityError::IdentityInvalid)?;
    server.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(Arc::new(server))
}

fn parse_one_private_key(bytes: &[u8]) -> Result<PrivateKeyDer<'static>, TlsIdentityError> {
    let items = rustls_pemfile::read_all(&mut Cursor::new(bytes))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| TlsIdentityError::IdentityInvalid)?;
    let [item] = items.as_slice() else {
        return Err(TlsIdentityError::IdentityInvalid);
    };
    match item {
        Item::Pkcs1Key(key) => Ok(PrivateKeyDer::Pkcs1(key.clone_key())),
        Item::Pkcs8Key(key) => Ok(PrivateKeyDer::Pkcs8(key.clone_key())),
        Item::Sec1Key(key) => Ok(PrivateKeyDer::Sec1(key.clone_key())),
        _ => Err(TlsIdentityError::IdentityInvalid),
    }
}

fn parse_certificate_chain(bytes: &[u8]) -> Result<Vec<CertificateDer<'static>>, TlsIdentityError> {
    rustls_pemfile::read_all(&mut Cursor::new(bytes))
        .map(
            |item| match item.map_err(|_| TlsIdentityError::IdentityInvalid)? {
                Item::X509Certificate(certificate) => Ok(certificate),
                _ => Err(TlsIdentityError::IdentityInvalid),
            },
        )
        .collect()
}

fn certificate_is_currently_valid(certificate: &CertificateDer<'_>) -> bool {
    let Ok((remainder, certificate)) =
        x509_parser::certificate::X509Certificate::from_der(certificate)
    else {
        return false;
    };
    remainder.is_empty() && certificate.validity().is_valid()
}

fn read_bounded_regular_file(
    path: &Path,
    max_bytes: u64,
    owner_private: bool,
) -> Result<Vec<u8>, ()> {
    let mut options = File::options();
    options
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
    let mut file = options.open(path).map_err(|_| ())?;
    let metadata = file.metadata().map_err(|_| ())?;
    let mode = metadata.permissions().mode();
    if !metadata.is_file()
        || metadata.len() == 0
        || metadata.len() > max_bytes
        || (owner_private
            && (metadata.uid() != nix::unistd::geteuid().as_raw() || mode & 0o077 != 0))
    {
        return Err(());
    }
    let take = max_bytes.checked_add(1).ok_or(())?;
    let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).map_err(|_| ())?);
    file.by_ref()
        .take(take)
        .read_to_end(&mut bytes)
        .map_err(|_| ())?;
    if bytes.is_empty() || u64::try_from(bytes.len()).map_err(|_| ())? > max_bytes {
        return Err(());
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::{PermissionsExt as _, symlink};

    use rcgen::{CertifiedKey, generate_simple_self_signed};
    use tempfile::tempdir;

    use super::{TlsDaemonConfig, TlsIdentityError, load_server_config};

    fn identity() -> (String, String) {
        let CertifiedKey { cert, signing_key } =
            generate_simple_self_signed(["substrate.test".to_owned()]).expect("test identity");
        (cert.pem(), signing_key.serialize_pem())
    }

    #[test]
    fn identity_files_must_be_regular_and_the_key_owner_private() {
        let root = tempdir().expect("temporary root");
        let certificate = root.path().join("identity.pem");
        let key = root.path().join("identity.key");
        let (certificate_pem, key_pem) = identity();
        std::fs::write(&certificate, certificate_pem).expect("certificate");
        std::fs::write(&key, key_pem).expect("key");
        std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600))
            .expect("key permissions");
        let config = TlsDaemonConfig {
            listen: "127.0.0.1:0".parse().expect("address"),
            certificate_chain: certificate.clone(),
            private_key: key.clone(),
        };
        load_server_config(&config).expect("valid identity");

        std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o640))
            .expect("unsafe permissions");
        assert_eq!(
            load_server_config(&config).expect_err("group-readable key"),
            TlsIdentityError::PrivateKeyUnsafe
        );

        let key_target = root.path().join("target.key");
        std::fs::rename(&key, &key_target).expect("move key");
        symlink(&key_target, &key).expect("key symlink");
        assert_eq!(
            load_server_config(&config).expect_err("key symlink"),
            TlsIdentityError::PrivateKeyUnsafe
        );

        let certificate_target = root.path().join("target.pem");
        std::fs::rename(&certificate, &certificate_target).expect("move certificate");
        symlink(&certificate_target, &certificate).expect("certificate symlink");
        assert_eq!(
            load_server_config(&config).expect_err("certificate symlink"),
            TlsIdentityError::IdentityInvalid
        );
    }

    #[test]
    fn certificate_and_private_key_must_match() {
        let root = tempdir().expect("temporary root");
        let certificate = root.path().join("identity.pem");
        let key = root.path().join("identity.key");
        let (certificate_pem, _) = identity();
        let (_, other_key_pem) = identity();
        std::fs::write(&certificate, certificate_pem).expect("certificate");
        std::fs::write(&key, other_key_pem).expect("key");
        std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600))
            .expect("key permissions");
        let config = TlsDaemonConfig {
            listen: "127.0.0.1:0".parse().expect("address"),
            certificate_chain: certificate,
            private_key: key,
        };
        assert_eq!(
            load_server_config(&config).expect_err("mismatched identity"),
            TlsIdentityError::IdentityInvalid
        );
    }
}
