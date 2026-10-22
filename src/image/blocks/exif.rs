//! The one check made of an EXIF block before it is carried: that it starts like a TIFF stream.

use super::MAX_BLOCK_LEN;

/// Whether `block` is plausibly an EXIF TIFF stream: a byte-order mark (`II` or `MM`) with the magic number 42 in that
/// order, a first-IFD offset past the header, and that IFD's entry count and entries inside the block. Nothing beyond
/// IFD0's own extent is read — the block is carried, not interpreted — but a block that fails this cannot be EXIF any
/// reader would accept, and is left out rather than written into a file.
pub(in crate::image) fn plausible(block: &[u8]) -> bool {
    if block.len() < 8 || block.len() > MAX_BLOCK_LEN {
        return false;
    }
    let read_u16 = |at: usize| -> Option<u16> {
        let bytes: [u8; 2] = block.get(at..at.checked_add(2)?)?.try_into().ok()?;
        Some(if block[0] == b'I' {
            u16::from_le_bytes(bytes)
        } else {
            u16::from_be_bytes(bytes)
        })
    };
    let read_u32 = |at: usize| -> Option<u32> {
        let bytes: [u8; 4] = block.get(at..at.checked_add(4)?)?.try_into().ok()?;
        Some(if block[0] == b'I' {
            u32::from_le_bytes(bytes)
        } else {
            u32::from_be_bytes(bytes)
        })
    };

    if &block[0..2] != b"II" && &block[0..2] != b"MM" {
        return false;
    }
    if read_u16(2) != Some(42) {
        return false;
    }
    let Some(ifd0) = read_u32(4).and_then(|offset| usize::try_from(offset).ok()) else {
        return false;
    };
    if ifd0 < 8 {
        return false;
    }
    let Some(count) = read_u16(ifd0) else {
        return false;
    };
    // The entries, twelve bytes each, and the four-byte next-IFD offset after them.
    ifd0.checked_add(2 + 12 * usize::from(count) + 4)
        .is_some_and(|end| end <= block.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal stream: header, then an IFD0 at 8 with one entry and no next IFD.
    fn minimal(order: &[u8; 2]) -> Vec<u8> {
        let little = order == b"II";
        let u16b = |v: u16| if little { v.to_le_bytes() } else { v.to_be_bytes() };
        let u32b = |v: u32| if little { v.to_le_bytes() } else { v.to_be_bytes() };
        let mut block = order.to_vec();
        block.extend(u16b(42));
        block.extend(u32b(8));
        block.extend(u16b(1));
        block.extend(u16b(0x0112)); // Orientation
        block.extend(u16b(3)); // SHORT
        block.extend(u32b(1));
        block.extend(u16b(1));
        block.extend([0, 0]);
        block.extend(u32b(0));
        block
    }

    #[test]
    fn both_byte_orders_are_plausible() {
        assert!(plausible(&minimal(b"II")));
        assert!(plausible(&minimal(b"MM")));
    }

    #[test]
    fn a_wrong_mark_magic_or_offset_is_not() {
        let mut wrong_mark = minimal(b"II");
        wrong_mark[0] = b'X';
        assert!(!plausible(&wrong_mark));

        let mut wrong_magic = minimal(b"MM");
        wrong_magic[3] = 43;
        assert!(!plausible(&wrong_magic));

        let mut inside_header = minimal(b"II");
        inside_header[4] = 4;
        assert!(!plausible(&inside_header));

        let mut beyond = minimal(b"II");
        beyond[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(!plausible(&beyond));
    }

    #[test]
    fn a_stream_cut_inside_ifd0_is_not() {
        let block = minimal(b"MM");
        for len in 0..block.len() {
            assert!(!plausible(&block[..len]), "{len}");
        }
    }
}
