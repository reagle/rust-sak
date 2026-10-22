//! The EXIF primitives against a synthetic block laid out byte by byte, in both byte orders, with a MakerNote whose
//! internal pointer is absolute from the start of the TIFF stream — the kind a move corrupts.

use std::ops::Range;

use super::model::{Model, Order};
use super::{ExifEdit, ExifError, ExifIfd, ExifValue, exif_field, patch_exif, rebuild_exif};

const SOFTWARE: u16 = 0x0131;
const DATE_TIME: u16 = 0x0132;
const ORIENTATION: u16 = 0x0112;
const IMAGE_WIDTH: u16 = 0x0100;
const ARTIST: u16 = 0x013B;
const OFFSET_TIME: u16 = 0x9010;
const PIXEL_X: u16 = 0xA002;
const PIXEL_Y: u16 = 0xA003;
const MAKER_NOTE: u16 = 0x927C;

/// The synthetic block and where its parts are.
struct Sample {
    exif: Vec<u8>,
    /// The MakerNote's bytes.
    maker_note: Range<usize>,
    /// IFD1's table and the thumbnail it points to, which end the block.
    thumbnail: Range<usize>,
    /// The DateTime value.
    date_time: Range<usize>,
}

const SECRET: &[u8; 8] = b"SECRET!!";
const DATE: &[u8; 20] = b"2026:10:18 16:03:50\0";

/// Header, IFD0 (8 entries), its values, the Exif IFD (6 entries) and its values including the MakerNote, the Interop
/// IFD, the GPS IFD and its value, IFD1 and a 100-byte thumbnail last. Offsets are fixed and written out in full, so
/// the test does not depend on the code it tests to lay anything out.
fn sample(order: Order) -> Sample {
    let mut b = vec![0_u8; 514];
    let put = |b: &mut Vec<u8>, at: usize, bytes: &[u8]| b[at..at + bytes.len()].copy_from_slice(bytes);
    let table = |b: &mut Vec<u8>, at: usize, entries: &[(u16, u16, u32, [u8; 4])], next: u32| {
        put(b, at, &order.u16_bytes(u16::try_from(entries.len()).unwrap()));
        for (index, (tag, kind, count, raw)) in entries.iter().enumerate() {
            let entry = at + 2 + 12 * index;
            put(b, entry, &order.u16_bytes(*tag));
            put(b, entry + 2, &order.u16_bytes(*kind));
            put(b, entry + 4, &order.u32_bytes(*count));
            put(b, entry + 8, raw);
        }
        put(b, at + 2 + 12 * entries.len(), &order.u32_bytes(next));
    };
    let at = |offset: u32| order.u32_bytes(offset);
    let short = |value: u16| {
        let mut raw = [0; 4];
        raw[..2].copy_from_slice(&order.u16_bytes(value));
        raw
    };
    let longs = |values: &[u32]| {
        values
            .iter()
            .flat_map(|value| order.u32_bytes(*value))
            .collect::<Vec<u8>>()
    };

    put(
        &mut b,
        0,
        if order == Order::Little {
            b"II\x2a\0"
        } else {
            b"MM\0\x2a"
        },
    );
    put(&mut b, 4, &order.u32_bytes(8));

    // IFD0 at 8, 8 entries: ends at 110. Next IFD: IFD1 at 372.
    table(
        &mut b,
        8,
        &[
            (0x010F, 2, 6, at(110)),       // Make "Canon"
            (0x0110, 2, 9, at(116)),       // Model "EOS Test"
            (ORIENTATION, 3, 1, short(6)), // Orientation
            (0x011A, 5, 1, at(126)),       // XResolution
            (SOFTWARE, 2, 14, at(134)),    // Software "Camera FW 1.0"
            (DATE_TIME, 2, 20, at(148)),   // DateTime
            (0x8769, 4, 1, at(168)),       // Exif IFD
            (0x8825, 4, 1, at(318)),       // GPS IFD
        ],
        372,
    );
    put(&mut b, 110, b"Canon\0");
    put(&mut b, 116, b"EOS Test\0");
    put(&mut b, 126, &longs(&[240, 1]));
    put(&mut b, 134, b"Camera FW 1.0\0");
    put(&mut b, 148, DATE);

    // Exif IFD at 168, 6 entries: ends at 246.
    table(
        &mut b,
        168,
        &[
            (0x829A, 5, 1, at(246)),      // ExposureTime
            (0x9003, 2, 20, at(254)),     // DateTimeOriginal
            (MAKER_NOTE, 7, 26, at(274)), // MakerNote
            (PIXEL_X, 3, 1, short(4000)), // PixelXDimension
            (PIXEL_Y, 3, 1, short(3000)), // PixelYDimension
            (0xA005, 4, 1, at(300)),      // Interop IFD
        ],
        0,
    );
    put(&mut b, 246, &longs(&[1, 1600]));
    put(&mut b, 254, DATE);
    // The MakerNote at 274: a Canon-style IFD of one entry whose value is at an *absolute* offset, 292.
    table(&mut b, 274, &[(0x0001, 7, 8, at(292))], 0);
    put(&mut b, 292, SECRET);

    // Interop IFD at 300: InteropIndex "R98", inline.
    table(&mut b, 300, &[(0x0001, 2, 4, *b"R98\0")], 0);

    // GPS IFD at 318: GPSLatitudeRef "N" inline, GPSLatitude three RATIONALs at 348.
    table(&mut b, 318, &[(0x0001, 2, 2, *b"N\0\0\0"), (0x0002, 5, 3, at(348))], 0);
    put(&mut b, 348, &longs(&[42, 1, 22, 1, 12, 1]));

    // IFD1 at 372: Compression 6, thumbnail at 414 for 100 bytes.
    table(
        &mut b,
        372,
        &[
            (0x0103, 3, 1, short(6)),
            (0x0201, 4, 1, at(414)),
            (0x0202, 4, 1, at(100)),
        ],
        0,
    );
    let mut thumbnail = vec![0xAB; 100];
    thumbnail[..2].copy_from_slice(&[0xFF, 0xD8]);
    thumbnail[98..].copy_from_slice(&[0xFF, 0xD9]);
    put(&mut b, 414, &thumbnail);

    Sample {
        exif: b,
        maker_note: 274..300,
        thumbnail: 372..514,
        date_time: 148..168,
    }
}

