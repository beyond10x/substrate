//! HTTPS-only, transiently authorized Git v2 transport. No Git configuration supplies authority.

use std::any::Any;
use std::borrow::Cow;
use std::io::{self, BufRead, Read};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use gix::bstr::BStr;
use gix::protocol::transport::{
    Protocol, Service,
    client::{
        Error, MessageKind, TransportWithoutIO, WriteMode,
        blocking_io::{RequestWriter, SetServiceResponse, Transport, http, http::Http as _},
    },
};
use zeroize::Zeroizing;

use super::{failed, refused};
use crate::{DriverError, GitSourceBinding};

const FETCH_TIMEOUT: Duration = Duration::from_mins(5);
const METADATA_LIMIT: u64 = 1024 * 1024;
const HEADER_LIMIT: u64 = 64 * 1024;

pub(super) struct Control {
    pub interrupt: Arc<AtomicBool>,
    received: AtomicU64,
    max_bytes: u64,
    deadline: Instant,
    exhausted: AtomicBool,
    legacy: AtomicBool,
}

impl Control {
    pub(super) fn new(max_bytes: u64, interrupt: Arc<AtomicBool>) -> Arc<Self> {
        Arc::new(Self {
            interrupt,
            received: AtomicU64::new(0),
            max_bytes,
            deadline: Instant::now() + FETCH_TIMEOUT,
            exhausted: AtomicBool::new(false),
            legacy: AtomicBool::new(false),
        })
    }

    pub(super) fn check(&self) -> Result<(), DriverError> {
        if self.interrupt.load(Ordering::Relaxed) {
            return Err(failed("workspace.git-fetch-cancelled"));
        }
        if Instant::now() >= self.deadline {
            return Err(failed("workspace.git-fetch-timeout"));
        }
        Ok(())
    }

    pub(super) fn error(&self) -> DriverError {
        if self.exhausted.load(Ordering::Relaxed) {
            DriverError::exhausted(
                "workspace.git-transfer-limit",
                "The Git transfer exceeds the admitted byte limit.",
                "storage",
            )
        } else if self.legacy.load(Ordering::Relaxed) {
            refused("workspace.git-protocol-refused")
        } else {
            self.check()
                .err()
                .unwrap_or_else(|| failed("workspace.git-fetch-failed"))
        }
    }

    pub(super) fn received_bytes(&self) -> u64 {
        self.received.load(Ordering::Relaxed)
    }

    fn check_io(&self) -> io::Result<()> {
        self.check()
            .map_err(|_| io::Error::other("Git transfer interrupted"))
    }

    fn remaining(&self) -> u64 {
        self.max_bytes.saturating_sub(self.received_bytes())
    }

    fn exhausted(&self) -> io::Error {
        self.exhausted.store(true, Ordering::Relaxed);
        io::Error::other("Git transfer byte limit reached")
    }
}

/// Limits bytes before parsers can buffer them. `consume` charges each byte once, including when
/// callers mix `Read` and `BufRead`. Reading EOF exactly at the ceiling remains legal.
struct BoundedReader<R> {
    inner: R,
    control: Arc<Control>,
    remaining: u64,
}

impl<R> BoundedReader<R> {
    fn new(inner: R, control: &Arc<Control>, limit: u64) -> Self {
        Self {
            inner,
            control: Arc::clone(control),
            remaining: limit,
        }
    }
}

impl<R: BufRead> Read for BoundedReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        let available = self.fill_buf()?;
        let length = buffer.len().min(available.len());
        buffer[..length].copy_from_slice(&available[..length]);
        self.consume(length);
        Ok(length)
    }
}

impl<R: BufRead> BufRead for BoundedReader<R> {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        self.control.check_io()?;
        let bytes = self.inner.fill_buf()?;
        self.control.check_io()?;
        let remaining = self.remaining.min(self.control.remaining());
        if !bytes.is_empty() && remaining == 0 {
            return Err(self.control.exhausted());
        }
        let length = bytes
            .len()
            .min(usize::try_from(remaining).unwrap_or(usize::MAX));
        Ok(&bytes[..length])
    }

    fn consume(&mut self, length: usize) {
        self.inner.consume(length);
        let length = u64::try_from(length).expect("read length fits u64");
        self.remaining -= length;
        self.control.received.fetch_add(length, Ordering::Relaxed);
    }
}

