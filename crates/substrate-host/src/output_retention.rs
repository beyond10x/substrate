//! Which bytes of a bounded output stream a run keeps (design 29 §§ 5–6).
//!
//! Every stream keeps at most its bound `B`. [`Retention`] chooses which `B`: the first bytes, the
//! last bytes, or the first half of the bound and the last half. No bound changes here; this only
//! decides which bytes inside it are kept. One [`RetentionBuffer`] per stream serves every adapter
//! that reads its own pipes and the host driver's drain, so the arithmetic exists once.

use std::collections::VecDeque;
use std::sync::Arc;

use parking_lot::Mutex;
use tokio::io::{AsyncRead, AsyncReadExt as _};

use crate::confinement::RunOutput;

/// Which bytes of one stream are kept within its bound.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Retention {
    /// The first bytes, as every adapter keeps today.
    #[default]
    Head,
    /// The last bytes.
    Tail,
    /// The first half of the bound and the last half.
    HeadAndTail,
}

/// The retention of each stream, chosen separately.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OutputRetention {
    pub stdout: Retention,
    pub stderr: Retention,
}

impl OutputRetention {
    /// Whether both streams keep their head, the default.
    pub(crate) fn is_head(self) -> bool {
        self == Self::default()
    }
}

/// Which bytes of one stream `RunOutput` holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct KeptOutput {
    pub retention: Retention,
    /// Every byte the stream produced. Present only when the adapter read it to end of file.
    /// Absent, never guessed, under `pod` and after a read error (invariant 3).
    pub observed_bytes: Option<u64>,
    /// The stream's first bytes at the start of the kept bytes.
    pub head_bytes: u64,
    /// The stream's last bytes, directly after the head.
    pub tail_bytes: u64,
}

impl KeptOutput {
    /// A record of `head_bytes` first bytes whose total the adapter did not observe.
    pub(crate) fn unobserved_head(head_bytes: usize) -> Self {
        Self {
            retention: Retention::Head,
            observed_bytes: None,
            head_bytes: head_bytes as u64,
            tail_bytes: 0,
        }
    }
}

/// One stream's kept bytes under its retention and bound.
#[derive(Debug)]
pub(crate) struct RetentionBuffer {
    retention: Retention,
    head_cap: usize,
    tail_cap: usize,
    /// Empty until bytes arrive; grows to `head_cap`, never beyond, in length or allocation.
    head: Vec<u8>,
    /// Empty until bytes arrive; the front is dropped before bytes are added, so it never holds
    /// or allocates more than `tail_cap`.
    tail: VecDeque<u8>,
    /// Saturating.
    observed: u64,
    read_failed: bool,
    finished: bool,
}

impl RetentionBuffer {
    pub(crate) fn new(retention: Retention, bound: usize) -> Self {
        let (head_cap, tail_cap) = match retention {
            Retention::Head => (bound, 0),
            Retention::Tail => (0, bound),
            Retention::HeadAndTail => (bound / 2, bound - bound / 2),
        };
        Self {
            retention,
            head_cap,
            tail_cap,
            head: Vec::new(),
            tail: VecDeque::new(),
            observed: 0,
            read_failed: false,
            finished: false,
        }
    }

    /// Adds the next bytes the stream produced.
    pub(crate) fn push(&mut self, chunk: &[u8]) {
        self.observed = self.observed.saturating_add(chunk.len() as u64);
        let take = self
            .head_cap
            .saturating_sub(self.head.len())
            .min(chunk.len());
        self.head.reserve_exact(growth(
            self.head.len(),
            self.head.capacity(),
            take,
            self.head_cap,
        ));
        self.head.extend_from_slice(&chunk[..take]);
        let rest = &chunk[take..];
        if self.tail_cap == 0 || rest.is_empty() {
            return;
        }
        // Trim before extending, so the ring never holds more than its cap, even for a moment.
        let rest = &rest[rest.len().saturating_sub(self.tail_cap)..];
        let excess = (self.tail.len() + rest.len()).saturating_sub(self.tail_cap);
        self.tail.drain(..excess);
        self.tail.reserve_exact(growth(
            self.tail.len(),
            self.tail.capacity(),
            rest.len(),
            self.tail_cap,
        ));
        self.tail.extend(rest);
    }

    /// The stream ended at end of file: its total is known.
    pub(crate) fn finish(&mut self) {
        self.finished = true;
    }

    /// The stream could not be read to its end: its total is unknown.
    pub(crate) fn fail(&mut self) {
        self.read_failed = true;
    }

    /// Bytes were dropped, or the read failed.
    pub(crate) fn truncated(&self) -> bool {
        self.read_failed || self.observed > (self.head_cap + self.tail_cap) as u64
    }

    /// The kept bytes, head then tail, and what they are.
    pub(crate) fn snapshot(&self) -> (Vec<u8>, KeptOutput) {
        let mut kept = Vec::with_capacity(self.head.len() + self.tail.len());
        kept.extend_from_slice(&self.head);
        kept.extend(&self.tail);
        (kept, self.kept())
    }

