//! ADR 0036 S12: reading one record's log forward from a logical byte offset, as the per-process
//! event stream does. Offsets are the log's own: the first record ever written starts at 0, and
//! the oldest retained record starts at the discarded count.

use super::log::{LogRead, LogStream, ProcessLog};
use super::{ProcessLogs, StoredProcessLogs};

fn log(directory: &std::path::Path, bound: u64) -> ProcessLog {
    ProcessLog::create_bounded(&directory.join("p.log"), bound, 64).expect("log")
}

fn texts(read: &LogRead) -> Vec<&[u8]> {
    read.lines
        .iter()
        .map(|(line, _)| line.bytes.as_slice())
        .collect()
}

#[test]
fn a_read_from_zero_returns_every_line_with_the_offset_after_it() {
    let directory = tempfile::tempdir().expect("dir");
    let log = log(directory.path(), 4_096);
    log.append_line(LogStream::Stdout, b"one", None)
        .expect("append");
    log.append_line(LogStream::Stderr, b"two", None)
        .expect("append");
    assert_eq!(log.end_offset(), 12);

    let read = log
        .read_from(0, 65_536)
        .expect("read")
        .expect("valid offset");
    assert_eq!(read.gap, 0);
    assert_eq!(texts(&read), [b"one".as_slice(), b"two"]);
    assert_eq!(read.lines[0].1, 6, "o one\\n is six bytes");
    assert_eq!(read.lines[1].1, 12);
    assert_eq!(read.lines[1].0.stream, LogStream::Stderr);
    assert_eq!(read.end, 12);

    let rest = log
        .read_from(6, 65_536)
        .expect("read")
        .expect("valid offset");
    assert_eq!(texts(&rest), [b"two".as_slice()]);
    let none = log
        .read_from(12, 65_536)
        .expect("read")
        .expect("valid offset");
    assert!(none.lines.is_empty());
}

#[test]
fn an_offset_the_bound_discarded_is_a_gap_then_the_oldest_retained_line() {
    let directory = tempfile::tempdir().expect("dir");
    let log = log(directory.path(), 64);
    for index in 0..20 {
        log.append_line(
            LogStream::Stdout,
            format!("line-{index:02}").as_bytes(),
            None,
        )
        .expect("append");
    }
    let read = log
        .read_from(0, 65_536)
        .expect("read")
        .expect("valid offset");
    assert!(read.gap > 0, "the oldest bytes are gone");
    assert_eq!(read.gap % 10, 0, "whole ten-byte records were discarded");
    assert_eq!(texts(&read).last(), Some(&b"line-19".as_slice()));
    assert_eq!(read.end, 200);
    assert_eq!(read.lines.last().map(|(_, end)| *end), Some(200));
}

#[test]
fn an_offset_beyond_the_end_or_inside_a_record_is_invalid() {
    let directory = tempfile::tempdir().expect("dir");
    let log = log(directory.path(), 4_096);
    log.append_line(LogStream::Stdout, b"one", None)
        .expect("append");
    assert!(
        log.read_from(7, 65_536).expect("read").is_none(),
        "beyond the end"
    );
    assert!(
        log.read_from(3, 65_536).expect("read").is_none(),
        "inside a record"
    );
}

#[test]
fn a_read_is_bounded_by_bytes_and_never_splits_a_record() {
    let directory = tempfile::tempdir().expect("dir");
    let log = log(directory.path(), 4_096);
    for line in [b"aaaa".as_slice(), b"bbbb", b"cccc"] {
        log.append_line(LogStream::Stdout, line, None)
            .expect("append");
    }
    let read = log.read_from(0, 15).expect("read").expect("valid offset");
    assert_eq!(texts(&read), [b"aaaa".as_slice(), b"bbbb"]);
    assert_eq!(read.lines.last().map(|(_, end)| *end), Some(14));
}

/// A record larger than the whole log discards what the log retained with it, so the retained
/// records stay one contiguous range of offsets that ends at the end offset.
#[test]
fn a_record_larger_than_the_log_discards_the_retained_ones_with_it() {
    let directory = tempfile::tempdir().expect("dir");
    let path = directory.path().join("p.log");
    let log = ProcessLog::create_bounded(&path, 16, 64).expect("log");
    log.append_line(LogStream::Stdout, b"ab", None)
        .expect("append");
    log.append_line(LogStream::Stdout, &[b'x'; 40], None)
        .expect("append");
    assert_eq!(log.end_offset(), 5 + 43);
    let read = log
        .read_from(0, 65_536)
        .expect("read")
        .expect("valid offset");
    assert_eq!(read.gap, 48);
    assert!(read.lines.is_empty());
    log.append_line(LogStream::Stdout, b"cd", None)
        .expect("append");
    let read = log
        .read_from(48, 65_536)
        .expect("read")
        .expect("valid offset");
    assert_eq!(texts(&read), [b"cd".as_slice()]);
}

#[test]
fn a_stored_log_is_read_forward_through_the_port() {
    let directory = tempfile::tempdir().expect("dir");
    let state = directory.path().join("state.processes");
    std::fs::create_dir(&state).expect("state");
    let log = ProcessLog::create(&state.join("proc_known.log")).expect("log");
    log.append_line(LogStream::Stdout, b"left behind", None)
        .expect("append");
    drop(log);
    let reader = StoredProcessLogs::open(&state);
    let read = reader
        .log_read("proc_known", 0, 65_536)
        .expect("read")
        .expect("valid offset");
    assert_eq!(texts(&read), [b"left behind".as_slice()]);
    assert_eq!(reader.log_end("proc_known").expect("end"), 14);
    assert!(reader.log_read("proc_absent", 0, 65_536).is_err());
}
