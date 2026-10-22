//! Reading and writing a JPEG's gain-map container: the ISO 21496-1 `APP2` payload, the MP Index and the gain-map image
//! after the primary's `EOI`.
//!
//! # The fixtures
//!
//! Built here, shaped like a Lightroom Ultra HDR export (`18-1603-hawk-bird-mt_auburn-cemetery.jpg`, the photograph the
//! feature was written for): a primary with `hdrgm:Version` XMP, an MP Index and the ISO version marker, then a gain-map
//! JPEG holding the hawk's own gain-map XMP (`fixtures/gain-map.xmp`, `exiftool -b -MPImage2` then its `APP1` cut out)
//! and its 141-byte ISO 21496-1 metadata (`fixtures/gain-map.iso`, its `APP2` after the URN). The pixels are generated.

use ::image::{DynamicImage, GrayImage, Luma};

use super::super::{Embedded, ImageFormat, Metadata, decode_bytes, encode_writer, encode_writer_with, metadata_bytes};
use super::super::{Hdr, hdr_bytes};
use super::mpf;

const GAIN_MAP_XMP: &[u8] = include_bytes!("fixtures/gain-map.xmp");
const GAIN_MAP_ISO: &[u8] = include_bytes!("fixtures/gain-map.iso");
const ISO_URN: &[u8] = b"urn:iso:std:iso:ts:21496:-1\0";
const PRIMARY_XMP: &[u8] = br#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description rdf:about="" xmlns:hdrgm="http://ns.adobe.com/hdr-gain-map/1.0/" hdrgm:Version="1.0"/></rdf:RDF></x:xmpmeta>"#;
/// A primary's ISO 21496-1 payload: minimum and writer version.
const VERSION: &[u8] = &[0, 0, 0, 0];

fn primary_pixels() -> DynamicImage {
    DynamicImage::ImageRgb8(::image::RgbImage::from_fn(32, 24, |x, y| {
        ::image::Rgb([(x * 8) as u8, (y * 10) as u8, 120])
    }))
}

/// A gain-map JPEG as Lightroom writes one: its own `hdrgm` XMP and ISO 21496-1 metadata, no ICC, no EXIF.
fn gain_map_jpeg() -> Vec<u8> {
    let pixels = DynamicImage::ImageLuma8(GrayImage::from_fn(16, 12, |x, y| Luma([(x * 9 + y * 5) as u8])));
    let mut jpeg = Vec::new();
    let metadata = Metadata {
        xmp: Some(GAIN_MAP_XMP.to_vec()),
        ..Metadata::default()
    };
    encode_writer_with(&pixels, &mut jpeg, ImageFormat::Jpeg, None, &metadata).unwrap();
    with_segment(&jpeg, 0xE2, &[ISO_URN, GAIN_MAP_ISO].concat())
}

/// `jpeg` with a segment of `marker` holding `data` inserted straight after `SOI`.
fn with_segment(jpeg: &[u8], marker: u8, data: &[u8]) -> Vec<u8> {
    let length = u16::try_from(data.len() + 2).unwrap().to_be_bytes();
    let mut out = jpeg[..2].to_vec();
    out.extend([0xFF, marker, length[0], length[1]]);
    out.extend(data);
    out.extend(&jpeg[2..]);
    out
}

/// An MP Index `APP2` body of `entries` (`(attribute, size, offset)`) in the byte order `big` names, written by hand
/// rather than by [`mpf::index`] so the reader is not tested against its own writer.
fn index(entries: &[(u32, u32, u32)], big: bool) -> Vec<u8> {
    let u16b = |v: u16| if big { v.to_be_bytes() } else { v.to_le_bytes() };
    let u32b = |v: u32| if big { v.to_be_bytes() } else { v.to_le_bytes() };
    let count = u32::try_from(entries.len()).unwrap();
    let mut tiff = Vec::new();
    tiff.extend(if big { *b"MM\0\x2a" } else { *b"II\x2a\0" });
    tiff.extend(u32b(8));
    tiff.extend(u16b(3));
    for (tag, kind, n, value) in [
        (0xB000, 7, 4, u32::from_be_bytes(*b"0100")),
        (0xB001, 4, 1, count),
        (0xB002, 7, 16 * count, 50),
    ] {
        tiff.extend(u16b(tag));
        tiff.extend(u16b(kind));
        tiff.extend(u32b(n));
        if tag == 0xB000 {
            tiff.extend(value.to_be_bytes());
        } else {
            tiff.extend(u32b(value));
        }
    }
    tiff.extend([0; 4]);
    for &(attribute, size, offset) in entries {
        tiff.extend(u32b(attribute));
        tiff.extend(u32b(size));
        tiff.extend(u32b(offset));
        tiff.extend([0; 4]);
    }
    [b"MPF\0".as_slice(), &tiff].concat()
}

