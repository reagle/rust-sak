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
}

impl Metadata {
    /// Whether there is nothing to embed.
    pub fn is_empty(&self) -> bool {
        self.icc_profile.is_none()
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
}
