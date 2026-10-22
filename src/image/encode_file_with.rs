use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

use ::image::DynamicImage;

use super::dispatch::{encode_into, resolve_options};
use super::error::{ImageError, Result};
use super::{Embedded, EncodeOptions, ImageFormat, Metadata};

/// Encodes `image` with `metadata` embedded and writes it to `path`, selecting the format from the file extension, and
/// reports what was embedded.
///
/// [`encode_file`](super::encode_file) with a [`Metadata`] to carry; see
/// [`encode_writer_with`](super::encode_writer_with) for which formats hold what. A block the format cannot hold is
/// left out and reported `false` in the [`Embedded`], and the file is still written.
///
/// # Errors
///
/// Returns [`ImageError::UnknownExtension`] if the path has no recognized image extension,
/// [`ImageError::FormatMismatch`] if `options` is for a different format than the extension names,
/// [`ImageError::Io`](super::ImageError::Io) if the file cannot be written, and the codec's own error if encoding
/// fails. Never because of `metadata`.
pub fn encode_file_with(
    image: &DynamicImage,
    path: impl AsRef<Path>,
    options: Option<EncodeOptions>,
    metadata: &Metadata,
) -> Result<Embedded> {
    let path = path.as_ref();
    let format = ImageFormat::from_path(path).ok_or(ImageError::UnknownExtension)?;
    let options = resolve_options(format, options)?;

    let mut writer = BufWriter::new(File::create(path)?);
    let embedded = encode_into(image, options, metadata, &mut writer)?;

    // `BufWriter` swallows a write failure on drop, which would turn a full disk into a silently truncated image.
    std::io::Write::flush(&mut writer)?;

    Ok(embedded)
}
