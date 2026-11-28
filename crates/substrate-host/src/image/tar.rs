//! A streaming tar reader: ustar, pax (`path`, `linkpath`, `size`) and GNU long names and long
//! links (design 38 § 3.3). It reads headers and data from any [`Read`] and writes nothing; the
//! unpack decides what each entry becomes.
//!
//! Every byte it reads counts against the inflated bound, so a small archive that inflates past
//! it stops at it. The archive ends with two zero blocks; what follows them must be zeros until
//! the stream ends.

use std::io::{ErrorKind, Read};

use super::{
    EntryRefusal, ImageBound, ImageError, MAX_IMAGE_LINK_TARGET_BYTES, MAX_IMAGE_PATH_BYTES,
    MAX_IMAGE_PAX_HEADER_BYTES, RefusedKind,
};

const BLOCK: usize = 512;
const BLOCK_BYTES: u64 = 512;

/// What an entry is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    Regular,
    HardLink,
    Symlink,
    Directory,
    Refused(RefusedKind),
}

/// One entry, with every extension applied. Its data is read with
/// [`TarReader::read_data`] before the next [`TarReader::next_entry`].
#[derive(Debug)]
pub(super) struct Entry {
    pub path: Vec<u8>,
    pub link: Vec<u8>,
    pub kind: Kind,
    pub mode: u32,
}

/// The records of a pax extended header the reader honours.
#[derive(Debug, Default)]
struct Pax {
    path: Option<Vec<u8>>,
    linkpath: Option<Vec<u8>>,
    size: Option<u64>,
    sparse: bool,
}

pub(super) struct TarReader<R> {
    inner: R,
    inflated: u64,
    max_inflated: u64,
    remaining: u64,
    padding: u64,
    ended: bool,
}

impl<R: Read> TarReader<R> {
    pub(super) fn new(inner: R, max_inflated: u64) -> Self {
        Self {
            inner,
            inflated: 0,
            max_inflated,
            remaining: 0,
            padding: 0,
            ended: false,
        }
    }

    /// The bytes read from the inflated stream so far.
    pub(super) fn inflated(&self) -> u64 {
        self.inflated
    }

    /// The next entry, or `None` after the end-of-archive blocks and the zeros after them.
    pub(super) fn next_entry(&mut self) -> Result<Option<Entry>, ImageError> {
        if self.ended {
            return Ok(None);
        }
        self.skip_data()?;
        let mut pax: Option<Pax> = None;
        let mut long_name: Option<Vec<u8>> = None;
        let mut long_link: Option<Vec<u8>> = None;
        loop {
            let mut block = [0_u8; BLOCK];
            self.read_exact(&mut block)?;
            if block.iter().all(|&byte| byte == 0) {
                if pax.is_some() || long_name.is_some() || long_link.is_some() {
                    return Err(ImageError::invalid(
                        "an extension header is followed by the end of the archive",
                    ));
                }
                self.read_exact(&mut block)?;
                if block.iter().any(|&byte| byte != 0) {
                    return Err(ImageError::invalid(
                        "a zero block is followed by a header, not by a second zero block",
                    ));
                }
                self.drain_zeros()?;
                self.ended = true;
                return Ok(None);
            }
            let header = parse_header(&block)?;
            match header.flag {
                b'x' => {
                    let data = self.read_extension(header.size)?;
                    pax = Some(parse_pax(&data, pax.take().unwrap_or_default())?);
                }
                b'g' => {
                    // A global header's records are ignored (design 38 § 3.3).
                    self.read_extension(header.size)?;
                }
                b'L' => {
                    long_name = Some(self.read_long(
                        header.size,
                        &header.name,
                        EntryRefusal::PathTooLong,
                        MAX_IMAGE_PATH_BYTES,
                    )?);
                }
                b'K' => {
                    long_link = Some(self.read_long(
                        header.size,
                        &header.name,
                        EntryRefusal::LinkTargetTooLong,
                        MAX_IMAGE_LINK_TARGET_BYTES,
                    )?);
                }
                flag => {
                    let pax = pax.unwrap_or_default();
                    let mut kind = kind_of(flag);
                    if pax.sparse {
                        kind = Kind::Refused(RefusedKind::Sparse);
                    }
                    let size = pax.size.unwrap_or(header.size);
                    // Data the stream could not hold within the inflated bound is refused
                    // before a byte of it is read, so no size reaches the arithmetic below
                    // that it could overflow.
                    if size > self.max_inflated {
                        return Err(ImageError::ArchiveLimit {
                            bound: ImageBound::UnpackedBytes,
                            limit: self.max_inflated,
                        });
                    }
                    self.remaining = size;
                    self.padding = padding(size);
                    return Ok(Some(Entry {
                        path: pax.path.or(long_name).unwrap_or(header.name),
                        link: pax.linkpath.or(long_link).unwrap_or(header.link),
                        kind,
                        mode: header.mode,
                    }));
                }
            }
        }
    }

