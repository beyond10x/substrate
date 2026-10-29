//! The exec Pod's container log, decoded (design 34 § 7.3, ADR 0047 N10).
//!
//! Only the executor writes to the `run` container's stdout and stderr; the child's descriptors
//! are pipes it drains. So every line of the log is a frame, in one order: `applied` first, then
//! either a refusal, a spawn failure, or output and truncation records followed by `exit`, which
//! is last. A line that is not a frame, a frame out of that order, output past the run's bound or
//! a log that ends before its last record is a protocol failure, never output.

use substrate_executor::frame::{Applied, Control, ExitRecord, Frame, Refusal, Stream};

/// A log that is not the executor's: the output it would carry is never reported.
pub const EXECUTOR_PROTOCOL: &str = "kubernetes.executor-protocol";

/// How a decoded run ended, as its last record states.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunEnd {
    /// The argv ran and its direct child ended.
    Exit {
        /// The child's stdout, as framed.
        stdout: Vec<u8>,
        /// The child's stderr, as framed.
        stderr: Vec<u8>,
        /// The executor discarded stdout past the run's bound.
        stdout_truncated: bool,
        /// The executor discarded stderr past the run's bound.
        stderr_truncated: bool,
        /// The executor's own record of the end, which the driver holds to the API's exit code.
        record: ExitRecord,
    },
    /// The argv was not started.
    Refused(Refusal),
    /// The argv could not be started.
    SpawnFailed {
        /// The `execve` errno.
        errno: i32,
    },
}

/// One exec's log, decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decoded {
    /// The bounds the executor read back from its cgroup.
    pub applied: Applied,
    /// How the run ended.
    pub end: RunEnd,
}

/// Decodes `log`, the whole container log of a run bounded at `output_bytes` per stream.
///
/// # Errors
///
/// What makes `log` not the executor's: a line that is not a frame, a frame out of order, output
/// past `output_bytes` or after its stream's truncation record, or a log that ends before its last
/// record or without a newline.
pub fn decode(log: &[u8], output_bytes: u64) -> Result<Decoded, &'static str> {
    let text = std::str::from_utf8(log).map_err(|_| "the log is not UTF-8")?;
    let body = text
        .strip_suffix('\n')
        .ok_or("the log does not end with a whole line")?;
    let mut lines = body
        .split('\n')
        .map(|line| Frame::decode(line).map_err(|error| error.0));
    let Some(Frame::Control(Control::Applied(applied))) = lines.next().transpose()? else {
        return Err("the log does not open with the applied record");
    };
    let mut output = [Vec::new(), Vec::new()];
    let mut truncated = [false, false];
    let mut end = None;
    for frame in lines {
        if end.is_some() {
            return Err("a frame follows the run's last record");
        }
        match frame? {
            Frame::Output { stream, bytes } => {
                let index = slot(stream);
                if truncated[index] {
                    return Err("output follows its stream's truncation record");
                }
                output[index].extend_from_slice(&bytes);
                if output[index].len() as u64 > output_bytes {
                    return Err("output passes the run's bound");
                }
            }
            Frame::Control(Control::Truncated(stream)) => {
                let index = slot(stream);
                if truncated[index] {
                    return Err("a stream is truncated twice");
                }
                truncated[index] = true;
            }
            Frame::Control(Control::Exit(record)) => {
                end = Some(RunEnd::Exit {
                    stdout: std::mem::take(&mut output[0]),
                    stderr: std::mem::take(&mut output[1]),
                    stdout_truncated: truncated[0],
                    stderr_truncated: truncated[1],
                    record,
                });
            }
            Frame::Control(Control::Refused(refusal)) => {
                if output.iter().any(|bytes| !bytes.is_empty()) || truncated.contains(&true) {
                    return Err("a refusal follows output");
                }
                end = Some(RunEnd::Refused(refusal));
            }
            Frame::Control(Control::SpawnFailed { errno }) => {
                if output.iter().any(|bytes| !bytes.is_empty()) || truncated.contains(&true) {
                    return Err("a spawn failure follows output");
                }
                end = Some(RunEnd::SpawnFailed { errno });
            }
            Frame::Control(Control::Applied(_)) => {
                return Err("the applied record is written twice");
            }
        }
    }
    let end = end.ok_or("the log ends before the run's last record")?;
    Ok(Decoded { applied, end })
}

