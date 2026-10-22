//! JPEG: the `APP1` EXIF and XMP segments (extended XMP included), the `APP13` Photoshop segments, and the gain-map
//! container — the `APP2` ISO 21496-1 segment and the MPF gain-map image after the primary's `EOI`.

use super::{Blocks, MAX_BLOCK_LEN, MAX_GAIN_MAP_LEN, ToWrite, Wrote, be_u16, be_u32, exif, mpf, xmp};

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
/// The ISO 21496-1 gain-map `APP2` header: a primary's version marker, or a gain-map image's metadata.
const ISO_GAIN_MAP_HEADER: &[u8] = b"urn:iso:std:iso:ts:21496:-1\0";
/// The namespace of Adobe's and Android's (Ultra HDR) gain-map XMP description, `hdrgm:`.
const HDRGM_NAMESPACE: &[u8] = b"http://ns.adobe.com/hdr-gain-map/1.0/";
/// How many secondary MP Entries are looked at for the gain map: a gain-map JPEG has one, and a phone's may put a
/// depth map or a preview beside it.
const SECONDARY_ENTRIES: usize = 2;

/// The most data one segment holds: its length field counts itself, and is 16 bits.
const MAX_SEGMENT_DATA: usize = 65_535 - 2;
/// The largest EXIF block one `APP1` holds once its `Exif\0\0` header is in.
pub(super) const MAX_EXIF_LEN: usize = MAX_SEGMENT_DATA - EXIF_HEADER.len();
/// The most of an extended XMP packet one chunk carries: what is left after the header, the GUID and the two offsets.
const EXTENDED_CHUNK_LEN: usize = MAX_SEGMENT_DATA - EXTENDED_HEADER.len() - 32 - 4 - 4;
/// The most of a Photoshop resource block one `APP13` carries.
const PHOTOSHOP_CHUNK_LEN: usize = MAX_SEGMENT_DATA - PHOTOSHOP_HEADER.len();
/// The largest ISO 21496-1 payload one `APP2` carries; it is never split.
const MAX_ISO_GAIN_MAP_LEN: usize = MAX_SEGMENT_DATA - ISO_GAIN_MAP_HEADER.len();

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
        APP2 => data.starts_with(ISO_GAIN_MAP_HEADER) || mpf::has_gain_map(data),
        APP1 => data.starts_with(XMP_HEADER) && xmp::contains(&data[XMP_HEADER.len()..], HDRGM_NAMESPACE),
        _ => false,
    })
}

/// The gain-map image the MP Index `data` locates in `bytes`, the index's TIFF header being at `tiff_at` in it: the
/// first of the secondary entries of type Gain Map; failing that, when the primary `declared` a gain map, the first
/// secondary entry if its type is 0 (an Android file).
///
/// Located by the entry's offset and size — never by the primary's recorded size, which an editor may have left stale —
/// and then checked: it must lie inside `bytes` after the index, start with `SOI`, hold an `EOI` (where the image is
/// cut, dropping any padding after it), and be no larger than [`MAX_GAIN_MAP_LEN`]. Where the offset is stale too (an
/// editor that grew the primary after its index, as ExifTool does when it adds a `COM`, moves the gain map without
/// updating it), the first secondary entry is looked for where MPF puts it, straight after the primary's `EOI`, and
/// taken to its own `EOI` (at most [`MAX_GAIN_MAP_LEN`]); `libultrahdr` and Apple's ImageIO read such a file too.
/// Otherwise there is none.
fn gain_map_image<'a>(bytes: &'a [u8], tiff_at: usize, data: &[u8], declared: bool) -> Option<&'a [u8]> {
    let mut secondary = mpf::entries(data).skip(1).take(SECONDARY_ENTRIES);
    let first = secondary.next();
    let (index, entry) = first
        .into_iter()
        .chain(secondary)
        .enumerate()
        .find(|(_, entry)| entry.is_gain_map())
        .or(first
            .filter(|entry| declared && entry.is_untyped())
            .map(|entry| (0, entry)))?;

    let size = usize::try_from(entry.size).ok()?.min(MAX_GAIN_MAP_LEN);
    // After the index itself, which is after `SOI`: an offset of zero names the primary.
    let index_end = tiff_at.checked_add(data.len() - mpf::HEADER.len())?;
    let at = |start: usize| -> Option<&'a [u8]> {
        if start < index_end || start >= bytes.len() {
            return None;
        }
        let image = &bytes[start..start.saturating_add(size).min(bytes.len())];
        if !image.starts_with(&[0xFF, SOI, 0xFF]) {
            return None;
        }
        let end = image.windows(2).rposition(|pair| pair == [0xFF, EOI])?;
        Some(&image[..end + 2])
    };

    let recorded = usize::try_from(entry.offset)
        .ok()
        .and_then(|offset| tiff_at.checked_add(offset));
    let exact = recorded.filter(|&start| start.checked_add(size).is_some_and(|end| end <= bytes.len()));
    exact.and_then(at).or_else(|| {
        // Only the first secondary image follows the primary directly. Its recorded size is no more to be trusted than
        // its offset, so it is walked to its own `EOI`.
        if index != 0 {
            return None;
        }
        let start = primary_end(bytes).filter(|&start| start >= index_end)?;
        let rest = &bytes[start..bytes.len().min(start.saturating_add(MAX_GAIN_MAP_LEN))];
        Some(&rest[..primary_end(rest)?])
    })
}