    /// Reads at most `buffer.len()` bytes of the current entry's data; `0` once it is read.
    pub(super) fn read_data(&mut self, buffer: &mut [u8]) -> Result<usize, ImageError> {
        let wanted = usize::try_from(self.remaining)
            .unwrap_or(usize::MAX)
            .min(buffer.len());
        if wanted == 0 {
            return Ok(0);
        }
        self.read_exact(&mut buffer[..wanted])?;
        self.remaining -= wanted as u64;
        Ok(wanted)
    }

    /// The stream, after [`Self::next_entry`] answered `None`.
    pub(super) fn into_inner(self) -> R {
        self.inner
    }

    fn skip_data(&mut self) -> Result<(), ImageError> {
        let mut scratch = [0_u8; 4 * BLOCK];
        let mut left = self
            .remaining
            .checked_add(self.padding)
            .ok_or_else(|| ImageError::invalid("an entry's size overflows"))?;
        while left > 0 {
            let step = usize::try_from(left)
                .unwrap_or(usize::MAX)
                .min(scratch.len());
            self.read_exact(&mut scratch[..step])?;
            left -= step as u64;
        }
        self.remaining = 0;
        self.padding = 0;
        Ok(())
    }

    fn read_extension(&mut self, size: u64) -> Result<Vec<u8>, ImageError> {
        if size > MAX_IMAGE_PAX_HEADER_BYTES {
            return Err(ImageError::invalid(format!(
                "a pax header is longer than MAX_IMAGE_PAX_HEADER_BYTES ({MAX_IMAGE_PAX_HEADER_BYTES})"
            )));
        }
        self.read_extension_data(size)
    }

    fn read_long(
        &mut self,
        size: u64,
        header_name: &[u8],
        refusal: EntryRefusal,
        bound: usize,
    ) -> Result<Vec<u8>, ImageError> {
        // The text and its terminating NUL.
        if size > bound as u64 + 1 {
            return Err(ImageError::refused(header_name, refusal));
        }
        let mut data = self.read_extension_data(size)?;
        while data.last() == Some(&0) {
            data.pop();
        }
        Ok(data)
    }

    fn read_extension_data(&mut self, size: u64) -> Result<Vec<u8>, ImageError> {
        let length = usize::try_from(size).map_err(|_| ImageError::invalid("extension size"))?;
        let mut data = vec![0_u8; length];
        self.read_exact(&mut data)?;
        let mut pad = [0_u8; BLOCK];
        let pad_length = usize::try_from(padding(size)).expect("padding is under one block");
        self.read_exact(&mut pad[..pad_length])?;
        Ok(data)
    }

    fn drain_zeros(&mut self) -> Result<(), ImageError> {
        let mut scratch = [0_u8; 16 * BLOCK];
        loop {
            let read = self.read_some(&mut scratch)?;
            if read == 0 {
                return Ok(());
            }
            if scratch[..read].iter().any(|&byte| byte != 0) {
                return Err(ImageError::invalid(
                    "bytes other than zeros follow the end of the archive",
                ));
            }
        }
    }

    fn read_exact(&mut self, buffer: &mut [u8]) -> Result<(), ImageError> {
        let mut filled = 0;
        while filled < buffer.len() {
            let read = self.read_some(&mut buffer[filled..])?;
            if read == 0 {
                return Err(ImageError::invalid(
                    "the archive ends inside a header or an entry's data",
                ));
            }
            filled += read;
        }
        Ok(())
    }

