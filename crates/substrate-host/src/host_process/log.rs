//! S7: one bounded log per host process.
//!
//! The program's standard output and error are stored as lines in one file per record under the
//! daemon's owner-private state directory. The file is a fixed-capacity ring: a 32-byte header
//! (magic, head offset, retained length, discarded count) followed by exactly `bound` data bytes.
//! Each line is one record, `o ` or `e ` (or `O <full_len> ` / `E <full_len> ` for a line cut at
//! the line bound), its bytes, and a newline; a record holds no other newline, so record
//! boundaries are found by scanning for one. At the byte bound the oldest whole records are
//! discarded and their bytes added to the discarded count, which the header keeps across a
//! daemon restart. The format is internal: no wire field names it (wave point 14).

use std::fs::{File, OpenOptions};
use std::os::unix::fs::{FileExt as _, OpenOptionsExt as _};
use std::path::Path;

use parking_lot::Mutex;

use crate::DriverError;

use super::{Confinement, HostProfile};

/// The byte bound of one process's retained log: at most this many record bytes are kept, and
/// the oldest are discarded and counted beyond it (design 26 § S7, wave point 14).
pub const MAX_PROCESS_LOG_BYTES: u64 = 8 * 1024 * 1024;

/// The longest line stored whole. A longer line is stored cut at this bound, marked truncated,
/// with its full length; it is never split into two lines (design 26 § S12).
pub const MAX_PROCESS_LOG_LINE_BYTES: usize = 16 * 1024;

const MAGIC: &[u8; 8] = b"SHPLOG1\0";
const HEADER_BYTES: u64 = 32;
const LOG_FAILED: &str = "process.log-failed";
const DISCARD_SCAN_BYTES: u64 = 4_096;

/// Which standard stream a line came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LogStream {
    Stdout,
    Stderr,
}

/// Which stream, or both, a tail read returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogStreamSelection {
    Stdout,
    Stderr,
    Both,
}

impl LogStreamSelection {
    const fn admits(self, stream: LogStream) -> bool {
        matches!(
            (self, stream),
            (Self::Both, _) | (Self::Stdout, LogStream::Stdout) | (Self::Stderr, LogStream::Stderr)
        )
    }
}

/// One retained line. `bytes` are exactly what the program wrote, without the newline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogLine {
    pub stream: LogStream,
    pub bytes: Vec<u8>,
    /// The line is shorter than the program wrote: cut at the line bound when stored, or cut
    /// from its front to fit a tail read's byte budget.
    pub truncated: bool,
    /// The length the program wrote.
    pub full_len: u64,
}

/// A bounded tail of one process's log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogTail {
    /// Oldest first.
    pub lines: Vec<LogLine>,
    /// Record bytes discarded at the byte bound since the log was created.
    pub discarded_bytes: u64,
    /// Record bytes the log retains now, both streams.
    pub retained_bytes: u64,
    pub profile: HostProfile,
    pub confinement: Confinement,
}

struct Ring {
    file: File,
    capacity: u64,
    head: u64,
    len: u64,
    discarded: u64,
}

/// One process's bounded log file.
pub(crate) struct ProcessLog {
    ring: Mutex<Ring>,
    max_line: usize,
}

impl std::fmt::Debug for ProcessLog {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("ProcessLog").finish_non_exhaustive()
    }
}

fn failed(context: &str, error: &std::io::Error) -> DriverError {
    DriverError::failed(LOG_FAILED, format!("process log {context}: {error}"))
}

impl ProcessLog {
    /// Creates the log at the default bounds.
    pub(crate) fn create(path: &Path) -> Result<Self, DriverError> {
        Self::create_bounded(path, MAX_PROCESS_LOG_BYTES, MAX_PROCESS_LOG_LINE_BYTES)
    }

