//! Reading the EXIF, XMP and IPTC blocks out of an encoded file, and splicing them into one, without interpreting
//! them.
//!
//! Everything here works on the container — JPEG segments, PNG chunks, RIFF chunks — and treats each block as opaque
//! bytes. Nothing is parsed beyond the few bytes that say whether a block is plausibly what its container claims (an
//! EXIF block's TIFF header, an XMP packet's UTF-8), so that a malformed block is left out rather than handed on.
//!
//! The input is untrusted: every length is checked against the bytes actually present, every size is bounded by
//! [`MAX_BLOCK_LEN`], and nothing here panics on any input (the tests feed truncations and byte flips of every
//! fixture).

mod exif;
pub(super) mod jpeg;
pub(super) mod png;
mod webp;
mod xmp;

#[cfg(test)]
mod tests;

use super::{ImageFormat, Metadata};

/// The largest EXIF, XMP or IPTC block read or embedded. A JPEG cannot carry an EXIF block over 64 KiB at all, and real
/// XMP packets reach a few hundred kilobytes; a PNG's compressed `iTXt` is the one place a small file could claim a huge
/// block, so the bound is what keeps a hostile one from allocating without limit.
pub(super) const MAX_BLOCK_LEN: usize = 16 * 1024 * 1024;

/// The three descriptive blocks of a file, borrowed or owned as the reader found them.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(super) struct Blocks {
    pub(super) exif: Option<Vec<u8>>,
    pub(super) xmp: Option<Vec<u8>>,
    pub(super) iptc: Option<Vec<u8>>,
}

/// Reads the EXIF, XMP and IPTC blocks of `bytes`, known to be in `format`. Formats without a reader yet (TIFF, AVIF,
/// HEIF, BMP, GIF) give nothing; a malformed block gives `None` for that block only.
pub(super) fn read(bytes: &[u8], format: ImageFormat) -> Blocks {
    match format {
        ImageFormat::Jpeg => jpeg::read(bytes),
        ImageFormat::Png => png::read(bytes),
        ImageFormat::WebP => webp::read(bytes),
        _ => Blocks::default(),
    }
}

/// What of `metadata` a splice into an encoded file has to write: the three descriptive blocks, each only if present
/// and within [`MAX_BLOCK_LEN`].
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct ToWrite<'a> {
    pub(super) exif: Option<&'a [u8]>,
    pub(super) xmp: Option<&'a [u8]>,
    pub(super) iptc: Option<&'a [u8]>,
}

impl<'a> ToWrite<'a> {
    pub(super) fn of(metadata: &'a Metadata) -> Self {
        let bounded = |block: &'a Option<Vec<u8>>| block.as_deref().filter(|block| block.len() <= MAX_BLOCK_LEN);
        Self {
            exif: bounded(&metadata.exif),
            xmp: bounded(&metadata.xmp),
            iptc: bounded(&metadata.iptc),
        }
    }

    /// Whether there is nothing to splice, in which case the encoder's output is written as it stands.
    pub(super) fn is_empty(&self) -> bool {
        self.exif.is_none() && self.xmp.is_none() && self.iptc.is_none()
    }
}

/// Which blocks a splice wrote.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct Wrote {
    pub(super) exif: bool,
    pub(super) xmp: bool,
    pub(super) iptc: bool,
}

/// `encoded`, a file the `format` encoder just wrote, with the blocks of `blocks` it can hold spliced in, and which
/// those were. `None` when `encoded` could not be walked (it came from this crate's own encoders, so that is not
/// expected) — the caller then writes it as it stands.
pub(super) fn splice(encoded: &[u8], format: ImageFormat, blocks: ToWrite<'_>) -> Option<(Vec<u8>, Wrote)> {
    match format {
        ImageFormat::Jpeg => jpeg::splice(encoded, blocks),
        ImageFormat::Png => png::splice(encoded, blocks),
        _ => None,
    }
}

/// A big-endian `u16` at `at`, if the bytes are there.
fn be_u16(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(bytes.get(at..at.checked_add(2)?)?.try_into().ok()?))
}

/// A big-endian `u32` at `at`, if the bytes are there.
fn be_u32(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(bytes.get(at..at.checked_add(4)?)?.try_into().ok()?))
}