const ORDERS: [Order; 2] = [Order::Little, Order::Big];

fn ascii(text: &str) -> ExifValue {
    ExifValue::Ascii(text.to_owned())
}

/// The MakerNote's own pointer, read from wherever the MakerNote now is, resolved against the start of the block.
fn resolve_secret(exif: &[u8], order: Order) -> Option<&[u8]> {
    let maker_note = Model::parse(exif).ok()?.exif?.find(MAKER_NOTE)?.range(order, exif)?;
    let pointer = order.u32(exif, maker_note.start + 2 + 8)? as usize;
    exif.get(pointer..pointer + 8)
}

/// Asserts that the MakerNote is byte for byte where it was and that its absolute pointer still finds its data.
fn assert_maker_note_kept(sample: &Sample, patched: &[u8], order: Order) {
    assert_eq!(
        &patched[sample.maker_note.clone()],
        &sample.exif[sample.maker_note.clone()],
        "the MakerNote moved"
    );
    assert_eq!(
        resolve_secret(patched, order),
        Some(&SECRET[..]),
        "the MakerNote's pointer no longer resolves"
    );
}

// ── Reading ─────────────────────────────────────────────────────────────────────────────────────────────────────

#[test]
fn fields_are_read_from_every_ifd_in_both_byte_orders() {
    for order in ORDERS {
        let exif = sample(order).exif;
        assert_eq!(
            exif_field(&exif, ExifIfd::Primary, 0x010F),
            Some(ascii("Canon")),
            "{order:?}"
        );
        assert_eq!(
            exif_field(&exif, ExifIfd::Primary, ORIENTATION),
            Some(ExifValue::Short(6))
        );
        assert_eq!(
            exif_field(&exif, ExifIfd::Primary, SOFTWARE),
            Some(ascii("Camera FW 1.0"))
        );
        assert_eq!(
            exif_field(&exif, ExifIfd::Exif, 0x9003),
            Some(ascii("2026:10:18 16:03:50"))
        );
        assert_eq!(exif_field(&exif, ExifIfd::Exif, PIXEL_X), Some(ExifValue::Short(4000)));
        assert_eq!(exif_field(&exif, ExifIfd::Interop, 0x0001), Some(ascii("R98")));
        assert_eq!(exif_field(&exif, ExifIfd::Gps, 0x0001), Some(ascii("N")));
        assert_eq!(exif_field(&exif, ExifIfd::Primary, 0x8769), Some(ExifValue::Long(168)));
        // A RATIONAL is not one of the three types read.
        assert_eq!(exif_field(&exif, ExifIfd::Exif, 0x829A), None);
        assert_eq!(exif_field(&exif, ExifIfd::Primary, ARTIST), None);
        assert_eq!(resolve_secret(&exif, order), Some(&SECRET[..]));
    }
}

