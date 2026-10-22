use super::{ImageFormat, blocks, container, icc, icc_profile_bytes};

/// The ITU-T H.273 transfer characteristics of SMPTE ST 2084 (PQ).
const TRANSFER_PQ: u16 = 16;
/// The ITU-T H.273 transfer characteristics of ARIB STD-B67 (HLG).
const TRANSFER_HLG: u16 = 18;

/// How an image file declares that it holds high dynamic range, as [`hdr_bytes`] finds it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Hdr {
    /// An SDR base image with a gain map that recovers the HDR rendition: an ISO 21496-1 or Ultra HDR / Adobe
    /// (`hdrgm`) JPEG.
    GainMap,
    /// Pixels encoded with the PQ transfer (SMPTE ST 2084), declared by CICP or an ICC profile's `cicp` tag.
    Pq,
    /// Pixels encoded with the HLG transfer (ARIB STD-B67), declared the same way.
    Hlg,
}

impl Hdr {
    /// A short English name: `gain map`, `PQ`, `HLG`.
    pub fn name(self) -> &'static str {
        match self {
            Self::GainMap => "gain map",
            Self::Pq => "PQ",
            Self::Hlg => "HLG",
        }
    }
}

/// Reports whether the encoded image in `bytes` declares HDR, **without decoding the pixels**, guessing the format from
/// its magic bytes; `None` for an SDR file, an unrecognised format, or anything that cannot be read that far.
///
/// | Found | Gives |
/// |---|---|
/// | JPEG: an `APP2` ISO 21496-1 marker (`urn:iso:std:iso:ts:21496:-1`), an `APP2` MPF entry of type Gain Map, or `APP1` XMP declaring `http://ns.adobe.com/hdr-gain-map/1.0/` | [`Hdr::GainMap`] |
/// | AVIF/HEIF `nclx` or PNG `cICP` with transfer 16 / 18 | [`Hdr::Pq`] / [`Hdr::Hlg`] |
/// | An embedded ICC profile whose `cicp` tag has transfer 16 / 18, in any format | [`Hdr::Pq`] / [`Hdr::Hlg`] |
///
/// A gain map wins over a transfer: its base image is SDR. Only the primary image's segments ahead of its first scan
/// are read in a JPEG, so the gain map image itself is never parsed. Not detected: AVIF/HEIF gain maps (`tmap` items),
/// and Apple's earlier private gain maps, which carry no ISO marker, Gain Map type or `hdrgm` namespace.
///
/// **Infallible and bounded**, like [`metadata_bytes`](super::metadata_bytes): every length is checked against the bytes
/// present, and no input panics.
///
/// ```
/// use image::{DynamicImage, RgbImage};
/// use rust_sak::image::{ImageFormat, encode_writer, hdr_bytes};
///
/// let mut jpeg = Vec::new();
/// encode_writer(&DynamicImage::ImageRgb8(RgbImage::new(2, 2)), &mut jpeg, ImageFormat::Jpeg, None).unwrap();
///
/// assert_eq!(hdr_bytes(&jpeg), None);
/// ```
pub fn hdr_bytes(bytes: &[u8]) -> Option<Hdr> {
    let format = ImageFormat::from_magic(bytes)?;

    if format == ImageFormat::Jpeg && blocks::jpeg::has_gain_map(bytes) {
        return Some(Hdr::GainMap);
    }

    let coded = match format {
        ImageFormat::Png => blocks::png::cicp_transfer(bytes).map(u16::from),
        ImageFormat::Avif | ImageFormat::Heif => container::coded_transfer(bytes, format),
        _ => None,
    };
    let transfer = coded.or_else(|| {
        let profile = icc_profile_bytes(bytes).ok().flatten()?;
        icc::cicp_transfer(&profile).map(u16::from)
    })?;

    match transfer {
        TRANSFER_PQ => Some(Hdr::Pq),
        TRANSFER_HLG => Some(Hdr::Hlg),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use ::image::{DynamicImage, Rgb, RgbImage};

    use super::super::{Metadata, encode_writer, encode_writer_with};
    use super::*;

    fn picture(width: u32, height: u32) -> DynamicImage {
        DynamicImage::ImageRgb8(RgbImage::from_fn(width, height, |x, y| {
            Rgb([(x * 7) as u8, (y * 5) as u8, 90])
        }))
    }

    fn encoded(format: ImageFormat) -> Vec<u8> {
        let mut bytes = Vec::new();
        encode_writer(&picture(64, 64), &mut bytes, format, None).unwrap();
        bytes
    }

    /// `jpeg` with an `APP2` segment holding `data` inserted straight after `SOI`.
    fn with_app2(jpeg: &[u8], data: &[u8]) -> Vec<u8> {
        let length = u16::try_from(data.len() + 2).unwrap().to_be_bytes();
        let mut out = jpeg[..2].to_vec();
        out.extend([0xFF, 0xE2, length[0], length[1]]);
        out.extend(data);
        out.extend(&jpeg[2..]);
        out
    }

    /// An MPF `APP2` body indexing two images, the second of type `second` (the low 24 bits of its attribute), in the
    /// byte order `big` names.
    fn mpf(second: u32, big: bool) -> Vec<u8> {
        let u16b = |v: u16| if big { v.to_be_bytes() } else { v.to_le_bytes() };
        let u32b = |v: u32| if big { v.to_be_bytes() } else { v.to_le_bytes() };
        let mut tiff = Vec::new();
        tiff.extend(if big { *b"MM\0\x2a" } else { *b"II\x2a\0" });
        tiff.extend(u32b(8));
        // Three entries: MPFVersion, NumberOfImages, MPEntry; then the next-IFD offset; then the MP Entry table.
        tiff.extend(u16b(3));
        tiff.extend(u16b(0xB000));
        tiff.extend(u16b(7));
        tiff.extend(u32b(4));
        tiff.extend(*b"0100");
        tiff.extend(u16b(0xB001));
        tiff.extend(u16b(4));
        tiff.extend(u32b(1));
        tiff.extend(u32b(2));
        tiff.extend(u16b(0xB002));
        tiff.extend(u16b(7));
        tiff.extend(u32b(32));
        let table = u32::try_from(tiff.len() + 4 + 4).unwrap();
        tiff.extend(u32b(table));
        tiff.extend(u32b(0));
        // The primary image (Baseline MP Primary, representative) and the second.
        tiff.extend(u32b(0x2003_0000));
        tiff.extend(u32b(1000));
        tiff.extend(u32b(0));
        tiff.extend(u32b(0));
        tiff.extend(u32b(second));
        tiff.extend(u32b(500));
        tiff.extend(u32b(1000));
        tiff.extend(u32b(0));
        let mut out = b"MPF\0".to_vec();
        out.extend(tiff);
        out
    }

    /// A minimal ICC v4 profile whose only tag is a `cicp` with `transfer`.
    fn icc_with_cicp(transfer: u8) -> Vec<u8> {
        let mut profile = vec![0u8; 128];
        profile[16..20].copy_from_slice(b"RGB ");
        profile[36..40].copy_from_slice(b"acsp");
        profile.extend(1u32.to_be_bytes());
        profile.extend(*b"cicp");
        profile.extend(144u32.to_be_bytes());
        profile.extend(12u32.to_be_bytes());
        profile.extend(*b"cicp");
        profile.extend([0, 0, 0, 0, 9, transfer, 0, 1]);
        let size = u32::try_from(profile.len()).unwrap().to_be_bytes();
        profile[0..4].copy_from_slice(&size);
        profile
    }

    /// `png` with a `cICP` chunk of `transfer` inserted after `IHDR`.
    fn png_with_cicp(png: &[u8], transfer: u8) -> Vec<u8> {
        let ihdr_end = 8 + 4 + 4 + 13 + 4;
        let data = [9, transfer, 0, 1];
        let mut hasher = crc32fast::Hasher::new();
        hasher.update(b"cICP");
        hasher.update(&data);
        let mut out = png[..ihdr_end].to_vec();
        out.extend(4u32.to_be_bytes());
        out.extend(*b"cICP");
        out.extend(data);
        out.extend(hasher.finalize().to_be_bytes());
        out.extend(&png[ihdr_end..]);
        out
    }

    #[test]
    fn an_sdr_file_of_every_format_is_not_hdr() {
        for format in [
            ImageFormat::Bmp,
            ImageFormat::Gif,
            ImageFormat::Jpeg,
            ImageFormat::Png,
            ImageFormat::Tiff,
            ImageFormat::WebP,
            ImageFormat::Avif,
            ImageFormat::Heif,
        ] {
            assert_eq!(hdr_bytes(&encoded(format)), None, "{format:?}");
        }
        assert_eq!(hdr_bytes(b"not an image"), None);
        assert_eq!(hdr_bytes(&[]), None);
    }

    #[test]
    fn a_jpeg_with_an_iso_21496_marker_has_a_gain_map() {
        let jpeg = with_app2(&encoded(ImageFormat::Jpeg), b"urn:iso:std:iso:ts:21496:-1\0\0\0\0\0");

        assert_eq!(hdr_bytes(&jpeg), Some(Hdr::GainMap));
    }

    #[test]
    fn a_jpeg_whose_mpf_indexes_a_gain_map_has_one_in_either_byte_order() {
        let jpeg = encoded(ImageFormat::Jpeg);

        for big in [true, false] {
            assert_eq!(
                hdr_bytes(&with_app2(&jpeg, &mpf(0x05_0000, big))),
                Some(Hdr::GainMap),
                "big: {big}"
            );
            // A large thumbnail (Multi-Frame Preview, 0x010001), as cameras write, is not a gain map; the flags in
            // the attribute's top byte do not count.
            assert_eq!(hdr_bytes(&with_app2(&jpeg, &mpf(0x0001_0001, big))), None, "big: {big}");
            assert_eq!(
                hdr_bytes(&with_app2(&jpeg, &mpf(0x8005_0000, big))),
                Some(Hdr::GainMap),
                "big: {big}"
            );
        }
    }

    #[test]
    fn a_jpeg_whose_xmp_declares_hdrgm_has_a_gain_map() {
        let packet = br#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description xmlns:hdrgm="http://ns.adobe.com/hdr-gain-map/1.0/" hdrgm:Version="1.0"/></rdf:RDF></x:xmpmeta>"#;
        let mut metadata = Metadata {
            xmp: Some(packet.to_vec()),
            ..Metadata::default()
        };
        let mut jpeg = Vec::new();
        encode_writer_with(&picture(16, 16), &mut jpeg, ImageFormat::Jpeg, None, &metadata).unwrap();

        assert_eq!(hdr_bytes(&jpeg), Some(Hdr::GainMap));

        // The same packet without the namespace is an ordinary one.
        metadata.xmp = Some(
            String::from_utf8_lossy(packet)
                .replace("hdr-gain-map", "other")
                .into_bytes(),
        );
        let mut plain = Vec::new();
        encode_writer_with(&picture(16, 16), &mut plain, ImageFormat::Jpeg, None, &metadata).unwrap();
        assert_eq!(hdr_bytes(&plain), None);
    }

    #[test]
    fn a_png_cicp_chunk_names_pq_and_hlg() {
        let png = encoded(ImageFormat::Png);

        assert_eq!(hdr_bytes(&png_with_cicp(&png, 16)), Some(Hdr::Pq));
        assert_eq!(hdr_bytes(&png_with_cicp(&png, 18)), Some(Hdr::Hlg));
        assert_eq!(hdr_bytes(&png_with_cicp(&png, 13)), None);
        // A chunk whose CRC does not match is not believed.
        let mut corrupt = png_with_cicp(&png, 16);
        let crc = 8 + 25 + 8 + 4;
        corrupt[crc] ^= 0xFF;
        assert_eq!(hdr_bytes(&corrupt), None);
    }

    #[test]
    fn an_avif_nclx_names_pq_and_hlg() {
        let avif = encoded(ImageFormat::Avif);
        let with_transfer = |transfer: u16| {
            let at = avif
                .windows(8)
                .position(|w| w == b"colrnclx")
                .expect("an nclx colr box")
                + 8;
            let mut out = avif.clone();
            out[at..at + 2].copy_from_slice(&9u16.to_be_bytes());
            out[at + 2..at + 4].copy_from_slice(&transfer.to_be_bytes());
            out
        };

        assert_eq!(hdr_bytes(&with_transfer(16)), Some(Hdr::Pq));
        assert_eq!(hdr_bytes(&with_transfer(18)), Some(Hdr::Hlg));
        assert_eq!(hdr_bytes(&with_transfer(13)), None);
    }

    #[test]
    fn an_icc_profile_with_a_cicp_tag_names_pq_and_hlg() {
        for (transfer, expected) in [(16, Some(Hdr::Pq)), (18, Some(Hdr::Hlg)), (13, None)] {
            let metadata = Metadata {
                icc_profile: Some(icc_with_cicp(transfer)),
                ..Metadata::default()
            };
            let mut png = Vec::new();
            encode_writer_with(&picture(8, 8), &mut png, ImageFormat::Png, None, &metadata).unwrap();

            assert_eq!(hdr_bytes(&png), expected, "transfer {transfer}");
        }
    }

    #[test]
    fn no_truncation_or_byte_flip_of_a_gain_map_jpeg_panics() {
        let jpeg = with_app2(
            &with_app2(&encoded(ImageFormat::Jpeg), &mpf(0x05_0000, true)),
            b"urn:iso:std:iso:ts:21496:-1\0\0\0\0\0",
        );
        let header = jpeg.len().min(400);

        for cut in 0..header {
            let _ = hdr_bytes(&jpeg[..cut]);
        }
        for at in 0..header {
            for flip in [0x01, 0x80, 0xFF] {
                let mut flipped = jpeg.clone();
                flipped[at] ^= flip;
                let _ = hdr_bytes(&flipped);
            }
        }
        // An MPF whose IFD offset and entry length point far outside the segment.
        let mut hostile = mpf(0x05_0000, true);
        hostile[8..12].copy_from_slice(&u32::MAX.to_be_bytes());
        assert_eq!(hdr_bytes(&with_app2(&encoded(ImageFormat::Jpeg), &hostile)), None);
    }
}
