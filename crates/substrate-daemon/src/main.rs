#![forbid(unsafe_code)]

use std::net::SocketAddr;
use std::path::PathBuf;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as BASE64URL;
use clap::Parser;
use substrate_daemon::{
    DaemonConfig, DelegatedContextKey, EgressAperture, SecretSlot, TcpDaemonConfig,
    TlsDaemonConfig, serve,
};
use tracing_subscriber::EnvFilter;

/// Parses one `--secret-slot name=path`.
///
/// Splits at the **first** `=`, so a path may contain one. The name shape is the wire's own rule,
/// not a second copy of it.
fn parse_secret_slot(value: &str) -> Result<SecretSlot, String> {
    let (name, path) = value
        .split_once('=')
        .ok_or_else(|| "a secret slot is declared as name=path".to_owned())?;
    if !substrate_wire::valid_secret_slot_name(name) {
        return Err("a secret slot name must match [a-z][a-z0-9_]{0,63}".to_owned());
    }
    if path.is_empty() {
        return Err(format!("secret slot {name} declares no file"));
    }
    Ok(SecretSlot {
        name: name.to_owned(),
        path: PathBuf::from(path),
    })
}

/// A decimal byte count with an optional **binary** suffix: `1048576`, `512KiB`, `64MiB`, `2GiB`.
///
/// Never a decimal-power unit. `MB` means 1,000,000 bytes to a disk vendor and 1,048,576 to most
/// of the people who would write it in a configuration file, and a bound that means two things is
/// an operator error waiting to happen — so `1MB` is refused rather than guessed at (ADR 0014).
fn parse_byte_ceiling(value: &str) -> Result<u64, String> {
    let (digits, scale) = [
        ("KiB", 1_u64 << 10),
        ("MiB", 1 << 20),
        ("GiB", 1 << 30),
        ("TiB", 1 << 40),
    ]
    .into_iter()
    .find_map(|(suffix, scale)| value.strip_suffix(suffix).map(|digits| (digits, scale)))
    .unwrap_or((value, 1));
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(
            "a byte ceiling is a decimal count with an optional KiB/MiB/GiB/TiB suffix".to_owned(),
        );
    }
    let count: u64 = digits
        .parse()
        .map_err(|_| "a byte ceiling is out of range".to_owned())?;
    let bytes = count
        .checked_mul(scale)
        .ok_or_else(|| "a byte ceiling is out of range".to_owned())?;
    if bytes == 0 {
        return Err("a byte ceiling of zero passes nothing".to_owned());
    }
    Ok(bytes)
}

fn parse_project_quota_ids(value: &str) -> Result<(u32, u32), String> {
    let (start, end) = value
        .split_once('-')
        .ok_or_else(|| "a project quota range is START-END".to_owned())?;
    let start = start
        .parse::<u32>()
        .map_err(|_| "a project quota range has an invalid start".to_owned())?;
    let end = end
        .parse::<u32>()
        .map_err(|_| "a project quota range has an invalid end".to_owned())?;
    if start == 0 || start > end || u64::from(end - start) + 1 < 128 {
        return Err("a project quota range must contain at least 128 nonzero ids".to_owned());
    }
    Ok((start, end))
}

/// Parses one `--egress-aperture name=host:port/tcp[/max=<size>]`.
///
/// The protocol suffix is required and is `tcp`, so a later slice that serves another one does not
/// silently reinterpret a declaration written today (design 10 § 9 decision 3). The ceiling is a
/// second term in the same place, and an **unrecognised term is a startup error rather than an
/// ignored one** — a term the daemon reads past would give back exactly the silent
/// reinterpretation `/tcp` exists to prevent (ADR 0014). A comma cannot be the separator: it
/// splits repeated declarations at `value_delimiter` below.
fn parse_egress_aperture(value: &str) -> Result<EgressAperture, String> {
    let (name, declaration) = value.split_once('=').ok_or_else(|| {
        "an egress aperture is declared as name=host:port/tcp[/max=<size>]".to_owned()
    })?;
    if !substrate_wire::valid_aperture_name(name) {
        return Err("an egress aperture name must match [a-z][a-z0-9_]{0,63}".to_owned());
    }
    let mut terms = declaration.split('/');
    let destination = terms.next().unwrap_or_default();
    if terms.next() != Some("tcp") {
        return Err(format!(
            "egress aperture {name} must declare a /tcp destination"
        ));
    }
    let mut max_bytes = None;
    for term in terms {
        let ceiling = term.strip_prefix("max=").ok_or_else(|| {
            format!("egress aperture {name} declares an unrecognised term /{term}")
        })?;
        if max_bytes.is_some() {
            return Err(format!(
                "egress aperture {name} declares more than one byte ceiling"
            ));
        }
        max_bytes = Some(
            parse_byte_ceiling(ceiling)
                .map_err(|reason| format!("egress aperture {name}: {reason}"))?,
        );
    }
    let (host, port) = destination
        .rsplit_once(':')
        .ok_or_else(|| format!("egress aperture {name} must declare host:port"))?;
    if host.is_empty() {
        return Err(format!("egress aperture {name} declares no host"));
    }
    let port: u16 = port
        .parse()
        .map_err(|_| format!("egress aperture {name} declares no usable port"))?;
    if port == 0 {
        return Err(format!("egress aperture {name} declares no usable port"));
    }
    Ok(EgressAperture {
        name: name.to_owned(),
        host: host.to_owned(),
        port,
        max_bytes,
    })
}

