use super::ImageFormat;
use super::dispatch::icc_profile_with_format;
use super::error::{ImageError, Result};

/// Returns the exact bytes of the ICC colour profile the encoded image in `bytes` embeds, **without decoding the
/// pixels**, guessing the format from its magic bytes.
///
/// `Ok(None)` when the file embeds no profile — including an AVIF or HEIF that describes its colour only with coded
/// (`nclx`) values — or when what it embeds is not a usable ICC profile (shorter than the 128-byte header, missing the
/// `acsp` signature, or larger than 16 MiB). The bytes are otherwise returned verbatim, ready for
/// [`Metadata::icc_profile`](super::Metadata::icc_profile).
///
/// Where it is found: JPEG `APP2` (reassembled across segments), PNG `iCCP`, TIFF tag 34675, WebP `ICCP`, and the
/// AVIF/HEIF primary item's `colr` box of type `prof`/`rICC`. BMP and GIF give `None`.
///
/// # Errors
///
/// Returns [`ImageError::UnrecognizedFormat`] if no supported format's signature matches, and the codec's own error
/// if a native format's header is malformed. A malformed or truncated AVIF, HEIF or WebP container gives `Ok(None)`
/// rather than an error: the profile is a label, not the image.
///
/// ```
/// use image::{DynamicImage, RgbImage};
/// use rust_sak::image::{ImageFormat, encode_writer, icc_profile_bytes};
///
/// let mut png = Vec::new();
/// encode_writer(&DynamicImage::ImageRgb8(RgbImage::new(2, 2)), &mut png, ImageFormat::Png, None).unwrap();
/// assert_eq!(icc_profile_bytes(&png).unwrap(), None);
/// ```
pub fn icc_profile_bytes(bytes: &[u8]) -> Result<Option<Vec<u8>>> {
    let format = ImageFormat::from_magic(bytes).ok_or(ImageError::UnrecognizedFormat)?;
    icc_profile_with_format(bytes, format)
}
