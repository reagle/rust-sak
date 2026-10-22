//! [`rebuild_exif`](super::rebuild_exif): a fresh, compact block from the model, without IFD1 and the MakerNote.
//!
//! Laid out as header, IFD0's table and then its values, then the Exif IFD, the Interop IFD and the GPS IFD the same
//! way. Every value is copied as stored — type, count and bytes — so a field this knows nothing about survives as long
//! as its type has a known size and its value lies inside the source block.

use super::model::{EXIF_POINTER, GPS_POINTER, INTEROP_POINTER, Ifd, MAKER_NOTE, Model, Order, table_len};
use super::{ExifEdit, ExifIfd, Rebuilt};

/// One field as it will be written.
struct Field {
    tag: u16,
    kind: u16,
    count: u32,
    bytes: Vec<u8>,
}

pub(super) fn rebuild(block: &[u8], model: &Model, edits: &[ExifEdit]) -> Rebuilt {
    let order = model.order;
    let collect = |ifd: &Ifd| -> Vec<Field> {
        ifd.entries
            .iter()
            .filter(|entry| !matches!(entry.tag, EXIF_POINTER | GPS_POINTER | INTEROP_POINTER | MAKER_NOTE))
            .filter_map(|entry| {
                let bytes = entry.value(order, block)?.to_vec();
                Some(Field {
                    tag: entry.tag,
                    kind: entry.kind,
                    count: entry.count,
                    bytes,
                })
            })
            .collect()
    };

    let mut primary = collect(&model.primary);
    let mut exif = model.exif.as_ref().map(collect);
    let mut gps = model.gps.as_ref().map(collect);
    let mut interop = model.interop.as_ref().map(collect);
    let maker_note_dropped = model.exif.as_ref().is_some_and(|exif| exif.find(MAKER_NOTE).is_some());

    for edit in edits {
        let (which, tag, value, add) = match edit {
            ExifEdit::Set { ifd, tag, value } => (*ifd, *tag, value, true),
            ExifEdit::Replace { ifd, tag, value } => (*ifd, *tag, value, false),
            ExifEdit::DropThumbnail => continue,
        };
        let fields = match which {
            ExifIfd::Primary => Some(&mut primary),
            ExifIfd::Exif => exif.as_mut(),
            ExifIfd::Gps => gps.as_mut(),
            ExifIfd::Interop => interop.as_mut(),
        };
        let Some(fields) = fields else { continue };
        let (kind, count, bytes) = value.encode(order);
        let room = fields.len() < usize::from(u16::MAX) - 2;
        match fields.iter_mut().find(|field| field.tag == tag) {
            Some(field) => {
                *field = Field {
                    tag,
                    kind,
                    count,
                    bytes,
                }
            }
            // Room is kept for the pointer entries added below.
            None if add && room => fields.push(Field {
                tag,
                kind,
                count,
                bytes,
            }),
            None => {}
        }
    }

    // Lay out every table and its values, then fill in the pointers, which are fixed-size and so change no length.
    let pointer = |tag| Field {
        tag,
        kind: 4,
        count: 1,
        bytes: vec![0; 4],
    };
    if exif.is_some() {
        primary.push(pointer(EXIF_POINTER));
    }
    if gps.is_some() {
        primary.push(pointer(GPS_POINTER));
    }
    if let (Some(exif), Some(_)) = (exif.as_mut(), interop.as_ref()) {
        exif.push(pointer(INTEROP_POINTER));
    }

    let primary_at = 8;
    let exif_at = primary_at + extent(&primary);
    let interop_at = exif_at + exif.as_ref().map_or(0, |fields| extent(fields));
    let gps_at = interop_at + interop.as_ref().map_or(0, |fields| extent(fields));

    let set_pointer = |fields: &mut Vec<Field>, tag: u16, at: usize| {
        if let Some(field) = fields.iter_mut().find(|field| field.tag == tag) {
            field.bytes = order.u32_bytes(u32::try_from(at).unwrap_or(u32::MAX)).to_vec();
        }
    };
    set_pointer(&mut primary, EXIF_POINTER, exif_at);
    set_pointer(&mut primary, GPS_POINTER, gps_at);
    if let Some(exif) = exif.as_mut() {
        set_pointer(exif, INTEROP_POINTER, interop_at);
    }

    let mut out = match order {
        Order::Little => b"II\x2a\0".to_vec(),
        Order::Big => b"MM\0\x2a".to_vec(),
    };
    out.extend_from_slice(&order.u32_bytes(8));
    write_ifd(&mut out, order, &mut primary);
    for fields in [exif.as_mut(), interop.as_mut(), gps.as_mut()].into_iter().flatten() {
        write_ifd(&mut out, order, fields);
    }

    Rebuilt {
        exif: out,
        maker_note_dropped,
    }
}

/// The bytes a table of `fields` and its out-of-line values take, each value padded to an even length.
fn extent(fields: &[Field]) -> usize {
    table_len(fields.len())
        + fields
            .iter()
            .filter(|field| field.bytes.len() > 4)
            .map(|field| padded(field.bytes.len()))
            .sum::<usize>()
}

fn padded(len: usize) -> usize {
    len + (len & 1)
}

/// Appends a table of `fields`, sorted by tag, with no next IFD, followed by the values that do not fit inline.
fn write_ifd(out: &mut Vec<u8>, order: Order, fields: &mut [Field]) {
    fields.sort_by_key(|field| field.tag);
    let table_at = out.len();
    let mut value_at = table_at + table_len(fields.len());
    let count = u16::try_from(fields.len()).expect("kept within a table's count above");
    out.extend_from_slice(&order.u16_bytes(count));
    let mut values = Vec::new();
    for field in fields.iter() {
        out.extend_from_slice(&order.u16_bytes(field.tag));
        out.extend_from_slice(&order.u16_bytes(field.kind));
        out.extend_from_slice(&order.u32_bytes(field.count));
        if field.bytes.len() <= 4 {
            let mut raw = [0; 4];
            raw[..field.bytes.len()].copy_from_slice(&field.bytes);
            out.extend_from_slice(&raw);
        } else {
            out.extend_from_slice(&order.u32_bytes(u32::try_from(value_at).unwrap_or(u32::MAX)));
            values.extend_from_slice(&field.bytes);
            if field.bytes.len() & 1 == 1 {
                values.push(0);
            }
            value_at += padded(field.bytes.len());
        }
    }
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&values);
}
