//! Reading single fields of an EXIF block, and changing fields without moving the bytes that are kept.
//!
//! [`Metadata::exif`](super::Metadata::exif) is carried byte for byte; a program that re-encodes a photograph usually
//! needs to change a few of its fields first — the pixel dimensions after a resize, the software that wrote the file,
//! the thumbnail of a picture that no longer looks like it. Rebuilding the block from parsed fields moves every value,
//! and a MakerNote — the camera maker's private block, whose internal pointers are, for Canon, Sony, Pentax and others,
//! absolute from the start of the TIFF stream — is silently corrupted by the move. [`patch_exif`] therefore changes
//! fields **without moving any byte that is kept**, and [`rebuild_exif`] is the compact fallback for when that cannot
//! fit, which leaves the MakerNote out rather than breaking it.
//!
//! Which fields to change, and to what, is the caller's decision; nothing here knows what a tag means beyond the
//! pointers that lead to the Exif, GPS and Interop IFDs and the thumbnail.

mod model;
mod patch;
mod rebuild;

#[cfg(test)]
mod tests;

use model::Model;

/// The largest EXIF block a JPEG holds: one `APP1` segment, less its length field and `Exif\0\0` header.
pub const MAX_JPEG_EXIF_LEN: usize = 65_535 - 2 - 6;

/// One of the IFDs (image file directories) of an EXIF block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ExifIfd {
    /// IFD0, the primary image's: Make, Model, Orientation, Software, DateTime, Artist, Copyright…
    Primary,
    /// The Exif IFD, reached through tag `0x8769`: exposure, capture dates and offsets, pixel dimensions, MakerNote…
    Exif,
    /// The GPS IFD, reached through tag `0x8825`.
    Gps,
    /// The Interoperability IFD, reached through tag `0xA005` of the Exif IFD.
    Interop,
}

/// A field value, of the three types callers read and write: `ASCII`, `SHORT` and `LONG`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ExifValue {
    /// `ASCII`: text up to its first NUL. Written as its bytes plus one NUL.
    Ascii(String),
    /// `SHORT`: one unsigned 16-bit value.
    Short(u16),
    /// `LONG`: one unsigned 32-bit value.
    Long(u32),
}

impl ExifValue {
    /// `(type, count, bytes)` as stored, in `order`.
    fn encode(&self, order: model::Order) -> (u16, u32, Vec<u8>) {
        match self {
            ExifValue::Ascii(text) => {
                let mut bytes = text.as_bytes().to_vec();
                bytes.push(0);
                let count = u32::try_from(bytes.len()).unwrap_or(u32::MAX);
                (2, count, bytes)
            }
            ExifValue::Short(value) => (3, 1, order.u16_bytes(*value).to_vec()),
            ExifValue::Long(value) => (4, 1, order.u32_bytes(*value).to_vec()),
        }
    }
}

/// One change [`patch_exif`] or [`rebuild_exif`] makes.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ExifEdit {
    /// Sets `tag` in `ifd` to `value`, adding the entry if it is absent. Nothing is done if the block has no such IFD:
    /// an IFD is never created.
    Set {
        /// The IFD the tag belongs to.
        ifd: ExifIfd,
        /// The tag.
        tag: u16,
        /// Its new value.
        value: ExifValue,
    },
    /// Sets `tag` in `ifd` to `value` only if the entry is already there.
    Replace {
        /// The IFD the tag belongs to.
        ifd: ExifIfd,
        /// The tag.
        tag: u16,
        /// Its new value.
        value: ExifValue,
    },
    /// Removes IFD1 — the thumbnail's directory — and the thumbnail it points to.
    DropThumbnail,
}

/// Why an EXIF block could not be patched or rebuilt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ExifError {
    /// The block's TIFF header or IFD0 does not parse: there is no structure to change safely.
    #[error("the EXIF block is not a well-formed TIFF structure")]
    Malformed,
    /// An edit names a tag that holds an offset to another IFD (`0x8769`, `0x8825`, `0xA005`), which only this module
    /// may change.
    #[error("tag {0:#06x} is an IFD pointer and cannot be edited")]
    PointerTag(u16),
}

/// What [`rebuild_exif`] produced.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Rebuilt {
    /// The new EXIF block.
    pub exif: Vec<u8>,
    /// Whether the source held a MakerNote, which the rebuild left out.
    pub maker_note_dropped: bool,
}