#[test]
fn the_fixtures_exiftool_wrote_are_read() {
    let exif = include_bytes!("../blocks/fixtures/meta.jpg.exif");
    assert_eq!(exif_field(exif, ExifIfd::Primary, 0x010F), Some(ascii("FixtureCam")));
    assert_eq!(
        exif_field(exif, ExifIfd::Primary, ORIENTATION),
        Some(ExifValue::Short(6))
    );
    assert_eq!(exif_field(exif, ExifIfd::Exif, 0x9011), Some(ascii("-04:00")));
    assert_eq!(exif_field(exif, ExifIfd::Gps, 0x0001), Some(ascii("N")));
}

// ── Patching ────────────────────────────────────────────────────────────────────────────────────────────────────

#[test]
fn a_value_of_the_same_length_is_overwritten_where_it_is() {
    for order in ORDERS {
        let sample = sample(order);
        let edits = [ExifEdit::Set {
            ifd: ExifIfd::Primary,
            tag: DATE_TIME,
            value: ascii("2026:10:22 09:30:00"),
        }];
        let patched = patch_exif(&sample.exif, &edits).unwrap();

        assert_eq!(patched.len(), sample.exif.len());
        assert_eq!(&patched[sample.date_time.clone()], b"2026:10:22 09:30:00\0");
        // Nothing else changed at all.
        for (at, (before, after)) in sample.exif.iter().zip(&patched).enumerate() {
            if !sample.date_time.contains(&at) {
                assert_eq!(before, after, "byte {at} changed ({order:?})");
            }
        }
    }
}

#[test]
fn a_longer_value_is_put_elsewhere_and_its_old_bytes_zeroed() {
    for order in ORDERS {
        let sample = sample(order);
        let software = "Open Photo AI 26.10.0 (2026-10-22)";
        let edits = [ExifEdit::Set {
            ifd: ExifIfd::Primary,
            tag: SOFTWARE,
            value: ascii(software),
        }];
        let patched = patch_exif(&sample.exif, &edits).unwrap();

        assert_eq!(exif_field(&patched, ExifIfd::Primary, SOFTWARE), Some(ascii(software)));
        assert_eq!(&patched[134..148], &[0; 14], "the old value was left behind");
        assert_maker_note_kept(&sample, &patched, order);
        assert_eq!(exif_field(&patched, ExifIfd::Gps, 0x0001), Some(ascii("N")));
    }
}

#[test]
fn a_shorter_value_is_written_over_the_old_one() {
    for order in ORDERS {
        let sample = sample(order);
        let edits = [ExifEdit::Set {
            ifd: ExifIfd::Primary,
            tag: SOFTWARE,
            value: ascii("OPAI"),
        }];
        let patched = patch_exif(&sample.exif, &edits).unwrap();

        // Five bytes: too long to go inline, short enough for the old slot.
        assert_eq!(exif_field(&patched, ExifIfd::Primary, SOFTWARE), Some(ascii("OPAI")));
        assert_eq!(&patched[134..139], b"OPAI\0");
        assert_eq!(&patched[139..148], &[0; 9]);
        assert_eq!(patched.len(), sample.exif.len());
    }
}

