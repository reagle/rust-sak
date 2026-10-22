use std::path::Path;

use ::image::DynamicImage;

use super::error::Result;
use super::{EncodeOptions, Metadata, encode_file_with};

/// Encodes `image` and writes it to `path`, selecting the format from the file extension.
///
/// The encoded bytes are streamed to the file as they are produced rather than accumulated in memory first, so peak
/// memory is the decoded image plus a buffer, not the decoded image plus a whole encoded copy of it.
///
/// `options` may be `None` (encode with the format's defaults) or `Some(_)`.
///
/// # Errors
///
/// Returns [`ImageError::UnknownExtension`](super::ImageError::UnknownExtension) if the path has no recognized image
/// extension, [`ImageError::FormatMismatch`](super::ImageError::FormatMismatch) if `options` is for a different format
/// than the extension names, [`ImageError::Io`](super::ImageError::Io) if the file cannot be written, and the codec's
/// own error if encoding fails.
///
/// Embeds nothing beside the pixels; [`encode_file_with`] is the same encode with a [`Metadata`] to embed.
pub fn encode_file(image: &DynamicImage, path: impl AsRef<Path>, options: Option<EncodeOptions>) -> Result<()> {
    encode_file_with(image, path, options, &Metadata::default()).map(drop)
}
