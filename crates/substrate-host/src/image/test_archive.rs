//! A tar writer for the unpacker's tests: ustar headers, pax records and GNU long names, built in
//! memory, so no test reads an archive from anywhere else.

use std::io::Write as _;

use sha2::{Digest as _, Sha256};

#[derive(Default)]
pub(super) struct Tar {
    bytes: Vec<u8>,
}

/// One header's fields.
pub(super) struct Header<'a> {
    pub name: &'a [u8],
    pub flag: u8,
    pub mode: u32,
    pub size: u64,
    pub link: &'a [u8],
    pub gnu: bool,
}

impl<'a> Header<'a> {
    pub(super) fn new(name: &'a [u8], flag: u8) -> Self {
        Self {
            name,
            flag,
            mode: 0o644,
            size: 0,
            link: b"",
            gnu: false,
        }
    }
}

impl Tar {
    pub(super) fn header(&mut self, header: &Header<'_>) -> &mut Self {
        let mut block = [0_u8; 512];
        let name_length = header.name.len().min(100);
        block[..name_length].copy_from_slice(&header.name[..name_length]);
        write_octal(&mut block[100..108], u64::from(header.mode));
        write_octal(&mut block[108..116], 0);
        write_octal(&mut block[116..124], 0);
        write_octal(&mut block[124..136], header.size);
        write_octal(&mut block[136..148], 0);
        block[156] = header.flag;
        let link_length = header.link.len().min(100);
        block[157..157 + link_length].copy_from_slice(&header.link[..link_length]);
        if header.gnu {
            block[257..265].copy_from_slice(b"ustar  \0");
        } else {
            block[257..263].copy_from_slice(b"ustar\0");
            block[263..265].copy_from_slice(b"00");
        }
        block[148..156].copy_from_slice(b"        ");
        let sum: u64 = block.iter().map(|&byte| u64::from(byte)).sum();
        write_octal(&mut block[148..155], sum);
        block[155] = b' ';
        self.bytes.extend_from_slice(&block);
        self
    }

    pub(super) fn data(&mut self, data: &[u8]) -> &mut Self {
        self.bytes.extend_from_slice(data);
        let padding = (512 - data.len() % 512) % 512;
        self.bytes.extend(std::iter::repeat_n(0, padding));
        self
    }

    pub(super) fn file(&mut self, name: &str, mode: u32, data: &[u8]) -> &mut Self {
        self.header(&Header {
            mode,
            size: data.len() as u64,
            ..Header::new(name.as_bytes(), b'0')
        })
        .data(data)
    }

    pub(super) fn directory(&mut self, name: &str, mode: u32) -> &mut Self {
        self.header(&Header {
            mode,
            ..Header::new(name.as_bytes(), b'5')
        })
    }

    pub(super) fn symlink(&mut self, name: &str, target: &str) -> &mut Self {
        self.header(&Header {
            mode: 0o777,
            link: target.as_bytes(),
            ..Header::new(name.as_bytes(), b'2')
        })
    }

    pub(super) fn hard_link(&mut self, name: &str, target: &str) -> &mut Self {
        self.header(&Header {
            link: target.as_bytes(),
            ..Header::new(name.as_bytes(), b'1')
        })
    }

    pub(super) fn special(&mut self, name: &str, flag: u8) -> &mut Self {
        self.header(&Header::new(name.as_bytes(), flag))
    }

    /// A pax extended header holding `records`, each `key=value`.
    pub(super) fn pax(&mut self, records: &[(&str, &[u8])]) -> &mut Self {
        let mut data = Vec::new();
        for (key, value) in records {
            let body = key.len() + value.len() + 3;
            let mut length = body + 1;
            while length.to_string().len() + body != length {
                length += 1;
            }
            write!(data, "{length} {key}=").expect("in memory");
            data.extend_from_slice(value);
            data.push(b'\n');
        }
        self.header(&Header {
            size: data.len() as u64,
            ..Header::new(b"././@PaxHeader", b'x')
        })
        .data(&data)
    }

    /// A GNU long-name (`L`) or long-link (`K`) entry holding `text`.
    pub(super) fn gnu_long(&mut self, flag: u8, text: &[u8]) -> &mut Self {
        let mut data = text.to_vec();
        data.push(0);
        self.header(&Header {
            size: data.len() as u64,
            gnu: true,
            ..Header::new(b"././@LongLink", flag)
        })
        .data(&data)
    }

    /// The archive's bytes with its two end blocks and a record's zero padding.
    pub(super) fn finish(&self) -> Vec<u8> {
        let mut bytes = self.bytes.clone();
        bytes.extend(std::iter::repeat_n(0, 1024));
        let padding = (10_240 - bytes.len() % 10_240) % 10_240;
        bytes.extend(std::iter::repeat_n(0, padding));
        bytes
    }

    /// [`Self::finish`], gzip-compressed.
    pub(super) fn gzip(&self) -> Vec<u8> {
        gzip(&self.finish())
    }
}

pub(super) fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    encoder.write_all(bytes).expect("in memory");
    encoder.finish().expect("in memory")
}

pub(super) fn digest(bytes: &[u8]) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(bytes)))
}

fn write_octal(field: &mut [u8], value: u64) {
    let width = field.len() - 1;
    let text = format!("{value:0width$o}");
    field[..width].copy_from_slice(text.as_bytes());
    field[width] = 0;
}
