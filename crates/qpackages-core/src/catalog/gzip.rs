//! Reading gzip files: AppStream catalogs ship as `*.xml.gz`.
//!
//! The deflate stream inside is decoded by `miniz_oxide`, which forbids `unsafe`. The framing
//! around it (RFC 1952) is a short header and an eight-byte trailer, read here, and the trailer's
//! CRC-32 and length are checked so a file cut short or damaged is an error, never a catalog
//! that is quietly missing its end.

use std::fmt;

use miniz_oxide::inflate::TINFLStatus;
use miniz_oxide::inflate::core::{self as inflate, DecompressorOxide, inflate_flags};

/// The two bytes every gzip member starts with.
const MAGIC: [u8; 2] = [0x1f, 0x8b];
/// The only compression method gzip defines: deflate.
const DEFLATE: u8 = 8;
/// Header flags, RFC 1952 section 2.3.1.
const FLAG_HCRC: u8 = 0x02;
const FLAG_EXTRA: u8 = 0x04;
const FLAG_NAME: u8 = 0x08;
const FLAG_COMMENT: u8 = 0x10;
/// Flag bits the RFC reserves; a member with any of them set is not one this reader understands.
const FLAG_RESERVED: u8 = 0xe0;
/// Bytes of the fixed header and of the trailer.
const HEADER_LEN: usize = 10;
const TRAILER_LEN: usize = 8;
/// The least room the output starts with, and the least it grows by.
const CHUNK: usize = 256 * 1024;
/// The most the output is sized to up front, as a multiple of the compressed size. Catalogs
/// compress about four to one; deflate cannot pass about a thousand to one.
const MAX_RATIO: usize = 16;

/// Why a gzip file could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GzipError {
    /// The data does not start like a gzip file.
    NotGzip,
    /// The header asks for something gzip does not define: another method or reserved flags.
    Unsupported,
    /// The data ends before the member does.
    Truncated,
    /// The compressed data is damaged.
    Corrupt,
    /// The data decoded, but its checksum or length disagrees with the trailer.
    Checksum,
}

impl fmt::Display for GzipError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::NotGzip => "not a gzip file",
            Self::Unsupported => "a gzip header this reader does not understand",
            Self::Truncated => "the gzip data ends too early",
            Self::Corrupt => "the compressed data is damaged",
            Self::Checksum => "the decompressed data does not match its checksum",
        })
    }
}

impl std::error::Error for GzipError {}

/// Decompresses a whole gzip file. Several members one after another, which `gzip` itself
/// accepts, decode to their contents joined in order.
pub fn decompress(data: &[u8]) -> Result<Vec<u8>, GzipError> {
    let mut output = Vec::new();
    let mut rest = data;
    loop {
        rest = member(rest, &mut output)?;
        if rest.is_empty() {
            return Ok(output);
        }
    }
}

/// Decodes the member at the start of `data` onto the end of `output` and returns what follows it.
fn member<'a>(data: &'a [u8], output: &mut Vec<u8>) -> Result<&'a [u8], GzipError> {
    let body = &data[header_len(data)?..];
    let start = output.len();
    // The whole output is kept as the window, so back-references never need copying, and it
    // starts at the size the file's last trailer gives, capped so a damaged trailer cannot ask
    // for gigabytes: growing is only a fallback.
    let hinted = data.len().checked_sub(4).map_or(0, |at| {
        let tail = [data[at], data[at + 1], data[at + 2], data[at + 3]];
        usize::try_from(u32::from_le_bytes(tail)).unwrap_or(0)
    });
    output.resize(start + hinted.min(body.len().saturating_mul(MAX_RATIO)).max(CHUNK), 0);
    let mut decompressor = Box::<DecompressorOxide>::default();
    let mut consumed = 0;
    let mut written = start;
    loop {
        let flags = inflate_flags::TINFL_FLAG_USING_NON_WRAPPING_OUTPUT_BUF;
        let (status, used, produced) =
            inflate::decompress(&mut decompressor, &body[consumed..], output, written, flags);
        consumed += used;
        written += produced;
        match status {
            TINFLStatus::Done => break,
            TINFLStatus::HasMoreOutput => {
                let grown = output.len().saturating_mul(2).max(output.len() + CHUNK);
                output.resize(grown, 0);
            }
            TINFLStatus::NeedsMoreInput | TINFLStatus::FailedCannotMakeProgress => {
                output.truncate(start);
                return Err(GzipError::Truncated);
            }
            _ => {
                output.truncate(start);
                return Err(GzipError::Corrupt);
            }
        }
    }
    output.truncate(written);
    let after = &body[consumed..];
    let trailer = after.get(..TRAILER_LEN).ok_or(GzipError::Truncated)?;
    let expected_crc = u32::from_le_bytes([trailer[0], trailer[1], trailer[2], trailer[3]]);
    let expected_len = u32::from_le_bytes([trailer[4], trailer[5], trailer[6], trailer[7]]);
    let decoded = &output[start..];
    // The trailer keeps the length modulo 2^32, so compare it that way.
    let length_matches = (decoded.len() as u64 & u64::from(u32::MAX)) == u64::from(expected_len);
    if crc32(decoded) != expected_crc || !length_matches {
        return Err(GzipError::Checksum);
    }
    Ok(&after[TRAILER_LEN..])
}