    pub(crate) fn kept(&self) -> KeptOutput {
        KeptOutput {
            retention: self.retention,
            observed_bytes: (self.finished && !self.read_failed).then_some(self.observed),
            head_bytes: self.head.len() as u64,
            tail_bytes: self.tail.len() as u64,
        }
    }
}

/// How many bytes to reserve exactly so a buffer of `len` bytes in `capacity` takes `more`:
/// none when they fit, else enough to double the capacity, never past `cap`. So a quiet stream
/// allocates nothing, a growing one reallocates a logarithmic number of times, and no buffer is
/// ever allocated beyond its share of the bound. `len + more` never exceeds `cap` here.
fn growth(len: usize, capacity: usize, more: usize, cap: usize) -> usize {
    let needed = len + more;
    if needed <= capacity {
        return 0;
    }
    needed.max(capacity.saturating_mul(2)).min(cap) - len
}

/// Both streams of one run an adapter reads from its own pipes.
#[derive(Debug)]
pub(crate) struct Captured {
    stdout: RetentionBuffer,
    stderr: RetentionBuffer,
}

impl Captured {
    pub(crate) fn new(retention: OutputRetention, bound: usize) -> Self {
        Self {
            stdout: RetentionBuffer::new(retention.stdout, bound),
            stderr: RetentionBuffer::new(retention.stderr, bound),
        }
    }

    fn stream(&mut self, stdout: bool) -> &mut RetentionBuffer {
        if stdout {
            &mut self.stdout
        } else {
            &mut self.stderr
        }
    }

    /// What the run has produced so far, or in all once both streams ended.
    pub(crate) fn output(&self) -> RunOutput {
        let (stdout, stdout_kept) = self.stdout.snapshot();
        let (stderr, stderr_kept) = self.stderr.snapshot();
        RunOutput {
            stdout,
            stderr,
            stdout_truncated: self.stdout.truncated(),
            stderr_truncated: self.stderr.truncated(),
            stdout_kept,
            stderr_kept,
        }
    }
}

