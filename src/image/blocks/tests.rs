//! The readers and splices against files another writer produced, and their robustness against hostile ones.
//!
//! # The fixtures
//!
//! Generated, not taken from a camera, so they can be committed. Each started as a 16×16 gradient (`magick -size 16x16
//! gradient:red-blue -strip`, then `cwebp -q 80` for the WebP), and was given its metadata by ExifTool 13.55, an
//! independent writer, from `fixtures/exiftool-args.txt`:
//!
//! - `meta.jpg` — `exiftool -@ exiftool-args.txt -IPTC:Keywords=hawk -IPTC:Keywords=bird -IPTC:By-line=Fixture`:
//!   `APP1` EXIF (with a GPS IFD), `APP13` Photoshop/IPTC and `APP1` XMP.
//! - `meta.png` — `eXIf` and an uncompressed XMP `iTXt`; `meta-z.png` the same written with `exiftool -z`, so its
//!   `iTXt` is compressed.
//! - `meta.webp` — `VP8X` with `EXIF` and `XMP ` chunks.
//! - `extended.jpg` — `meta.jpg`'s arguments plus a 70,000-character `XMP-dc:Description`, which ExifTool moves into
//!   two extended XMP chunks under a GUID the standard packet names.
//! - `truncated-exif.jpg` — `meta.jpg` with its EXIF segment cut to the TIFF header and IFD0's entry count (ExifTool:
//!   "Bad IFD0 directory").
//!
//! The expected blocks beside them are ExifTool's own reading: `exiftool -b -EXIF` (`.exif`) and `exiftool -b -XMP`
//! (`.xmp`); `.iptc` is the `APP13` segment's data after `Photoshop 3.0\0`, cut out by offset.

use ::image::{DynamicImage, Rgb, RgbImage};

use super::super::{
    EncodeOptions, ImageFormat, Metadata, encode_writer, encode_writer_with, icc_profile_bytes, metadata_bytes,
};
use super::{MAX_BLOCK_LEN, jpeg, xmp};

const META_JPG: &[u8] = include_bytes!("fixtures/meta.jpg");
const META_PNG: &[u8] = include_bytes!("fixtures/meta.png");
const META_Z_PNG: &[u8] = include_bytes!("fixtures/meta-z.png");
const META_WEBP: &[u8] = include_bytes!("fixtures/meta.webp");
const EXTENDED_JPG: &[u8] = include_bytes!("fixtures/extended.jpg");
const TRUNCATED_EXIF_JPG: &[u8] = include_bytes!("fixtures/truncated-exif.jpg");

const FIXTURES: [&[u8]; 6] = [
    META_JPG,
    META_PNG,
    META_Z_PNG,
    META_WEBP,
    EXTENDED_JPG,
    TRUNCATED_EXIF_JPG,
];

fn sample() -> DynamicImage {
    let mut image = RgbImage::new(16, 8);
    for (x, y, pixel) in image.enumerate_pixels_mut() {
        *pixel = Rgb([(x * 16) as u8, (y * 32) as u8, 128]);
    }
    DynamicImage::ImageRgb8(image)
}

/// The fixtures' own blocks, as a [`Metadata`] to embed.
fn fixture_blocks() -> Metadata {
    Metadata {
        exif: Some(include_bytes!("fixtures/meta.jpg.exif").to_vec()),
        xmp: Some(include_bytes!("fixtures/meta.jpg.xmp").to_vec()),
        iptc: Some(include_bytes!("fixtures/meta.jpg.iptc").to_vec()),
        ..Metadata::default()
    }
}

/// A stand-in ICC profile with a valid header.
fn profile() -> Vec<u8> {
    let mut profile = vec![0_u8; 300];
    profile[0..4].copy_from_slice(&300_u32.to_be_bytes());
    profile[16..20].copy_from_slice(b"RGB ");
    profile[36..40].copy_from_slice(b"acsp");
    profile
}

fn encode_with(format: ImageFormat, metadata: &Metadata) -> (Vec<u8>, super::super::Embedded) {
    let mut bytes = Vec::new();
    let embedded = encode_writer_with(&sample(), &mut bytes, format, None, metadata).unwrap();
    (bytes, embedded)
}