    /// Creates a new owner-private log file. An existing path — a planted symlink included — is
    /// refused rather than opened or followed.
    pub(crate) fn create_bounded(
        path: &Path,
        bound: u64,
        max_line: usize,
    ) -> Result<Self, DriverError> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)
            .map_err(|error| failed("create", &error))?;
        file.set_len(HEADER_BYTES + bound)
            .map_err(|error| failed("size", &error))?;
        let ring = Ring {
            file,
            capacity: bound,
            head: 0,
            len: 0,
            discarded: 0,
        };
        ring.write_header()?;
        Ok(Self {
            ring: Mutex::new(ring),
            max_line,
        })
    }

    /// Opens an existing log for reading and appending, as a restarted daemon finds it.
    pub(crate) fn open_existing(path: &Path) -> Result<Self, DriverError> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)
            .map_err(|error| failed("open", &error))?;
        let size = file
            .metadata()
            .map_err(|error| failed("metadata", &error))?
            .len();
        let mut header = [0_u8; 32];
        file.read_exact_at(&mut header, 0)
            .map_err(|error| failed("header", &error))?;
        let word = |index: usize| {
            u64::from_le_bytes(
                header[index * 8..index * 8 + 8]
                    .try_into()
                    .expect("eight header bytes"),
            )
        };
        let capacity = size.saturating_sub(HEADER_BYTES);
        let (head, len, discarded) = (word(1), word(2), word(3));
        if &header[..8] != MAGIC || capacity == 0 || head >= capacity || len > capacity {
            return Err(DriverError::failed(
                LOG_FAILED,
                "process log header is not a host-process log",
            ));
        }
        Ok(Self {
            ring: Mutex::new(Ring {
                file,
                capacity,
                head,
                len,
                discarded,
            }),
            max_line: MAX_PROCESS_LOG_LINE_BYTES,
        })
    }

    /// The longest line this log stores whole.
    pub(crate) const fn max_line(&self) -> usize {
        self.max_line
    }

    /// Appends one line. `full_len` is the length the program wrote when `bytes` were cut at the
    /// line bound; `None` means `bytes` are the whole line.
    pub(crate) fn append_line(
        &self,
        stream: LogStream,
        bytes: &[u8],
        full_len: Option<u64>,
    ) -> Result<(), DriverError> {
        let bytes = &bytes[..bytes.len().min(self.max_line)];
        let mut record = Vec::with_capacity(bytes.len() + 24);
        let tag = match stream {
            LogStream::Stdout => b'o',
            LogStream::Stderr => b'e',
        };
        match full_len {
            Some(full) if full > bytes.len() as u64 => {
                record.push(tag.to_ascii_uppercase());
                record.push(b' ');
                record.extend_from_slice(full.to_string().as_bytes());
            }
            _ => record.push(tag),
        }
        record.push(b' ');
        record.extend(
            bytes
                .iter()
                .map(|byte| if *byte == b'\n' { b' ' } else { *byte }),
        );
        record.push(b'\n');
        self.ring.lock().append(&record)
    }

    /// The newest lines of `selection` whose bytes fit `max_bytes`, oldest first. A newest line
    /// longer than the whole budget is returned cut from its front, marked truncated.
    pub(crate) fn tail(
        &self,
        selection: LogStreamSelection,
        max_bytes: u64,
    ) -> Result<LogTail, DriverError> {
        let ring = self.ring.lock();
        let data = ring.read_retained()?;
        let (discarded_bytes, retained_bytes) = (ring.discarded, ring.len);
        drop(ring);
        let mut lines = Vec::new();
        let mut budget = max_bytes;
        for record in data.split(|byte| *byte == b'\n').rev() {
            let Some(line) = parse_record(record) else {
                continue;
            };
            if !selection.admits(line.stream) {
                continue;
            }
            let size = line.bytes.len() as u64;
            if size <= budget {
                budget -= size;
                lines.push(line);
                continue;
            }
            if lines.is_empty() && budget > 0 {
                let keep = usize::try_from(budget).unwrap_or(usize::MAX);
                let start = line.bytes.len() - keep;
                lines.push(LogLine {
                    bytes: line.bytes[start..].to_vec(),
                    truncated: true,
                    ..line
                });
            }
            break;
        }
        lines.reverse();
        Ok(LogTail {
            lines,
            discarded_bytes,
            retained_bytes,
            profile: HostProfile::Host,
            confinement: Confinement::None,
        })
    }
}