/// How a fixture is shaped.
#[derive(Clone, Copy)]
struct Shape {
    big: bool,
    /// The primary's recorded size short of the truth, as ExifTool leaves it after growing the metadata.
    stale: bool,
    /// The secondary entry's attribute: Gain Map, or Android's 0.
    second: u32,
    /// Whether the primary declares the gain map (ISO marker and `hdrgm` XMP).
    declared: bool,
}

const HAWK: Shape = Shape {
    big: true,
    stale: true,
    second: 0x05_0000,
    declared: true,
};

/// A gain-map JPEG of `shape`, and the gain-map image it carries.
fn ultra_hdr(shape: Shape) -> (Vec<u8>, Vec<u8>) {
    assemble(shape, &[(shape.second, true)])
}

/// A gain-map JPEG of `shape` whose MP Index lists `secondaries` after the primary — `(attribute, whether it points at
/// the gain map)`; one that does not points one byte past it, at no `SOI` — and the gain-map image appended.
fn assemble(shape: Shape, secondaries: &[(u32, bool)]) -> (Vec<u8>, Vec<u8>) {
    let mut metadata = Metadata::default();
    if shape.declared {
        metadata.xmp = Some(PRIMARY_XMP.to_vec());
    }
    let mut primary = Vec::new();
    encode_writer_with(&primary_pixels(), &mut primary, ImageFormat::Jpeg, None, &metadata).unwrap();
    if shape.declared {
        primary = with_segment(&primary, 0xE2, &[ISO_URN, VERSION].concat());
    }
    let gain_map = gain_map_jpeg();

    let entries = |primary_len: u32, recorded: u32| {
        let tiff_at = 2 + 4 + 4;
        let size = u32::try_from(gain_map.len()).unwrap();
        let mut entries = vec![(0x2003_0000, recorded, 0)];
        for &(attribute, points) in secondaries {
            let offset = primary_len - tiff_at;
            entries.push(if points {
                (attribute, size, offset)
            } else {
                (attribute, 10, offset + 1)
            });
        }
        index(&entries, shape.big)
    };
    // The index goes in first with placeholder values, which keep its size, then is rewritten with the true ones.
    let primary_len = u32::try_from(with_segment(&primary, 0xE2, &entries(10, 0)).len()).unwrap();
    let recorded = if shape.stale { primary_len - 100 } else { primary_len };
    let mut file = with_segment(&primary, 0xE2, &entries(primary_len, recorded));
    assert_eq!(file.len(), primary_len as usize);
    file.extend(&gain_map);
    (file, gain_map)
}

/// The offset of `needle` in `haystack`.
fn find(haystack: &[u8], needle: &[u8]) -> usize {
    haystack.windows(needle.len()).position(|w| w == needle).unwrap()
}

// ── Reading ─────────────────────────────────────────────────────────────────────────────────────────────────────

#[test]
fn a_gain_map_jpeg_gives_its_gain_map_and_iso_payload_in_either_byte_order() {
    for big in [true, false] {
        let (file, gain_map) = ultra_hdr(Shape { big, ..HAWK });

        let read = metadata_bytes(&file);

        assert_eq!(read.gain_map.as_deref(), Some(&gain_map[..]), "big: {big}");
        assert_eq!(read.iso_gain_map.as_deref(), Some(VERSION), "big: {big}");
        assert_eq!(read.xmp.as_deref(), Some(PRIMARY_XMP), "big: {big}");
        // And the gain map's own metadata reads out of it in turn.
        let inner = metadata_bytes(read.gain_map.as_deref().unwrap());
        assert_eq!(inner.xmp.as_deref(), Some(GAIN_MAP_XMP));
        assert_eq!(inner.iso_gain_map.as_deref(), Some(GAIN_MAP_ISO));
        assert_eq!(inner.gain_map, None);
        assert_eq!(decode_bytes(read.gain_map.as_deref().unwrap()).unwrap().width(), 16);
    }
}