// ── Reading what ExifTool wrote ─────────────────────────────────────────────────────────────────────────────────

#[test]
fn a_jpeg_gives_its_exif_xmp_and_iptc_as_exiftool_reads_them() {
    let metadata = metadata_bytes(META_JPG);
    assert_eq!(
        metadata.exif.as_deref(),
        Some(&include_bytes!("fixtures/meta.jpg.exif")[..])
    );
    assert_eq!(
        metadata.xmp.as_deref(),
        Some(&include_bytes!("fixtures/meta.jpg.xmp")[..])
    );
    assert_eq!(
        metadata.iptc.as_deref(),
        Some(&include_bytes!("fixtures/meta.jpg.iptc")[..])
    );
    assert_eq!(metadata.icc_profile, None);
}

#[test]
fn a_png_gives_its_exif_and_xmp_compressed_or_not() {
    for (png, exif, xmp) in [
        (
            META_PNG,
            &include_bytes!("fixtures/meta.png.exif")[..],
            &include_bytes!("fixtures/meta.png.xmp")[..],
        ),
        (
            META_Z_PNG,
            &include_bytes!("fixtures/meta-z.png.exif")[..],
            &include_bytes!("fixtures/meta-z.png.xmp")[..],
        ),
    ] {
        let metadata = metadata_bytes(png);
        assert_eq!(metadata.exif.as_deref(), Some(exif));
        assert_eq!(metadata.xmp.as_deref(), Some(xmp));
        assert_eq!(metadata.iptc, None);
    }
}

#[test]
fn a_webp_gives_its_exif_and_xmp() {
    let metadata = metadata_bytes(META_WEBP);
    assert_eq!(
        metadata.exif.as_deref(),
        Some(&include_bytes!("fixtures/meta.webp.exif")[..])
    );
    assert_eq!(
        metadata.xmp.as_deref(),
        Some(&include_bytes!("fixtures/meta.webp.xmp")[..])
    );
    assert_eq!(metadata.iptc, None);
}

#[test]
fn extended_xmp_is_joined_into_one_packet() {
    let metadata = metadata_bytes(EXTENDED_JPG);
    let packet = String::from_utf8(metadata.xmp.expect("the XMP")).unwrap();

    // The standard part's properties and the extended part's 70,000-character description, in one `rdf:RDF`.
    assert!(packet.contains("<xmp:Rating>3</xmp:Rating>"), "{packet}");
    assert!(packet.contains(&"x".repeat(70_000)));
    assert!(packet.contains("<dc:description>"));
    assert_eq!(packet.matches("<rdf:RDF").count(), 1);
    assert_eq!(packet.matches("</rdf:RDF>").count(), 1);
    // The reference to the extended part is gone with the split it described.
    assert!(!packet.contains("HasExtendedXMP"));
    assert!(xmp::plausible(packet.as_bytes()));
}

#[test]
fn an_extended_part_with_a_chunk_missing_leaves_the_standard_packet() {
    // Drop the second extended chunk: the packet cannot be reassembled, and the standard part is all that is kept.
    let mut segments = Vec::new();
    let mut at = 2;
    while EXTENDED_JPG[at + 1] != 0xDA {
        let length = usize::from(u16::from_be_bytes([EXTENDED_JPG[at + 2], EXTENDED_JPG[at + 3]]));
        segments.push((at, at + 2 + length));
        at += 2 + length;
    }
    let extended: Vec<_> = segments
        .iter()
        .filter(|(start, _)| EXTENDED_JPG[start + 4..].starts_with(b"http://ns.adobe.com/xmp/extension/"))
        .collect();
    assert_eq!(extended.len(), 2);
    let (cut_from, cut_to) = *extended[1];
    let damaged = [&EXTENDED_JPG[..cut_from], &EXTENDED_JPG[cut_to..]].concat();

    let packet = String::from_utf8(metadata_bytes(&damaged).xmp.expect("the standard XMP")).unwrap();
    assert!(packet.contains("<xmp:Rating>3</xmp:Rating>"));
    assert!(!packet.contains("xxxxxxxxxx"));
}

