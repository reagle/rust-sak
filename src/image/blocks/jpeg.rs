//! JPEG: the `APP1` EXIF and XMP segments (extended XMP included) and the `APP13` Photoshop segments.

use super::{Blocks, MAX_BLOCK_LEN, ToWrite, Wrote, be_u16, be_u32, exif, xmp};

const SOI: u8 = 0xD8;
const EOI: u8 = 0xD9;
const SOS: u8 = 0xDA;
const APP0: u8 = 0xE0;
const APP1: u8 = 0xE1;
const APP2: u8 = 0xE2;
const APP13: u8 = 0xED;

const EXIF_HEADER: &[u8] = b"Exif\0\0";
const XMP_HEADER: &[u8] = b"http://ns.adobe.com/xap/1.0/\0";
const EXTENDED_HEADER: &[u8] = b"http://ns.adobe.com/xmp/extension/\0";
const PHOTOSHOP_HEADER: &[u8] = b"Photoshop 3.0\0";
/// The ISO 21496-1 gain-map version marker's `APP2` header.
const ISO_GAIN_MAP_HEADER: &[u8] = b"urn:iso:std:iso:ts:21496:-1\0";
/// The CIPA DC-007 Multi-Picture Format `APP2` header.
const MPF_HEADER: &[u8] = b"MPF\0";
/// The namespace of Adobe's and Android's (Ultra HDR) gain-map XMP description, `hdrgm:`.
const HDRGM_NAMESPACE: &[u8] = b"http://ns.adobe.com/hdr-gain-map/1.0/";
/// MPF's `MPEntry` tag, whose value is one 16-byte entry per image.
const MP_ENTRY: u16 = 0xB002;
/// The MP Entry image type code of a gain map (CIPA DC-007, the low 24 bits of an entry's attribute).
const MP_TYPE_GAIN_MAP: u32 = 0x05_0000;

/// The most data one segment holds: its length field counts itself, and is 16 bits.
const MAX_SEGMENT_DATA: usize = 65_535 - 2;
/// The largest EXIF block one `APP1` holds once its `Exif\0\0` header is in.
pub(super) const MAX_EXIF_LEN: usize = MAX_SEGMENT_DATA - EXIF_HEADER.len();
/// The most of an extended XMP packet one chunk carries: what is left after the header, the GUID and the two offsets.
const EXTENDED_CHUNK_LEN: usize = MAX_SEGMENT_DATA - EXTENDED_HEADER.len() - 32 - 4 - 4;
/// The most of a Photoshop resource block one `APP13` carries.
const PHOTOSHOP_CHUNK_LEN: usize = MAX_SEGMENT_DATA - PHOTOSHOP_HEADER.len();

/// The marker segments ahead of the first scan, as `(marker, data, offset of the marker's 0xFF)`. Stops at `SOS`,
/// `EOI`, or the first thing that is not a well-formed segment, so a truncated file yields the segments before the cut.
fn segments(bytes: &[u8]) -> impl Iterator<Item = (u8, &[u8], usize)> {
    let mut at = if bytes.starts_with(&[0xFF, SOI]) {
        2
    } else {
        bytes.len()
    };
    std::iter::from_fn(move || {
        loop {
            if *bytes.get(at)? != 0xFF {
                return None;
            }
            let start = at;
            // Any number of 0xFF fill bytes may precede a marker.
            while *bytes.get(at + 1)? == 0xFF {
                at += 1;
            }
            let marker = bytes[at + 1];
            at += 2;
            match marker {
                SOS | EOI => return None,
                // Markers with no length and no data.
                0x01 | 0xD0..=0xD7 => continue,
                _ => {}
            }
            let length = usize::from(be_u16(bytes, at)?);
            let data = bytes.get(at + 2..at.checked_add(length)?).filter(|_| length >= 2)?;
            at += length;
            return Some((marker, data, start));
        }
    })
}

/// Whether the JPEG in `bytes` declares a gain map in the segments ahead of its first scan: an ISO 21496-1 version
/// marker, an MPF entry of type Gain Map, or an XMP packet declaring the `hdrgm` namespace. Nothing after the primary
/// image's first `SOS` is read, so the gain map itself, and a malformed file, cost nothing.
pub(in crate::image) fn has_gain_map(bytes: &[u8]) -> bool {
    segments(bytes).any(|(marker, data, _)| match marker {
        APP2 => data.starts_with(ISO_GAIN_MAP_HEADER) || mpf_has_gain_map(data),
        APP1 => data.starts_with(XMP_HEADER) && xmp::contains(&data[XMP_HEADER.len()..], HDRGM_NAMESPACE),
        _ => false,
    })
}

