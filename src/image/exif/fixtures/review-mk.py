import struct, sys, os
out = sys.argv[1]
def ifd(entries, nxt=0):
    b = struct.pack('<H', len(entries))
    for tag, typ, cnt, raw in entries:
        b += struct.pack('<HHI', tag, typ, cnt) + raw
    return b + struct.pack('<I', nxt)
L = lambda v: struct.pack('<I', v)
S = lambda v: struct.pack('<HH', v, 0)
def w(name, data): open(os.path.join(out, name), 'wb').write(data)

# a) SubIFDs (0x014A) pointing at an IFD that ends the block: its next=0 is trailing zeros.
hdr = b'II*\x00' + L(8)
# IFD0 at 8: Make inline, SubIFDs -> X, 2 entries => 8+2+24+4 = 38
sub_at = 38
sub = ifd([(0x0100, 3, 1, S(10)), (0x0101, 3, 1, S(10))], 0)
blk = hdr + ifd([(0x010F, 2, 4, b'Cam\x00'), (0x014A, 4, 1, L(sub_at))]) + sub
w('a-subifd.exif', blk)

# b) cautious: unknown type in IFD0, plus IFD1 thumbnail with a marker
ifd0_len = 2 + 12*2 + 4
ifd1_at = 8 + ifd0_len
thumb_at = ifd1_at + 2 + 24 + 4
thumb = b'\xff\xd8SECRET-ORIGINAL-THUMBNAIL' + b'\xff\xd9'
blk = hdr + ifd([(0x010F, 2, 4, b'Cam\x00'), (0x9999, 99, 1, L(0))], ifd1_at) + ifd([(0x0201, 4, 1, L(thumb_at)), (0x0202, 4, 1, L(len(thumb)))]) + thumb
w('b-cautious-thumb.exif', blk)

# c) Software value range overlapping the TIFF header (offset 0), IFD0 placed later.
ifd0_at = 64
pre = b'II*\x00' + L(ifd0_at) + b'\x00' * (ifd0_at - 8)
blk = pre + ifd([(0x010F, 2, 4, b'Cam\x00'), (0x0131, 2, 40, L(0))])
w('c-header-overlap.exif', blk)

# d) Exif IFD pointing at IFD0 (loop), interop pointing at Exif, huge counts
blk = hdr + ifd([(0x8769, 4, 1, L(8)), (0x0131, 12, 0xFFFFFFFF, L(0xFFFFFFF0)), (0x8825, 4, 1, L(0xFFFFFFFF))])
w('d-loop.exif', blk)
