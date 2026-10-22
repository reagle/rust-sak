use super::blocks;
use super::{ImageFormat, Metadata, icc_profile_bytes};

/// Reads everything [`Metadata`] can carry out of the encoded image in `bytes`, **without decoding the pixels**,
/// guessing the format from its magic bytes: the ICC profile (as [`icc_profile_bytes`] reads it), and the EXIF, XMP and
/// IPTC blocks, each byte for byte.
///
/// | Format | EXIF | XMP | IPTC |
/// |---|---|---|---|
/// | JPEG | `APP1` `Exif\0\0`, prefix removed | `APP1` XMP, extended XMP joined in | `APP13` `Photoshop 3.0`, segments joined |
/// | PNG | `eXIf` | `iTXt` `XML:com.adobe.xmp`, inflated if compressed | — |
/// | WebP | `EXIF` | `XMP ` | — |
/// | TIFF, AVIF, HEIF, BMP, GIF | not read yet | not read yet | — |
///
/// A JPEG also gives its gain-map container, for an Ultra HDR / ISO 21496-1 file:
/// [`iso_gain_map`](Metadata::iso_gain_map), the payload of its `APP2` `urn:iso:std:iso:ts:21496:-1` segment, and
/// [`gain_map`](Metadata::gain_map), the gain-map image its MPF index locates — the entry of type Gain Map, or the
/// first secondary entry when the primary declares a gain map (an ISO marker or `hdrgm` XMP) — by that entry's offset
/// and size, never the primary's recorded size. The image must lie in the file after the index, start with `SOI` and
/// hold an `EOI` (it is cut there), and be at most 64 MiB; otherwise it is `None`. Every other format gives `None` for
/// both.
///
/// **Infallible**: metadata is never a reason to refuse a photograph. An unrecognised format gives
/// [`Metadata::default()`]; a block that is truncated, overruns its container, fails its checksum, or is not what its
/// container says (an EXIF block without a TIFF header, an XMP packet that is not UTF-8, a Photoshop block that does
/// not start with `8BIM`) gives `None` for that block alone. No block is larger than 16 MiB, nor a gain map than 64 MiB. Nothing is interpreted:
/// a block that passes is returned exactly as stored.
///
/// ```
/// use image::{DynamicImage, RgbImage};
/// use rust_sak::image::{ImageFormat, Metadata, encode_writer_with, metadata_bytes};
///
/// // A minimal big-endian EXIF block: the TIFF header and an empty IFD0.
/// let exif = b"MM\0\x2a\0\0\0\x08\0\0\0\0\0\0".to_vec();
/// let mut metadata = Metadata::default();
/// metadata.exif = Some(exif.clone());
///
/// let mut jpeg = Vec::new();
/// let image = DynamicImage::ImageRgb8(RgbImage::new(2, 2));
/// encode_writer_with(&image, &mut jpeg, ImageFormat::Jpeg, None, &metadata).unwrap();
///
/// assert_eq!(metadata_bytes(&jpeg).exif, Some(exif));
/// ```
pub fn metadata_bytes(bytes: &[u8]) -> Metadata {
    let Some(format) = ImageFormat::from_magic(bytes) else {
        return Metadata::default();
    };
    let blocks = blocks::read(bytes, format);
    Metadata {
        icc_profile: icc_profile_bytes(bytes).ok().flatten(),
        exif: blocks.exif,
        xmp: blocks.xmp,
        iptc: blocks.iptc,
        iso_gain_map: blocks.iso_gain_map,
        gain_map: blocks.gain_map,
    }
}