const fn slot(stream: Stream) -> usize {
    match stream {
        Stream::Stdout => 0,
        Stream::Stderr => 1,
    }
}

#[cfg(test)]
mod tests {
    use substrate_executor::frame::{Applied, Control, ExitRecord, Frame, Refusal, Stream};

    use super::{RunEnd, decode};

    fn line(frame: &Frame) -> String {
        frame.encode().expect("encodes") + "\n"
    }

    fn applied() -> String {
        line(&Frame::Control(Control::Applied(Applied {
            memory_bytes: Some(67_108_864),
            cpu_millicores: Some(500),
            pids_max: Some("max".to_owned()),
        })))
    }

    fn output(stream: Stream, bytes: &[u8]) -> String {
        line(&Frame::Output {
            stream,
            bytes: bytes.to_vec(),
        })
    }

    fn exit(code: i32) -> String {
        line(&Frame::Control(Control::Exit(ExitRecord::Code(code))))
    }

    /// Interleaved frames decode into each stream's bytes in order, with the truncation records
    /// and the exit record.
    #[test]
    fn a_whole_log_decodes_into_both_streams() {
        let log = [
            applied(),
            output(Stream::Stdout, b"one "),
            output(Stream::Stderr, b"err"),
            output(Stream::Stdout, b"two"),
            line(&Frame::Control(Control::Truncated(Stream::Stdout))),
            exit(3),
        ]
        .concat();
        let decoded = decode(log.as_bytes(), 7).expect("decodes");
        assert_eq!(
            decoded.end,
            RunEnd::Exit {
                stdout: b"one two".to_vec(),
                stderr: b"err".to_vec(),
                stdout_truncated: true,
                stderr_truncated: false,
                record: ExitRecord::Code(3),
            }
        );
        assert_eq!(decoded.applied.cpu_millicores, Some(500));
    }

    /// A refusal ends the log with no output.
    #[test]
    fn a_limits_refusal_decodes_as_refused() {
        let log = [
            applied(),
            line(&Frame::Control(Control::Refused(Refusal::LimitsUnapplied))),
        ]
        .concat();
        assert_eq!(
            decode(log.as_bytes(), 64).expect("decodes").end,
            RunEnd::Refused(Refusal::LimitsUnapplied)
        );
    }

    /// Every log that is not the executor's is refused, never read as output: a non-frame line
    /// anywhere, frames out of order, output past the bound or after truncation, and a log that
    /// ends before its last record or mid-line.
    #[test]
    fn a_log_that_is_not_the_executors_is_refused() {
        let cases = [
            (
                "a non-frame line",
                [applied(), "hello\n".to_owned(), exit(0)].concat(),
            ),
            (
                "a non-frame first line",
                ["hello\n".to_owned(), exit(0)].concat(),
            ),
            (
                "no applied record",
                [output(Stream::Stdout, b"x"), exit(0)].concat(),
            ),
            (
                "a frame after exit",
                [applied(), exit(0), output(Stream::Stdout, b"x")].concat(),
            ),
            (
                "no exit record",
                [applied(), output(Stream::Stdout, b"x")].concat(),
            ),
            (
                "past the bound",
                [applied(), output(Stream::Stdout, b"12345678"), exit(0)].concat(),
            ),
            (
                "after truncation",
                [
                    applied(),
                    line(&Frame::Control(Control::Truncated(Stream::Stderr))),
                    output(Stream::Stderr, b"x"),
                    exit(0),
                ]
                .concat(),
            ),
            (
                "no final newline",
                [applied(), exit(0)].concat().trim_end().to_owned(),
            ),
            ("an empty log", String::new()),
            (
                "an unknown tag",
                [applied(), "b10x1 x aGk=\n".to_owned(), exit(0)].concat(),
            ),
            (
                "a blank line",
                [applied(), "\n".to_owned(), exit(0)].concat(),
            ),
        ];
        for (case, log) in cases {
            assert!(decode(log.as_bytes(), 7).is_err(), "{case} decoded");
        }
    }
}
