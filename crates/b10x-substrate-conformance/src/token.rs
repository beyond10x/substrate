//! The token provider (design 31 § 5): one JSON request line out, one credential line in, from a
//! token command or an inherited Unix stream socket. A credential lives in a `Zeroizing` buffer
//! for one request and never enters error text.

use std::ffi::OsString;
use std::os::fd::{FromRawFd as _, OwnedFd};
use std::os::unix::fs::FileTypeExt as _;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::net::UnixStream;
use tokio::sync::Mutex;
use zeroize::Zeroizing;

/// The bound on one provider exchange.
const PROVIDER_DEADLINE: Duration = Duration::from_secs(10);
/// The longest line a provider may answer, newline excluded.
const MAX_LINE: usize = 128;
const PREFIX: &str = "identity_access_v1_";

/// Why a credential is requested.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    Request,
    Refresh,
}

/// Which credential is requested.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    Primary,
    Narrowed,
}

/// Why no credential came back. Neither carries a provider byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenError {
    /// The provider does not serve this purpose (exit 3 or the line `unsupported`).
    Unsupported,
    /// The provider failed, timed out or answered something that is not a credential.
    Failed,
}

/// The declared provider.
pub enum TokenSource {
    Command {
        program: PathBuf,
        arguments: Vec<String>,
        environment: Vec<(String, OsString)>,
    },
    Socket(Mutex<BufReader<UnixStream>>),
}

/// Whether a line is a credential in the bundle's format.
pub fn is_credential(line: &str) -> bool {
    line.strip_prefix(PREFIX).is_some_and(|body| {
        body.len() == 43
            && body
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    })
}

/// The request line, without credential material.
pub fn request_line(reason: Reason, purpose: Purpose, scopes: &[String]) -> String {
    let reason = match reason {
        Reason::Request => "request",
        Reason::Refresh => "refresh",
    };
    let purpose = match purpose {
        Purpose::Primary => "primary",
        Purpose::Narrowed => "narrowed",
    };
    let scopes = serde_json::to_string(scopes).unwrap_or_else(|_| "[]".to_owned());
    format!("{{\"reason\":\"{reason}\",\"purpose\":\"{purpose}\",\"scopes\":{scopes}}}\n")
}

/// Reads one answer of at most [`MAX_LINE`] bytes and judges it.
fn judge(answer: &Zeroizing<Vec<u8>>) -> Result<Zeroizing<String>, TokenError> {
    let line = answer.strip_suffix(b"\n").unwrap_or(answer);
    if line == b"unsupported" {
        return Err(TokenError::Unsupported);
    }
    if line.len() > MAX_LINE || line.contains(&b'\n') {
        return Err(TokenError::Failed);
    }
    let text = std::str::from_utf8(line).map_err(|_| TokenError::Failed)?;
    if !is_credential(text) {
        return Err(TokenError::Failed);
    }
    Ok(Zeroizing::new(text.to_owned()))
}

impl TokenSource {
    /// Adopts `--token-fd`: an inherited Unix stream socket, then close-on-exec, so neither the
    /// hook nor a later child inherits it.
    ///
    /// # Errors
    ///
    /// When the descriptor is a standard stream, not open, or not a socket.
    pub fn adopt_socket(descriptor: i32) -> Result<Self, &'static str> {
        if descriptor <= 2 {
            return Err("token-fd-invalid");
        }
        let metadata = std::fs::metadata(format!("/proc/self/fd/{descriptor}"))
            .map_err(|_| "token-fd-invalid")?;
        if !metadata.file_type().is_socket() {
            return Err("token-fd-invalid");
        }
        let owned = adopt_descriptor(descriptor);
        nix::fcntl::fcntl(
            &owned,
            nix::fcntl::FcntlArg::F_SETFD(nix::fcntl::FdFlag::FD_CLOEXEC),
        )
        .map_err(|_| "token-fd-invalid")?;
        let stream = std::os::unix::net::UnixStream::from(owned);
        stream
            .set_nonblocking(true)
            .map_err(|_| "token-fd-invalid")?;
        let stream = UnixStream::from_std(stream).map_err(|_| "token-fd-invalid")?;
        Ok(Self::Socket(Mutex::new(BufReader::new(stream))))
    }

    /// One credential, or why there is none.
    pub async fn fetch(
        &self,
        reason: Reason,
        purpose: Purpose,
        scopes: &[String],
    ) -> Result<Zeroizing<String>, TokenError> {
        let request = request_line(reason, purpose, scopes);
        let exchange = async {
            match self {
                Self::Command {
                    program,
                    arguments,
                    environment,
                } => command_exchange(program, arguments, environment, &request).await,
                Self::Socket(socket) => {
                    let mut socket = socket.lock().await;
                    socket
                        .get_mut()
                        .write_all(request.as_bytes())
                        .await
                        .map_err(|_| TokenError::Failed)?;
                    let mut answer = Zeroizing::new(Vec::with_capacity(MAX_LINE + 2));
                    let mut limited = (&mut *socket).take(MAX_LINE as u64 + 2);
                    limited
                        .read_until(b'\n', &mut answer)
                        .await
                        .map_err(|_| TokenError::Failed)?;
                    if !answer.ends_with(b"\n") {
                        return Err(TokenError::Failed);
                    }
                    judge(&answer)
                }
            }
        };
        tokio::time::timeout(PROVIDER_DEADLINE, exchange)
            .await
            .map_err(|_| TokenError::Failed)?
    }
}

