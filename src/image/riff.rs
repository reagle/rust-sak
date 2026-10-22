//! Rewrites a finished WebP file's RIFF chunks, for what `webp-rs` has no way to write itself.
//!
//! libwebp embeds metadata through its mux API, which `webp-rs` does not expose, so the encoder's output is edited
//! instead. A simple file (one `VP8 ` or `VP8L` chunk) is promoted to the extended format by putting a `VP8X` header in
//! front of it; an extended one has its `VP8X` flags updated. The RIFF size is recomputed from what is written.
//!
//! The chunk order is the one the WebP container specification requires: `VP8X`, then `ICCP`, then everything else
//! the encoder wrote, in the order it wrote it.

use super::container::riff_chunk;

/// The `VP8X` flag that says an `ICCP` chunk follows.
const VP8X_ICC: u8 = 0x20;
/// The `VP8X` flag that says the image has an alpha channel.
const VP8X_ALPHA: u8 = 0x10;

/// Returns `webp` with `profile` embedded as its `ICCP` chunk, replacing any profile already there, or `None` when
/// `webp` is not a WebP file this can rewrite (malformed, truncated, or too large for RIFF once the profile is added).
pub(super) fn with_iccp(webp: &[u8], profile: &[u8]) -> Option<Vec<u8>> {
    if webp.get(0..4)? != b"RIFF" || webp.get(8..12)? != b"WEBP" {
        return None;
    }
    let declared = u32::from_le_bytes(webp.get(4..8)?.try_into().ok()?) as usize;
    let mut rest = webp.get(12..8usize.checked_add(declared)?)?;

    let mut chunks = Vec::new();
    while !rest.is_empty() {
        let (kind, data, next) = riff_chunk(rest).ok()?;
        chunks.push((kind, data));
        rest = next;
    }

    let (first, data) = *chunks.first()?;
    let vp8x = match &first {
        b"VP8X" => {
            let mut header: [u8; 10] = data.get(0..10)?.try_into().ok()?;
            header[0] |= VP8X_ICC;
            header
        }
        b"VP8 " => extended_header(vp8_size(data)?, false)?,
        b"VP8L" => {
            let (size, alpha) = vp8l_size(data)?;
            extended_header(size, alpha)?
        }
        _ => return None,
    };

    let mut out = Vec::with_capacity(webp.len() + profile.len() + 32);
    out.extend_from_slice(b"RIFF\0\0\0\0WEBP");
    push_chunk(&mut out, b"VP8X", &vp8x)?;
    push_chunk(&mut out, b"ICCP", profile)?;
    for (kind, data) in &chunks {
        if kind != b"VP8X" && kind != b"ICCP" {
            push_chunk(&mut out, kind, data)?;
        }
    }

    let riff_size = u32::try_from(out.len() - 8).ok()?;
    out[4..8].copy_from_slice(&riff_size.to_le_bytes());
    Some(out)
}

/// A `VP8X` payload for a still image of `(width, height)`, with the ICC flag and, if `alpha`, the alpha flag set.
fn extended_header((width, height): (u32, u32), alpha: bool) -> Option<[u8; 10]> {
    // The canvas is stored as one less than its size, in 24 bits.
    let (width, height) = (width.checked_sub(1)?, height.checked_sub(1)?);
    if width >= 1 << 24 || height >= 1 << 24 {
        return None;
    }

    let mut header = [0_u8; 10];
    header[0] = VP8X_ICC | if alpha { VP8X_ALPHA } else { 0 };
    header[4..7].copy_from_slice(&width.to_le_bytes()[..3]);
    header[7..10].copy_from_slice(&height.to_le_bytes()[..3]);
    Some(header)
}

/// The frame size from a lossy `VP8 ` chunk's key-frame header: a three-byte frame tag, the `9d 01 2a` start code,
/// then two little-endian 14-bit dimensions (the top two bits of each are a scaling hint, not size).
fn vp8_size(data: &[u8]) -> Option<(u32, u32)> {
    if data.get(3..6)? != [0x9d, 0x01, 0x2a] {
        return None;
    }
    let width = u16::from_le_bytes(data.get(6..8)?.try_into().ok()?) & 0x3FFF;
    let height = u16::from_le_bytes(data.get(8..10)?.try_into().ok()?) & 0x3FFF;
    Some((u32::from(width), u32::from(height)))
}

/// The image size and whether alpha is used, from a lossless `VP8L` chunk's header: the `0x2f` signature, then a
/// little-endian word holding 14 bits of width − 1, 14 bits of height − 1 and the `alpha_is_used` bit.
fn vp8l_size(data: &[u8]) -> Option<((u32, u32), bool)> {
    if *data.first()? != 0x2F {
        return None;
    }
    let bits = u32::from_le_bytes(data.get(1..5)?.try_into().ok()?);
    let width = (bits & 0x3FFF) + 1;
    let height = ((bits >> 14) & 0x3FFF) + 1;
    Some(((width, height), (bits >> 28) & 1 == 1))
}

/// Appends one RIFF chunk, padded to an even length; `None` if `data` is too large for a chunk.
fn push_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) -> Option<()> {
    let size = u32::try_from(data.len()).ok()?;
    // The padded chunk must still leave the RIFF size representable.
    if size > u32::MAX - 16 {
        return None;
    }
    out.extend_from_slice(kind);
    out.extend_from_slice(&size.to_le_bytes());
    out.extend_from_slice(data);
    if !data.len().is_multiple_of(2) {
        out.push(0);
    }
    Some(())
}