#[test]
fn a_short_dimension_becomes_long_in_its_own_slot() {
    for order in ORDERS {
        let sample = sample(order);
        let edits = [
            ExifEdit::Set {
                ifd: ExifIfd::Exif,
                tag: PIXEL_X,
                value: ExifValue::Long(70_000),
            },
            ExifEdit::Set {
                ifd: ExifIfd::Exif,
                tag: PIXEL_Y,
                value: ExifValue::Short(500),
            },
        ];
        let patched = patch_exif(&sample.exif, &edits).unwrap();

        assert_eq!(
            exif_field(&patched, ExifIfd::Exif, PIXEL_X),
            Some(ExifValue::Long(70_000))
        );
        assert_eq!(
            exif_field(&patched, ExifIfd::Exif, PIXEL_Y),
            Some(ExifValue::Short(500))
        );
        // The table did not move.
        assert_eq!(
            exif_field(&patched, ExifIfd::Primary, 0x8769),
            Some(ExifValue::Long(168))
        );
        assert_eq!(patched.len(), sample.exif.len());
        assert_maker_note_kept(&sample, &patched, order);
    }
}

#[test]
fn an_added_entry_moves_its_table_and_the_parent_follows() {
    for order in ORDERS {
        let sample = sample(order);
        let edits = [
            ExifEdit::Set {
                ifd: ExifIfd::Exif,
                tag: OFFSET_TIME,
                value: ascii("-04:00"),
            },
            ExifEdit::Set {
                ifd: ExifIfd::Primary,
                tag: ARTIST,
                value: ascii("Joseph"),
            },
        ];
        let patched = patch_exif(&sample.exif, &edits).unwrap();

        assert_eq!(exif_field(&patched, ExifIfd::Exif, OFFSET_TIME), Some(ascii("-04:00")));
        assert_eq!(exif_field(&patched, ExifIfd::Primary, ARTIST), Some(ascii("Joseph")));
        // Both tables moved; everything they held is still there, and so is everything below them.
        assert_ne!(
            exif_field(&patched, ExifIfd::Primary, 0x8769),
            Some(ExifValue::Long(168))
        );
        assert_ne!(order.u32(&patched, 4), Some(8));
        for (ifd, tag, value) in [
            (ExifIfd::Primary, 0x010F, ascii("Canon")),
            (ExifIfd::Primary, ORIENTATION, ExifValue::Short(6)),
            (ExifIfd::Exif, PIXEL_Y, ExifValue::Short(3000)),
            (ExifIfd::Interop, 0x0001, ascii("R98")),
            (ExifIfd::Gps, 0x0001, ascii("N")),
        ] {
            assert_eq!(
                exif_field(&patched, ifd, tag),
                Some(value),
                "{ifd:?} {tag:#x} ({order:?})"
            );
        }
        assert_maker_note_kept(&sample, &patched, order);

        // The moved tables are in tag order, as TIFF requires.
        let model = Model::parse(&patched).unwrap();
        for table in [&model.primary, model.exif.as_ref().unwrap()] {
            assert!(table.entries.windows(2).all(|pair| pair[0].tag < pair[1].tag));
        }
    }
}

#[test]
fn the_thumbnail_is_detached_zeroed_and_trimmed_and_its_space_reused() {
    for order in ORDERS {
        let sample = sample(order);
        let patched = patch_exif(&sample.exif, &[ExifEdit::DropThumbnail]).unwrap();

        // IFD1 and the thumbnail ended the block, so the block now ends where the GPS value does.
        assert_eq!(patched.len(), sample.thumbnail.start);
        assert_eq!(Model::parse(&patched).unwrap().primary.next, 0);
        assert_maker_note_kept(&sample, &patched, order);

        // With a value to append as well, the freed space is what it goes into: the block does not grow past it.
        let edits = [
            ExifEdit::DropThumbnail,
            ExifEdit::Set {
                ifd: ExifIfd::Primary,
                tag: SOFTWARE,
                value: ascii("Open Photo AI 26.10.0 (2026-10-22)"),
            },
        ];
        let patched = patch_exif(&sample.exif, &edits).unwrap();
        assert!(patched.len() < sample.exif.len());
        assert!(
            !patched.windows(2).any(|pair| pair == [0xFF, 0xD8]),
            "thumbnail bytes are left"
        );
        assert_maker_note_kept(&sample, &patched, order);
    }
}

