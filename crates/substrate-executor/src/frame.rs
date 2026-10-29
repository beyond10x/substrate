//! The lines an exec Pod's PID 1 writes to its container log (design 34 § 7.3, ADR 0047 N10).
//!
//! Every line is a frame: `b10x1 o <base64>` for a stdout chunk, `b10x1 e <base64>` for a stderr
//! chunk, or `b10x1 c <json>` for a control record. A chunk is at most [`MAX_CHUNK_BYTES`], so a
//! line stays under the CRI log line limit and the kubelet never splits one. The child's
//! descriptors are pipes the executor reads, so only the executor writes these lines; a line that
//! does not decode is a protocol failure, never output.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde::{Deserialize, Serialize};

/// The frame-line prefix and protocol generation.
pub const PREFIX: &str = "b10x1";

/// The most output bytes one frame carries.
pub const MAX_CHUNK_BYTES: usize = 6144;

/// The CRI log line limit; every frame line with its newline stays below it.
pub const MAX_LINE_BYTES: usize = 16 * 1024;

/// One output stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Stream {
    #[serde(rename = "o")]
    Stdout,
    #[serde(rename = "e")]
    Stderr,
}

impl Stream {
    const fn tag(self) -> char {
        match self {
            Self::Stdout => 'o',
            Self::Stderr => 'e',
        }
    }
}

/// The bounds the executor read back from its own cgroup before starting the argv. A value it
/// could not read or parse is `None`. `pids_max` is the file's text as read (`max` under a
/// container cgroup, design 34 § 14 Q2), reported and not compared.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Applied {
    pub memory_bytes: Option<u64>,
    pub cpu_millicores: Option<u64>,
    pub pids_max: Option<String>,
}

/// Why the argv was not started.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Refusal {
    /// A bound read back differs from the spec (exit 125).
    LimitsUnapplied,
    /// The run spec did not decode (exit 2).
    SpecInvalid,
}

/// How the direct child ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExitRecord {
    Code(i32),
    /// The lower-case signal name, such as `term` or `kill`.
    Signal(String),
}

/// One control record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum Control {
    Applied(Applied),
    Refused(Refusal),
    /// Output past the spec's bound on this stream was discarded from here on.
    Truncated(Stream),
    Exit(ExitRecord),
    /// The argv could not be started (exit 127).
    SpawnFailed {
        errno: i32,
    },
}

/// One frame line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
    Output { stream: Stream, bytes: Vec<u8> },
    Control(Control),
}

/// Why a frame was not encoded or decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameError(pub &'static str);

impl std::fmt::Display for FrameError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "not a frame: {}", self.0)
    }
}

impl std::error::Error for FrameError {}

impl Frame {
    /// The frame as one line, without its newline.
    ///
    /// # Errors
    ///
    /// An output chunk over [`MAX_CHUNK_BYTES`] or an empty one.
    pub fn encode(&self) -> Result<String, FrameError> {
        let line = match self {
            Self::Output { stream, bytes } => {
                if bytes.is_empty() {
                    return Err(FrameError("an output chunk is never empty"));
                }
                if bytes.len() > MAX_CHUNK_BYTES {
                    return Err(FrameError("an output chunk over the chunk bound"));
                }
                format!("{PREFIX} {} {}", stream.tag(), STANDARD.encode(bytes))
            }
            Self::Control(control) => {
                let json = serde_json::to_string(control)
                    .map_err(|_| FrameError("a control record that does not serialize"))?;
                format!("{PREFIX} c {json}")
            }
        };
        if line.len() + 1 >= MAX_LINE_BYTES {
            return Err(FrameError("a line at or over the CRI line limit"));
        }
        Ok(line)
    }

    /// Decodes one line, without its newline.
    ///
    /// # Errors
    ///
    /// Any line that is not exactly one frame.
    pub fn decode(line: &str) -> Result<Self, FrameError> {
        if line.len() + 1 >= MAX_LINE_BYTES {
            return Err(FrameError("a line at or over the CRI line limit"));
        }
        let rest = line
            .strip_prefix(PREFIX)
            .and_then(|rest| rest.strip_prefix(' '))
            .ok_or(FrameError("no frame prefix"))?;
        let (tag, payload) = rest.split_once(' ').ok_or(FrameError("no frame payload"))?;
        if payload.is_empty() {
            return Err(FrameError("an empty frame payload"));
        }
        match tag {
            "o" | "e" => {
                let stream = if tag == "o" {
                    Stream::Stdout
                } else {
                    Stream::Stderr
                };
                let bytes = STANDARD
                    .decode(payload)
                    .map_err(|_| FrameError("output that is not padded standard base64"))?;
                if bytes.is_empty() || bytes.len() > MAX_CHUNK_BYTES {
                    return Err(FrameError("an output chunk outside the chunk bound"));
                }
                Ok(Self::Output { stream, bytes })
            }
            "c" => serde_json::from_str(payload)
                .map(Self::Control)
                .map_err(|_| FrameError("a control record outside the closed set")),
            _ => Err(FrameError("an unknown frame tag")),
        }
    }
}
