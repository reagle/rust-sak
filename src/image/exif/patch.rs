//! [`patch_exif`](super::patch_exif): edits applied to the model, then written back over the original block without
//! moving any byte that is kept.
//!
//! In five steps:
//!
//! 1. **Decide** each edit against the original block: a value of four bytes or fewer goes inline; a longer one goes
//!    over the old value when it fits there and nothing else points into it; anything else waits for new space. An
//!    entry that is added makes its IFD *grow*, and a grown IFD is copied rather than rewritten in place.
//! 2. **Free** what is no longer referenced — old values, IFD1, the thumbnail, the old tables of grown IFDs — by
//!    zeroing it, but only where no live reference (a kept entry's value, a kept table, the header) overlaps it.
//! 3. **Trim** the zeros that end the block, back to the last live byte.
//! 4. **Allocate** waiting values and grown tables, first in freed space, then at the end.
//! 5. **Write** the tables, children before parents, so a copied table's new offset reaches its parent's pointer.

use std::ops::Range;

use super::model::{Entry, Ifd, Model, Order, STRIP_OFFSETS, THUMBNAIL_LENGTH, THUMBNAIL_OFFSET, overlaps, table_len};
use super::{ExifEdit, ExifError, ExifIfd, ExifValue};

/// The IFDs in the order their tables are written: every child before its parent.
const CHILDREN_FIRST: [ExifIfd; 4] = [ExifIfd::Interop, ExifIfd::Exif, ExifIfd::Gps, ExifIfd::Primary];

fn slot(ifd: ExifIfd) -> usize {
    match ifd {
        ExifIfd::Primary => 0,
        ExifIfd::Exif => 1,
        ExifIfd::Gps => 2,
        ExifIfd::Interop => 3,
    }
}

/// Where an edited entry's value goes.
enum Placement {
    /// In the entry itself, or in its old out-of-line range: nothing to allocate.
    Done,
    /// Over its old range, starting at the range's start.
    InPlace(Range<usize>, Vec<u8>),
    /// In new space.
    Allocate(Vec<u8>),
}