/// Where the first image of `bytes` ends: just past its `EOI`, found by walking its segments and skipping each scan's
/// entropy-coded data (where a `0xFF` is followed by a stuffed `0x00` or is a restart marker). Linear in the file and
/// bounded by it; `None` for a file that is not a complete JPEG.
fn primary_end(bytes: &[u8]) -> Option<usize> {
    if !bytes.starts_with(&[0xFF, SOI]) {
        return None;
    }
    let mut at = 2;
    loop {
        if *bytes.get(at)? != 0xFF {
            return None;
        }
        // Any number of 0xFF fill bytes may precede a marker.
        while *bytes.get(at + 1)? == 0xFF {
            at += 1;
        }
        let marker = bytes[at + 1];
        at += 2;
        match marker {
            EOI => return Some(at),
            0x01 | 0xD0..=0xD7 => continue,
            _ => {}
        }
        let length = usize::from(be_u16(bytes, at)?);
        if length < 2 {
            return None;
        }
        at = at.checked_add(length)?;
        if marker == SOS {
            // The scan's entropy-coded data runs to the next marker that is neither a stuffed byte nor a restart.
            loop {
                let ff = at + bytes.get(at..)?.iter().position(|&byte| byte == 0xFF)?;
                match *bytes.get(ff + 1)? {
                    0x00 | 0xD0..=0xD7 => at = ff + 2,
                    _ => {
                        at = ff;
                        break;
                    }
                }
            }
        }
    }
}

/// Reads a JPEG's EXIF, XMP and IPTC, its ISO 21496-1 payload, and its gain-map image.
pub(super) fn read(bytes: &[u8]) -> Blocks {
    let mut blocks = Blocks::default();
    let mut standard: Option<&[u8]> = None;
    // Extended XMP chunks by GUID: (full length, offset, data).
    let mut extended: Vec<(&[u8], u32, u32, &[u8])> = Vec::new();
    let mut photoshop: Option<Vec<u8>> = None;
    // The first MP Index, and where its TIFF header is in `bytes`.
    let mut index: Option<(usize, &[u8])> = None;

    for (marker, data, _) in segments(bytes) {
        match marker {
            APP2 if data.starts_with(ISO_GAIN_MAP_HEADER) => {
                blocks
                    .iso_gain_map
                    .get_or_insert_with(|| data[ISO_GAIN_MAP_HEADER.len()..].to_vec());
            }
            APP2 if data.starts_with(mpf::HEADER) && index.is_none() => {
                // `data` is a slice of `bytes`, so the difference of their starts is where it is; the TIFF header
                // follows the `MPF\0`.
                let at = (data.as_ptr() as usize - bytes.as_ptr() as usize) + mpf::HEADER.len();
                index = Some((at, data));
            }
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
    let standard = standard.filter(|packet| xmp::plausible(packet));
    let declared =
        blocks.iso_gain_map.is_some() || standard.is_some_and(|packet| xmp::contains(packet, HDRGM_NAMESPACE));
    blocks.xmp = standard.map(|packet| with_extended(packet, &extended));
    blocks.gain_map = index
        .and_then(|(tiff_at, data)| gain_map_image(bytes, tiff_at, data, declared))
        .map(<[u8]>::to_vec);
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
/// XMP and any extended XMP, then `APP13`, then the `APP2` ISO 21496-1 segment, then — with a gain map — the `APP2` MP
/// Index; and the gain-map image appended after `encoded`'s `EOI`, the index patched with both images' sizes and the
/// gain map's offset.
///
/// An ISO payload too large for one segment, or a gain map that is not a complete JPEG (`SOI` to a final `EOI`), is
/// left out and reported unwritten; so is the gain map when `encoded` does not end at its `EOI`.
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
    if let Some(payload) = blocks
        .iso_gain_map
        .filter(|payload| payload.len() <= MAX_ISO_GAIN_MAP_LEN)
    {
        push_segment(&mut inserted, APP2, &[ISO_GAIN_MAP_HEADER, payload]);
        wrote.iso_gain_map = true;
    }
    let gain_map = blocks
        .gain_map
        .filter(|image| is_complete(image) && is_complete(encoded));
    // Where the index segment starts in the output.
    let index_at = gain_map.map(|_| {
        let at = insert_at + inserted.len();
        inserted.extend_from_slice(&mpf::index());
        at
    });

    let appended = gain_map.map_or(0, <[u8]>::len);
    let mut out = Vec::with_capacity(encoded.len() + inserted.len() + appended);
    out.extend_from_slice(&encoded[..insert_at]);
    out.extend_from_slice(&inserted);
    out.extend_from_slice(&encoded[insert_at..]);

    if let (Some(image), Some(at)) = (gain_map, index_at) {
        let primary = out.len();
        if mpf::patch(&mut out, at, primary, image.len()).is_some() {
            out.extend_from_slice(image);
            wrote.gain_map = true;
        } else {
            // Sizes past 4 GiB: the index cannot describe them, so it goes and the file is the primary alone.
            out.drain(at..at + mpf::INDEX_LEN);
        }
    }
    Some((out, wrote))
}

/// Whether `image` is a whole JPEG as far as its ends go: `SOI` and a marker first, `EOI` last.
fn is_complete(image: &[u8]) -> bool {
    image.starts_with(&[0xFF, SOI, 0xFF]) && image.ends_with(&[0xFF, EOI])
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
