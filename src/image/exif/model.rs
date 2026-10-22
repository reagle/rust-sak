//! An EXIF block as a list of IFDs and their entries, parsed with every bound checked.
//!
//! The model is what both [`patch`](super::patch) and [`rebuild`](super::rebuild) work from. It records, for every
//! entry, where its value lies in the block (inline in the entry, or a range elsewhere), so a patch can tell which
//! bytes are still referenced before it zeroes any, and a rebuild can copy each value without knowing what it means.
//!
//! **Strict where it matters, lenient elsewhere.** The header and IFD0 must parse, or the block is
//! [`Malformed`](super::ExifError::Malformed). A sub-IFD (Exif, GPS, Interop) or IFD1 whose pointer leads outside the
//! block, or onto another IFD's table, is treated as absent: its pointer entry stays as it was, and nothing is
//! written through it. An entry whose out-of-line value overruns the block keeps its entry but has no value range, so
//! nothing relies on it.

use std::ops::Range;

use super::{ExifError, ExifIfd};

/// The tags whose values are offsets to other IFDs.
pub(super) const EXIF_POINTER: u16 = 0x8769;
pub(super) const GPS_POINTER: u16 = 0x8825;
pub(super) const INTEROP_POINTER: u16 = 0xA005;
/// The tag of the maker's private block in the Exif IFD.
pub(super) const MAKER_NOTE: u16 = 0x927C;
/// IFD1's thumbnail: the offset of the JPEG data and its length.
const THUMBNAIL_OFFSET: u16 = 0x0201;
const THUMBNAIL_LENGTH: u16 = 0x0202;

/// The block's byte order, from its `II`/`MM` mark.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Order {
    Little,
    Big,
}

impl Order {
    pub(super) fn u16(self, bytes: &[u8], at: usize) -> Option<u16> {
        let bytes: [u8; 2] = bytes.get(at..at.checked_add(2)?)?.try_into().ok()?;
        Some(match self {
            Order::Little => u16::from_le_bytes(bytes),
            Order::Big => u16::from_be_bytes(bytes),
        })
    }

    pub(super) fn u32(self, bytes: &[u8], at: usize) -> Option<u32> {
        let bytes: [u8; 4] = bytes.get(at..at.checked_add(4)?)?.try_into().ok()?;
        Some(match self {
            Order::Little => u32::from_le_bytes(bytes),
            Order::Big => u32::from_be_bytes(bytes),
        })
    }

    pub(super) fn u16_bytes(self, value: u16) -> [u8; 2] {
        match self {
            Order::Little => value.to_le_bytes(),
            Order::Big => value.to_be_bytes(),
        }
    }

    pub(super) fn u32_bytes(self, value: u32) -> [u8; 4] {
        match self {
            Order::Little => value.to_le_bytes(),
            Order::Big => value.to_be_bytes(),
        }
    }
}

/// The size of one value of a TIFF field type, or `None` for a type this does not know.
pub(super) fn type_size(kind: u16) -> Option<usize> {
    match kind {
        1 | 2 | 6 | 7 => Some(1),   // BYTE, ASCII, SBYTE, UNDEFINED
        3 | 8 => Some(2),           // SHORT, SSHORT
        4 | 9 | 11 | 13 => Some(4), // LONG, SLONG, FLOAT, IFD
        5 | 10 | 12 => Some(8),     // RATIONAL, SRATIONAL, DOUBLE
        _ => None,
    }
}

/// One IFD entry, as stored: its tag, type, count, and the four bytes that hold either the value or its offset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Entry {
    pub(super) tag: u16,
    pub(super) kind: u16,
    pub(super) count: u32,
    pub(super) raw: [u8; 4],
}

impl Entry {
    /// The value's size in bytes, or `None` for an unknown type or a size that overflows.
    pub(super) fn size(&self) -> Option<usize> {
        type_size(self.kind)?.checked_mul(usize::try_from(self.count).ok()?)
    }

    /// Where an out-of-line value lies in `block`, or `None` when the value is inline, of unknown size, or overruns.
    pub(super) fn range(&self, order: Order, block: &[u8]) -> Option<Range<usize>> {
        let size = self.size()?;
        if size <= 4 {
            return None;
        }
        let start = usize::try_from(order.u32(&self.raw, 0)?).ok()?;
        let end = start.checked_add(size)?;
        (end <= block.len()).then_some(start..end)
    }

    /// The value's bytes in `block`: inline from the entry, or from its range. `None` as for [`range`](Self::range).
    pub(super) fn value<'a>(&'a self, order: Order, block: &'a [u8]) -> Option<&'a [u8]> {
        let size = self.size()?;
        if size <= 4 {
            return Some(&self.raw[..size]);
        }
        Some(&block[self.range(order, block)?])
    }

    /// The entry's value read as one offset, for the pointer tags.
    pub(super) fn offset(&self, order: Order) -> Option<usize> {
        let single = self.count == 1 && matches!(self.kind, 4 | 13);
        single
            .then(|| order.u32(&self.raw, 0))
            .flatten()
            .and_then(|value| usize::try_from(value).ok())
    }
}

/// One parsed IFD: where its table is, its entries in stored order, and its next-IFD offset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Ifd {
    pub(super) at: usize,
    pub(super) entries: Vec<Entry>,
    pub(super) next: u32,
}