pub(super) fn patch(block: &[u8], mut model: Model, edits: &[ExifEdit]) -> Result<Vec<u8>, ExifError> {
    let order = model.order;
    let original = model.clone();
    let mut out = block.to_vec();

    // An entry of a type this does not know may hold an offset this cannot see the extent of. Then nothing is zeroed or
    // trimmed: the edits are still made, only without reclaiming space.
    let cautious = all_ifds(&original).any(|ifd| ifd.entries.iter().any(|entry| entry.size().is_none()));

    let mut released: Vec<Range<usize>> = Vec::new();
    // What a dropped thumbnail occupied: zeroed whatever else is uncertain, since it must not survive the drop.
    let mut thumbnail_bytes: Vec<Range<usize>> = Vec::new();
    let mut dirty = [false; 4];
    let mut grown = [false; 4];
    // Per IFD, per tag: the placement decided for an edited entry.
    let mut placed: Vec<(ExifIfd, u16, Placement)> = Vec::new();

    // ── 1. Decide ──
    if edits.contains(&ExifEdit::DropThumbnail) {
        match model.thumbnail.take() {
            Some(thumbnail) => {
                // A thumbnail this cannot locate exactly — JPEG tags missing or out of the block, or strips — cannot be
                // proven gone, so the caller is told rather than handed a block that still holds it.
                let data = original.thumbnail_data(block);
                let strips = thumbnail.find(STRIP_OFFSETS).is_some();
                let jpeg_tags =
                    thumbnail.find(THUMBNAIL_OFFSET).is_some() || thumbnail.find(THUMBNAIL_LENGTH).is_some();
                if strips || (jpeg_tags && data.is_none()) {
                    return Err(ExifError::ThumbnailUnremovable);
                }
                thumbnail_bytes.push(thumbnail.table());
                thumbnail_bytes.extend(thumbnail.entries.iter().filter_map(|entry| entry.range(order, block)));
                thumbnail_bytes.extend(data);
            }
            // IFD0 points at an IFD1 that does not parse: whatever it holds cannot be found to be removed.
            None if model.primary.next != 0 => return Err(ExifError::ThumbnailUnremovable),
            None => {}
        }
        if model.primary.next != 0 {
            model.primary.next = 0;
            dirty[slot(ExifIfd::Primary)] = true;
        }
    }

    for (ifd, tag, value, add) in last_edit_per_tag(edits) {
        let Some(table) = model.ifd_mut(ifd) else { continue };
        let (kind, count, bytes) = value.encode(order);
        let room = table.entries.len() < usize::from(u16::MAX);
        let placement = match table.entries.iter_mut().find(|entry| entry.tag == tag) {
            Some(entry) => {
                let old = entry.range(order, block);
                entry.kind = kind;
                entry.count = count;
                if bytes.len() <= 4 {
                    entry.raw = inline(&bytes);
                    released.extend(old);
                    Placement::Done
                } else {
                    match old {
                        Some(old) if bytes.len() <= old.len() && !shared(&original, order, block, &old, ifd, tag) => {
                            released.push(old.start + bytes.len()..old.end);
                            Placement::InPlace(old, bytes)
                        }
                        old => {
                            released.extend(old);
                            Placement::Allocate(bytes)
                        }
                    }
                }
            }
            // A table already at the 65,535 entries its count can say has no room for another.
            None if add && room => {
                let raw = if bytes.len() <= 4 { inline(&bytes) } else { [0; 4] };
                table.entries.push(Entry { tag, kind, count, raw });
                grown[slot(ifd)] = true;
                if bytes.len() <= 4 {
                    Placement::Done
                } else {
                    Placement::Allocate(bytes)
                }
            }
            None => continue,
        };
        dirty[slot(ifd)] = true;
        placed.push((ifd, tag, placement));
    }

    for ifd in CHILDREN_FIRST {
        if grown[slot(ifd)]
            && let Some(table) = original.ifd(ifd)
        {
            released.push(table.table());
        }
    }

    // ── 2. Free ──
    let waiting = |ifd: ExifIfd, tag: u16| {
        placed
            .iter()
            .any(|(at, edited, placement)| *at == ifd && *edited == tag && matches!(placement, Placement::Allocate(_)))
    };
    // The header is always live.
    let mut live: Vec<Range<usize>> = Vec::new();
    live.push(0..8);
    for ifd in CHILDREN_FIRST {
        let Some(table) = model.ifd(ifd) else { continue };
        if !grown[slot(ifd)] {
            live.push(table.table());
        }
        for entry in &table.entries {
            if waiting(ifd, entry.tag) {
                continue;
            }
            let in_place = placed.iter().find_map(|(at, edited, placement)| match placement {
                Placement::InPlace(range, bytes) if *at == ifd && *edited == entry.tag => {
                    Some(range.start..range.start + bytes.len())
                }
                _ => None,
            });
            live.extend(in_place.or_else(|| entry.range(order, block)));
        }
    }
    if let Some(thumbnail) = model.thumbnail.as_ref() {
        live.push(thumbnail.table());
        live.extend(thumbnail.entries.iter().filter_map(|entry| entry.range(order, block)));
        live.extend(model.thumbnail_data(block));
    }

    let mut free: Vec<Range<usize>> = Vec::new();
    for range in thumbnail_bytes {
        if range.is_empty() {
            continue;
        }
        // Something still in use shares the thumbnail's bytes: they can be neither kept nor zeroed.
        if live.iter().any(|kept| overlaps(kept, &range)) {
            return Err(ExifError::ThumbnailUnremovable);
        }
        out[range.clone()].fill(0);
        free.push(range);
    }
    if !cautious {
        for range in released {
            if range.is_empty() || live.iter().any(|kept| overlaps(kept, &range)) {
                continue;
            }
            out[range.clone()].fill(0);
            free.push(range);
        }
    }

    // The same bytes can be released twice (two edited entries sharing one value, a thumbnail inside IFD1's values):
    // merged, so no byte is handed out by two allocations.
    free.sort_by_key(|range| range.start);
    free = free
        .into_iter()
        .fold(Vec::new(), |mut merged: Vec<Range<usize>>, range| {
            match merged.last_mut() {
                Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
                _ => merged.push(range),
            }
            merged
        });

    // ── 3. Trim ──
    // Only bytes this freed, and only where they reach the end of the block: a structure the model does not see (a
    // SubIFD, a MakerNote's own directory) may end in zeros that look unused and are not.
    if let Some(last) = free.last()
        && last.end == out.len()
    {
        // Keep the block an even length, as the end of a TIFF stream conventionally is.
        let end = (last.start + (last.start & 1)).min(out.len());
        out.truncate(end);
    }

    // ── 4. Allocate and write values ──
    // Always past the end: freed bytes in the middle are zeroed, never reused, since only what the model sees is
    // known not to point into them.
    let mut space = Space;
    for (ifd, tag, placement) in placed {
        let Some(table) = model.ifd_mut(ifd) else { continue };
        let Some(entry) = table.entries.iter_mut().find(|entry| entry.tag == tag) else {
            continue;
        };
        match placement {
            Placement::Done => {}
            Placement::InPlace(range, bytes) => {
                out[range.start..range.start + bytes.len()].copy_from_slice(&bytes);
            }
            Placement::Allocate(bytes) => {
                let at = space.allocate(&mut out, bytes.len());
                out[at..at + bytes.len()].copy_from_slice(&bytes);
                entry.raw = offset_bytes(order, at)?;
            }
        }
    }

    // ── 5. Write tables, children first ──
    for ifd in CHILDREN_FIRST {
        if !dirty[slot(ifd)] {
            continue;
        }
        let Some(table) = model.ifd_mut(ifd) else { continue };
        if grown[slot(ifd)] {
            table.entries.sort_by_key(|entry| entry.tag);
            table.at = space.allocate(&mut out, table_len(table.entries.len()));
            let at = table.at;
            match ifd {
                ExifIfd::Primary => out[4..8].copy_from_slice(&offset_bytes(order, at)?),
                ExifIfd::Exif => repoint(&mut model.primary, super::model::EXIF_POINTER, order, at, &mut dirty)?,
                ExifIfd::Gps => repoint(&mut model.primary, super::model::GPS_POINTER, order, at, &mut dirty)?,
                ExifIfd::Interop => {
                    if let Some(exif) = model.exif.as_mut() {
                        repoint(exif, super::model::INTEROP_POINTER, order, at, &mut dirty)?;
                    }
                }
            }
        }
        let table = model.ifd(ifd).expect("present: checked above");
        write_table(&mut out, order, table);
    }

    Ok(out)
}