struct AuthorizedHttp {
    inner: http::curl::Curl,
    locator: url::Url,
    header: Zeroizing<String>,
    control: Arc<Control>,
    posts: usize,
}

impl AuthorizedHttp {
    fn headers(
        &self,
        requested: &str,
        base_url: &str,
        headers: impl IntoIterator<Item = impl AsRef<str>>,
    ) -> Result<Vec<Zeroizing<String>>, http::Error> {
        self.control.check_io()?;
        let requested = url::Url::parse(requested)
            .map_err(|_| io::Error::other("Invalid Git transport URL"))?;
        let base = url::Url::parse(base_url)
            .map_err(|_| io::Error::other("Invalid Git transport base URL"))?;
        if base != self.locator
            || requested.origin() != self.locator.origin()
            || requested.username() != ""
            || requested.password().is_some()
            || requested.fragment().is_some()
            || !matches!(
                requested
                    .path()
                    .strip_prefix(self.locator.path().trim_end_matches('/')),
                Some("/info/refs" | "/git-upload-pack")
            )
        {
            return Err(io::Error::other("Git transport endpoint refused").into());
        }
        let mut headers: Vec<_> = headers
            .into_iter()
            .map(|header| Zeroizing::new(header.as_ref().to_owned()))
            .collect();
        headers.push(self.header.clone());
        Ok(headers)
    }
}

impl http::Http for AuthorizedHttp {
    type Headers = BoundedReader<<http::curl::Curl as http::Http>::Headers>;
    type ResponseBody = BoundedReader<<http::curl::Curl as http::Http>::ResponseBody>;
    type PostBody = <http::curl::Curl as http::Http>::PostBody;

    fn get(
        &mut self,
        url: &str,
        base_url: &str,
        headers: impl IntoIterator<Item = impl AsRef<str>>,
    ) -> Result<http::GetResponse<Self::Headers, Self::ResponseBody>, http::Error> {
        let headers = self.headers(url, base_url, headers)?;
        let response =
            self.inner
                .get(url, base_url, headers.iter().map(|header| header.as_str()))?;
        Ok(http::GetResponse {
            headers: BoundedReader::new(response.headers, &self.control, HEADER_LIMIT),
            body: BoundedReader::new(response.body, &self.control, METADATA_LIMIT),
        })
    }

    fn post(
        &mut self,
        url: &str,
        base_url: &str,
        headers: impl IntoIterator<Item = impl AsRef<str>>,
        body: http::PostBodyDataKind,
    ) -> Result<http::PostResponse<Self::Headers, Self::ResponseBody, Self::PostBody>, http::Error>
    {
        let headers = self.headers(url, base_url, headers)?;
        self.posts += 1;
        // Initial v2 materialization has one ls-refs request and one fetch request, with no haves.
        if self.posts > 2 {
            return Err(io::Error::other("Unexpected Git negotiation round").into());
        }
        let response = self.inner.post(
            url,
            base_url,
            headers.iter().map(|header| header.as_str()),
            body,
        )?;
        Ok(http::PostResponse {
            post_body: response.post_body,
            headers: BoundedReader::new(response.headers, &self.control, HEADER_LIMIT),
            body: BoundedReader::new(
                response.body,
                &self.control,
                if self.posts == 1 {
                    METADATA_LIMIT
                } else {
                    self.control.max_bytes
                },
            ),
        })
    }

    fn configure(&mut self, _: &dyn Any) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // The constructor supplies the complete source-specific policy; repository config cannot
        // replace TLS trust, headers, redirects, proxy, timeouts, or tracing options.
        Ok(())
    }
}

pub(super) struct V2Transport {
    inner: http::Transport<AuthorizedHttp>,
    control: Arc<Control>,
}

impl V2Transport {
    pub(super) fn new(
        locator: &url::Url,
        binding: &GitSourceBinding,
        authority: &Zeroizing<String>,
        control: &Arc<Control>,
    ) -> Result<Self, DriverError> {
        let mut inner = http::curl::Curl::default();
        inner
            .configure(&http::Options {
                follow_redirects: http::options::FollowRedirects::None,
                low_speed_limit_bytes_per_second: 1024,
                low_speed_time_seconds: 30,
                proxy: Some(String::new()),
                no_proxy: Some("*".to_owned()),
                connect_timeout: Some(Duration::from_secs(10)),
                ssl_ca_info: binding.ca_bundle.clone(),
                ssl_verify: true,
                http_version: Some(http::options::HttpVersion::V1_1),
                ..http::Options::default()
            })
            .map_err(|_| failed("workspace.git-trust-failed"))?;
        let http = AuthorizedHttp {
            inner,
            locator: locator.clone(),
            header: Zeroizing::new(format!(
                "X-B10X-Git-Source-Authorization: {}",
                authority.as_str()
            )),
            control: Arc::clone(control),
            posts: 0,
        };
        Ok(Self {
            inner: http::Transport::new_http(
                http,
                gix::url::parse(locator.as_str())
                    .map_err(|_| refused("workspace.git-locator-refused"))?,
                Protocol::V2,
                false,
            ),
            control: Arc::clone(control),
        })
    }
}