#[test]
fn freed_space_in_the_middle_is_reused_before_the_block_grows() {
    for order in ORDERS {
        let sample = sample(order);
        // Software outgrows its slot and Copyright is added, so IFD0's table moves too: the old table and the old
        // Software value are freed, and both new values land in that freed space rather than past the end.
        let edits = [
            ExifEdit::Set {
                ifd: ExifIfd::Primary,
                tag: SOFTWARE,
                value: ascii("a much longer software name"),
            },
            ExifEdit::Set {
                ifd: ExifIfd::Primary,
                tag: 0x8298,
                value: ascii("(c) Example"),
            },
        ];
        let patched = patch_exif(&sample.exif, &edits).unwrap();
        let model = Model::parse(&patched).unwrap();
        for tag in [SOFTWARE, 0x8298] {
            let range = model.primary.find(tag).unwrap().range(order, &patched).unwrap();
            assert!(range.end <= 110, "{tag:#x} at {range:?} ({order:?})");
        }
        assert_eq!(
            exif_field(&patched, ExifIfd::Primary, 0x8298),
            Some(ascii("(c) Example"))
        );
        assert_eq!(
            exif_field(&patched, ExifIfd::Primary, SOFTWARE),
            Some(ascii("a much longer software name"))
        );
        assert_maker_note_kept(&sample, &patched, order);
    }
}

#[test]
fn replace_leaves_an_absent_tag_absent_and_set_skips_an_absent_ifd() {
    for order in ORDERS {
        let sample = sample(order);
        let edits = [
            ExifEdit::Replace {
                ifd: ExifIfd::Primary,
                tag: IMAGE_WIDTH,
                value: ExifValue::Long(10),
            },
            ExifEdit::Set {
                ifd: ExifIfd::Gps,
                tag: 0x0003,
                value: ascii("W"),
            },
        ];
        let patched = patch_exif(&sample.exif, &edits).unwrap();
        assert_eq!(exif_field(&patched, ExifIfd::Primary, IMAGE_WIDTH), None);
        assert_eq!(exif_field(&patched, ExifIfd::Gps, 0x0003), Some(ascii("W")));
    }

    // A block with no GPS IFD: setting a GPS tag does nothing at all.
    let minimal = b"MM\0\x2a\0\0\0\x08\0\x01\x01\x12\0\x03\0\0\0\x01\0\x06\0\0\0\0\0\0";
    let edits = [ExifEdit::Set {
        ifd: ExifIfd::Gps,
        tag: 0x0001,
        value: ascii("N"),
    }];
    assert_eq!(patch_exif(minimal, &edits).unwrap(), minimal.to_vec());
}

#[test]
fn the_last_edit_of_a_tag_wins() {
    let sample = sample(Order::Big);
    let edits = [
        ExifEdit::Set {
            ifd: ExifIfd::Primary,
            tag: SOFTWARE,
            value: ascii("first, and long enough to move"),
        },
        ExifEdit::Set {
            ifd: ExifIfd::Primary,
            tag: SOFTWARE,
            value: ascii("second"),
        },
    ];
    let patched = patch_exif(&sample.exif, &edits).unwrap();
    assert_eq!(exif_field(&patched, ExifIfd::Primary, SOFTWARE), Some(ascii("second")));
    assert_eq!(patched.len(), sample.exif.len());
}