#[test]
fn a_truncated_exif_block_is_left_out_and_the_rest_kept() {
    let metadata = metadata_bytes(TRUNCATED_EXIF_JPG);
    assert_eq!(metadata.exif, None);
    assert_eq!(
        metadata.xmp.as_deref(),
        Some(&include_bytes!("fixtures/meta.jpg.xmp")[..])
    );
    assert_eq!(
        metadata.iptc.as_deref(),
        Some(&include_bytes!("fixtures/meta.jpg.iptc")[..])
    );
}

#[test]
fn a_file_without_metadata_or_of_another_format_gives_none() {
    for format in ImageFormat::ALL {
        if matches!(format, ImageFormat::Avif | ImageFormat::Heif) {
            continue;
        }
        let mut bytes = Vec::new();
        encode_writer(&sample(), &mut bytes, format, None).unwrap();
        assert_eq!(metadata_bytes(&bytes), Metadata::default(), "{format:?}");
    }
    assert_eq!(metadata_bytes(b"not an image"), Metadata::default());
    assert_eq!(metadata_bytes(b""), Metadata::default());
}

#[test]
fn a_png_block_failing_its_crc_is_left_out() {
    let mut damaged = META_PNG.to_vec();
    let exif_at = damaged.windows(4).position(|window| window == b"eXIf").unwrap();
    // One byte of the chunk's data, so the walk carries on and only the CRC disagrees.
    damaged[exif_at + 4 + 20] ^= 0x01;
    let metadata = metadata_bytes(&damaged);
    assert_eq!(metadata.exif, None);
    assert!(metadata.xmp.is_some());
}

// ── Hostile input ───────────────────────────────────────────────────────────────────────────────────────────────

/// Every prefix of the small fixtures, and a sample of the large one's, read without panicking.
#[test]
fn truncations_never_panic() {
    for fixture in FIXTURES {
        let step = if fixture.len() > 8192 { 97 } else { 1 };
        for len in (0..fixture.len()).step_by(step) {
            let _ = metadata_bytes(&fixture[..len]);
        }
        // Near the front, where the segments are, every length.
        for len in 0..fixture.len().min(4096) {
            let _ = metadata_bytes(&fixture[..len]);
        }
    }
}

/// Every byte of the small fixtures flipped in turn (and a sample of the large one's), then a few overwritten with the
/// values most likely to break a length: 0x00 and 0xFF.
#[test]
fn byte_flips_never_panic() {
    for fixture in FIXTURES {
        let mut bytes = fixture.to_vec();
        let step = if fixture.len() > 8192 { 31 } else { 1 };
        for at in (0..bytes.len()).step_by(step) {
            for value in [bytes[at] ^ 0xFF, 0x00, 0xFF, bytes[at] ^ 0x01] {
                let original = std::mem::replace(&mut bytes[at], value);
                let _ = metadata_bytes(&bytes);
                bytes[at] = original;
            }
        }
    }
}

#[test]
fn a_compressed_xmp_that_inflates_past_the_bound_is_left_out() {
    // A zlib stream of zeros that inflates to just over the bound, in an `iTXt` with a correct CRC.
    let inflated = vec![b' '; MAX_BLOCK_LEN + 1];
    let deflated = miniz_oxide::deflate::compress_to_vec_zlib(&inflated, 9);
    let mut text = b"XML:com.adobe.xmp\0\x01\0\0\0".to_vec();
    text.extend_from_slice(&deflated);

    let (png, _) = encode_with(ImageFormat::Png, &Metadata::default());
    let idat = png.windows(4).position(|window| window == b"IDAT").unwrap() - 4;
    let mut chunk = (u32::try_from(text.len()).unwrap()).to_be_bytes().to_vec();
    chunk.extend_from_slice(b"iTXt");
    chunk.extend_from_slice(&text);
    let mut crc = crc32fast::Hasher::new();
    crc.update(b"iTXt");
    crc.update(&text);
    chunk.extend_from_slice(&crc.finalize().to_be_bytes());
    let hostile = [&png[..idat], &chunk, &png[idat..]].concat();

    assert_eq!(metadata_bytes(&hostile).xmp, None);
}

