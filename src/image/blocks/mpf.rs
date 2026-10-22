//! CIPA DC-007 Multi-Picture Format: reading the MP Index a JPEG's `APP2` `MPF` segment holds, and writing the
//! two-image index of an Ultra HDR / ISO 21496-1 gain-map JPEG.
//!
//! The MP Index is a little TIFF stream: a byte-order mark, an IFD whose `MPEntry` tag (`0xB002`) points at a table of
//! 16-byte entries, one per image — its attribute (flags and type), its size, and its offset relative to the TIFF
//! header's first byte (zero for the primary image, which starts the file). Every read here is bounded by the segment:
//! the entries are read from the bytes present, never from a count the file claims.

/// The `APP2` header that marks an MP Index.
pub(super) const HEADER: &[u8] = b"MPF\0";
/// MPF's `MPEntry` tag, whose value is one 16-byte entry per image.
const MP_ENTRY: u16 = 0xB002;
/// The MP Entry image type code of a gain map (the low 24 bits of an entry's attribute).
const TYPE_GAIN_MAP: u32 = 0x05_0000;
/// The low 24 bits of an attribute: the image type. The top byte holds flags (dependent parent/child, representative).
const TYPE_MASK: u32 = 0x00FF_FFFF;

/// One MP Entry, in the index's byte order already resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Entry {
    /// Flags and type.
    pub(super) attribute: u32,
    /// The image's size in bytes, as the index records it. A primary's is often stale (an editor that grew the
    /// primary's metadata updated only the offsets), so nothing here relies on it.
    pub(super) size: u32,
    /// The image's offset from the MP Index's TIFF header; zero for the primary.
    pub(super) offset: u32,
}

impl Entry {
    /// Whether the entry's type is Gain Map.
    pub(super) fn is_gain_map(self) -> bool {
        self.attribute & TYPE_MASK == TYPE_GAIN_MAP
    }
}

/// The MP Entries of the `APP2` segment `data`, in order; nothing if it is not an MP Index or cannot be walked. Lazy,
/// and bounded by the segment: a table that claims more entries than it holds yields those present.
pub(super) fn entries(data: &[u8]) -> impl Iterator<Item = Entry> + '_ {
    let (table, big) = table(data).unwrap_or((&[], true));
    table.as_chunks::<16>().0.iter().map(move |entry| {
        let u32_at = |at: usize| {
            let quad = [entry[at], entry[at + 1], entry[at + 2], entry[at + 3]];
            if big {
                u32::from_be_bytes(quad)
            } else {
                u32::from_le_bytes(quad)
            }
        };
        Entry {
            attribute: u32_at(0),
            size: u32_at(4),
            offset: u32_at(8),
        }
    })
}

/// The MP Entry table of `data` and whether it is big-endian.
fn table(data: &[u8]) -> Option<(&[u8], bool)> {
    let tiff = data.strip_prefix(HEADER)?;
    let big = match tiff.get(0..4)? {
        b"MM\0\x2a" => true,
        b"II\x2a\0" => false,
        _ => return None,
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

    let ifd = usize::try_from(u32_at(4)?).ok()?;
    let count = usize::from(u16_at(ifd)?);
    let entry = (0..count)
        .map_while(|i| ifd.checked_add(2)?.checked_add(i.checked_mul(12)?))
        .take_while(|&at| at.checked_add(12).is_some_and(|end| end <= tiff.len()))
        .find(|&at| u16_at(at) == Some(MP_ENTRY))?;
    let length = usize::try_from(u32_at(entry + 4)?).ok()?;
    let offset = usize::try_from(u32_at(entry + 8)?).ok()?;
    Some((tiff.get(offset..offset.checked_add(length)?)?, big))
}

/// Whether the `APP2` segment `data` is an MP Index with an entry of type Gain Map.
pub(super) fn has_gain_map(data: &[u8]) -> bool {
    entries(data).any(Entry::is_gain_map)
}

/// How many bytes the marker, length and `MPF\0` take ahead of the TIFF header: offsets in the index are relative to
/// the segment's start plus this.
pub(super) const TIFF_AT: usize = 2 + 2 + HEADER.len();

/// The `APP2` segment [`index`] writes: marker and length included, 90 bytes (its length field says 88).
pub(super) const INDEX_LEN: usize = 90;
/// Where, from the segment's first byte, the primary's size, the gain map's size and the gain map's offset are.
const PRIMARY_SIZE_AT: usize = TIFF_AT + 50 + 4;
const GAIN_MAP_SIZE_AT: usize = TIFF_AT + 50 + 16 + 4;
const GAIN_MAP_OFFSET_AT: usize = GAIN_MAP_SIZE_AT + 4;

/// The complete `APP2` MP Index segment of a primary image and one gain map, big-endian as Lightroom and `libultrahdr`
/// write it, with both sizes and the gain map's offset zero for [`patch`] to fill in once they are known (the segment's
/// size does not depend on them, so patching moves nothing).
///
/// Three tags — `MPFVersion` `0100`, `NumberOfImages` 2, `MPEntry` 32 bytes at offset 50 — then the two entries: the
/// primary (representative, Baseline MP Primary, offset 0) and the gain map (type Gain Map).
pub(super) fn index() -> [u8; INDEX_LEN] {
    let mut segment = [0_u8; INDEX_LEN];
    let mut at = 0;
    let mut put = |bytes: &[u8]| {
        segment[at..at + bytes.len()].copy_from_slice(bytes);
        at += bytes.len();
    };
    put(&[0xFF, 0xE2]);
    put(&88_u16.to_be_bytes());
    put(HEADER);
    put(b"MM\0\x2a");
    put(&8_u32.to_be_bytes());
    put(&3_u16.to_be_bytes());
    // Tag, type (7 = UNDEFINED, 4 = LONG), count, value or offset.
    put(&[0xB0, 0x00, 0x00, 0x07, 0, 0, 0, 4, b'0', b'1', b'0', b'0']);
    put(&[0xB0, 0x01, 0x00, 0x04, 0, 0, 0, 1, 0, 0, 0, 2]);
    put(&[0xB0, 0x02, 0x00, 0x07, 0, 0, 0, 32, 0, 0, 0, 50]);
    // No next IFD.
    put(&[0, 0, 0, 0]);
    // The primary: representative (0x20) Baseline MP Primary (0x030000); size, offset 0, no dependents.
    put(&[0x20, 0x03, 0x00, 0x00]);
    put(&[0; 12]);
    // The gain map: no flags, type Gain Map; size, offset, no dependents.
    put(&TYPE_GAIN_MAP.to_be_bytes());
    put(&[0; 12]);
    debug_assert_eq!(at, INDEX_LEN);
    segment
}

/// Fills in the sizes and the gain map's offset of the [`index`] segment that starts at `at` in `file`, the primary
/// image of `primary` bytes being `file` itself and the gain map of `gain_map` bytes following it directly. `None` if
/// the segment is not there or a value does not fit in 32 bits.
pub(super) fn patch(file: &mut [u8], at: usize, primary: usize, gain_map: usize) -> Option<()> {
    let offset = primary.checked_sub(at.checked_add(TIFF_AT)?)?;
    let values = [
        (PRIMARY_SIZE_AT, u32::try_from(primary).ok()?),
        (GAIN_MAP_SIZE_AT, u32::try_from(gain_map).ok()?),
        (GAIN_MAP_OFFSET_AT, u32::try_from(offset).ok()?),
    ];
    let segment = file.get_mut(at..at.checked_add(INDEX_LEN)?)?;
    for (field, value) in values {
        segment[field..field + 4].copy_from_slice(&value.to_be_bytes());
    }
    Some(())
}