/// The edits that set a tag, the last for each `(ifd, tag)`, in the order they were given: `(ifd, tag, value, add)`.
fn last_edit_per_tag(edits: &[ExifEdit]) -> Vec<(ExifIfd, u16, &ExifValue, bool)> {
    let mut chosen: Vec<(ExifIfd, u16, &ExifValue, bool)> = Vec::new();
    for edit in edits {
        let (ifd, tag, value, add) = match edit {
            ExifEdit::Set { ifd, tag, value } => (*ifd, *tag, value, true),
            ExifEdit::Replace { ifd, tag, value } => (*ifd, *tag, value, false),
            ExifEdit::DropThumbnail => continue,
        };
        chosen.retain(|(at, edited, ..)| (*at, *edited) != (ifd, tag));
        chosen.push((ifd, tag, value, add));
    }
    chosen
}

/// Whether `range`, the value of `tag` in `ifd`, is also pointed into by anything else in the original block.
fn shared(model: &Model, order: Order, block: &[u8], range: &Range<usize>, ifd: ExifIfd, tag: u16) -> bool {
    // The TIFF header is always in use.
    let mut others: Vec<Range<usize>> = Vec::new();
    others.push(0..8);
    for which in CHILDREN_FIRST {
        let Some(table) = model.ifd(which) else { continue };
        others.push(table.table());
        for entry in &table.entries {
            if (which, entry.tag) != (ifd, tag) {
                others.extend(entry.range(order, block));
            }
        }
    }
    if let Some(thumbnail) = model.thumbnail.as_ref() {
        others.push(thumbnail.table());
        others.extend(thumbnail.entries.iter().filter_map(|entry| entry.range(order, block)));
        others.extend(model.thumbnail_data(block));
    }
    others.iter().any(|other| overlaps(other, range))
}

fn all_ifds(model: &Model) -> impl Iterator<Item = &Ifd> {
    [
        Some(&model.primary),
        model.exif.as_ref(),
        model.gps.as_ref(),
        model.interop.as_ref(),
        model.thumbnail.as_ref(),
    ]
    .into_iter()
    .flatten()
}

/// Points `parent`'s `pointer` entry at `at`, and marks the parent for rewriting.
fn repoint(parent: &mut Ifd, pointer: u16, order: Order, at: usize, dirty: &mut [bool; 4]) -> Result<(), ExifError> {
    let entry = parent
        .entries
        .iter_mut()
        .find(|entry| entry.tag == pointer)
        .ok_or(ExifError::Malformed)?;
    entry.raw = offset_bytes(order, at)?;
    let parent_slot = if pointer == super::model::INTEROP_POINTER {
        ExifIfd::Exif
    } else {
        ExifIfd::Primary
    };
    dirty[slot(parent_slot)] = true;
    Ok(())
}

fn inline(bytes: &[u8]) -> [u8; 4] {
    let mut raw = [0; 4];
    raw[..bytes.len()].copy_from_slice(bytes);
    raw
}

fn offset_bytes(order: Order, at: usize) -> Result<[u8; 4], ExifError> {
    Ok(order.u32_bytes(u32::try_from(at).map_err(|_| ExifError::Malformed)?))
}

fn write_table(out: &mut [u8], order: Order, table: &Ifd) {
    let mut at = table.at;
    let count =
        u16::try_from(table.entries.len()).expect("a parsed table has at most 65,535 entries, plus a few added");
    out[at..at + 2].copy_from_slice(&order.u16_bytes(count));
    at += 2;
    for entry in &table.entries {
        out[at..at + 2].copy_from_slice(&order.u16_bytes(entry.tag));
        out[at + 2..at + 4].copy_from_slice(&order.u16_bytes(entry.kind));
        out[at + 4..at + 8].copy_from_slice(&order.u32_bytes(entry.count));
        out[at + 8..at + 12].copy_from_slice(&entry.raw);
        at += 12;
    }
    out[at..at + 4].copy_from_slice(&order.u32_bytes(table.next));
}

/// Space to write into: the end of the block, every start on an even offset.
struct Space;

impl Space {
    fn allocate(&mut self, out: &mut Vec<u8>, len: usize) -> usize {
        if out.len() & 1 == 1 {
            out.push(0);
        }
        let start = out.len();
        out.resize(start + len, 0);
        start
    }
}
