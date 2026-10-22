/// What an encode embeds in the file beside the pixels, handed to [`encode_file_with`](super::encode_file_with) and
/// [`encode_writer_with`](super::encode_writer_with).
///
/// Every field is carried **byte for byte**: nothing here is parsed, validated against the pixels or converted. A
/// field left `None` writes nothing, so [`Metadata::default()`] encodes exactly as
/// [`encode_writer`](super::encode_writer) does.
///
/// `#[non_exhaustive]` so further blocks can join it without breaking a caller: build one with
/// [`Metadata::default()`] and assign the fields wanted.
///
/// ```
/// use rust_sak::image::Metadata;
///
/// let mut metadata = Metadata::default();
/// metadata.icc_profile = Some(vec![0; 128]);
/// assert!(!metadata.is_empty());
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Metadata {
    /// An ICC colour profile, as read by [`icc_profile_bytes`](super::icc_profile_bytes): the whole profile, header
    /// included. Embedded in JPEG (`APP2`, split across segments above 64 KiB), PNG (`iCCP`), TIFF (tag 34675) and
    /// WebP (`ICCP`); BMP, GIF, AVIF and HEIF cannot carry it yet.
    pub icc_profile: Option<Vec<u8>>,
    /// An EXIF block, as read by [`metadata_bytes`](super::metadata_bytes): the TIFF stream itself, from its `II`/`MM`
    /// byte-order mark on, **without** the `Exif\0\0` prefix a JPEG puts in front of it — the form PNG `eXIf`, WebP
    /// `EXIF` and the HEIF `Exif` item all use. Embedded in JPEG (`APP1`, at most 65,527 bytes), PNG (`eXIf`) and WebP
    /// (`EXIF`). Offsets inside it are relative to its own first byte, so it is valid in all three as it stands.
    pub exif: Option<Vec<u8>>,
    /// An XMP packet: one whole serialized `x:xmpmeta` document, normally wrapped in `<?xpacket?>` instructions, in
    /// UTF-8. A JPEG source's extended XMP has already been merged into it. Embedded in JPEG (`APP1`, re-split into
    /// standard and extended XMP above 65,502 bytes), PNG (`iTXt` with keyword `XML:com.adobe.xmp`) and WebP (`XMP `).
    pub xmp: Option<Vec<u8>>,
    /// A Photoshop image-resource block (a sequence of `8BIM` resources), as a JPEG carries it in `APP13`
    /// `Photoshop 3.0`. The IPTC-IIM data is its resource `0x0404`. Embedded in JPEG only (split across `APP13`
    /// segments above 65,519 bytes); no other format has a standard place for it.
    pub iptc: Option<Vec<u8>>,
}

impl Metadata {
    /// Whether there is nothing to embed.
    pub fn is_empty(&self) -> bool {
        self.icc_profile.is_none() && self.exif.is_none() && self.xmp.is_none() && self.iptc.is_none()
    }
}

/// What an encode actually embedded, returned by [`encode_file_with`](super::encode_file_with) and
/// [`encode_writer_with`](super::encode_writer_with): one flag per [`Metadata`] field.
///
/// A flag is `true` only when the block was given **and** written. A format that cannot hold a block, or a block too
/// large for the format, reports `false` and the encode still succeeds: metadata is never a reason to refuse a
/// picture.
///
/// `#[non_exhaustive]` for the same reason as [`Metadata`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Embedded {
    /// The [`Metadata::icc_profile`] was written into the file.
    pub icc_profile: bool,
    /// The [`Metadata::exif`] block was written into the file.
    pub exif: bool,
    /// The [`Metadata::xmp`] packet was written into the file.
    pub xmp: bool,
    /// The [`Metadata::iptc`] block was written into the file.
    pub iptc: bool,
}
