use std::io::Write;

use ::image::DynamicImage;

use super::dispatch::{encode_into, resolve_options};
use super::error::Result;
use super::{Embedded, EncodeOptions, ImageFormat, Metadata};

/// Encodes `image` in the given `format` with `metadata` embedded, writes the bytes to `writer`, and reports what was
/// embedded.
///
/// [`encode_writer`](super::encode_writer) with a [`Metadata`] to carry. Each block is written byte for byte where the
/// format can hold it:
///
/// | Format | ICC profile | EXIF | XMP | IPTC |
/// |---|---|---|---|---|
/// | JPEG | `APP2` `ICC_PROFILE` segments, split above 64 KiB (up to 255 × 65,519 bytes) | `APP1` (up to 65,527 bytes) | `APP1`, extended XMP above 65,502 bytes | `APP13` |
/// | PNG | `iCCP` (compressed) | `eXIf` | `iTXt` | not written |
/// | TIFF | tag 34675 (`InterColorProfile`) | not written yet | not written yet | not written yet |
/// | WebP | `ICCP` chunk, with a `VP8X` header added or updated | `EXIF` | `XMP ` | not written |
/// | BMP, GIF, AVIF, HEIF | not written | not written | not written | not written |
///
/// A block the format cannot hold is left out and reported `false` in the returned [`Embedded`]; the encode still
/// succeeds. With [`Metadata::default()`] the bytes are exactly those [`encode_writer`](super::encode_writer) writes.
///
/// # Errors
///
/// Returns [`ImageError::FormatMismatch`](super::ImageError::FormatMismatch) if `options` is for a different format
/// than `format`, [`ImageError::Io`](super::ImageError::Io) if `writer` fails, and the codec's own error if encoding
/// fails. Never because of `metadata`.
///
/// BMP, GIF and TIFF are buffered whole before being written, as with [`encode_writer`](super::encode_writer). So are a
/// JPEG, PNG or WebP with EXIF, XMP or IPTC to embed, and a WebP with a profile, whose segments or chunks are spliced in
/// once the codec has finished; without them those three stream straight to `writer`.
///
/// ```
/// use image::{DynamicImage, RgbImage};
/// use rust_sak::image::{ImageFormat, Metadata, encode_writer_with, icc_profile_bytes};
///
/// // Any ICC profile: here a stand-in with a valid header.
/// let mut profile = vec![0_u8; 132];
/// profile[0..4].copy_from_slice(&132_u32.to_be_bytes());
/// profile[36..40].copy_from_slice(b"acsp");
///
/// let mut metadata = Metadata::default();
/// metadata.icc_profile = Some(profile.clone());
///
/// let mut png = Vec::new();
/// let image = DynamicImage::ImageRgb8(RgbImage::new(2, 2));
/// let embedded = encode_writer_with(&image, &mut png, ImageFormat::Png, None, &metadata).unwrap();
///
/// assert!(embedded.icc_profile);
/// assert_eq!(icc_profile_bytes(&png).unwrap(), Some(profile));
/// ```
pub fn encode_writer_with(
    image: &DynamicImage,
    writer: &mut impl Write,
    format: ImageFormat,
    options: Option<EncodeOptions>,
    metadata: &Metadata,
) -> Result<Embedded> {
    let options = resolve_options(format, options)?;
    encode_into(image, options, metadata, writer)
}
