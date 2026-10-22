//! PNG: the `eXIf` chunk and the XMP `iTXt` chunk (keyword `XML:com.adobe.xmp`). PNG has no standard place for a
//! Photoshop resource block, so IPTC is never read from or written to one.

use super::{Blocks, MAX_BLOCK_LEN, ToWrite, Wrote, be_u32, exif, xmp};

const SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
const XMP_KEYWORD: &[u8] = b"XML:com.adobe.xmp";
/// A chunk's length is a 31-bit number.
const MAX_CHUNK_LEN: usize = 0x7FFF_FFFF;

/// The chunks of a PNG, as `(type, data, offset of the chunk's length field, whether its CRC matches)`. Stops at `IEND`,
/// at the end of the bytes, or at the first chunk that overruns them. The CRC is only computed for the chunks a caller
/// asks about, so walking past a large image's `IDAT`s costs nothing.
fn chunks(bytes: &[u8]) -> impl Iterator<Item = ([u8; 4], &[u8], usize, u32)> {
    let mut at = if bytes.starts_with(SIGNATURE) {
        SIGNATURE.len()
    } else {
        bytes.len()
    };
    std::iter::from_fn(move || {
        let length = usize::try_from(be_u32(bytes, at)?)
            .ok()
            .filter(|&length| length <= MAX_CHUNK_LEN)?;
        let kind: [u8; 4] = bytes.get(at + 4..at + 8)?.try_into().ok()?;
        let end = at.checked_add(8)?.checked_add(length)?;
        let data = bytes.get(at + 8..end)?;
        let crc = be_u32(bytes, end)?;
        if &kind == b"IEND" {
            return None;
        }
        let start = at;
        at = end + 4;
        Some((kind, data, start, crc))
    })
}

/// The transfer characteristics code of a PNG's `cICP` chunk (PNG Third Edition), if it has one with a valid CRC.
pub(in crate::image) fn cicp_transfer(bytes: &[u8]) -> Option<u8> {
    chunks(bytes)
        .find(|(kind, ..)| kind == b"cICP")
        .filter(|(kind, data, _, crc)| data.len() == 4 && crc32(kind, data) == *crc)
        .map(|(_, data, ..)| data[1])
}

fn crc32(kind: &[u8; 4], data: &[u8]) -> u32 {
    let mut hasher = crc32fast::Hasher::new();
    hasher.update(kind);
    hasher.update(data);
    hasher.finalize()
}

/// Reads a PNG's EXIF and XMP.
pub(super) fn read(bytes: &[u8]) -> Blocks {
    let mut blocks = Blocks::default();
    let mut seen_exif = false;
    for (kind, data, _, crc) in chunks(bytes) {
        match &kind {
            b"eXIf" if !seen_exif => {
                seen_exif = true;
                blocks.exif = (crc32(&kind, data) == crc && exif::plausible(data)).then(|| data.to_vec());
            }
            b"iTXt" if blocks.xmp.is_none() && crc32(&kind, data) == crc => blocks.xmp = xmp_text(data),
            _ => {}
        }
    }
    blocks
}

/// The XMP packet in an `iTXt` chunk's `data`, if its keyword is the XMP one: the text after the keyword, the
/// compression flag and method, and the (empty, for XMP) language tag and translated keyword, inflated if compressed.
fn xmp_text(data: &[u8]) -> Option<Vec<u8>> {
    let rest = data.strip_prefix(XMP_KEYWORD)?.strip_prefix(b"\0")?;
    let (&compressed, rest) = rest.split_first()?;
    let (&method, rest) = rest.split_first()?;
    let language = rest.iter().position(|&byte| byte == 0)?;
    let rest = &rest[language + 1..];
    let translated = rest.iter().position(|&byte| byte == 0)?;
    let text = &rest[translated + 1..];

    let packet = match (compressed, method) {
        (0, _) => text.to_vec(),
        // zlib, inflated no further than the bound: the one place a small file could claim a huge block.
        (1, 0) => miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(text, MAX_BLOCK_LEN).ok()?,
        _ => return None,
    };
    xmp::plausible(&packet).then_some(packet)
}

/// `encoded` with `eXIf` and the XMP `iTXt` (uncompressed, as the XMP specification recommends) inserted in front of its
/// first `IDAT`. IPTC is never written: PNG has no place for it.
pub(super) fn splice(encoded: &[u8], blocks: ToWrite<'_>) -> Option<(Vec<u8>, Wrote)> {
    let (_, _, insert_at, _) = chunks(encoded).find(|(kind, ..)| kind == b"IDAT")?;

    let mut inserted = Vec::new();
    let mut wrote = Wrote::default();
    if let Some(block) = blocks.exif.filter(|block| block.len() <= MAX_CHUNK_LEN) {
        push_chunk(&mut inserted, *b"eXIf", &[block]);
        wrote.exif = true;
    }
    if let Some(packet) = blocks
        .xmp
        .filter(|packet| packet.len() <= MAX_CHUNK_LEN - XMP_KEYWORD.len() - 5)
    {
        // Keyword, its terminator, uncompressed, method 0, an empty language tag and an empty translated keyword.
        push_chunk(&mut inserted, *b"iTXt", &[XMP_KEYWORD, b"\0\0\0\0\0", packet]);
        wrote.xmp = true;
    }

    let mut out = Vec::with_capacity(encoded.len() + inserted.len());
    out.extend_from_slice(&encoded[..insert_at]);
    out.extend_from_slice(&inserted);
    out.extend_from_slice(&encoded[insert_at..]);
    Some((out, wrote))
}

/// Appends one chunk whose data is `parts` joined, with its CRC. The caller keeps the data within [`MAX_CHUNK_LEN`].
fn push_chunk(out: &mut Vec<u8>, kind: [u8; 4], parts: &[&[u8]]) {
    let length: usize = parts.iter().map(|part| part.len()).sum();
    let length = u32::try_from(length).expect("chunk data is kept within a chunk's length");
    let mut hasher = crc32fast::Hasher::new();
    hasher.update(&kind);
    out.extend_from_slice(&length.to_be_bytes());
    out.extend_from_slice(&kind);
    for part in parts {
        hasher.update(part);
        out.extend_from_slice(part);
    }
    out.extend_from_slice(&hasher.finalize().to_be_bytes());
}