    fn read_some(&mut self, buffer: &mut [u8]) -> Result<usize, ImageError> {
        let read = loop {
            match self.inner.read(buffer) {
                Ok(read) => break read,
                Err(error) if error.kind() == ErrorKind::Interrupted => {}
                Err(error) => return Err(stream_error(error)),
            }
        };
        self.inflated = self.inflated.saturating_add(read as u64);
        if self.inflated > self.max_inflated {
            return Err(ImageError::ArchiveLimit {
                bound: ImageBound::UnpackedBytes,
                limit: self.max_inflated,
            });
        }
        Ok(read)
    }
}

/// A failed read of the inflated stream: a corrupt or truncated gzip stream is the archive's
/// fault, anything else the store's.
pub(super) fn stream_error(error: std::io::Error) -> ImageError {
    match error.kind() {
        ErrorKind::InvalidData | ErrorKind::InvalidInput | ErrorKind::UnexpectedEof => {
            ImageError::invalid(format!("the gzip stream is invalid: {error}"))
        }
        _ => ImageError::io("reading the archive", error),
    }
}

struct Header {
    name: Vec<u8>,
    link: Vec<u8>,
    mode: u32,
    size: u64,
    flag: u8,
}

fn parse_header(block: &[u8; BLOCK]) -> Result<Header, ImageError> {
    let stored = parse_number(&block[148..156])?;
    let unsigned: u64 = block
        .iter()
        .enumerate()
        .map(|(at, &byte)| {
            if (148..156).contains(&at) {
                32
            } else {
                u64::from(byte)
            }
        })
        .sum();
    let signed: i64 = block
        .iter()
        .enumerate()
        .map(|(at, &byte)| {
            if (148..156).contains(&at) {
                32
            } else {
                i64::from(byte.cast_signed())
            }
        })
        .sum();
    if stored != unsigned && i64::try_from(stored).ok() != Some(signed) {
        return Err(ImageError::invalid("a header checksum does not match"));
    }
    let posix = &block[257..263] == b"ustar\0";
    let gnu = &block[257..265] == b"ustar  \0";
    if !posix && !gnu {
        return Err(ImageError::invalid("a header is neither ustar nor GNU"));
    }
    let mut name = field_text(&block[0..100]).to_vec();
    if posix {
        let prefix = field_text(&block[345..500]);
        if !prefix.is_empty() {
            let mut joined = prefix.to_vec();
            joined.push(b'/');
            joined.extend_from_slice(&name);
            name = joined;
        }
    }
    let mode = u32::try_from(parse_number(&block[100..108])? & 0o7777).expect("masked to 12 bits");
    Ok(Header {
        name,
        link: field_text(&block[157..257]).to_vec(),
        mode,
        size: parse_number(&block[124..136])?,
        flag: block[156],
    })
}

fn kind_of(flag: u8) -> Kind {
    match flag {
        b'0' | 0 | b'7' => Kind::Regular,
        b'1' => Kind::HardLink,
        b'2' => Kind::Symlink,
        b'3' => Kind::Refused(RefusedKind::CharacterDevice),
        b'4' => Kind::Refused(RefusedKind::BlockDevice),
        b'5' => Kind::Directory,
        b'6' => Kind::Refused(RefusedKind::Fifo),
        b'S' => Kind::Refused(RefusedKind::Sparse),
        other => Kind::Refused(RefusedKind::Other(other)),
    }
}

/// A header text field up to its first NUL.
fn field_text(field: &[u8]) -> &[u8] {
    let end = field
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(field.len());
    &field[..end]
}

fn padding(size: u64) -> u64 {
    (BLOCK_BYTES - size % BLOCK_BYTES) % BLOCK_BYTES
}