impl TransportWithoutIO for V2Transport {
    fn to_url(&self) -> Cow<'_, BStr> {
        self.inner.to_url()
    }

    fn supported_protocol_versions(&self) -> &[Protocol] {
        &[Protocol::V2]
    }

    fn connection_persists_across_multiple_requests(&self) -> bool {
        self.inner.connection_persists_across_multiple_requests()
    }

    fn configure(
        &mut self,
        config: &dyn Any,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.inner.configure(config)
    }
}

impl Transport for V2Transport {
    fn handshake<'a>(
        &mut self,
        service: Service,
        parameters: &'a [(&'a str, Option<&'a str>)],
    ) -> Result<SetServiceResponse<'_>, Error> {
        let response = self.inner.handshake(service, parameters)?;
        if response.actual_protocol != Protocol::V2 {
            self.control.legacy.store(true, Ordering::Relaxed);
            return Err(io::Error::other("Git protocol v2 required").into());
        }
        Ok(response)
    }

    fn request(
        &mut self,
        mode: WriteMode,
        on_read: MessageKind,
        _: bool,
    ) -> Result<RequestWriter<'_>, Error> {
        self.control.check_io()?;
        self.inner.request(mode, on_read, false)
    }
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead as _, Cursor, Read as _};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    use super::{BoundedReader, Control};

    #[test]
    fn shared_budget_charges_mixed_reads_once_and_refuses_the_first_extra_byte() {
        let control = Control::new(6, Arc::new(AtomicBool::new(false)));
        let mut header = BoundedReader::new(Cursor::new(b"ab"), &control, 6);
        let mut body = BoundedReader::new(Cursor::new(b"cdefg"), &control, 6);
        assert_eq!(header.fill_buf().expect("header"), b"ab");
        header.consume(2);
        assert!(header.fill_buf().expect("exact EOF").is_empty());
        let mut prefix = [0; 2];
        body.read_exact(&mut prefix).expect("body prefix");
        assert_eq!(&prefix, b"cd");
        assert_eq!(body.fill_buf().expect("remaining budget"), b"ef");
        body.consume(2);
        assert!(body.fill_buf().is_err());
        assert_eq!(control.received_bytes(), 6);
        assert_eq!(control.error().code, "workspace.git-transfer-limit");
    }

    #[test]
    fn metadata_ceiling_applies_even_when_the_aggregate_budget_is_larger() {
        let control = Control::new(100, Arc::new(AtomicBool::new(false)));
        let mut reader = BoundedReader::new(Cursor::new(b"abcd"), &control, 3);
        let mut result = Vec::new();
        assert!(reader.read_to_end(&mut result).is_err());
        assert_eq!(result, b"abc");
        assert_eq!(control.error().code, "workspace.git-transfer-limit");
    }

    #[test]
    fn cancellation_and_deadline_stop_reads_with_redacted_stage_errors() {
        let interrupt = Arc::new(AtomicBool::new(false));
        let control = Control::new(100, Arc::clone(&interrupt));
        let mut reader = BoundedReader::new(Cursor::new(b"body"), &control, 100);
        interrupt.store(true, Ordering::Relaxed);
        assert!(reader.fill_buf().is_err());
        assert_eq!(control.error().code, "workspace.git-fetch-cancelled");

        let mut expired = Control::new(100, Arc::new(AtomicBool::new(false)));
        Arc::get_mut(&mut expired).expect("unique control").deadline = Instant::now()
            .checked_sub(Duration::from_secs(1))
            .expect("past deadline");
        let mut reader = BoundedReader::new(Cursor::new(b"body"), &expired, 100);
        assert!(reader.fill_buf().is_err());
        assert_eq!(expired.error().code, "workspace.git-fetch-timeout");
    }
}