/// The length of the member header at the start of `data`, optional fields included.
fn header_len(data: &[u8]) -> Result<usize, GzipError> {
    let fixed = data.get(..HEADER_LEN).ok_or(GzipError::NotGzip)?;
    if fixed[..2] != MAGIC {
        return Err(GzipError::NotGzip);
    }
    let flags = fixed[3];
    if fixed[2] != DEFLATE || flags & FLAG_RESERVED != 0 {
        return Err(GzipError::Unsupported);
    }
    let mut position = HEADER_LEN;
    if flags & FLAG_EXTRA != 0 {
        let size = data.get(position..position + 2).ok_or(GzipError::Truncated)?;
        position += 2 + usize::from(u16::from_le_bytes([size[0], size[1]]));
    }
    for flag in [FLAG_NAME, FLAG_COMMENT] {
        if flags & flag != 0 {
            let field = data.get(position..).ok_or(GzipError::Truncated)?;
            let end = field.iter().position(|&byte| byte == 0).ok_or(GzipError::Truncated)?;
            position += end + 1;
        }
    }
    if flags & FLAG_HCRC != 0 {
        position += 2;
    }
    if position > data.len() {
        return Err(GzipError::Truncated);
    }
    Ok(position)
}

/// The CRC-32 lookup table (the reflected polynomial `0xEDB88320`), built at compile time.
const CRC_TABLE: [u32; 256] = {
    let mut table = [0; 256];
    let mut index = 0;
    while index < 256 {
        let mut value = index as u32;
        let mut bit = 0;
        while bit < 8 {
            value = if value & 1 == 1 { 0xedb8_8320 ^ (value >> 1) } else { value >> 1 };
            bit += 1;
        }
        table[index] = value;
        index += 1;
    }
    table
};

/// The CRC-32 gzip's trailer carries.
fn crc32(data: &[u8]) -> u32 {
    !data.iter().fold(u32::MAX, |crc, &byte| CRC_TABLE[usize::from((crc as u8) ^ byte)] ^ (crc >> 8))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `printf 'hello\n' | gzip -n`: no name, no time stamp.
    const HELLO: [u8; 26] = [
        0x1f, 0x8b, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x03, 0xcb, 0x48, 0xcd, 0xc9, 0xc9, 0xe7, 0x02, 0x00,
        0x20, 0x30, 0x3a, 0x36, 0x06, 0x00, 0x00, 0x00,
    ];

    const APPSTREAM: &[u8] = include_bytes!("../../tests/fixtures/catalog/arch-extra.xml.gz");

    #[test]
    fn the_checksum_is_the_standard_crc32() {
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn a_small_file_decodes() {
        assert_eq!(decompress(&HELLO).expect("valid gzip"), b"hello\n");
    }

    #[test]
    fn the_recorded_catalog_decodes_to_xml() {
        let xml = decompress(APPSTREAM).expect("the fixture is valid gzip");
        assert!(xml.starts_with(b"<?xml"), "the catalog is XML");
        assert!(xml.ends_with(b"</components>\n"), "the whole file came out");
    }

    #[test]
    fn members_one_after_another_are_joined() {
        let twice = [HELLO, HELLO].concat();
        assert_eq!(decompress(&twice).expect("two members"), b"hello\nhello\n");
    }

    #[test]
    fn a_file_name_in_the_header_is_skipped() {
        // The same member with FNAME set and "a.txt\0" after the fixed header.
        let mut named = HELLO[..HEADER_LEN].to_vec();
        named[3] = FLAG_NAME;
        named.extend_from_slice(b"a.txt\0");
        named.extend_from_slice(&HELLO[HEADER_LEN..]);
        assert_eq!(decompress(&named).expect("a named member"), b"hello\n");
    }

    #[test]
    fn something_that_is_not_gzip_is_an_error() {
        assert_eq!(decompress(b"<?xml version"), Err(GzipError::NotGzip));
        assert_eq!(decompress(b""), Err(GzipError::NotGzip));
        let mut other_method = HELLO;
        other_method[2] = 7;
        assert_eq!(decompress(&other_method), Err(GzipError::Unsupported));
    }

    #[test]
    fn a_file_cut_short_is_an_error_at_every_length() {
        for length in HEADER_LEN..HELLO.len() {
            assert!(decompress(&HELLO[..length]).is_err(), "{length} bytes must not decode");
        }
        let cut = &APPSTREAM[..APPSTREAM.len() / 2];
        assert_eq!(decompress(cut), Err(GzipError::Truncated));
    }

    #[test]
    fn a_damaged_trailer_is_caught() {
        let mut damaged = HELLO;
        damaged[20] ^= 0xff;
        assert_eq!(decompress(&damaged), Err(GzipError::Checksum));
    }

    #[test]
    fn damaged_data_is_an_error_not_a_panic() {
        let mut damaged = HELLO;
        damaged[10] = 0xff;
        assert!(decompress(&damaged).is_err());
    }
}