/// Reads one field of the EXIF block `exif`: `None` when the block does not parse, the IFD or the tag is absent, or
/// the value is not a single `SHORT`, a single `LONG` or `ASCII`.
///
/// ```
/// use rust_sak::image::{ExifIfd, ExifValue, exif_field};
///
/// // Big-endian, IFD0 with one entry: Orientation (0x0112), SHORT, 6.
/// let exif = b"MM\0\x2a\0\0\0\x08\0\x01\x01\x12\0\x03\0\0\0\x01\0\x06\0\0\0\0\0\0";
/// assert_eq!(exif_field(exif, ExifIfd::Primary, 0x0112), Some(ExifValue::Short(6)));
/// assert_eq!(exif_field(exif, ExifIfd::Exif, 0x9003), None);
/// ```
pub fn exif_field(exif: &[u8], ifd: ExifIfd, tag: u16) -> Option<ExifValue> {
    let model = Model::parse(exif).ok()?;
    let entry = model.ifd(ifd)?.find(tag)?;
    let value = entry.value(model.order, exif)?;
    match (entry.kind, entry.count) {
        (2, _) => {
            let text = value.split(|&byte| byte == 0).next().unwrap_or_default();
            Some(ExifValue::Ascii(String::from_utf8_lossy(text).into_owned()))
        }
        (3, 1) => model.order.u16(value, 0).map(ExifValue::Short),
        (4, 1) => model.order.u32(value, 0).map(ExifValue::Long),
        _ => None,
    }
}

/// Applies `edits` to the EXIF block `exif` **without moving any byte that is kept**, and returns the new block.
///
/// - A value that fits in its entry's four bytes is written there (a `SHORT` may become a `LONG` in the same slot).
/// - A longer value is written over the old one if it is no longer, and otherwise appended and its entry repointed.
/// - An IFD that gains an entry is copied, entries in tag order, to free space, and its parent pointer updated.
/// - [`ExifEdit::DropThumbnail`] sets IFD0's next-IFD offset to 0 and frees IFD1 and the thumbnail.
///
/// Every byte freed is zeroed — unless another entry's value overlaps it, in which case it is left alone. Freed bytes
/// that reach the end of the block are cut off before anything is appended, and the rest are reused before the block
/// grows. So a MakerNote, which is an out-of-line value like any other, keeps both its bytes and its offset, and its
/// absolute internal pointers stay valid; so does anything else in the block that nothing here touches.
///
/// The one residual risk is one every EXIF editor shares: a MakerNote whose internal pointers reach outside its own
/// declared extent, into bytes this frees. They are freed only when no parsed entry points into them.
///
/// # Errors
///
/// [`ExifError::Malformed`] if the header or IFD0 does not parse, [`ExifError::PointerTag`] if an edit names an IFD
/// pointer. A missing sub-IFD is not an error: edits to it are skipped.
///
/// ```
/// use rust_sak::image::{ExifEdit, ExifIfd, ExifValue, exif_field, patch_exif};
///
/// let exif = b"MM\0\x2a\0\0\0\x08\0\x01\x01\x12\0\x03\0\0\0\x01\0\x06\0\0\0\0\0\0";
/// let software = ExifValue::Ascii("Example 1.0".into());
/// let edits = [ExifEdit::Set { ifd: ExifIfd::Primary, tag: 0x0131, value: software.clone() }];
///
/// let patched = patch_exif(exif, &edits).unwrap();
/// assert_eq!(exif_field(&patched, ExifIfd::Primary, 0x0131), Some(software));
/// assert_eq!(exif_field(&patched, ExifIfd::Primary, 0x0112), Some(ExifValue::Short(6)));
/// ```
pub fn patch_exif(exif: &[u8], edits: &[ExifEdit]) -> Result<Vec<u8>, ExifError> {
    reject_pointer_edits(edits)?;
    patch::patch(exif, Model::parse(exif)?, edits)
}

/// Writes a new, compact EXIF block holding every field of `exif` with `edits` applied, **without** IFD1 (the
/// thumbnail) and **without** the MakerNote, and reports whether a MakerNote was left out.
///
/// The fallback for when [`patch_exif`]'s result cannot be used — in practice, when it would not fit a JPEG's
/// [`MAX_JPEG_EXIF_LEN`]. Every value moves, so a MakerNote cannot come along safely; every standard field does. An
/// entry of a type this does not know, or whose value lies outside the block, is left out too.
///
/// # Errors
///
/// As [`patch_exif`].
pub fn rebuild_exif(exif: &[u8], edits: &[ExifEdit]) -> Result<Rebuilt, ExifError> {
    reject_pointer_edits(edits)?;
    Ok(rebuild::rebuild(exif, &Model::parse(exif)?, edits))
}

fn reject_pointer_edits(edits: &[ExifEdit]) -> Result<(), ExifError> {
    for edit in edits {
        if let ExifEdit::Set { tag, .. } | ExifEdit::Replace { tag, .. } = edit
            && matches!(*tag, model::EXIF_POINTER | model::GPS_POINTER | model::INTEROP_POINTER)
        {
            return Err(ExifError::PointerTag(*tag));
        }
    }
    Ok(())
}