#[test]
fn a_value_another_entry_shares_is_neither_overwritten_nor_zeroed() {
    // Point Model at Make's six bytes too, then shorten Make: it must move rather than overwrite what Model reads.
    let mut exif = sample(Order::Little).exif;
    exif[8 + 2 + 12 + 4..8 + 2 + 12 + 12].copy_from_slice(&[6, 0, 0, 0, 110, 0, 0, 0]);
    let edits = [ExifEdit::Set {
        ifd: ExifIfd::Primary,
        tag: 0x010F,
        value: ascii("Sony"),
    }];
    let patched = patch_exif(&exif, &edits).unwrap();
    assert_eq!(exif_field(&patched, ExifIfd::Primary, 0x010F), Some(ascii("Sony")));
    assert_eq!(exif_field(&patched, ExifIfd::Primary, 0x0110), Some(ascii("Canon")));

    let edits = [ExifEdit::Set {
        ifd: ExifIfd::Primary,
        tag: 0x010F,
        value: ascii("Nikon Corp."),
    }];
    let patched = patch_exif(&exif, &edits).unwrap();
    assert_eq!(
        exif_field(&patched, ExifIfd::Primary, 0x0110),
        Some(ascii("Canon")),
        "a shared value was zeroed"
    );
}

#[test]
fn two_edited_entries_sharing_a_value_do_not_share_their_new_space() {
    // Model points at Make's bytes; both grow, so both are allocated, from the one released range at most once.
    let mut exif = sample(Order::Little).exif;
    exif[8 + 2 + 12 + 4..8 + 2 + 12 + 12].copy_from_slice(&[6, 0, 0, 0, 110, 0, 0, 0]);
    let edits = [
        ExifEdit::Set {
            ifd: ExifIfd::Primary,
            tag: 0x010F,
            value: ascii("Maker"),
        },
        ExifEdit::Set {
            ifd: ExifIfd::Primary,
            tag: 0x0110,
            value: ascii("Model"),
        },
        ExifEdit::Set {
            ifd: ExifIfd::Primary,
            tag: SOFTWARE,
            value: ascii("Soft"),
        },
    ];
    let patched = patch_exif(&exif, &edits).unwrap();
    assert_eq!(exif_field(&patched, ExifIfd::Primary, 0x010F), Some(ascii("Maker")));
    assert_eq!(exif_field(&patched, ExifIfd::Primary, 0x0110), Some(ascii("Model")));
    assert_eq!(exif_field(&patched, ExifIfd::Primary, SOFTWARE), Some(ascii("Soft")));
}

#[test]
fn an_entry_of_an_unknown_type_turns_off_zeroing_and_trimming() {
    let mut exif = sample(Order::Little).exif;
    // Give the Interop entry type 99, whose size nothing knows.
    exif[300 + 2 + 2..300 + 2 + 4].copy_from_slice(&99_u16.to_le_bytes());
    let patched = patch_exif(&exif, &[ExifEdit::DropThumbnail]).unwrap();
    assert_eq!(
        patched.len(),
        exif.len(),
        "trimmed despite an entry it cannot see the extent of"
    );
    assert_eq!(
        &patched[414..514],
        &exif[414..514],
        "zeroed despite an entry it cannot see the extent of"
    );
    assert_eq!(Model::parse(&patched).unwrap().primary.next, 0);
}

#[test]
fn pointer_tags_cannot_be_edited_and_malformed_blocks_are_refused() {
    let sample = sample(Order::Big);
    let edits = [ExifEdit::Set {
        ifd: ExifIfd::Primary,
        tag: 0x8769,
        value: ExifValue::Long(8),
    }];
    assert_eq!(patch_exif(&sample.exif, &edits), Err(ExifError::PointerTag(0x8769)));
    assert_eq!(rebuild_exif(&sample.exif, &edits), Err(ExifError::PointerTag(0x8769)));
    for bad in [
        &b""[..],
        b"MM\0\x2a\0\0\0\x08",
        b"XX\0\x2a\0\0\0\x08\0\0\0\0\0\0",
        b"II\x2a\0\x04\0\0\0\0\0\0\0\0\0",
    ] {
        assert_eq!(patch_exif(bad, &[]), Err(ExifError::Malformed));
        assert_eq!(rebuild_exif(bad, &[]), Err(ExifError::Malformed));
        assert_eq!(exif_field(bad, ExifIfd::Primary, ORIENTATION), None);
    }
}