/// Parses one `--delegated-context-key kid=issuer=base64url-ed25519-public-key`.
///
/// Public material only. Substrate mints no delegated context, so there is no shape of this flag
/// that takes a seed or a signing key, and which service signs is exactly this declaration
/// (ADR 0011). The key is base64url without padding, so the value carries no `=` of its own and the
/// two splits are unambiguous.
fn parse_delegated_context_key(value: &str) -> Result<DelegatedContextKey, String> {
    let (kid, rest) = value.split_once('=').ok_or_else(|| {
        "a delegated-context key is declared as kid=issuer=base64url-public-key".to_owned()
    })?;
    let (issuer, encoded) = rest.split_once('=').ok_or_else(|| {
        "a delegated-context key is declared as kid=issuer=base64url-public-key".to_owned()
    })?;
    if kid.is_empty() || kid.len() > 128 {
        return Err("a delegated-context key id must be 1..=128 bytes".to_owned());
    }
    if issuer.is_empty() || issuer.len() > 512 {
        return Err(format!("delegated-context key {kid} declares no issuer"));
    }
    let raw = BASE64URL
        .decode(encoded)
        .map_err(|_| format!("delegated-context key {kid} is not unpadded base64url"))?;
    let public_key: [u8; 32] = raw
        .try_into()
        .map_err(|_| format!("delegated-context key {kid} is not a 32-byte Ed25519 key"))?;
    Ok(DelegatedContextKey {
        kid: kid.to_owned(),
        issuer: issuer.to_owned(),
        public_key,
    })
}

#[derive(Debug, Parser)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "independent clap switches are intentionally represented as boolean flags"
)]
#[command(
    name = "substrate-daemon",
    version,
    about = "b10x minimum substrate host"
)]
struct Arguments {
    #[arg(long, env = "SUBSTRATE_SOCKET")]
    socket: PathBuf,

    #[arg(long, env = "SUBSTRATE_STATE")]
    state: PathBuf,

    #[arg(long, env = "SUBSTRATE_WORKSPACES")]
    workspaces: PathBuf,

    #[arg(long, env = "SUBSTRATE_DEPLOYMENT")]
    deployment: String,

    #[arg(long = "allow-uid", env = "SUBSTRATE_ALLOW_UID", value_delimiter = ',')]
    allow_uids: Vec<u32>,

    #[arg(long, env = "SUBSTRATE_CGROUP_ROOT")]
    cgroup_root: Option<PathBuf>,