// ── Writing ─────────────────────────────────────────────────────────────────────────────────────────────────────

#[test]
fn jpeg_png_and_webp_round_trip_the_blocks_they_can_hold() {
    let metadata = fixture_blocks();
    for format in [ImageFormat::Jpeg, ImageFormat::Png, ImageFormat::WebP] {
        let (bytes, embedded) = encode_with(format, &metadata);
        let read = metadata_bytes(&bytes);

        assert!(embedded.exif && embedded.xmp, "{format:?}");
        assert_eq!(read.exif, metadata.exif, "{format:?}");
        assert_eq!(read.xmp, metadata.xmp, "{format:?}");
        // IPTC has a place only in JPEG.
        assert_eq!(embedded.iptc, format == ImageFormat::Jpeg, "{format:?}");
        let iptc = if format == ImageFormat::Jpeg {
            metadata.iptc.clone()
        } else {
            None
        };
        assert_eq!(read.iptc, iptc, "{format:?}");
        // The pixels are still there, and the fixture's Orientation (6) took effect where the decode applies it: JPEG
        // and PNG come back upright, WebP as stored.
        let decoded = super::super::decode_bytes(&bytes).unwrap();
        let upright = if format == ImageFormat::WebP { (16, 8) } else { (8, 16) };
        assert_eq!((decoded.width(), decoded.height()), upright, "{format:?}");
    }
}

#[test]
fn an_icc_profile_and_the_blocks_are_written_together() {
    let mut metadata = fixture_blocks();
    metadata.icc_profile = Some(profile());
    for format in [ImageFormat::Jpeg, ImageFormat::Png, ImageFormat::WebP] {
        let (bytes, embedded) = encode_with(format, &metadata);
        assert!(embedded.icc_profile && embedded.exif && embedded.xmp, "{format:?}");
        let read = metadata_bytes(&bytes);
        assert_eq!(read.icc_profile, metadata.icc_profile, "{format:?}");
        assert_eq!(icc_profile_bytes(&bytes).unwrap(), metadata.icc_profile, "{format:?}");
        assert_eq!(
            (read.exif, read.xmp),
            (metadata.exif.clone(), metadata.xmp.clone()),
            "{format:?}"
        );
    }
}

#[test]
fn a_webp_with_alpha_and_a_lossless_one_take_the_blocks_too() {
    let metadata = fixture_blocks();
    // Alpha that varies, so the encoder cannot drop it as opaque.
    let mut rgba = sample().to_rgba8();
    for (x, _, pixel) in rgba.enumerate_pixels_mut() {
        pixel.0[3] = (x * 16) as u8;
    }
    let rgba = DynamicImage::ImageRgba8(rgba);
    for (image, lossless) in [(rgba.clone(), false), (rgba, true), (sample(), true)] {
        let options = EncodeOptions::WebP {
            quality: 80,
            quality_alpha: 100,
            compression: 4,
            lossless,
            threads: false,
        };
        let mut bytes = Vec::new();
        let embedded = encode_writer_with(&image, &mut bytes, ImageFormat::WebP, Some(options), &metadata).unwrap();
        assert!(embedded.exif && embedded.xmp && !embedded.iptc);
        let read = metadata_bytes(&bytes);
        assert_eq!((read.exif, read.xmp), (metadata.exif.clone(), metadata.xmp.clone()));
        let decoded = super::super::decode_bytes(&bytes).unwrap();
        assert_eq!(decoded.color().has_alpha(), image.color().has_alpha());
    }
}

#[test]
fn the_other_formats_write_exactly_what_they_wrote_without_the_blocks() {
    let metadata = fixture_blocks();
    for format in [
        ImageFormat::Bmp,
        ImageFormat::Gif,
        ImageFormat::Tiff,
        ImageFormat::Avif,
        ImageFormat::Heif,
    ] {
        let image = if matches!(format, ImageFormat::Avif | ImageFormat::Heif) {
            DynamicImage::ImageRgb8(RgbImage::from_fn(64, 64, |x, y| Rgb([x as u8 * 4, y as u8 * 4, 99])))
        } else {
            sample()
        };
        let mut plain = Vec::new();
        encode_writer(&image, &mut plain, format, None).unwrap();
        let mut with = Vec::new();
        let embedded = encode_writer_with(&image, &mut with, format, None, &metadata).unwrap();

        assert!(!embedded.exif && !embedded.xmp && !embedded.iptc, "{format:?}");
        assert_eq!(with, plain, "{format:?}");
    }
}