fn parse_record(record: &[u8]) -> Option<LogLine> {
    let (&tag, rest) = record.split_first()?;
    let rest = rest.strip_prefix(b" ")?;
    let stream = match tag.to_ascii_lowercase() {
        b'o' => LogStream::Stdout,
        b'e' => LogStream::Stderr,
        _ => return None,
    };
    if tag.is_ascii_uppercase() {
        let space = rest.iter().position(|byte| *byte == b' ')?;
        let full_len = std::str::from_utf8(&rest[..space]).ok()?.parse().ok()?;
        return Some(LogLine {
            stream,
            bytes: rest[space + 1..].to_vec(),
            truncated: true,
            full_len,
        });
    }
    Some(LogLine {
        stream,
        bytes: rest.to_vec(),
        truncated: false,
        full_len: rest.len() as u64,
    })
}

impl Ring {
    fn write_header(&self) -> Result<(), DriverError> {
        let mut header = [0_u8; 32];
        header[..8].copy_from_slice(MAGIC);
        header[8..16].copy_from_slice(&self.head.to_le_bytes());
        header[16..24].copy_from_slice(&self.len.to_le_bytes());
        header[24..32].copy_from_slice(&self.discarded.to_le_bytes());
        self.file
            .write_all_at(&header, 0)
            .map_err(|error| failed("header", &error))
    }

    /// Reads `len` bytes of the circular data region starting at logical offset `from`.
    fn read_circular(&self, from: u64, len: u64) -> Result<Vec<u8>, DriverError> {
        let mut out = vec![0_u8; usize::try_from(len).unwrap_or(usize::MAX)];
        let start = from % self.capacity;
        let first = (self.capacity - start).min(len);
        let split = usize::try_from(first).unwrap_or(usize::MAX);
        self.file
            .read_exact_at(&mut out[..split], HEADER_BYTES + start)
            .map_err(|error| failed("read", &error))?;
        if split < out.len() {
            self.file
                .read_exact_at(&mut out[split..], HEADER_BYTES)
                .map_err(|error| failed("read", &error))?;
        }
        Ok(out)
    }

    fn read_retained(&self) -> Result<Vec<u8>, DriverError> {
        self.read_circular(self.head, self.len)
    }

    fn append(&mut self, record: &[u8]) -> Result<(), DriverError> {
        let size = record.len() as u64;
        if size > self.capacity {
            // A record larger than the whole log is discarded whole and counted.
            self.discarded += size;
            return self.write_header();
        }
        if self.len + size > self.capacity {
            // Discard whole oldest records until the new one fits, scanning forward from the
            // head a chunk at a time: a record is at most one line bound long, so this reads a
            // few kilobytes, never the whole log.
            let need = self.len + size - self.capacity;
            let mut drop = 0_u64;
            let mut offset = 0_u64;
            'scan: while offset < self.len {
                let chunk_len = (self.len - offset).min(DISCARD_SCAN_BYTES);
                let chunk = self.read_circular(self.head + offset, chunk_len)?;
                for (index, byte) in chunk.iter().enumerate() {
                    if *byte == b'\n' {
                        drop = offset + index as u64 + 1;
                        if drop >= need {
                            break 'scan;
                        }
                    }
                }
                offset += chunk_len;
            }
            if drop < need {
                drop = self.len;
            }
            self.head = (self.head + drop) % self.capacity;
            self.len -= drop;
            self.discarded += drop;
        }
        let at = (self.head + self.len) % self.capacity;
        let first = usize::try_from((self.capacity - at).min(size)).unwrap_or(usize::MAX);
        self.file
            .write_all_at(&record[..first], HEADER_BYTES + at)
            .map_err(|error| failed("write", &error))?;
        if first < record.len() {
            self.file
                .write_all_at(&record[first..], HEADER_BYTES)
                .map_err(|error| failed("write", &error))?;
        }
        self.len += size;
        self.write_header()
    }
}