/// Whether the `APP2` segment `data` is an MPF index with an entry of type Gain Map. Every read is bounded by the
/// segment: the entries are read from the bytes present, never from a count the file claims.
fn mpf_has_gain_map(data: &[u8]) -> bool {
    let Some(tiff) = data.strip_prefix(MPF_HEADER) else {
        return false;
    };
    let big = match tiff.get(0..4) {
        Some(b"MM\0\x2a") => true,
        Some(b"II\x2a\0") => false,
        _ => return false,
    };
    let u16_at = |at: usize| {
        let pair: [u8; 2] = tiff.get(at..at.checked_add(2)?)?.try_into().ok()?;
        Some(if big {
            u16::from_be_bytes(pair)
        } else {
            u16::from_le_bytes(pair)
        })
    };
    let u32_at = |at: usize| {
        let quad: [u8; 4] = tiff.get(at..at.checked_add(4)?)?.try_into().ok()?;
        Some(if big {
            u32::from_be_bytes(quad)
        } else {
            u32::from_le_bytes(quad)
        })
    };

    let found = (|| {
        let ifd = usize::try_from(u32_at(4)?).ok()?;
        let count = usize::from(u16_at(ifd)?);
        let entry = (0..count)
            .map_while(|i| ifd.checked_add(2)?.checked_add(i.checked_mul(12)?))
            .take_while(|&at| at.checked_add(12).is_some_and(|end| end <= tiff.len()))
            .find(|&at| u16_at(at) == Some(MP_ENTRY))?;
        let length = usize::try_from(u32_at(entry + 4)?).ok()?;
        let offset = usize::try_from(u32_at(entry + 8)?).ok()?;
        let table = tiff.get(offset..offset.checked_add(length)?)?;
        Some(
            table
                .as_chunks::<16>()
                .0
                .iter()
                .any(|image| attribute(image, big) & 0x00FF_FFFF == MP_TYPE_GAIN_MAP),
        )
    })();
    found.unwrap_or(false)
}

/// The first four bytes of `image`, an MP Entry, as its attribute in the index's byte order.
fn attribute(image: &[u8; 16], big: bool) -> u32 {
    let quad = [image[0], image[1], image[2], image[3]];
    if big {
        u32::from_be_bytes(quad)
    } else {
        u32::from_le_bytes(quad)
    }
}

/// Reads a JPEG's EXIF, XMP and IPTC.
pub(super) fn read(bytes: &[u8]) -> Blocks {
    let mut blocks = Blocks::default();
    let mut standard: Option<&[u8]> = None;
    // Extended XMP chunks by GUID: (full length, offset, data).
    let mut extended: Vec<(&[u8], u32, u32, &[u8])> = Vec::new();
    let mut photoshop: Option<Vec<u8>> = None;

    for (marker, data, _) in segments(bytes) {
        match marker {
            APP1 if data.starts_with(EXIF_HEADER) => {
                if blocks.exif.is_none() {
                    let block = &data[EXIF_HEADER.len()..];
                    blocks.exif = exif::plausible(block).then(|| block.to_vec());
                }
            }
            APP1 if data.starts_with(XMP_HEADER) => {
                standard.get_or_insert(&data[XMP_HEADER.len()..]);
            }
            APP1 if data.starts_with(EXTENDED_HEADER) => {
                let chunk = &data[EXTENDED_HEADER.len()..];
                if let (Some(guid), Some(full), Some(offset)) = (chunk.get(..32), be_u32(chunk, 32), be_u32(chunk, 36))
                {
                    extended.push((guid, full, offset, &chunk[40..]));
                }
            }
            // A resource block too large for one segment is continued in the next; they are joined in order.
            APP13 if data.starts_with(PHOTOSHOP_HEADER) => {
                let part = &data[PHOTOSHOP_HEADER.len()..];
                let joined = photoshop.get_or_insert_with(Vec::new);
                if joined.len() + part.len() <= MAX_BLOCK_LEN {
                    joined.extend_from_slice(part);
                }
            }
            _ => {}
        }
    }

    blocks.iptc = photoshop.filter(|block| block.starts_with(b"8BIM"));
    blocks.xmp = standard
        .filter(|packet| xmp::plausible(packet))
        .map(|packet| with_extended(packet, &extended));
    blocks
}