#[test]
fn a_stale_primary_size_does_not_matter() {
    for stale in [true, false] {
        let (file, gain_map) = ultra_hdr(Shape { stale, ..HAWK });
        assert_eq!(metadata_bytes(&file).gain_map, Some(gain_map), "stale: {stale}");
    }
}

#[test]
fn an_android_secondary_of_type_0_is_the_gain_map_only_when_the_primary_declares_one() {
    let (file, gain_map) = ultra_hdr(Shape { second: 0, ..HAWK });
    assert_eq!(metadata_bytes(&file).gain_map, Some(gain_map));

    // A camera's MPF with a secondary image and no gain-map declaration: a preview, not a gain map.
    let (file, _) = ultra_hdr(Shape {
        second: 0,
        declared: false,
        ..HAWK
    });
    assert_eq!(metadata_bytes(&file).gain_map, None);
    // A Gain Map entry needs no declaration.
    let (file, gain_map) = ultra_hdr(Shape {
        declared: false,
        ..HAWK
    });
    assert_eq!(metadata_bytes(&file).gain_map, Some(gain_map));
}

#[test]
fn a_gain_map_entry_after_another_secondary_is_found_and_a_third_is_not_looked_at() {
    // A depth map (type 0x040000, say) first, then the gain map.
    let (file, gain_map) = assemble(HAWK, &[(0x04_0000, false), (0x05_0000, true)]);
    assert_eq!(metadata_bytes(&file).gain_map, Some(gain_map));

    // At most two secondary entries are examined.
    let (file, _) = assemble(HAWK, &[(0x04_0000, false), (0x04_0000, false), (0x05_0000, true)]);
    assert_eq!(metadata_bytes(&file).gain_map, None);
}

/// `file` with the gain map's MP Entry rewritten to `size` bytes at `offset`.
fn with_entry(file: &[u8], size: u32, offset: u32) -> Vec<u8> {
    let at = find(file, b"MPF\0") + 4 + 50 + 16;
    let mut out = file.to_vec();
    out[at + 4..at + 8].copy_from_slice(&size.to_be_bytes());
    out[at + 8..at + 12].copy_from_slice(&offset.to_be_bytes());
    out
}

#[test]
fn a_gain_map_that_is_not_where_the_index_says_is_left_out_and_the_rest_kept() {
    let (file, gain_map) = ultra_hdr(HAWK);
    let tiff_at = u32::try_from(find(&file, b"MPF\0") + 4).unwrap();
    let start = u32::try_from(file.len() - gain_map.len()).unwrap() - tiff_at;
    let size = u32::try_from(gain_map.len()).unwrap();

    for (why, hostile) in [
        ("past the end", with_entry(&file, size, start + 1)),
        ("size past the end", with_entry(&file, size + 1, start)),
        ("far past the end", with_entry(&file, u32::MAX, u32::MAX)),
        ("offset zero, the primary", with_entry(&file, size, 0)),
        ("inside the index", with_entry(&file, size, 4)),
        ("not at an SOI", with_entry(&file, size - 1, start + 1)),
        ("truncated, no EOI", file[..file.len() - 2].to_vec()),
        ("truncated mid-image", file[..file.len() - gain_map.len() / 2].to_vec()),
    ] {
        let read = metadata_bytes(&hostile);
        assert_eq!(read.gain_map, None, "{why}");
        assert_eq!(read.iso_gain_map.as_deref(), Some(VERSION), "{why}");
        assert_eq!(read.xmp.as_deref(), Some(PRIMARY_XMP), "{why}");
    }

    // Padding after the gain map's `EOI` that the size covers is cut off.
    let mut padded = file.clone();
    padded.extend([0; 7]);
    let padded = with_entry(&padded, size + 7, start);
    assert_eq!(metadata_bytes(&padded).gain_map, Some(gain_map));
}