#[test]
fn a_sub_ifd_pointing_outside_the_block_is_treated_as_absent() {
    let mut exif = sample(Order::Big).exif;
    // The GPS pointer, entry 7 of IFD0, now points past the end.
    exif[8 + 2 + 12 * 7 + 8..8 + 2 + 12 * 7 + 12].copy_from_slice(&9_999_u32.to_be_bytes());
    assert_eq!(exif_field(&exif, ExifIfd::Gps, 0x0001), None);
    let edits = [ExifEdit::Set {
        ifd: ExifIfd::Gps,
        tag: 0x0001,
        value: ascii("S"),
    }];
    let patched = patch_exif(&exif, &edits).unwrap();
    assert_eq!(patched, exif);
    // And the rebuild drops the dangling pointer rather than copying it.
    let rebuilt = rebuild_exif(&exif, &[]).unwrap().exif;
    assert_eq!(exif_field(&rebuilt, ExifIfd::Primary, 0x8825), None);
}

// ── Rebuilding ──────────────────────────────────────────────────────────────────────────────────────────────────

#[test]
fn a_rebuild_keeps_every_field_but_the_maker_note_and_thumbnail_and_applies_the_edits() {
    for order in ORDERS {
        let sample = sample(order);
        let edits = [
            ExifEdit::Set {
                ifd: ExifIfd::Primary,
                tag: SOFTWARE,
                value: ascii("Open Photo AI"),
            },
            ExifEdit::Set {
                ifd: ExifIfd::Exif,
                tag: OFFSET_TIME,
                value: ascii("+02:00"),
            },
            ExifEdit::Replace {
                ifd: ExifIfd::Primary,
                tag: IMAGE_WIDTH,
                value: ExifValue::Long(1),
            },
            ExifEdit::DropThumbnail,
        ];
        let rebuilt = rebuild_exif(&sample.exif, &edits).unwrap();

        assert!(rebuilt.maker_note_dropped);
        let exif = &rebuilt.exif;
        assert!(exif.len() < sample.thumbnail.start, "{} bytes", exif.len());
        assert_eq!(exif_field(exif, ExifIfd::Exif, MAKER_NOTE), None);
        assert_eq!(Model::parse(exif).unwrap().thumbnail, None);
        for (ifd, tag, value) in [
            (ExifIfd::Primary, 0x010F, ascii("Canon")),
            (ExifIfd::Primary, 0x0110, ascii("EOS Test")),
            (ExifIfd::Primary, ORIENTATION, ExifValue::Short(6)),
            (ExifIfd::Primary, SOFTWARE, ascii("Open Photo AI")),
            (ExifIfd::Primary, DATE_TIME, ascii("2026:10:18 16:03:50")),
            (ExifIfd::Exif, 0x9003, ascii("2026:10:18 16:03:50")),
            (ExifIfd::Exif, OFFSET_TIME, ascii("+02:00")),
            (ExifIfd::Exif, PIXEL_X, ExifValue::Short(4000)),
            (ExifIfd::Interop, 0x0001, ascii("R98")),
            (ExifIfd::Gps, 0x0001, ascii("N")),
        ] {
            assert_eq!(exif_field(exif, ifd, tag), Some(value), "{ifd:?} {tag:#x} ({order:?})");
        }
        assert_eq!(exif_field(exif, ExifIfd::Primary, IMAGE_WIDTH), None);
        // The RATIONAL values came along byte for byte.
        let model = Model::parse(exif).unwrap();
        let latitude = model
            .gps
            .as_ref()
            .unwrap()
            .find(0x0002)
            .unwrap()
            .value(order, exif)
            .unwrap();
        assert_eq!(latitude, &sample.exif[348..372]);
    }
}

#[test]
fn a_rebuild_of_a_block_without_a_maker_note_says_none_was_dropped() {
    let exif = include_bytes!("../blocks/fixtures/meta.jpg.exif");
    let rebuilt = rebuild_exif(exif, &[]).unwrap();
    assert!(!rebuilt.maker_note_dropped);
    assert_eq!(
        exif_field(&rebuilt.exif, ExifIfd::Primary, 0x010F),
        Some(ascii("FixtureCam"))
    );
    assert_eq!(exif_field(&rebuilt.exif, ExifIfd::Gps, 0x0001), Some(ascii("N")));
}