/// A numeric header field: octal digits between spaces and NULs, or GNU's base-256 form.
fn parse_number(field: &[u8]) -> Result<u64, ImageError> {
    let invalid = || ImageError::invalid("a header holds a malformed number");
    if field.first().is_some_and(|&byte| byte & 0x80 != 0) {
        if field[0] == 0xff {
            return Err(invalid());
        }
        let mut value = u64::from(field[0] & 0x7f);
        for &byte in &field[1..] {
            value = value
                .checked_mul(256)
                .and_then(|value| value.checked_add(u64::from(byte)))
                .ok_or_else(invalid)?;
        }
        return Ok(value);
    }
    let mut digits = field.iter().skip_while(|&&byte| byte == b' ').peekable();
    let mut value = 0_u64;
    while let Some(&&byte) = digits.peek() {
        if !(b'0'..=b'7').contains(&byte) {
            break;
        }
        value = value
            .checked_mul(8)
            .and_then(|value| value.checked_add(u64::from(byte - b'0')))
            .ok_or_else(invalid)?;
        digits.next();
    }
    if digits.all(|&byte| byte == b' ' || byte == 0) {
        Ok(value)
    } else {
        Err(invalid())
    }
}

/// The records of one pax extended header, over those of an earlier one.
fn parse_pax(data: &[u8], mut pax: Pax) -> Result<Pax, ImageError> {
    let invalid = || ImageError::invalid("a pax header holds a malformed record");
    let mut rest = data;
    while !rest.is_empty() {
        let space = rest
            .iter()
            .position(|&byte| byte == b' ')
            .ok_or_else(invalid)?;
        let length: usize = std::str::from_utf8(&rest[..space])
            .ok()
            .filter(|digits| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
            .and_then(|digits| digits.parse().ok())
            .ok_or_else(invalid)?;
        if length <= space + 1 || length > rest.len() || rest[length - 1] != b'\n' {
            return Err(invalid());
        }
        let record = &rest[space + 1..length - 1];
        let equals = record
            .iter()
            .position(|&byte| byte == b'=')
            .ok_or_else(invalid)?;
        let (key, value) = (&record[..equals], &record[equals + 1..]);
        let text = |value: &[u8]| (!value.is_empty()).then(|| value.to_vec());
        match key {
            b"path" => pax.path = text(value),
            b"linkpath" => pax.linkpath = text(value),
            b"size" => {
                pax.size = Some(
                    std::str::from_utf8(value)
                        .ok()
                        .filter(|digits| digits.bytes().all(|b| b.is_ascii_digit()))
                        .and_then(|digits| digits.parse().ok())
                        .ok_or_else(invalid)?,
                );
            }
            key if key.starts_with(b"GNU.sparse.") => pax.sparse = true,
            _ => {}
        }
        rest = &rest[length..];
    }
    Ok(pax)
}

#[cfg(test)]
mod tests {
    use super::{Pax, parse_number, parse_pax};

    #[test]
    fn a_number_is_octal_between_spaces_and_nuls_or_base_256() {
        assert_eq!(parse_number(b"0000644\0").expect("octal"), 0o644);
        assert_eq!(parse_number(b"  755 \0\0").expect("spaced octal"), 0o755);
        assert_eq!(parse_number(b"\0\0\0\0\0\0\0\0").expect("empty"), 0);
        assert_eq!(
            parse_number(&[0x80, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0]).expect("base 256"),
            65_536
        );
        assert!(parse_number(b"00008\0\0\0").is_err());
        assert!(parse_number(b"0644 x\0\0").is_err());
        assert!(parse_number(&[0xff; 12]).is_err());
    }

    #[test]
    fn a_pax_header_sets_path_linkpath_size_and_marks_a_sparse_file() {
        let pax = parse_pax(
            b"12 path=a/b\n17 linkpath=/abs\n11 size=42\n22 GNU.sparse.major=1\n",
            Pax::default(),
        )
        .expect("pax");
        assert_eq!(pax.path.as_deref(), Some(&b"a/b"[..]));
        assert_eq!(pax.linkpath.as_deref(), Some(&b"/abs"[..]));
        assert_eq!(pax.size, Some(42));
        assert!(pax.sparse);
        assert!(parse_pax(b"99 path=a\n", Pax::default()).is_err());
        assert!(parse_pax(b"8 patha\n", Pax::default()).is_err());
        assert!(parse_pax(b"11 size=4x\n", Pax::default()).is_err());
    }
}