    /// Inclusive project-id range reserved exclusively for Substrate storage quotas.
    #[arg(
        long,
        env = "SUBSTRATE_PROJECT_QUOTA_IDS",
        value_name = "START-END",
        value_parser = parse_project_quota_ids
    )]
    project_quota_ids: Option<(u32, u32)>,

    #[arg(long, default_value = "/usr/bin/bwrap")]
    bubblewrap: PathBuf,

    #[arg(long, default_value_t = 10_000)]
    event_retention: u64,

    /// Stop this managed child when its parent's stdin pipe closes.
    #[arg(long, hide = true)]
    exit_on_stdin_close: bool,

    /// Declare a secret slot as `name=path` (repeatable). ADR 0012.
    ///
    /// The path never leaves this process: it is not a capability fact, not an event field and not
    /// an error message. Rotating the file behind a declared name needs no restart and invalidates
    /// no admitted operation.
    #[arg(
        long = "secret-slot",
        env = "SUBSTRATE_SECRET_SLOT",
        value_name = "NAME=PATH",
        value_delimiter = ',',
        value_parser = parse_secret_slot
    )]
    secret_slots: Vec<SecretSlot>,

    /// Declare an egress aperture as `name=host:port/tcp[/max=<size>]` (repeatable).
    /// ADR 0013, ADR 0014.
    ///
    /// This is where reach is decided, and now how much of it. A request may name one of these and
    /// may never carry a destination or a ceiling; the host is resolved once, here, and pinned for
    /// this process's lifetime. The optional ceiling bounds `to_destination + from_destination` for
    /// one run, and an aperture declared without it passes what it always passed.
    #[arg(
        long = "egress-aperture",
        env = "SUBSTRATE_EGRESS_APERTURE",
        value_name = "NAME=HOST:PORT/tcp[/max=SIZE]",
        value_delimiter = ',',
        value_parser = parse_egress_aperture
    )]
    egress_apertures: Vec<EgressAperture>,

    /// A certificate bundle a run with an aperture gets a private read-only snapshot of.
    ///
    /// Without it a sandbox has no trust anchor — it has no `/etc` at all — so TLS crosses the
    /// aperture intact and fails verification inside. Absent and unverifiable, never present and
    /// unverified.
    #[arg(long, env = "SUBSTRATE_CA_BUNDLE", value_name = "PATH")]
    ca_bundle: Option<PathBuf>,

    /// Trust a delegated-context signer as `kid=issuer=base64url-public-key` (repeatable). ADR 0011.
    ///
    /// This is the whole of "who signs". Substrate verifies the binding a document declares and
    /// records the grant it names; it never evaluates that grant and never calls the issuer.
    #[arg(
        long = "delegated-context-key",
        env = "SUBSTRATE_DELEGATED_CONTEXT_KEY",
        value_name = "KID=ISSUER=BASE64URL",
        value_delimiter = ',',
        value_parser = parse_delegated_context_key
    )]
    delegated_context_keys: Vec<DelegatedContextKey>,

    /// Refuse an effectful operation that presents no delegated context.
    ///
    /// Requires a trusted key: requiring what cannot be verified refuses every mutation, which
    /// startup rejects rather than serving.
    #[arg(
        long = "require-delegated-context",
        env = "SUBSTRATE_REQUIRE_DELEGATED_CONTEXT",
        requires = "delegated_context_keys"
    )]
    require_delegated_context: bool,

    #[arg(
        long,
        env = "SUBSTRATE_TCP_LISTEN",
        requires_all = ["tcp_bearer_file", "tcp_subject", "tcp_actor", "tcp_private_overlay"]
    )]
    tcp_listen: Option<SocketAddr>,

    #[arg(long, env = "SUBSTRATE_TCP_PATH_PREFIX", requires = "tcp_listen")]
    tcp_path_prefix: Option<String>,

    #[arg(long, env = "SUBSTRATE_TCP_BEARER_FILE", requires = "tcp_listen")]
    tcp_bearer_file: Option<PathBuf>,

    #[arg(long, env = "SUBSTRATE_TCP_SUBJECT", requires = "tcp_listen")]
    tcp_subject: Option<String>,

    #[arg(long, env = "SUBSTRATE_TCP_ACTOR", requires = "tcp_listen")]
    tcp_actor: Option<String>,

    #[arg(long, env = "SUBSTRATE_TCP_PRIVATE_OVERLAY", requires = "tcp_listen")]
    tcp_private_overlay: bool,

    #[arg(long, env = "SUBSTRATE_TCP_DEVELOPMENT_ONLY", requires = "tcp_listen")]
    tcp_development_only: bool,

    /// Bind the production HTTPS/WSS control listener (ADR 0024).
    #[arg(
        long,
        env = "SUBSTRATE_TLS_LISTEN",
        requires_all = ["tls_certificate_chain", "tls_private_key"],
        conflicts_with = "tcp_listen"
    )]
    tls_listen: Option<SocketAddr>,

    /// PEM certificate chain for the production listener. Reloaded atomically on SIGHUP.
    #[arg(
        long,
        env = "SUBSTRATE_TLS_CERTIFICATE_CHAIN",
        value_name = "PATH",
        requires = "tls_listen"
    )]
    tls_certificate_chain: Option<PathBuf>,

    /// Owner-private PEM private key for the production listener. Reloaded atomically on SIGHUP.
    #[arg(
        long,
        env = "SUBSTRATE_TLS_PRIVATE_KEY",
        value_name = "PATH",
        requires = "tls_listen"
    )]
    tls_private_key: Option<PathBuf>,
}

