//! The one deadline every test helper gives a daemon it started to become ready.
//!
//! A full local gate on a shared workstation (load averages of 79 and 133 on twenty cores) timed
//! out two suites that pass when run alone, on deadlines of 10 s
//! (`story:test-daemon-readiness-under-load`). A deadline only costs time when the daemon never
//! becomes ready, so it is set for a loaded machine, once, here. No helper retries a start: a
//! daemon that never becomes ready fails its case, with its own stderr in the message.
//!
//! This one file is included with `#[path]` by every crate's tests that start a daemon; it is test
//! support, never part of a production path.
#![allow(
    dead_code,
    reason = "each including test binary uses the part it needs and nothing else"
)]

use std::time::Duration;

/// How long a test waits for a daemon it started to become ready.
pub const DAEMON_READINESS: Duration = Duration::from_mins(1);

/// How long the stderr of a daemon that missed [`DAEMON_READINESS`] is read after it is killed.
const STDERR_GRACE: Duration = Duration::from_secs(5);

/// Kills a daemon that did not become ready by [`DAEMON_READINESS`] and returns what it wrote to
/// its piped stderr, for the failure message.
pub async fn stderr_of_unready(child: &mut tokio::process::Child) -> String {
    use tokio::io::AsyncReadExt as _;

    let _ = child.start_kill();
    let _ = tokio::time::timeout(STDERR_GRACE, child.wait()).await;
    let mut stderr = Vec::new();
    if let Some(mut pipe) = child.stderr.take() {
        // A descendant may still hold the pipe open; what was read by then is kept.
        let _ = tokio::time::timeout(STDERR_GRACE, pipe.read_to_end(&mut stderr)).await;
    }
    String::from_utf8_lossy(&stderr).into_owned()
}
