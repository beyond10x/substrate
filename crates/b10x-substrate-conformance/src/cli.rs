//! The command line (design 31 § 3 to § 5). Every value is checked here or at start; a refused
//! value is invalid input and exits 2 without a report.

use std::path::PathBuf;

use clap::{ArgGroup, Parser};

/// Certifies one deployed Substrate endpoint over HTTPS and WSS, from outside its process,
/// container and checkout, and writes `report.json` and `report.json.sha256` into `--out`.
#[derive(Debug, Parser)]
#[command(name = "b10x-substrate-conformance", version)]
#[command(group(ArgGroup::new("token").required(true).args(["token_command", "token_fd"])))]
pub struct Args {
    /// The endpoint origin, `https://<host>:<port>/`.
    #[arg(long)]
    pub endpoint: String,
    /// PEM file with the trust roots the endpoint's certificate must chain to.
    #[arg(long)]
    pub trust_roots: PathBuf,
    /// The name the certificate is verified against; defaults to the endpoint host.
    #[arg(long)]
    pub server_name: Option<String>,
    /// A name the certificate must not carry (for example a headless Service). Repeatable.
    #[arg(long = "forbidden-name")]
    pub forbidden_names: Vec<String>,
    /// Absolute path of the token command, run argv-only for each credential.
    #[arg(long)]
    pub token_command: Option<PathBuf>,
    /// One argument of the token command. Repeatable.
    #[arg(long = "token-command-arg", allow_hyphen_values = true)]
    pub token_command_argv: Vec<String>,
    /// An environment variable passed to the token command, and to no other child. Repeatable.
    #[arg(long = "token-env")]
    pub token_env: Vec<String>,
    /// An inherited Unix stream socket that answers credential requests.
    #[arg(long)]
    pub token_fd: Option<i32>,
    /// Absolute path of the restart hook, run argv-only once by the restart group.
    #[arg(long)]
    pub restart_hook: Option<PathBuf>,
    /// One argument of the restart hook. Repeatable.
    #[arg(long = "restart-hook-arg", allow_hyphen_values = true)]
    pub restart_hook_argv: Vec<String>,
    /// An environment variable passed to the restart hook, and to no other child. Repeatable.
    #[arg(long = "hook-env")]
    pub hook_env: Vec<String>,
    /// The plaintext operational listener, `http://<host>:<port>` (ADR 0045).
    #[arg(long)]
    pub ops_origin: Option<String>,
    /// The hosted scopes the credential carries.
    #[arg(long, value_delimiter = ',', default_value = "exec,observe,workspaces")]
    pub scopes: Vec<String>,
    /// Run only these groups, for debugging; the report is then partial and never passes.
    #[arg(long, value_delimiter = ',')]
    pub only: Vec<String>,
    /// The report directory; it must not exist.
    #[arg(long)]
    pub out: PathBuf,
    /// Seconds the whole run may take.
    #[arg(long, default_value_t = 300, value_parser = clap::value_parser!(u64).range(1..=3600))]
    pub deadline: u64,
    /// Seconds one connect, handshake, read, write or frame wait may take.
    #[arg(long, default_value_t = 10, value_parser = clap::value_parser!(u64).range(1..=60))]
    pub io_timeout: u64,
    /// Seconds the restart hook and the restart poll may take together.
    #[arg(long, default_value_t = 120, value_parser = clap::value_parser!(u64).range(1..=600))]
    pub restart_timeout: u64,
    /// Absolute path of the shell the exec, session and restart cases run inside the sandbox.
    #[arg(long, default_value = "/bin/sh")]
    pub probe_shell: String,
    /// The most events the retention-gap case may write.
    #[arg(long, default_value_t = 256, value_parser = clap::value_parser!(u64).range(1..=10_000))]
    pub max_generated_events: u64,
}

/// The three hosted scopes (ADR 0026).
pub const HOSTED_SCOPES: [&str; 3] = ["exec", "observe", "workspaces"];

/// An environment variable name a child may receive: `[A-Za-z_][A-Za-z0-9_]*`.
pub fn is_environment_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    bytes
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == b'_')
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        && name.len() <= 128
}

#[cfg(test)]
mod tests {
    use clap::Parser as _;

    use super::{Args, is_environment_name};

    #[test]
    fn exactly_one_token_provider_is_required() {
        let base = [
            "runner",
            "--endpoint",
            "https://a:1/",
            "--trust-roots",
            "r",
            "--out",
            "o",
        ];
        assert!(Args::try_parse_from(base).is_err());
        let mut both = base.to_vec();
        both.extend(["--token-command", "/t", "--token-fd", "4"]);
        assert!(Args::try_parse_from(both).is_err());
        let mut one = base.to_vec();
        one.extend(["--token-fd", "4"]);
        let args = Args::try_parse_from(one).expect("one provider");
        assert_eq!(args.scopes, ["exec", "observe", "workspaces"]);
        assert_eq!(
            (args.deadline, args.io_timeout, args.restart_timeout),
            (300, 10, 120)
        );
        assert_eq!(args.max_generated_events, 256);
        assert_eq!(args.probe_shell, "/bin/sh");
    }

    #[test]
    fn bounds_above_their_ceiling_are_refused() {
        for (flag, value) in [
            ("--deadline", "3601"),
            ("--io-timeout", "61"),
            ("--restart-timeout", "601"),
            ("--deadline", "0"),
        ] {
            let arguments = [
                "runner",
                "--endpoint",
                "https://a:1/",
                "--trust-roots",
                "r",
                "--out",
                "o",
                "--token-fd",
                "4",
                flag,
                value,
            ];
            assert!(Args::try_parse_from(arguments).is_err(), "{flag} {value}");
        }
    }

    #[test]
    fn environment_names_are_plain_identifiers() {
        assert!(is_environment_name("HOME"));
        assert!(is_environment_name("_A1"));
        assert!(!is_environment_name("1A"));
        assert!(!is_environment_name("A=B"));
        assert!(!is_environment_name(""));
    }
}