// ── Hostile input ───────────────────────────────────────────────────────────────────────────────────────────────

fn every_edit() -> Vec<ExifEdit> {
    vec![
        ExifEdit::DropThumbnail,
        ExifEdit::Set {
            ifd: ExifIfd::Primary,
            tag: SOFTWARE,
            value: ascii("Open Photo AI 26.10.0 (2026-10-22)"),
        },
        ExifEdit::Set {
            ifd: ExifIfd::Primary,
            tag: DATE_TIME,
            value: ascii("2026:10:22 09:30:00"),
        },
        ExifEdit::Set {
            ifd: ExifIfd::Exif,
            tag: OFFSET_TIME,
            value: ascii("-04:00"),
        },
        ExifEdit::Set {
            ifd: ExifIfd::Exif,
            tag: PIXEL_X,
            value: ExifValue::Long(100_000),
        },
        ExifEdit::Replace {
            ifd: ExifIfd::Primary,
            tag: IMAGE_WIDTH,
            value: ExifValue::Short(5),
        },
        ExifEdit::Replace {
            ifd: ExifIfd::Primary,
            tag: ORIENTATION,
            value: ExifValue::Short(1),
        },
    ]
}

fn exercise(block: &[u8]) {
    let edits = every_edit();
    let _ = exif_field(block, ExifIfd::Exif, PIXEL_X);
    if let Ok(patched) = patch_exif(block, &edits) {
        // What a patch produces must itself parse, and say what it was told to.
        assert!(Model::parse(&patched).is_ok());
        assert_eq!(
            exif_field(&patched, ExifIfd::Primary, DATE_TIME),
            Some(ascii("2026:10:22 09:30:00")),
            "{block:?}"
        );
    }
    if let Ok(rebuilt) = rebuild_exif(block, &edits) {
        assert!(Model::parse(&rebuilt.exif).is_ok());
    }
}

#[test]
fn truncations_and_byte_flips_never_panic() {
    let blocks: Vec<Vec<u8>> = vec![
        sample(Order::Little).exif,
        sample(Order::Big).exif,
        include_bytes!("../blocks/fixtures/meta.jpg.exif").to_vec(),
        include_bytes!("../blocks/fixtures/meta.png.exif").to_vec(),
        include_bytes!("../blocks/fixtures/meta.webp.exif").to_vec(),
    ];
    for block in &blocks {
        for len in 0..block.len() {
            exercise(&block[..len]);
        }
        let mut flipped = block.clone();
        for at in 0..flipped.len() {
            for value in [flipped[at] ^ 0xFF, 0x00, 0xFF, flipped[at] ^ 0x01, flipped[at] ^ 0x80] {
                let original = std::mem::replace(&mut flipped[at], value);
                exercise(&flipped);
                flipped[at] = original;
            }
        }
    }
}

#[test]
fn a_full_ifd_neither_panics_nor_overflows_its_count() {
    // IFD0 claims 65,535 entries and the block holds them all: adding one more must not overflow the count.
    let mut block = b"II\x2a\0\x08\0\0\0".to_vec();
    block.extend_from_slice(&u16::MAX.to_le_bytes());
    for tag in 1..=u16::MAX {
        block.extend_from_slice(&tag.to_le_bytes());
        block.extend_from_slice(&[1, 0, 1, 0, 0, 0, 0, 0, 0, 0]);
    }
    block.extend_from_slice(&[0; 4]);
    let edits = [ExifEdit::Set {
        ifd: ExifIfd::Primary,
        tag: 0,
        value: ascii("x"),
    }];
    let patched = patch_exif(&block, &edits).unwrap();
    assert_eq!(patched, block);
    let rebuilt = rebuild_exif(&block, &edits).unwrap();
    assert!(Model::parse(&rebuilt.exif).is_ok());
}