impl From<Arguments> for DaemonConfig {
    fn from(arguments: Arguments) -> Self {
        let tcp = arguments.tcp_listen.map(|listen| TcpDaemonConfig {
            listen,
            path_prefix: arguments.tcp_path_prefix.unwrap_or_else(|| "/".to_owned()),
            bearer_file: arguments
                .tcp_bearer_file
                .expect("clap requires a bearer file with TCP"),
            subject: arguments
                .tcp_subject
                .expect("clap requires a subject with TCP"),
            actor: arguments
                .tcp_actor
                .expect("clap requires an actor with TCP"),
            private_overlay: arguments.tcp_private_overlay,
            development_only: arguments.tcp_development_only,
        });
        let tls = arguments.tls_listen.map(|listen| TlsDaemonConfig {
            listen,
            certificate_chain: arguments
                .tls_certificate_chain
                .expect("clap requires a certificate chain with TLS"),
            private_key: arguments
                .tls_private_key
                .expect("clap requires a private key with TLS"),
        });
        Self {
            socket: arguments.socket,
            state: arguments.state,
            workspaces: arguments.workspaces,
            deployment: arguments.deployment,
            allow_uids: arguments.allow_uids,
            cgroup_root: arguments.cgroup_root,
            project_quota_ids: arguments.project_quota_ids,
            bubblewrap: arguments.bubblewrap,
            event_retention: arguments.event_retention,
            secret_slots: arguments.secret_slots,
            egress_apertures: arguments.egress_apertures,
            ca_bundle: arguments.ca_bundle,
            delegated_context_keys: arguments.delegated_context_keys,
            require_delegated_context: arguments.require_delegated_context,
            tcp,
            tls,
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
    let arguments = Arguments::parse();
    let exit_on_stdin_close = arguments.exit_on_stdin_close;
    let daemon = serve(arguments.into());
    tokio::pin!(daemon);
    if !exit_on_stdin_close {
        return daemon.await;
    }
    let mut stdin = tokio::io::stdin();
    let mut byte = [0_u8; 1];
    tokio::select! {
        result = &mut daemon => result,
        read = tokio::io::AsyncReadExt::read(&mut stdin, &mut byte) => {
            read?;
            let pid = i32::try_from(std::process::id())
                .map_err(|_| anyhow::anyhow!("process id is outside i32"))?;
            nix::sys::signal::kill(
                nix::unistd::Pid::from_raw(pid),
                nix::sys::signal::Signal::SIGTERM,
            )?;
            daemon.await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_egress_aperture, parse_project_quota_ids};

    #[test]
    fn project_quota_ids_are_nonzero_inclusive_and_bounded_below() {
        assert_eq!(
            parse_project_quota_ids("200000-200127"),
            Ok((200_000, 200_127))
        );
        for invalid in ["", "1", "0-127", "200127-200000", "1-127", "one-two"] {
            assert!(
                parse_project_quota_ids(invalid).is_err(),
                "{invalid:?} must not reserve a quota range"
            );
        }
    }

    /// The declaration that exists today parses exactly as it does today, and declares no ceiling.
    #[test]
    fn an_aperture_without_the_term_declares_no_ceiling() {
        let declared = parse_egress_aperture("model=127.0.0.1:443/tcp").expect("declared");
        assert_eq!(declared.name, "model");
        assert_eq!(declared.host, "127.0.0.1");
        assert_eq!(declared.port, 443);
        assert_eq!(declared.max_bytes, None);
    }

    /// `<size>` is a decimal byte count with an optional **binary** suffix (ADR 0014).
    #[test]
    fn the_ceiling_takes_a_byte_count_or_a_binary_suffix() {
        for (declared, expected) in [
            ("model=app.example.invalid:443/tcp/max=1048576", 1_048_576),
            ("model=app.example.invalid:443/tcp/max=512KiB", 524_288),
            ("model=app.example.invalid:443/tcp/max=64MiB", 67_108_864),
            ("model=app.example.invalid:443/tcp/max=2GiB", 2_147_483_648),
            ("model=app.example.invalid:443/tcp/max=1", 1),
        ] {
            assert_eq!(
                parse_egress_aperture(declared).expect(declared).max_bytes,
                Some(expected),
                "{declared}"
            );
        }
    }

    /// A `MB` that means two things is an operator error waiting in a configuration file, so it
    /// is refused rather than guessed at.
    #[test]
    fn a_decimal_power_unit_is_refused() {
        for declared in [
            "model=app.example.invalid:443/tcp/max=1MB",
            "model=app.example.invalid:443/tcp/max=1M",
            "model=app.example.invalid:443/tcp/max=1kB",
            "model=app.example.invalid:443/tcp/max=1GB",
            "model=app.example.invalid:443/tcp/max=1gb",
        ] {
            parse_egress_aperture(declared).expect_err(declared);
        }
    }

    /// An unrecognised term is a startup error rather than an ignored one: the `/tcp` term exists
    /// so that a declaration written today cannot be silently reinterpreted later, and a term
    /// nobody reads would give that back.
    #[test]
    fn an_unrecognised_term_is_a_startup_error() {
        for declared in [
            "model=app.example.invalid:443/tcp/turbo",
            "model=app.example.invalid:443/tcp/max=1MiB/turbo",
            "model=app.example.invalid:443/tcp/max=1MiB/max=2MiB",
            "model=app.example.invalid:443/tcp/max=",
            "model=app.example.invalid:443/tcp/max=0",
            "model=app.example.invalid:443/tcp/max=-1",
            "model=app.example.invalid:443/tcp/max=1MiB extra",
            "model=app.example.invalid:443/udp",
            "model=app.example.invalid:443",
        ] {
            parse_egress_aperture(declared).expect_err(declared);
        }
    }
}