impl Ifd {
    /// The bytes the table occupies: the count, the entries and the next-IFD offset.
    pub(super) fn table(&self) -> Range<usize> {
        self.at..self.at + table_len(self.entries.len())
    }

    pub(super) fn find(&self, tag: u16) -> Option<&Entry> {
        self.entries.iter().find(|entry| entry.tag == tag)
    }
}

/// The length of a table of `entries` entries.
pub(super) fn table_len(entries: usize) -> usize {
    2 + 12 * entries + 4
}

/// The whole block: its byte order and the IFDs found in it.
#[derive(Debug, Clone)]
pub(super) struct Model {
    pub(super) order: Order,
    pub(super) primary: Ifd,
    pub(super) exif: Option<Ifd>,
    pub(super) gps: Option<Ifd>,
    pub(super) interop: Option<Ifd>,
    pub(super) thumbnail: Option<Ifd>,
}

impl Model {
    /// Parses `block`. [`ExifError::Malformed`] unless the header and IFD0 are sound.
    pub(super) fn parse(block: &[u8]) -> Result<Self, ExifError> {
        let order = match block.get(0..2) {
            Some(b"II") => Order::Little,
            Some(b"MM") => Order::Big,
            _ => return Err(ExifError::Malformed),
        };
        if order.u16(block, 2) != Some(42) {
            return Err(ExifError::Malformed);
        }
        let primary_at = order
            .u32(block, 4)
            .and_then(|offset| usize::try_from(offset).ok())
            .ok_or(ExifError::Malformed)?;
        let primary = parse_ifd(order, block, primary_at).ok_or(ExifError::Malformed)?;

        let mut taken = vec![primary.table()];
        let mut child = |parent: &Ifd, pointer: u16| -> Option<Ifd> {
            let at = parent.find(pointer)?.offset(order)?;
            let ifd = parse_ifd(order, block, at)?;
            claim(&mut taken, ifd)
        };
        let exif = child(&primary, EXIF_POINTER);
        let gps = child(&primary, GPS_POINTER);
        let interop = exif.as_ref().and_then(|exif| child(exif, INTEROP_POINTER));
        let thumbnail = usize::try_from(primary.next)
            .ok()
            .filter(|&at| at != 0)
            .and_then(|at| parse_ifd(order, block, at))
            .and_then(|ifd| claim(&mut taken, ifd));

        Ok(Self {
            order,
            primary,
            exif,
            gps,
            interop,
            thumbnail,
        })
    }

    pub(super) fn ifd(&self, which: ExifIfd) -> Option<&Ifd> {
        match which {
            ExifIfd::Primary => Some(&self.primary),
            ExifIfd::Exif => self.exif.as_ref(),
            ExifIfd::Gps => self.gps.as_ref(),
            ExifIfd::Interop => self.interop.as_ref(),
        }
    }

    pub(super) fn ifd_mut(&mut self, which: ExifIfd) -> Option<&mut Ifd> {
        match which {
            ExifIfd::Primary => Some(&mut self.primary),
            ExifIfd::Exif => self.exif.as_mut(),
            ExifIfd::Gps => self.gps.as_mut(),
            ExifIfd::Interop => self.interop.as_mut(),
        }
    }

    /// The thumbnail's JPEG data, from IFD1's offset and length tags, if both are there and in the block.
    pub(super) fn thumbnail_data(&self, block: &[u8]) -> Option<Range<usize>> {
        let ifd = self.thumbnail.as_ref()?;
        let single = |tag| {
            let entry = ifd.find(tag)?;
            match (entry.kind, entry.count) {
                (4, 1) => self.order.u32(&entry.raw, 0),
                (3, 1) => self.order.u16(&entry.raw, 0).map(u32::from),
                _ => None,
            }
        };
        let start = usize::try_from(single(THUMBNAIL_OFFSET)?).ok()?;
        let end = start.checked_add(usize::try_from(single(THUMBNAIL_LENGTH)?).ok()?)?;
        (end <= block.len()).then_some(start..end)
    }
}

/// `ifd`, if its table overlaps no table already `taken` (so no two IFDs are written through the same bytes).
fn claim(taken: &mut Vec<Range<usize>>, ifd: Ifd) -> Option<Ifd> {
    let table = ifd.table();
    if taken.iter().any(|other| overlaps(other, &table)) {
        return None;
    }
    taken.push(table);
    Some(ifd)
}

/// Parses the IFD table at `at`: the whole table, next-IFD offset included, must lie in `block`, past the header.
fn parse_ifd(order: Order, block: &[u8], at: usize) -> Option<Ifd> {
    if at < 8 {
        return None;
    }
    let count = usize::from(order.u16(block, at)?);
    let end = at.checked_add(table_len(count))?;
    if end > block.len() {
        return None;
    }
    let entries = (0..count)
        .map(|index| {
            let entry = at + 2 + 12 * index;
            Entry {
                tag: order.u16(block, entry).expect("inside the checked table"),
                kind: order.u16(block, entry + 2).expect("inside the checked table"),
                count: order.u32(block, entry + 4).expect("inside the checked table"),
                raw: block[entry + 8..entry + 12].try_into().expect("four bytes"),
            }
        })
        .collect();
    let next = order.u32(block, end - 4)?;
    Some(Ifd { at, entries, next })
}

/// Whether two ranges share a byte.
pub(super) fn overlaps(a: &Range<usize>, b: &Range<usize>) -> bool {
    a.start < b.end && b.start < a.end
}
