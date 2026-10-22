//! WebP: the `EXIF` and `XMP ` RIFF chunks. Writing them is [`riff`](crate::image::riff)'s, which rewrites the
//! encoder's output once for the ICC profile and these together. WebP has no place for a Photoshop resource block.

use super::{Blocks, exif, xmp};
use crate::image::container::riff_chunk;

/// Reads a WebP's EXIF and XMP. The chunks are found wherever they are; the `VP8X` flags that should announce them are
/// not trusted either way, since writers disagree about setting them.
pub(super) fn read(bytes: &[u8]) -> Blocks {
    let mut blocks = Blocks::default();
    if bytes.get(0..4) != Some(&b"RIFF"[..]) || bytes.get(8..12) != Some(&b"WEBP"[..]) {
        return blocks;
    }
    let declared = bytes
        .get(4..8)
        .and_then(|size| size.try_into().ok())
        .map_or(0, |size| u32::from_le_bytes(size) as usize);
    let end = declared.saturating_add(8).min(bytes.len());

    let mut rest = bytes.get(12..end).unwrap_or_default();
    let mut seen_exif = false;
    while let Ok((kind, data, next)) = riff_chunk(rest) {
        match &kind {
            b"EXIF" if !seen_exif => {
                seen_exif = true;
                // Some writers put JPEG's `Exif\0\0` in front of the TIFF stream; it is not part of the block.
                let block = data.strip_prefix(b"Exif\0\0").unwrap_or(data);
                blocks.exif = exif::plausible(block).then(|| block.to_vec());
            }
            b"XMP " if blocks.xmp.is_none() => blocks.xmp = xmp::plausible(data).then(|| data.to_vec()),
            _ => {}
        }
        if next.len() == rest.len() {
            break;
        }
        rest = next;
    }
    blocks
}