#[test]
fn a_gain_map_without_an_index_or_a_file_without_soi_gives_none() {
    let (file, _) = ultra_hdr(HAWK);
    assert_eq!(metadata_bytes(&file[1..]), Metadata::default());
    // The ISO marker and an appended JPEG with no MP Index: nothing locates it.
    let mut primary = Vec::new();
    encode_writer(&primary_pixels(), &mut primary, ImageFormat::Jpeg, None).unwrap();
    let mut file = with_segment(&primary, 0xE2, &[ISO_URN, VERSION].concat());
    file.extend(gain_map_jpeg());
    assert_eq!(metadata_bytes(&file).gain_map, None);
    assert_eq!(metadata_bytes(&file).iso_gain_map.as_deref(), Some(VERSION));
}

#[test]
fn truncations_and_byte_flips_of_a_gain_map_jpeg_never_panic() {
    let (file, _) = ultra_hdr(Shape { big: false, ..HAWK });
    for len in 0..file.len() {
        let _ = metadata_bytes(&file[..len]);
    }
    let mut bytes = file.clone();
    for at in 0..bytes.len() {
        for value in [bytes[at] ^ 0xFF, 0x00, 0xFF, bytes[at] ^ 0x01] {
            let original = std::mem::replace(&mut bytes[at], value);
            let _ = metadata_bytes(&bytes);
            bytes[at] = original;
        }
    }
}

// ── Writing ─────────────────────────────────────────────────────────────────────────────────────────────────────

/// The metadata of an Ultra HDR export: the primary's XMP and version marker, and the gain map.
fn ultra_hdr_metadata() -> Metadata {
    Metadata {
        xmp: Some(PRIMARY_XMP.to_vec()),
        iso_gain_map: Some(VERSION.to_vec()),
        gain_map: Some(gain_map_jpeg()),
        ..Metadata::default()
    }
}

fn encode(metadata: &Metadata, format: ImageFormat) -> (Vec<u8>, Embedded) {
    let mut bytes = Vec::new();
    let embedded = encode_writer_with(&primary_pixels(), &mut bytes, format, None, metadata).unwrap();
    (bytes, embedded)
}

#[test]
fn a_jpeg_with_a_gain_map_reads_back_both_fields_byte_for_byte() {
    let metadata = ultra_hdr_metadata();

    let (file, embedded) = encode(&metadata, ImageFormat::Jpeg);

    assert!(
        embedded.xmp && embedded.iso_gain_map && embedded.gain_map,
        "{embedded:?}"
    );
    let read = metadata_bytes(&file);
    assert_eq!(read.gain_map, metadata.gain_map);
    assert_eq!(read.iso_gain_map, metadata.iso_gain_map);
    assert_eq!(read.xmp, metadata.xmp);
    assert_eq!(hdr_bytes(&file), Some(Hdr::GainMap));
    assert_eq!(decode_bytes(&file).unwrap().width(), 32, "the primary still decodes");
}

#[test]
fn the_written_index_is_true_of_the_file() {
    let metadata = ultra_hdr_metadata();
    let gain_map = metadata.gain_map.clone().unwrap();
    let (file, _) = encode(&metadata, ImageFormat::Jpeg);

    let at = find(&file, b"MPF\0");
    let segment_len = usize::from(u16::from_be_bytes([file[at - 2], file[at - 1]]));
    assert_eq!(segment_len, 88);
    let entries: Vec<_> = mpf::entries(&file[at..at + segment_len - 2]).collect();
    let primary = file.len() - gain_map.len();

    assert_eq!(entries.len(), 2);
    assert_eq!((entries[0].attribute, entries[0].offset), (0x2003_0000, 0));
    assert_eq!(entries[0].size as usize, primary, "the primary's size, SOI to EOI");
    assert!(entries[1].is_gain_map());
    assert_eq!(entries[1].size as usize, gain_map.len());
    let start = at + 4 + entries[1].offset as usize;
    assert_eq!(start, primary, "the offset lands on the gain map's SOI");
    assert_eq!(&file[start..], &gain_map[..]);
    assert_eq!(&file[primary - 2..primary], &[0xFF, 0xD9]);
}

