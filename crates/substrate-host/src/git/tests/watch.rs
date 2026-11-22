//! What the kernel reports for the entries of watched directories, in the order it reports it.

use std::ffi::CString;
use std::fs::File;
use std::io::Read as _;
use std::os::fd::{AsRawFd as _, FromRawFd as _};
use std::os::unix::ffi::OsStrExt as _;
use std::path::Path;

/// What happened to an entry of a watched directory, or to the directory itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::git) enum Kind {
    Created,
    MovedOut,
    MovedIn,
    /// Opened or read.
    Read,
}

/// One report: the label of the watched directory, what happened, and the entry's name, empty
/// for the directory itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::git) struct Event {
    pub(in crate::git) directory: &'static str,
    pub(in crate::git) kind: Kind,
    pub(in crate::git) name: String,
}

pub(in crate::git) struct Watch {
    queue: File,
    watched: Vec<(i32, &'static str)>,
}

impl Watch {
    pub(in crate::git) fn new() -> Self {
        // SAFETY: inotify_init1 takes only the documented flag bitset and returns an owned fd.
        let descriptor = unsafe { libc::inotify_init1(libc::IN_CLOEXEC | libc::IN_NONBLOCK) };
        assert!(
            descriptor >= 0,
            "inotify: {}",
            std::io::Error::last_os_error()
        );
        Self {
            // SAFETY: the successful call returned a new descriptor, transferred exactly once.
            queue: unsafe { File::from_raw_fd(descriptor) },
            watched: Vec::new(),
        }
    }

    /// Watches `directory`; its reports carry `label`.
    pub(in crate::git) fn add(&mut self, label: &'static str, directory: &Path) {
        let path = CString::new(directory.as_os_str().as_bytes()).expect("path without NUL");
        let mask = libc::IN_CREATE
            | libc::IN_MOVED_FROM
            | libc::IN_MOVED_TO
            | libc::IN_OPEN
            | libc::IN_ACCESS;
        // SAFETY: the queue is a live inotify descriptor and the path a live C string.
        let watch = unsafe { libc::inotify_add_watch(self.queue.as_raw_fd(), path.as_ptr(), mask) };
        assert!(watch >= 0, "watch: {}", std::io::Error::last_os_error());
        self.watched.push((watch, label));
    }

    /// The reports since the last call, oldest first.
    pub(in crate::git) fn events(&mut self) -> Vec<Event> {
        let mut bytes = Vec::new();
        let mut buffer = [0_u8; 16 * 1024];
        loop {
            match self.queue.read(&mut buffer) {
                Ok(0) => panic!("inotify closed"),
                Ok(count) => bytes.extend_from_slice(&buffer[..count]),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(error) => panic!("inotify: {error}"),
            }
        }
        let field = |at: usize| {
            u32::from_ne_bytes(
                bytes[at..at + 4]
                    .try_into()
                    .expect("four bytes of a report"),
            )
        };
        let mut events = Vec::new();
        let mut at = 0;
        // One report: the watch, the mask, a cookie, the length of the name, the padded name.
        while at < bytes.len() {
            let watch = i32::from_ne_bytes(bytes[at..at + 4].try_into().expect("watch"));
            let mask = field(at + 4);
            let length = usize::try_from(field(at + 12)).expect("name length");
            let name = &bytes[at + 16..at + 16 + length];
            let name = name.split(|byte| *byte == 0).next().unwrap_or_default();
            at += 16 + length;
            let directory = self
                .watched
                .iter()
                .find(|(known, _)| *known == watch)
                .expect("report of a watched directory")
                .1;
            let kind = if mask & libc::IN_CREATE != 0 {
                Kind::Created
            } else if mask & libc::IN_MOVED_FROM != 0 {
                Kind::MovedOut
            } else if mask & libc::IN_MOVED_TO != 0 {
                Kind::MovedIn
            } else {
                Kind::Read
            };
            events.push(Event {
                directory,
                kind,
                name: String::from_utf8_lossy(name).into_owned(),
            });
        }
        events
    }
}