#[allow(
    unsafe_code,
    reason = "adopting an inherited descriptor has no safe constructor; the caller checked that it is an open socket above the standard streams"
)]
fn adopt_descriptor(descriptor: i32) -> OwnedFd {
    // SAFETY: the descriptor was inherited from the parent for this runner's exclusive use, is
    // open (its `/proc/self/fd` entry was just read), is not a standard stream, and nothing else
    // in this process owns it.
    unsafe { OwnedFd::from_raw_fd(descriptor) }
}

async fn command_exchange(
    program: &PathBuf,
    arguments: &[String],
    environment: &[(String, OsString)],
    request: &str,
) -> Result<Zeroizing<String>, TokenError> {
    let mut command = tokio::process::Command::new(program);
    command
        .args(arguments)
        .env_clear()
        .envs(environment.iter().map(|(name, value)| (name, value)))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    crate::harden::standard_streams_only(&mut command);
    let mut child = command.spawn().map_err(|_| TokenError::Failed)?;
    let mut stdin = child.stdin.take().ok_or(TokenError::Failed)?;
    let mut stdout = child.stdout.take().ok_or(TokenError::Failed)?;
    // A provider that does not read its request still answers; a closed pipe is not a failure.
    let _ = stdin.write_all(request.as_bytes()).await;
    drop(stdin);
    let mut answer = Zeroizing::new(Vec::with_capacity(MAX_LINE + 2));
    let mut limited = (&mut stdout).take(MAX_LINE as u64 + 2);
    limited
        .read_to_end(&mut answer)
        .await
        .map_err(|_| TokenError::Failed)?;
    drop(stdout);
    let status = child.wait().await.map_err(|_| TokenError::Failed)?;
    match status.code() {
        Some(0) => judge(&answer),
        Some(3) => Err(TokenError::Unsupported),
        _ => Err(TokenError::Failed),
    }
}

/// The provider shared by the SDK and the raw probe.
pub type SharedSource = Arc<TokenSource>;

#[cfg(test)]
mod tests {
    use zeroize::Zeroizing;

    use super::{Purpose, Reason, TokenError, is_credential, judge, request_line};

    #[test]
    fn only_the_bundle_format_is_a_credential() {
        let body = "A".repeat(43);
        assert!(is_credential(&format!("identity_access_v1_{body}")));
        assert!(!is_credential(&format!(
            "identity_access_v1_{}",
            "A".repeat(42)
        )));
        assert!(!is_credential(&format!("identity_access_v2_{body}")));
        assert!(!is_credential(&format!(
            "identity_access_v1_{}=",
            "A".repeat(42)
        )));
    }

    #[test]
    fn an_answer_is_one_short_line_and_unsupported_is_named() {
        let body = "b".repeat(43);
        let good = Zeroizing::new(format!("identity_access_v1_{body}\n").into_bytes());
        assert_eq!(
            judge(&good).expect("credential").as_str(),
            format!("identity_access_v1_{body}")
        );
        assert_eq!(
            judge(&Zeroizing::new(b"unsupported\n".to_vec())).err(),
            Some(TokenError::Unsupported)
        );
        assert_eq!(
            judge(&Zeroizing::new(b"identity_access_v1_short\n".to_vec())).err(),
            Some(TokenError::Failed)
        );
        assert_eq!(
            judge(&Zeroizing::new(vec![b'a'; 200])).err(),
            Some(TokenError::Failed)
        );
    }

    #[test]
    fn the_request_line_names_reason_purpose_and_scopes_only() {
        let line = request_line(Reason::Refresh, Purpose::Narrowed, &["observe".to_owned()]);
        assert_eq!(
            line,
            "{\"reason\":\"refresh\",\"purpose\":\"narrowed\",\"scopes\":[\"observe\"]}\n"
        );
    }
}