#[test]
fn the_segments_are_in_the_documented_order() {
    let mut metadata = ultra_hdr_metadata();
    metadata.exif = Some(b"MM\0\x2a\0\0\0\x08\0\0\0\0\0\0".to_vec());
    let mut profile = vec![0_u8; 300];
    profile[0..4].copy_from_slice(&300_u32.to_be_bytes());
    profile[36..40].copy_from_slice(b"acsp");
    metadata.icc_profile = Some(profile);
    let (file, _) = encode(&metadata, ImageFormat::Jpeg);

    let mut found = Vec::new();
    let mut at = 2;
    while file[at + 1] != 0xDA {
        let data = &file[at + 4..];
        let name = match file[at + 1] {
            0xE0 => "JFIF",
            0xE1 if data.starts_with(b"Exif") => "EXIF",
            0xE1 => "XMP",
            0xE2 if data.starts_with(ISO_URN) => "ISO",
            0xE2 if data.starts_with(b"MPF") => "MPF",
            0xE2 => "ICC",
            _ => "other",
        };
        if name != "other" {
            found.push(name);
        }
        at += 2 + usize::from(u16::from_be_bytes([file[at + 2], file[at + 3]]));
    }
    assert_eq!(found, ["JFIF", "EXIF", "XMP", "ISO", "MPF", "ICC"]);
}

#[test]
fn a_gain_map_that_is_not_a_whole_jpeg_or_an_oversized_iso_payload_is_left_out() {
    for broken in [b"not a jpeg".to_vec(), {
        let mut cut = gain_map_jpeg();
        cut.truncate(cut.len() - 1);
        cut
    }] {
        let mut metadata = ultra_hdr_metadata();
        metadata.gain_map = Some(broken);
        let (file, embedded) = encode(&metadata, ImageFormat::Jpeg);
        assert!(!embedded.gain_map && embedded.iso_gain_map);
        assert!(!file.windows(4).any(|w| w == b"MPF\0"), "no index without a gain map");
        assert_eq!(metadata_bytes(&file).gain_map, None);
    }

    let mut metadata = Metadata {
        iso_gain_map: Some(vec![7; 65_506]),
        ..Metadata::default()
    };
    let (file, embedded) = encode(&metadata, ImageFormat::Jpeg);
    assert!(!embedded.iso_gain_map);
    assert_eq!(metadata_bytes(&file).iso_gain_map, None);
    metadata.iso_gain_map = Some(vec![7; 65_505]);
    assert!(
        encode(&metadata, ImageFormat::Jpeg).1.iso_gain_map,
        "the largest that fits one segment"
    );
}

#[test]
fn every_other_format_leaves_both_out() {
    let metadata = Metadata {
        iso_gain_map: Some(VERSION.to_vec()),
        gain_map: Some(gain_map_jpeg()),
        ..Metadata::default()
    };
    for format in [ImageFormat::Png, ImageFormat::WebP, ImageFormat::Tiff, ImageFormat::Bmp] {
        let (with, embedded) = encode(&metadata, format);
        let (plain, _) = encode(&Metadata::default(), format);
        assert!(!embedded.iso_gain_map && !embedded.gain_map, "{format:?}");
        assert_eq!(with, plain, "{format:?}");
    }
}

#[test]
fn ultrahdr_app_recognises_the_file_as_ultra_hdr() {
    // `libultrahdr`'s reference tool, as an independent reader. Skipped, saying so, where it is not installed
    // (`brew install libultrahdr`); it is not a build dependency.
    let Ok(found) = std::process::Command::new("ultrahdr_app").arg("-h").output() else {
        eprintln!("skipped: ultrahdr_app is not on PATH");
        return;
    };
    drop(found);
    let (file, _) = encode(&ultra_hdr_metadata(), ImageFormat::Jpeg);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ultra.jpg");
    std::fs::write(&path, &file).unwrap();

    let output = std::process::Command::new("ultrahdr_app")
        .args(["-m", "1", "-j"])
        .arg(&path)
        .arg("-P")
        .current_dir(dir.path())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Ultra HDR Image: Yes"),
        "{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