/// Reads one stream to its end into `captured`, keeping what its retention keeps.
pub(crate) async fn drain(
    mut stream: impl AsyncRead + Unpin,
    captured: Arc<Mutex<Captured>>,
    stdout: bool,
) {
    let mut buffer = vec![0_u8; 16 * 1024];
    loop {
        match stream.read(&mut buffer).await {
            Ok(0) => {
                captured.lock().stream(stdout).finish();
                return;
            }
            Ok(read) => captured.lock().stream(stdout).push(&buffer[..read]),
            Err(_) => {
                captured.lock().stream(stdout).fail();
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stream(len: usize) -> Vec<u8> {
        (0..=250_u8).cycle().take(len).collect()
    }

    fn kept(retention: Retention, bound: usize, chunks: &[&[u8]]) -> (Vec<u8>, KeptOutput, bool) {
        let mut buffer = RetentionBuffer::new(retention, bound);
        for chunk in chunks {
            buffer.push(chunk);
        }
        buffer.finish();
        let (bytes, kept) = buffer.snapshot();
        (bytes, kept, buffer.truncated())
    }

    /// B1: at or under the bound, every mode keeps the whole stream in order, nothing omitted.
    #[test]
    fn at_or_under_the_bound_every_mode_keeps_the_whole_stream() {
        for retention in [Retention::Head, Retention::Tail, Retention::HeadAndTail] {
            for len in [0, 1, 7, 64] {
                let whole = stream(len);
                let (bytes, record, truncated) = kept(retention, 64, &[&whole[..]]);
                assert_eq!(bytes, whole, "{retention:?} {len}");
                assert!(!truncated, "{retention:?} {len}");
                assert_eq!(record.retention, retention);
                assert_eq!(record.observed_bytes, Some(len as u64));
                assert_eq!(
                    record.head_bytes + record.tail_bytes,
                    len as u64,
                    "{retention:?} {len}: nothing omitted"
                );
            }
        }
    }

    /// B1: over the bound, `Head` keeps the first `B`, `Tail` the last `B`, and `HeadAndTail` the
    /// first `B / 2` and the last `B - B / 2`, whatever the chunking.
    #[test]
    fn over_the_bound_each_mode_keeps_its_bytes() {
        let bound = 65;
        let whole = stream(1000);
        let chunkings: [Vec<&[u8]>; 3] = [
            vec![&whole[..]],
            whole.chunks(1).collect(),
            whole.chunks(17).collect(),
        ];
        for chunks in &chunkings {
            let (bytes, record, truncated) = kept(Retention::Head, bound, chunks);
            assert_eq!(bytes, &whole[..bound]);
            assert!(truncated);
            assert_eq!((record.head_bytes, record.tail_bytes), (65, 0));
            assert_eq!(record.observed_bytes, Some(1000));

            let (bytes, record, truncated) = kept(Retention::Tail, bound, chunks);
            assert_eq!(bytes, &whole[1000 - bound..]);
            assert!(truncated);
            assert_eq!((record.head_bytes, record.tail_bytes), (0, 65));

            let (bytes, record, truncated) = kept(Retention::HeadAndTail, bound, chunks);
            let mut expected = whole[..32].to_vec();
            expected.extend_from_slice(&whole[1000 - 33..]);
            assert_eq!(bytes, expected);
            assert!(truncated);
            assert_eq!((record.head_bytes, record.tail_bytes), (32, 33));
            assert_eq!(
                record.observed_bytes.unwrap() - record.head_bytes - record.tail_bytes,
                1000 - 65
            );
        }
    }

    /// B1: a chunk larger than the ring replaces it with the chunk's last bytes; a smaller one
    /// after it pushes the oldest out.
    #[test]
    fn a_chunk_larger_than_the_ring_refills_it_and_a_smaller_one_rolls_it() {
        let mut buffer = RetentionBuffer::new(Retention::Tail, 4);
        buffer.push(b"abcdefghij");
        assert_eq!(buffer.snapshot().0, b"ghij");
        buffer.push(b"kl");
        assert_eq!(buffer.snapshot().0, b"ijkl");
        buffer.push(b"mnopqrstuvwxyz");
        assert_eq!(buffer.snapshot().0, b"wxyz");
        assert!(buffer.truncated());
    }

    /// B1: head and tail never exceed the bound, at every step.
    #[test]
    fn kept_bytes_never_exceed_the_bound() {
        for retention in [Retention::Head, Retention::Tail, Retention::HeadAndTail] {
            for bound in [1, 2, 3, 10] {
                let mut buffer = RetentionBuffer::new(retention, bound);
                for chunk in stream(200).chunks(7) {
                    buffer.push(chunk);
                    let (bytes, record) = buffer.snapshot();
                    assert!(bytes.len() <= bound, "{retention:?} {bound}");
                    assert_eq!(
                        usize::try_from(record.head_bytes + record.tail_bytes).expect("kept"),
                        bytes.len()
                    );
                }
            }
        }
    }

    /// A quiet stream costs nothing: the ring allocates nothing when the stream opens. It grows
    /// with what it keeps, trims before it extends, and never holds or allocates more than its cap,
    /// even for a moment, so a `Tail` stream stores its bound, not about twice it.
    #[test]
    fn the_ring_grows_lazily_and_never_past_its_cap() {
        for bound in [1, 4, 5, 64, 4096] {
            for retention in [Retention::Head, Retention::Tail, Retention::HeadAndTail] {
                let mut buffer = RetentionBuffer::new(retention, bound);
                let cap = buffer.tail_cap;
                assert_eq!(buffer.tail.capacity(), 0, "{retention:?} {bound}: at open");
                assert_eq!(buffer.head.capacity(), 0, "{retention:?} {bound}: at open");
                let whole = stream(bound * 7 + 3);
                let mut chunks: Vec<&[u8]> = whole[..bound * 2].chunks(1).collect();
                chunks.extend(whole[bound * 2..bound * 4].chunks(cap.saturating_sub(1).max(1)));
                chunks.push(&whole[bound * 4..=bound * 6]);
                chunks.extend(whole[bound * 6 + 1..].chunks(1));
                for chunk in chunks {
                    buffer.push(chunk);
                    assert!(
                        buffer.head.capacity() <= buffer.head_cap,
                        "{retention:?} {bound}: {} allocated for a head of {}",
                        buffer.head.capacity(),
                        buffer.head_cap
                    );
                    assert!(buffer.tail.len() <= cap, "{retention:?} {bound}");
                    assert!(
                        buffer.tail.capacity() <= cap,
                        "{retention:?} {bound}: {} allocated for a ring of {cap} after a {}-byte \
                         chunk",
                        buffer.tail.capacity(),
                        chunk.len()
                    );
                }
                assert_eq!(
                    buffer.snapshot().0[buffer.head.len()..],
                    whole[whole.len() - cap..],
                    "{retention:?} {bound}"
                );
            }
        }
    }

    /// B1: `B = 1` under `HeadAndTail` is a one-byte tail.
    #[test]
    fn a_one_byte_bound_under_head_and_tail_is_a_one_byte_tail() {
        let (bytes, record, truncated) = kept(Retention::HeadAndTail, 1, &[b"abc", b"de"]);
        assert_eq!(bytes, b"e");
        assert_eq!((record.head_bytes, record.tail_bytes), (0, 1));
        assert!(truncated);
    }

    /// B1: the total is known only once the stream was read to its end, and never after a failed
    /// read, which is also truncation.
    #[test]
    fn the_total_is_reported_only_after_end_of_file() {
        let mut buffer = RetentionBuffer::new(Retention::Tail, 8);
        buffer.push(b"abc");
        assert_eq!(buffer.kept().observed_bytes, None);
        assert!(!buffer.truncated());
        buffer.fail();
        buffer.finish();
        assert_eq!(buffer.kept().observed_bytes, None);
        assert!(buffer.truncated());
    }
}