/// The standard packet `standard` with its extended XMP joined in, if it names a GUID whose chunks are all present and
/// cover the declared length exactly; otherwise `standard` as it is.
fn with_extended(standard: &[u8], chunks: &[(&[u8], u32, u32, &[u8])]) -> Vec<u8> {
    // Read once from the standard packet, then matched against each chunk: searching the packet per chunk would let a
    // file of many tiny chunks make the read quadratic.
    let Some(guid) = xmp::extended_guid(standard).filter(|_| !chunks.is_empty()) else {
        return standard.to_vec();
    };

    let reassembled = (|| {
        let mut full = None;
        let mut parts: Vec<(usize, &[u8])> = Vec::new();
        for (chunk_guid, length, offset, data) in chunks {
            if *chunk_guid != guid.as_bytes() {
                continue;
            }
            // Every chunk must agree on the whole length.
            if *full.get_or_insert(*length) != *length {
                return None;
            }
            parts.push((usize::try_from(*offset).ok()?, data));
        }
        let full = usize::try_from(full?).ok().filter(|&full| full <= MAX_BLOCK_LEN)?;
        parts.sort_by_key(|(offset, _)| *offset);
        parts.dedup_by_key(|(offset, _)| *offset);

        // The chunks must tile the packet with no gap, overlap or overrun.
        let mut packet = Vec::with_capacity(full);
        for (offset, data) in parts {
            if offset != packet.len() || offset + data.len() > full {
                return None;
            }
            packet.extend_from_slice(data);
        }
        (packet.len() == full).then_some(packet)
    })();

    reassembled
        .and_then(|extended| xmp::join(standard, guid, &extended))
        .unwrap_or_else(|| standard.to_vec())
}

/// `encoded` with `blocks` inserted after its `APP0` (or straight after `SOI` if it has none): `APP1` EXIF, then `APP1`
/// XMP and any extended XMP, then `APP13`.
pub(super) fn splice(encoded: &[u8], blocks: ToWrite<'_>) -> Option<(Vec<u8>, Wrote)> {
    let first = segments(encoded).next();
    // The `JFIF` (or `JFXX`) segment, which must stay first.
    let insert_at = match first {
        Some((APP0, data, start)) => start + 4 + data.len(),
        _ if encoded.starts_with(&[0xFF, SOI]) => 2,
        _ => return None,
    };

    let mut inserted = Vec::new();
    let mut wrote = Wrote::default();

    if let Some(block) = blocks.exif.filter(|block| block.len() <= MAX_EXIF_LEN) {
        push_segment(&mut inserted, APP1, &[EXIF_HEADER, block]);
        wrote.exif = true;
    }
    if let Some(packet) = blocks.xmp {
        if packet.len() <= xmp::MAX_STANDARD_LEN {
            push_segment(&mut inserted, APP1, &[XMP_HEADER, packet]);
        } else {
            let (standard, guid, extended) = xmp::split(packet);
            push_segment(&mut inserted, APP1, &[XMP_HEADER, &standard]);
            let full = u32::try_from(extended.len()).ok()?.to_be_bytes();
            for (index, chunk) in extended.chunks(EXTENDED_CHUNK_LEN).enumerate() {
                let offset = u32::try_from(index * EXTENDED_CHUNK_LEN).ok()?.to_be_bytes();
                push_segment(
                    &mut inserted,
                    APP1,
                    &[EXTENDED_HEADER, guid.as_bytes(), &full, &offset, chunk],
                );
            }
        }
        wrote.xmp = true;
    }
    if let Some(block) = blocks.iptc {
        for chunk in block.chunks(PHOTOSHOP_CHUNK_LEN) {
            push_segment(&mut inserted, APP13, &[PHOTOSHOP_HEADER, chunk]);
        }
        wrote.iptc = true;
    }

    let mut out = Vec::with_capacity(encoded.len() + inserted.len());
    out.extend_from_slice(&encoded[..insert_at]);
    out.extend_from_slice(&inserted);
    out.extend_from_slice(&encoded[insert_at..]);
    Some((out, wrote))
}

/// Appends one marker segment whose data is `parts` joined. The caller keeps the data within [`MAX_SEGMENT_DATA`].
fn push_segment(out: &mut Vec<u8>, marker: u8, parts: &[&[u8]]) {
    let length: usize = parts.iter().map(|part| part.len()).sum();
    debug_assert!(length <= MAX_SEGMENT_DATA, "a segment of {length} bytes");
    let length = u16::try_from(length + 2).expect("segment data is kept within a segment's length");
    out.extend_from_slice(&[0xFF, marker]);
    out.extend_from_slice(&length.to_be_bytes());
    for part in parts {
        out.extend_from_slice(part);
    }
}