#[test]
fn nothing_to_splice_writes_the_same_bytes_as_before() {
    for format in [ImageFormat::Jpeg, ImageFormat::Png, ImageFormat::WebP] {
        let mut plain = Vec::new();
        encode_writer(&sample(), &mut plain, format, None).unwrap();
        let (with, embedded) = encode_with(format, &Metadata::default());
        assert_eq!(with, plain, "{format:?}");
        assert_eq!(embedded, super::super::Embedded::default());
    }
}

#[test]
fn a_jpeg_keeps_jfif_first_and_the_blocks_ahead_of_the_image() {
    let (bytes, _) = encode_with(ImageFormat::Jpeg, &fixture_blocks());
    let markers: Vec<u8> = {
        let mut markers = Vec::new();
        let mut at = 2;
        while bytes[at + 1] != 0xDA {
            markers.push(bytes[at + 1]);
            at += 2 + usize::from(u16::from_be_bytes([bytes[at + 2], bytes[at + 3]]));
        }
        markers
    };
    assert_eq!(&markers[..4], [0xE0, 0xE1, 0xE1, 0xED]);
}

#[test]
fn an_exif_block_too_large_for_a_jpeg_is_left_out_of_it_only() {
    let mut metadata = Metadata::default();
    let mut exif = include_bytes!("fixtures/meta.jpg.exif").to_vec();
    exif.resize(jpeg::MAX_EXIF_LEN + 1, 0);
    metadata.exif = Some(exif.clone());

    let (jpeg, embedded) = encode_with(ImageFormat::Jpeg, &metadata);
    assert!(!embedded.exif);
    assert_eq!(metadata_bytes(&jpeg).exif, None);

    let (png, embedded) = encode_with(ImageFormat::Png, &metadata);
    assert!(embedded.exif);
    assert_eq!(metadata_bytes(&png).exif, Some(exif));
}

#[test]
fn a_large_xmp_packet_is_split_for_a_jpeg_and_joined_again() {
    let packet = metadata_bytes(EXTENDED_JPG).xmp.unwrap();
    assert!(packet.len() > xmp::MAX_STANDARD_LEN);
    let mut metadata = Metadata {
        xmp: Some(packet.clone()),
        ..Metadata::default()
    };

    let (jpeg, embedded) = encode_with(ImageFormat::Jpeg, &metadata);
    assert!(embedded.xmp);
    let once = metadata_bytes(&jpeg).xmp.unwrap();
    let text = String::from_utf8(once.clone()).unwrap();
    assert!(text.contains(&"x".repeat(70_000)) && text.contains("<xmp:Rating>3</xmp:Rating>"));

    // Stable from then on: the split and the join undo each other.
    metadata.xmp = Some(once.clone());
    let (again, _) = encode_with(ImageFormat::Jpeg, &metadata);
    assert_eq!(metadata_bytes(&again).xmp, Some(once));

    // PNG and WebP hold it whole.
    for format in [ImageFormat::Png, ImageFormat::WebP] {
        metadata.xmp = Some(packet.clone());
        let (bytes, _) = encode_with(format, &metadata);
        assert_eq!(metadata_bytes(&bytes).xmp, Some(packet.clone()), "{format:?}");
    }
}

#[test]
fn a_large_photoshop_block_is_split_across_segments_and_joined_again() {
    let mut block = b"8BIM".to_vec();
    block.extend((0..150_000_u32).map(|n| (n % 251) as u8));
    let metadata = Metadata {
        iptc: Some(block.clone()),
        ..Metadata::default()
    };

    let (jpeg, embedded) = encode_with(ImageFormat::Jpeg, &metadata);
    assert!(embedded.iptc);
    assert_eq!(metadata_bytes(&jpeg).iptc, Some(block));
}
